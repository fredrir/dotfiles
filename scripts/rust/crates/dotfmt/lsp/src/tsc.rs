//! Project-wide TypeScript diagnostics from `tsc --noEmit`.
//!
//! The check runs in its own thread so formatting never waits for it. There is
//! one compiler run per project, keyed by a fingerprint of the configuration
//! chain, package metadata and source files. Diagnostics are errors only;
//! unused-code codes are filtered out because dotfmt is a formatter.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range, Uri};

use crate::diagnostics::Diagnostics;
use crate::documents::Documents;
use crate::settings::{Settings, TscScope};
use crate::uri;

const TIMEOUT: Duration = Duration::from_secs(30);
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
const SCAN_LIMIT: usize = 20_000;
const MAX_FAILURES: u32 = 3;
const MAX_DIAGNOSTICS_PER_URI: usize = 200;
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

/// TypeScript codes that report unused code. The language crate reports these
/// as formatting/fix findings, never as editor diagnostics.
pub const FILTERED_CODES: &[u32] = &[6133, 6138, 6192, 6196, 6198, 6199, 6205, 7027, 7028];

/// Lockfiles whose change invalidates a cached compiler run.
const LOCKFILES: &[&str] = &[
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
    "npm-shrinkwrap.json",
];

pub struct Tsc {
    jobs: crossbeam_channel::Sender<Job>,
    cache: Arc<Mutex<HashMap<PathBuf, Cache>>>,
    state: Arc<Mutex<State>>,
    /// One generation per workspace folder; a folder reload never cancels
    /// another folder's in-flight compiler run.
    generations: Arc<Mutex<HashMap<PathBuf, u64>>>,
    /// Queued checks per project, with the documents that asked for them.
    /// Duplicate schedules for the same project/document collapse into one job.
    pending: Arc<Mutex<HashMap<PathBuf, HashSet<PathBuf>>>>,
}

enum Job {
    Check {
        project: PathBuf,
        tsconfig: PathBuf,
        root: PathBuf,
        generation: u64,
        settings: Settings,
    },
    Invalidate,
    InvalidateRoot(PathBuf),
    Shutdown,
}

#[derive(Clone)]
struct Cache {
    fingerprint: u64,
    diagnostics: Vec<(Uri, Vec<Diagnostic>)>,
}

/// Compiler discovery depends on the folder's `tsdk`, so the cache key keeps it.
type CompilerKey = (PathBuf, Option<PathBuf>);

#[derive(Default)]
struct State {
    compilers: HashMap<CompilerKey, Option<Compiler>>,
    versions: HashMap<CompilerKey, Option<String>>,
    logged: HashSet<CompilerKey>,
    failures: HashMap<PathBuf, u32>,
    backoff_logged: HashSet<PathBuf>,
}

impl State {
    fn clear(&mut self) {
        self.compilers.clear();
        self.versions.clear();
        self.logged.clear();
        self.failures.clear();
        self.backoff_logged.clear();
    }

    fn retain_root(&mut self, root: &Path) {
        self.compilers
            .retain(|(project, _), _| !project.starts_with(root));
        self.versions
            .retain(|(project, _), _| !project.starts_with(root));
        self.logged
            .retain(|(project, _)| !project.starts_with(root));
        self.failures
            .retain(|project, _| !project.starts_with(root));
        self.backoff_logged
            .retain(|project| !project.starts_with(root));
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) struct Compiler {
    program: PathBuf,
    node: bool,
    cmd: bool,
}

impl Compiler {
    fn command(&self) -> Command {
        if self.node {
            let mut command = Command::new("node");
            command.arg(&self.program);
            command
        } else if self.cmd {
            let mut command = Command::new("cmd");
            command.arg("/C").arg(&self.program);
            command
        } else {
            Command::new(&self.program)
        }
    }
}

pub struct ProjectStatus {
    pub root: PathBuf,
    pub tsconfig: Option<PathBuf>,
    pub compiler: Option<PathBuf>,
    pub version: Option<String>,
    pub state: &'static str,
}

impl Tsc {
    pub fn new(diagnostics: Arc<Diagnostics>, documents: Arc<Mutex<Documents>>) -> Self {
        let (jobs, receiver) = crossbeam_channel::unbounded();
        let cache = Arc::new(Mutex::new(HashMap::new()));
        let state = Arc::new(Mutex::new(State::default()));
        let generations = Arc::new(Mutex::new(HashMap::new()));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let cache_for_worker = Arc::clone(&cache);
        let state_for_worker = Arc::clone(&state);
        let generations_for_worker = Arc::clone(&generations);
        let pending_for_worker = Arc::clone(&pending);
        let _ = thread::Builder::new()
            .name("dotfmt-lsp-tsc".into())
            .spawn(move || {
                run(
                    receiver,
                    cache_for_worker,
                    state_for_worker,
                    generations_for_worker,
                    pending_for_worker,
                    documents,
                    diagnostics,
                )
            });
        Self {
            jobs,
            cache,
            state,
            generations,
            pending,
        }
    }

    pub fn schedule(&self, path: PathBuf, root: PathBuf, settings: Settings) {
        let Some(tsconfig) = nearest_tsconfig(&path, &root) else {
            return;
        };
        let project = tsconfig.parent().unwrap_or(&root).to_path_buf();
        let generation = self.generation_for(&root);
        let queued = {
            let mut pending = self
                .pending
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let documents = pending.entry(project.clone()).or_default();
            // The same document never queues a second check while its
            // project is pending, and a second document joins the one job.
            let first = documents.is_empty();
            if !documents.insert(path) {
                return;
            }
            first
        };
        if !queued {
            return;
        }
        let _ = self.jobs.send(Job::Check {
            project,
            tsconfig,
            root,
            generation,
            settings,
        });
    }

    fn generation_for(&self, root: &Path) -> u64 {
        *self
            .generations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .entry(root.to_path_buf())
            .or_insert(0)
    }

    fn bump_generation(&self, root: &Path) -> u64 {
        let mut generations = self
            .generations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let generation = generations.entry(root.to_path_buf()).or_insert(0);
        *generation += 1;
        *generation
    }

    pub fn invalidate(&self) {
        let roots: Vec<PathBuf> = self
            .generations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .keys()
            .cloned()
            .collect();
        for root in roots {
            self.bump_generation(&root);
        }
        self.cache
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
        self.pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
        let _ = self.jobs.send(Job::Invalidate);
    }

    /// Invalidates one workspace folder's projects and compiler state.
    ///
    /// Other folders keep their caches, in-flight runs and diagnostics.
    pub fn invalidate_root(&self, root: &Path) {
        self.bump_generation(root);
        self.cache
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|project, _| !project.starts_with(root));
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain_root(root);
        self.pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|project, _| !project.starts_with(root));
        let _ = self.jobs.send(Job::InvalidateRoot(root.to_path_buf()));
    }

    /// Resolved compiler state for `dotfmt/doctor`. May spawn `<tsc> --version`.
    pub fn status(&self, roots: &[(PathBuf, Settings)]) -> Vec<ProjectStatus> {
        roots
            .iter()
            .map(|(root, settings)| {
                let tsconfig = root_tsconfig(root);
                let project = tsconfig
                    .as_deref()
                    .and_then(Path::parent)
                    .unwrap_or(root)
                    .to_path_buf();
                let compiler = self.compiler_for(&project, settings.tsdk.as_deref());
                let version = compiler.as_ref().and_then(|compiler| {
                    self.version_for(&project, settings.tsdk.as_deref(), compiler)
                });
                let state = if compiler.is_none() {
                    "unavailable"
                } else if self.backed_off(&project) {
                    "backoff"
                } else if tsconfig.is_none() {
                    "no-tsconfig"
                } else {
                    "ok"
                };
                ProjectStatus {
                    root: root.clone(),
                    tsconfig,
                    compiler: compiler.map(|compiler| compiler.program),
                    version,
                    state,
                }
            })
            .collect()
    }

    fn compiler_for(&self, project: &Path, tsdk: Option<&Path>) -> Option<Compiler> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state
            .compilers
            .entry((project.to_path_buf(), tsdk.map(Path::to_path_buf)))
            .or_insert_with(|| discover(project, tsdk))
            .clone()
    }

    fn version_for(
        &self,
        project: &Path,
        tsdk: Option<&Path>,
        compiler: &Compiler,
    ) -> Option<String> {
        let key = (project.to_path_buf(), tsdk.map(Path::to_path_buf));
        if let Some(version) = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .versions
            .get(&key)
        {
            return version.clone();
        }
        let version = version_of(compiler, project);
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .versions
            .insert(key, version.clone());
        version
    }

    fn backed_off(&self, project: &Path) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .failures
            .get(project)
            .is_some_and(|failures| *failures >= MAX_FAILURES)
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
    state: Arc<Mutex<State>>,
    generations: Arc<Mutex<HashMap<PathBuf, u64>>>,
    pending: Arc<Mutex<HashMap<PathBuf, HashSet<PathBuf>>>>,
    documents: Arc<Mutex<Documents>>,
    diagnostics: Arc<Diagnostics>,
) {
    while let Ok(job) = jobs.recv() {
        match job {
            Job::Check {
                project,
                tsconfig,
                root,
                generation: job_generation,
                settings,
            } => {
                pending
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .remove(&project);
                if job_generation != current_generation(&generations, &root) {
                    continue;
                }
                check_project(
                    &project,
                    &tsconfig,
                    &root,
                    &settings,
                    &cache,
                    &state,
                    &generations,
                    &documents,
                    &diagnostics,
                );
            }
            Job::Invalidate => {
                diagnostics.clear_tsc();
            }
            Job::InvalidateRoot(root) => {
                diagnostics.clear_tsc_under(root);
            }
            Job::Shutdown => break,
        }
    }
}

fn current_generation(generations: &Mutex<HashMap<PathBuf, u64>>, root: &Path) -> u64 {
    generations
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(root)
        .copied()
        .unwrap_or(0)
}

#[allow(clippy::too_many_arguments)]
fn check_project(
    project: &Path,
    tsconfig: &Path,
    root: &Path,
    settings: &Settings,
    cache: &Mutex<HashMap<PathBuf, Cache>>,
    state: &Mutex<State>,
    generations: &Mutex<HashMap<PathBuf, u64>>,
    documents: &Mutex<Documents>,
    diagnostics: &Diagnostics,
) {
    let generation_now = current_generation(generations, root);
    let compiler = {
        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
        state
            .compilers
            .entry((project.to_path_buf(), settings.tsdk.clone()))
            .or_insert_with(|| discover(project, settings.tsdk.as_deref()))
            .clone()
    };
    let Some(compiler) = compiler else {
        log_once(
            state,
            diagnostics,
            project,
            settings.tsdk.as_deref(),
            format!("tsc: no compiler found for {}", project.display()),
        );
        diagnostics.set_tsc(project, generation_now, Vec::new());
        return;
    };
    if state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .failures
        .get(project)
        .is_some_and(|failures| *failures >= MAX_FAILURES)
    {
        diagnostics.set_tsc(project, generation_now, Vec::new());
        return;
    }
    if log_once(
        state,
        diagnostics,
        project,
        settings.tsdk.as_deref(),
        String::new(),
    ) {
        let version = {
            let key = (project.to_path_buf(), settings.tsdk.clone());
            let cached = state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .versions
                .get(&key)
                .cloned();
            match cached {
                Some(version) => version,
                None => {
                    let version = version_of(&compiler, project);
                    state
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .versions
                        .insert(key, version.clone());
                    version
                }
            }
        };
        diagnostics.log(format!(
            "tsc: {} ({})",
            compiler.program.display(),
            version.as_deref().unwrap_or("unknown version")
        ));
    }
    let fingerprint = fingerprint(tsconfig, project, &compiler, settings.tsdk.as_deref());
    let cached = cache
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(project)
        .filter(|entry| entry.fingerprint == fingerprint)
        .map(|entry| entry.diagnostics.clone());
    let entries = match cached {
        Some(entries) => entries,
        None => {
            let Some(lines) = check(project, tsconfig, &compiler, state, diagnostics) else {
                return;
            };
            state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .failures
                .remove(project);
            let entries = group(project, lines, diagnostics);
            if current_generation(generations, root) == generation_now {
                cache
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .insert(
                        project.to_path_buf(),
                        Cache {
                            fingerprint,
                            diagnostics: entries.clone(),
                        },
                    );
            }
            entries
        }
    };
    if current_generation(generations, root) != generation_now {
        return;
    }
    let entries = scoped(tsconfig, entries, documents, settings.tsc_scope);
    diagnostics.set_tsc(project, generation_now, entries);
}

/// Logs `message` once per project and SDK; returns whether this call was the first.
fn log_once(
    state: &Mutex<State>,
    diagnostics: &Diagnostics,
    project: &Path,
    tsdk: Option<&Path>,
    message: String,
) -> bool {
    let first = state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .logged
        .insert((project.to_path_buf(), tsdk.map(Path::to_path_buf)));
    if first && !message.is_empty() {
        diagnostics.log(message);
    }
    first
}

fn nearest_tsconfig(file: &Path, root: &Path) -> Option<PathBuf> {
    let mut directory = file.parent()?;
    loop {
        for name in ["tsconfig.json", "jsconfig.json"] {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        if directory == root || !directory.starts_with(root) {
            break;
        }
        directory = directory.parent()?;
    }
    for name in ["tsconfig.json", "jsconfig.json"] {
        let candidate = root.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn root_tsconfig(root: &Path) -> Option<PathBuf> {
    ["tsconfig.json", "jsconfig.json"]
        .into_iter()
        .map(|name| root.join(name))
        .find(|candidate| candidate.is_file())
}

fn scoped(
    tsconfig: &Path,
    entries: Vec<(Uri, Vec<Diagnostic>)>,
    documents: &Mutex<Documents>,
    scope: TscScope,
) -> Vec<(Uri, Vec<Diagnostic>)> {
    match scope {
        TscScope::Project => entries,
        TscScope::Open => {
            let open: HashSet<Uri> = documents
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .iter()
                .map(|(uri, _)| uri.clone())
                .collect();
            let config = uri::from_path(tsconfig);
            entries
                .into_iter()
                .filter(|(uri, _)| open.contains(uri) || *uri == config)
                .collect()
        }
    }
}

fn check(
    project: &Path,
    tsconfig: &Path,
    compiler: &Compiler,
    state: &Mutex<State>,
    diagnostics: &Diagnostics,
) -> Option<Vec<TscLine>> {
    let mut command = compiler.command();
    command
        .arg("--noEmit")
        .arg("--pretty")
        .arg("false")
        .arg("--locale")
        .arg("en")
        .arg("--project")
        .arg(tsconfig)
        .current_dir(project);
    match run_command(command, TIMEOUT) {
        Ok(output) => Some(parse_output(&output, project, tsconfig)),
        Err(error) => {
            let message = match error {
                RunError::Spawn(error) => {
                    format!("failed to start ({}): {error}", compiler.program.display())
                }
                RunError::Timeout => {
                    format!("timed out after {}s and was killed", TIMEOUT.as_secs())
                }
            };
            let (count, backoff) = {
                let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
                let count = state
                    .failures
                    .entry(project.to_path_buf())
                    .and_modify(|count| *count += 1)
                    .or_insert(1);
                let count = *count;
                let backoff =
                    count >= MAX_FAILURES && state.backoff_logged.insert(project.to_path_buf());
                (count, backoff)
            };
            diagnostics.log(format!(
                "tsc: {} {message} ({count}/{MAX_FAILURES})",
                project.display()
            ));
            if backoff {
                diagnostics.log(format!(
                    "tsc: disabling compiler checks for {} until the configuration or compiler changes",
                    project.display()
                ));
            }
            None
        }
    }
}

struct Output {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

enum RunError {
    Spawn(std::io::Error),
    Timeout,
}

fn run_command(mut command: Command, timeout: Duration) -> Result<Output, RunError> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(RunError::Spawn)?;
    let stdout = child
        .stdout
        .take()
        .map(|mut pipe| {
            thread::spawn(move || {
                let mut buffer = Vec::new();
                let _ = pipe.read_to_end(&mut buffer);
                buffer
            })
        })
        .ok_or_else(|| RunError::Spawn(std::io::Error::other("missing stdout pipe")))?;
    let stderr = child
        .stderr
        .take()
        .map(|mut pipe| {
            thread::spawn(move || {
                let mut buffer = Vec::new();
                let _ = pipe.read_to_end(&mut buffer);
                buffer
            })
        })
        .ok_or_else(|| RunError::Spawn(std::io::Error::other("missing stderr pipe")))?;
    match wait(&mut child, timeout) {
        Ok(_) => {}
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout.join();
            let _ = stderr.join();
            return Err(error);
        }
    }
    Ok(Output {
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

fn wait(child: &mut Child, timeout: Duration) -> Result<ExitStatus, RunError> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if started.elapsed() < timeout => thread::sleep(Duration::from_millis(25)),
            Ok(None) => return Err(RunError::Timeout),
            Err(error) => return Err(RunError::Spawn(error)),
        }
    }
}

fn version_of(compiler: &Compiler, project: &Path) -> Option<String> {
    let mut command = compiler.command();
    command.arg("--version").current_dir(project);
    let output = run_command(command, VERSION_TIMEOUT).ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let first = text.lines().next()?.trim();
    if first.is_empty() {
        None
    } else {
        Some(first.trim_start_matches("Version ").to_owned())
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
    fn parse(line: &str, project: &Path, tsconfig: &Path) -> Option<Self> {
        if let Some((file, row, column, rest)) = location(line) {
            let (severity, rest) = severity(rest)?;
            let (code, message) = rest.split_once(": ")?;
            let code = code.strip_prefix("TS").unwrap_or(code);
            let file = Path::new(file);
            return Some(Self {
                file: if file.is_absolute() || windows_absolute(file) {
                    file.to_path_buf()
                } else {
                    project.join(file)
                },
                line: row.max(1),
                column: column.max(1),
                severity,
                code: format!("TS{code}"),
                message: message.to_owned(),
            });
        }
        // Position-less errors (`error TS5053: ...`) belong to the project
        // configuration; attach them to the tsconfig so they are not dropped.
        let (severity, rest) = severity(line.trim_start())?;
        let (code, message) = rest.split_once(": ")?;
        let code = code.strip_prefix("TS").unwrap_or(code);
        Some(Self {
            file: tsconfig.to_path_buf(),
            line: 1,
            column: 1,
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

    fn number(&self) -> u32 {
        self.code
            .strip_prefix("TS")
            .and_then(|code| code.parse().ok())
            .unwrap_or(0)
    }
}

/// Splits `file(line,col): rest` and `file(line,col) - rest`, including paths
/// that themselves contain parentheses.
fn location(line: &str) -> Option<(&str, u32, u32, &str)> {
    for (open, _) in line.match_indices('(').rev() {
        let Some(close) = line[open + 1..].find(')').map(|offset| open + 1 + offset) else {
            continue;
        };
        let Some((row, column)) = line[open + 1..close].split_once(',') else {
            continue;
        };
        let (Ok(row), Ok(column)) = (row.trim().parse(), column.trim().parse()) else {
            continue;
        };
        let rest = &line[close + 1..];
        let Some(rest) = rest.strip_prefix(": ").or_else(|| rest.strip_prefix(" - ")) else {
            continue;
        };
        let file = &line[..open];
        if file.is_empty() {
            continue;
        }
        return Some((file, row, column, rest));
    }
    None
}

fn severity(rest: &str) -> Option<(DiagnosticSeverity, &str)> {
    if let Some(rest) = rest.strip_prefix("error TS") {
        return Some((DiagnosticSeverity::ERROR, rest));
    }
    if let Some(rest) = rest.strip_prefix("warning TS") {
        return Some((DiagnosticSeverity::WARNING, rest));
    }
    None
}

fn windows_absolute(path: &Path) -> bool {
    let text = path.to_string_lossy();
    let bytes = text.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

fn parse_output(output: &Output, project: &Path, tsconfig: &Path) -> Vec<TscLine> {
    let mut lines = Vec::new();
    let mut continuation = false;
    for raw in String::from_utf8_lossy(&output.stdout)
        .lines()
        .chain(String::from_utf8_lossy(&output.stderr).lines())
    {
        if raw.trim().is_empty() {
            continue;
        }
        if let Some(line) = TscLine::parse(raw, project, tsconfig) {
            // Warnings are editor noise for a formatter; unused-code codes are
            // handled by dotfmt's own fix pipeline.
            let kept = line.severity == DiagnosticSeverity::ERROR
                && !FILTERED_CODES.contains(&line.number());
            continuation = kept;
            if kept {
                lines.push(line);
            }
        } else if continuation
            && (raw.starts_with(' ') || raw.starts_with('\t'))
            && let Some(last) = lines.last_mut()
        {
            last.message.push('\n');
            last.message.push_str(raw.trim());
        } else {
            continuation = false;
        }
    }
    lines
}

fn group(
    project: &Path,
    lines: Vec<TscLine>,
    diagnostics: &Diagnostics,
) -> Vec<(Uri, Vec<Diagnostic>)> {
    let mut grouped: HashMap<Uri, Vec<Diagnostic>> = HashMap::new();
    for line in lines {
        let path = line.file.strip_prefix(project).unwrap_or(&line.file);
        let absolute = if (line.file.is_absolute() || windows_absolute(&line.file))
            && !path.starts_with(project)
        {
            line.file.clone()
        } else {
            project.join(path)
        };
        grouped
            .entry(uri::from_path(&absolute))
            .or_default()
            .push(line.diagnostic());
    }
    grouped
        .into_iter()
        .map(|(uri, mut diagnostics_for_uri)| {
            diagnostics_for_uri.sort_by_key(|diagnostic| {
                (
                    diagnostic.range.start.line,
                    diagnostic.range.start.character,
                )
            });
            let total = diagnostics_for_uri.len();
            if total > MAX_DIAGNOSTICS_PER_URI {
                diagnostics.log(format!(
                    "tsc: truncating {} diagnostics for {} to {}",
                    total - MAX_DIAGNOSTICS_PER_URI,
                    uri.as_str(),
                    MAX_DIAGNOSTICS_PER_URI
                ));
                diagnostics_for_uri.truncate(MAX_DIAGNOSTICS_PER_URI);
            }
            (uri, diagnostics_for_uri)
        })
        .collect()
}

fn fingerprint(tsconfig: &Path, project: &Path, compiler: &Compiler, tsdk: Option<&Path>) -> u64 {
    let mut hasher = DefaultHasher::new();
    compiler.hash(&mut hasher);
    tsdk.hash(&mut hasher);
    hash_file(&mut hasher, &compiler.program);
    for config in config_chain(tsconfig) {
        hash_file(&mut hasher, &config);
    }
    hash_file(&mut hasher, &project.join("package.json"));
    for lockfile in LOCKFILES {
        hash_file(&mut hasher, &project.join(lockfile));
    }
    let mut seen = 0;
    walk(&mut hasher, project, &mut seen);
    hasher.finish()
}

fn config_chain(tsconfig: &Path) -> Vec<PathBuf> {
    let mut chain = Vec::new();
    let mut stack = vec![(tsconfig.to_path_buf(), 0usize)];
    while let Some((path, depth)) = stack.pop() {
        if depth > 8 || chain.contains(&path) || !path.is_file() {
            continue;
        }
        chain.push(path.clone());
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(Some(jsonc_parser::JsonValue::Object(object))) =
            jsonc_parser::parse_to_value(&text, &Default::default())
        else {
            continue;
        };
        let directory = path.parent().unwrap_or(Path::new("."));
        for extends in extends_values(&object) {
            if let Some(resolved) = resolve_extends(directory, &extends) {
                stack.push((resolved, depth + 1));
            }
        }
    }
    chain
}

fn extends_values(object: &jsonc_parser::JsonObject<'_>) -> Vec<String> {
    if let Some(extends) = object.get_string("extends") {
        return vec![extends.to_string()];
    }
    let Some(extends) = object.get_array("extends") else {
        return Vec::new();
    };
    extends
        .iter()
        .filter_map(|value| match value {
            jsonc_parser::JsonValue::String(extends) => Some(extends.to_string()),
            _ => None,
        })
        .collect()
}

fn resolve_extends(directory: &Path, spec: &str) -> Option<PathBuf> {
    let raw = Path::new(spec);
    let candidates = if raw.is_absolute() {
        vec![raw.to_path_buf()]
    } else if spec.starts_with('.') {
        vec![directory.join(raw)]
    } else {
        let base = directory.join("node_modules");
        match spec.split_once('/') {
            Some((scope, rest)) if scope.starts_with('@') => match rest.split_once('/') {
                Some((package, sub)) => vec![base.join(scope).join(package).join(sub)],
                None => vec![
                    base.join(scope).join(rest).join("tsconfig.json"),
                    base.join(scope).join(rest),
                ],
            },
            Some((package, sub)) => vec![base.join(package).join(sub)],
            None => vec![base.join(spec).join("tsconfig.json"), base.join(spec)],
        }
    };
    candidates.into_iter().find_map(|candidate| {
        if candidate.is_file() {
            Some(candidate)
        } else if candidate.join("tsconfig.json").is_file() {
            Some(candidate.join("tsconfig.json"))
        } else {
            let json = candidate.with_extension("json");
            json.is_file().then_some(json)
        }
    })
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
    ) || path
        .file_name()
        .is_some_and(|name| name == "tsconfig.json" || name == "jsconfig.json")
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

fn discover(project: &Path, tsdk: Option<&Path>) -> Option<Compiler> {
    let environment = std::env::var_os("DOTFMT_TSC").map(PathBuf::from);
    discover_with(
        project,
        tsdk,
        environment.as_deref(),
        std::env::var_os("PATH").as_deref(),
    )
}

fn discover_with(
    project: &Path,
    tsdk: Option<&Path>,
    environment: Option<&Path>,
    path: Option<&OsStr>,
) -> Option<Compiler> {
    if let Some(compiler) = environment.and_then(compiler_at) {
        return Some(compiler);
    }
    if let Some(compiler) = tsdk.and_then(compiler_at) {
        return Some(compiler);
    }
    for relative in [
        "node_modules/typescript/lib/tsc.js",
        "node_modules/.bin/tsc",
        "node_modules/@typescript/native-preview/bin/tsgo",
        "node_modules/.bin/tsgo",
    ] {
        if let Some(compiler) = walk_up(project, Path::new(relative)) {
            return Some(compiler);
        }
    }
    if let Some(compiler) = find_on_path("tsgo", path) {
        return Some(compiler);
    }
    find_on_path("tsc", path)
}

/// Resolves a `DOTFMT_TSC`/`tsdk` value: a package directory, `lib` directory,
/// `bin` directory, or the compiler file itself.
fn compiler_at(base: &Path) -> Option<Compiler> {
    if base.is_file() {
        return Some(compiler(base.to_path_buf()));
    }
    if !base.is_dir() {
        return None;
    }
    [
        "lib/tsc.js",
        "tsc.js",
        "bin/tsc",
        "bin/tsc.cmd",
        "bin/tsgo",
        ".bin/tsc",
        ".bin/tsc.cmd",
    ]
    .into_iter()
    .map(|relative| base.join(relative))
    .find(|candidate| candidate.is_file())
    .map(compiler)
}

fn walk_up(project: &Path, relative: &Path) -> Option<Compiler> {
    let mut directory = Some(project);
    while let Some(current) = directory {
        let candidate = current.join(relative);
        if candidate.is_file() {
            return Some(compiler(candidate));
        }
        directory = current.parent();
    }
    None
}

fn find_on_path(name: &str, path: Option<&OsStr>) -> Option<Compiler> {
    let path = path?;
    let names = if cfg!(windows) {
        vec![
            format!("{name}.cmd"),
            format!("{name}.exe"),
            format!("{name}.bat"),
            name.to_owned(),
        ]
    } else {
        vec![name.to_owned()]
    };
    for directory in std::env::split_paths(path) {
        for name in &names {
            let candidate = directory.join(name);
            if is_executable(&candidate) {
                return Some(compiler(candidate));
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(windows)]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

fn compiler(program: PathBuf) -> Compiler {
    let extension = program.extension().and_then(OsStr::to_str);
    Compiler {
        node: matches!(extension, Some("js" | "mjs" | "cjs")),
        cmd: cfg!(windows) && extension == Some("cmd"),
        program,
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
            Path::new("/p/tsconfig.json"),
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
    fn parses_legacy_and_windows_location_variants() {
        let line = TscLine::parse(
            "src/a.ts(3,7) - error TS2322: nope",
            Path::new("/p"),
            Path::new("/p/tsconfig.json"),
        )
        .unwrap();
        assert_eq!(line.code, "TS2322");
        let windows = TscLine::parse(
            r"C:\work (x)\a.ts(3,7): error TS2322: nope",
            Path::new("/p"),
            Path::new("/p/tsconfig.json"),
        )
        .unwrap();
        assert_eq!(windows.file, Path::new(r"C:\work (x)\a.ts"));
        assert_eq!((windows.line, windows.column), (3, 7));
    }

    #[test]
    fn positionless_and_continuation_lines_are_kept() {
        let output = Output {
            stdout: b"error TS5053: Option 'x' is invalid.\n  Try changing y.\nFound 1 error.\n"
                .to_vec(),
            stderr: Vec::new(),
        };
        let lines = parse_output(&output, Path::new("/p"), Path::new("/p/tsconfig.json"));
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].file, Path::new("/p/tsconfig.json"));
        assert_eq!(lines[0].message, "Option 'x' is invalid.\nTry changing y.");
    }

    #[test]
    fn unused_codes_and_warnings_are_filtered() {
        let output = Output {
            stdout: b"src/a.ts(1,1): error TS6133: unused\n\
                      src/a.ts(2,1): error TS7027: unreachable\n\
                      src/a.ts(3,1): error TS7028: unused ignore\n\
                      src/a.ts(4,1): warning TS9999: warn\n\
                      src/a.ts(5,1): error TS2322: real\n"
                .to_vec(),
            stderr: Vec::new(),
        };
        let lines = parse_output(&output, Path::new("/p"), Path::new("/p/tsconfig.json"));
        assert_eq!(
            lines.len(),
            1,
            "{:?}",
            lines.iter().map(|line| &line.code).collect::<Vec<_>>()
        );
        assert_eq!(lines[0].code, "TS2322");
    }

    #[test]
    fn ignores_prose_and_finds_the_nearest_config() {
        assert!(
            TscLine::parse(
                "Found 1 error.",
                Path::new("/p"),
                Path::new("/p/tsconfig.json")
            )
            .is_none()
        );
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path().join("project");
        let source = project.join("src");
        fs::create_dir_all(&source).unwrap();
        fs::write(project.join("tsconfig.json"), "{}").unwrap();
        assert_eq!(
            nearest_tsconfig(&source.join("a.ts"), &project).unwrap(),
            project.join("tsconfig.json")
        );
        fs::write(project.join("jsconfig.json"), "{}").unwrap();
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
        let (sender, _receiver) = crossbeam_channel::unbounded();
        let workspace = Arc::new(crate::workspace::Workspace::new(vec![PathBuf::from("/p")]));
        let documents = Arc::new(Mutex::new(Documents::default()));
        let client = Arc::new(crate::client::Client::new(sender));
        let settings = Arc::new(crate::settings::SettingsStore::new(
            false,
            Settings::default(),
        ));
        let diagnostics = Diagnostics::new(workspace, documents, client, settings, false);
        let grouped = group(project, lines, &diagnostics);
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped[0].0, Uri::from_str("file:///p/src/a.ts").unwrap());
    }

    #[test]
    fn discovery_prefers_the_project_compiler_over_path() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path();
        let path_bin = temporary.path().join("bin");
        fs::create_dir_all(&path_bin).unwrap();
        let path_tsc = path_bin.join("tsc");
        fs::write(&path_tsc, "").unwrap();
        make_executable(&path_tsc);

        let lib = project.join("node_modules/typescript/lib");
        fs::create_dir_all(&lib).unwrap();
        let lib_tsc = lib.join("tsc.js");
        fs::write(&lib_tsc, "").unwrap();
        let found = discover_with(project, None, None, Some(path_bin.as_os_str())).unwrap();
        assert_eq!(found.program, lib_tsc);
        assert!(found.node);

        fs::remove_file(&lib_tsc).unwrap();
        let bin = project.join("node_modules/.bin");
        fs::create_dir_all(&bin).unwrap();
        let bin_tsc = bin.join("tsc");
        fs::write(&bin_tsc, "").unwrap();
        make_executable(&bin_tsc);
        let found = discover_with(project, None, None, Some(path_bin.as_os_str())).unwrap();
        assert_eq!(found.program, bin_tsc);
    }

    #[test]
    fn explicit_compiler_values_win_and_accept_directories() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let package = root.join("typescript");
        let lib = package.join("lib");
        fs::create_dir_all(&lib).unwrap();
        let tsc = lib.join("tsc.js");
        fs::write(&tsc, "").unwrap();
        assert_eq!(compiler_at(&package).unwrap().program, tsc);
        assert_eq!(compiler_at(&lib).unwrap().program, tsc);
        assert_eq!(compiler_at(&tsc).unwrap().program, tsc);

        let environment = root.join("env-tsc");
        fs::write(&environment, "").unwrap();
        let found = discover_with(root, Some(&package), Some(&environment), None).unwrap();
        assert_eq!(found.program, environment);
    }

    #[test]
    fn fingerprint_tracks_extended_configs() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path();
        let base = project.join("tsconfig.base.json");
        fs::write(&base, "{}").unwrap();
        let config = project.join("tsconfig.json");
        fs::write(&config, r#"{"extends": "./tsconfig.base.json"}"#).unwrap();
        let compiler = Compiler {
            program: project.join("tsc"),
            node: false,
            cmd: false,
        };
        let before = fingerprint(&config, project, &compiler, None);
        fs::write(&base, r#"{"compilerOptions":{"strict":true}}"#).unwrap();
        let after = fingerprint(&config, project, &compiler, None);
        assert_ne!(before, after);
        assert_eq!(config_chain(&config).len(), 2);
    }

    fn make_executable(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions).unwrap();
        }
    }

    #[cfg(unix)]
    fn fake_tsc(project: &Path, output: &str) -> PathBuf {
        let directory = project.join("node_modules/.bin");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("tsc");
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'Version 9.9.9'; exit 0; fi\n\
                 echo run >> marker.log\n{output}\n"
            ),
        )
        .unwrap();
        make_executable(&path);
        path
    }

    #[cfg(unix)]
    #[allow(clippy::type_complexity)]
    fn harness(
        project: &Path,
        scope: TscScope,
    ) -> (
        Arc<Tsc>,
        Arc<Diagnostics>,
        Arc<Mutex<Documents>>,
        crossbeam_channel::Receiver<lsp_server::Message>,
        Settings,
    ) {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let workspace = Arc::new(crate::workspace::Workspace::new(vec![
            project.to_path_buf(),
        ]));
        let documents = Arc::new(Mutex::new(Documents::default()));
        let settings = Settings {
            tsc_scope: scope,
            ..Settings::default()
        };
        let client = Arc::new(crate::client::Client::new(sender));
        let current = Arc::new(crate::settings::SettingsStore::new(true, settings.clone()));
        let diagnostics = Arc::new(Diagnostics::new(
            workspace,
            Arc::clone(&documents),
            client,
            current,
            false,
        ));
        let tsc = Arc::new(Tsc::new(Arc::clone(&diagnostics), Arc::clone(&documents)));
        (tsc, diagnostics, documents, receiver, settings)
    }

    #[cfg(unix)]
    fn wait_for_tsc(
        receiver: &crossbeam_channel::Receiver<lsp_server::Message>,
        uri: &Uri,
        timeout: Duration,
    ) -> lsp_types::PublishDiagnosticsParams {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .expect("timed out waiting for tsc diagnostics");
            let message = receiver
                .recv_timeout(remaining)
                .expect("diagnostics message");
            let lsp_server::Message::Notification(notification) = message else {
                continue;
            };
            if notification.method != "textDocument/publishDiagnostics" {
                continue;
            }
            let params: lsp_types::PublishDiagnosticsParams =
                serde_json::from_value(notification.params).unwrap();
            if &params.uri == uri && !params.diagnostics.is_empty() {
                return params;
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn one_run_serves_many_documents_and_filters_codes() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path();
        fs::write(project.join("tsconfig.json"), "{}").unwrap();
        fake_tsc(
            project,
            "echo \"src/a.ts(1,7): error TS2322: Type 'string' is not assignable to type 'number'.\"\n\
             echo \"src/a.ts(2,1): error TS6133: 'unused' is declared but its value is never read.\"\n\
             echo \"src/a.ts(3,1): error TS7027: Unreachable code detected.\"",
        );
        let source = project.join("src/a.ts");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, "const a: number = 'x';\n").unwrap();
        let uri = uri::from_path(&source);
        let (tsc, _diagnostics, documents, receiver, settings) = harness(project, TscScope::Open);
        documents.lock().unwrap().open(
            uri.clone(),
            crate::documents::Document::new("x".into(), "typescript".into(), 1),
        );
        for _ in 0..5 {
            tsc.schedule(source.clone(), project.to_path_buf(), settings.clone());
        }
        let params = wait_for_tsc(&receiver, &uri, Duration::from_secs(10));
        assert_eq!(params.diagnostics.len(), 1, "{:?}", params.diagnostics);
        assert_eq!(
            params.diagnostics[0].code,
            Some(NumberOrString::String("TS2322".into()))
        );
        assert_eq!(
            params.diagnostics[0].severity,
            Some(DiagnosticSeverity::ERROR)
        );
        thread::sleep(Duration::from_millis(250));
        let runs = fs::read_to_string(project.join("marker.log")).unwrap();
        assert_eq!(runs.lines().count(), 1, "{runs}");
    }

    #[cfg(unix)]
    #[test]
    fn open_scope_publishes_only_open_documents() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path();
        fs::write(project.join("tsconfig.json"), "{}").unwrap();
        fake_tsc(
            project,
            "echo \"src/a.ts(1,1): error TS2322: a\"\necho \"src/b.ts(1,1): error TS2322: b\"",
        );
        for name in ["a.ts", "b.ts"] {
            let path = project.join("src").join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "const a: number = 'x';\n").unwrap();
        }
        let a = uri::from_path(&project.join("src/a.ts"));
        let b = uri::from_path(&project.join("src/b.ts"));
        let (tsc, _diagnostics, documents, receiver, settings) = harness(project, TscScope::Open);
        documents.lock().unwrap().open(
            a.clone(),
            crate::documents::Document::new("a".into(), "typescript".into(), 1),
        );
        tsc.schedule(
            project.join("src/a.ts"),
            project.to_path_buf(),
            settings.clone(),
        );
        let params = wait_for_tsc(&receiver, &a, Duration::from_secs(10));
        assert_eq!(params.diagnostics.len(), 1);
        let deadline = Instant::now() + Duration::from_millis(300);
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            if let Ok(lsp_server::Message::Notification(notification)) =
                receiver.recv_timeout(remaining)
                && notification.method == "textDocument/publishDiagnostics"
            {
                let params: lsp_types::PublishDiagnosticsParams =
                    serde_json::from_value(notification.params).unwrap();
                assert_ne!(params.uri, b, "closed document received tsc diagnostics");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn project_scope_publishes_every_file() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path();
        fs::write(project.join("tsconfig.json"), "{}").unwrap();
        fake_tsc(
            project,
            "echo \"src/a.ts(1,1): error TS2322: a\"\necho \"src/b.ts(1,1): error TS2322: b\"",
        );
        for name in ["a.ts", "b.ts"] {
            let path = project.join("src").join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "const a: number = 'x';\n").unwrap();
        }
        let a = uri::from_path(&project.join("src/a.ts"));
        let b = uri::from_path(&project.join("src/b.ts"));
        let (tsc, _diagnostics, _documents, receiver, settings) =
            harness(project, TscScope::Project);
        tsc.schedule(
            project.join("src/a.ts"),
            project.to_path_buf(),
            settings.clone(),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = HashSet::new();
        while seen.len() < 2 {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .expect("timed out waiting for project-scope diagnostics");
            let message = receiver
                .recv_timeout(remaining)
                .expect("diagnostics message");
            let lsp_server::Message::Notification(notification) = message else {
                continue;
            };
            if notification.method != "textDocument/publishDiagnostics" {
                continue;
            }
            let params: lsp_types::PublishDiagnosticsParams =
                serde_json::from_value(notification.params).unwrap();
            if !params.diagnostics.is_empty() {
                seen.insert(params.uri);
            }
        }
        assert!(seen.contains(&a), "{seen:?}");
        assert!(seen.contains(&b), "{seen:?}");
    }
}
