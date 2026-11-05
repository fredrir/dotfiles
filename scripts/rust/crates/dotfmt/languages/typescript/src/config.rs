use dotfmt_core::config::Settings;
use dotfmt_core::diagnostic::Diagnostic;
use oxc_formatter::{
    ArrowParentheses, AttributePosition, BracketSameLine, BracketSpacing, Expand, JsdocOptions,
    QuoteProperties, QuoteStyle, Semicolons, SortImportsOptions, TrailingCommas,
};

use crate::dialect::Dialect;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IndentType {
    #[default]
    Spaces,
    Tabs,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LineEndings {
    #[default]
    Unix,
    Windows,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FixMode {
    #[default]
    Off,
    Safe,
    Unsafe,
}

/// Fix categories enabled while formatting. `safe` and `unsafe` summarize the
/// selected categories; the category flags decide which fixes are applied.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FixSelection {
    pub safe: bool,
    pub r#unsafe: bool,
    pub remove_unused_imports: bool,
    pub remove_unused_exports: bool,
    pub remove_unused_locals: bool,
    pub remove_unused_parameters: bool,
    pub convert_type_only_imports: bool,
    pub organize_imports: bool,
}

impl FixSelection {
    /// Whether a fix belongs to an enabled category.
    pub fn selects(&self, code: Option<&str>, kind: &str) -> bool {
        if kind.contains("organizeImports") {
            return self.organize_imports;
        }
        match code {
            Some("unused-import") => self.remove_unused_imports,
            Some("type-only-import") => self.convert_type_only_imports,
            Some("unused-variable" | "unused-class-member") => self.remove_unused_locals,
            Some("unused-parameter" | "unused-type-parameter") => self.remove_unused_parameters,
            Some("unused-export") => self.remove_unused_exports,
            _ => false,
        }
    }
}

/// Layout and feature choices for TypeScript, TSX, JavaScript and JSX.
#[derive(Clone, Debug)]
pub struct Config {
    pub indent: usize,
    pub indent_type: IndentType,
    pub width: usize,
    pub final_newline: bool,
    pub line_endings: LineEndings,
    pub quote_style: QuoteStyle,
    pub jsx_quote_style: QuoteStyle,
    pub quote_properties: QuoteProperties,
    pub semicolons: Semicolons,
    pub trailing_commas: TrailingCommas,
    pub arrow_parentheses: ArrowParentheses,
    pub bracket_spacing: bool,
    pub bracket_same_line: bool,
    pub attribute_position: AttributePosition,
    pub expand: Expand,
    pub jsdoc: bool,
    pub embedded_css: bool,
    pub sort_imports: bool,
    pub import_order: SortOrder,
    pub import_internal: Vec<String>,
    pub import_side_effects: bool,
    pub import_newlines: bool,
    pub import_partition_by_newline: bool,
    pub import_partition_by_comment: bool,
    pub import_groups: Option<Vec<Vec<oxc_formatter::GroupEntry>>>,
    pub tailwind: dotfmt_tailwind::Config,
    pub fixes: FixMode,
    pub remove_unused_imports: bool,
    pub remove_unused_exports: bool,
    pub remove_unused_locals: bool,
    pub remove_unused_parameters: bool,
    pub convert_type_only_imports: bool,
    pub organize_imports: bool,
    pub type_aware: bool,
    /// Verify that formatting did not remove code.
    pub verify: bool,
    /// Dialect override from the engine; `Auto` resolves from the path.
    pub dialect: Dialect,
    /// Project order prepared by the LSP/CLI. `None` uses the Rust fallback.
    pub tailwind_sorter: Option<dotfmt_tailwind::Sorter>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortOrder {
    #[default]
    Asc,
    Desc,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            indent: 2,
            indent_type: IndentType::Spaces,
            width: 80,
            final_newline: true,
            line_endings: LineEndings::Unix,
            quote_style: QuoteStyle::Double,
            jsx_quote_style: QuoteStyle::Double,
            quote_properties: QuoteProperties::AsNeeded,
            semicolons: Semicolons::Always,
            trailing_commas: TrailingCommas::All,
            arrow_parentheses: ArrowParentheses::Always,
            bracket_spacing: true,
            bracket_same_line: false,
            attribute_position: AttributePosition::Auto,
            expand: Expand::Auto,
            jsdoc: false,
            embedded_css: true,
            sort_imports: false,
            import_order: SortOrder::Asc,
            import_internal: vec!["~/".to_owned(), "@/".to_owned()],
            import_side_effects: false,
            import_newlines: true,
            import_partition_by_newline: false,
            import_partition_by_comment: false,
            import_groups: None,
            tailwind: dotfmt_tailwind::Config::default(),
            fixes: FixMode::Off,
            remove_unused_imports: false,
            remove_unused_exports: false,
            remove_unused_locals: false,
            remove_unused_parameters: false,
            convert_type_only_imports: false,
            organize_imports: false,
            type_aware: true,
            verify: true,
            dialect: Dialect::Auto,
            tailwind_sorter: None,
        }
    }
}

impl Config {
    pub fn from_settings(settings: &Settings) -> Result<Self, Diagnostic> {
        let mut config = Self::default();
        for (key, setting) in settings {
            config
                .set(key, &setting.value)
                .map_err(|error| setting.error(error))?;
        }
        Ok(config)
    }

    fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "indent" => self.indent = number(key, value, 0, 24)?,
            "indent_type" => {
                self.indent_type = match value {
                    "spaces" => IndentType::Spaces,
                    "tabs" => IndentType::Tabs,
                    other => {
                        return Err(format!("indent_type must be spaces or tabs, not {other}"));
                    }
                }
            }
            "width" => self.width = number(key, value, 1, 320)?,
            "final_newline" => self.final_newline = flag(key, value)?,
            "line_endings" => {
                self.line_endings = match value {
                    "unix" | "lf" => LineEndings::Unix,
                    "windows" | "crlf" => LineEndings::Windows,
                    other => {
                        return Err(format!("line_endings must be unix or windows, not {other}"));
                    }
                }
            }
            "quote_style" => {
                self.quote_style = match value {
                    "double" | "auto" | "auto-prefer-double" => QuoteStyle::Double,
                    "single" | "auto-prefer-single" => QuoteStyle::Single,
                    other => {
                        return Err(format!(
                            "quote_style must be double, single, auto, auto-prefer-double or auto-prefer-single, not {other}"
                        ));
                    }
                }
            }
            "jsx_quote_style" => {
                self.jsx_quote_style = match value {
                    "double" => QuoteStyle::Double,
                    "single" => QuoteStyle::Single,
                    other => {
                        return Err(format!(
                            "jsx_quote_style must be double or single, not {other}"
                        ));
                    }
                }
            }
            "quote_properties" => {
                self.quote_properties = match value {
                    "as-needed" | "as_needed" => QuoteProperties::AsNeeded,
                    "preserve" => QuoteProperties::Preserve,
                    "consistent" => QuoteProperties::Consistent,
                    other => {
                        return Err(format!(
                            "quote_properties must be as-needed, preserve or consistent, not {other}"
                        ));
                    }
                }
            }
            "semicolons" => {
                self.semicolons = match value {
                    "always" => Semicolons::Always,
                    "as-needed" | "as_needed" => Semicolons::AsNeeded,
                    other => {
                        return Err(format!(
                            "semicolons must be always or as-needed, not {other}"
                        ));
                    }
                }
            }
            "trailing_commas" => {
                self.trailing_commas = match value {
                    "all" => TrailingCommas::All,
                    "es5" => TrailingCommas::Es5,
                    "none" => TrailingCommas::None,
                    other => {
                        return Err(format!(
                            "trailing_commas must be all, es5 or none, not {other}"
                        ));
                    }
                }
            }
            "arrow_parentheses" => {
                self.arrow_parentheses = match value {
                    "always" => ArrowParentheses::Always,
                    "as-needed" | "as_needed" => ArrowParentheses::AsNeeded,
                    other => {
                        return Err(format!(
                            "arrow_parentheses must be always or as-needed, not {other}"
                        ));
                    }
                }
            }
            "bracket_spacing" => self.bracket_spacing = flag(key, value)?,
            "bracket_same_line" => self.bracket_same_line = flag(key, value)?,
            "attribute_position" => {
                self.attribute_position = match value {
                    "auto" => AttributePosition::Auto,
                    "multiline" => AttributePosition::Multiline,
                    other => {
                        return Err(format!(
                            "attribute_position must be auto or multiline, not {other}"
                        ));
                    }
                }
            }
            "expand" | "object_wrap" => {
                self.expand = match value {
                    "auto" | "preserve" => Expand::Auto,
                    "never" | "collapse" => Expand::Never,
                    other => {
                        return Err(format!(
                            "{key} must be auto/preserve or never/collapse, not {other}"
                        ));
                    }
                }
            }
            "jsdoc" => self.jsdoc = flag(key, value)?,
            "embedded_css" => self.embedded_css = flag(key, value)?,
            "sort_imports" => self.sort_imports = flag(key, value)?,
            "import_order" => {
                self.import_order = match value {
                    "asc" | "ascending" => SortOrder::Asc,
                    "desc" | "descending" => SortOrder::Desc,
                    other => return Err(format!("import_order must be asc or desc, not {other}")),
                }
            }
            "import_internal" => self.import_internal = dotfmt_tailwind::Config::list(value),
            "import_side_effects" => self.import_side_effects = flag(key, value)?,
            "import_newlines" => self.import_newlines = flag(key, value)?,
            "import_partition_by_newline" => self.import_partition_by_newline = flag(key, value)?,
            "import_partition_by_comment" => self.import_partition_by_comment = flag(key, value)?,
            "import_groups" => {
                if value.trim().is_empty()
                    || matches!(value, "default" | "perfectionist" | "biome" | "none")
                {
                    self.import_groups = None;
                } else {
                    let groups = value
                        .split('|')
                        .map(|group| {
                            group
                                .split('+')
                                .map(|entry| oxc_formatter::GroupEntry::parse(entry.trim()))
                                .collect::<Vec<_>>()
                        })
                        .collect::<Vec<_>>();
                    let probe = SortImportsOptions {
                        groups: groups.clone(),
                        ..SortImportsOptions::default()
                    };
                    probe.validate()?;
                    self.import_groups = Some(groups);
                }
            }
            "tailwind" => {
                self.tailwind.mode = match value {
                    "off" | "false" => dotfmt_tailwind::Mode::Off,
                    "on" | "true" => dotfmt_tailwind::Mode::On,
                    "auto" => dotfmt_tailwind::Mode::Auto,
                    other => return Err(format!("tailwind must be off, on or auto, not {other}")),
                }
            }
            "tailwind_order" => {
                self.tailwind.order = match value {
                    "auto" => dotfmt_tailwind::OrderSource::Auto,
                    "project" => dotfmt_tailwind::OrderSource::Project,
                    "rust" => dotfmt_tailwind::OrderSource::Rust,
                    other => {
                        return Err(format!(
                            "tailwind_order must be auto, project or rust, not {other}"
                        ));
                    }
                }
            }
            "tailwind_functions" => self.tailwind.functions = dotfmt_tailwind::Config::list(value),
            "tailwind_attributes" => {
                self.tailwind.attributes = dotfmt_tailwind::Config::list(value)
            }
            "tailwind_dedupe" => self.tailwind.dedupe = flag(key, value)?,
            "tailwind_merge" => self.tailwind.merge = flag(key, value)?,
            "tailwind_stylesheet" => self.tailwind.stylesheet = Some(value.into()),
            "tailwind_config" => self.tailwind.config_file = Some(value.into()),
            "fixes" => {
                self.fixes = match value {
                    "off" | "none" => FixMode::Off,
                    "safe" => FixMode::Safe,
                    "unsafe" | "all" => FixMode::Unsafe,
                    other => return Err(format!("fixes must be off, safe or unsafe, not {other}")),
                }
            }
            "remove_unused_imports" => self.remove_unused_imports = flag(key, value)?,
            "remove_unused_exports" => self.remove_unused_exports = flag(key, value)?,
            "remove_unused_locals" => self.remove_unused_locals = flag(key, value)?,
            "remove_unused_parameters" => self.remove_unused_parameters = flag(key, value)?,
            "convert_type_only_imports" => self.convert_type_only_imports = flag(key, value)?,
            "organize_imports" => self.organize_imports = flag(key, value)?,
            "type_aware" => self.type_aware = flag(key, value)?,
            "verify" => self.verify = flag(key, value)?,
            "dialect" => self.dialect = Dialect::parse(value, true)?,
            other => return Err(format!("unknown setting: {other}")),
        }
        Ok(())
    }

    pub fn js_options(&self) -> oxc_formatter::JsFormatOptions {
        oxc_formatter::JsFormatOptions {
            indent_style: match self.indent_type {
                IndentType::Spaces => oxc_formatter_core::IndentStyle::Space,
                IndentType::Tabs => oxc_formatter_core::IndentStyle::Tab,
            },
            indent_width: oxc_formatter_core::IndentWidth::try_from(self.indent as u8)
                .unwrap_or_default(),
            line_ending: oxc_formatter_core::LineEnding::Lf,
            line_width: oxc_formatter_core::LineWidth::try_from(self.width as u16)
                .unwrap_or_default(),
            quote_style: self.quote_style,
            jsx_quote_style: self.jsx_quote_style,
            quote_properties: self.quote_properties,
            trailing_commas: self.trailing_commas,
            semicolons: self.semicolons,
            arrow_parentheses: self.arrow_parentheses,
            bracket_spacing: BracketSpacing::from(self.bracket_spacing),
            bracket_same_line: BracketSameLine::from(self.bracket_same_line),
            attribute_position: self.attribute_position,
            expand: self.expand,
            sort_imports: self.sort_imports.then(|| self.sort_imports_options()),
            sort_tailwindcss: self.tailwind_enabled().then(|| {
                oxc_formatter::SortTailwindcssOptions {
                    functions: self.tailwind.functions.clone(),
                    attributes: self.tailwind.attributes.clone(),
                    preserve_whitespace: false,
                }
            }),
            jsdoc: self.jsdoc.then(JsdocOptions::default),
            ..Default::default()
        }
    }

    fn sort_imports_options(&self) -> SortImportsOptions {
        let mut options = SortImportsOptions {
            partition_by_newline: self.import_partition_by_newline,
            partition_by_comment: self.import_partition_by_comment,
            sort_side_effects: self.import_side_effects,
            order: match self.import_order {
                SortOrder::Asc => oxc_formatter::SortOrder::Asc,
                SortOrder::Desc => oxc_formatter::SortOrder::Desc,
            },
            newlines_between: self.import_newlines,
            internal_pattern: self.import_internal.clone(),
            ..SortImportsOptions::default()
        };
        if let Some(groups) = &self.import_groups {
            options.groups = groups.clone();
        }
        options
    }

    /// Import options with project path aliases added as internal prefixes.
    pub fn js_options_for_path(&self, path: &std::path::Path) -> oxc_formatter::JsFormatOptions {
        let mut options = self.js_options();
        if let Some(sort) = &mut options.sort_imports
            && let Some(patterns) = crate::tsconfig::import_patterns(path)
        {
            for pattern in patterns {
                if !sort.internal_pattern.contains(&pattern) {
                    sort.internal_pattern.push(pattern);
                }
            }
        }
        options
    }

    pub fn tailwind_enabled(&self) -> bool {
        !matches!(self.tailwind.mode, dotfmt_tailwind::Mode::Off)
    }

    /// Fixes to apply before formatting a TypeScript or JavaScript document.
    ///
    /// `fixes = off` selects nothing on its own; `safe` enables imports and
    /// type-only conversion; `unsafe` adds unused locals. Export removal is
    /// never implied by a mode and must be requested explicitly. Parameter
    /// removal is diagnose-only: there is no safe parameter-removal fix.
    pub fn format_fix_selection(&self) -> Option<FixSelection> {
        let mut selection = FixSelection::default();
        match self.fixes {
            FixMode::Off => {}
            FixMode::Safe => {
                selection.remove_unused_imports = true;
                selection.convert_type_only_imports = true;
                selection.organize_imports = true;
            }
            FixMode::Unsafe => {
                selection.remove_unused_imports = true;
                selection.convert_type_only_imports = true;
                selection.organize_imports = true;
                // Export removal changes public API and is never implied by a
                // mode; it must be requested explicitly with
                // `remove_unused_exports = true`.
                selection.remove_unused_locals = true;
            }
        }
        selection.remove_unused_imports |= self.remove_unused_imports;
        selection.remove_unused_exports |= self.remove_unused_exports;
        selection.remove_unused_locals |= self.remove_unused_locals;
        selection.convert_type_only_imports |= self.convert_type_only_imports;
        selection.organize_imports |= self.organize_imports;
        selection.safe = selection.remove_unused_imports
            || selection.convert_type_only_imports
            || selection.organize_imports;
        selection.r#unsafe = selection.remove_unused_exports || selection.remove_unused_locals;
        (selection.safe || selection.r#unsafe).then_some(selection)
    }
}

fn flag(key: &str, value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("{key} must be true or false, not {other}")),
    }
}

fn number(key: &str, value: &str, minimum: usize, maximum: usize) -> Result<usize, String> {
    let number: usize = value
        .parse()
        .map_err(|_| format!("{key} must be a number, not {value}"))?;
    if (minimum..=maximum).contains(&number) {
        Ok(number)
    } else {
        Err(format!("{key} must be between {minimum} and {maximum}"))
    }
}
