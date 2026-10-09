//! What is known about the shell at a point in the startup sequence.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::expand::{Env, Var};

/// Lowercase parameters zsh itself provides; never assumed unset.
const SPECIAL: &[&str] = &[
    "argv",
    "aliases",
    "builtins",
    "cdpath",
    "commands",
    "dirstack",
    "fignore",
    "fpath",
    "funcfiletrace",
    "funcsourcetrace",
    "funcstack",
    "functions",
    "functrace",
    "galiases",
    "history",
    "historywords",
    "jobdirs",
    "jobstates",
    "jobtexts",
    "keymaps",
    "mailpath",
    "manpath",
    "module_path",
    "modules",
    "nameddirs",
    "options",
    "parameters",
    "patchars",
    "path",
    "pipestatus",
    "psvar",
    "reswords",
    "saliases",
    "signals",
    "status",
    "terminfo",
    "termcap",
    "userdirs",
    "watch",
    "widgets",
    "zsh_eval_context",
    "zsh_scheduled_events",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    pub file: PathBuf,
    pub fingerprint: u64,
}

#[derive(Debug, Clone)]
pub struct State {
    vars: BTreeMap<String, Var>,
    ambient: BTreeMap<String, Option<String>>,
    host: String,
    path: Option<Vec<PathBuf>>,
    pub functions: BTreeMap<String, Function>,
    pub omz_alias_styles: bool,
    frames: Vec<BTreeMap<String, Var>>,
}

impl State {
    pub fn new(
        ambient: BTreeMap<String, Option<String>>,
        host: String,
        path: Vec<PathBuf>,
    ) -> Self {
        Self {
            vars: BTreeMap::new(),
            ambient,
            host,
            path: Some(path),
            functions: BTreeMap::new(),
            omz_alias_styles: false,
            frames: Vec::new(),
        }
    }

    pub fn set(&mut self, name: &str, value: Var) {
        if name == "PATH" || name == "path" {
            self.path = match &value {
                Var::Scalar(value) => Some(std::env::split_paths(value).collect()),
                Var::Array(values) => Some(values.iter().map(PathBuf::from).collect()),
                _ => None,
            };
            return;
        }
        self.vars.insert(name.to_string(), value);
    }

    pub fn forget(&mut self, name: &str) {
        self.set(name, Var::Unknown);
    }

    pub fn path(&self) -> Option<&[PathBuf]> {
        self.path.as_deref()
    }

    pub fn prepend_path(&mut self, dirs: &[PathBuf]) {
        if let Some(path) = &mut self.path {
            let mut next: Vec<PathBuf> = dirs.to_vec();
            next.append(path);
            let mut seen = BTreeSet::new();
            next.retain(|dir| seen.insert(dir.clone()));
            *path = next;
        }
    }

    /// `None` when the search path is unknown.
    pub fn command(&self, name: &str) -> Option<Option<PathBuf>> {
        if name.contains('/') {
            let path = PathBuf::from(name);
            return Some(is_executable(&path).then_some(path));
        }
        let path = self.path.as_ref()?;
        Some(
            path.iter()
                .map(|dir| dir.join(name))
                .find(|candidate| is_executable(candidate)),
        )
    }

    pub fn push_frame(&mut self) {
        self.frames.push(BTreeMap::new());
    }

    pub fn declare_local(&mut self, name: &str) {
        let previous = self.var(name);
        if let Some(frame) = self.frames.last_mut() {
            frame.entry(name.to_string()).or_insert(previous);
        }
    }

    pub fn in_function(&self) -> bool {
        !self.frames.is_empty()
    }

    pub fn is_local(&self, name: &str) -> bool {
        self.frames.iter().any(|frame| frame.contains_key(name))
    }

    pub fn pop_frame(&mut self) {
        if let Some(frame) = self.frames.pop() {
            for (name, value) in frame {
                self.set(&name, value);
            }
        }
    }

    /// Facts that hold on every path: the join of two branches.
    pub fn merge(&mut self, other: &State) {
        let names: BTreeSet<String> = self.vars.keys().chain(other.vars.keys()).cloned().collect();
        for name in names {
            if self.var(&name) != other.var(&name) {
                self.vars.insert(name, Var::Unknown);
            }
        }
        if self.path != other.path {
            self.path = None;
        }
        self.functions
            .retain(|name, function| other.functions.get(name) == Some(function));
        self.omz_alias_styles |= other.omz_alias_styles;
    }
}

impl Env for State {
    fn var(&self, name: &str) -> Var {
        if name == "PATH" || name == "path" {
            return match &self.path {
                Some(dirs) if name == "path" => Var::Array(
                    dirs.iter()
                        .map(|dir| dir.to_string_lossy().into_owned())
                        .collect(),
                ),
                Some(dirs) => std::env::join_paths(dirs)
                    .map(|joined| Var::Scalar(joined.to_string_lossy().into_owned()))
                    .unwrap_or(Var::Unknown),
                None => Var::Unknown,
            };
        }
        if let Some(value) = self.vars.get(name) {
            return value.clone();
        }
        if name == "HOST" {
            return Var::Scalar(self.host.clone());
        }
        if let Some(value) = self.ambient.get(name) {
            return value.clone().map_or(Var::Unset, Var::Scalar);
        }
        let shell_local = name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if shell_local && !SPECIAL.contains(&name) {
            Var::Unset
        } else {
            Var::Unknown
        }
    }
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
#[path = "../tests/unit/state_tests.rs"]
mod tests;
