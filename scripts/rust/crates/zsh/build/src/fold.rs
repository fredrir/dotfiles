//! Commands whose output is fixed between builds, run once at build time.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use crate::cache::{self, Cache, Key};
use crate::state::State;

pub struct Folder {
    allowed: Vec<Vec<String>>,
    timeout: Duration,
    runs: BTreeMap<Vec<String>, Result<String, String>>,
    pub cache: Cache,
}

impl Folder {
    pub fn new(allowed: Vec<Vec<String>>, timeout: Duration, cache: Cache) -> Self {
        Self {
            allowed,
            timeout,
            runs: BTreeMap::new(),
            cache,
        }
    }

    pub fn allows(&self, argv: &[String]) -> bool {
        let Some((program, args)) = argv.split_first() else {
            return false;
        };
        let name = Path::new(program)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.allowed
            .iter()
            .any(|allowed| allowed[0] == name && allowed[1..] == *args)
    }

    /// A listed command installed on this host; folds of anything else are skipped.
    pub fn applies(&self, argv: &[String], state: &State) -> bool {
        self.allows(argv) && matches!(state.command(&argv[0]), Some(Some(_)))
    }

    /// Standard output of `argv`, found on the build-time search path; a
    /// listed command's output is kept between builds.
    pub fn run(&mut self, argv: &[String], state: &State) -> Result<String, String> {
        self.output(argv, state, self.allows(argv))
    }

    /// `run`, keeping the output between builds as `cached_eval` keeps it between shells.
    pub fn run_cached(&mut self, argv: &[String], state: &State) -> Result<String, String> {
        self.output(argv, state, true)
    }

    fn output(&mut self, argv: &[String], state: &State, keep: bool) -> Result<String, String> {
        if let Some(previous) = self.runs.get(argv) {
            return previous.clone();
        }
        let result = self.execute(argv, state, keep);
        self.runs.insert(argv.to_vec(), result.clone());
        result
    }

    fn execute(&mut self, argv: &[String], state: &State, keep: bool) -> Result<String, String> {
        let program = match state.command(&argv[0]) {
            Some(Some(program)) => program,
            Some(None) => return Err(format!("{}: not found", argv[0])),
            None => return Err(format!("{}: search path unknown", argv[0])),
        };
        let path = state
            .path()
            .and_then(|path| std::env::join_paths(path).ok());
        let key = keep
            .then(|| Key::new(argv, &program, path.as_deref()))
            .flatten();
        let timeout = self.timeout;
        self.cache
            .get_or_run(key, || cache::run(argv, &program, path.as_deref(), timeout))
    }
}

/// Folded values, emitted once as globals ahead of the bundle.
#[derive(Default)]
pub struct Constants {
    values: Vec<String>,
}

impl Constants {
    pub fn name(&mut self, value: String) -> String {
        let index = match self.values.iter().position(|existing| *existing == value) {
            Some(index) => index,
            None => {
                self.values.push(value);
                self.values.len() - 1
            }
        };
        format!("__zb{}", index + 1)
    }

    pub fn declarations(&self) -> String {
        self.values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                format!(
                    "typeset -g __zb{}={}\n",
                    index + 1,
                    crate::quote::ansi(value)
                )
            })
            .collect()
    }

    pub fn truncate(&mut self, len: usize) {
        self.values.truncate(len);
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}
