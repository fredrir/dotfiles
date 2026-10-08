use std::path::Path;

use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};

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

/// Lays one body out. `editor` is the flag of the same name: it takes the
/// repairs jq would refuse, and counts them rather than hiding them.
pub fn format(
    path: &Path,
    input: &[u8],
    config: &Config,
    editor: bool,
    dialect: Dialect,
) -> Result<Formatted, Diagnostic> {
    let dialect = if editor { Dialect::Json } else { dialect };
    let shaped = shape(path, input, config, editor, dialect)?;
    guard(path, &shaped.text, config, dialect)?;
    Ok(shaped)
}

fn shape(
    path: &Path,
    input: &[u8],
    config: &Config,
    editor: bool,
    dialect: Dialect,
) -> Result<Formatted, Diagnostic> {
    if dialect.comments() {
        return commented::format(input, config.layout())
            .map(|text| Formatted {
                text,
                repairs: Repairs::default(),
            })
            .map_err(|problem| {
                Diagnostic::new(DiagnosticKind::Syntax, problem.message)
                    .with_path(path)
                    .with_location(problem.line, Some(problem.column))
            });
    }
    let parsed = parse::parse(input, Options { editor }).map_err(|problem| {
        Diagnostic::new(DiagnosticKind::Syntax, problem.message)
            .with_path(path)
            .with_location(problem.line, Some(problem.column))
    })?;
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
        return Err(Diagnostic::new(DiagnosticKind::Syntax, "expected a value")
            .with_path(path)
            .with_location(1, Some(1)));
    };
    Ok(Formatted {
        text: render::write(&value, config.layout()),
        repairs: parsed.repairs,
    })
}

/// Validate output in its dialect and require a stable second formatting pass.
fn guard(path: &Path, text: &str, config: &Config, dialect: Dialect) -> Result<(), Diagnostic> {
    let again = shape(path, text.as_bytes(), config, false, dialect)?;
    if again.text != text {
        return Err(broken(path, "laying it out again does not settle"));
    }
    Ok(())
}

fn broken(path: &Path, why: &str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticKind::Internal,
        format!("internal error: {why}, so nothing was written"),
    )
    .with_path(path)
}
