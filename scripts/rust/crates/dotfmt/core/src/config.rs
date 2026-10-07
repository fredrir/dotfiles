use std::collections::{BTreeMap, HashMap};
use std::fmt::Display;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

mod parse;

pub const CONFIG_NAME: &str = "dotfmt.dotfile";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Language {
    Conf,
    Json,
    Lua,
    Markdown,
}

impl Language {
    pub const ALL: [Self; 4] = [Self::Conf, Self::Json, Self::Lua, Self::Markdown];

    pub fn name(self) -> &'static str {
        match self {
            Self::Conf => "conf",
            Self::Json => "json",
            Self::Lua => "lua",
            Self::Markdown => "markdown",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "conf" => Ok(Self::Conf),
            "json" => Ok(Self::Json),
            "lua" => Ok(Self::Lua),
            "md" | "markdown" => Ok(Self::Markdown),
            _ => Err(format!(
                "unknown language '{value}'; expected conf, json, lua, or markdown (md)"
            )),
        }
    }

    pub fn for_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "conf" | "config" | "dotfile" => Some(Self::Conf),
            "json" | "jsonc" | "hujson" | "jwcc" => Some(Self::Json),
            "lua" | "luau" => Some(Self::Lua),
            "md" | "markdown" | "mdown" | "mkd" => Some(Self::Markdown),
            _ => None,
        }
    }
}

pub type Settings = BTreeMap<String, Setting>;

#[derive(Clone, Debug)]
pub struct Setting {
    pub value: String,
    pub source: PathBuf,
    pub line: usize,
    pub global: bool,
}

impl Setting {
    pub fn error(&self, message: impl Display) -> String {
        format!("{}:{}: {message}", self.source.display(), self.line)
    }
}

#[derive(Clone, Debug, Default)]
pub struct LanguageConfig {
    pub enabled: bool,
    pub settings: Settings,
}

#[derive(Clone, Debug)]
pub struct Effective {
    pub languages: BTreeMap<Language, LanguageConfig>,
    cwd: PathBuf,
    includes: Patterns,
    excludes: Patterns,
    language_filters: BTreeMap<Language, (Patterns, Patterns)>,
}

impl Default for Effective {
    fn default() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            languages: Language::ALL
                .into_iter()
                .map(|language| (language, LanguageConfig::default()))
                .collect(),
            includes: Patterns::default(),
            excludes: Patterns::default(),
            language_filters: BTreeMap::new(),
        }
    }
}

impl Effective {
    pub fn settings(&self, language: Language) -> &Settings {
        &self.languages[&language].settings
    }

    pub fn select(
        &self,
        path: &Path,
        forced: Option<Language>,
    ) -> Result<Option<Language>, String> {
        self.select_languages(path, &[], forced)
    }

    /// Restricts ownership to the requested languages before detecting conflicts.
    /// A forced language additionally permits an explicitly named unfamiliar file.
    pub fn select_languages(
        &self,
        path: &Path,
        requested: &[Language],
        forced: Option<Language>,
    ) -> Result<Option<Language>, String> {
        let absolute = absolute(&self.cwd, path)?;
        let path = absolute.as_path();
        if (!self.includes.is_empty() && !self.includes.matches(path, false).unwrap_or(false))
            || self.excludes.matches(path, true).unwrap_or(false)
        {
            return Ok(None);
        }
        let builtin = Language::for_path(path);
        let mut selected = None;
        for (&language, config) in &self.languages {
            if !config.enabled
                || (!requested.is_empty() && !requested.contains(&language))
                || forced.is_some_and(|forced| forced != language)
            {
                continue;
            }
            let filters = self.language_filters.get(&language);
            if filters.is_some_and(|(_, excluded)| excluded.matches(path, true).unwrap_or(false)) {
                continue;
            }
            let included = filters.and_then(|(included, _)| included.matches(path, false));
            if included == Some(false) {
                continue;
            }
            if forced.is_some() || builtin == Some(language) || included == Some(true) {
                if let Some(previous) = selected {
                    return Err(format!(
                        "{}: ambiguous language mapping: {} and {}; use --lang to select one",
                        path.display(),
                        Language::name(previous),
                        language.name()
                    ));
                }
                selected = Some(language);
            }
        }
        Ok(selected)
    }

    fn apply(&mut self, layer: Layer) {
        for config in self.languages.values_mut() {
            config.settings.extend(layer.global.clone());
        }
        self.includes.extend(layer.includes);
        self.excludes.extend(layer.excludes);
        for (language, local) in layer.languages {
            let config = self.languages.entry(language).or_default();
            config.enabled = local.enabled;
            config.settings.extend(local.settings);
            let filters = self.language_filters.entry(language).or_default();
            filters.0.extend(local.includes);
            filters.1.extend(local.excludes);
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Patterns(Vec<Arc<PatternLayer>>);

#[derive(Debug)]
struct PatternLayer {
    root: PathBuf,
    matcher: Gitignore,
}

impl Patterns {
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn extend(&mut self, layer: Option<Arc<PatternLayer>>) {
        if let Some(layer) = layer {
            if layer.matcher.is_empty() {
                self.0.clear();
            } else {
                self.0.push(layer);
            }
        }
    }

    fn matches(&self, path: &Path, excluded: bool) -> Option<bool> {
        let mut matched = None;
        // A negated file cannot reopen an excluded parent directory. Match
        // parent directories too, with the most local rule winning at each
        // path. Walking explicitly also avoids ignore's outside-root panic.
        for (depth, candidate) in path.ancestors().enumerate() {
            if candidate.parent().is_none() {
                break;
            }
            let result = self.0.iter().rev().find_map(|layer| {
                if candidate == layer.root || layer.root.starts_with(candidate) {
                    return None;
                }
                let result = layer.matcher.matched(candidate, depth != 0);
                if result.is_ignore() {
                    Some(true)
                } else if result.is_whitelist() {
                    Some(false)
                } else {
                    None
                }
            });
            if let Some(result) = result {
                if !excluded || result {
                    return Some(result);
                }
                matched = Some(false);
            }
        }
        matched
    }
}

#[derive(Default)]
struct Layer {
    global: Settings,
    languages: BTreeMap<Language, Local>,
    includes: Option<Arc<PatternLayer>>,
    excludes: Option<Arc<PatternLayer>>,
}

struct Local {
    enabled: bool,
    settings: Settings,
    includes: Option<Arc<PatternLayer>>,
    excludes: Option<Arc<PatternLayer>>,
}

type Resolution = Result<Arc<Effective>, String>;
type Cached = Arc<OnceLock<Resolution>>;

pub struct Resolver {
    cwd: PathBuf,
    global: Option<PathBuf>,
    base: OnceLock<Resolution>,
    directories: Mutex<HashMap<PathBuf, Cached>>,
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new()
    }
}

impl Resolver {
    pub fn new() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|value| !value.is_empty())
                    .map(|home| PathBuf::from(home).join(".config"))
            });
        Self::with_paths(cwd, base.map(|base| base.join("dotfmt").join(CONFIG_NAME)))
    }

    /// Constructs an isolated resolver without changing the process environment.
    pub fn with_paths(cwd: PathBuf, global_config: Option<PathBuf>) -> Self {
        Self {
            cwd,
            global: global_config,
            base: OnceLock::new(),
            directories: Mutex::new(HashMap::new()),
        }
    }

    pub fn for_file(&self, path: &Path) -> Resolution {
        let path = self.absolute_path(path)?;
        self.for_directory(path.parent().unwrap_or(&self.cwd))
    }

    pub fn for_directory(&self, path: &Path) -> Resolution {
        let directory = self.absolute_path(path)?;
        let cell = self
            .directories
            .lock()
            .map_err(|_| "configuration cache lock poisoned".to_string())?
            .entry(directory.clone())
            .or_default()
            .clone();
        cell.get_or_init(|| {
            let inherited = if let Some(parent) = directory.parent() {
                self.for_directory(parent)?
            } else {
                self.base
                    .get_or_init(|| {
                        let mut effective = Effective {
                            cwd: self.cwd.clone(),
                            ..Effective::default()
                        };
                        if let Some(global) = &self.global
                            && let Some(layer) = read_layer(global, &self.cwd)?
                        {
                            effective.apply(layer);
                        }
                        Ok(Arc::new(effective))
                    })
                    .clone()?
            };
            let config = directory.join(CONFIG_NAME);
            if self.global.as_ref() == Some(&config) {
                return Ok(inherited);
            }
            let Some(layer) = read_layer(&config, &directory)? else {
                return Ok(inherited);
            };
            let mut effective = (*inherited).clone();
            effective.apply(layer);
            Ok(Arc::new(effective))
        })
        .clone()
    }

    /// Resolves relative and parent components using the same path semantics as
    /// configuration discovery, without following ordinary symlink targets.
    pub fn absolute_path(&self, path: &Path) -> Result<PathBuf, String> {
        absolute(&self.cwd, path)
    }
}

fn absolute(cwd: &Path, path: &Path) -> Result<PathBuf, String> {
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
                // Only resolve links at this boundary; ordinary link paths keep
                // their lexical spelling and configuration beside the link.
                if std::fs::symlink_metadata(&normalized)
                    .is_ok_and(|metadata| metadata.file_type().is_symlink())
                {
                    normalized = std::fs::canonicalize(&normalized)
                        .map_err(|error| format!("{}: {error}", normalized.display()))?;
                }
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    Ok(normalized)
}

fn read_layer(path: &Path, root: &Path) -> Result<Option<Layer>, String> {
    match std::fs::read_to_string(path) {
        Ok(source) => parse::parse(&source, path, root).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}
