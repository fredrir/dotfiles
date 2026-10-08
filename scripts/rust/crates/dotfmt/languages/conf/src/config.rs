use dotfmt_core::diagnostic::Diagnostic;

#[derive(Clone, Debug)]
pub struct Config {
    pub indent: usize,
    pub align: bool,
    pub align_max: usize,
    pub blank_lines: usize,
    pub final_newline: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            indent: 2,
            align: true,
            align_max: 24,
            blank_lines: 1,
            final_newline: false,
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

    fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "indent" => self.indent = number(key, value)?,
            "align" => self.align = flag(key, value)?,
            "align_max" => self.align_max = number(key, value)?,
            "blank_lines" => self.blank_lines = number(key, value)?,
            "final_newline" => self.final_newline = flag(key, value)?,
            other => return Err(format!("unknown setting: {other}")),
        }
        Ok(())
    }
}

fn number(key: &str, value: &str) -> Result<usize, String> {
    value
        .parse()
        .map_err(|_| format!("{key} must be a whole number, not {value}"))
}

fn flag(key: &str, value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("{key} must be true or false, not {other}")),
    }
}
