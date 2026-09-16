//! The same shape as `dotfmt`'s: a config found by walking up from the file
//! being formatted, then the two settled places, then the compiled-in defaults
//! — which `shared/tools/jqfmt.dotfile` repeats, and a test holds the two to
//! each other.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use workstation::path::home_relative;

use crate::render::{Indent, Layout};

pub const NAME: &str = "jqfmt.dotfile";

const BLOCK: &str = "jqfmt";
const HOME: &str = "jqfmt";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub indent: Indent,
    pub final_newline: bool,
    pub source: Option<PathBuf>,
    pub warnings: Vec<String>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            indent: Indent::Spaces(2),
            final_newline: true,
            source: None,
            warnings: Vec::new(),
        }
    }
}

impl Config {
    pub fn layout(&self) -> Layout {
        Layout {
            indent: self.indent,
            final_newline: self.final_newline,
        }
    }

    pub fn resolve(directory: &Path) -> Result<Config, String> {
        // Made absolute without touching the filesystem, so walking up from `.`
        // climbs the real tree and a symlinked target reads the config sitting
        // next to it rather than next to what it points at.
        let from = absolute(directory);
        for ancestor in from.ancestors() {
            let candidate = ancestor.join(NAME);
            if candidate.is_file() {
                return Config::read(&candidate);
            }
        }
        for candidate in [config_home().join(HOME).join(NAME), home().join(NAME)] {
            if candidate.is_file() {
                return Config::read(&candidate);
            }
        }
        Ok(Config::default())
    }

    pub fn read(path: &Path) -> Result<Config, String> {
        let text = fs::read_to_string(path).map_err(|error| complain(path, error))?;
        let mut config = Config::default();
        let entries = workstation::blocks::parse(&text).map_err(|problem| at(path, &problem))?;
        for entry in &entries {
            if entry.opens {
                if entry.block != BLOCK {
                    return Err(at(
                        path,
                        &format!("line {}: unknown block: {}", entry.number, entry.block),
                    ));
                }
                continue;
            }
            let (key, value) = entry.split();
            config
                .set(key, value)
                .map_err(|message| at(path, &format!("line {}: {message}", entry.number)))?;
        }
        config.source = Some(path.to_path_buf());
        Ok(config)
    }

    fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "indent" => self.indent = indent(value)?,
            "final_newline" => self.final_newline = flag(key, value)?,
            // The keys `dotfmt.dotfile` carries. A jq formatter has no column
            // to align and no blank lines to place, and saying so beats
            // ignoring a line somebody wrote on purpose.
            "align" | "align_max" | "blank_lines" => self.warnings.push(format!(
                "warning: jq does not support {key}, so it is ignored"
            )),
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

/// jq takes `--indent -1` for a tab and `--indent 0` for one line, and refuses
/// anything past seven.
fn indent(value: &str) -> Result<Indent, String> {
    let width: i64 = value
        .parse()
        .map_err(|_| format!("indent must be a whole number, not {value}"))?;
    match width {
        -1 => Ok(Indent::Tabs),
        0 => Ok(Indent::Compact),
        1..=7 => Ok(Indent::Spaces(width as usize)),
        _ => Err(format!(
            "indent must be -1 for a tab, or between 0 and 7, not {width}"
        )),
    }
}

fn flag(key: &str, value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("{key} must be true or false, not {other}")),
    }
}

fn at(path: &Path, message: &str) -> String {
    format!("{}: {message}", home_relative(path))
}

fn complain(path: &Path, error: std::io::Error) -> String {
    format!("{}: {error}", home_relative(path))
}
