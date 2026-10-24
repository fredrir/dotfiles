use dotfmt_core::diagnostic::Diagnostic;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEndings {
    Unix,
    Windows,
}

/// Layout choices for [`format`][crate::format].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Spaces per indentation level.
    pub indent: usize,
    /// Target line width; arrays and inline tables expand beyond it.
    pub width: usize,
    pub final_newline: bool,
    pub line_endings: LineEndings,
    /// Maximum consecutive blank lines kept between elements.
    pub allowed_blank_lines: usize,
    /// Align `=` within contiguous groups of entries.
    pub align_entries: bool,
    /// Align trailing comments within contiguous groups.
    pub align_comments: bool,
    /// Also align a lone trailing comment in a group.
    pub align_single_comments: bool,
    /// Write a trailing comma on multi-line arrays.
    pub array_trailing_comma: bool,
    /// Expand arrays that do not fit the configured width.
    pub array_auto_expand: bool,
    /// Collapse arrays that fit on one line.
    pub array_auto_collapse: bool,
    /// Omit padding inside single-line arrays.
    pub compact_arrays: bool,
    /// Omit padding inside inline tables.
    pub compact_inline_tables: bool,
    /// Omit spaces around `=`.
    pub compact_entries: bool,
    /// Expand values nested in inline tables that do not fit the width.
    pub inline_table_expand: bool,
    /// Indent nested table headers and their contents by depth.
    pub indent_tables: bool,
    /// Indent entries under a table header by one level.
    pub indent_entries: bool,
    /// Sort direct entries within a contiguous group by key.
    pub reorder_keys: bool,
    /// Add a space after `#` where one is missing.
    pub format_comments: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            indent: 2,
            width: 80,
            final_newline: true,
            line_endings: LineEndings::Unix,
            allowed_blank_lines: 1,
            align_entries: false,
            align_comments: true,
            align_single_comments: true,
            array_trailing_comma: true,
            array_auto_expand: true,
            array_auto_collapse: true,
            compact_arrays: true,
            compact_inline_tables: false,
            compact_entries: false,
            inline_table_expand: true,
            indent_tables: false,
            indent_entries: false,
            reorder_keys: false,
            format_comments: false,
        }
    }
}

impl Config {
    pub fn from_settings(settings: &dotfmt_core::config::Settings) -> Result<Self, Diagnostic> {
        let mut config = Self::default();
        for (key, setting) in settings {
            // Through the shared block this reaches every language; TOML keeps
            // string literals verbatim and has nothing to apply it to.
            if setting.global && key == "quote_style" {
                continue;
            }
            config
                .set(key, &setting.value)
                .map_err(|error| setting.error(error))?;
        }
        Ok(config)
    }

    fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "indent" => self.indent = number(key, value, 1, 16)?,
            "width" => self.width = number(key, value, 1, 10000)?,
            "allowed_blank_lines" => self.allowed_blank_lines = number(key, value, 0, 10)?,
            "final_newline" => self.final_newline = flag(key, value)?,
            "line_endings" => {
                self.line_endings = choice(
                    key,
                    value,
                    &[
                        ("unix", LineEndings::Unix),
                        ("windows", LineEndings::Windows),
                    ],
                )?
            }
            "align_entries" => self.align_entries = flag(key, value)?,
            "align_comments" => self.align_comments = flag(key, value)?,
            "align_single_comments" => self.align_single_comments = flag(key, value)?,
            "array_trailing_comma" => self.array_trailing_comma = flag(key, value)?,
            "array_auto_expand" => self.array_auto_expand = flag(key, value)?,
            "array_auto_collapse" => self.array_auto_collapse = flag(key, value)?,
            "compact_arrays" => self.compact_arrays = flag(key, value)?,
            "compact_inline_tables" => self.compact_inline_tables = flag(key, value)?,
            "compact_entries" => self.compact_entries = flag(key, value)?,
            "inline_table_expand" => self.inline_table_expand = flag(key, value)?,
            "indent_tables" => self.indent_tables = flag(key, value)?,
            "indent_entries" => self.indent_entries = flag(key, value)?,
            "reorder_keys" => self.reorder_keys = flag(key, value)?,
            "format_comments" => self.format_comments = flag(key, value)?,
            other => return Err(format!("unknown setting: {other}")),
        }
        Ok(())
    }
}

fn choice<T: Copy>(key: &str, value: &str, choices: &[(&str, T)]) -> Result<T, String> {
    choices
        .iter()
        .find(|(name, _)| *name == value)
        .map(|(_, choice)| *choice)
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

fn number(key: &str, value: &str, min: usize, max: usize) -> Result<usize, String> {
    value
        .parse::<usize>()
        .ok()
        .filter(|number| (min..=max).contains(number))
        .ok_or_else(|| format!("{key} must be between {min} and {max}, not {value}"))
}

fn flag(key: &str, value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("{key} must be true or false, not {other}")),
    }
}
