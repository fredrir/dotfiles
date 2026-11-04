use dotfmt_core::diagnostic::Diagnostic;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEndings {
    Unix,
    Windows,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndentType {
    Spaces,
    Tabs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrologStyle {
    Preserve,
    Normalize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmptyStyle {
    Preserve,
    Always,
    Never,
}

/// Layout choices for [`format`][crate::format].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// Spaces per indentation level when `indent_type` is [`IndentType::Spaces`].
    pub indent: usize,
    /// Unit used for one indentation level.
    pub indent_type: IndentType,
    /// Target line width; accepted from the shared global block, unused in v1.
    pub width: usize,
    pub final_newline: bool,
    pub line_endings: LineEndings,
    /// Maximum consecutive blank lines kept between elements.
    pub blank_lines: usize,
    /// Re-wrap `<data>` base64 at this width; `0` preserves the original bytes.
    pub data_width: usize,
    /// Keep the XML declaration, DOCTYPE and prolog comments exactly as written,
    /// or replace the declaration and DOCTYPE with Apple's canonical spelling.
    pub prolog: PrologStyle,
    /// Print empty elements self-closed, paired, or exactly as written.
    pub self_closing: EmptyStyle,
    /// Reordering dictionary keys changes duplicate-key shadowing, so v1 accepts
    /// only `false`.
    pub sort_keys: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            indent: 2,
            indent_type: IndentType::Spaces,
            width: 80,
            final_newline: true,
            line_endings: LineEndings::Unix,
            blank_lines: 1,
            data_width: 0,
            prolog: PrologStyle::Preserve,
            self_closing: EmptyStyle::Preserve,
            sort_keys: false,
        }
    }
}

impl Config {
    pub fn from_settings(settings: &dotfmt_core::config::Settings) -> Result<Self, Diagnostic> {
        let mut config = Self::default();
        for (key, setting) in settings {
            // Through the shared block this reaches every language; plists never
            // choose a quote style.
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
            "indent_type" => {
                self.indent_type = choice(
                    key,
                    value,
                    &[("spaces", IndentType::Spaces), ("tabs", IndentType::Tabs)],
                )?
            }
            "width" => self.width = number(key, value, 1, 10000)?,
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
            "blank_lines" => self.blank_lines = number(key, value, 0, 10)?,
            "data_width" => self.data_width = number(key, value, 0, 120)?,
            "prolog" => {
                self.prolog = choice(
                    key,
                    value,
                    &[
                        ("preserve", PrologStyle::Preserve),
                        ("normalize", PrologStyle::Normalize),
                    ],
                )?
            }
            "self_closing" => {
                self.self_closing = choice(
                    key,
                    value,
                    &[
                        ("preserve", EmptyStyle::Preserve),
                        ("always", EmptyStyle::Always),
                        ("never", EmptyStyle::Never),
                    ],
                )?
            }
            "sort_keys" => {
                if flag(key, value)? {
                    return Err(
                        "sort_keys must be false: reordering keys is not supported".to_owned()
                    );
                }
                self.sort_keys = false;
            }
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
