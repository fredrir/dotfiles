use std::path::Path;

use super::{Repairs, configuration};
use dotfmt_core::config::{Effective, Settings};
use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};
use dotfmt_core::edit::{Fix, apply_fixes};
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
#[allow(clippy::large_enum_variant)]
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
    TypeScript(
        dotfmt_typescript::config::Config,
        dotfmt_typescript::dialect::Dialect,
    ),
    Javascript(
        dotfmt_typescript::config::Config,
        dotfmt_typescript::dialect::Dialect,
    ),
    Css(dotfmt_css::config::Config, dotfmt_css::dialect::Dialect),
    Html(dotfmt_html::config::Config, dotfmt_html::dialect::Dialect),
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
            Language::TypeScript => Kind::TypeScript(
                dotfmt_typescript::config::Config::from_settings(settings)?,
                dotfmt_typescript::dialect::Dialect::Auto,
            ),
            Language::Javascript => Kind::Javascript(
                dotfmt_typescript::config::Config::from_settings(settings)?,
                dotfmt_typescript::dialect::Dialect::Auto,
            ),
            Language::Css => Kind::Css(
                dotfmt_css::config::Config::from_settings(settings)?,
                dotfmt_css::dialect::Dialect::Auto,
            ),
            Language::Html => Kind::Html(
                dotfmt_html::config::Config::from_settings(settings)?,
                dotfmt_html::dialect::Dialect::Auto,
            ),
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
            Kind::TypeScript(_, dialect) => {
                *dialect = dotfmt_typescript::dialect::Dialect::parse(value, true)
                    .map_err(configuration)?;
            }
            Kind::Javascript(_, dialect) => {
                *dialect = dotfmt_typescript::dialect::Dialect::parse(value, true)
                    .map_err(configuration)?;
            }
            Kind::Css(_, dialect) => {
                *dialect =
                    dotfmt_css::dialect::Dialect::parse(value, true).map_err(configuration)?;
            }
            Kind::Html(_, dialect) => {
                *dialect =
                    dotfmt_html::dialect::Dialect::parse(value, true).map_err(configuration)?;
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

    /// The dialect `format` would use; JSON reports `auto` while comments still decide it.
    pub fn dialect(
        &self,
        path: &Path,
        context: &Path,
        detected: dotfmt_markdown::dialect::Dialect,
    ) -> Option<&'static str> {
        match &self.kind {
            Kind::Bib(_)
            | Kind::Conf(_)
            | Kind::Env(_)
            | Kind::Plist(_)
            | Kind::Toml(_)
            | Kind::Yaml(_) => None,
            Kind::TypeScript(config, dialect) => Some(dialect.resolve(config.dialect, path).name()),
            Kind::Javascript(config, dialect) => Some(dialect.resolve(config.dialect, path).name()),
            Kind::Css(config, dialect) => Some(dialect.resolve(config.dialect, path).name()),
            Kind::Html(config, dialect) => Some(dialect.resolve(config.dialect, path).name()),
            Kind::Ignore(config, dialect) => Some(
                dialect
                    .resolve(
                        config.dialect,
                        dotfmt_ignore::dialect::Dialect::detect(context),
                    )
                    .name(),
            ),
            Kind::Ini(config, dialect) => Some(dialect.resolve(config.dialect, context).name()),
            Kind::Json(config, dialect) => {
                use dotfmt_json::dialect::Dialect;
                let chosen = if *dialect == Dialect::Auto {
                    config.dialect
                } else {
                    *dialect
                };
                Some(
                    if chosen == Dialect::Auto && chosen.resolve(path) == Dialect::Json {
                        Dialect::Auto
                    } else {
                        chosen.resolve(path)
                    }
                    .name(),
                )
            }
            Kind::Latex(config, dialect) => Some(dialect.resolve(config.dialect, path).name()),
            Kind::Lua(config, dialect) => Some(dialect.resolve(config.dialect, path).name()),
            Kind::Markdown(config, dialect) => {
                Some(dialect.resolve(config.dialect, detected).name())
            }
        }
    }

    /// Format a document, applying configured fixes first for TypeScript and
    /// JavaScript. A fix that cannot be analyzed or applied never fails the
    /// format: the raw formatter result is returned instead.
    pub fn format(
        &self,
        path: &Path,
        context: &Path,
        input: &[u8],
        detected: dotfmt_markdown::dialect::Dialect,
        clean: bool,
    ) -> Result<Formatted, Diagnostic> {
        let selection = match &self.kind {
            Kind::TypeScript(config, _) | Kind::Javascript(config, _) => {
                config.format_fix_selection()
            }
            _ => None,
        };
        let Some(selection) = selection else {
            return self.format_text(path, context, input, detected, clean);
        };
        self.format_with_fixes(path, context, input, detected, clean, selection)
    }

    fn format_with_fixes(
        &self,
        path: &Path,
        context: &Path,
        input: &[u8],
        detected: dotfmt_markdown::dialect::Dialect,
        clean: bool,
        selection: dotfmt_typescript::config::FixSelection,
    ) -> Result<Formatted, Diagnostic> {
        let Ok(text) = std::str::from_utf8(input) else {
            return self.format_text(path, context, input, detected, clean);
        };
        let (bom, text) = match text.strip_prefix('\u{feff}') {
            Some(text) => (true, text),
            None => (false, text),
        };
        let mut current = text.to_owned();
        let mut formatted = None;
        for _ in 0..3 {
            let analysis = match self.analyze(
                path,
                current.as_bytes(),
                true,
                selection.r#unsafe,
                Some(false),
                selection.remove_unused_exports,
            ) {
                Ok(analysis) => analysis,
                Err(_) => return self.format_text(path, context, input, detected, clean),
            };
            let selected: Vec<&Fix> = analysis
                .fixes
                .iter()
                .filter(|fix| {
                    selection.selects(
                        fix.finding.as_ref().map(|finding| finding.code.as_str()),
                        &fix.kind,
                    )
                })
                .collect();
            if selected.is_empty() {
                break;
            }
            let edited = match apply_fixes(&current, &selected) {
                Ok(edited) => edited,
                Err(_) => return self.format_text(path, context, input, detected, clean),
            };
            let with_bom = if bom {
                format!("\u{feff}{edited}")
            } else {
                edited
            };
            let result = match self.format_text(path, context, with_bom.as_bytes(), detected, clean)
            {
                Ok(result) => result,
                Err(_) => return self.format_text(path, context, input, detected, clean),
            };
            let Formatted::Text { text, repairs } = result else {
                return self.format_text(path, context, input, detected, clean);
            };
            let next = text
                .strip_prefix('\u{feff}')
                .map_or_else(|| text.clone(), str::to_owned);
            let changed = next != current;
            current = next;
            formatted = Some(Formatted::Text { text, repairs });
            if !changed {
                break;
            }
        }
        match formatted {
            Some(formatted) => Ok(formatted),
            None => self.format_text(path, context, input, detected, clean),
        }
    }

    /// The language formatter without configured fixes.
    pub(super) fn format_text(
        &self,
        path: &Path,
        context: &Path,
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
            Kind::Conf(config) => unrepaired(dotfmt_conf::format_with_context(
                path, context, input, config,
            )?),
            Kind::Env(config) => unrepaired(dotfmt_env::format(utf8(input)?, config)?),
            Kind::Ignore(config, dialect) => {
                let mut config = config.clone();
                let detected = dotfmt_ignore::dialect::Dialect::detect(context);
                config.dialect = dialect.resolve(config.dialect, detected);
                unrepaired(dotfmt_ignore::format_with_dialect(
                    utf8(input)?,
                    &config,
                    config.dialect,
                )?)
            }
            Kind::Ini(config, dialect) => {
                let mut config = config.clone();
                config.dialect = dialect.resolve(config.dialect, context);
                unrepaired(dotfmt_ini::format_with_dialect(
                    utf8(input)?,
                    &config,
                    config.dialect,
                )?)
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
            Kind::TypeScript(config, dialect) => {
                let mut config = config.clone();
                self.prepare_tailwind(
                    path,
                    utf8(input)?,
                    &mut config.tailwind,
                    &mut config.tailwind_sorter,
                )?;
                let dialect = dialect.resolve(config.dialect, path);
                unrepaired(dotfmt_typescript::format(
                    path,
                    utf8(input)?,
                    &config,
                    dialect,
                )?)
            }
            Kind::Javascript(config, dialect) => {
                let mut config = config.clone();
                self.prepare_tailwind(
                    path,
                    utf8(input)?,
                    &mut config.tailwind,
                    &mut config.tailwind_sorter,
                )?;
                let dialect = dialect.resolve(config.dialect, path);
                unrepaired(dotfmt_typescript::format(
                    path,
                    utf8(input)?,
                    &config,
                    dialect,
                )?)
            }
            Kind::Css(config, dialect) => {
                let mut config = config.clone();
                self.prepare_tailwind(
                    path,
                    utf8(input)?,
                    &mut config.tailwind,
                    &mut config.tailwind_sorter,
                )?;
                let dialect = dialect.resolve(config.dialect, path);
                unrepaired(dotfmt_css::format(path, utf8(input)?, &config, dialect)?)
            }
            Kind::Html(config, dialect) => {
                let mut config = config.clone();
                self.prepare_tailwind(
                    path,
                    utf8(input)?,
                    &mut config.tailwind,
                    &mut config.tailwind_sorter,
                )?;
                let dialect = dialect.resolve(config.dialect, path);
                unrepaired(dotfmt_html::format(path, utf8(input)?, &config, dialect)?)
            }
        }
    }

    /// Resolve the project's Tailwind order once per format call.
    ///
    /// `Mode::Auto` only engages for projects that actually use Tailwind. A
    /// missing Node/Tailwind falls back to the Rust pattern sorter; the
    /// `tailwind_order = project` setting turns that into a diagnostic instead.
    fn prepare_tailwind(
        &self,
        path: &Path,
        input: &str,
        config: &mut dotfmt_tailwind::Config,
        sorter: &mut Option<dotfmt_tailwind::Sorter>,
    ) -> Result<(), Diagnostic> {
        if sorter.is_some() || matches!(config.mode, dotfmt_tailwind::Mode::Off) {
            return Ok(());
        }
        let root = path.parent().unwrap_or(Path::new("."));
        let source_uses_tailwind = input.contains("@import \"tailwindcss\"")
            || input.contains("@tailwind")
            || input.contains("@theme")
            || input.contains("@utility");
        if matches!(config.mode, dotfmt_tailwind::Mode::Auto)
            && !source_uses_tailwind
            && !dotfmt_tailwind::bridge::is_tailwind_project(root)
        {
            return Ok(());
        }
        let trusted = dotfmt_tailwind::bridge::trusted()
            && std::env::var_os("DOTFMT_TRUSTED")
                .is_none_or(|value| value != "0" && value != "false");
        if !trusted {
            // Project Tailwind may run project JavaScript; untrusted editors
            // get the deterministic Rust order and never the merge bridge.
            let mut safe = config.clone();
            safe.merge = false;
            *sorter = Some(
                dotfmt_tailwind::Sorter::new(Arc::new(dotfmt_tailwind::OrderTable::empty()), safe)
                    .with_project_root(root),
            );
            return Ok(());
        }
        if matches!(config.order, dotfmt_tailwind::OrderSource::Rust) {
            *sorter = Some(
                dotfmt_tailwind::Sorter::new(
                    Arc::new(dotfmt_tailwind::OrderTable::empty()),
                    config.clone(),
                )
                .with_project_root(root),
            );
            return Ok(());
        }
        let classes = dotfmt_tailwind::extract::candidates(input);
        match dotfmt_tailwind::bridge::project_order(
            root,
            config.stylesheet.as_deref(),
            config.config_file.as_deref(),
            &classes,
        ) {
            Ok(table) => {
                *sorter = Some(
                    dotfmt_tailwind::Sorter::new(Arc::new(table), config.clone())
                        .with_project_root(root),
                );
                Ok(())
            }
            Err(error) => {
                if matches!(config.order, dotfmt_tailwind::OrderSource::Project) {
                    return Err(Diagnostic::new(
                        DiagnosticKind::Configuration,
                        format!("tailwind order: {error}"),
                    )
                    .with_path(path));
                }
                *sorter = Some(
                    dotfmt_tailwind::Sorter::new(
                        Arc::new(dotfmt_tailwind::OrderTable::empty()),
                        config.clone(),
                    )
                    .with_project_root(root),
                );
                Ok(())
            }
        }
    }

    /// Findings and explicit fixes for one document.
    pub fn analyze(
        &self,
        path: &Path,
        input: &[u8],
        fix: bool,
        unsafe_fixes: bool,
        type_aware: Option<bool>,
        project_analysis: bool,
    ) -> Result<dotfmt_core::edit::Analysis, Diagnostic> {
        match &self.kind {
            Kind::TypeScript(config, dialect) | Kind::Javascript(config, dialect) => {
                let text = utf8(input)?;
                let (text, _) = match text.strip_prefix('\u{feff}') {
                    Some(text) => (text, true),
                    None => (text, false),
                };
                let dialect = dialect.resolve(config.dialect, path);
                dotfmt_typescript::analyze(
                    path,
                    text,
                    config,
                    dialect,
                    &dotfmt_typescript::analyze::Options {
                        fix,
                        unsafe_fixes,
                        type_aware: type_aware.unwrap_or(config.type_aware),
                        project_analysis,
                        type_diagnostics: Vec::new(),
                    },
                )
            }
            Kind::Css(config, dialect) => {
                let dialect = dialect.resolve(config.dialect, path);
                dotfmt_css::format(path, utf8(input)?, config, dialect)?;
                Ok(dotfmt_core::edit::Analysis::default())
            }
            Kind::Html(config, dialect) => {
                let dialect = dialect.resolve(config.dialect, path);
                dotfmt_html::format(path, utf8(input)?, config, dialect)?;
                Ok(dotfmt_core::edit::Analysis::default())
            }
            _ => Ok(dotfmt_core::edit::Analysis::default()),
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
