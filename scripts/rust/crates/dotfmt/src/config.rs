//! `dotfile.dotfile`: dotfmt's own settings, in dotfmt's own format.
//!
//! Which means the parser in `block.rs` reads it, the formatter in `block.rs`
//! keeps it laid out, and a typo in it is reported as `file:line: message`
//! like any other structural mistake. There is no second config language here
//! and no place for one to disagree.
//!
//! Resolution goes from the target directory upward, then the copy in
//! `~/.config/dotfmt`, then the table compiled in below. The first
//! `dotfile.dotfile` found wins outright — settings are not merged across
//! files, because a half-inherited layout is harder to explain than a whole
//! one. Within a file, a setting left out keeps its default, and a `modes`
//! block replaces the built-in patterns rather than adding to them, so a
//! project can take a pattern away as well as put one there.

use std::fs;
use std::path::{Path, PathBuf};

use crate::block::{self, Class};
use crate::conf::{self, Mode};
use crate::render::shorten;

/// The name looked for on the way up, and the name it is linked as in
/// `~/.config/dotfmt`.
pub const NAME: &str = "dotfile.dotfile";

/// The patterns `format.py` hardcoded, in the order it tested them.
///
/// Order is the whole rule: the first pattern that matches wins, so the
/// `plain` opt-outs are listed above the `kitty` patterns they have to beat.
/// `*/kitty/colors*.conf` and `*/kitty/*.conf` both match a colour scheme, and
/// only one of them should.
const MODES: &[(&str, Mode)] = &[
    ("*/hypr/*", Mode::Hypr),
    ("*/hypr-local.conf", Mode::Hypr),
    ("hypr*.conf", Mode::Hypr),
    ("*/kitty/colors*.conf", Mode::Plain),
    ("*/colors*.conf", Mode::Plain),
    ("*/kitty/conf.d/fonts.conf", Mode::Plain),
    ("*/kitty/*.conf", Mode::Kitty),
    ("*/kitty.conf", Mode::Kitty),
];

/// How the files under one target are laid out.
#[derive(Debug)]
pub struct Config {
    /// Spaces before a line inside a block.
    pub indent: usize,
    /// Whether the `=` of a group of entries shares a column.
    pub align: bool,
    /// The longest key allowed to widen that column.
    pub align_max: usize,
    /// How many blank lines a run of them collapses to.
    pub blank_lines: usize,
    /// Whether the file ends with a newline.
    pub final_newline: bool,
    /// `glob = mode`, first match wins.
    pub modes: Vec<(String, Mode)>,
    /// Which file said so, or `None` for the defaults compiled in here.
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
            modes: MODES
                .iter()
                .map(|(pattern, mode)| ((*pattern).to_string(), *mode))
                .collect(),
            source: None,
        }
    }
}

impl Config {
    /// The settings that govern `directory`: the nearest `dotfile.dotfile` at
    /// or above it, then the one in `~/.config/dotfmt`, then the defaults.
    pub fn resolve(directory: &Path) -> Result<Config, String> {
        // Made absolute without touching the filesystem, so walking up from
        // `.` climbs the real tree and a symlinked target still reads the
        // config sitting next to it rather than next to what it points at.
        let from = std::path::absolute(directory).unwrap_or_else(|_| directory.to_path_buf());
        for ancestor in from.ancestors() {
            let candidate = ancestor.join(NAME);
            if candidate.is_file() {
                return Config::read(&candidate);
            }
        }
        let home = config_home().join("dotfmt").join(NAME);
        if home.is_file() {
            return Config::read(&home);
        }
        Ok(Config::default())
    }

    /// Read one `dotfile.dotfile`.
    pub fn read(path: &Path) -> Result<Config, String> {
        let text = fs::read_to_string(path).map_err(|error| complain(path, error))?;
        let mut config = Config::default();
        let parsed =
            block::parse(&text).map_err(|problem| at(path, problem.line, &problem.message))?;
        let mut modes: Option<Vec<(String, Mode)>> = None;
        for line in &parsed {
            let fault = match line.class {
                Class::Blank | Class::Comment | Class::Open | Class::Close => None,
                _ if line.depth == 0 => Some("setting outside a block".to_string()),
                _ if line.class != Class::Entry => Some("expected key = value".to_string()),
                _ => match line.block {
                    "dotfmt" => config.set(line.key, line.value).err(),
                    "modes" => match Mode::parse(line.value) {
                        Some(mode) => {
                            modes
                                .get_or_insert_with(Vec::new)
                                .push((line.key.to_string(), mode));
                            None
                        }
                        None => Some(format!("unknown mode: {}", line.value)),
                    },
                    other => Some(format!("unknown block: {other}")),
                },
            };
            if let Some(message) = fault {
                return Err(at(path, line.number, &message));
            }
        }
        if let Some(modes) = modes {
            config.modes = modes;
        }
        config.source = Some(path.to_path_buf());
        Ok(config)
    }

    /// Which mode a `.conf` or `.config` file is laid out in.
    pub fn mode_for(&self, path: &str) -> Mode {
        conf::mode(path, &self.modes)
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

/// Where `~/.config` is, honouring the variable that moves it.
fn config_home() -> PathBuf {
    if let Some(set) = std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(set);
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(".config")
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
