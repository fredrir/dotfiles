use std::path::Path;

use super::{Repairs, configuration};
use dotfmt_core::config::{Effective, Settings};
use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};
use dotfmt_core::edit::{FixMode, apply_fixes, select_fixes};
use dotfmt_core::language::Language;
use dotfmt_core::repair::Repaired;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

const REPAIR: &str = "repair";

fn export_fixes_trusted() -> bool {
    dotfmt_tailwind::bridge::trusted()
        && std::env::var_os("DOTFMT_TRUSTED").is_none_or(|value| value != "0" && value != "false")
}

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
        let Some(mut selection) = selection else {
            return self.format_text(path, context, input, detected, clean);
        };
        if !export_fixes_trusted() {
            selection.remove_unused_exports = false;
            selection.remove_unused_locals = false;
            selection.remove_unused_parameters = false;
            selection.r#unsafe = false;
        }
        self.format_with_fixes(path, context, input, detected, clean, selection)
    }

    pub fn fix_selection(&self, mode: FixMode) -> Option<dotfmt_typescript::config::FixSelection> {
        match &self.kind {
            Kind::TypeScript(config, _) | Kind::Javascript(config, _) if mode != FixMode::None => {
                config.requested_fix_selection(mode)
            }
            _ => None,
        }
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
                context,
                context,
                current.as_bytes(),
                detected,
                true,
                selection.r#unsafe,
                Some(false),
                selection.organize_imports,
                selection.remove_unused_exports && export_fixes_trusted(),
            ) {
                Ok(analysis) => analysis,
                Err(_) => return self.format_text(path, context, input, detected, clean),
            };
            let mode = if selection.r#unsafe {
                FixMode::Unsafe
            } else {
                FixMode::Safe
            };
            let selected = select_fixes(mode, &selection, &analysis.fixes);
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
        let explicit_path = config.stylesheet.is_some() || config.config_file.is_some();
        if matches!(config.mode, dotfmt_tailwind::Mode::Auto)
            && !source_uses_tailwind
            && !explicit_path
            && !dotfmt_tailwind::bridge::is_tailwind_project(root)
        {
            return Ok(());
        }
        let trusted = dotfmt_tailwind::bridge::trusted()
            && std::env::var_os("DOTFMT_TRUSTED")
                .is_none_or(|value| value != "0" && value != "false");
        if !trusted {
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
        match dotfmt_tailwind::bridge::project_order_for(
            root,
            Some(path),
            config.stylesheet.as_deref(),
            config.stylesheet_base.as_deref(),
            config.config_file.as_deref(),
            config.config_base.as_deref(),
            config.order,
            &classes,
        ) {
            Ok(order) => {
                for warning in &order.discovery.warnings {
                    eprintln!("dotfmt: tailwind: {warning} for {}", path.display());
                }
                let mut resolved = config.clone();
                resolved.stylesheet = order.discovery.stylesheet.clone();
                resolved.stylesheet_base = None;
                resolved.config_file = order.discovery.config.clone();
                resolved.config_base = None;
                *sorter = Some(
                    dotfmt_tailwind::Sorter::new(Arc::new(order.table), resolved)
                        .with_project_root(order.discovery.package_root),
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
                eprintln!(
                    "dotfmt: tailwind: {error}; using the Rust order for {}",
                    path.display()
                );
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

    /// `path` is the user-facing name, `context` the absolute path used for resolution.
    #[allow(clippy::too_many_arguments)]
    pub fn analyze(
        &self,
        path: &Path,
        context: &Path,
        input: &[u8],
        detected: dotfmt_markdown::dialect::Dialect,
        fix: bool,
        unsafe_fixes: bool,
        type_aware: Option<bool>,
        organize: bool,
        project_analysis: bool,
    ) -> Result<dotfmt_core::edit::Analysis, Diagnostic> {
        match &self.kind {
            Kind::TypeScript(config, dialect) | Kind::Javascript(config, dialect) => {
                let text = utf8(input)?;
                let (text, _) = match text.strip_prefix('\u{feff}') {
                    Some(text) => (text, true),
                    None => (text, false),
                };
                let dialect = dialect.resolve(config.dialect, context);
                dotfmt_typescript::analyze(
                    context,
                    text,
                    config,
                    dialect,
                    &dotfmt_typescript::analyze::Options {
                        fix,
                        unsafe_fixes,
                        type_aware: type_aware.unwrap_or(config.type_aware),
                        organize,
                        project_analysis,
                        type_diagnostics: Vec::new(),
                    },
                )
            }
            Kind::Css(config, dialect) => {
                let dialect = dialect.resolve(config.dialect, context);
                dotfmt_css::format(context, utf8(input)?, config, dialect)?;
                Ok(dotfmt_core::edit::Analysis::default())
            }
            Kind::Html(config, dialect) => {
                let dialect = dialect.resolve(config.dialect, context);
                dotfmt_html::format(context, utf8(input)?, config, dialect)?;
                Ok(dotfmt_core::edit::Analysis::default())
            }
            _ => {
                // These languages contribute no findings, but a diagnose run still has to
                // report malformed input. The format path is the validator (and applies the
                // same repair semantics); only its diagnostic matters here. Fix runs format
                // the buffer again afterwards, so they must not pay for a second pass.
                if !fix {
                    self.format_text(path, context, input, detected, false)?;
                }
                Ok(dotfmt_core::edit::Analysis::default())
            }
        }
    }

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

/// Embedded `<style>`/`<script>` blocks follow the effective CSS, TypeScript and
/// JavaScript settings instead of the HTML engine defaults. Blocks stay formatted even
/// when the sub-language is disabled for standalone files; only the `javascript` block
/// (an opt-in language) is optional, and `<script>` then uses the TypeScript settings.
fn embedded_html(html: &mut dotfmt_html::Config, config: &Effective) -> Result<(), Diagnostic> {
    if let Some(css) = configured(
        config,
        Language::Css,
        dotfmt_css::config::Config::from_settings,
    )? {
        html.css = css;
    }
    let typescript = configured(
        config,
        Language::TypeScript,
        dotfmt_typescript::config::Config::from_settings,
    )?;
    if let Some(typescript) = &typescript {
        html.typescript = typescript.clone();
    }
    html.javascript = match configured(
        config,
        Language::Javascript,
        dotfmt_typescript::config::Config::from_settings,
    )? {
        Some(javascript) => javascript,
        None => html.typescript.clone(),
    };
    Ok(())
}

fn configured<T>(
    config: &Effective,
    language: Language,
    parse: impl Fn(&Settings) -> Result<T, Diagnostic>,
) -> Result<Option<T>, Diagnostic> {
    match config.languages.get(&language) {
        Some(local) if local.enabled => parse(&local.settings).map(Some),
        _ => Ok(None),
    }
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
                            let mut engine = Engine::new(language, &settings.settings, None)?;
                            if let Kind::Html(html, _) = &mut engine.kind {
                                embedded_html(html, config)?;
                            }
                            self.configured.insert((id, language), Arc::new(engine));
                        }
                    }
                    Ok(())
                })();
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

#[cfg(test)]
mod tests {
    use super::*;
    use dotfmt_core::config::Setting;

    fn settings(root: &Path, order: &str, stylesheet: &str, source: &Path) -> Settings {
        let mut settings = Settings::new();
        for (key, value) in [
            ("tailwind", "on"),
            ("tailwind_order", order),
            ("tailwind_stylesheet", stylesheet),
        ] {
            settings.insert(
                key.to_owned(),
                Setting {
                    value: value.to_owned(),
                    source: source.to_path_buf(),
                    line: 1,
                    global: false,
                },
            );
        }
        settings.insert(
            "type_aware".to_owned(),
            Setting {
                value: "false".to_owned(),
                source: root.join("dotfmt.dotfile"),
                line: 1,
                global: false,
            },
        );
        settings
    }

    fn fixture() -> tempfile::TempDir {
        let directory = tempfile::tempdir().expect("fixture");
        let package = directory.path().join("packages/ui");
        std::fs::create_dir_all(package.join("src")).expect("src");
        std::fs::write(package.join("package.json"), "{\"name\":\"ui\"}\n").expect("package.json");
        directory
    }

    #[test]
    fn project_order_reports_a_missing_config_relative_stylesheet() {
        let directory = fixture();
        let root = directory.path();
        let source = root.join("packages/ui/dotfmt.dotfile");
        let path = root.join("packages/ui/src/a.tsx");
        let config = settings(root, "project", "styles/missing.css", &source);
        let engine = Engine::new(Language::TypeScript, &config, None).expect("engine");
        let error = match engine.format(
            &path,
            &path,
            b"const value = 'p-2 p-4';\n",
            dotfmt_markdown::dialect::Dialect::Gfm,
            false,
        ) {
            Ok(_) => panic!("project order must fail"),
            Err(error) => error,
        };
        assert!(error.message.contains("missing.css"), "{error}");
        assert!(
            error.message.contains("packages/ui/styles/missing.css"),
            "{error}"
        );
    }

    #[test]
    fn auto_order_logs_and_falls_back_for_a_missing_explicit_stylesheet() {
        let directory = fixture();
        let root = directory.path();
        let source = root.join("packages/ui/dotfmt.dotfile");
        let path = root.join("packages/ui/src/a.tsx");
        let config = settings(root, "auto", "styles/missing.css", &source);
        let engine = Engine::new(Language::TypeScript, &config, None).expect("engine");
        let formatted = engine
            .format(
                &path,
                &path,
                b"const value = 'p-2 p-4';\n",
                dotfmt_markdown::dialect::Dialect::Gfm,
                false,
            )
            .expect("auto order falls back to Rust");
        let Formatted::Text { text, .. } = formatted else {
            panic!("expected text");
        };
        assert!(text.contains("p-2 p-4"), "{text}");
    }
}
