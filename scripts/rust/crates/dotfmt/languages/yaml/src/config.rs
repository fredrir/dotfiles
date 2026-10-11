use dotfmt_core::diagnostic::Diagnostic;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuoteStyle {
    PreferDouble,
    PreferSingle,
    Double,
    Single,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEndings {
    Unix,
    Windows,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DashSpacing {
    OneSpace,
    Indent,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub width: usize,
    pub indent: usize,
    pub quote_style: QuoteStyle,
    pub line_endings: LineEndings,
    pub indent_sequences: bool,
    pub trailing_comma: bool,
    pub brace_spacing: bool,
    pub bracket_spacing: bool,
    pub dash_spacing: DashSpacing,
    pub prefer_single_line: bool,
    pub format_comments: bool,
    pub final_newline: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            width: 80,
            indent: 2,
            quote_style: QuoteStyle::PreferDouble,
            line_endings: LineEndings::Unix,
            indent_sequences: true,
            trailing_comma: false,
            brace_spacing: false,
            bracket_spacing: false,
            dash_spacing: DashSpacing::OneSpace,
            prefer_single_line: false,
            format_comments: false,
            final_newline: true,
        }
    }
}

impl Config {
    pub fn from_settings(settings: &dotfmt_core::config::Settings) -> Result<Self, Diagnostic> {
        let mut config = Self::default();
        for (key, setting) in settings {
            config
                .set(key, &setting.value)
                .map_err(|error| setting.error(error))?;
        }
        Ok(config)
    }

    pub(crate) fn dash_width(&self) -> usize {
        match self.dash_spacing {
            DashSpacing::OneSpace => 2,
            DashSpacing::Indent => self.indent.max(2),
        }
    }

    fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "width" => self.width = number(key, value, 1, 10000)?,
            "indent" => self.indent = number(key, value, 1, 16)?,
            "quote_style" => {
                self.quote_style = choice(
                    key,
                    value,
                    &[
                        ("auto-prefer-double", QuoteStyle::PreferDouble),
                        ("auto", QuoteStyle::PreferDouble),
                        ("auto-prefer-single", QuoteStyle::PreferSingle),
                        ("force-double", QuoteStyle::Double),
                        ("double", QuoteStyle::Double),
                        ("force-single", QuoteStyle::Single),
                        ("single", QuoteStyle::Single),
                    ],
                )?
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
            "indent_sequences" => self.indent_sequences = flag(key, value)?,
            "trailing_comma" => self.trailing_comma = flag(key, value)?,
            "brace_spacing" => self.brace_spacing = flag(key, value)?,
            "bracket_spacing" => self.bracket_spacing = flag(key, value)?,
            "dash_spacing" => {
                self.dash_spacing = choice(
                    key,
                    value,
                    &[
                        ("one-space", DashSpacing::OneSpace),
                        ("indent", DashSpacing::Indent),
                    ],
                )?
            }
            "prefer_single_line" => self.prefer_single_line = flag(key, value)?,
            "format_comments" => self.format_comments = flag(key, value)?,
            "final_newline" => self.final_newline = flag(key, value)?,
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
        .filter(|n| (min..=max).contains(n))
        .ok_or_else(|| format!("{key} must be between {min} and {max}, not {value}"))
}

fn flag(key: &str, value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("{key} must be true or false, not {other}")),
    }
}
