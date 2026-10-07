use std::path::Path;

use clap::ValueEnum;
use dotfmt_core::config::{Language, Settings};

pub enum Engine {
    Conf(confmt::config::Config),
    Json(jqfmt::config::Config, jqfmt::dialect::Dialect),
    Lua(luafmt::config::Config, luafmt::dialect::Dialect),
    Markdown(mdfmt::config::Config, mdfmt::dialect::Dialect),
}

pub struct Formatted {
    pub text: String,
    pub repairs: Option<String>,
}

impl Engine {
    pub fn new(
        language: Language,
        settings: &Settings,
        dialect: Option<&str>,
    ) -> Result<Self, String> {
        let value = dialect.unwrap_or("auto");
        match language {
            Language::Conf => {
                if value != "auto" {
                    return Err(format!("conf has no {value:?} dialect"));
                }
                Ok(Self::Conf(confmt::config::Config::from_settings(settings)?))
            }
            Language::Json => Ok(Self::Json(
                jqfmt::config::Config::from_settings(settings)?,
                jqfmt::dialect::Dialect::from_str(value, true)?,
            )),
            Language::Lua => Ok(Self::Lua(
                luafmt::config::Config::from_settings(settings)?,
                luafmt::dialect::Dialect::from_str(value, true)?,
            )),
            Language::Markdown => Ok(Self::Markdown(
                mdfmt::config::Config::from_settings(settings)?,
                mdfmt::dialect::Dialect::from_str(value, true)?,
            )),
        }
    }

    pub fn format(&self, path: &Path, input: &[u8], editor: bool) -> Result<Formatted, String> {
        let mut repairs = None;
        let text = match self {
            Self::Conf(config) => confmt::format(path, input, config)?,
            Self::Json(config, dialect) => {
                let output = jqfmt::format(
                    &path.display().to_string(),
                    input,
                    config,
                    editor,
                    if *dialect == jqfmt::dialect::Dialect::Auto {
                        config.dialect
                    } else {
                        *dialect
                    }
                    .resolve(path),
                )?;
                if !output.repairs.is_empty() {
                    repairs = Some(output.repairs.describe());
                }
                output.text
            }
            Self::Lua(config, dialect) => luafmt::format_with_dialect(
                utf8(input)?,
                config,
                dialect.resolve(config.dialect, path),
            )?,
            Self::Markdown(config, dialect) => {
                let mut config = config.clone();
                config.dialect = dialect.resolve(config.dialect, path);
                mdfmt::format(utf8(input)?, &config)?
            }
        };
        Ok(Formatted { text, repairs })
    }
}

fn utf8(input: &[u8]) -> Result<&str, String> {
    std::str::from_utf8(input).map_err(|_| "not UTF-8".into())
}
