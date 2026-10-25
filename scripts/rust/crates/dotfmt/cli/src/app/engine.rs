use std::path::Path;

use super::{Repairs, configuration};
use dotfmt_core::config::{Effective, Settings};
use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};
use dotfmt_core::language::Language;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub enum Engine {
    Bib(dotfmt_bib::config::Config),
    Conf(dotfmt_conf::config::Config),
    Json(dotfmt_json::config::Config, dotfmt_json::dialect::Dialect),
    Latex(dotfmt_latex::config::Config, dotfmt_latex::dialect::Dialect),
    Lua(dotfmt_lua::config::Config, dotfmt_lua::dialect::Dialect),
    Markdown(
        dotfmt_markdown::config::Config,
        dotfmt_markdown::dialect::Dialect,
    ),
    Toml(dotfmt_toml::config::Config),
    Yaml(dotfmt_yaml::config::Config),
}

pub struct Formatted {
    pub text: String,
    pub repairs: Repairs,
}

impl Engine {
    pub fn new(
        language: Language,
        settings: &Settings,
        dialect: Option<&str>,
    ) -> Result<Self, Diagnostic> {
        let engine = match language {
            Language::Bib => Self::Bib(dotfmt_bib::config::Config::from_settings(settings)?),
            Language::Conf => Self::Conf(dotfmt_conf::config::Config::from_settings(settings)?),
            Language::Json => Self::Json(
                dotfmt_json::config::Config::from_settings(settings)?,
                dotfmt_json::dialect::Dialect::Auto,
            ),
            Language::Latex => Self::Latex(
                dotfmt_latex::config::Config::from_settings(settings)?,
                dotfmt_latex::dialect::Dialect::Auto,
            ),
            Language::Lua => Self::Lua(
                dotfmt_lua::config::Config::from_settings(settings)?,
                dotfmt_lua::dialect::Dialect::Auto,
            ),
            Language::Markdown => Self::Markdown(
                dotfmt_markdown::config::Config::from_settings(settings)?,
                dotfmt_markdown::dialect::Dialect::Auto,
            ),
            Language::Toml => Self::Toml(dotfmt_toml::config::Config::from_settings(settings)?),
            Language::Yaml => Self::Yaml(dotfmt_yaml::config::Config::from_settings(settings)?),
        };
        engine.with_dialect(dialect)
    }

    pub fn with_dialect(mut self, dialect: Option<&str>) -> Result<Self, Diagnostic> {
        let value = dialect.unwrap_or("auto");
        match &mut self {
            Self::Bib(_) => {
                if !value.eq_ignore_ascii_case("auto") {
                    return Err(configuration(format!("bib has no {value:?} dialect")));
                }
            }
            Self::Conf(_) => {
                if !value.eq_ignore_ascii_case("auto") {
                    return Err(configuration(format!("conf has no {value:?} dialect")));
                }
            }
            Self::Toml(_) => {
                if !value.eq_ignore_ascii_case("auto") {
                    return Err(configuration(format!("toml has no {value:?} dialect")));
                }
            }
            Self::Yaml(_) => {
                if !value.eq_ignore_ascii_case("auto") {
                    return Err(configuration(format!("yaml has no {value:?} dialect")));
                }
            }
            Self::Json(_, dialect) => {
                *dialect =
                    dotfmt_json::dialect::Dialect::parse(value, true).map_err(configuration)?
            }
            Self::Latex(_, dialect) => {
                *dialect =
                    dotfmt_latex::dialect::Dialect::parse(value, true).map_err(configuration)?
            }
            Self::Lua(_, dialect) => {
                *dialect =
                    dotfmt_lua::dialect::Dialect::parse(value, true).map_err(configuration)?
            }
            Self::Markdown(_, dialect) => {
                *dialect =
                    dotfmt_markdown::dialect::Dialect::parse(value, true).map_err(configuration)?
            }
        }
        Ok(self)
    }

    pub fn format(
        &self,
        path: &Path,
        input: &[u8],
        editor: bool,
        detected: dotfmt_markdown::dialect::Dialect,
        clean: bool,
    ) -> Result<Formatted, Diagnostic> {
        let mut repairs = Repairs::default();
        let text = match self {
            Self::Bib(config) => dotfmt_bib::format(utf8(input)?, config)?,
            Self::Conf(config) => dotfmt_conf::format(path, input, config)?,
            Self::Json(config, dialect) => {
                let output = dotfmt_json::format(
                    path,
                    input,
                    config,
                    editor,
                    if *dialect == dotfmt_json::dialect::Dialect::Auto {
                        config.dialect
                    } else {
                        *dialect
                    }
                    .resolve(path),
                )?;
                repairs = output.repairs;
                output.text
            }
            Self::Latex(config, dialect) => {
                dotfmt_latex::format(utf8(input)?, config, dialect.resolve(config.dialect, path))?
            }
            Self::Lua(config, dialect) => dotfmt_lua::format_with_dialect(
                utf8(input)?,
                config,
                dialect.resolve(config.dialect, path),
            )?,
            Self::Markdown(config, dialect) => {
                let mut config = config.clone();
                config.dialect = dialect.resolve(config.dialect, detected);
                let formatted = dotfmt_markdown::format(utf8(input)?, &config)?;
                if clean {
                    dotfmt_markdown::clean(&formatted, &config)
                } else {
                    formatted
                }
            }
            Self::Toml(config) => dotfmt_toml::format(utf8(input)?, config)?,
            Self::Yaml(config) => dotfmt_yaml::format(utf8(input)?, config)?,
        };
        Ok(Formatted { text, repairs })
    }
}

fn utf8(input: &[u8]) -> Result<&str, Diagnostic> {
    std::str::from_utf8(input).map_err(|_| Diagnostic::new(DiagnosticKind::Syntax, "not UTF-8"))
}

#[derive(Default)]
pub(super) struct Engines {
    validated: HashMap<usize, (Arc<Effective>, Result<(), Diagnostic>)>,
    configured: HashMap<(usize, Language), Arc<Engine>>,
    overridden: HashMap<(usize, Language, String), Arc<Engine>>,
}

impl Engines {
    pub fn validate(&mut self, config: &Arc<Effective>) -> Result<(), Diagnostic> {
        let id = Arc::as_ptr(config) as usize;
        self.validated
            .entry(id)
            .or_insert_with(|| {
                let result = (|| {
                    for (&language, settings) in &config.languages {
                        if settings.enabled {
                            let engine = Engine::new(language, &settings.settings, None)?;
                            self.configured.insert((id, language), Arc::new(engine));
                        }
                    }
                    Ok(())
                })();
                // Prepared plans can come from another Session. Retain the
                // configuration so its pointer cannot be reused as a cache key.
                (Arc::clone(config), result)
            })
            .1
            .clone()
    }

    pub fn get(
        &mut self,
        config: &Arc<Effective>,
        language: Language,
        dialect: Option<&str>,
    ) -> Result<Arc<Engine>, Diagnostic> {
        self.validate(config)?;
        let id = Arc::as_ptr(config) as usize;
        let configured = &self.configured[&(id, language)];
        let Some(dialect) = dialect else {
            return Ok(Arc::clone(configured));
        };
        let key = (id, language, dialect.to_owned());
        if let Some(found) = self.overridden.get(&key) {
            return Ok(Arc::clone(found));
        }
        let engine = Arc::new((**configured).clone().with_dialect(Some(dialect))?);
        self.overridden.insert(key, Arc::clone(&engine));
        Ok(engine)
    }
}

#[derive(Default)]
pub(super) struct DirectoryContext {
    markdown: HashMap<PathBuf, dotfmt_markdown::dialect::Dialect>,
}

impl DirectoryContext {
    pub fn markdown(&mut self, path: &Path) -> dotfmt_markdown::dialect::Dialect {
        use dotfmt_markdown::dialect::Dialect;
        let directory = path.parent().unwrap_or(path);
        let mut pending = Vec::new();
        let mut detected = Dialect::Gfm;
        for ancestor in directory.ancestors() {
            if let Some(cached) = self.markdown.get(ancestor) {
                detected = *cached;
                break;
            }
            pending.push(ancestor.to_path_buf());
            if ancestor.join(".obsidian").is_dir() {
                detected = Dialect::Obsidian;
                break;
            }
        }
        for directory in pending {
            self.markdown.insert(directory, detected);
        }
        detected
    }
}
