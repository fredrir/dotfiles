//! Command output kept between builds. Like `cached_eval`'s runtime cache,
//! an entry holds until the program changes, and also until its search path does.
//! A script's entry also ends with a new commit in the git checkout holding it.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, UNIX_EPOCH};

use hostkit::process::{CaptureLimits, output};
use serde::{Deserialize, Serialize};

use crate::git;

pub const FILE: &str = "commands.json";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Key {
    argv: Vec<String>,
    /// The program with symlinks resolved, so an upgrade changes the key.
    program: PathBuf,
    modified: (u64, u32),
    size: u64,
    /// For a script, the commit of the git checkout holding it.
    revision: Option<String>,
    /// `PATH` as passed to the program; `None` when inherited.
    path: Option<String>,
}

impl Key {
    /// `None` when a change could go unseen, as for a script outside a git checkout.
    pub fn new(argv: &[String], program: &Path, path: Option<&OsStr>) -> Option<Self> {
        let program = fs::canonicalize(program).ok()?;
        let meta = fs::metadata(&program).ok()?;
        let modified = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
        let revision = if is_script(&program) {
            Some(git::revision(&git::dir(program.parent()?)?)?)
        } else {
            None
        };
        Some(Self {
            argv: argv.to_vec(),
            program,
            modified: (modified.as_secs(), modified.subsec_nanos()),
            size: meta.len(),
            revision,
            path: path.map(|path| path.to_string_lossy().into_owned()),
        })
    }
}

fn is_script(program: &Path) -> bool {
    let mut magic = [0; 2];
    fs::File::open(program)
        .and_then(|mut file| file.read_exact(&mut magic))
        .is_ok_and(|()| magic == *b"#!")
}

/// A command's output, or why it failed.
pub type Outcome = Result<String, String>;

#[derive(Serialize, Deserialize)]
struct Entry {
    key: Key,
    outcome: Outcome,
}

#[derive(Default)]
pub struct Cache {
    path: Option<PathBuf>,
    stored: BTreeMap<Key, Outcome>,
    used: BTreeMap<Key, Outcome>,
}

impl Cache {
    /// The entries at `path`; none when it is missing or unreadable.
    pub fn load(path: PathBuf) -> Self {
        let stored = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Vec<Entry>>(&bytes).ok())
            .map(|entries| {
                entries
                    .into_iter()
                    .map(|entry| (entry.key, entry.outcome))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            path: Some(path),
            stored,
            used: BTreeMap::new(),
        }
    }

    /// The outcome stored for `key`, else `run`'s; an `Err` from `run` is
    /// worth retrying and is not kept.
    pub fn get_or_run(
        &mut self,
        key: Option<Key>,
        run: impl FnOnce() -> Result<Outcome, String>,
    ) -> Outcome {
        let Some(key) = key else {
            return run()?;
        };
        if let Some(outcome) = self.used.get(&key).or_else(|| self.stored.get(&key)) {
            let outcome = outcome.clone();
            self.used.insert(key, outcome.clone());
            return outcome;
        }
        let outcome = run()?;
        self.used.insert(key, outcome.clone());
        outcome
    }

    /// Keeps the entries this build used; returns whether the file changed.
    pub fn save(&self) -> Result<bool, String> {
        let Some(path) = &self.path else {
            return Ok(false);
        };
        if self.used == self.stored {
            return Ok(false);
        }
        let entries: Vec<Entry> = self
            .used
            .iter()
            .map(|(key, outcome)| Entry {
                key: key.clone(),
                outcome: outcome.clone(),
            })
            .collect();
        let text = serde_json::to_string(&entries)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        crate::emit::write_atomic(path, &text)?;
        Ok(true)
    }
}

/// Runs `program` as `argv`, with `PATH` set to `path` when given.
pub fn run(
    argv: &[String],
    program: &Path,
    path: Option<&OsStr>,
    timeout: Duration,
) -> Result<Outcome, String> {
    let shown = argv.join(" ");
    let mut command = Command::new(program);
    command.args(&argv[1..]).stdin(Stdio::null());
    if let Some(path) = path {
        command.env("PATH", path);
    }
    let captured = output(&mut command, CaptureLimits::default(), timeout)
        .map_err(|error| format!("{shown}: {error}"))?;
    if !captured.status.success() {
        return Ok(Err(format!("{shown}: {}", captured.status)));
    }
    if captured.stdout_truncated {
        return Ok(Err(format!("{shown}: output too large")));
    }
    Ok(String::from_utf8(captured.stdout).map_err(|_| format!("{shown}: output not UTF-8")))
}

#[cfg(test)]
#[path = "../tests/unit/cache_tests.rs"]
mod tests;
