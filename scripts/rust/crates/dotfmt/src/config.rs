
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::block::{self, Class};
use crate::render::shorten;
use crate::select::{Selection, Token};

pub const NAME: &str = "dotfmt.dotfile";

const HOME: &str = "dotfmt";

#[derive(Debug)]
pub struct Config {
    pub indent: usize,
    pub align: bool,
    pub align_max: usize,
    pub blank_lines: usize,
    pub final_newline: bool,
    pub selection: Selection,
    pub root: PathBuf,
    pub source: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            indent: 2,
            align: true,
            align_max: 24,
            blank_lines: 1,
            final_newline: true,
            selection: Selection::default(),
            root: everywhere(),
            source: None,
        }
    }
}

impl Config {
    pub fn resolve(directory: &Path) -> Result<Config, String> {
        // Made absolute without touching the filesystem, so walking up from
        // `.` climbs the real tree and a symlinked target still reads the
        // config sitting next to it rather than next to what it points at.
        let from = absolute(directory);
        for ancestor in from.ancestors() {
            let candidate = ancestor.join(NAME);
            if candidate.is_file() {
                return Config::read(&candidate);
            }
        }
        // The two settled places, in that order. Neither is an ancestor of the
        // file being formatted, so their patterns are read as though written
        // at the root of the filesystem: a global config is about every path
        // there is, and there is no other directory for it to anchor to.
        for candidate in [config_home().join(HOME).join(NAME), home().join(NAME)] {
            if candidate.is_file() {
                return Config::read_at(&candidate, everywhere());
            }
        }
        Ok(Config::default())
    }

    pub fn read(path: &Path) -> Result<Config, String> {
        Config::read_at(path, beside(path))
    }

    fn read_at(path: &Path, root: PathBuf) -> Result<Config, String> {
        let text = fs::read_to_string(path).map_err(|error| complain(path, error))?;
        let mut config = Config {
            root,
            ..Config::default()
        };
        let parsed =
            block::parse(&text).map_err(|problem| at(path, problem.line, &problem.message))?;
        for line in &parsed {
            let fault = match line.class {
                Class::Blank | Class::Comment | Class::Open | Class::Close => None,
                _ if line.depth == 0 => Some("setting outside a block".to_string()),
                _ => match line.block {
                    "dotfmt" if line.class != Class::Entry => {
                        Some("expected key = value".to_string())
                    }
                    "dotfmt" => config.set(line.key, line.value).err(),
                    // A pattern holding an `=` is read as `key = value` by the
                    // grammar, and laying this very file out would then rewrite
                    // it as `key  = value`. Turning it away here is what keeps
                    // dotfmt from silently editing its own config.
                    "include" | "exclude" if line.class != Class::Bare => {
                        Some("expected a pattern; a pattern cannot hold an =".to_string())
                    }
                    "include" => config.selection.include(line.body).err(),
                    "exclude" => config.selection.exclude(line.body).err(),
                    other => Some(format!("unknown block: {other}")),
                },
            };
            if let Some(message) = fault {
                return Err(at(path, line.number, &message));
            }
        }
        config.source = Some(path.to_path_buf());
        Ok(config)
    }

    pub fn owns(&self, path: &Path) -> Option<Token> {
        self.selection.owns(&self.relative(path))
    }

    fn relative(&self, path: &Path) -> PathBuf {
        let absolute = absolute(path);
        match absolute.strip_prefix(&self.root) {
            Ok(rest) => rest.to_path_buf(),
            // Only a config named on the command line can govern a file that
            // is not below it. Matching the whole path is a worse answer than
            // matching part of it and a much better one than matching nothing.
            Err(_) => absolute
                .strip_prefix(everywhere())
                .unwrap_or(&absolute)
                .to_path_buf(),
        }
    }

    fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "indent" => self.indent = number(key, value)?,
            "align" => self.align = flag(key, value)?,
            "align_max" => self.align_max = number(key, value)?,
            "blank_lines" => self.blank_lines = number(key, value)?,
            "final_newline" => self.final_newline = flag(key, value)?,
            other => return Err(format!("unknown setting: {other}")),
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct Configs {
    known: Mutex<HashMap<PathBuf, Result<Arc<Config>, String>>>,
}

impl Configs {
    pub fn new() -> Configs {
        Configs::default()
    }

    pub fn for_file(&self, path: &Path) -> Result<Arc<Config>, String> {
        self.for_directory(&beside(path))
    }

    pub fn for_directory(&self, directory: &Path) -> Result<Arc<Config>, String> {
        let key = absolute(directory);
        if let Some(known) = self.remembered().get(&key) {
            return known.clone();
        }
        let found = Config::resolve(&key).map(Arc::new);
        self.remembered().insert(key, found.clone());
        found
    }

    fn remembered(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<PathBuf, Result<Arc<Config>, String>>> {
        self.known.lock().unwrap_or_else(|held| held.into_inner())
    }
}

pub fn beside(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

fn everywhere() -> PathBuf {
    PathBuf::from(std::path::MAIN_SEPARATOR_STR)
}

fn config_home() -> PathBuf {
    if let Some(set) = std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(set);
    }
    home().join(".config")
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn number(key: &str, value: &str) -> Result<usize, String> {
    value
        .parse()
        .map_err(|_| format!("{key} must be a whole number, not {value}"))
}

fn flag(key: &str, value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("{key} must be true or false, not {other}")),
    }
}

fn at(path: &Path, line: usize, message: &str) -> String {
    format!("{}:{line}: {message}", shorten(path))
}

fn complain(path: &Path, error: std::io::Error) -> String {
    format!("{}: {error}", shorten(path))
}
