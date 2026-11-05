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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range, Uri};

use crate::diagnostics::Diagnostics;
use crate::documents::Documents;
use crate::settings::{Settings, TscScope};
use crate::uri;
use crate::workspace::Workspace;

const TIMEOUT: Duration = Duration::from_secs(30);
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
/// How long to wait for the stdout/stderr readers after the compiler exited.
/// A grandchild process can keep a pipe open indefinitely; the worker must not
/// be wedged by it.
const PIPE_GRACE: Duration = Duration::from_secs(1);
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
    /// Configs waiting for a check, coalesced per workspace root. Every
    /// schedule adds its nearest config; the worker drains the whole set in
    /// one job before a new job is queued for that root.
    pending: Arc<Mutex<HashMap<PathBuf, HashSet<PathBuf>>>>,
    /// The compiler process the worker currently owns.
    running: Running,
    workspace: Arc<Workspace>,
}

enum Job {
    Check {
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

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum GraphIssue {
    Missing,
    Cycle,
}

#[derive(Default)]
struct State {
    compilers: HashMap<CompilerKey, Option<Compiler>>,
    versions: HashMap<CompilerKey, Option<String>>,
    logged: HashSet<CompilerKey>,
    failures: HashMap<PathBuf, u32>,
    backoff_logged: HashSet<PathBuf>,
    graph_logged: HashSet<(PathBuf, GraphIssue)>,
}

impl State {
    fn clear(&mut self) {
        self.compilers.clear();
        self.versions.clear();
        self.logged.clear();
        self.failures.clear();
        self.backoff_logged.clear();
        self.graph_logged.clear();
    }

    fn retain_root(&mut self, workspace: &Workspace, root: &Path) {
        self.compilers
            .retain(|(project, _), _| !workspace.in_root(project, root));
        self.versions
            .retain(|(project, _), _| !workspace.in_root(project, root));
        self.logged
            .retain(|(project, _)| !workspace.in_root(project, root));
        self.failures
            .retain(|project, _| !workspace.in_root(project, root));
        self.backoff_logged
            .retain(|project| !workspace.in_root(project, root));
        self.graph_logged
            .retain(|(path, _)| !workspace.in_root(path, root));
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

#[derive(Clone, Default)]
struct Running {
    child: Arc<Mutex<Option<Child>>>,
    stopped: Arc<AtomicBool>,
}

impl Running {
    fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    /// Registers a spawned child, killing it if shutdown already started.
    fn start(&self, mut child: Child) -> Result<(), RunError> {
        let mut slot = self.child.lock().unwrap_or_else(|error| error.into_inner());
        if self.is_stopped() {
            drop(slot);
            let _ = child.kill();
            let _ = child.wait();
            return Err(RunError::Shutdown);
        }
        *slot = Some(child);
        Ok(())
    }

    /// Kills the tracked child, if any; later runs are still allowed.
    fn kill(&self) {
        let child = self
            .child
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// Kills the tracked child and refuses any further compiler run.
    fn shutdown(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.kill();
    }
}

pub struct ProjectStatus {
    /// The workspace folder that owns this config.
    pub root: PathBuf,
    /// The directory `tsc -p` runs in.
    pub project: PathBuf,
    pub tsconfig: Option<PathBuf>,
    pub compiler: Option<PathBuf>,
    pub version: Option<String>,
    pub state: &'static str,
}

impl Tsc {
    pub fn new(
        diagnostics: Arc<Diagnostics>,
        documents: Arc<Mutex<Documents>>,
        workspace: Arc<Workspace>,
    ) -> Self {
        let (jobs, receiver) = crossbeam_channel::unbounded();
        let cache = Arc::new(Mutex::new(HashMap::new()));
        let state = Arc::new(Mutex::new(State::default()));
        let generations = Arc::new(Mutex::new(HashMap::new()));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let running = Running::default();
        let cache_for_worker = Arc::clone(&cache);
        let state_for_worker = Arc::clone(&state);
        let generations_for_worker = Arc::clone(&generations);
        let pending_for_worker = Arc::clone(&pending);
        let running_for_worker = running.clone();
        let _ = thread::Builder::new()
            .name("dotfmt-lsp-tsc".into())
            .spawn(move || {
                run(
                    receiver,
                    cache_for_worker,
                    state_for_worker,
                    generations_for_worker,
                    pending_for_worker,
                    running_for_worker,
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
            running,
            workspace,
        }
    }

    /// Queues the config nearest to `path` for its workspace root.
    ///
    /// Every document of a root joins one pending job; the worker expands
    /// project references and runs each config at most once per fingerprint.
    pub fn schedule(&self, path: PathBuf, root: PathBuf, settings: Settings) {
        let Some(tsconfig) = nearest_tsconfig(&path, &root) else {
            return;
        };
        let tsconfig = crate::workspace::normalize(&tsconfig);
        let generation = self.generation_for(&root);
        let first = {
            let mut pending = self
                .pending
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let configs = pending.entry(root.clone()).or_default();
            let first = configs.is_empty();
            configs.insert(tsconfig);
            first
        };
        if !first {
            return;
        }
        let _ = self.jobs.send(Job::Check {
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

    /// Invalidates one workspace folder's configs and compiler state.
    ///
    /// Configs that belong to a nested workspace root keep their cache, in
    /// addition to every other folder's in-flight runs and diagnostics.
    pub fn invalidate_root(&self, root: &Path) {
        self.bump_generation(root);
        self.cache
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|config, _| !self.workspace.in_root(config, root));
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain_root(&self.workspace, root);
        self.pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|pending_root, _| pending_root != root);
        let _ = self.jobs.send(Job::InvalidateRoot(root.to_path_buf()));
    }

    /// Resolved compiler state for `dotfmt/doctor`, one entry per config and
    /// referenced config. May spawn `<tsc> --version` per project.
    pub fn status(&self, roots: &[(PathBuf, Settings)]) -> Vec<ProjectStatus> {
        let mut statuses = Vec::new();
        let mut seen = HashSet::new();
        for (root, settings) in roots {
            let Some(tsconfig) = root_tsconfig(root) else {
                let project = root.clone();
                let compiler = self.compiler_for(&project, settings.tsdk.as_deref());
                let version = compiler.as_ref().and_then(|compiler| {
                    self.version_for(&project, settings.tsdk.as_deref(), compiler)
                });
                let state = if compiler.is_none() {
                    "unavailable"
                } else {
                    "no-tsconfig"
                };
                statuses.push(ProjectStatus {
                    root: root.clone(),
                    project,
                    tsconfig: None,
                    compiler: compiler.map(|compiler| compiler.program),
                    version,
                    state,
                });
                continue;
            };
            for config in config_graph(&[tsconfig]).configs {
                if !seen.insert(config.clone()) {
                    continue;
                }
                let project = config
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| root.clone());
                let compiler = self.compiler_for(&project, settings.tsdk.as_deref());
                let version = compiler.as_ref().and_then(|compiler| {
                    self.version_for(&project, settings.tsdk.as_deref(), compiler)
                });
                let state = if compiler.is_none() {
                    "unavailable"
                } else if self.backed_off(&project) {
                    "backoff"
                } else {
                    "ok"
                };
                statuses.push(ProjectStatus {
                    root: root.clone(),
                    project,
                    tsconfig: Some(config),
                    compiler: compiler.map(|compiler| compiler.program),
                    version,
                    state,
                });
            }
        }
        statuses
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

    /// Stops the worker and kills any in-flight compiler process.
    ///
    /// Called from [`Drop`], so a killed server never leaves a `tsc` child
    /// behind; idempotent and safe to call from another thread.
    pub fn shutdown(&self) {
        self.running.shutdown();
        let _ = self.jobs.send(Job::Shutdown);
    }
}

impl Drop for Tsc {
    fn drop(&mut self) {
        // Kill the child instead of joining the worker: `wait` notices an
        // emptied `Running` slot within one poll interval, so shutdown never
        // waits for `TIMEOUT`.
        self.shutdown();
    }
}

#[allow(clippy::too_many_arguments)]
fn run(
    jobs: crossbeam_channel::Receiver<Job>,
    cache: Arc<Mutex<HashMap<PathBuf, Cache>>>,
    state: Arc<Mutex<State>>,
    generations: Arc<Mutex<HashMap<PathBuf, u64>>>,
    pending: Arc<Mutex<HashMap<PathBuf, HashSet<PathBuf>>>>,
    running: Running,
    documents: Arc<Mutex<Documents>>,
    diagnostics: Arc<Diagnostics>,
) {
    while let Ok(job) = jobs.recv() {
        match job {
            Job::Check {
                root,
                generation: job_generation,
                settings,
            } => {
                let anchors: Vec<PathBuf> = pending
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .remove(&root)
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
                if job_generation != current_generation(&generations, &root) {
                    continue;
                }
                let graph = config_graph(&anchors);
                log_graph_issues(&state, &diagnostics, &root, &graph);
                for config in graph.configs {
                    if running.is_stopped()
                        || job_generation != current_generation(&generations, &root)
                    {
                        break;
                    }
                    check_config(
                        &config,
                        &root,
                        &settings,
                        &cache,
                        &state,
                        &generations,
                        &running,
                        &documents,
                        &diagnostics,
                    );
                }
            }
            Job::Invalidate => {
                diagnostics.clear_tsc();
            }
            Job::InvalidateRoot(root) => {
                diagnostics.clear_tsc_under(root);
            }
            Job::Shutdown => {
                running.shutdown();
                break;
            }
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
fn check_config(
    config: &Path,
    root: &Path,
    settings: &Settings,
    cache: &Mutex<HashMap<PathBuf, Cache>>,
    state: &Mutex<State>,
    generations: &Mutex<HashMap<PathBuf, u64>>,
    running: &Running,
    documents: &Mutex<Documents>,
    diagnostics: &Diagnostics,
) {
    if running.is_stopped() {
        return;
    }
    let project = config.parent().unwrap_or(root);
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
    let fingerprint = fingerprint(config, project, &compiler, settings.tsdk.as_deref());
    let cached = cache
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(config)
        .filter(|entry| entry.fingerprint == fingerprint)
        .map(|entry| entry.diagnostics.clone());
    let entries = match cached {
        Some(entries) => entries,
        None => {
            let Some(lines) = check(project, config, &compiler, state, diagnostics, running) else {
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
                        config.to_path_buf(),
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
    let entries = scoped(config, entries, documents, settings.tsc_scope);
    diagnostics.set_tsc(project, generation_now, entries);
}

/// Every config reachable from the scheduled anchors through `references`.
#[derive(Default)]
struct ConfigGraph {
    configs: Vec<PathBuf>,
    missing: Vec<PathBuf>,
    cycles: Vec<PathBuf>,
}

fn config_graph(anchors: &[PathBuf]) -> ConfigGraph {
    let mut graph = ConfigGraph::default();
    let mut seen = HashSet::new();
    let mut active = HashSet::new();
    for anchor in anchors {
        visit_config(anchor, &mut graph, &mut seen, &mut active);
    }
    graph
}

fn visit_config(
    config: &Path,
    graph: &mut ConfigGraph,
    seen: &mut HashSet<PathBuf>,
    active: &mut HashSet<PathBuf>,
) {
    let config = crate::workspace::normalize(config);
    if active.contains(&config) {
        if !graph.cycles.contains(&config) {
            graph.cycles.push(config);
        }
        return;
    }
    if !seen.insert(config.clone()) {
        return;
    }
    graph.configs.push(config.clone());
    if !config.is_file() {
        graph.missing.push(config);
        return;
    }
    active.insert(config.clone());
    for reference in reference_specs(&config) {
        let path = reference_path(&config, &reference);
        if path.is_file() {
            visit_config(&path, graph, seen, active);
        } else {
            graph.missing.push(path);
        }
    }
    active.remove(&config);
}

/// `references` entries are `{ "path": ... }` objects (and, leniently, strings).
fn reference_specs(config: &Path) -> Vec<String> {
    let Ok(text) = fs::read_to_string(config) else {
        return Vec::new();
    };
    let Ok(Some(jsonc_parser::JsonValue::Object(object))) =
        jsonc_parser::parse_to_value(&text, &Default::default())
    else {
        return Vec::new();
    };
    let Some(references) = object.get_array("references") else {
        return Vec::new();
    };
    references
        .iter()
        .filter_map(|value| match value {
            jsonc_parser::JsonValue::String(path) => Some(path.to_string()),
            jsonc_parser::JsonValue::Object(reference) => {
                reference.get_string("path").map(|path| path.to_string())
            }
            _ => None,
        })
        .collect()
}

/// A reference may point at a tsconfig file or at its directory.
fn reference_path(config: &Path, spec: &str) -> PathBuf {
    let raw = Path::new(spec);
    let directory = config.parent().unwrap_or(Path::new("."));
    let candidate = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        directory.join(raw)
    };
    let resolved = if candidate.is_file() {
        candidate
    } else if candidate.extension().is_none() || candidate.join("tsconfig.json").is_file() {
        candidate.join("tsconfig.json")
    } else {
        candidate
    };
    crate::workspace::normalize(&resolved)
}

fn log_graph_issues(
    state: &Mutex<State>,
    diagnostics: &Diagnostics,
    root: &Path,
    graph: &ConfigGraph,
) {
    let mut messages = Vec::new();
    {
        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
        for path in &graph.missing {
            if state
                .graph_logged
                .insert((path.clone(), GraphIssue::Missing))
            {
                messages.push(format!(
                    "tsc: referenced config {} for {} is missing",
                    path.display(),
                    root.display()
                ));
            }
        }
        for path in &graph.cycles {
            if state.graph_logged.insert((path.clone(), GraphIssue::Cycle)) {
                messages.push(format!(
                    "tsc: project reference cycle at {} for {}",
                    path.display(),
                    root.display()
                ));
            }
        }
    }
    for message in messages {
        diagnostics.log(message);
    }
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
    running: &Running,
) -> Option<Vec<TscLine>> {
    let mut command = compiler.command();
    command
        .arg("--noEmit")
        .arg("--pretty")
        .arg("false")
        .arg("--locale")
        .arg("en")
        .arg("-p")
        .arg(tsconfig)
        .current_dir(project);
    match run_command(command, TIMEOUT, running) {
        Ok(output) => Some(parse_output(&output, project, tsconfig)),
        Err(RunError::Shutdown) => None,
        Err(error) => {
            let message = match error {
                RunError::Spawn(error) => {
                    format!("failed to start ({}): {error}", compiler.program.display())
                }
                RunError::Timeout => {
                    format!("timed out after {}s and was killed", TIMEOUT.as_secs())
                }
                RunError::Shutdown => return None,
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
    Shutdown,
}

fn run_command(
    mut command: Command,
    timeout: Duration,
    running: &Running,
) -> Result<Output, RunError> {
    if running.is_stopped() {
        return Err(RunError::Shutdown);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(RunError::Spawn)?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(RunError::Spawn(std::io::Error::other(
            "missing stdout pipe",
        )));
    };
    let Some(stderr) = child.stderr.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(RunError::Spawn(std::io::Error::other(
            "missing stderr pipe",
        )));
    };
    let stdout = read_pipe(stdout);
    let stderr = read_pipe(stderr);
    running.start(child)?;
    match wait(running, timeout) {
        Ok(_) => {
            let deadline = Instant::now() + PIPE_GRACE;
            Ok(Output {
                stdout: collect(stdout, deadline),
                stderr: collect(stderr, deadline),
            })
        }
        Err(error) => {
            // The child is already killed and reaped; a grandchild may still
            // hold the pipes, so drop them instead of joining the readers.
            running.kill();
            Err(error)
        }
    }
}

/// Drains one pipe on a detached thread and hands the bytes back over a channel.
fn read_pipe<R: Read + Send + 'static>(mut pipe: R) -> crossbeam_channel::Receiver<Vec<u8>> {
    let (sender, receiver) = crossbeam_channel::bounded(1);
    let _ = thread::Builder::new()
        .name("dotfmt-lsp-tsc-pipe".into())
        .spawn(move || {
            let mut buffer = Vec::new();
            let _ = pipe.read_to_end(&mut buffer);
            let _ = sender.send(buffer);
        });
    receiver
}

/// Reads whatever the pipe reader produced before `deadline`.
fn collect(receiver: crossbeam_channel::Receiver<Vec<u8>>, deadline: Instant) -> Vec<u8> {
    receiver
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .unwrap_or_default()
}

fn wait(running: &Running, timeout: Duration) -> Result<ExitStatus, RunError> {
    let started = Instant::now();
    loop {
        {
            let mut slot = running
                .child
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let Some(child) = slot.as_mut() else {
                return Err(RunError::Shutdown);
            };
            match child.try_wait() {
                Ok(Some(status)) => {
                    *slot = None;
                    return Ok(status);
                }
                Ok(None) if started.elapsed() < timeout => {}
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    *slot = None;
                    return Err(RunError::Timeout);
                }
                Err(error) => {
                    *slot = None;
                    return Err(RunError::Spawn(error));
                }
            }
        }
        if running.is_stopped() {
            return Err(RunError::Shutdown);
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn version_of(compiler: &Compiler, project: &Path) -> Option<String> {
    let mut command = compiler.command();
    command.arg("--version").current_dir(project);
    let output = run_command(command, VERSION_TIMEOUT, &Running::default()).ok()?;
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
        for reference in reference_specs(&path) {
            let resolved = reference_path(&path, &reference);
            if resolved.is_file() {
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

    /// A compiler that leaves a long-lived grandchild holding its stdout pipe.
    #[cfg(unix)]
    fn hanging_tsc(project: &Path) -> PathBuf {
        let directory = project.join("node_modules/.bin");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("tsc");
        fs::write(
            &path,
            "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then echo 'Version 9.9.9'; exit 0; fi\n\
             echo $$ > tsc.pid\n\
             sleep 5 &\n\
             echo $! > grandchild.pid\n\
             echo run >> marker.log\n\
             sleep 60\n",
        )
        .unwrap();
        make_executable(&path);
        path
    }

    #[cfg(unix)]
    fn wait_for_pid(path: &Path) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(text) = fs::read_to_string(path)
                && let Ok(pid) = text.trim().parse()
            {
                return pid;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {}",
                path.display()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(unix)]
    fn process_alive(pid: u32) -> bool {
        Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    #[cfg(unix)]
    #[test]
    fn a_timed_out_compiler_does_not_wait_for_its_pipes() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let compiler = hanging_tsc(root);
        let mut command = Command::new(compiler);
        command.current_dir(root);
        let started = Instant::now();
        let result = run_command(command, Duration::from_millis(200), &Running::default());
        assert!(
            matches!(result, Err(RunError::Timeout)),
            "expected a timeout"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the timeout path waited for a grandchild's pipe: {:?}",
            started.elapsed()
        );
    }

    #[cfg(unix)]
    #[test]
    fn shutdown_kills_an_in_flight_compiler() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().to_path_buf();
        let compiler = hanging_tsc(&root);
        let running = Running::default();
        let worker = {
            let running = running.clone();
            let root = root.clone();
            thread::spawn(move || {
                let mut command = Command::new(compiler);
                command.current_dir(root);
                run_command(command, Duration::from_secs(30), &running)
            })
        };
        let pid = wait_for_pid(&root.join("tsc.pid"));
        let started = Instant::now();
        running.shutdown();
        let result = worker.join().unwrap();
        assert!(
            matches!(result, Err(RunError::Shutdown)),
            "expected a shutdown error"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "shutdown waited for the compiler: {:?}",
            started.elapsed()
        );
        assert!(!process_alive(pid), "shutdown left the compiler running");
    }

    #[cfg(unix)]
    #[test]
    fn dropping_the_worker_kills_an_in_flight_compiler() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::write(root.join("tsconfig.json"), "{}").unwrap();
        let source = root.join("src/a.ts");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, "const a: number = 'x';\n").unwrap();
        hanging_tsc(root);
        let (tsc, _diagnostics, _documents, _receiver, settings) = harness(root, TscScope::Project);
        tsc.schedule(source, root.to_path_buf(), settings);
        let pid = wait_for_pid(&root.join("tsc.pid"));
        drop(tsc);
        assert!(!process_alive(pid), "Tsc::drop left the compiler running");
    }

    /// Every config runs in its own project directory, so the fake compiler's
    /// relative marker lands in the config's project.
    #[cfg(unix)]
    fn runs_in(root: &Path) -> usize {
        ["marker.log", "app/marker.log"]
            .into_iter()
            .filter_map(|path| fs::read_to_string(root.join(path)).ok())
            .map(|runs| runs.lines().count())
            .sum()
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
            Arc::clone(&workspace),
            Arc::clone(&documents),
            client,
            current,
            false,
        ));
        let tsc = Arc::new(Tsc::new(
            Arc::clone(&diagnostics),
            Arc::clone(&documents),
            workspace,
        ));
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

    #[test]
    fn reference_graph_resolves_directories_skips_duplicates_and_breaks_cycles() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::write(
            root.join("tsconfig.json"),
            r#"{"files":[],"references":[{"path":"./app"},{"path":"./app/tsconfig.json"},{"path":"./missing"},{"path":"."}]}"#,
        )
        .unwrap();
        fs::create_dir_all(root.join("app")).unwrap();
        fs::write(
            root.join("app/tsconfig.json"),
            r#"{"references":[{"path":".."}]}"#,
        )
        .unwrap();
        let graph = config_graph(&[root.join("tsconfig.json")]);
        assert_eq!(graph.configs.len(), 2, "{:?}", graph.configs);
        assert_eq!(graph.missing.len(), 1, "{:?}", graph.missing);
        assert_eq!(graph.cycles.len(), 1, "{:?}", graph.cycles);
        assert!(graph.missing[0].ends_with("missing/tsconfig.json"));
    }

    #[cfg(unix)]
    #[test]
    fn project_references_run_each_config_once_and_merge_diagnostics() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::write(
            root.join("tsconfig.json"),
            r#"{"files":[],"references":[{"path":"./app"},{"path":"./missing"},{"path":"."}]}"#,
        )
        .unwrap();
        fs::create_dir_all(root.join("app/src")).unwrap();
        fs::write(root.join("app/tsconfig.json"), "{}").unwrap();
        let app_source = root.join("app/src/a.ts");
        fs::write(&app_source, "const value: number = 'x';\n").unwrap();
        let main_source = root.join("src/main.ts");
        fs::create_dir_all(main_source.parent().unwrap()).unwrap();
        fs::write(&main_source, "export const main = 1;\n").unwrap();
        fake_tsc(
            root,
            "case \"$*\" in\n\
             *app/tsconfig.json*) echo \"src/a.ts(1,1): error TS2322: app error\" ;;\n\
             esac",
        );
        let app_uri = uri::from_path(&app_source);
        let (tsc, _diagnostics, _documents, receiver, settings) = harness(root, TscScope::Project);
        tsc.schedule(main_source.clone(), root.to_path_buf(), settings.clone());
        // Missing configs and reference cycles log once, not per document.
        let mut missing = 0;
        let mut cycles = 0;
        let deadline = Instant::now() + Duration::from_secs(10);
        let params = loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .expect("timed out waiting for referenced diagnostics");
            let message = receiver
                .recv_timeout(remaining)
                .expect("diagnostics message");
            let lsp_server::Message::Notification(notification) = message else {
                continue;
            };
            if notification.method == "window/logMessage" {
                let params: lsp_types::LogMessageParams =
                    serde_json::from_value(notification.params).unwrap();
                missing += usize::from(params.message.contains("referenced config"));
                cycles += usize::from(params.message.contains("project reference cycle"));
                continue;
            }
            if notification.method != "textDocument/publishDiagnostics" {
                continue;
            }
            let params: lsp_types::PublishDiagnosticsParams =
                serde_json::from_value(notification.params).unwrap();
            if params.uri == app_uri && !params.diagnostics.is_empty() {
                break params;
            }
        };
        assert_eq!(
            params.diagnostics[0].code,
            Some(NumberOrString::String("TS2322".into()))
        );
        assert_eq!((missing, cycles), (1, 1));
        assert_eq!(
            runs_in(root),
            2,
            "the root and referenced config must each run once"
        );

        // A later schedule of the referenced project reuses its cached run.
        tsc.schedule(app_source, root.to_path_buf(), settings);
        thread::sleep(Duration::from_millis(300));
        assert_eq!(runs_in(root), 2, "a cached referenced config was run again");
        let deadline = Instant::now() + Duration::from_millis(200);
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            let Ok(lsp_server::Message::Notification(notification)) =
                receiver.recv_timeout(remaining)
            else {
                continue;
            };
            assert!(
                notification.method != "window/logMessage"
                    || !notification
                        .params
                        .to_string()
                        .contains("referenced config"),
                "a missing reference was logged twice"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn status_lists_referenced_configs_and_their_compiler_state() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::write(
            root.join("tsconfig.json"),
            r#"{"files":[],"references":[{"path":"./app"}]}"#,
        )
        .unwrap();
        fs::create_dir_all(root.join("app")).unwrap();
        fs::write(root.join("app/tsconfig.json"), "{}").unwrap();
        fake_tsc(root, "");
        let (tsc, _diagnostics, _documents, _receiver, settings) = harness(root, TscScope::Project);
        let statuses = tsc.status(&[(root.to_path_buf(), settings)]);
        assert_eq!(statuses.len(), 2, "{:?}", statuses.len());
        let configs: Vec<&Path> = statuses
            .iter()
            .filter_map(|status| status.tsconfig.as_deref())
            .collect();
        assert_eq!(
            fs::canonicalize(configs[0]).unwrap(),
            fs::canonicalize(root.join("tsconfig.json")).unwrap()
        );
        assert_eq!(
            fs::canonicalize(configs[1]).unwrap(),
            fs::canonicalize(root.join("app/tsconfig.json")).unwrap()
        );
        assert!(statuses.iter().all(|status| status.state == "ok"));
        assert!(
            statuses
                .iter()
                .all(|status| status.version.as_deref() == Some("9.9.9"))
        );
        assert!(
            statuses
                .iter()
                .all(|status| status.compiler.as_deref().is_some())
        );
    }
}
