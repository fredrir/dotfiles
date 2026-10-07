use std::fs;
use std::path::Path;

pub use dotfmt_core::file::Done;
use dotfmt_core::file::replace;

use crate::commented;
use crate::config::Config;
use crate::dialect::Dialect;
use crate::parse::{self, Options};
use crate::render;
use crate::repair::Repairs;

pub struct Formatted {
    pub text: String,
    pub repairs: Repairs,
}

pub struct Outcome {
    pub done: Done,
    pub repairs: Repairs,
}

/// Lays one body out. `editor` is the flag of the same name: it takes the
/// repairs jq would refuse, and counts them rather than hiding them.
pub fn format(
    label: &str,
    input: &[u8],
    config: &Config,
    editor: bool,
    dialect: Dialect,
) -> Result<Formatted, String> {
    let dialect = if editor { Dialect::Json } else { dialect };
    let shaped = shape(label, input, config, editor, dialect)?;
    guard(label, &shaped.text, config, dialect)?;
    Ok(shaped)
}

fn shape(
    label: &str,
    input: &[u8],
    config: &Config,
    editor: bool,
    dialect: Dialect,
) -> Result<Formatted, String> {
    if dialect.comments() {
        return commented::format(input, config.layout())
            .map(|text| Formatted {
                text,
                repairs: Repairs::default(),
            })
            .map_err(|problem| format!("{label}:{}", problem.said()));
    }
    let parsed = parse::parse(input, Options { editor })
        .map_err(|problem| format!("{label}:{}", problem.said()))?;
    let Some(value) = parsed.value else {
        // jq makes nothing of an empty input, so nothing is what an empty file
        // becomes. Anything else that holds no value is refused instead: a file
        // of comments is a file somebody wrote, and a formatter that answered
        // it with zero bytes is one nobody could leave on save.
        if input.is_empty() {
            return Ok(Formatted {
                text: String::new(),
                repairs: parsed.repairs,
            });
        }
        return Err(format!("{label}:1:1: expected a value"));
    };
    Ok(Formatted {
        text: render::write(&value, config.layout()),
        repairs: parsed.repairs,
    })
}

/// Validate output in its dialect and require a stable second formatting pass.
fn guard(label: &str, text: &str, config: &Config, dialect: Dialect) -> Result<(), String> {
    let again = shape(label, text.as_bytes(), config, false, dialect)?;
    if again.text != text {
        return Err(broken(label, "laying it out again does not settle"));
    }
    Ok(())
}

pub fn apply(
    path: &Path,
    label: &str,
    config: &Config,
    editor: bool,
    write: bool,
    dialect: Dialect,
) -> Result<Outcome, String> {
    let raw = fs::read(path).map_err(|error| format!("{label}: {error}"))?;
    let formatted = format(label, &raw, config, editor, dialect)?;
    if formatted.text.as_bytes() == raw {
        return Ok(Outcome {
            done: Done::Unchanged,
            repairs: formatted.repairs,
        });
    }
    if write {
        replace(path, formatted.text.as_bytes()).map_err(|error| format!("{label}: {error}"))?;
    }
    Ok(Outcome {
        done: Done::Changed,
        repairs: formatted.repairs,
    })
}

fn broken(label: &str, why: &str) -> String {
    format!("{label}: internal error: {why}, so nothing was written")
}
