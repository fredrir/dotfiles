use std::collections::BTreeMap;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::language::Language;
use patterns::{PatternLayer, Patterns};
use resolve::absolute;

mod parse;
mod patterns;
mod resolve;

pub use resolve::Resolver;

pub const CONFIG_NAME: &str = "dotfmt.dotfile";

pub type Settings = BTreeMap<String, Setting>;

#[derive(Clone, Debug)]
pub struct Setting {
    pub value: String,
    pub source: PathBuf,
    pub line: usize,
    pub global: bool,
}

impl Setting {
    pub fn error(&self, message: impl Display) -> Diagnostic {
        Diagnostic::config(&self.source, self.line, message.to_string())
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
    sources: Vec<PathBuf>,
}

/// Why a file is or is not formatted under an effective configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    Language(Language),
    Unmapped,
    Excluded,
    Disabled(Language),
}

impl Selection {
    pub fn language(self) -> Option<Language> {
        match self {
            Self::Language(language) => Some(language),
            _ => None,
        }
    }
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
            sources: Vec::new(),
        }
    }
}

impl Effective {
    pub fn settings(&self, language: Language) -> &Settings {
        &self.languages[&language].settings
    }

    /// Configuration files applied to this configuration, outermost first.
    pub fn sources(&self) -> &[PathBuf] {
        &self.sources
    }

    pub fn select(
        &self,
        path: &Path,
        forced: Option<Language>,
    ) -> Result<Option<Language>, Diagnostic> {
        self.select_languages(path, &[], forced)
    }

    /// Restricts ownership to the requested languages before detecting conflicts.
    /// A forced language additionally permits an explicitly named unfamiliar file.
    pub fn select_languages(
        &self,
        path: &Path,
        requested: &[Language],
        forced: Option<Language>,
    ) -> Result<Option<Language>, Diagnostic> {
        Ok(self.selection(path, requested, forced)?.language())
    }

    pub fn selection(
        &self,
        path: &Path,
        requested: &[Language],
        forced: Option<Language>,
    ) -> Result<Selection, Diagnostic> {
        let absolute = absolute(&self.cwd, path)?;
        let path = absolute.as_path();
        if (!self.includes.is_empty() && !self.includes.matches(path, false).unwrap_or(false))
            || self.excludes.matches(path, true).unwrap_or(false)
        {
            return Ok(Selection::Excluded);
        }
        let builtin = Language::for_path(path);
        let mut selected = None;
        let mut excluded = false;
        for (&language, config) in &self.languages {
            if !config.enabled
                || (!requested.is_empty() && !requested.contains(&language))
                || forced.is_some_and(|forced| forced != language)
            {
                continue;
            }
            let filters = self.language_filters.get(&language);
            let included = filters.and_then(|(included, _)| included.matches(path, false));
            if !(forced.is_some() || builtin == Some(language) || included == Some(true)) {
                continue;
            }
            if included == Some(false)
                || filters.is_some_and(|(_, excluded)| excluded.matches(path, true).unwrap_or(false))
            {
                excluded = true;
                continue;
            }
            if let Some(previous) = selected {
                return Err(Diagnostic::new(
                    DiagnosticKind::Configuration,
                    format!(
                        "ambiguous language mapping: {} and {}; use --lang to select one",
                        Language::name(previous),
                        language.name()
                    ),
                )
                .with_path(path));
            }
            selected = Some(language);
        }
        Ok(match (selected, builtin) {
            (Some(language), _) => Selection::Language(language),
            _ if excluded => Selection::Excluded,
            (None, Some(language))
                if !self
                    .languages
                    .get(&language)
                    .is_some_and(|config| config.enabled) =>
            {
                Selection::Disabled(language)
            }
            _ => Selection::Unmapped,
        })
    }

    fn apply(&mut self, layer: Layer) {
        self.sources.extend(layer.source);
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

#[derive(Default)]
struct Layer {
    source: Option<PathBuf>,
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
