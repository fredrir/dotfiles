//! Project-wide TypeScript diagnostics from `tsc --noEmit`.
//!
//! The check runs in its own thread so formatting never waits for it. There is
//! one compiler run per project, keyed by a fingerprint of the configuration
//! chain, package metadata and source files. Diagnostics are errors only;
//! unused-code codes are filtered out because dotfmt is a formatter.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use dotfmt_typescript::project::{
    ConfigGraph, config_graph, is_reference_build_diagnostic, referenced_configs,
};
use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range, Uri};

use crate::diagnostics::Diagnostics;
use crate::documents::Documents;
use crate::settings::{Settings, TscScope};
use crate::uri::{self, Key};
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
    ".bun",
    ".bun-cache",
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
    /// A `--version` probe from doctor, tracked separately so it cannot
    /// overwrite the worker's child slot and is still killed on shutdown.
    version_running: Running,
    /// Doctor can run concurrently; serialize the single version child slot.
    version_lock: Mutex<()>,
    /// The latest settings used to schedule each workspace root, so a watched
    /// file can requeue a dependent config without a document notification.
    settings: Arc<Mutex<HashMap<PathBuf, Settings>>>,
    workspace: Arc<Workspace>,
    diagnostics: Arc<Diagnostics>,
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
    /// Directories and files whose changes invalidate this result.
    seeds: Vec<PathBuf>,
    /// The workspace folder that scheduled the config.
    root: PathBuf,
}

/// Compiler discovery depends on the folder's `tsdk`, so the cache key keeps it.
type CompilerKey = (PathBuf, Option<PathBuf>);

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum GraphIssue {
    Missing,
    Cycle,
    Unbuilt,
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
        Self::new_with_timeout(diagnostics, documents, workspace, TIMEOUT)
    }

    fn new_with_timeout(
        diagnostics: Arc<Diagnostics>,
        documents: Arc<Mutex<Documents>>,
        workspace: Arc<Workspace>,
        timeout: Duration,
    ) -> Self {
        let (jobs, receiver) = crossbeam_channel::unbounded();
        let cache = Arc::new(Mutex::new(HashMap::new()));
        let state = Arc::new(Mutex::new(State::default()));
        let generations = Arc::new(Mutex::new(HashMap::new()));
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let settings = Arc::new(Mutex::new(HashMap::new()));
        let running = Running::default();
        let cache_for_worker = Arc::clone(&cache);
        let state_for_worker = Arc::clone(&state);
        let generations_for_worker = Arc::clone(&generations);
        let pending_for_worker = Arc::clone(&pending);
        let running_for_worker = running.clone();
        let diagnostics_for_worker = Arc::clone(&diagnostics);
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
                    diagnostics_for_worker,
                    timeout,
                )
            });
        Self {
            jobs,
            cache,
            state,
            generations,
            pending,
            running,
            version_running: Running::default(),
            version_lock: Mutex::new(()),
            settings,
            workspace,
            diagnostics,
        }
    }

    /// Queues the config nearest to `path` for its workspace root.
    ///
    /// Every document of a root joins one pending job; the worker expands
    /// project references and runs each config at most once per fingerprint.
    pub fn schedule(&self, path: PathBuf, root: PathBuf, settings: Settings) {
        let tsconfigs = nearest_tsconfigs(&path, &root);
        if tsconfigs.is_empty() {
            return;
        }
        self.settings
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(root.clone(), settings.clone());
        let generation = self.generation_for(&root);
        let first = {
            let mut pending = self
                .pending
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let configs = pending.entry(root.clone()).or_default();
            let first = configs.is_empty();
            for tsconfig in tsconfigs {
                configs.insert(crate::workspace::normalize(&tsconfig));
            }
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

    /// Drops every cached result whose dependency seeds contain `path` and
    /// requeues each owning config once. Returns the scheduled workspace roots.
    pub fn invalidate_path(&self, path: &Path) -> Vec<PathBuf> {
        self.invalidate_path_except(path, &HashSet::new())
    }

    /// Like [`Tsc::invalidate_path`], but skips configs owned by `skip` roots.
    ///
    /// A workspace-folder reload already invalidates its own root; watched
    /// config files use this to invalidate only dependents in other folders.
    pub(crate) fn invalidate_path_except(
        &self,
        path: &Path,
        skip: &HashSet<PathBuf>,
    ) -> Vec<PathBuf> {
        let path = crate::workspace::normalize(path);
        let mut affected: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
        {
            let mut cache = self.cache.lock().unwrap_or_else(|error| error.into_inner());
            cache.retain(|config, entry| {
                let matches = entry
                    .seeds
                    .iter()
                    .any(|seed| seed == &path || path.starts_with(seed) || seed.starts_with(&path));
                if matches && !skip.contains(&entry.root) {
                    affected
                        .entry(entry.root.clone())
                        .or_default()
                        .push(config.clone());
                    false
                } else {
                    true
                }
            });
        }
        if affected.is_empty() {
            return Vec::new();
        }
        let settings = self
            .settings
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let mut jobs = Vec::with_capacity(affected.len());
        let mut roots = Vec::with_capacity(affected.len());
        {
            let mut pending = self
                .pending
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            for (root, configs) in affected {
                let generation = self.bump_generation(&root);
                pending.entry(root.clone()).or_default().extend(configs);
                jobs.push((
                    root.clone(),
                    generation,
                    settings.get(&root).cloned().unwrap_or_default(),
                ));
                roots.push(root);
            }
        }
        for (root, generation, settings) in jobs {
            let _ = self.jobs.send(Job::Check {
                root,
                generation,
                settings,
            });
        }
        roots
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
            let tsconfigs = root_tsconfigs(root);
            if tsconfigs.is_empty() {
                let project = root.clone();
                let compiler = self.compiler_for(&project, settings.tsdk.as_deref());
                let version = compiler.as_ref().and_then(|compiler| {
                    self.version_for(&project, settings.tsdk.as_deref(), compiler)
                });
                let state = if compiler.is_none() {
                    self.log_missing_compiler(&project, settings.tsdk.as_deref());
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
            }
            for tsconfig in tsconfigs {
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
                        self.log_missing_compiler(&project, settings.tsdk.as_deref());
                        "unavailable"
                    } else if self.backed_off(&config) {
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
        }
        statuses
    }

    /// Doctor's view of roots the workspace has not trusted: configuration
    /// discovery is static, but no compiler process is spawned.
    pub fn status_untrusted(&self, roots: &[(PathBuf, Settings)]) -> Vec<ProjectStatus> {
        let mut statuses = Vec::new();
        let mut seen = HashSet::new();
        for (root, _) in roots {
            let tsconfigs = root_tsconfigs(root);
            if tsconfigs.is_empty() {
                statuses.push(ProjectStatus {
                    root: root.clone(),
                    project: root.clone(),
                    tsconfig: None,
                    compiler: None,
                    version: None,
                    state: "untrusted",
                });
                continue;
            }
            for tsconfig in tsconfigs {
                for config in config_graph(&[tsconfig]).configs {
                    if !seen.insert(config.clone()) {
                        continue;
                    }
                    let project = config
                        .parent()
                        .map(Path::to_path_buf)
                        .unwrap_or_else(|| root.clone());
                    statuses.push(ProjectStatus {
                        root: root.clone(),
                        project,
                        tsconfig: Some(config),
                        compiler: None,
                        version: None,
                        state: "untrusted",
                    });
                }
            }
        }
        statuses
    }

    fn log_missing_compiler(&self, project: &Path, tsdk: Option<&Path>) {
        let message = missing_compiler_message(project, tsdk);
        let _ = log_once(&self.state, &self.diagnostics, project, tsdk, message);
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
        let _guard = self
            .version_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(version) = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .versions
            .get(&key)
        {
            return version.clone();
        }
        let version = version_of(compiler, project, &self.version_running);
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
        self.version_running.shutdown();
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
    timeout: Duration,
) {
    while let Ok(job) = jobs.recv() {
        match job {
            Job::Check {
                root,
                generation: job_generation,
                settings,
            } => {
                let anchors: Vec<PathBuf> = {
                    let mut pending = pending.lock().unwrap_or_else(|error| error.into_inner());
                    if job_generation != current_generation(&generations, &root) {
                        // A reload bumped the generation and re-added the anchors
                        // for its fresh Check. Do not steal them for a stale job.
                        continue;
                    }
                    pending
                        .remove(&root)
                        .unwrap_or_default()
                        .into_iter()
                        .collect()
                };
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
                        timeout,
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
    timeout: Duration,
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
            missing_compiler_message(project, settings.tsdk.as_deref()),
        );
        cache
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(config);
        diagnostics.set_tsc(config, generation_now, Vec::new());
        return;
    };
    if state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .failures
        .get(config)
        .is_some_and(|failures| *failures >= MAX_FAILURES)
    {
        cache
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(config);
        diagnostics.set_tsc(config, generation_now, Vec::new());
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
                    let version = version_of(&compiler, project, running);
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
    let (fingerprint, seeds) = fingerprint(config, project, &compiler, settings.tsdk.as_deref());
    let cached = cache
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(config)
        .filter(|entry| entry.fingerprint == fingerprint)
        .map(|entry| entry.diagnostics.clone());
    let entries = match cached {
        Some(entries) => entries,
        None => {
            let lines = match check(
                project,
                config,
                &compiler,
                state,
                diagnostics,
                running,
                timeout,
            ) {
                CheckResult::Lines {
                    lines,
                    reference_build,
                } => {
                    if reference_build {
                        log_reference_build(state, diagnostics, config);
                    }
                    lines
                }
                CheckResult::Failed => {
                    cache
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .remove(config);
                    if current_generation(generations, root) == generation_now {
                        diagnostics.set_tsc(config, generation_now, Vec::new());
                    }
                    return;
                }
                CheckResult::Shutdown => return,
            };
            state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .failures
                .remove(config);
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
                            seeds,
                            root: root.to_path_buf(),
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
    diagnostics.set_tsc(config, generation_now, entries);
}

fn log_reference_build(state: &Mutex<State>, diagnostics: &Diagnostics, config: &Path) {
    let first = state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .graph_logged
        .insert((config.to_path_buf(), GraphIssue::Unbuilt));
    if first {
        diagnostics.log(format!(
            "tsc: referenced project not built; run tsc -b for {}",
            config.display()
        ));
    }
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

/// Explains an unavailable compiler without suggesting a silent PATH fallback.
fn missing_compiler_message(project: &Path, tsdk: Option<&Path>) -> String {
    if let Some(environment) = std::env::var_os("DOTFMT_TSC") {
        return format!(
            "tsc: no compiler found for {} (invalid DOTFMT_TSC={})",
            project.display(),
            PathBuf::from(environment).display()
        );
    }
    if let Some(tsdk) = tsdk {
        return format!(
            "tsc: no compiler found for {} (invalid dotfmt.typescript.tsdk={})",
            project.display(),
            tsdk.display()
        );
    }
    format!("tsc: no compiler found for {}", project.display())
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

fn nearest_tsconfigs(file: &Path, root: &Path) -> Vec<PathBuf> {
    let Some(mut directory) = file.parent() else {
        return root_tsconfigs(root);
    };
    loop {
        let configs = dotfmt_typescript::project::configs_in(directory);
        if !configs.is_empty() {
            return configs;
        }
        if directory == root || !directory.starts_with(root) {
            break;
        }
        let Some(parent) = directory.parent() else {
            break;
        };
        directory = parent;
    }
    root_tsconfigs(root)
}

#[cfg(test)]
fn nearest_tsconfig(file: &Path, root: &Path) -> Option<PathBuf> {
    nearest_tsconfigs(file, root).into_iter().next()
}

fn root_tsconfigs(root: &Path) -> Vec<PathBuf> {
    dotfmt_typescript::project::configs_in(root)
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
            let open: HashSet<Key> = documents
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .iter()
                .map(|(uri, _)| uri::key(uri))
                .collect();
            let config = uri::key(&uri::from_path(tsconfig));
            entries
                .into_iter()
                .filter(|(uri, _)| {
                    let key = uri::key(uri);
                    key == config || open.contains(&key)
                })
                .collect()
        }
    }
}

enum CheckResult {
    Lines {
        lines: Vec<TscLine>,
        reference_build: bool,
    },
    Failed,
    Shutdown,
}

#[allow(clippy::too_many_arguments)]
fn check(
    project: &Path,
    tsconfig: &Path,
    compiler: &Compiler,
    state: &Mutex<State>,
    diagnostics: &Diagnostics,
    running: &Running,
    timeout: Duration,
) -> CheckResult {
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
    match run_command(command, timeout, running) {
        Ok(output) => {
            let parsed = parse_output(&output, project, tsconfig);
            CheckResult::Lines {
                lines: parsed.lines,
                reference_build: parsed.reference_build,
            }
        }
        Err(RunError::Shutdown) => CheckResult::Shutdown,
        Err(error) => {
            let message = match error {
                RunError::Spawn(error) => {
                    format!("failed to start ({}): {error}", compiler.program.display())
                }
                RunError::Timeout => {
                    format!("timed out after {:?} and was killed", timeout)
                }
                RunError::Shutdown => return CheckResult::Shutdown,
            };
            let (count, backoff) = {
                let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
                let count = state
                    .failures
                    .entry(tsconfig.to_path_buf())
                    .and_modify(|count| *count += 1)
                    .or_insert(1);
                let count = *count;
                let backoff =
                    count >= MAX_FAILURES && state.backoff_logged.insert(tsconfig.to_path_buf());
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
            CheckResult::Failed
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

fn version_of(compiler: &Compiler, project: &Path, running: &Running) -> Option<String> {
    let mut command = compiler.command();
    command.arg("--version").current_dir(project);
    let output = run_command(command, VERSION_TIMEOUT, running).ok()?;
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

struct ParsedOutput {
    lines: Vec<TscLine>,
    /// TS6305/TS6310 were present and filtered.
    reference_build: bool,
}

fn parse_output(output: &Output, project: &Path, tsconfig: &Path) -> ParsedOutput {
    let mut lines = Vec::new();
    let mut reference_build = false;
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
            // handled by dotfmt's own fix pipeline. Unbuilt referenced projects
            // are reported once as a configuration message, not per source.
            if line.severity == DiagnosticSeverity::ERROR
                && is_reference_build_diagnostic(line.number())
            {
                reference_build = true;
                continuation = false;
                continue;
            }
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
    ParsedOutput {
        lines,
        reference_build,
    }
}

fn group(
    project: &Path,
    lines: Vec<TscLine>,
    diagnostics: &Diagnostics,
) -> Vec<(Uri, Vec<Diagnostic>)> {
    let mut grouped: HashMap<Key, (Uri, Vec<Diagnostic>)> = HashMap::new();
    for line in lines {
        let path = line.file.strip_prefix(project).unwrap_or(&line.file);
        let absolute = if (line.file.is_absolute() || windows_absolute(&line.file))
            && !path.starts_with(project)
        {
            line.file.clone()
        } else {
            project.join(path)
        };
        let key = uri::key(&uri::from_path(&crate::workspace::lexical(&absolute)));
        grouped
            .entry(key.clone())
            .or_insert_with(|| (key.uri(), Vec::new()))
            .1
            .push(line.diagnostic());
    }
    grouped
        .into_values()
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
        for resolved in referenced_configs(&path) {
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

fn fingerprint(
    tsconfig: &Path,
    project: &Path,
    compiler: &Compiler,
    tsdk: Option<&Path>,
) -> (u64, Vec<PathBuf>) {
    fingerprint_with_limit(tsconfig, project, compiler, tsdk, SCAN_LIMIT)
}

fn fingerprint_with_limit(
    tsconfig: &Path,
    project: &Path,
    compiler: &Compiler,
    tsdk: Option<&Path>,
    limit: usize,
) -> (u64, Vec<PathBuf>) {
    let mut inputs = FingerprintInputs::default();
    analyze_inputs(tsconfig, project, &mut inputs);
    let seeds: Vec<PathBuf> = inputs.seeds.iter().cloned().collect();

    let mut hasher = DefaultHasher::new();
    compiler.hash(&mut hasher);
    tsdk.hash(&mut hasher);
    hash_metadata(&mut hasher, &compiler.program);
    for seed in &inputs.seeds {
        seed.hash(&mut hasher);
    }
    for file in &inputs.content {
        hash_content(&mut hasher, file);
    }
    let mut scanner = Scanner::new(&inputs.content, limit);
    for root in &inputs.roots {
        scanner.scan(root);
    }
    scanner.hash(&mut hasher, &inputs.content);
    (hasher.finish(), seeds)
}

#[derive(Default)]
struct FingerprintInputs {
    /// Files whose contents are hashed and whose metadata is ignored.
    content: BTreeSet<PathBuf>,
    /// Directories walked for non-seed project files.
    roots: BTreeSet<PathBuf>,
    /// Every file and directory that makes a cached result depend on a path.
    seeds: BTreeSet<PathBuf>,
}

impl FingerprintInputs {
    fn content_file(&mut self, path: &Path) {
        let path = crate::workspace::normalize(path);
        self.content.insert(path.clone());
        self.seeds.insert(path);
    }

    fn root(&mut self, path: &Path) {
        let path = crate::workspace::normalize(path);
        self.roots.insert(path.clone());
        self.seeds.insert(path);
    }
}

fn analyze_inputs(tsconfig: &Path, project: &Path, inputs: &mut FingerprintInputs) {
    let project = crate::workspace::normalize(project);
    inputs.root(&project);
    let config = crate::workspace::normalize(tsconfig);
    inputs.content_file(&config);
    let chain = config_chain(tsconfig);
    for config in &chain {
        inputs.content_file(config);
        if let Some(directory) = config.parent() {
            inputs.root(directory);
        }
    }

    let mut manifests = BTreeSet::new();
    manifests.insert(project.clone());
    if let Some(root) = dotfmt_typescript::project::discover_root(&project) {
        manifests.insert(crate::workspace::normalize(&root));
    }
    for config in &chain {
        if let Some(directory) = config.parent() {
            manifests.insert(crate::workspace::normalize(directory));
        }
    }
    for directory in &manifests {
        inputs.content_file(&directory.join("package.json"));
        for lockfile in LOCKFILES {
            inputs.content_file(&directory.join(lockfile));
        }
    }

    for config in &chain {
        analyze_config(config, inputs);
    }
    analyze_dependencies(&project, &manifests, inputs);
}

fn analyze_config(config: &Path, inputs: &mut FingerprintInputs) {
    let directory = config.parent().unwrap_or(Path::new("."));
    let Ok(text) = fs::read_to_string(config) else {
        return;
    };
    let Ok(Some(jsonc_parser::JsonValue::Object(object))) =
        jsonc_parser::parse_to_value(&text, &Default::default())
    else {
        return;
    };
    let options = object.get_object("compilerOptions");
    let base = options
        .as_ref()
        .and_then(|options| options.get_string("baseUrl"))
        .map_or_else(
            || directory.to_path_buf(),
            |base_url| expand_config_path(directory, base_url),
        );
    if let Some(options) = options
        && let Some(paths) = options.get_object("paths")
    {
        for (_, targets) in paths.clone().into_iter() {
            let jsonc_parser::JsonValue::Array(targets) = targets else {
                continue;
            };
            for target in targets {
                let jsonc_parser::JsonValue::String(target) = target else {
                    continue;
                };
                add_alias_target(inputs, &base, directory, &target);
            }
        }
    }
    for reference in referenced_configs(config) {
        inputs.content_file(&reference);
        if let Some(directory) = reference.parent() {
            inputs.root(directory);
        }
    }
}

fn expand_config_path(directory: &Path, value: &str) -> PathBuf {
    let expanded = value.replace("${configDir}", &directory.to_string_lossy());
    let path = Path::new(&expanded);
    if path.is_absolute() {
        crate::workspace::normalize(path)
    } else {
        crate::workspace::normalize(&directory.join(path))
    }
}

fn add_alias_target(inputs: &mut FingerprintInputs, base: &Path, config_dir: &Path, target: &str) {
    let expanded = target.replace("${configDir}", &config_dir.to_string_lossy());
    let raw = Path::new(&expanded);
    let target = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        base.join(raw)
    };
    let text = target.to_string_lossy();
    if let Some(index) = text.find('*') {
        let prefix = &text[..index];
        let prefix_path = Path::new(prefix);
        let root = if prefix.ends_with('/') || prefix.ends_with(std::path::MAIN_SEPARATOR) {
            prefix_path.to_path_buf()
        } else {
            prefix_path.parent().unwrap_or(prefix_path).to_path_buf()
        };
        let root = if root.as_os_str().is_empty() {
            base.to_path_buf()
        } else {
            root
        };
        inputs.root(&root);
        return;
    }
    let target = crate::workspace::normalize(&target);
    if target.is_dir() {
        inputs.root(&target);
        return;
    }
    if target.is_file() || target.extension().is_some() {
        inputs.content_file(&target);
    } else {
        inputs.root(&target);
    }
    if let Some(parent) = target.parent() {
        inputs.root(parent);
    }
}

fn analyze_dependencies(
    project: &Path,
    manifests: &BTreeSet<PathBuf>,
    inputs: &mut FingerprintInputs,
) {
    let workspace_root = dotfmt_typescript::project::discover_root(project)
        .map(|root| crate::workspace::normalize(&root))
        .unwrap_or_else(|| project.to_path_buf());
    let mut dependencies = BTreeSet::new();
    let mut patterns = Vec::new();
    for directory in manifests {
        let Some(package) = read_package(&directory.join("package.json")) else {
            continue;
        };
        for key in [
            "dependencies",
            "devDependencies",
            "peerDependencies",
            "optionalDependencies",
        ] {
            if let Some(values) = package.get(key).and_then(serde_json::Value::as_object) {
                dependencies.extend(values.keys().cloned());
            }
        }
        workspace_patterns(&package, &mut patterns);
    }
    if let Some(package) = read_package(&workspace_root.join("package.json")) {
        workspace_patterns(&package, &mut patterns);
    }
    patterns.sort();
    patterns.dedup();

    let mut members = None;
    for dependency in dependencies {
        let mut linked = false;
        let mut directory = Some(project);
        let mut depth = 0;
        while let Some(current) = directory {
            if depth > 16 {
                break;
            }
            let declared = current.join("node_modules").join(&dependency);
            if symlink(&declared)
                && let Ok(target) = fs::canonicalize(&declared)
            {
                add_dependency_target(inputs, &declared, &target);
                linked = true;
                break;
            }
            directory = current.parent();
            depth += 1;
        }
        if linked {
            continue;
        }
        let members = members.get_or_insert_with(|| {
            workspace_members(&workspace_root, &patterns)
                .into_iter()
                .map(|path| {
                    let name = read_package(&path.join("package.json"))
                        .and_then(|package| {
                            package
                                .get("name")
                                .and_then(serde_json::Value::as_str)
                                .map(str::to_owned)
                        })
                        .unwrap_or_default();
                    (name, path)
                })
                .collect::<Vec<_>>()
        });
        if let Some((_, target)) = members.iter().find(|(name, _)| name == &dependency) {
            add_dependency_target(inputs, target, target);
        }
    }
}

fn symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
}

fn add_dependency_target(inputs: &mut FingerprintInputs, declared: &Path, target: &Path) {
    let target = crate::workspace::normalize(target);
    inputs.seeds.insert(crate::workspace::normalize(declared));
    inputs.root(&target);
    let package_path = target.join("package.json");
    inputs.content_file(&package_path);
    let Some(package) = read_package(&package_path) else {
        return;
    };
    for key in ["types", "typings", "module", "main"] {
        if let Some(entry) = package.get(key).and_then(serde_json::Value::as_str) {
            let entry = target.join(entry);
            if entry.is_file() {
                inputs.content_file(&entry);
            }
        }
    }
    let mut entries = Vec::new();
    collect_package_entries(package.get("exports"), &mut entries);
    for entry in entries {
        if entry.contains('*') || !entry.starts_with('.') {
            continue;
        }
        let entry = target.join(entry.trim_start_matches("./"));
        if entry.is_file() {
            inputs.content_file(&entry);
        }
    }
}

fn collect_package_entries(value: Option<&serde_json::Value>, entries: &mut Vec<String>) {
    match value {
        Some(serde_json::Value::String(value)) => entries.push(value.clone()),
        Some(serde_json::Value::Array(values)) => {
            for value in values {
                collect_package_entries(Some(value), entries);
            }
        }
        Some(serde_json::Value::Object(values)) => {
            for value in values.values() {
                collect_package_entries(Some(value), entries);
            }
        }
        _ => {}
    }
}

fn read_package(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

fn workspace_patterns(package: &serde_json::Value, patterns: &mut Vec<String>) {
    let Some(workspaces) = package.get("workspaces") else {
        return;
    };
    let values = workspaces.as_array().or_else(|| {
        workspaces
            .get("packages")
            .and_then(serde_json::Value::as_array)
    });
    let Some(values) = values else {
        return;
    };
    patterns.extend(
        values
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_owned),
    );
}

fn workspace_members(root: &Path, patterns: &[String]) -> Vec<PathBuf> {
    let mut members = BTreeSet::new();
    for pattern in patterns {
        for member in expand_workspace_pattern(root, pattern) {
            members.insert(member);
        }
    }
    members.into_iter().collect()
}

fn expand_workspace_pattern(root: &Path, pattern: &str) -> Vec<PathBuf> {
    let pattern = pattern
        .trim()
        .trim_start_matches("./")
        .trim_end_matches('/');
    if pattern.is_empty() || pattern.starts_with('!') {
        return Vec::new();
    }
    let mut current = vec![root.to_path_buf()];
    for segment in pattern.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        let mut next = Vec::new();
        for directory in current {
            if segment == "**" {
                collect_directories(&directory, 0, &mut next);
            } else if segment.contains('*') {
                let Ok(entries) = fs::read_dir(&directory) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let path = entry.path();
                    if path.is_dir()
                        && !SKIP.contains(&name.as_str())
                        && wildcard_component(segment, &name)
                    {
                        next.push(path);
                    }
                }
            } else {
                let path = directory.join(segment);
                if path.is_dir() {
                    next.push(path);
                }
            }
        }
        current = next;
        if current.len() > 4096 {
            current.truncate(4096);
        }
    }
    current
}

fn collect_directories(directory: &Path, depth: usize, output: &mut Vec<PathBuf>) {
    if depth > 8 || output.len() >= 4096 {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if path.is_dir() && !SKIP.contains(&name.as_str()) {
            output.push(path.clone());
            collect_directories(&path, depth + 1, output);
        }
    }
}

fn wildcard_component(pattern: &str, value: &str) -> bool {
    let mut pattern = pattern.chars().peekable();
    let mut value = value.chars().peekable();
    let mut star = None;
    let mut retry = None;
    while value.peek().is_some() {
        match pattern.peek() {
            Some('*') => {
                pattern.next();
                star = Some(pattern.clone());
                retry = Some(value.clone());
            }
            Some(expected) if Some(*expected) == value.peek().copied() => {
                pattern.next();
                value.next();
            }
            _ => {
                let Some(saved) = star.as_mut() else {
                    return false;
                };
                pattern = saved.clone();
                let Some(retry_value) = retry.as_mut() else {
                    return false;
                };
                retry_value.next();
                value = retry_value.clone();
            }
        }
    }
    pattern.all(|character| character == '*')
}

struct Scanner<'a> {
    content: &'a BTreeSet<PathBuf>,
    visited: HashSet<PathBuf>,
    files: BTreeMap<PathBuf, bool>,
    seen: usize,
    limit: usize,
}

impl<'a> Scanner<'a> {
    fn new(content: &'a BTreeSet<PathBuf>, limit: usize) -> Self {
        Self {
            content,
            visited: HashSet::new(),
            files: BTreeMap::new(),
            seen: 0,
            limit,
        }
    }

    fn scan(&mut self, path: &Path) {
        if self.seen >= self.limit {
            return;
        }
        let path = crate::workspace::normalize(path);
        let Ok(metadata) = fs::metadata(&path) else {
            return;
        };
        if metadata.is_file() {
            self.seen += 1;
            self.add_file(path);
        } else if metadata.is_dir()
            && let Ok(canonical) = fs::canonicalize(&path)
        {
            self.walk(&canonical);
        }
    }

    fn walk(&mut self, directory: &Path) {
        if self.seen >= self.limit {
            return;
        }
        let Ok(canonical) = fs::canonicalize(directory) else {
            return;
        };
        if !self.visited.insert(canonical.clone()) {
            return;
        }
        let Ok(entries) = fs::read_dir(&canonical) else {
            return;
        };
        for entry in entries.flatten() {
            if self.seen >= self.limit {
                return;
            }
            self.seen += 1;
            let path = entry.path();
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !SKIP.contains(&name.as_str()) {
                    self.walk(&path);
                }
            } else if metadata.is_file() && relevant(&path) {
                self.add_file(path);
            }
        }
    }

    fn add_file(&mut self, path: PathBuf) {
        let path = crate::workspace::normalize(&path);
        if self.files.contains_key(&path) {
            return;
        }
        let content = self.content.contains(&path) || content_name(&path);
        self.files.insert(path, content);
    }

    fn hash(&self, hasher: &mut DefaultHasher, explicit: &BTreeSet<PathBuf>) {
        for (path, content) in &self.files {
            if *content {
                if !explicit.contains(path) {
                    hash_content(hasher, path);
                }
            } else {
                hash_metadata(hasher, path);
            }
        }
    }
}

fn content_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    name == "package.json"
        || LOCKFILES.contains(&name)
        || ((name.starts_with("tsconfig") || name.starts_with("jsconfig"))
            && path.extension().and_then(OsStr::to_str) == Some("json"))
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

fn hash_content(hasher: &mut DefaultHasher, path: &Path) {
    path.hash(hasher);
    match fs::read(path) {
        Ok(bytes) => {
            bytes.len().hash(hasher);
            bytes.hash(hasher);
        }
        Err(_) => 0u8.hash(hasher),
    }
}

fn hash_metadata(hasher: &mut DefaultHasher, path: &Path) {
    path.hash(hasher);
    let Ok(metadata) = fs::metadata(path) else {
        0u8.hash(hasher);
        return;
    };
    metadata.len().hash(hasher);
    if let Ok(modified) = metadata.modified() {
        modified
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()
            .hash(hasher);
    }
    #[cfg(unix)]
    {
        metadata.ino().hash(hasher);
        metadata.ctime().hash(hasher);
        metadata.ctime_nsec().hash(hasher);
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
    // An explicit compiler is authoritative. If it is invalid, report the
    // compiler as unavailable instead of silently using a different one.
    if let Some(environment) = environment {
        return compiler_at(environment);
    }
    if let Some(tsdk) = tsdk {
        return compiler_at(tsdk);
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
        let parsed = parse_output(&output, Path::new("/p"), Path::new("/p/tsconfig.json"));
        let lines = parsed.lines;
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].file, Path::new("/p/tsconfig.json"));
        assert_eq!(lines[0].message, "Option 'x' is invalid.\nTry changing y.");
    }

    #[test]
    fn unused_codes_warnings_and_reference_build_errors_are_filtered() {
        let output = Output {
            stdout: b"src/a.ts(1,1): error TS6133: unused\n\
                      src/a.ts(2,1): error TS7027: unreachable\n\
                      src/a.ts(3,1): error TS7028: unused ignore\n\
                      src/a.ts(4,1): warning TS9999: warn\n\
                      src/a.ts(5,1): error TS6305: not built\n\
                      src/a.ts(6,1): error TS6310: may not disable emit\n\
                      src/a.ts(7,1): error TS2322: real\n"
                .to_vec(),
            stderr: Vec::new(),
        };
        let parsed = parse_output(&output, Path::new("/p"), Path::new("/p/tsconfig.json"));
        let lines = parsed.lines;
        assert!(parsed.reference_build);
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
    fn nonstandard_configs_are_discovered_like_the_language_crate() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path();
        fs::create_dir_all(project.join("src")).unwrap();
        fs::write(project.join("src/a.ts"), "").unwrap();
        fs::write(project.join("tsconfig.router.json"), "{}").unwrap();
        let source = project.join("src/a.ts");
        assert_eq!(
            nearest_tsconfigs(&source, project),
            vec![project.join("tsconfig.router.json")]
        );
        assert_eq!(
            root_tsconfigs(project),
            vec![project.join("tsconfig.router.json")]
        );
        assert_eq!(
            dotfmt_typescript::project::tsconfigs_for(&source),
            nearest_tsconfigs(&source, project),
            "the CLI and LSP must discover the same configs"
        );

        fs::write(project.join("tsconfig.json"), "{}").unwrap();
        assert_eq!(
            nearest_tsconfigs(&source, project),
            vec![project.join("tsconfig.json")]
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
    fn invalid_explicit_compilers_do_not_fall_back_to_path() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path();
        let path_bin = project.join("bin");
        fs::create_dir_all(&path_bin).unwrap();
        let path_tsc = path_bin.join("tsc");
        fs::write(&path_tsc, "").unwrap();
        make_executable(&path_tsc);
        let missing = project.join("missing-typescript");

        assert!(
            discover_with(project, None, Some(&missing), Some(path_bin.as_os_str())).is_none(),
            "an invalid DOTFMT_TSC fell back to PATH"
        );
        assert!(
            discover_with(project, Some(&missing), None, Some(path_bin.as_os_str())).is_none(),
            "an invalid tsdk fell back to PATH"
        );
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
        assert_ne!(before.0, after.0);
        assert!(before.1.contains(&project.join("tsconfig.json")));
        assert_eq!(config_chain(&config).len(), 2);
    }

    fn compiler_at_for_fingerprint(project: &Path) -> Compiler {
        Compiler {
            program: project.join("tsc"),
            node: false,
            cmd: false,
        }
    }

    fn set_modified(path: &Path, modified: SystemTime) {
        let file = fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
    }

    #[test]
    fn fingerprint_tracks_path_alias_sources() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let app = root.join("app");
        let lib = root.join("lib/src");
        fs::create_dir_all(app.join("src")).unwrap();
        fs::create_dir_all(&lib).unwrap();
        fs::write(
            app.join("tsconfig.json"),
            r#"{"compilerOptions":{"paths":{"@lib":["../lib/src/index.ts"]}}}"#,
        )
        .unwrap();
        fs::write(app.join("src/main.ts"), "export const a = 1;\n").unwrap();
        let entry = lib.join("index.ts");
        fs::write(&entry, "export const value = 1;\n").unwrap();
        fs::write(lib.join("other.ts"), "export const other = 1;\n").unwrap();
        let config = app.join("tsconfig.json");
        let before = fingerprint(&config, &app, &compiler_at_for_fingerprint(&app), None);
        assert!(
            before.1.contains(&crate::workspace::normalize(&entry)),
            "{:?}",
            before.1
        );
        fs::write(&entry, "export const value = 'x';\n").unwrap();
        let changed = fingerprint(&config, &app, &compiler_at_for_fingerprint(&app), None);
        assert_ne!(before.0, changed.0);
        fs::write(lib.join("other.ts"), "export const other = 'x';\n").unwrap();
        let sibling = fingerprint(&config, &app, &compiler_at_for_fingerprint(&app), None);
        assert_ne!(changed.0, sibling.0);
    }

    #[cfg(unix)]
    #[test]
    fn fingerprint_tracks_bun_workspace_symlinks() {
        use std::os::unix::fs::symlink;

        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let app = root.join("apps/web");
        let ui = root.join("packages/ui");
        fs::create_dir_all(app.join("src")).unwrap();
        fs::create_dir_all(ui.join("src")).unwrap();
        fs::write(
            root.join("package.json"),
            r#"{"private":true,"workspaces":["apps/*","packages/*"]}"#,
        )
        .unwrap();
        fs::write(
            app.join("package.json"),
            r#"{"dependencies":{"@repo/ui":"workspace:*"}}"#,
        )
        .unwrap();
        fs::write(
            ui.join("package.json"),
            r#"{"name":"@repo/ui","types":"src/index.ts"}"#,
        )
        .unwrap();
        fs::write(app.join("tsconfig.json"), "{}").unwrap();
        fs::write(
            app.join("src/main.ts"),
            "import { label } from '@repo/ui';\n",
        )
        .unwrap();
        let entry = ui.join("src/index.ts");
        fs::write(&entry, "export const label = 1;\n").unwrap();
        fs::create_dir_all(root.join("node_modules/@repo")).unwrap();
        symlink("../../packages/ui", root.join("node_modules/@repo/ui")).unwrap();

        let config = app.join("tsconfig.json");
        let before = fingerprint(&config, &app, &compiler_at_for_fingerprint(&app), None);
        assert!(
            before.1.contains(&crate::workspace::normalize(&ui)),
            "{:?}",
            before.1
        );
        fs::write(&entry, "export const label = 'x';\n").unwrap();
        let changed = fingerprint(&config, &app, &compiler_at_for_fingerprint(&app), None);
        assert_ne!(before.0, changed.0);
    }

    #[test]
    fn fingerprint_tracks_referenced_project_sources() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let app = root.join("app");
        fs::create_dir_all(app.join("src")).unwrap();
        fs::write(
            root.join("tsconfig.json"),
            r#"{"files":[],"references":[{"path":"./app"}]}"#,
        )
        .unwrap();
        fs::write(app.join("tsconfig.json"), "{}").unwrap();
        let source = app.join("src/a.ts");
        fs::write(&source, "export const value = 1;\n").unwrap();
        let config = root.join("tsconfig.json");
        let before = fingerprint(&config, root, &compiler_at_for_fingerprint(root), None);
        assert!(
            before.1.contains(&crate::workspace::normalize(&app)),
            "{:?}",
            before.1
        );
        fs::write(&source, "export const value = 'x';\n").unwrap();
        let after = fingerprint(&config, root, &compiler_at_for_fingerprint(root), None);
        assert_ne!(before.0, after.0);
    }

    #[cfg(unix)]
    #[test]
    fn same_size_same_mtime_rewrite_changes_the_fingerprint() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path();
        fs::write(project.join("tsconfig.json"), "{}").unwrap();
        let source = project.join("src/a.ts");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, "const a = 1;\n").unwrap();
        let config = project.join("tsconfig.json");
        let compiler = compiler_at_for_fingerprint(project);
        let before = fingerprint(&config, project, &compiler, None);
        let modified = fs::metadata(&source).unwrap().modified().unwrap();
        fs::write(&source, "const a = 2;\n").unwrap();
        set_modified(&source, modified);
        let after = fingerprint(&config, project, &compiler, None);
        assert_ne!(before.0, after.0);
    }

    #[test]
    fn content_preserving_touch_of_a_seed_keeps_the_fingerprint() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let app = root.join("app");
        let lib = root.join("lib/src");
        fs::create_dir_all(app.join("src")).unwrap();
        fs::create_dir_all(&lib).unwrap();
        let config = app.join("tsconfig.json");
        fs::write(
            &config,
            r#"{"compilerOptions":{"paths":{"@lib":["../lib/src/index.ts"]}}}"#,
        )
        .unwrap();
        let package = app.join("package.json");
        fs::write(&package, r#"{"name":"app"}"#).unwrap();
        let entry = lib.join("index.ts");
        fs::write(&entry, "export const value = 1;\n").unwrap();
        let compiler = compiler_at_for_fingerprint(&app);
        let before = fingerprint(&config, &app, &compiler, None);
        let future = SystemTime::now() + Duration::from_secs(60);
        for path in [&config, &package, &entry] {
            set_modified(path, future);
        }
        let after = fingerprint(&config, &app, &compiler, None);
        assert_eq!(before.0, after.0, "{:?}", before.1);
    }

    #[test]
    fn fingerprint_walk_is_cycle_safe() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let app = root.join("app");
        fs::create_dir_all(app.join("src")).unwrap();
        fs::write(
            root.join("tsconfig.json"),
            r#"{"files":[],"references":[{"path":"./app"}],"compilerOptions":{"paths":{"@root/*":["app/src/*"]}}}"#,
        )
        .unwrap();
        fs::write(
            app.join("tsconfig.json"),
            r#"{"references":[{"path":".."}],"compilerOptions":{"paths":{"@app/*":["../app/src/*"]}}}"#,
        )
        .unwrap();
        fs::write(app.join("src/a.ts"), "export const a = 1;\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("..", app.join("src/loop")).unwrap();
        let config = root.join("tsconfig.json");
        let compiler = compiler_at_for_fingerprint(root);
        let first = fingerprint(&config, root, &compiler, None);
        let second = fingerprint(&config, root, &compiler, None);
        assert_eq!(first.0, second.0);
    }

    #[test]
    fn scan_budget_bounds_the_walk() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        for index in 0..20 {
            fs::write(root.join(format!("file{index}.ts")), "export {};\n").unwrap();
        }
        let content = BTreeSet::new();
        let mut scanner = Scanner::new(&content, 3);
        scanner.scan(root);
        assert!(scanner.seen <= 3, "{}", scanner.seen);
        assert!(scanner.files.len() <= 3, "{}", scanner.files.len());
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
    fn make_non_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o644);
        fs::set_permissions(path, permissions).unwrap();
    }

    #[cfg(unix)]
    fn wait_for_runs(root: &Path, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if runs_in(root) >= expected {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {expected} compiler runs"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(unix)]
    fn wait_for_empty(diagnostics: &Diagnostics, uri: &Uri) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if diagnostics.merged(uri, None).diagnostics.is_empty() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for cleared diagnostics"
            );
            thread::sleep(Duration::from_millis(20));
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

    /// A stable compiler whose result switches with marker files, so the
    /// fingerprint does not change when a test simulates a failure.
    #[cfg(unix)]
    fn switchable_tsc(project: &Path) -> PathBuf {
        let directory = project.join("node_modules/.bin");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("tsc");
        fs::write(
            &path,
            "#!/bin/sh
             if [ \"$1\" = \"--version\" ]; then echo 'Version 9.9.9'; exit 0; fi
             echo run >> marker.log
             if [ -f mode_hang ]; then sleep 60; exit 0; fi
             if [ -f mode_new ]; then
               echo \"src/a.ts(1,1): error TS2322: new\"
             else
               echo \"src/a.ts(1,1): error TS2322: old\"
             fi
",
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
        harness_with(project, scope, None, TIMEOUT)
    }

    #[cfg(unix)]
    #[allow(clippy::type_complexity)]
    fn harness_with(
        project: &Path,
        scope: TscScope,
        tsdk: Option<PathBuf>,
        timeout: Duration,
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
            tsdk,
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
        let tsc = Arc::new(Tsc::new_with_timeout(
            Arc::clone(&diagnostics),
            Arc::clone(&documents),
            workspace,
            timeout,
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
    fn open_scope_matches_route_group_spellings_by_identity() {
        let temporary = tempfile::tempdir().unwrap();
        let project = temporary.path();
        fs::write(project.join("tsconfig.json"), "{}").unwrap();
        let source = project.join("app/(dashboard)/page.tsx");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, "const value: number = 'x';\n").unwrap();
        fake_tsc(
            project,
            "echo \"app/(dashboard)/page.tsx(1,7): error TS2322: route group\"",
        );
        let client_uri = Uri::from_str(&format!(
            "file://{}/app/%28dashboard%29/page.tsx",
            project.display()
        ))
        .unwrap();
        let (tsc, _diagnostics, documents, receiver, settings) = harness(project, TscScope::Open);
        documents.lock().unwrap().open(
            client_uri.clone(),
            crate::documents::Document::new("x".into(), "typescript".into(), 1),
        );
        tsc.schedule(source, project.to_path_buf(), settings);
        let params = wait_for_tsc(&receiver, &client_uri, Duration::from_secs(10));
        assert_eq!(
            params.diagnostics[0].code,
            Some(NumberOrString::String("TS2322".into()))
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_timeout_clears_stale_diagnostics_and_the_cache() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::write(root.join("tsconfig.json"), "{}").unwrap();
        let source = root.join("src/a.ts");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        let original = "const a: number = 'x';\n";
        fs::write(&source, original).unwrap();
        switchable_tsc(root);
        let uri = uri::from_path(&source);
        let (tsc, diagnostics, _documents, receiver, settings) =
            harness_with(root, TscScope::Project, None, Duration::from_millis(200));
        tsc.schedule(source.clone(), root.to_path_buf(), settings.clone());
        let params = wait_for_tsc(&receiver, &uri, Duration::from_secs(10));
        assert!(params.diagnostics[0].message.contains("old"));

        fs::write(root.join("mode_hang"), "").unwrap();
        fs::write(&source, "const a: number = 'y';\n").unwrap();
        tsc.schedule(source.clone(), root.to_path_buf(), settings.clone());
        wait_for_empty(&diagnostics, &uri);

        fs::remove_file(root.join("mode_hang")).unwrap();
        fs::write(root.join("mode_new"), "").unwrap();
        fs::write(&source, original).unwrap();
        tsc.schedule(source, root.to_path_buf(), settings);
        let params = wait_for_tsc(&receiver, &uri, Duration::from_secs(10));
        assert!(
            params.diagnostics[0].message.contains("new"),
            "a stale cached result replayed after a timeout: {:?}",
            params.diagnostics
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_spawn_failure_clears_stale_diagnostics_and_the_cache() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::write(root.join("tsconfig.json"), "{}").unwrap();
        let source = root.join("src/a.ts");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        let original = "const a: number = 'x';\n";
        fs::write(&source, original).unwrap();
        let compiler = switchable_tsc(root);
        let uri = uri::from_path(&source);
        let (tsc, diagnostics, _documents, receiver, settings) =
            harness_with(root, TscScope::Project, None, Duration::from_secs(5));
        tsc.schedule(source.clone(), root.to_path_buf(), settings.clone());
        wait_for_tsc(&receiver, &uri, Duration::from_secs(10));
        assert_eq!(runs_in(root), 1);

        make_non_executable(&compiler);
        fs::write(&source, "const a: number = 'y';\n").unwrap();
        tsc.schedule(source.clone(), root.to_path_buf(), settings.clone());
        wait_for_empty(&diagnostics, &uri);

        make_executable(&compiler);
        fs::write(&source, original).unwrap();
        tsc.schedule(source, root.to_path_buf(), settings);
        wait_for_tsc(&receiver, &uri, Duration::from_secs(10));
        assert_eq!(
            runs_in(root),
            2,
            "the cache survived a spawn failure and was replayed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn invalidate_path_requeues_a_dependent_config_in_another_root() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let app = root.join("app");
        let lib = root.join("lib");
        fs::create_dir_all(app.join("src")).unwrap();
        fs::create_dir_all(lib.join("src")).unwrap();
        fs::write(
            app.join("tsconfig.json"),
            r#"{"compilerOptions":{"paths":{"@lib":["../lib/src/index.ts"]}}}"#,
        )
        .unwrap();
        let app_source = app.join("src/a.ts");
        fs::write(
            &app_source,
            "import { value } from '@lib';\nexport const text: string = value;\n",
        )
        .unwrap();
        let lib_source = lib.join("src/index.ts");
        fs::write(&lib_source, "export const value: number = 1;\n").unwrap();
        switchable_tsc(&app);

        let (sender, receiver) = crossbeam_channel::unbounded();
        let workspace = Arc::new(crate::workspace::Workspace::new(vec![
            app.clone(),
            lib.clone(),
        ]));
        let documents = Arc::new(Mutex::new(Documents::default()));
        let settings = Settings {
            tsc_scope: TscScope::Project,
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
        let tsc = Arc::new(Tsc::new_with_timeout(
            Arc::clone(&diagnostics),
            documents,
            workspace,
            TIMEOUT,
        ));
        let app_uri = uri::from_path(&app_source);
        tsc.schedule(app_source, app.clone(), settings);
        let first = wait_for_tsc(&receiver, &app_uri, Duration::from_secs(10));
        assert!(first.diagnostics[0].message.contains("old"));

        fs::write(app.join("mode_new"), "").unwrap();
        fs::write(&lib_source, "export const value: string = 'x';\n").unwrap();
        let roots = tsc.invalidate_path(&lib_source);
        assert!(
            roots.contains(&crate::workspace::normalize(&app)),
            "{roots:?}"
        );
        let second = wait_for_tsc(&receiver, &app_uri, Duration::from_secs(10));
        assert!(
            second.diagnostics[0].message.contains("new"),
            "{:?}",
            second.diagnostics
        );
        assert_eq!(runs_in(&app), 2, "the dependent config ran more than once");
    }

    #[cfg(unix)]
    #[test]
    fn diagnostics_stay_cleared_through_backoff() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::write(root.join("tsconfig.json"), "{}").unwrap();
        let source = root.join("src/a.ts");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, "const a: number = '0';\n").unwrap();
        switchable_tsc(root);
        let uri = uri::from_path(&source);
        let (tsc, diagnostics, _documents, receiver, settings) =
            harness_with(root, TscScope::Project, None, Duration::from_millis(150));
        tsc.schedule(source.clone(), root.to_path_buf(), settings.clone());
        wait_for_tsc(&receiver, &uri, Duration::from_secs(10));
        fs::write(root.join("mode_hang"), "").unwrap();
        for attempt in 1..=3 {
            fs::write(&source, format!("const a: number = '{attempt}';\n")).unwrap();
            tsc.schedule(source.clone(), root.to_path_buf(), settings.clone());
            wait_for_runs(root, attempt + 1);
            wait_for_empty(&diagnostics, &uri);
        }
        let runs = runs_in(root);
        fs::write(&source, "const a: number = '4';\n").unwrap();
        tsc.schedule(source.clone(), root.to_path_buf(), settings.clone());
        thread::sleep(Duration::from_millis(300));
        assert_eq!(runs_in(root), runs, "a backed-off config ran again");
        assert!(
            diagnostics.merged(&uri, None).diagnostics.is_empty(),
            "diagnostics reappeared during backoff"
        );

        fs::remove_file(root.join("mode_hang")).unwrap();
        fs::write(root.join("mode_new"), "").unwrap();
        tsc.invalidate_root(root);
        tsc.schedule(source, root.to_path_buf(), settings);
        let params = wait_for_tsc(&receiver, &uri, Duration::from_secs(10));
        assert!(params.diagnostics[0].message.contains("new"));
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

    #[cfg(unix)]
    #[test]
    fn nonstandard_configs_are_all_scheduled() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::write(root.join("tsconfig.router.json"), "{}").unwrap();
        fs::write(root.join("tsconfig.node.json"), "{}").unwrap();
        let source = root.join("src/a.ts");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, "const value: number = 'x';\n").unwrap();
        fake_tsc(
            root,
            r#"case "$*" in
*tsconfig.router.json*) echo "src/a.ts(1,7): error TS2322: ROUTER error" ;;
*tsconfig.node.json*) echo "src/a.ts(1,8): error TS2322: NODE error" ;;
esac"#,
        );
        let uri = uri::from_path(&source);
        let (tsc, _diagnostics, _documents, receiver, settings) = harness(root, TscScope::Project);
        tsc.schedule(source, root.to_path_buf(), settings);
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut messages = Vec::new();
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let Ok(message) = receiver.recv_timeout(remaining) else {
                break;
            };
            let lsp_server::Message::Notification(notification) = message else {
                continue;
            };
            if notification.method != "textDocument/publishDiagnostics" {
                continue;
            }
            let params: lsp_types::PublishDiagnosticsParams =
                serde_json::from_value(notification.params).unwrap();
            if params.uri != uri {
                continue;
            }
            messages = params
                .diagnostics
                .into_iter()
                .map(|diagnostic| diagnostic.message)
                .collect();
            if messages.iter().any(|message| message.contains("ROUTER"))
                && messages.iter().any(|message| message.contains("NODE"))
            {
                break;
            }
        }
        assert!(
            messages.iter().any(|message| message.contains("ROUTER")),
            "{messages:?}"
        );
        assert!(
            messages.iter().any(|message| message.contains("NODE")),
            "{messages:?}"
        );
        assert_eq!(runs_in(root), 2, "each nonstandard config must run once");
    }

    #[cfg(unix)]
    #[test]
    fn reference_build_codes_are_filtered_and_logged_once() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::write(root.join("tsconfig.json"), "{}").unwrap();
        let source = root.join("src/a.ts");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, "const value: number = 'x';\n").unwrap();
        fake_tsc(
            root,
            "echo \"src/a.ts(1,7): error TS6305: output not built\"\n\
             echo \"src/a.ts(1,7): error TS6310: referenced project may not disable emit\"\n\
             echo \"src/a.ts(1,7): error TS2322: real error\"",
        );
        let uri = uri::from_path(&source);
        let (tsc, _diagnostics, _documents, receiver, settings) = harness(root, TscScope::Project);
        tsc.schedule(source, root.to_path_buf(), settings);
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut diagnostics = Vec::new();
        let mut hints = 0;
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let Ok(message) = receiver.recv_timeout(remaining) else {
                break;
            };
            let lsp_server::Message::Notification(notification) = message else {
                continue;
            };
            match notification.method.as_str() {
                "window/logMessage" => {
                    let params: lsp_types::LogMessageParams =
                        serde_json::from_value(notification.params).unwrap();
                    hints += usize::from(params.message.contains("referenced project not built"));
                }
                "textDocument/publishDiagnostics" => {
                    let params: lsp_types::PublishDiagnosticsParams =
                        serde_json::from_value(notification.params).unwrap();
                    if params.uri == uri && !params.diagnostics.is_empty() {
                        diagnostics = params.diagnostics;
                    }
                }
                _ => {}
            }
            if !diagnostics.is_empty() && hints > 0 {
                break;
            }
        }
        let codes: Vec<String> = diagnostics
            .iter()
            .filter_map(|diagnostic| match &diagnostic.code {
                Some(NumberOrString::String(code)) => Some(code.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(codes, vec!["TS2322"], "{diagnostics:?}");
        assert_eq!(hints, 1, "the build hint must be logged once");
    }

    #[cfg(unix)]
    #[test]
    fn cli_and_lsp_merge_the_same_project_reference_diagnostics() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::write(
            root.join("tsconfig.json"),
            r#"{"files":[],"references":[{"path":"./app"}]}"#,
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
            r#"case "$*" in
*app/tsconfig.json*) echo "src/a.ts(1,7): error TS2322: shared error" ;;
esac"#,
        );

        let cli = dotfmt_typescript::project::type_diagnostics_for(
            &app_source,
            root,
            &root.join("tsconfig.json"),
        )
        .unwrap();
        let cli_codes: Vec<u32> = cli.iter().map(|diagnostic| diagnostic.code).collect();
        assert_eq!(cli_codes, vec![2322], "{cli:?}");

        let app_uri = uri::from_path(&app_source);
        let (tsc, _diagnostics, _documents, receiver, settings) = harness(root, TscScope::Project);
        tsc.schedule(main_source, root.to_path_buf(), settings);
        let params = wait_for_tsc(&receiver, &app_uri, Duration::from_secs(10));
        let lsp_codes: Vec<String> = params
            .diagnostics
            .iter()
            .filter_map(|diagnostic| match &diagnostic.code {
                Some(NumberOrString::String(code)) => Some(code.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(lsp_codes, vec!["TS2322"], "{:?}", params.diagnostics);
        assert_eq!(params.diagnostics[0].message, cli[0].message);
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
    fn same_directory_referenced_configs_keep_distinct_diagnostics() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::write(
            root.join("tsconfig.json"),
            r#"{"files":[],"references":[{"path":"./tsconfig.app.json"},{"path":"./tsconfig.node.json"}]}"#,
        )
        .unwrap();
        fs::write(root.join("tsconfig.app.json"), "{}").unwrap();
        fs::write(root.join("tsconfig.node.json"), "{}").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        let source = root.join("src/a.ts");
        fs::write(&source, "const value: number = 'x';\n").unwrap();
        fake_tsc(
            root,
            r#"case "$*" in
*tsconfig.app.json*) echo "src/a.ts(1,7): error TS2322: APP error" ;;
*tsconfig.node.json*) echo "src/a.ts(1,8): error TS2322: NODE error" ;;
esac"#,
        );
        let uri = uri::from_path(&source);
        let (tsc, _diagnostics, _documents, receiver, settings) = harness(root, TscScope::Project);
        tsc.schedule(source, root.to_path_buf(), settings);
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut last = Vec::new();
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let Ok(message) = receiver.recv_timeout(remaining) else {
                break;
            };
            let lsp_server::Message::Notification(notification) = message else {
                continue;
            };
            if notification.method != "textDocument/publishDiagnostics" {
                continue;
            }
            let params: lsp_types::PublishDiagnosticsParams =
                serde_json::from_value(notification.params).unwrap();
            if params.uri == uri && !params.diagnostics.is_empty() {
                let complete = params
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.message.contains("APP error"))
                    && params
                        .diagnostics
                        .iter()
                        .any(|diagnostic| diagnostic.message.contains("NODE error"));
                last = params.diagnostics;
                if complete {
                    break;
                }
            }
        }
        let messages: Vec<String> = last
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect();
        assert!(
            messages.iter().any(|message| message.contains("APP error")),
            "{messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("NODE error")),
            "{messages:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_reload_does_not_lose_anchors_to_a_stale_check() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::write(root.join("tsconfig.json"), "{}").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.ts"), "export const a = 1;\n").unwrap();
        fs::write(root.join("src/b.ts"), "export const b = 1;\n").unwrap();
        fake_tsc(root, "sleep 0.5\necho \"src/b.ts(1,1): error TS2322: b\"");
        let (tsc, _diagnostics, _documents, receiver, settings) = harness(root, TscScope::Project);
        tsc.schedule(root.join("src/a.ts"), root.to_path_buf(), settings.clone());
        let marker = root.join("marker.log");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(marker.exists(), "fake compiler did not start");
        tsc.schedule(root.join("src/b.ts"), root.to_path_buf(), settings.clone());
        tsc.invalidate_root(root);
        tsc.schedule(root.join("src/b.ts"), root.to_path_buf(), settings);
        let uri = uri::from_path(&root.join("src/b.ts"));
        let params = wait_for_tsc(&receiver, &uri, Duration::from_secs(10));
        assert!(
            params
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains('b')),
            "{:?}",
            params.diagnostics
        );
    }

    #[cfg(unix)]
    #[test]
    fn invalid_compiler_status_is_unavailable_and_logs_once() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::write(root.join("tsconfig.json"), "{}\n").unwrap();
        let missing = root.join("missing-typescript");
        let (tsc, _diagnostics, _documents, receiver, settings) =
            harness_with(root, TscScope::Project, Some(missing), TIMEOUT);
        let statuses = tsc.status(&[(root.to_path_buf(), settings.clone())]);
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].state, "unavailable");
        assert_eq!(statuses[0].compiler, None);
        let logs = |receiver: &crossbeam_channel::Receiver<lsp_server::Message>| {
            receiver
                .try_iter()
                .filter_map(|message| match message {
                    lsp_server::Message::Notification(notification)
                        if notification.method == "window/logMessage" =>
                    {
                        notification.params["message"]
                            .as_str()
                            .filter(|message| message.contains("no compiler found"))
                            .map(ToOwned::to_owned)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(logs(&receiver).len(), 1);
        let _ = tsc.status(&[(root.to_path_buf(), settings)]);
        assert_eq!(
            logs(&receiver).len(),
            0,
            "the compiler error was logged twice"
        );
    }

    #[cfg(unix)]
    #[test]
    fn untrusted_status_does_not_spawn_the_compiler() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::write(root.join("tsconfig.json"), "{}\n").unwrap();
        fake_tsc(root, "echo run >> marker.log");
        let (tsc, _diagnostics, _documents, _receiver, settings) = harness(root, TscScope::Project);
        let statuses = tsc.status_untrusted(&[(root.to_path_buf(), settings)]);
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].state, "untrusted");
        assert_eq!(statuses[0].compiler, None);
        assert_eq!(statuses[0].version, None);
        thread::sleep(Duration::from_millis(100));
        assert!(
            !root.join("marker.log").exists(),
            "untrusted status spawned the compiler"
        );
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
