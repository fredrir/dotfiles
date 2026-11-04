//! `doc-purge.dotfile` parsing and resolution.
//!
//! The grammar is the one `dotfmt.dotfile` uses: a global `{ ... }` block,
//! `key = value` settings, per-language blocks, and `included_files` /
//! `excluded_files` blocks whose lines are gitignore patterns.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Display};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use doc_purge_core::lang;

pub const CONFIG_NAME: &str = "doc-purge.dotfile";

/// What to do with generated files.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Generated {
    /// Leave generated files alone.
    #[default]
    Skip,
    /// Purge generated files like any other source.
    Purge,
}

#[derive(Clone, Debug)]
pub struct LanguageConfig {
    pub enabled: bool,
    pub includes: Option<Arc<Patterns>>,
    pub excludes: Option<Arc<Patterns>>,
}

impl Default for LanguageConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            includes: None,
            excludes: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub glyphs: bool,
    pub docstrings: bool,
    pub first_sentence: bool,
    pub generated: Generated,
    pub respect_gitignore: bool,
    pub languages: BTreeMap<String, LanguageConfig>,
    pub includes: Option<Arc<Patterns>>,
    pub excludes: Option<Arc<Patterns>>,
    /// Non-fatal notes, such as a language block the current registry does not
    /// know yet.
    pub warnings: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            glyphs: true,
            docstrings: true,
            first_sentence: true,
            generated: Generated::Skip,
            respect_gitignore: true,
            languages: BTreeMap::new(),
            includes: None,
            excludes: None,
            warnings: Vec::new(),
        }
    }
}

impl Config {
    /// Global `included_files` / `excluded_files`, applied to every candidate.
    pub fn allows_path(&self, path: &Path) -> bool {
        if self
            .includes
            .as_ref()
            .is_some_and(|patterns| patterns.matches(path, false) != Some(true))
        {
            return false;
        }
        if self
            .excludes
            .as_ref()
            .is_some_and(|patterns| patterns.matches(path, true) == Some(true))
        {
            return false;
        }
        true
    }

    /// A language block's `enabled` flag and file patterns.
    pub fn language_enabled(&self, name: &str, path: &Path) -> bool {
        let Some(language) = self.languages.get(name) else {
            return true;
        };
        if !language.enabled {
            return false;
        }
        if language
            .includes
            .as_ref()
            .is_some_and(|patterns| patterns.matches(path, false) != Some(true))
        {
            return false;
        }
        if language
            .excludes
            .as_ref()
            .is_some_and(|patterns| patterns.matches(path, true) == Some(true))
        {
            return false;
        }
        true
    }
}

#[derive(Debug)]
pub struct Patterns {
    root: PathBuf,
    matcher: Gitignore,
}

impl Patterns {
    /// `excluded` asks whether the path itself is ignored; includes ask whether
    /// it is whitelisted. Parent directories are checked too, so a negated file
    /// cannot reopen an excluded directory.
    fn matches(&self, path: &Path, excluded: bool) -> Option<bool> {
        let mut matched = None;
        for (depth, candidate) in path.ancestors().enumerate() {
            if candidate.parent().is_none() {
                break;
            }
            if candidate == self.root || self.root.starts_with(candidate) {
                continue;
            }
            let result = self.matcher.matched(candidate, depth != 0);
            let found = if result.is_ignore() {
                Some(true)
            } else if result.is_whitelist() {
                Some(false)
            } else {
                None
            };
            if let Some(found) = found {
                if !excluded || found {
                    return Some(found);
                }
                // A whitelist below an excluded parent can reopen the path.
                matched = Some(false);
            }
        }
        matched
    }
}

#[derive(Debug)]
pub struct ConfigError {
    pub path: PathBuf,
    pub line: Option<usize>,
    pub message: String,
}

impl ConfigError {
    fn at(path: &Path, line: usize, message: impl Display) -> Self {
        Self {
            path: path.to_path_buf(),
            line: Some(line),
            message: message.to_string(),
        }
    }

    fn io(path: &Path, message: impl Display) -> Self {
        Self {
            path: path.to_path_buf(),
            line: None,
            message: message.to_string(),
        }
    }
}

impl Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(
                formatter,
                "{}:{}: {}",
                self.path.display(),
                line,
                self.message
            ),
            None => write!(formatter, "{}: {}", self.path.display(), self.message),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Finds the configuration that applies to a target.
///
/// Resolution is: the nearest `doc-purge.dotfile` at or above the target, then
/// `$XDG_CONFIG_HOME/doc-purge/doc-purge.dotfile`, then built-in defaults.
pub struct Resolver {
    cwd: PathBuf,
    global: Option<PathBuf>,
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new()
    }
}

impl Resolver {
    pub fn new() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        let global = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|value| !value.is_empty())
                    .map(|home| PathBuf::from(home).join(".config"))
            })
            .map(|base| base.join("doc-purge").join(CONFIG_NAME));
        Self { cwd, global }
    }

    pub fn absolute(&self, path: &Path) -> PathBuf {
        absolute(&self.cwd, path)
    }

    pub fn for_path(&self, path: &Path) -> Result<Config, ConfigError> {
        let absolute = self.absolute(path);
        let start = if absolute.is_dir() {
            absolute.as_path()
        } else {
            absolute.parent().unwrap_or(self.cwd.as_path())
        };
        for directory in start.ancestors() {
            let candidate = directory.join(CONFIG_NAME);
            if candidate.is_file() {
                return read(&candidate, directory);
            }
        }
        if let Some(global) = &self.global
            && global.is_file()
        {
            // Global patterns are written against the working tree, as
            // dotfmt's global layer is, not against the config's own directory.
            return read(global, &self.cwd);
        }
        Ok(Config::default())
    }
}

/// Resolves relative and parent components the way configuration discovery
/// does, without following ordinary symlink targets.
fn absolute(cwd: &Path, path: &Path) -> PathBuf {
    let supplied = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in supplied.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // A symlink followed by `..` names the physical target's parent.
                if std::fs::symlink_metadata(&normalized)
                    .is_ok_and(|metadata| metadata.file_type().is_symlink())
                    && let Ok(target) = std::fs::canonicalize(&normalized)
                {
                    normalized = target;
                }
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn read(path: &Path, root: &Path) -> Result<Config, ConfigError> {
    let source = std::fs::read_to_string(path).map_err(|error| ConfigError::io(path, error))?;
    parse(&source, path, root)
}

fn parse(source: &str, path: &Path, root: &Path) -> Result<Config, ConfigError> {
    let mut parser = Parser {
        source,
        offset: 0,
        line: 1,
        path,
        root,
        config: Config::default(),
    };
    let mut global_seen = false;
    while parser.skip() {
        let line = parser.line;
        let name = parser.head()?;
        match name.as_str() {
            "" => {
                if global_seen {
                    return Err(parser.error(line, "duplicate global block"));
                }
                global_seen = true;
                parser.global_settings()?;
            }
            "included_files" | "include" => {
                if parser.config.includes.is_some() {
                    return Err(parser.error(line, "duplicate included_files block"));
                }
                parser.config.includes = Some(parser.patterns()?);
            }
            "excluded_files" | "exclude" => {
                if parser.config.excludes.is_some() {
                    return Err(parser.error(line, "duplicate excluded_files block"));
                }
                parser.config.excludes = Some(parser.patterns()?);
            }
            _ => {
                let key = match lang::for_token(&name) {
                    Some(language) => language.name.to_string(),
                    None => {
                        parser.config.warnings.push(format!(
                            "{}:{line}: unknown language '{name}'; the block is kept for when the registry learns it",
                            path.display()
                        ));
                        name.clone()
                    }
                };
                if parser.config.languages.contains_key(&key) {
                    return Err(parser.error(line, format!("duplicate {key} block")));
                }
                let local = parser.language_settings()?;
                parser.config.languages.insert(key, local);
            }
        }
    }
    Ok(parser.config)
}

struct Parser<'a> {
    source: &'a str,
    offset: usize,
    line: usize,
    path: &'a Path,
    root: &'a Path,
    config: Config,
}

impl Parser<'_> {
    fn error(&self, line: usize, message: impl Display) -> ConfigError {
        ConfigError::at(self.path, line, message)
    }

    fn peek(&self) -> Option<char> {
        self.source[self.offset..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.offset += ch.len_utf8();
        if ch == '\n' {
            self.line += 1;
        }
        Some(ch)
    }

    fn skip(&mut self) -> bool {
        loop {
            while self.peek().is_some_and(char::is_whitespace) {
                self.bump();
            }
            if self.peek() != Some('#') {
                return self.peek().is_some();
            }
            while self.peek().is_some_and(|ch| ch != '\n') {
                self.bump();
            }
        }
    }

    fn head(&mut self) -> Result<String, ConfigError> {
        let start = self.offset;
        let line = self.line;
        while let Some(ch) = self.peek() {
            if ch == '{' {
                let name = self.source[start..self.offset].trim().replace('-', "_");
                self.bump();
                return Ok(name);
            }
            if matches!(ch, '}' | '\n' | '=' | '#') {
                break;
            }
            self.bump();
        }
        Err(self.error(line, "expected a block followed by '{'"))
    }

    fn global_settings(&mut self) -> Result<(), ConfigError> {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        loop {
            if !self.skip() {
                return Err(self.error(self.line, "unterminated global block"));
            }
            if self.peek() == Some('}') {
                self.bump();
                return Ok(());
            }
            let (name, line) = self.setting_head()?;
            if self.peek() == Some('{') {
                self.bump();
                if !seen.insert(name.clone()) {
                    return Err(self.error(line, format!("duplicate setting '{name}'")));
                }
                match name.as_str() {
                    "included_files" | "include" => {
                        if self.config.includes.is_some() {
                            return Err(self.error(line, "duplicate included_files block"));
                        }
                        self.config.includes = Some(self.patterns()?);
                    }
                    "excluded_files" | "exclude" => {
                        if self.config.excludes.is_some() {
                            return Err(self.error(line, "duplicate excluded_files block"));
                        }
                        self.config.excludes = Some(self.patterns()?);
                    }
                    _ => return Err(self.error(line, format!("unknown nested block '{name}'"))),
                }
                continue;
            }
            if self.peek() != Some('=') {
                return Err(self.error(line, "expected a setting in the form key = value"));
            }
            self.bump();
            let value = self.value()?;
            if !seen.insert(name.clone()) {
                return Err(self.error(line, format!("duplicate setting '{name}'")));
            }
            match name.as_str() {
                "glyphs" => {
                    let wanted = self.boolean(line, &name, &value)?;
                    if !wanted {
                        return Err(self.error(
                            line,
                            "glyphs = false is not applied until core purge options land (wave 2)",
                        ));
                    }
                    self.config.glyphs = wanted;
                }
                "docstrings" => {
                    let wanted = self.boolean(line, &name, &value)?;
                    if !wanted {
                        return Err(self.error(
                            line,
                            "docstrings = false is not applied until core purge options land (wave 2)",
                        ));
                    }
                    self.config.docstrings = wanted;
                }
                "first_sentence" => {
                    let wanted = self.boolean(line, &name, &value)?;
                    if !wanted {
                        return Err(self.error(
                            line,
                            "first_sentence = false (aggressive) is not applied until core purge options land (wave 2)",
                        ));
                    }
                    self.config.first_sentence = wanted;
                }
                "generated" => {
                    self.config.generated = match value.as_str() {
                        "skip" => Generated::Skip,
                        "purge" => Generated::Purge,
                        _ => return Err(self.error(line, "generated must be skip or purge")),
                    };
                }
                "respect_gitignore" => {
                    self.config.respect_gitignore = self.boolean(line, &name, &value)?;
                }
                _ => {
                    return Err(self.error(line, format!("unknown global setting '{name}'")));
                }
            }
        }
    }

    fn language_settings(&mut self) -> Result<LanguageConfig, ConfigError> {
        let mut local = LanguageConfig::default();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        loop {
            if !self.skip() {
                return Err(self.error(self.line, "unterminated configuration block"));
            }
            if self.peek() == Some('}') {
                self.bump();
                return Ok(local);
            }
            let (name, line) = self.setting_head()?;
            if self.peek() == Some('{') {
                self.bump();
                if !seen.insert(name.clone()) {
                    return Err(self.error(line, format!("duplicate setting '{name}'")));
                }
                match name.as_str() {
                    "included_files" | "include" => {
                        if local.includes.is_some() {
                            return Err(self.error(line, "duplicate included_files block"));
                        }
                        local.includes = Some(self.patterns()?);
                    }
                    "excluded_files" | "exclude" => {
                        if local.excludes.is_some() {
                            return Err(self.error(line, "duplicate excluded_files block"));
                        }
                        local.excludes = Some(self.patterns()?);
                    }
                    _ => return Err(self.error(line, format!("unknown nested block '{name}'"))),
                }
                continue;
            }
            if self.peek() != Some('=') {
                return Err(self.error(line, "expected a setting in the form key = value"));
            }
            self.bump();
            let value = self.value()?;
            if !seen.insert(name.clone()) {
                return Err(self.error(line, format!("duplicate setting '{name}'")));
            }
            match name.as_str() {
                "enabled" => local.enabled = self.boolean(line, &name, &value)?,
                "directives" => {
                    return Err(self.error(
                        line,
                        "directives is not applied until core purge options land (wave 2)",
                    ));
                }
                _ => {
                    return Err(self.error(line, format!("unknown language setting '{name}'")));
                }
            }
        }
    }

    fn setting_head(&mut self) -> Result<(String, usize), ConfigError> {
        let start = self.offset;
        let line = self.line;
        while self
            .peek()
            .is_some_and(|ch| !matches!(ch, '=' | '{' | '}' | '\n' | '#'))
        {
            self.bump();
        }
        let name = self.source[start..self.offset].trim().replace('-', "_");
        if name.is_empty() || name.chars().any(char::is_whitespace) {
            return Err(self.error(line, "expected a setting in the form key = value"));
        }
        Ok((name, line))
    }

    fn boolean(&self, line: usize, name: &str, value: &str) -> Result<bool, ConfigError> {
        match value {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(self.error(line, format!("{name} must be true or false"))),
        }
    }

    fn value(&mut self) -> Result<String, ConfigError> {
        while self.peek().is_some_and(|ch| ch == ' ' || ch == '\t') {
            self.bump();
        }
        let line = self.line;
        if matches!(self.peek(), Some('"' | '\'')) {
            let quote = self.bump().unwrap_or('"');
            let mut value = String::new();
            loop {
                match self.bump() {
                    Some(ch) if ch == quote => break,
                    Some('\\') => match self.bump() {
                        Some(ch) if ch == quote || ch == '\\' => value.push(ch),
                        Some(ch) => {
                            value.push('\\');
                            value.push(ch);
                        }
                        None => return Err(self.error(line, "unterminated quoted value")),
                    },
                    Some('\n') | None => {
                        return Err(self.error(line, "unterminated quoted value"));
                    }
                    Some(ch) => value.push(ch),
                }
            }
            while self
                .peek()
                .is_some_and(|ch| ch == ' ' || ch == '\t' || ch == '\r')
            {
                self.bump();
            }
            if self
                .peek()
                .is_some_and(|ch| !matches!(ch, '\n' | '#' | '}'))
            {
                return Err(self.error(line, "unexpected text after quoted value"));
            }
            return Ok(value);
        }
        let start = self.offset;
        while self
            .peek()
            .is_some_and(|ch| !matches!(ch, '\n' | '}' | '#'))
        {
            self.bump();
        }
        let value = self.source[start..self.offset].trim();
        if value.is_empty() {
            return Err(self.error(line, "missing setting value"));
        }
        Ok(value.to_string())
    }

    fn patterns(&mut self) -> Result<Arc<Patterns>, ConfigError> {
        let mut builder = GitignoreBuilder::new(self.root);
        loop {
            if !self.skip() {
                return Err(self.error(self.line, "unterminated file-pattern block"));
            }
            if self.peek() == Some('}') {
                self.bump();
                break;
            }
            let start = self.offset;
            let line = self.line;
            while self.peek().is_some_and(|ch| ch != '\n') {
                self.bump();
            }
            let pattern = self.source[start..self.offset].trim();
            if !pattern.is_empty() && !pattern.starts_with('#') {
                builder
                    .add_line(Some(self.path.to_path_buf()), pattern)
                    .map_err(|error| self.error(line, error))?;
            }
            if self.peek() == Some('\n') {
                self.bump();
            }
        }
        let matcher = builder
            .build()
            .map_err(|error| self.error(self.line, error))?;
        Ok(Arc::new(Patterns {
            root: self.root.to_path_buf(),
            matcher,
        }))
    }
}
