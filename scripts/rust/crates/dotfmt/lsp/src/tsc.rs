//! Project-wide TypeScript diagnostics from `tsc --noEmit`.
//!
//! The check runs in its own thread so formatting never waits for it. Results
//! are cached by project and a fingerprint of the files that can affect them.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range, Uri};

use crate::diagnostics::Diagnostics;
use crate::uri;

const TIMEOUT: Duration = Duration::from_secs(10);
const SCAN_LIMIT: usize = 20_000;
const SKIP: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "coverage",
    ".git",
    ".next",
    ".nuxt",
    ".svelte-kit",
    "vendor",
];

pub struct Tsc {
    jobs: crossbeam_channel::Sender<Job>,
    cache: Arc<Mutex<HashMap<PathBuf, Cache>>>,
    generation: Arc<AtomicU64>,
}

enum Job {
    Check { path: PathBuf, root: PathBuf },
    Invalidate,
    Shutdown,
}

struct Cache {
    fingerprint: u64,
    diagnostics: Vec<(Uri, Vec<Diagnostic>)>,
}

impl Tsc {
    pub fn new(diagnostics: Arc<Diagnostics>) -> Self {
        let (jobs, receiver) = crossbeam_channel::unbounded();
        let cache = Arc::new(Mutex::new(HashMap::new()));
        let generation = Arc::new(AtomicU64::new(0));
        let cache_for_worker = Arc::clone(&cache);
        let generation_for_worker = Arc::clone(&generation);
        let _ = thread::Builder::new()
            .name("dotfmt-lsp-tsc".into())
            .spawn(move || {
                run(
                    receiver,
                    cache_for_worker,
                    generation_for_worker,
                    diagnostics,
                )
            });
        Self {
            jobs,
            cache,
            generation,
        }
    }

    pub fn schedule(&self, path: PathBuf, root: PathBuf) {
        let _ = self.jobs.send(Job::Check { path, root });
    }

    pub fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.cache
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
        let _ = self.jobs.send(Job::Invalidate);
    }
}

impl Drop for Tsc {
    fn drop(&mut self) {
        // Do not join: a tsc run can take up to TIMEOUT and must not delay shutdown.
        let _ = self.jobs.send(Job::Shutdown);
    }
}

fn run(
    jobs: crossbeam_channel::Receiver<Job>,
    cache: Arc<Mutex<HashMap<PathBuf, Cache>>>,
    generation: Arc<AtomicU64>,
    diagnostics: Arc<Diagnostics>,
) {
    while let Ok(job) = jobs.recv() {
        match job {
            Job::Check { path, root } => {
                let Some(tsconfig) = nearest_tsconfig(&path, &root) else {
                    continue;
                };
                let project = tsconfig.parent().unwrap_or(&root).to_path_buf();
                let fingerprint = fingerprint(&tsconfig, &project);
                let generation_now = generation.load(Ordering::SeqCst);
                let cached = cache
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .get(&project)
                    .filter(|entry| entry.fingerprint == fingerprint)
                    .map(|entry| entry.diagnostics.clone());
                let entries = match cached {
                    Some(entries) => entries,
                    None => {
                        let Some(lines) = check(&project, &tsconfig, &diagnostics) else {
                            continue;
                        };
                        let entries = group(&project, lines);
                        cache
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .insert(
                                project.clone(),
                                Cache {
                                    fingerprint,
                                    diagnostics: entries.clone(),
                                },
                            );
                        entries
                    }
                };
                if generation.load(Ordering::SeqCst) == generation_now {
                    diagnostics.set_tsc(&project, entries);
                }
            }
            Job::Invalidate => {
                diagnostics.clear_tsc();
            }
            Job::Shutdown => break,
        }
    }
}

fn nearest_tsconfig(file: &Path, root: &Path) -> Option<PathBuf> {
    let mut directory = file.parent()?;
    loop {
        let candidate = directory.join("tsconfig.json");
        if candidate.is_file() {
            return Some(candidate);
        }
        if directory == root || !directory.starts_with(root) {
            break;
        }
        directory = directory.parent()?;
    }
    let candidate = root.join("tsconfig.json");
    candidate.is_file().then_some(candidate)
}

fn tsc_binary(project: &Path) -> PathBuf {
    #[cfg(windows)]
    let local = project.join("node_modules").join(".bin").join("tsc.cmd");
    #[cfg(not(windows))]
    let local = project.join("node_modules").join(".bin").join("tsc");
    if local.is_file() {
        local
    } else {
        PathBuf::from("tsc")
    }
}

fn check(project: &Path, tsconfig: &Path, diagnostics: &Diagnostics) -> Option<Vec<TscLine>> {
    let mut child = match Command::new(tsc_binary(project))
        .arg("--noEmit")
        .arg("--pretty")
        .arg("false")
        .arg("--project")
        .arg(tsconfig)
        .current_dir(project)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            diagnostics.log(format!("tsc unavailable: {error}"));
            return None;
        }
    };
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;
    let reader = thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stdout.read_to_end(&mut buffer);
        buffer
    });
    let errors = thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stderr.read_to_end(&mut buffer);
        buffer
    });
    if let Err(error) = wait(&mut child) {
        diagnostics.log(format!("tsc failed: {error}"));
        let _ = reader.join();
        let _ = errors.join();
        return None;
    }
    let stdout = reader.join().unwrap_or_default();
    let stderr = errors.join().unwrap_or_default();
    let mut lines = Vec::new();
    for line in String::from_utf8_lossy(&stdout)
        .lines()
        .chain(String::from_utf8_lossy(&stderr).lines())
    {
        if let Some(line) = TscLine::parse(line, project) {
            lines.push(line);
        }
    }
    Some(lines)
}

fn wait(child: &mut Child) -> Result<(), String> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) if started.elapsed() < TIMEOUT => thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {}s", TIMEOUT.as_secs()));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

struct TscLine {
    file: PathBuf,
    line: u32,
    column: u32,
    severity: DiagnosticSeverity,
    code: String,
    message: String,
}

impl TscLine {
    fn parse(line: &str, project: &Path) -> Option<Self> {
        let open = line.find('(')?;
        let close = open + line[open..].find("): ")?;
        let file = &line[..open];
        let rest = &line[close + 3..];
        let position = &line[open + 1..close];
        let (row, column) = position.split_once(',')?;
        let row: u32 = row.parse().ok()?;
        let column: u32 = column.parse().ok()?;
        let (severity, rest) = if let Some(rest) = rest.strip_prefix("error ") {
            (DiagnosticSeverity::ERROR, rest)
        } else {
            let rest = rest.strip_prefix("warning ")?;
            (DiagnosticSeverity::WARNING, rest)
        };
        let (code, message) = rest.split_once(": ")?;
        let code = code.strip_prefix("TS").unwrap_or(code);
        let file = Path::new(file);
        Some(Self {
            file: if file.is_absolute() {
                file.to_path_buf()
            } else {
                project.join(file)
            },
            line: row.max(1),
            column: column.max(1),
            severity,
            code: format!("TS{code}"),
            message: message.to_owned(),
        })
    }

    fn diagnostic(&self) -> Diagnostic {
        Diagnostic {
            range: Range::new(
                Position::new(self.line - 1, self.column.saturating_sub(1)),
                Position::new(self.line - 1, self.column),
            ),
            severity: Some(self.severity),
            code: Some(NumberOrString::String(self.code.clone())),
            source: Some("dotfmt(tsc)".into()),
            message: self.message.clone(),
            ..Diagnostic::default()
        }
    }
}

fn group(project: &Path, lines: Vec<TscLine>) -> Vec<(Uri, Vec<Diagnostic>)> {
    let mut grouped: HashMap<Uri, Vec<Diagnostic>> = HashMap::new();
    for line in lines {
        let path = line.file.strip_prefix(project).unwrap_or(&line.file);
        let absolute = if line.file.is_absolute() && !path.starts_with(project) {
            line.file.clone()
        } else {
            project.join(path)
        };
        grouped
            .entry(uri::from_path(&absolute))
            .or_default()
            .push(line.diagnostic());
    }
    grouped.into_iter().collect()
}

fn fingerprint(tsconfig: &Path, project: &Path) -> u64 {
    let mut hasher = DefaultHasher::new();
    hash_file(&mut hasher, tsconfig);
    let mut seen = 0;
    walk(&mut hasher, project, &mut seen);
    hasher.finish()
}

fn walk(hasher: &mut DefaultHasher, directory: &Path, seen: &mut usize) {
    if *seen >= SCAN_LIMIT {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        if *seen >= SCAN_LIMIT {
            return;
        }
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            if !SKIP.contains(&entry.file_name().to_string_lossy().as_ref()) {
                walk(hasher, &path, seen);
            }
            continue;
        }
        if !relevant(&path) {
            continue;
        }
        *seen += 1;
        hash_file(hasher, &path);
    }
}

fn relevant(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some(
            "ts" | "tsx"
                | "mts"
                | "cts"
                | "js"
                | "jsx"
                | "mjs"
                | "cjs"
                | "json"
                | "vue"
                | "svelte"
                | "astro"
        )
    ) || path.file_name().is_some_and(|name| name == "tsconfig.json")
}

fn hash_file(hasher: &mut DefaultHasher, path: &Path) {
    path.hash(hasher);
    if let Ok(metadata) = fs::metadata(path) {
        metadata.len().hash(hasher);
        if let Ok(modified) = metadata.modified() {
            modified
                .duration_since(SystemTime::UNIX_EPOCH)
                .ok()
                .hash(hasher);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn parses_tsc_error_lines() {
        let line = TscLine::parse(
            "src/a.ts(3,7): error TS2322: Type 'string' is not assignable.",
            Path::new("/p"),
        )
        .unwrap();
        assert_eq!(line.file, Path::new("/p/src/a.ts"));
        assert_eq!((line.line, line.column), (3, 7));
        assert_eq!(line.code, "TS2322");
        assert_eq!(line.severity, DiagnosticSeverity::ERROR);
        let diagnostic = line.diagnostic();
        assert_eq!(diagnostic.range.start, Position::new(2, 6));
        assert_eq!(diagnostic.source.as_deref(), Some("dotfmt(tsc)"));
    }

    #[test]
    fn ignores_prose_and_finds_the_nearest_tsconfig() {
        assert!(TscLine::parse("Found 1 error.", Path::new("/p")).is_none());
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path().join("project");
        let source = project.join("src");
        fs::create_dir_all(&source).unwrap();
        fs::write(project.join("tsconfig.json"), "{}").unwrap();
        assert_eq!(
            nearest_tsconfig(&source.join("a.ts"), &project).unwrap(),
            project.join("tsconfig.json")
        );
    }

    #[test]
    fn groups_diagnostics_by_file_uri() {
        let project = Path::new("/p");
        let lines = vec![TscLine {
            file: PathBuf::from("/p/src/a.ts"),
            line: 1,
            column: 1,
            severity: DiagnosticSeverity::ERROR,
            code: "TS1".into(),
            message: "x".into(),
        }];
        let grouped = group(project, lines);
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped[0].0, Uri::from_str("file:///p/src/a.ts").unwrap());
    }
}
