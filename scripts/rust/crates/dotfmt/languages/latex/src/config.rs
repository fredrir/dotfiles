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

#[derive(Clone, Debug)]
pub struct Config {
    pub dialect: Dialect,
    pub width: usize,
    pub indent: usize,
    pub indent_type: IndentType,
    pub line_endings: LineEndings,
    pub wrap: bool,
    pub wrap_min: Option<usize>,
    pub wrap_chars: Vec<char>,
    pub format_tables: bool,
    pub blank_lines: usize,
    pub final_newline: bool,
    pub verify: bool,
    pub lists: Vec<String>,
    pub verbatims: Vec<String>,
    pub no_indent_envs: Vec<String>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            dialect: Dialect::Auto,
            width: 80,
            indent: 2,
            indent_type: IndentType::Spaces,
            line_endings: LineEndings::Unix,
            wrap: true,
            wrap_min: None,
            wrap_chars: vec![' '],
            format_tables: false,
            blank_lines: 1,
            final_newline: false,
            verify: false,
            lists: Vec::new(),
            verbatims: Vec::new(),
            no_indent_envs: Vec::new(),
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
            "dialect" => {
                self.dialect = value
                    .parse::<Dialect>()
                    .map_err(|_| "dialect must be auto, latex, plain, or context".to_string())?
            }
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
            "wrap" => self.wrap = flag(key, value)?,
            "wrap_min" => {
                let minimum = number(key, value, 0, 10000)?;
                self.wrap_min = (minimum > 0).then_some(minimum);
            }
            "wrap_chars" => {
                let mut chars = value.chars().collect::<Vec<_>>();
                if chars.is_empty() {
                    chars.push(' ');
                }
                if !chars.contains(&' ') {
                    chars.push(' ');
                }
                self.wrap_chars = chars;
            }
            "format_tables" => self.format_tables = flag(key, value)?,
            "blank_lines" => self.blank_lines = number(key, value, 0, 10)?,
            "final_newline" => self.final_newline = flag(key, value)?,
            "verify" => self.verify = flag(key, value)?,
            "lists" => self.lists = names(value),
            "verbatims" => self.verbatims = names(value),
            "no_indent_envs" => self.no_indent_envs = names(value),
            other => return Err(format!("unknown setting: {other}")),
        }
        Ok(())
    }

    pub fn wrap_min(&self) -> usize {
        self.wrap_min
            .unwrap_or_else(|| self.width.saturating_sub(10).max(1))
    }
}

fn names(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .collect()
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
