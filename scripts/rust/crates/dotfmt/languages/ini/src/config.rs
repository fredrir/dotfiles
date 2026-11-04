use dotfmt_core::diagnostic::Diagnostic;

use crate::dialect::Dialect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndentType {
    Spaces,
    Tabs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEndings {
    Unix,
    Windows,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub dialect: Dialect,
    /// Spaces per indentation level. `None` uses the dialect default.
    pub indent: Option<usize>,
    /// Explicit indentation character; `None` emits spaces.
    pub indent_type: Option<IndentType>,
    pub line_endings: LineEndings,
    pub final_newline: bool,
    pub blank_lines: usize,
    pub align: bool,
    pub align_max: usize,
    pub format_comments: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            dialect: Dialect::Auto,
            indent: None,
            indent_type: None,
            line_endings: LineEndings::Unix,
            final_newline: true,
            blank_lines: 1,
            align: true,
            align_max: 24,
            format_comments: false,
        }
    }
}

impl Config {
    pub fn from_settings(settings: &dotfmt_core::config::Settings) -> Result<Self, Diagnostic> {
        let mut config = Self::default();
        for (key, setting) in settings {
            if setting.global {
                match key.as_str() {
                    // Through the shared block these reach every language; INI has no
                    // use for either.
                    "width" | "quote_style" => continue,
                    "indent" => {
                        config.indent = Some(
                            number(key, &setting.value).map_err(|error| setting.error(error))?,
                        );
                        continue;
                    }
                    "final_newline" => {
                        config.final_newline =
                            flag(key, &setting.value).map_err(|error| setting.error(error))?;
                        continue;
                    }
                    _ => {}
                }
            }
            config
                .set(key, &setting.value)
                .map_err(|error| setting.error(error))?;
        }
        Ok(config)
    }

    fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "dialect" => self.dialect = Dialect::parse(value, true)?,
            "indent" => self.indent = Some(number(key, value)?),
            "indent_type" => {
                self.indent_type = Some(choice(
                    key,
                    value,
                    &[("spaces", IndentType::Spaces), ("tabs", IndentType::Tabs)],
                )?)
            }
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
            "final_newline" => self.final_newline = flag(key, value)?,
            "blank_lines" => self.blank_lines = number_in(key, value, 0, 10)?,
            "align" => self.align = flag(key, value)?,
            "align_max" => self.align_max = number_in(key, value, 0, 1000)?,
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

fn number(key: &str, value: &str) -> Result<usize, String> {
    value
        .parse()
        .map_err(|_| format!("{key} must be a whole number, not {value}"))
}

fn number_in(key: &str, value: &str, min: usize, max: usize) -> Result<usize, String> {
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
