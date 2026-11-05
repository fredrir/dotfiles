use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub use dotfmt_core::config::Selection;
pub use dotfmt_core::edit::FixMode;

use dotfmt_core::config::{ConfigOverlay, Resolver};
use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};
use dotfmt_core::language::Language;

mod discovery;
mod engine;
mod execute;
mod ignore;
mod plan;

pub use dotfmt_core::repair::{Fix, Repair, Repairs};
pub use plan::{Plan, Prepared};

const WORKER_STACK: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Buffer {
    pub path: PathBuf,
    pub input: Vec<u8>,
}

#[derive(Clone, Debug)]
pub enum Operation {
    Format {
        targets: Vec<PathBuf>,
        buffer: Option<Buffer>,
        check: bool,
        fixes: bool,
    },
    Owns {
        paths: Vec<PathBuf>,
    },
    Inspect {
        targets: Vec<PathBuf>,
        buffer: Option<Buffer>,
        fix: FixMode,
        diff: bool,
        json: bool,
        type_aware: Option<bool>,
        project_analysis: bool,
    },
}

#[derive(Clone, Debug)]
pub struct Request {
    pub operation: Operation,
    pub languages: Vec<Language>,
    pub dialect: Option<String>,
    pub clean: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    pub workers: Option<usize>,
}

pub struct Session {
    resolver: Resolver,
    engines: engine::Engines,
    context: engine::DirectoryContext,
    options: Options,
    pool: OnceLock<Result<rayon::ThreadPool, Diagnostic>>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new(Resolver::new(), Options::default())
    }
}

impl Session {
    pub fn new(resolver: Resolver, options: Options) -> Self {
        Self {
            resolver,
            engines: engine::Engines::default(),
            context: engine::DirectoryContext::default(),
            options,
            pool: OnceLock::new(),
        }
    }

    pub fn with_paths(cwd: PathBuf, global: Option<PathBuf>) -> Self {
        Self::new(Resolver::with_paths(cwd, global), Options::default())
    }

    pub fn with_paths_overlay(
        cwd: PathBuf,
        global: Option<PathBuf>,
        overlay: Option<ConfigOverlay>,
    ) -> Self {
        Self::new(
            Resolver::with_paths_overlay(cwd, global, overlay),
            Options::default(),
        )
    }

    pub fn run(&mut self, request: Request) -> Result<Outcome, Vec<Diagnostic>> {
        let prepared = self.prepare(request)?;
        let plan = self.validate(prepared)?;
        self.execute(plan).map_err(|error| vec![error])
    }

    fn pool(&self) -> Result<&rayon::ThreadPool, Diagnostic> {
        self.pool
            .get_or_init(|| {
                let workers = self
                    .options
                    .workers
                    .filter(|workers| *workers > 0)
                    .unwrap_or_else(|| {
                        std::thread::available_parallelism()
                            .map_or(1, usize::from)
                            .min(32)
                    });
                rayon::ThreadPoolBuilder::new()
                    .num_threads(workers)
                    .stack_size(WORKER_STACK)
                    .build()
                    .map_err(|error| {
                        Diagnostic::new(DiagnosticKind::Internal, format!("worker pool: {error}"))
                    })
            })
            .as_ref()
            .map_err(Clone::clone)
    }
}

#[derive(Clone, Debug)]
pub struct Change {
    pub changed: bool,
    pub repairs: Repairs,
}

#[derive(Clone, Debug)]
pub struct FileOutcome {
    pub path: PathBuf,
    pub language: Language,
    pub result: Result<Change, Diagnostic>,
}

#[derive(Clone, Debug)]
pub struct Inspection {
    pub findings: Vec<dotfmt_core::edit::Finding>,
    pub fixes: Vec<dotfmt_core::edit::Fix>,
    pub applied: Vec<String>,
    pub before: String,
    pub after: String,
    pub changed: bool,
    pub diff: Option<String>,
}

#[derive(Clone, Debug)]
pub struct InspectOutcome {
    pub path: PathBuf,
    pub language: Language,
    pub result: Result<Inspection, Diagnostic>,
}

#[derive(Clone, Debug)]
pub struct InspectBufferOutcome {
    pub path: PathBuf,
    pub language: Option<Language>,
    pub result: Result<Inspection, Diagnostic>,
}

#[derive(Clone, Debug)]
pub struct BufferChange {
    pub output: Vec<u8>,
    pub change: Change,
}

#[derive(Clone, Debug)]
pub struct BufferOutcome {
    pub path: PathBuf,
    pub language: Option<Language>,
    pub result: Result<BufferChange, Diagnostic>,
}

#[derive(Clone, Debug)]
pub struct Explanation {
    pub selection: Selection,
    pub dialect: Option<&'static str>,
    pub sources: Vec<PathBuf>,
}

#[derive(Clone, Debug)]
pub enum Outcome {
    Formatted {
        files: Vec<FileOutcome>,
        buffer: Option<BufferOutcome>,
        unreadable: usize,
        check: bool,
    },
    Owned(Vec<PathBuf>),
    Inspected {
        files: Vec<InspectOutcome>,
        buffer: Option<InspectBufferOutcome>,
        unreadable: usize,
        fix: FixMode,
        diff: bool,
        json: bool,
        type_aware: Option<bool>,
    },
}

impl Outcome {
    pub fn failed(&self) -> bool {
        match self {
            Self::Owned(_) => false,
            Self::Inspected {
                files,
                buffer,
                unreadable,
                fix,
                ..
            } => {
                *unreadable > 0
                    || files.iter().any(|file| match &file.result {
                        Err(_) => true,
                        Ok(inspection) => {
                            matches!(fix, FixMode::None) && !inspection.findings.is_empty()
                        }
                    })
                    || buffer.as_ref().is_some_and(|buffer| match &buffer.result {
                        Err(_) => true,
                        Ok(inspection) => {
                            matches!(fix, FixMode::None) && !inspection.findings.is_empty()
                        }
                    })
            }
            Self::Formatted {
                files,
                buffer,
                unreadable,
                check,
            } => {
                *unreadable > 0
                    || files.iter().any(|file| {
                        file.result
                            .as_ref()
                            .map_or(true, |change| *check && change.changed)
                    })
                    || buffer.as_ref().is_some_and(|buffer| {
                        buffer
                            .result
                            .as_ref()
                            .map_or(true, |formatted| *check && formatted.change.changed)
                    })
            }
        }
    }
}

fn requested(request: &Request, language: Language) -> bool {
    request.languages.is_empty() || request.languages.contains(&language)
}

fn forced(request: &Request) -> Option<Language> {
    (request.languages.len() == 1).then(|| request.languages[0])
}

fn configuration(message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Configuration, message)
}

fn attach_path(mut error: Diagnostic, path: &Path) -> Diagnostic {
    if error.path.is_none() {
        error.path = Some(path.to_path_buf());
    }
    error
}

fn ordered(mut errors: Vec<Diagnostic>) -> Vec<Diagnostic> {
    errors.sort_by_cached_key(ToString::to_string);
    errors.dedup();
    errors
}

fn with_stack<T: Send>(work: impl FnOnce() -> T + Send) -> Result<T, Diagnostic> {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("dotfmt".into())
            .stack_size(WORKER_STACK)
            .spawn_scoped(scope, work)
            .map_err(|error| {
                Diagnostic::new(
                    DiagnosticKind::Internal,
                    format!("formatting worker: {error}"),
                )
            })?
            .join()
            .map_err(|_| Diagnostic::new(DiagnosticKind::Internal, "formatting worker panicked"))
    })
}
