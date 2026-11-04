use std::path::Path;

use super::{Repairs, configuration};
use dotfmt_core::config::{Effective, Settings};
use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};
use dotfmt_core::language::Language;
use dotfmt_core::repair::Repaired;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

const REPAIR: &str = "repair";

#[derive(Clone)]
pub struct Engine {
    kind: Kind,
    repair: bool,
}

#[derive(Clone)]
enum Kind {
    Bib(dotfmt_bib::config::Config),
    Conf(dotfmt_conf::config::Config),
    Env(dotfmt_env::config::Config),
    Ignore(
        dotfmt_ignore::config::Config,
        dotfmt_ignore::dialect::Dialect,
    ),
    Ini(dotfmt_ini::config::Config, dotfmt_ini::dialect::Dialect),
    Json(dotfmt_json::config::Config, dotfmt_json::dialect::Dialect),
    Latex(dotfmt_latex::config::Config, dotfmt_latex::dialect::Dialect),
    Lua(dotfmt_lua::config::Config, dotfmt_lua::dialect::Dialect),
    Markdown(
        dotfmt_markdown::config::Config,
        dotfmt_markdown::dialect::Dialect,
    ),
    Plist(dotfmt_plist::config::Config),
    Toml(dotfmt_toml::config::Config),
    Yaml(dotfmt_yaml::config::Config),
}

pub enum Formatted {
    Text { text: String, repairs: Repairs },
    Verbatim,
}

impl Engine {
    pub fn new(
        language: Language,
        settings: &Settings,
        dialect: Option<&str>,
    ) -> Result<Self, Diagnostic> {
        let repair = match settings.get(REPAIR) {
            None => true,
            Some(setting) => match setting.value.as_str() {
                "true" => true,
                "false" => false,
                other => {
                    return Err(setting.error(format!("repair must be true or false, not {other}")));
                }
            },
        };
        let settings: Settings = settings
            .iter()
            .filter(|(key, _)| *key != REPAIR)
            .map(|(key, setting)| (key.clone(), setting.clone()))
            .collect();
        let settings = &settings;
        let kind = match language {
            Language::Bib => Kind::Bib(dotfmt_bib::config::Config::from_settings(settings)?),
            Language::Conf => Kind::Conf(dotfmt_conf::config::Config::from_settings(settings)?),
            Language::Env => Kind::Env(dotfmt_env::config::Config::from_settings(settings)?),
            Language::Ignore => Kind::Ignore(
                dotfmt_ignore::config::Config::from_settings(settings)?,
                dotfmt_ignore::dialect::Dialect::Auto,
            ),
            Language::Ini => Kind::Ini(
                dotfmt_ini::config::Config::from_settings(settings)?,
                dotfmt_ini::dialect::Dialect::Auto,
            ),
            Language::Json => Kind::Json(
                dotfmt_json::config::Config::from_settings(settings)?,
                dotfmt_json::dialect::Dialect::Auto,
            ),
            Language::Latex => Kind::Latex(
                dotfmt_latex::config::Config::from_settings(settings)?,
                dotfmt_latex::dialect::Dialect::Auto,
            ),
            Language::Lua => Kind::Lua(
                dotfmt_lua::config::Config::from_settings(settings)?,
                dotfmt_lua::dialect::Dialect::Auto,
            ),
            Language::Markdown => Kind::Markdown(
                dotfmt_markdown::config::Config::from_settings(settings)?,
                dotfmt_markdown::dialect::Dialect::Auto,
            ),
            Language::Plist => Kind::Plist(dotfmt_plist::config::Config::from_settings(settings)?),
            Language::Toml => Kind::Toml(dotfmt_toml::config::Config::from_settings(settings)?),
            Language::Yaml => Kind::Yaml(dotfmt_yaml::config::Config::from_settings(settings)?),
        };
        Self { kind, repair }.with_dialect(dialect)
    }

    pub fn with_dialect(mut self, dialect: Option<&str>) -> Result<Self, Diagnostic> {
        let value = dialect.unwrap_or("auto");
        match &mut self.kind {
            Kind::Bib(_) => {
                if !value.eq_ignore_ascii_case("auto") {
                    return Err(configuration(format!("bib has no {value:?} dialect")));
                }
            }
            Kind::Conf(_) => {
                if !value.eq_ignore_ascii_case("auto") {
                    return Err(configuration(format!("conf has no {value:?} dialect")));
                }
            }
            Kind::Env(_) => {
                if !value.eq_ignore_ascii_case("auto") {
                    return Err(configuration(format!("env has no {value:?} dialect")));
                }
            }
            Kind::Plist(_) => {
                if !value.eq_ignore_ascii_case("auto") {
                    return Err(configuration(format!("plist has no {value:?} dialect")));
                }
            }
            Kind::Toml(_) => {
                if !value.eq_ignore_ascii_case("auto") {
                    return Err(configuration(format!("toml has no {value:?} dialect")));
                }
            }
            Kind::Yaml(_) => {
                if !value.eq_ignore_ascii_case("auto") {
                    return Err(configuration(format!("yaml has no {value:?} dialect")));
                }
            }
            Kind::Ignore(_, dialect) => {
                *dialect =
                    dotfmt_ignore::dialect::Dialect::parse(value, true).map_err(configuration)?
            }
            Kind::Ini(_, dialect) => {
                *dialect =
                    dotfmt_ini::dialect::Dialect::parse(value, true).map_err(configuration)?
            }
            Kind::Json(_, dialect) => {
                *dialect =
                    dotfmt_json::dialect::Dialect::parse(value, true).map_err(configuration)?
            }
            Kind::Latex(_, dialect) => {
                *dialect =
                    dotfmt_latex::dialect::Dialect::parse(value, true).map_err(configuration)?
            }
            Kind::Lua(_, dialect) => {
                *dialect =
                    dotfmt_lua::dialect::Dialect::parse(value, true).map_err(configuration)?
            }
            Kind::Markdown(_, dialect) => {
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
        detected: dotfmt_markdown::dialect::Dialect,
        clean: bool,
    ) -> Result<Formatted, Diagnostic> {
        let unrepaired = |text| {
            Ok(Formatted::Text {
                text,
                repairs: Repairs::default(),
            })
        };
        match &self.kind {
            Kind::Bib(config) => unrepaired(dotfmt_bib::format(utf8(input)?, config)?),
            Kind::Conf(config) => unrepaired(dotfmt_conf::format(path, input, config)?),
            Kind::Env(config) => unrepaired(dotfmt_env::format(utf8(input)?, config)?),
            Kind::Ignore(config, dialect) => {
                let mut config = config.clone();
                let detected = dotfmt_ignore::dialect::Dialect::detect(path);
                config.dialect = dialect.resolve(config.dialect, detected);
                unrepaired(dotfmt_ignore::format(path, input, &config)?)
            }
            Kind::Ini(config, dialect) => {
                let mut config = config.clone();
                config.dialect = dialect.resolve(config.dialect, path);
                unrepaired(dotfmt_ini::format(path, input, &config)?)
            }
            Kind::Json(config, dialect) => {
                let output = dotfmt_json::format(
                    path,
                    input,
                    config,
                    self.repair,
                    if *dialect == dotfmt_json::dialect::Dialect::Auto {
                        config.dialect
                    } else {
                        *dialect
                    },
                )?;
                Ok(Formatted::Text {
                    text: output.text,
                    repairs: output.repairs,
                })
            }
            Kind::Latex(config, dialect) => unrepaired(dotfmt_latex::format(
                utf8(input)?,
                config,
                dialect.resolve(config.dialect, path),
            )?),
            Kind::Lua(config, dialect) => {
                let dialect = dialect.resolve(config.dialect, path);
                self.settled(
                    utf8(input)?,
                    |text| dotfmt_lua::format_with_dialect(text, config, dialect),
                    |text| dotfmt_lua::repair(text, dialect),
                )
            }
            Kind::Markdown(config, dialect) => {
                let mut config = config.clone();
                config.dialect = dialect.resolve(config.dialect, detected);
                let formatted = dotfmt_markdown::format(utf8(input)?, &config)?;
                unrepaired(if clean {
                    dotfmt_markdown::clean(&formatted, &config)
                } else {
                    formatted
                })
            }
            Kind::Plist(config) => match dotfmt_plist::format(input, config)? {
                dotfmt_plist::Output::Text(text) => unrepaired(text),
                dotfmt_plist::Output::Verbatim => Ok(Formatted::Verbatim),
            },
            Kind::Toml(config) => self.settled(
                utf8(input)?,
                |text| dotfmt_toml::format(text, config),
                dotfmt_toml::repair,
            ),
            Kind::Yaml(config) => self.settled(
                utf8(input)?,
                |text| dotfmt_yaml::format(text, config),
                dotfmt_yaml::repair,
            ),
        }
    }

    /// Repairs only once a syntax error refuses the input, so valid input parses once.
    fn settled(
        &self,
        input: &str,
        format: impl Fn(&str) -> Result<String, Diagnostic>,
        repair: impl Fn(&str) -> Repaired,
    ) -> Result<Formatted, Diagnostic> {
        match format(input) {
            Err(error) if self.repair && error.kind == DiagnosticKind::Syntax => {
                let repaired = repair(input);
                if repaired.repairs.is_empty() {
                    return Err(error);
                }
                Ok(Formatted::Text {
                    text: format(&repaired.text)?,
                    repairs: repaired.repairs,
                })
            }
            result => result.map(|text| Formatted::Text {
                text,
                repairs: Repairs::default(),
            }),
        }
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
