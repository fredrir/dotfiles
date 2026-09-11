use super::{canaries, patterns, recipients, sops, vault};
use crate::context::Context;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};

const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_LIST: usize = 32 * 1024 * 1024;
const MAX_FINDINGS: usize = 10_000;
const MAX_FINDING_BYTES: usize = 8 * 1024 * 1024;

pub fn git(context: &Context, arguments: &[&str]) -> Result<Vec<u8>, String> {
    let mut command = context.command("git");
    command
        .arg("-C")
        .arg(&context.root)
        .args(arguments)
        .stdin(Stdio::null());
    sops::capture(&mut command, MAX_LIST, "git read").map(|v| v.to_vec())
}

fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

pub fn tracked_paths(context: &Context) -> Result<Vec<PathBuf>, String> {
    Ok(git(context, &["ls-files", "-z"])?
        .split(|b| *b == 0)
        .filter(|b| !b.is_empty())
        .map(path_from_bytes)
        .collect())
}

pub fn encrypted_paths(context: &Context) -> Result<Vec<PathBuf>, String> {
    Ok(tracked_paths(context)?
        .into_iter()
        .filter(|p| vault::kind_of(p) == vault::SecretKind::Encrypted)
        .collect())
}

#[derive(Clone)]
struct Blob {
    path: PathBuf,
    oid: String,
    inside: Option<bool>,
}

fn index(context: &Context) -> Result<Vec<Blob>, String> {
    let data = git(context, &["ls-files", "--stage", "-z"])?;
    let mut found = Vec::new();
    for entry in data.split(|b| *b == 0).filter(|b| !b.is_empty()) {
        let tab = entry
            .iter()
            .position(|b| *b == b'\t')
            .ok_or("invalid git index output")?;
        let metadata =
            std::str::from_utf8(&entry[..tab]).map_err(|_| "invalid git index metadata")?;
        let fields: Vec<_> = metadata.split_whitespace().collect();
        if fields.len() != 3 || fields[2] != "0" {
            return Err("unmerged index; resolve conflicts before scanning".into());
        }
        found.push(Blob {
            path: path_from_bytes(&entry[tab + 1..]),
            oid: fields[1].to_string(),
            inside: None,
        });
    }
    Ok(found)
}

fn history(context: &Context, revisions: &str) -> Result<Vec<Blob>, String> {
    let revisions: Vec<_> = revisions.split_whitespace().collect();
    if revisions.is_empty()
        || revisions.iter().any(|arg| {
            arg.starts_with('-')
                && !matches!(
                    *arg,
                    "--not" | "--remotes" | "--branches" | "--tags" | "--all"
                )
        })
    {
        return Err("--commits needs revisions, not git options".into());
    }
    let mut args = vec![
        "log",
        "--format=%x1e%T",
        "--raw",
        "-z",
        "--no-abbrev",
        "--no-renames",
        "--root",
        "--diff-filter=AMT",
        "--diff-merges=separate",
    ];
    args.extend(revisions);
    args.push("--");
    let data = git(context, &args)?;
    let fields: Vec<_> = data.split(|b| *b == 0).collect();
    let mut found = Vec::new();
    let mut seen = BTreeSet::new();
    let mut i = 0;
    let mut current_tree = String::new();
    let mut marker_tree = String::new();
    let mut directories: Vec<PathBuf> = Vec::new();
    while i < fields.len() {
        let meta = String::from_utf8_lossy(fields[i]);
        let meta = meta.trim_start_matches('\n');
        if let Some(tree) = meta.strip_prefix('\x1e') {
            current_tree = tree.trim().to_string();
            i += 1;
            continue;
        }
        if meta.starts_with(':') {
            if current_tree.is_empty() {
                return Err("git history did not identify its source tree".into());
            }
            if marker_tree != current_tree {
                let listing = git(
                    context,
                    &["ls-tree", "-r", "--name-only", "-z", &current_tree],
                )?;
                directories = listing
                    .split(|b| *b == 0)
                    .filter(|b| !b.is_empty())
                    .map(path_from_bytes)
                    .filter(|p| p.file_name().is_some_and(|n| n == ".secret"))
                    .filter_map(|p| p.parent().map(Path::to_path_buf))
                    .collect();
                marker_tree.clone_from(&current_tree);
            }
            let parts: Vec<_> = meta.split_whitespace().collect();
            if parts.len() != 5 || i + 1 >= fields.len() {
                return Err("invalid git history output".into());
            }
            let path = path_from_bytes(fields[i + 1]);
            let inside = directories
                .iter()
                .any(|directory| path.starts_with(directory));
            let blob = Blob {
                path,
                oid: parts[3].to_string(),
                inside: Some(inside),
            };
            if seen.insert((blob.oid.clone(), blob.path.clone(), inside)) {
                found.push(blob);
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    Ok(found)
}

fn batch(
    context: &Context,
    objects: &[Blob],
    mut visit: impl FnMut(&Blob, Option<Vec<u8>>) -> Result<(), String>,
) -> Result<(), String> {
    // Batched size queries avoid reading giant blobs, and data batches have a
    // fixed upper bound. No subprocess is started for an individual blob.
    for chunk in objects.chunks(4096) {
        crate::cancel::check()?;
        let mut requests = tempfile::tempfile().map_err(|e| e.to_string())?;
        for blob in chunk {
            writeln!(requests, "{}", blob.oid).map_err(|e| e.to_string())?;
        }
        requests.rewind().map_err(|e| e.to_string())?;
        let mut command = context.command("git");
        command
            .arg("-C")
            .arg(&context.root)
            .args(["cat-file", "--batch-check", "--buffer"])
            .stdin(requests.try_clone().map_err(|e| e.to_string())?);
        let metadata = sops::capture(&mut command, 4096 * 256, "git batch metadata")?;
        let text = std::str::from_utf8(&metadata).map_err(|_| "invalid git batch metadata")?;
        let rows: Vec<_> = text.lines().collect();
        if rows.len() != chunk.len() {
            return Err("incomplete git batch metadata".into());
        }
        let mut selected = Vec::new();
        requests
            .set_len(0)
            .and_then(|()| requests.rewind())
            .map_err(|e| e.to_string())?;
        for (blob, row) in chunk.iter().zip(rows) {
            let fields: Vec<_> = row.split_whitespace().collect();
            if fields.len() != 3 || fields[1] != "blob" {
                return Err("git could not read a requested blob".into());
            }
            let size = fields[2]
                .parse::<usize>()
                .map_err(|_| "invalid git blob size")?;
            if size > MAX_BYTES {
                visit(blob, None)?;
            } else {
                writeln!(requests, "{}", blob.oid).map_err(|e| e.to_string())?;
                selected.push((blob, size));
            }
        }
        let mut cursor = 0;
        while cursor < selected.len() {
            let start = cursor;
            let mut total = 0usize;
            while cursor < selected.len() && total + selected[cursor].1 <= 8 * 1024 * 1024 {
                total += selected[cursor].1;
                cursor += 1;
            }
            let batch = &selected[start..cursor];
            requests
                .set_len(0)
                .and_then(|()| requests.rewind())
                .map_err(|e| e.to_string())?;
            for (blob, _) in batch {
                writeln!(requests, "{}", blob.oid).map_err(|e| e.to_string())?;
            }
            requests.rewind().map_err(|e| e.to_string())?;
            let mut command = context.command("git");
            command
                .arg("-C")
                .arg(&context.root)
                .args(["cat-file", "--batch", "--buffer"])
                .stdin(requests.try_clone().map_err(|e| e.to_string())?);
            let data = sops::capture(&mut command, total + batch.len() * 128, "git batch read")?;
            let mut bytes: &[u8] = &data;
            for (blob, size) in batch {
                let newline = bytes
                    .iter()
                    .position(|b| *b == b'\n')
                    .ok_or("invalid git batch data")?;
                bytes = &bytes[newline + 1..];
                if bytes.len() < size + 1 {
                    return Err("incomplete git batch data".into());
                }
                visit(blob, Some(bytes[..*size].to_vec()))?;
                bytes = &bytes[size + 1..];
            }
        }
    }
    Ok(())
}

#[derive(Debug)]
struct Finding {
    tier: u8,
    label: String,
    path: PathBuf,
    line: usize,
}

fn encrypted(text: &str) -> bool {
    let document: serde_json::Value = match serde_json::from_str(text) {
        Ok(document) => document,
        Err(_) => match serde_yaml_ng::from_str(text) {
            Ok(document) => document,
            Err(_) => return false,
        },
    };
    let Some(metadata) = document.get("sops").and_then(serde_json::Value::as_object) else {
        return false;
    };
    metadata
        .get("mac")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|value| value.starts_with("ENC[AES256_GCM,"))
        && metadata
            .get("version")
            .and_then(serde_json::Value::as_str)
            .is_some()
        && metadata
            .get("age")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|age| {
                !age.is_empty()
                    && age.iter().all(|item| {
                        item.get("recipient")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(recipients::valid_key)
                            && item
                                .get("enc")
                                .and_then(serde_json::Value::as_str)
                                .is_some_and(|value| {
                                    value.contains("-----BEGIN AGE ENCRYPTED FILE-----")
                                })
                    })
            })
}

fn looks_like_key(path: &Path) -> bool {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    [
        "id_rsa",
        "id_dsa",
        "id_ecdsa",
        "id_ecdsa_sk",
        "id_ed25519",
        "id_ed25519_sk",
        "keys.txt",
        "credentials",
        ".env",
        ".env.local",
        ".netrc",
        ".pgpass",
    ]
    .contains(&name.as_ref())
        || [
            ".pem",
            ".key",
            ".p12",
            ".pfx",
            ".jks",
            ".keystore",
            ".kdbx",
            ".ovpn",
        ]
        .iter()
        .any(|suffix| name.ends_with(suffix))
}

struct Scanner {
    directories: BTreeSet<PathBuf>,
    allowed: Vec<(globset::GlobMatcher, String)>,
    canaries: Vec<canaries::Canary>,
    matcher: Option<aho_corasick::AhoCorasick>,
    findings: Vec<Finding>,
    total_findings: usize,
    finding_bytes: usize,
    scanned: usize,
    skipped: usize,
}

impl Scanner {
    fn add(&mut self, tier: u8, label: &str, path: &Path, line: usize) {
        self.total_findings = self.total_findings.saturating_add(1);
        let bytes = label.len().saturating_add(path.as_os_str().len());
        if self.findings.len() >= MAX_FINDINGS
            || self.finding_bytes.saturating_add(bytes) > MAX_FINDING_BYTES
        {
            return;
        }
        self.finding_bytes += bytes;
        self.findings.push(Finding {
            tier,
            label: label.to_string(),
            path: path.to_path_buf(),
            line,
        });
    }
    fn allowed(&self, path: &Path, label: &str) -> bool {
        self.allowed
            .iter()
            .any(|(glob, only)| (only.is_empty() || only == label) && glob.is_match(path))
    }
    fn scan(
        &mut self,
        path: &Path,
        data: Option<Vec<u8>>,
        inside: Option<bool>,
    ) -> Result<(), String> {
        let text = data
            .as_ref()
            .filter(|v| v.len() <= MAX_BYTES && !v.iter().take(8192).any(|b| *b == 0))
            .map(|v| String::from_utf8_lossy(v));
        let inside = inside.unwrap_or_else(|| self.directories.iter().any(|d| path.starts_with(d)));
        let needs_envelope = inside || vault::kind_of(path) == vault::SecretKind::Encrypted;
        let is_encrypted = needs_envelope
            && text
                .as_ref()
                .is_some_and(|t| t.contains("ENC[AES256_GCM,") && encrypted(t));
        if vault::kind_of(path) == vault::SecretKind::Encrypted && !is_encrypted {
            self.add(1, "not-encrypted", path, 0);
        } else if inside
            && path.file_name().is_none_or(|n| n != ".secret")
            && !is_encrypted
            && vault::kind_of(path) != vault::SecretKind::Template
        {
            self.add(1, "plaintext", path, 0);
        } else if looks_like_key(path) && !inside {
            self.add(1, "key-file", path, 0);
        }
        let Some(text) = text else {
            self.skipped += 1;
            return Ok(());
        };
        self.scanned += 1;
        let lines: Vec<_> = std::iter::once(0)
            .chain(
                text.bytes()
                    .enumerate()
                    .filter_map(|(i, b)| (b == b'\n').then_some(i + 1)),
            )
            .collect();
        // Even genuine SOPS files may contain unencrypted fields. Scan all text;
        // envelope recognition only satisfies the encrypted-file invariant.
        for (label, regex) in patterns::TOKENS.iter() {
            if self.allowed(path, label) {
                continue;
            }
            for matched in regex.find_iter(&text) {
                self.add(
                    2,
                    label,
                    path,
                    lines.partition_point(|offset| *offset <= matched.start()),
                );
            }
        }
        if !self.allowed(path, "value") {
            for matched in patterns::VALUE.captures_iter(&text) {
                let value = &matched[3];
                if value
                    .trim_matches(['\'', '"'])
                    .starts_with("ENC[AES256_GCM,")
                {
                    continue;
                }
                self.add(
                    2,
                    "value",
                    path,
                    lines.partition_point(|offset| *offset <= matched.get(0).unwrap().start()),
                );
            }
        }
        if let Some(matcher) = &self.matcher {
            let lowered = zeroize::Zeroizing::new(text.to_lowercase());
            let mut found = BTreeMap::new();
            for matched in matcher.find_overlapping_iter(lowered.as_bytes()) {
                found
                    .entry(matched.pattern().as_usize())
                    .or_insert_with(|| {
                        lowered[..matched.start()]
                            .bytes()
                            .filter(|b| *b == b'\n')
                            .count()
                            + 1
                    });
            }
            for (index, line) in found {
                self.add(0, &self.canaries[index].label.clone(), path, line);
            }
        }
        Ok(())
    }
}

pub fn run(
    context: &Context,
    paths: &[PathBuf],
    staged: bool,
    commits: Option<&str>,
    use_canaries: bool,
    all: bool,
) -> Result<ExitCode, String> {
    if staged && commits.is_some() {
        return Err("--staged and --commits cannot be combined".into());
    }
    let tracked = tracked_paths(context)?;
    let (values, notes) = if use_canaries {
        canaries::load(context)?
    } else {
        (Vec::new(), Vec::new())
    };
    let matcher = canaries::matcher(&values)?;
    let mut allowed = Vec::new();
    for (number, line) in recipients::block(&context.root.join("config/scan.dotfile"), "allow")? {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.is_empty() || fields.len() > 2 {
            return Err(format!(
                "config/scan.dotfile:{number}: expected <glob> [label]"
            ));
        }
        allowed.push((
            globset::GlobBuilder::new(fields[0])
                .literal_separator(false)
                .backslash_escape(false)
                .build()
                .map_err(|e| e.to_string())?
                .compile_matcher(),
            fields.get(1).unwrap_or(&"").to_string(),
        ));
    }
    let mut scanner = Scanner {
        directories: tracked
            .iter()
            .filter(|p| p.file_name().is_some_and(|n| n == ".secret"))
            .filter_map(|p| p.parent().map(Path::to_path_buf))
            .collect(),
        allowed,
        canaries: values,
        matcher,
        findings: Vec::new(),
        total_findings: 0,
        finding_bytes: 0,
        scanned: 0,
        skipped: 0,
    };
    for note in notes {
        println!("! {note}");
    }
    if staged || commits.is_some() {
        let mut objects = if let Some(revisions) = commits {
            history(context, revisions)?
        } else {
            index(context)?
        };
        for object in &objects {
            if object.path.file_name().is_some_and(|n| n == ".secret")
                && let Some(parent) = object.path.parent()
            {
                scanner.directories.insert(parent.to_path_buf());
            }
        }
        if staged {
            let changed: BTreeSet<_> = git(
                context,
                &[
                    "diff",
                    "--cached",
                    "--name-only",
                    "--diff-filter=ACMRT",
                    "-z",
                ],
            )?
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(path_from_bytes)
            .collect();
            objects.retain(|b| changed.contains(&b.path));
        }
        batch(context, &objects, |object, data| {
            scanner.scan(&object.path, data, object.inside)
        })?;
    } else {
        for path in if paths.is_empty() { &tracked } else { paths } {
            crate::cancel::check()?;
            let absolute = if path.is_absolute() {
                path.clone()
            } else {
                context.root.join(path)
            };
            let relative = absolute
                .strip_prefix(&context.root)
                .map_err(|_| "scan path is outside the repository")?;
            if relative
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
            {
                return Err("scan path is outside the repository".into());
            }
            let metadata = fs::symlink_metadata(&absolute)
                .map_err(|e| format!("inspect {}: {e}", path.display()))?;
            if metadata.file_type().is_symlink() {
                let target = fs::read_link(&absolute).map_err(|e| e.to_string())?;
                scanner.scan(
                    relative,
                    Some(target.to_string_lossy().as_bytes().to_vec()),
                    None,
                )?;
            } else if metadata.is_file() {
                let mut options = fs::OpenOptions::new();
                options.read(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
                }
                let mut file = options
                    .open(&absolute)
                    .map_err(|e| format!("read {}: {e}", path.display()))?;
                if !file.metadata().map_err(|e| e.to_string())?.is_file() {
                    return Err(format!("not a regular scan source: {}", path.display()));
                }
                let mut data = Vec::new();
                (&mut file)
                    .take((MAX_BYTES + 1) as u64)
                    .read_to_end(&mut data)
                    .map_err(|e| e.to_string())?;
                scanner.scan(relative, Some(data), None)?;
            } else {
                scanner.scan(relative, None, None)?;
            }
        }
    }
    scanner.findings.sort_by(|a, b| {
        (a.tier, &a.path, a.line, &a.label).cmp(&(b.tier, &b.path, b.line, &b.label))
    });
    if scanner.total_findings == 0 {
        println!(
            "✓ clean  {} files, {} canaries, {} not text",
            scanner.scanned,
            scanner.canaries.len(),
            scanner.skipped
        );
        return Ok(ExitCode::SUCCESS);
    }
    for finding in scanner
        .findings
        .iter()
        .take(if all { usize::MAX } else { 12 })
    {
        let tier = ["canary", "invariant", "pattern"][finding.tier as usize];
        eprintln!(
            "✗ {tier:<9} {}{}  {}",
            finding.path.display(),
            if finding.line == 0 {
                String::new()
            } else {
                format!(":{}", finding.line)
            },
            finding.label
        );
    }
    if !all && scanner.findings.len() > 12 {
        eprintln!("… {} more (--all)", scanner.findings.len() - 12);
    }
    if scanner.total_findings > scanner.findings.len() {
        eprintln!(
            "… {} omitted by report size limit",
            scanner.total_findings - scanner.findings.len()
        );
    }
    eprintln!(
        "{} findings in {} files; a canary is never allowed",
        scanner.total_findings, scanner.scanned
    );
    Ok(ExitCode::FAILURE)
}
