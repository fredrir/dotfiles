use dotfmt_core::diagnostic::Diagnostic;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEndings {
    Unix,
    Windows,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub indent: usize,
    pub width: usize,
    pub final_newline: bool,
    pub line_endings: LineEndings,
    pub blank_lines: usize,
    pub align: bool,
    pub align_max: usize,
    pub format_comments: bool,
    pub trim_trailing_whitespace: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            indent: 2,
            width: 80,
            final_newline: true,
            line_endings: LineEndings::Unix,
            blank_lines: 1,
            align: false,
            align_max: 24,
            format_comments: false,
            trim_trailing_whitespace: true,
        }
    }
}

impl Config {
    pub fn from_settings(settings: &dotfmt_core::config::Settings) -> Result<Self, Diagnostic> {
        let mut config = Self::default();
        for (key, setting) in settings {
            if setting.global && key == "quote_style" {
                continue;
            }
            config
                .set(key, &setting.value, setting.global)
                .map_err(|error| setting.error(error))?;
        }
        Ok(config)
    }

    fn set(&mut self, key: &str, value: &str, global: bool) -> Result<(), String> {
        match key {
            "indent" if global => {
                if let Some(indent) = clamped(value, 1, 16) {
                    self.indent = indent;
                }
            }
            "indent" => self.indent = number(key, value, 1, 16)?,
            "width" if global => {
                if let Some(width) = clamped(value, 1, 10_000) {
                    self.width = width;
                }
            }
            "width" => self.width = number(key, value, 1, 10_000)?,
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
            "align" => self.align = flag(key, value)?,
            "align_max" => self.align_max = number(key, value, 0, 1000)?,
            "format_comments" => self.format_comments = flag(key, value)?,
            "trim_trailing_whitespace" => self.trim_trailing_whitespace = flag(key, value)?,
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

fn clamped(value: &str, minimum: usize, maximum: usize) -> Option<usize> {
    value
        .parse::<usize>()
        .ok()
        .map(|number| number.clamp(minimum, maximum))
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
