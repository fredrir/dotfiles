//! `dotfmt.dotfile`: dotfmt's own settings, in dotfmt's own format.
//!
//! Which means the parser in `block.rs` reads it, the formatter in `block.rs`
//! keeps it laid out, and a typo in it is reported as `file:line: message`
//! like any other structural mistake. There is no second config language here
//! and no place for one to disagree.
//!
//! Three blocks. `dotfmt` is the layout, `include` and `exclude` are which
//! files get laid out at all; how a `.conf` file is laid out is not a setting,
//! it is the eight patterns compiled into `conf.rs`.
//!
//! Resolution is **per file**, and it walks up from the file rather than from
//! the target the run was pointed at — the same rule rustfmt, stylua and ruff
//! use, so a subdirectory can hold its own `dotfmt.dotfile` and be believed.
//! Then `~/.config/dotfmt/dotfmt.dotfile`, then `~/dotfmt.dotfile`, then
//! the defaults compiled in below. The first file found wins outright —
//! settings are not merged across files, because a half-inherited layout is
//! harder to explain than a whole one. Within a file, a setting left out keeps
//! its default, and an `include` block adds to the built-in `.dotfile` entry
//! rather than replacing it, so `!.dotfile` is how a project takes it away.
//!
//! Walking up from every one of a few thousand files would read the same three
//! directories a few thousand times, so [`Configs`] answers once per directory
//! and remembers.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::block::{self, Class};
use crate::render::shorten;
use crate::select::{Selection, Token};

/// The name looked for on the way up, and the name it is linked as under
/// `~/.config/dotfmt`.
pub const NAME: &str = "dotfmt.dotfile";

/// The directory under `~/.config` the shipped copy is linked into, which is
/// where `config/targets.dotfile` puts it. Not `~/.config/dotfile`: that is
/// the Python CLI's own state directory, holding `age/`, `merge/`, `sync/` and
/// the profile, and dotfmt's settings have no business among them.
const HOME: &str = "dotfmt";

/// How the files one config governs are laid out, and which files those are.
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
    /// Which files this config picks up.
    pub selection: Selection,
    /// The directory `selection`'s patterns are written relative to, and so
    /// the directory a leading `/` in one of them anchors to.
    pub root: PathBuf,
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
            selection: Selection::default(),
            root: everywhere(),
            source: None,
        }
    }
}

impl Config {
    /// The settings that govern `directory`, worked out from scratch.
    ///
    /// [`Configs::for_file`] is what a run should call; this is the answer it
    /// remembers.
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

    /// Read one `dotfmt.dotfile`, with its patterns relative to the directory
    /// it sits in.
    pub fn read(path: &Path) -> Result<Config, String> {
        Config::read_at(path, beside(path))
    }

    /// Read one `dotfmt.dotfile` whose patterns are relative to `root`.
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

    /// The token a file is picked up by, or `None` for one these settings
    /// leave alone.
    pub fn owns(&self, path: &Path) -> Option<Token> {
        self.selection.owns(&self.relative(path))
    }

    /// A path as the patterns see it: below the directory they were written
    /// in, which is what makes a leading `/` in one of them mean "here".
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

/// The settings for each file of a run, read once per directory.
///
/// Every file in one directory resolves to the same config, so the chain is
/// walked once for the first of them and the answer handed to the rest. A tree
/// of four thousand files in four hundred directories reads four hundred
/// chains instead of four thousand, and the walk that finds those files is
/// parallel, which is why this is behind a lock.
#[derive(Default)]
pub struct Configs {
    known: Mutex<HashMap<PathBuf, Result<Arc<Config>, String>>>,
}

impl Configs {
    pub fn new() -> Configs {
        Configs::default()
    }

    /// The settings that govern one file.
    pub fn for_file(&self, path: &Path) -> Result<Arc<Config>, String> {
        self.for_directory(&beside(path))
    }

    /// The settings that govern the files sitting directly in one directory.
    pub fn for_directory(&self, directory: &Path) -> Result<Arc<Config>, String> {
        let key = absolute(directory);
        if let Some(known) = self.remembered().get(&key) {
            return known.clone();
        }
        let found = Config::resolve(&key).map(Arc::new);
        self.remembered().insert(key, found.clone());
        found
    }

    /// A poisoned lock is a panic somewhere else, and dropping this run's
    /// answers on the floor because of it would help nobody: the map holds
    /// nothing an interrupted write could have left half-formed.
    fn remembered(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<PathBuf, Result<Arc<Config>, String>>> {
        self.known.lock().unwrap_or_else(|held| held.into_inner())
    }
}

/// The directory a path sits in, as somewhere to start looking upward from.
pub fn beside(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// Absolute without touching the filesystem, so nothing here follows a link.
fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The root of the filesystem: the directory a global config's patterns are
/// read as being written in.
fn everywhere() -> PathBuf {
    PathBuf::from(std::path::MAIN_SEPARATOR_STR)
}

/// Where `~/.config` is, honouring the variable that moves it.
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
