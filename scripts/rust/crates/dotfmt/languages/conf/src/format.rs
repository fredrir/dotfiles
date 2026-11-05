use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};

use std::path::Path;

use crate::block;
use crate::conf;
use crate::config::Config;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Conf,
    Block,
}

pub fn format(path: &Path, input: &[u8], config: &Config) -> Result<String, Diagnostic> {
    format_with_context(path, path, input, config)
}

pub fn format_with_context(
    path: &Path,
    context: &Path,
    input: &[u8],
    config: &Config,
) -> Result<String, Diagnostic> {
    let text = std::str::from_utf8(input)
        .map_err(|_| Diagnostic::new(DiagnosticKind::Syntax, "not UTF-8").with_path(path))?;
    let kind = if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("dotfile"))
    {
        Kind::Block
    } else {
        Kind::Conf
    };
    format_as_context(path, context, text, kind, config)
}

fn format_as_context(
    path: &Path,
    context: &Path,
    text: &str,
    kind: Kind,
    config: &Config,
) -> Result<String, Diagnostic> {
    let formatted = shape(path, context, text, kind, config)?;
    guard(path, context, text, &formatted, kind, config)?;
    Ok(formatted)
}

fn shape(
    path: &Path,
    context: &Path,
    text: &str,
    kind: Kind,
    config: &Config,
) -> Result<String, Diagnostic> {
    match kind {
        Kind::Conf => Ok(conf::format(
            text,
            conf::mode(&shown(context)),
            config.final_newline,
        )),
        Kind::Block => block::format(text, config).map_err(|problem| {
            Diagnostic::new(DiagnosticKind::Syntax, problem.message)
                .with_path(path)
                .with_location(problem.line, None)
        }),
    }
}

fn guard(
    path: &Path,
    context: &Path,
    text: &str,
    formatted: &str,
    kind: Kind,
    config: &Config,
) -> Result<(), Diagnostic> {
    if shape(path, context, formatted, kind, config)? != formatted {
        return Err(broken(path, "laying it out again does not settle"));
    }
    if kind == Kind::Block {
        let before = block::signature(text).map_err(|problem| {
            Diagnostic::new(DiagnosticKind::Syntax, problem.message)
                .with_path(path)
                .with_location(problem.line, None)
        })?;
        let after = block::signature(formatted).map_err(|problem| {
            Diagnostic::new(DiagnosticKind::Syntax, problem.message)
                .with_path(path)
                .with_location(problem.line, None)
        })?;
        if before != after {
            return Err(broken(path, "the entries it holds would change"));
        }
    }
    Ok(())
}

fn shown(path: &Path) -> String {
    path.display().to_string()
}

fn broken(path: &Path, why: &str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticKind::Internal,
        format!("internal error: {why}, so nothing was written"),
    )
    .with_path(path)
}
