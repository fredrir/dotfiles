use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use workstation::path::home_relative;

use crate::dialect::Dialect;
use clap::ValueEnum;
use comrak::options::ListStyleType;
use ignore::gitignore::GitignoreBuilder;

use crate::files::Files;

pub const NAME: &str = "mdfmt.dotfile";

const BLOCK: &str = "mdfmt";
const HOME: &str = "mdfmt";

#[derive(Clone, Debug)]
pub struct Config {
    pub dialect: Dialect,
    pub width: usize,
    pub table_style: TableStyle,
    pub heading_blank_lines: usize,
    pub list_marker: ListStyleType,
    pub trim_trailing_blank_lines: bool,
    pub final_newline: bool,
    pub source: Option<PathBuf>,
    pub files: Files,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            dialect: Dialect::Auto,
            width: 80,
            table_style: TableStyle::Auto,
            heading_blank_lines: 1,
            list_marker: ListStyleType::Dash,
            trim_trailing_blank_lines: true,
            final_newline: false,
            source: None,
            files: Files::default(),
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
                return Config::read_from(&candidate, &absolute(Path::new(".")));
            }
        }
        Ok(Config::default())
    }

    pub fn read(path: &Path) -> Result<Config, String> {
        Self::read_from(path, &absolute(&beside(path)))
    }

    fn read_from(path: &Path, root: &Path) -> Result<Config, String> {
        let text = fs::read_to_string(path).map_err(|error| complain(path, error))?;
        let mut config = Config::default();
        let mut whitelist = GitignoreBuilder::new(root);
        let mut blacklist = GitignoreBuilder::new(root);
        let mut block = None;
        for (offset, raw) in text.lines().enumerate() {
            let error = |message: &str| at(path, &format!("line {}: {message}", offset + 1));
            // Settings retain inline comments; pattern blocks preserve literal #,
            // =, braces, and escaped trailing spaces for the gitignore parser.
            let line = if block == Some("whitelist") || block == Some("blacklist") {
                raw.trim_start()
            } else {
                raw.split('#').next().unwrap_or_default().trim()
            };
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            if line.trim() == "}" {
                if block.take().is_none() {
                    return Err(error("unexpected }"));
                }
                continue;
            }
            match block {
                Some("whitelist" | "blacklist") => {
                    let builder = if block == Some("whitelist") {
                        &mut whitelist
                    } else {
                        &mut blacklist
                    };
                    builder
                        .add_line(Some(path.to_path_buf()), line)
                        .map_err(|problem| error(&problem.to_string()))?;
                }
                Some(_) => {
                    if line.ends_with('{') {
                        return Err(error("nested block"));
                    }
                    let (key, value) = line.split_once('=').unwrap_or((line, ""));
                    config
                        .set(key.trim(), value.trim())
                        .map_err(|message| error(&message))?;
                }
                None => {
                    let name = line
                        .strip_suffix('{')
                        .ok_or_else(|| error("entry outside a block"))?
                        .trim();
                    if ![BLOCK, "whitelist", "blacklist"].contains(&name) {
                        return Err(error(&format!("unknown block: {name}")));
                    }
                    block = Some(name);
                }
            }
        }
        if let Some(block) = block {
            return Err(at(path, &format!("missing }} for {block}")));
        }
        config.files =
            Files::build(root, &whitelist, &blacklist).map_err(|message| at(path, &message))?;
        config.source = Some(path.to_path_buf());
        Ok(config)
    }

    fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "dialect" => {
                self.dialect = Dialect::from_str(value, false).map_err(|_| {
                    "dialect must be auto, commonmark, gfm, github, or obsidian".to_string()
                })?
            }
            "width" => self.width = number(key, value, 0, 10000)?,
            "heading_blank_lines" => self.heading_blank_lines = number(key, value, 0, 3)?,
            "trim_trailing_blank_lines" => self.trim_trailing_blank_lines = flag(key, value)?,
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
