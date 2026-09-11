mod approvals;
mod inspect;
mod review;

use super::{canaries, patterns, recipients, sops, vault};
use crate::context::Context;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};
use std::sync::Arc;

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

fn index(context: &Context) -> Result<(Vec<Blob>, Vec<u8>), String> {
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
    Ok((found, data))
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
    source: Arc<Source>,
    line: usize,
}

#[derive(Debug)]
struct Source {
    path: PathBuf,
    oid: Option<String>,
    sha256: String,
}

fn fingerprint(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
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
    approvals: approvals::Store,
    canaries: Vec<canaries::Canary>,
    matcher: Option<aho_corasick::AhoCorasick>,
    findings: Vec<Finding>,
    total_findings: usize,
    finding_bytes: usize,
    scanned: usize,
    skipped: usize,
}

impl Scanner {
    fn add(&mut self, tier: u8, label: &str, source: &Arc<Source>, line: usize) {
        self.total_findings = self.total_findings.saturating_add(1);
        let bytes = label
            .len()
            .saturating_add(source.path.as_os_str().len())
            .saturating_add(128);
        if self.findings.len() >= MAX_FINDINGS
            || self.finding_bytes.saturating_add(bytes) > MAX_FINDING_BYTES
        {
            return;
        }
        self.finding_bytes += bytes;
        self.findings.push(Finding {
            tier,
            label: label.to_string(),
            source: Arc::clone(source),
            line,
        });
    }
    fn allowed(&self, source: &Source, label: &str) -> bool {
        self.allowed
            .iter()
            .any(|(glob, only)| (only.is_empty() || only == label) && glob.is_match(&source.path))
            || self.approvals.contains(&source.path, &source.sha256, label)
    }
    fn scan(
        &mut self,
        path: &Path,
        data: Option<Vec<u8>>,
        inside: Option<bool>,
        oid: Option<&str>,
    ) -> Result<(), String> {
        let source = Arc::new(Source {
            path: path.to_path_buf(),
            oid: oid.map(str::to_owned),
            sha256: data.as_deref().map(fingerprint).unwrap_or_default(),
        });
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
            self.add(1, "not-encrypted", &source, 0);
        } else if inside
            && path.file_name().is_none_or(|n| n != ".secret")
            && !is_encrypted
            && vault::kind_of(path) != vault::SecretKind::Template
        {
            self.add(1, "plaintext", &source, 0);
        } else if looks_like_key(path) && !inside {
            self.add(1, "key-file", &source, 0);
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
            if self.allowed(&source, label) {
                continue;
            }
            for matched in regex.find_iter(&text) {
                self.add(
                    2,
                    label,
                    &source,
                    lines.partition_point(|offset| *offset <= matched.start()),
                );
            }
        }
        if !self.allowed(&source, "value") {
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
                    &source,
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
                self.add(0, &self.canaries[index].label.clone(), &source, line);
            }
        }
        Ok(())
    }
}

pub fn run(
    context: &Context,
    paths: &[PathBuf],
    staged: bool,
    commits: &[String],
    use_canaries: bool,
    all: bool,
    review_requested: bool,
) -> Result<ExitCode, String> {
    if staged && !commits.is_empty() {
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
        approvals: approvals::Store::load(context)?,
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
    let mut index_snapshot = None;
    if staged || !commits.is_empty() {
        let mut objects = if !commits.is_empty() {
            let mut objects = Vec::new();
            let mut seen = BTreeSet::new();
            for revisions in commits {
                for object in history(context, revisions)? {
                    if seen.insert((object.path.clone(), object.oid.clone(), object.inside)) {
                        objects.push(object);
                    }
                }
            }
            objects
        } else {
            let (objects, snapshot) = index(context)?;
            index_snapshot = Some(snapshot);
            objects
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
            scanner.scan(&object.path, data, object.inside, Some(&object.oid))
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
            scanner.scan(relative, read_worktree(&absolute)?, None, None)?;
        }
    }
    verify_index(context, index_snapshot.as_deref())?;
    scanner.findings.sort_by(|a, b| {
        (a.tier, &a.source.path, a.line, &a.label).cmp(&(b.tier, &b.source.path, b.line, &b.label))
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
    if review_requested
        && scanner.total_findings == scanner.findings.len()
        && let Some(session) = review::Session::open()?
    {
        return review_findings(
            context,
            &scanner,
            staged,
            !commits.is_empty(),
            index_snapshot.as_deref(),
            session,
        );
    }
    for finding in scanner
        .findings
        .iter()
        .take(if all { usize::MAX } else { 12 })
    {
        let tier = ["canary", "invariant", "pattern"][finding.tier as usize];
        eprintln!(
            "✗ {tier:<9} {}{}  {}",
            crate::ui::sanitize_text(&finding.source.path.to_string_lossy()),
            if finding.line == 0 {
                String::new()
            } else {
                format!(":{}", finding.line)
            },
            crate::ui::sanitize_text(&finding.label)
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
    let affected = scanner
        .findings
        .iter()
        .map(|f| &f.source.path)
        .collect::<BTreeSet<_>>()
        .len();
    eprintln!(
        "{} finding{} in {affected} file{} ({} scanned)",
        scanner.total_findings,
        if scanner.total_findings == 1 { "" } else { "s" },
        if affected == 1 { "" } else { "s" },
        scanner.scanned
    );
    if scanner.findings.iter().any(|f| f.tier < 2) {
        eprintln!("Canaries and encryption violations must be fixed before continuing.");
    }
    if review_requested {
        if scanner.total_findings != scanner.findings.len() {
            return Err("report limit reached; narrow the scan before reviewing".into());
        }
        eprintln!(
            "Review needs an interactive terminal; unresolved findings still block this operation."
        );
        eprintln!(
            "{}",
            if !commits.is_empty() {
                "Repeat this --commits scan with --review in an interactive terminal."
            } else if staged {
                "Run dotfile secret scan --staged --review in a terminal, then retry."
            } else {
                "Run dotfile secret scan --review in a terminal, then retry."
            }
        );
        return Ok(ExitCode::FAILURE);
    }
    eprintln!("Review with --review to inspect findings, accept false positives, or abort.");
    Ok(ExitCode::FAILURE)
}

fn read_worktree(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("inspect {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() {
        let target = fs::read_link(path).map_err(|error| error.to_string())?;
        return Ok(Some(target.to_string_lossy().as_bytes().to_vec()));
    }
    if !metadata.is_file() {
        return Ok(None);
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err(format!("not a regular scan source: {}", path.display()));
    }
    let mut data = Vec::new();
    (&mut file)
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut data)
        .map_err(|error| error.to_string())?;
    Ok(Some(data))
}

fn verify_index(context: &Context, snapshot: Option<&[u8]>) -> Result<(), String> {
    if let Some(snapshot) = snapshot
        && git(context, &["ls-files", "--stage", "-z"])? != snapshot
    {
        return Err("staged files changed during scan; retry before accepting".into());
    }
    Ok(())
}

fn source_bytes(context: &Context, source: &Source) -> Result<Vec<u8>, String> {
    let bytes = if let Some(oid) = &source.oid {
        git(context, &["cat-file", "blob", oid])?
    } else {
        read_worktree(&context.root.join(&source.path))?.ok_or("scan source is not text")?
    };
    if fingerprint(&bytes) != source.sha256 {
        return Err(format!(
            "{} changed during review; scan again",
            crate::ui::sanitize_text(&source.path.to_string_lossy())
        ));
    }
    Ok(bytes)
}

fn review_findings(
    context: &Context,
    scanner: &Scanner,
    staged: bool,
    history: bool,
    index_snapshot: Option<&[u8]>,
    mut session: review::Session,
) -> Result<ExitCode, String> {
    if scanner.total_findings != scanner.findings.len() {
        return Err("report limit reached; narrow the scan before reviewing".into());
    }
    let mut groups = BTreeMap::<(&Path, &str), Vec<&Finding>>::new();
    for finding in &scanner.findings {
        groups
            .entry((&finding.source.path, &finding.source.sha256))
            .or_default()
            .push(finding);
    }
    session.begin(
        scanner.total_findings,
        groups.len(),
        if staged {
            "staged"
        } else if history {
            "commits"
        } else {
            "working tree"
        },
    )?;
    let mut pending = Vec::new();
    for (index, findings) in groups.values().enumerate() {
        crate::cancel::check()?;
        let source = &findings[0].source;
        let labels = findings
            .iter()
            .map(|f| f.label.clone())
            .collect::<BTreeSet<_>>();
        let can_accept = findings.iter().all(|finding| finding.tier == 2);
        let mut rules = BTreeMap::new();
        for finding in findings {
            *rules.entry(finding.label.as_str()).or_insert(0) += 1;
        }
        let item = review::Item {
            path: &source.path,
            rules: &rules,
            position: index + 1,
            total: groups.len(),
            can_accept,
            version: history.then_some(source.oid.as_deref().unwrap_or(&source.sha256)),
        };
        let mut page = 0;
        let choice = session.choose(&item, || {
            let context = inspect::render(context, source, findings, &scanner.canaries, page)?;
            page += 1;
            Ok(context)
        })?;
        if choice == review::Decision::Abort {
            eprintln!(
                "{}",
                workstation::Style::for_stderr().dim("Aborted · no approvals saved")
            );
            return Ok(ExitCode::FAILURE);
        }
        if !can_accept {
            return Err("this finding cannot be accepted".into());
        }
        pending.push(approvals::Approval {
            path: source.path.clone(),
            sha256: source.sha256.clone(),
            labels,
        });
    }
    let verify = || {
        crate::cancel::check()?;
        verify_index(context, index_snapshot)?;
        if !staged && !history {
            for findings in groups.values() {
                source_bytes(context, &findings[0].source)?;
            }
        }
        Ok(())
    };
    verify()?;
    scanner.approvals.save(context, &pending, verify)?;
    verify()?;
    eprintln!(
        "{}",
        workstation::Style::for_stderr().green(&format!(
            "✓ {} approval{} saved",
            pending.len(),
            if pending.len() == 1 { "" } else { "s" }
        ))
    );
    Ok(ExitCode::SUCCESS)
}
