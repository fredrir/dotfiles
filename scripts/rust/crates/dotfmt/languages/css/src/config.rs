use dotfmt_core::config::Settings;
use dotfmt_core::diagnostic::Diagnostic;
use malva::config::{
    BlockSelectorLineBreak, DeclarationOrder, FormatOptions, HexCase, KeyframeSelectorNotation,
    OperatorLineBreak, Quotes,
};

pub(crate) const IGNORE_COMMENT_DIRECTIVE: &str = "prettier-ignore";
pub(crate) const IGNORE_FILE_COMMENT_DIRECTIVE: &str = "prettier-ignore-file";

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

#[derive(Clone, Debug)]
pub struct Config {
    pub dialect: crate::dialect::Dialect,
    pub indent: usize,
    pub indent_type: IndentType,
    pub width: usize,
    pub final_newline: bool,
    pub line_endings: LineEndings,
    pub hex_case: HexCase,
    pub quotes: Quotes,
    pub format_comments: bool,
    pub align_comments: bool,
    pub single_line_block_threshold: Option<usize>,
    pub operator_linebreak: OperatorLineBreak,
    pub block_selector_linebreak: BlockSelectorLineBreak,
    pub omit_number_leading_zero: bool,
    pub keyframe_selector_notation: Option<KeyframeSelectorNotation>,
    pub prefer_single_line: bool,
    /// Declaration sorting is opt-in; `None` preserves source order.
    pub declaration_order: Option<DeclarationOrder>,
    pub tailwind: dotfmt_tailwind::Config,
    pub tailwind_sorter: Option<dotfmt_tailwind::Sorter>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            dialect: crate::dialect::Dialect::Auto,
            indent: 2,
            indent_type: IndentType::Spaces,
            width: 80,
            final_newline: true,
            line_endings: LineEndings::Unix,
            hex_case: HexCase::Lower,
            quotes: Quotes::AlwaysDouble,
            format_comments: false,
            align_comments: true,
            single_line_block_threshold: None,
            operator_linebreak: OperatorLineBreak::After,
            block_selector_linebreak: BlockSelectorLineBreak::Consistent,
            omit_number_leading_zero: false,
            keyframe_selector_notation: None,
            prefer_single_line: false,
            declaration_order: None,
            tailwind: dotfmt_tailwind::Config::default(),
            tailwind_sorter: None,
        }
    }
}

impl Config {
    pub fn from_settings(settings: &Settings) -> Result<Self, Diagnostic> {
        let mut config = Self::default();
        for (key, setting) in settings {
            config
                .set(key, &setting.value, setting.global, &setting.source)
                .map_err(|error| setting.error(error))?;
        }
        Ok(config)
    }

    fn set(
        &mut self,
        key: &str,
        value: &str,
        global: bool,
        source: &std::path::Path,
    ) -> Result<(), String> {
        match key {
            "indent" if global => {
                if let Some(indent) = clamped(value, 0, 24) {
                    self.indent = indent;
                }
            }
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
            "width" if global => {
                if let Some(width) = clamped(value, 1, 320) {
                    self.width = width;
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
            "hex_case" => {
                self.hex_case = match value {
                    "ignore" => HexCase::Ignore,
                    "lower" => HexCase::Lower,
                    "upper" => HexCase::Upper,
                    other => {
                        return Err(format!(
                            "hex_case must be ignore, lower or upper, not {other}"
                        ));
                    }
                }
            }
            "quotes" | "quote_style" => {
                self.quotes = match value {
                    "double" | "always-double" => Quotes::AlwaysDouble,
                    "single" | "always-single" => Quotes::AlwaysSingle,
                    "prefer-double" | "preserve" | "auto" | "auto-prefer-double" => {
                        Quotes::PreferDouble
                    }
                    "prefer-single" | "auto-prefer-single" => Quotes::PreferSingle,
                    other => {
                        return Err(format!(
                            "quotes must be double, single, prefer-double, prefer-single, preserve, auto, auto-prefer-double or auto-prefer-single, not {other}"
                        ));
                    }
                }
            }
            "format_comments" => self.format_comments = flag(key, value)?,
            "align_comments" => self.align_comments = flag(key, value)?,
            "single_line_block_threshold" => {
                self.single_line_block_threshold = if value == "none" {
                    None
                } else {
                    Some(number(key, value, 1, 10_000)?)
                }
            }
            "operator_linebreak" => {
                self.operator_linebreak = choice(
                    key,
                    value,
                    &[
                        ("before", OperatorLineBreak::Before),
                        ("after", OperatorLineBreak::After),
                    ],
                )?
            }
            "block_selector_linebreak" => {
                self.block_selector_linebreak = choice(
                    key,
                    value,
                    &[
                        ("always", BlockSelectorLineBreak::Always),
                        ("consistent", BlockSelectorLineBreak::Consistent),
                        ("wrap", BlockSelectorLineBreak::Wrap),
                    ],
                )?
            }
            "omit_number_leading_zero" => self.omit_number_leading_zero = flag(key, value)?,
            "keyframe_selector_notation" => {
                self.keyframe_selector_notation = match value {
                    "none" => None,
                    "keyword" => Some(KeyframeSelectorNotation::Keyword),
                    "percentage" => Some(KeyframeSelectorNotation::Percentage),
                    other => {
                        return Err(format!(
                            "keyframe_selector_notation must be none, keyword or percentage, not {other}"
                        ));
                    }
                }
            }
            "prefer_single_line" => self.prefer_single_line = flag(key, value)?,
            "declaration_order" => {
                self.declaration_order = match value {
                    "none" => None,
                    "alphabetical" => Some(DeclarationOrder::Alphabetical),
                    other => {
                        return Err(format!(
                            "declaration_order must be none or alphabetical, not {other}"
                        ));
                    }
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
            "tailwind_dedupe" => self.tailwind.dedupe = flag(key, value)?,
            "tailwind_merge" => self.tailwind.merge = flag(key, value)?,
            "tailwind_stylesheet" => {
                self.tailwind.stylesheet = Some(value.into());
                self.tailwind.stylesheet_base = setting_base(global, source);
            }
            "tailwind_config" => {
                self.tailwind.config_file = Some(value.into());
                self.tailwind.config_base = setting_base(global, source);
            }
            "dialect" => self.dialect = crate::dialect::Dialect::parse(value, true)?,
            other => return Err(format!("unknown setting: {other}")),
        }
        Ok(())
    }

    pub fn malva_options(&self) -> FormatOptions {
        let mut options = FormatOptions::default();
        options.layout.print_width = self.width;
        options.layout.use_tabs = self.indent_type == IndentType::Tabs;
        options.layout.indent_width = self.indent;
        options.layout.line_break = malva::config::LineBreak::Lf;
        options.language.hex_case = self.hex_case.clone();
        options.language.quotes = self.quotes;
        options.language.format_comments = self.format_comments;
        options.language.align_comments = self.align_comments;
        options.language.single_line_block_threshold = self.single_line_block_threshold;
        options.language.operator_linebreak = self.operator_linebreak.clone();
        options.language.block_selector_linebreak = self.block_selector_linebreak.clone();
        options.language.omit_number_leading_zero = self.omit_number_leading_zero;
        options.language.keyframe_selector_notation = self.keyframe_selector_notation.clone();
        options.language.prefer_single_line = self.prefer_single_line;
        options.language.declaration_order = self.declaration_order.clone();
        options.language.ignore_comment_directive = IGNORE_COMMENT_DIRECTIVE.to_owned();
        options.language.ignore_file_comment_directive = IGNORE_FILE_COMMENT_DIRECTIVE.to_owned();
        options
    }

    pub fn tailwind_enabled(&self) -> bool {
        !matches!(self.tailwind.mode, dotfmt_tailwind::Mode::Off)
    }
}

/// A local setting is relative to its declaring config file; globals are
/// relative to the workspace root and therefore have no base here.
fn setting_base(global: bool, source: &std::path::Path) -> Option<std::path::PathBuf> {
    (!global).then(|| {
        source
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf()
    })
}

fn flag(key: &str, value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("{key} must be true or false, not {other}")),
    }
}

fn choice<T: Clone>(key: &str, value: &str, choices: &[(&str, T)]) -> Result<T, String> {
    choices
        .iter()
        .find(|(name, _)| *name == value)
        .map(|(_, choice)| choice.clone())
        .ok_or_else(|| {
            format!(
                "{key} must be {}, not {value}",
                choices
                    .iter()
                    .map(|(name, _)| *name)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

/// Clamps a shared-global number into what this engine can represent; invalid
/// native values are ignored like other unsupported globals.
fn clamped(value: &str, minimum: usize, maximum: usize) -> Option<usize> {
    value
        .parse::<usize>()
        .ok()
        .map(|number| number.clamp(minimum, maximum))
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
