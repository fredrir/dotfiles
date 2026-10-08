use dotfmt_core::diagnostic::Diagnostic;

use crate::dialect::Dialect;
use comrak::options::ListStyleType;

#[derive(Clone, Debug)]
pub struct Config {
    pub dialect: Dialect,
    pub width: usize,
    pub table_style: TableStyle,
    pub heading_blank_lines: usize,
    pub list_marker: ListStyleType,
    pub trim_trailing_blank_lines: bool,
    pub final_newline: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            dialect: Dialect::Auto,
            width: 80,
            table_style: TableStyle::Auto,
            heading_blank_lines: 1,
            list_marker: ListStyleType::Dash,
            trim_trailing_blank_lines: true,
            final_newline: false,
        }
    }
}

impl Config {
    pub fn from_settings(settings: &dotfmt_core::config::Settings) -> Result<Self, Diagnostic> {
        let mut config = Self::default();
        for (key, setting) in settings {
            if setting.global && matches!(key.as_str(), "indent" | "quote_style") {
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
                self.dialect = value.parse::<Dialect>().map_err(|_| {
                    "dialect must be auto, commonmark, gfm, github, or obsidian".to_string()
                })?
            }
            "width" => self.width = number(key, value, 0, 10000)?,
            "heading_blank_lines" => self.heading_blank_lines = number(key, value, 0, 3)?,
            "trim_trailing_blank_lines" => self.trim_trailing_blank_lines = flag(key, value)?,
            "final_newline" => self.final_newline = flag(key, value)?,
            "table_style" => {
                self.table_style = match value {
                    "auto" => TableStyle::Auto,
                    "aligned" => TableStyle::Aligned,
                    "compact" => TableStyle::Compact,
                    _ => return Err("table_style must be auto, aligned, or compact".into()),
                }
            }
            "list_marker" => {
                self.list_marker = match value {
                    "-" => ListStyleType::Dash,
                    "*" => ListStyleType::Star,
                    "+" => ListStyleType::Plus,
                    _ => return Err("list_marker must be -, *, or +".into()),
                }
            }
            other => return Err(format!("unknown setting: {other}")),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableStyle {
    Auto,
    Aligned,
    Compact,
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
