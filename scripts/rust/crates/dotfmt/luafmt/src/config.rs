use crate::dialect::Dialect;
use clap::ValueEnum;
use stylua_lib::{
    BlockNewlineGaps, CallParenType, CollapseSimpleStatement, IndentType, LineEndings, QuoteStyle,
    SpaceAfterFunctionNames,
};

#[derive(Clone, Debug)]
pub struct Config {
    pub dialect: Dialect,
    pub style: stylua_lib::Config,
    pub final_newline: bool,
    pub verify: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            dialect: Dialect::Auto,
            style: stylua_lib::Config {
                indent_type: IndentType::Spaces,
                indent_width: 2,
                call_parentheses: CallParenType::None,
                ..stylua_lib::Config::default()
            },
            final_newline: false,
            verify: false,
        }
    }
}

impl Config {
    pub fn from_settings(settings: &dotfmt_core::config::Settings) -> Result<Self, String> {
        let mut config = Self::default();
        for (key, setting) in settings {
            config.set(key, &setting.value).map_err(|error| {
                format!("{}:{}: {error}", setting.source.display(), setting.line)
            })?;
        }
        Ok(config)
    }

    fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "dialect" => {
                self.dialect = Dialect::from_str(value, false).map_err(|error| error.to_string())?
            }
            "width" => self.style.column_width = number(key, value, 1, 10000)?,
            "indent" => self.style.indent_width = number(key, value, 1, 16)?,
            "indent_type" => {
                self.style.indent_type = choice(
                    key,
                    value,
                    &[("spaces", IndentType::Spaces), ("tabs", IndentType::Tabs)],
                )?
            }
            "line_endings" => {
                self.style.line_endings = choice(
                    key,
                    value,
                    &[
                        ("unix", LineEndings::Unix),
                        ("windows", LineEndings::Windows),
                    ],
                )?
            }
            "quote_style" => {
                self.style.quote_style = choice(
                    key,
                    value,
                    &[
                        ("auto-prefer-double", QuoteStyle::AutoPreferDouble),
                        ("auto", QuoteStyle::AutoPreferDouble),
                        ("auto-prefer-single", QuoteStyle::AutoPreferSingle),
                        ("force-double", QuoteStyle::ForceDouble),
                        ("double", QuoteStyle::ForceDouble),
                        ("force-single", QuoteStyle::ForceSingle),
                        ("single", QuoteStyle::ForceSingle),
                    ],
                )?
            }
            "call_parentheses" => {
                self.style.call_parentheses = choice(
                    key,
                    value,
                    &[
                        ("always", CallParenType::Always),
                        ("no-single-string", CallParenType::NoSingleString),
                        ("no-single-table", CallParenType::NoSingleTable),
                        ("none", CallParenType::None),
                        ("input", CallParenType::Input),
                    ],
                )?
            }
            "collapse_simple_statement" => {
                self.style.collapse_simple_statement = choice(
                    key,
                    value,
                    &[
                        ("never", CollapseSimpleStatement::Never),
                        ("function-only", CollapseSimpleStatement::FunctionOnly),
                        ("conditional-only", CollapseSimpleStatement::ConditionalOnly),
                        ("always", CollapseSimpleStatement::Always),
                    ],
                )?
            }
            "space_after_function_names" => {
                self.style.space_after_function_names = choice(
                    key,
                    value,
                    &[
                        ("never", SpaceAfterFunctionNames::Never),
                        ("definitions", SpaceAfterFunctionNames::Definitions),
                        ("calls", SpaceAfterFunctionNames::Calls),
                        ("always", SpaceAfterFunctionNames::Always),
                    ],
                )?
            }
            "block_newline_gaps" => {
                self.style.block_newline_gaps = choice(
                    key,
                    value,
                    &[
                        ("never", BlockNewlineGaps::Never),
                        ("preserve", BlockNewlineGaps::Preserve),
                    ],
                )?
            }
            "sort_requires" => self.style.sort_requires.enabled = flag(key, value)?,
            "final_newline" => self.final_newline = flag(key, value)?,
            "verify" => self.verify = flag(key, value)?,
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
