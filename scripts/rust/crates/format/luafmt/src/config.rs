use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use workstation::path::home_relative;

use crate::dialect::Dialect;
use clap::ValueEnum;
use ignore::gitignore::GitignoreBuilder;
use stylua_lib::{
    BlockNewlineGaps, CallParenType, CollapseSimpleStatement, IndentType, LineEndings, QuoteStyle,
    SpaceAfterFunctionNames,
};

use crate::files::Files;

pub const NAME: &str = "luafmt.dotfile";

const BLOCK: &str = "luafmt";
const HOME: &str = "luafmt";

#[derive(Clone, Debug)]
pub struct Config {
    pub dialect: Dialect,
    pub style: stylua_lib::Config,
    pub final_newline: bool,
    pub verify: bool,
    pub source: Option<PathBuf>,
    pub files: Files,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            dialect: Dialect::Auto,
            style: stylua_lib::Config {
                indent_type: IndentType::Spaces,
                indent_width: 2,
                call_parentheses: CallParenType::None,
                ..stylua_lib::Config::default()
            },
            final_newline: false,
            verify: false,
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
        Self::fallback()
    }

    fn fallback() -> Result<Config, String> {
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
                self.dialect = Dialect::from_str(value, false).map_err(|error| error.to_string())?
            }
            "width" => self.style.column_width = number(key, value, 1, 10000)?,
            "indent" => self.style.indent_width = number(key, value, 1, 16)?,
            "indent_type" => {
                self.style.indent_type = choice(
                    key,
                    value,
                    &[("spaces", IndentType::Spaces), ("tabs", IndentType::Tabs)],
                )?
            }
            "line_endings" => {
                self.style.line_endings = choice(
                    key,
                    value,
                    &[
                        ("unix", LineEndings::Unix),
                        ("windows", LineEndings::Windows),
                    ],
                )?
            }
            "quote_style" => {
                self.style.quote_style = choice(
                    key,
                    value,
                    &[
                        ("auto-prefer-double", QuoteStyle::AutoPreferDouble),
                        ("auto-prefer-single", QuoteStyle::AutoPreferSingle),
                        ("force-double", QuoteStyle::ForceDouble),
                        ("force-single", QuoteStyle::ForceSingle),
                    ],
                )?
            }
            "call_parentheses" => {
                self.style.call_parentheses = choice(
                    key,
                    value,
                    &[
                        ("always", CallParenType::Always),
                        ("no-single-string", CallParenType::NoSingleString),
                        ("no-single-table", CallParenType::NoSingleTable),
                        ("none", CallParenType::None),
                        ("input", CallParenType::Input),
                    ],
                )?
            }
            "collapse_simple_statement" => {
                self.style.collapse_simple_statement = choice(
                    key,
                    value,
                    &[
                        ("never", CollapseSimpleStatement::Never),
                        ("function-only", CollapseSimpleStatement::FunctionOnly),
                        ("conditional-only", CollapseSimpleStatement::ConditionalOnly),
                        ("always", CollapseSimpleStatement::Always),
                    ],
                )?
            }
            "space_after_function_names" => {
                self.style.space_after_function_names = choice(
                    key,
                    value,
                    &[
                        ("never", SpaceAfterFunctionNames::Never),
                        ("definitions", SpaceAfterFunctionNames::Definitions),
                        ("calls", SpaceAfterFunctionNames::Calls),
                        ("always", SpaceAfterFunctionNames::Always),
                    ],
                )?
            }
            "block_newline_gaps" => {
                self.style.block_newline_gaps = choice(
                    key,
                    value,
                    &[
                        ("never", BlockNewlineGaps::Never),
                        ("preserve", BlockNewlineGaps::Preserve),
                    ],
                )?
            }
            "sort_requires" => self.style.sort_requires.enabled = flag(key, value)?,
            "final_newline" => self.final_newline = flag(key, value)?,
            "verify" => self.verify = flag(key, value)?,
            other => return Err(format!("unknown setting: {other}")),
        }
        Ok(())
    }
}

type Resolved = Result<Arc<Config>, String>;
type Cached = Arc<OnceLock<Resolved>>;

#[derive(Default)]
pub struct Configs {
    known: Mutex<HashMap<PathBuf, Cached>>,
    fallback: OnceLock<Resolved>,
}

impl Configs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn for_file(&self, path: &Path) -> Resolved {
        self.for_directory(&beside(path))
    }

    pub fn for_directory(&self, directory: &Path) -> Resolved {
        let key = absolute(directory);
        let cell = self
            .known
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .entry(key.clone())
            .or_default()
            .clone();
        // Each directory and config is resolved once, including concurrent misses.
        // Descendants share their ancestor's parsed settings and compiled globs.
        cell.get_or_init(|| {
            let candidate = key.join(NAME);
            if candidate.is_file() {
                Config::read(&candidate).map(Arc::new)
            } else if let Some(parent) = key.parent() {
                self.for_directory(parent)
            } else {
                self.fallback
                    .get_or_init(|| Config::fallback().map(Arc::new))
                    .clone()
            }
        })
        .clone()
    }
}

fn choice<T: Copy>(key: &str, value: &str, choices: &[(&str, T)]) -> Result<T, String> {
    choices
        .iter()
        .find(|(name, _)| *name == value)
        .map(|(_, choice)| *choice)
        .ok_or_else(|| {
            format!(
                "{key} must be {}, not {value}",
                choices
                    .iter()
                    .map(|(name, _)| *name)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
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
