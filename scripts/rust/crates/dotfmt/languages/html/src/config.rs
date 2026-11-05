use dotfmt_core::config::Settings;
use dotfmt_core::diagnostic::Diagnostic;
use markup_fmt::config::{ClosingTagLineBreakForEmpty, FormatOptions, LineBreak, Quotes};

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
    pub quotes: Quotes,
    pub format_comments: bool,
    pub closing_bracket_same_line: bool,
    pub closing_tag_line_break_for_empty: ClosingTagLineBreakForEmpty,
    pub max_attrs_per_line: Option<usize>,
    pub prefer_attrs_single_line: bool,
    pub single_attr_same_line: bool,
    pub script_indent: bool,
    pub style_indent: bool,
    pub typescript: dotfmt_typescript::Config,
    pub css: dotfmt_css::Config,
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
            quotes: Quotes::Double,
            format_comments: false,
            closing_bracket_same_line: false,
            closing_tag_line_break_for_empty: ClosingTagLineBreakForEmpty::Fit,
            max_attrs_per_line: None,
            prefer_attrs_single_line: false,
            single_attr_same_line: true,
            script_indent: true,
            style_indent: true,
            typescript: dotfmt_typescript::Config::default(),
            css: dotfmt_css::Config::default(),
            tailwind: dotfmt_tailwind::Config {
                attributes: vec!["class".to_owned(), "className".to_owned()],
                ..dotfmt_tailwind::Config::default()
            },
            tailwind_sorter: None,
        }
    }
}

impl Config {
    pub fn from_settings(settings: &Settings) -> Result<Self, Diagnostic> {
        let mut config = Self::default();
        // Embedded language settings are inherited from the global TypeScript
        // and CSS blocks, so `html {}` only needs HTML-specific overrides.
        for (key, setting) in settings {
            config
                .set(key, &setting.value, setting.global)
                .map_err(|error| setting.error(error))?;
        }
        Ok(config)
    }

    fn set(&mut self, key: &str, value: &str, global: bool) -> Result<(), String> {
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
            "quotes" | "quote_style" => {
                self.quotes = match value {
                    "double" | "auto" | "auto-prefer-double" => Quotes::Double,
                    "single" | "auto-prefer-single" => Quotes::Single,
                    other => {
                        return Err(format!(
                            "quotes must be double, single, auto, auto-prefer-double or auto-prefer-single, not {other}"
                        ));
                    }
                }
            }
            "format_comments" => self.format_comments = flag(key, value)?,
            "closing_bracket_same_line" => self.closing_bracket_same_line = flag(key, value)?,
            "closing_tag_line_break_for_empty" => {
                self.closing_tag_line_break_for_empty = match value {
                    "always" => ClosingTagLineBreakForEmpty::Always,
                    "fit" => ClosingTagLineBreakForEmpty::Fit,
                    "never" => ClosingTagLineBreakForEmpty::Never,
                    other => {
                        return Err(format!(
                            "closing_tag_line_break_for_empty must be always, fit or never, not {other}"
                        ));
                    }
                }
            }
            "max_attrs_per_line" => {
                self.max_attrs_per_line = if value == "none" {
                    None
                } else {
                    Some(number(key, value, 1, 1000)?)
                }
            }
            "prefer_attrs_single_line" => self.prefer_attrs_single_line = flag(key, value)?,
            "single_attr_same_line" => self.single_attr_same_line = flag(key, value)?,
            "script_indent" => self.script_indent = flag(key, value)?,
            "style_indent" => self.style_indent = flag(key, value)?,
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
            "tailwind_attributes" => {
                self.tailwind.attributes = dotfmt_tailwind::Config::list(value)
            }
            "tailwind_dedupe" => self.tailwind.dedupe = flag(key, value)?,
            "tailwind_merge" => self.tailwind.merge = flag(key, value)?,
            "tailwind_stylesheet" => self.tailwind.stylesheet = Some(value.into()),
            "tailwind_config" => self.tailwind.config_file = Some(value.into()),
            "dialect" => self.dialect = crate::dialect::Dialect::parse(value, true)?,
            other => return Err(format!("unknown setting: {other}")),
        }
        Ok(())
    }

    pub fn markup_options(&self) -> FormatOptions {
        let mut options = FormatOptions::default();
        options.layout.print_width = self.width;
        options.layout.use_tabs = self.indent_type == IndentType::Tabs;
        options.layout.indent_width = self.indent;
        options.layout.line_break = LineBreak::Lf;
        options.language.quotes = self.quotes;
        options.language.format_comments = self.format_comments;
        options.language.closing_bracket_same_line = self.closing_bracket_same_line;
        options.language.closing_tag_line_break_for_empty = self.closing_tag_line_break_for_empty;
        options.language.max_attrs_per_line = self
            .max_attrs_per_line
            .and_then(std::num::NonZeroUsize::new);
        options.language.prefer_attrs_single_line = self.prefer_attrs_single_line;
        options.language.single_attr_same_line = self.single_attr_same_line;
        options.language.script_indent = self.script_indent;
        options.language.style_indent = self.style_indent;
        options.language.ignore_comment_directive = "prettier-ignore".to_owned();
        options.language.ignore_file_comment_directive = "prettier-ignore-file".to_owned();
        options
    }

    pub fn tailwind_enabled(&self) -> bool {
        !matches!(self.tailwind.mode, dotfmt_tailwind::Mode::Off)
    }
}

fn flag(key: &str, value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("{key} must be true or false, not {other}")),
    }
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
