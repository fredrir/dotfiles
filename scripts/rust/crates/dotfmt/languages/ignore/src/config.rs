use dotfmt_core::diagnostic::Diagnostic;

use crate::dialect::Dialect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEndings {
    Unix,
    Windows,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub dialect: Dialect,
    pub line_endings: LineEndings,
    pub final_newline: bool,
    pub blank_lines: usize,
    pub trim_trailing_whitespace: bool,
    pub trim_leading_whitespace: bool,
    pub format_comments: bool,
    pub align: bool,
    pub align_max: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            dialect: Dialect::Auto,
            line_endings: LineEndings::Unix,
            final_newline: false,
            blank_lines: 1,
            trim_trailing_whitespace: true,
            trim_leading_whitespace: false,
            format_comments: false,
            align: true,
            align_max: 24,
        }
    }
}

impl Config {
    pub fn from_settings(settings: &dotfmt_core::config::Settings) -> Result<Self, Diagnostic> {
        let mut config = Self::default();
        for (key, setting) in settings {
            if setting.global && matches!(key.as_str(), "width" | "indent" | "quote_style") {
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
            "dialect" => self.dialect = dialect(key, value)?,
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
            "blank_lines" => self.blank_lines = number(key, value, 0, 10)?,
            "trim_trailing_whitespace" => self.trim_trailing_whitespace = flag(key, value)?,
            "trim_leading_whitespace" => self.trim_leading_whitespace = flag(key, value)?,
            "format_comments" => self.format_comments = flag(key, value)?,
            "align" => self.align = flag(key, value)?,
            "align_max" => self.align_max = number(key, value, 0, 1000)?,
            other => return Err(format!("unknown setting: {other}")),
        }
        Ok(())
    }
}

fn dialect(key: &str, value: &str) -> Result<Dialect, String> {
    Dialect::parse(value, false).map_err(|_| {
        let choices = Dialect::ALL
            .iter()
            .flat_map(|dialect| {
                std::iter::once(dialect.name()).chain(dialect.aliases().iter().copied())
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("{key} must be one of {choices}, not {value}")
    })
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
