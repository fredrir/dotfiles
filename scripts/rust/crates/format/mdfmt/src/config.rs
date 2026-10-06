use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use workstation::path::home_relative;

use comrak::options::ListStyleType;

pub const NAME: &str = "mdfmt.dotfile";

const BLOCK: &str = "mdfmt";
const HOME: &str = "mdfmt";

#[derive(Clone, Debug)]
pub struct Config {
    pub width: usize,
    pub table_style: TableStyle,
    pub heading_blank_lines: usize,
    pub list_marker: ListStyleType,
    pub final_newline: bool,
    pub source: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            width: 80,
            table_style: TableStyle::Auto,
            heading_blank_lines: 1,
            list_marker: ListStyleType::Dash,
            final_newline: false,
            source: None,
        }
    }
}

impl Config {
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
            "width" => self.width = number(key, value, 0, 10000)?,
            "heading_blank_lines" => self.heading_blank_lines = number(key, value, 0, 3)?,
            "final_newline" => self.final_newline = flag(key, value)?,
            "table_style" => {
                self.table_style = match value {
                    "auto" => TableStyle::Auto,
                    "aligned" => TableStyle::Aligned,
                    "compact" => TableStyle::Compact,
                    _ => return Err("table_style must be auto, aligned, or compact".into()),
                }
            }
            "list_marker" => {
                self.list_marker = match value {
                    "-" => ListStyleType::Dash,
                    "*" => ListStyleType::Star,
                    "+" => ListStyleType::Plus,
                    _ => return Err("list_marker must be -, *, or +".into()),
                }
            }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableStyle {
    Auto,
    Aligned,
    Compact,
}

fn number(key: &str, value: &str, min: usize, max: usize) -> Result<usize, String> {
    value
        .parse::<usize>()
        .ok()
        .filter(|n| (min..=max).contains(n))
        .ok_or_else(|| format!("{key} must be between {min} and {max}, not {value}"))
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
