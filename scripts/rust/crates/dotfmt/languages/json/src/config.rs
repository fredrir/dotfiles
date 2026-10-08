use dotfmt_core::diagnostic::Diagnostic;

use crate::dialect::Dialect;
use crate::render::{Indent, Layout};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub dialect: Dialect,
    pub indent: Indent,
    pub final_newline: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            dialect: Dialect::Auto,
            indent: Indent::Spaces(2),
            final_newline: true,
        }
    }
}

impl Config {
    pub fn from_settings(settings: &dotfmt_core::config::Settings) -> Result<Self, Diagnostic> {
        let mut config = Self::default();
        for (key, setting) in settings {
            if setting.global && matches!(key.as_str(), "width" | "quote_style") {
                continue;
            }
            config
                .set(key, &setting.value)
                .map_err(|error| setting.error(error))?;
        }
        Ok(config)
    }

    pub fn layout(&self) -> Layout {
        Layout {
            indent: self.indent,
            final_newline: self.final_newline,
        }
    }

    fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "dialect" => self.dialect = value.parse::<Dialect>()?,
            "indent" => self.indent = indent(value)?,
            "final_newline" => self.final_newline = flag(key, value)?,
            "quote_style" if value == "double" => {}
            other => return Err(format!("unknown setting: {other}")),
        }
        Ok(())
    }
}

fn indent(value: &str) -> Result<Indent, String> {
    let width: i64 = value
        .parse()
        .map_err(|_| format!("indent must be a whole number, not {value}"))?;
    match width {
        -1 => Ok(Indent::Tabs),
        0 => Ok(Indent::Compact),
        1..=7 => Ok(Indent::Spaces(width as usize)),
        _ => Err(format!(
            "indent must be -1 for a tab, or between 0 and 7, not {width}"
        )),
    }
}

fn flag(key: &str, value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("{key} must be true or false, not {other}")),
    }
}
