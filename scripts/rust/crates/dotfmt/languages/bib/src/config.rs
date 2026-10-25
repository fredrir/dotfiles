use dotfmt_core::diagnostic::Diagnostic;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NameCase {
    #[default]
    Preserve,
    Lower,
    Upper,
}

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

#[derive(Clone, Debug)]
pub struct Config {
    pub width: usize,
    pub indent: usize,
    pub indent_type: IndentType,
    pub line_endings: LineEndings,
    pub final_newline: bool,
    pub blank_lines: usize,
    pub align: bool,
    pub align_max: usize,
    pub trailing_comma: bool,
    pub wrap: bool,
    pub wrap_min: Option<usize>,
    pub field_name_case: NameCase,
    pub entry_type_case: NameCase,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            width: 80,
            indent: 2,
            indent_type: IndentType::Spaces,
            line_endings: LineEndings::Unix,
            final_newline: false,
            blank_lines: 1,
            align: true,
            align_max: 24,
            trailing_comma: false,
            wrap: false,
            wrap_min: None,
            field_name_case: NameCase::Lower,
            entry_type_case: NameCase::Lower,
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
                .set(key, &setting.value)
                .map_err(|error| setting.error(error))?;
        }
        Ok(config)
    }

    fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "width" => self.width = number(key, value, 1, 10000)?,
            "indent" => self.indent = number(key, value, 1, 16)?,
            "indent_type" => {
                self.indent_type = choice(
                    key,
                    value,
                    &[("spaces", IndentType::Spaces), ("tabs", IndentType::Tabs)],
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
            "final_newline" => self.final_newline = flag(key, value)?,
            "blank_lines" => self.blank_lines = number(key, value, 0, 10)?,
            "align" => self.align = flag(key, value)?,
            "align_max" => self.align_max = number(key, value, 0, 1000)?,
            "trailing_comma" => self.trailing_comma = flag(key, value)?,
            "wrap" => self.wrap = flag(key, value)?,
            "wrap_min" => {
                self.wrap_min = Some(number(key, value, 1, 10000)?);
            }
            "field_name_case" => self.field_name_case = name_case(key, value)?,
            "entry_type_case" => self.entry_type_case = name_case(key, value)?,
            other => return Err(format!("unknown setting: {other}")),
        }
        Ok(())
    }

    pub fn wrap_min(&self) -> usize {
        self.wrap_min
            .unwrap_or_else(|| self.width.saturating_sub(10).max(1))
    }

    pub fn indent_unit(&self) -> String {
        match self.indent_type {
            IndentType::Spaces => " ".repeat(self.indent),
            IndentType::Tabs => "\t".to_owned(),
        }
    }
}

fn name_case(key: &str, value: &str) -> Result<NameCase, String> {
    choice(
        key,
        value,
        &[
            ("preserve", NameCase::Preserve),
            ("lower", NameCase::Lower),
            ("lowercase", NameCase::Lower),
            ("upper", NameCase::Upper),
            ("uppercase", NameCase::Upper),
        ],
    )
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
