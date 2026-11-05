use std::path::Path;

use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};

use crate::commented;
use crate::config::Config;
use crate::dialect::Dialect;
use crate::parse::{self, Options};
use crate::render;
use dotfmt_core::repair::{Repair, Repairs};

pub struct Formatted {
    pub text: String,
    pub repairs: Repairs,
}

pub fn format(
    path: &Path,
    input: &[u8],
    config: &Config,
    repair: bool,
    dialect: Dialect,
) -> Result<Formatted, Diagnostic> {
    let mut resolved = dialect.resolve(path);
    let mut shaped = shape(path, input, config, repair, resolved)?;
    if dialect == Dialect::Auto
        && resolved == Dialect::Json
        && shaped.repairs.of(Repair::Comment) > 0
    {
        resolved = Dialect::Jsonc;
        shaped = shape(path, input, config, repair, resolved)?;
    }
    guard(path, &shaped.text, config, resolved)?;
    Ok(shaped)
}

fn shape(
    path: &Path,
    input: &[u8],
    config: &Config,
    repair: bool,
    dialect: Dialect,
) -> Result<Formatted, Diagnostic> {
    if dialect.comments() {
        return commented::format(input, config.layout(), repair)
            .map(|(text, repairs)| Formatted { text, repairs })
            .map_err(|problem| syntax(path, problem));
    }
    if dialect == Dialect::Jsonl {
        return lines(path, input, config, repair);
    }
    let options = Options {
        repair,
        commented: false,
    };
    let parsed = parse::parse(input, options).map_err(|problem| syntax(path, problem))?;
    let Some(value) = parsed.value else {
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

fn lines(
    path: &Path,
    input: &[u8],
    config: &Config,
    repair: bool,
) -> Result<Formatted, Diagnostic> {
    let options = Options {
        repair,
        commented: false,
    };
    let streamed = parse::stream(input, options).map_err(|problem| syntax(path, problem))?;
    if streamed.values.is_empty() && !input.trim_ascii().is_empty() {
        return Err(Diagnostic::new(DiagnosticKind::Syntax, "expected a value")
            .with_path(path)
            .with_location(1, Some(1)));
    }
    Ok(Formatted {
        text: render::write_lines(&streamed.values, config.final_newline),
        repairs: streamed.repairs,
    })
}

fn syntax(path: &Path, problem: parse::Problem) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Syntax, problem.message)
        .with_path(path)
        .with_location(problem.line, Some(problem.column))
}

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
