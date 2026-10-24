#![forbid(unsafe_code)]

pub mod config;
mod model;
mod print;
mod trivia;
mod verify;

use config::{Config, LineEndings};
use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};
use toml_edit::{ImDocument, TomlError};

#[cfg(test)]
#[path = "../tests/unit/verify_tests.rs"]
mod verify_tests;

/// Formats a TOML document, preserving comments, literals and values.
///
/// Fails with [`DiagnosticKind::Syntax`] when the input does not parse, and with
/// [`DiagnosticKind::Internal`] when the formatted text would no longer mean
/// the same thing as its input; in both cases the caller keeps the file.
pub fn format(input: &str, config: &Config) -> Result<String, Diagnostic> {
    let (bom, input) = match input.strip_prefix('\u{feff}') {
        Some(input) => (true, input),
        None => (false, input),
    };
    if let Some(offset) = bare_carriage_return(input) {
        let (line, column) = line_column(input, offset);
        return Err(Diagnostic::new(
            DiagnosticKind::Syntax,
            "error parsing: a carriage return must be followed by a line feed",
        )
        .with_location(line, Some(column)));
    }
    let text = input.replace("\r\n", "\n");
    let document = parse(&text)?;
    let (expected, mut output) = {
        let model = model::build(&document)?;
        let expected = verify::Fingerprint::of(&model, config);
        (expected, print::print(&model, config))
    };
    output.truncate(output.trim_end_matches('\n').len());
    if config.final_newline && !output.is_empty() {
        output.push('\n');
    }
    {
        let checked = ImDocument::parse(output.as_str())
            .map_err(|_| internal("formatting would produce invalid TOML"))?;
        let rebuilt = model::build(&checked)
            .map_err(|_| internal("formatting would produce invalid TOML"))?;
        compare(&expected, &verify::Fingerprint::of(&rebuilt, config))?;
    }
    if config.line_endings == LineEndings::Windows {
        output = output.replace('\n', "\r\n");
    }
    if bom {
        output.insert(0, '\u{feff}');
    }
    Ok(output)
}

/// A layout bug must never reach the file it would corrupt, so a mismatch is
/// reported as an internal failure and the input is left alone.
fn compare(expected: &verify::Fingerprint, actual: &verify::Fingerprint) -> Result<(), Diagnostic> {
    if !expected.matches(actual) {
        return Err(internal("formatting would change values"));
    }
    if !expected.comments_match(actual) {
        return Err(internal("formatting would change comments"));
    }
    Ok(())
}

fn parse(text: &str) -> Result<ImDocument<&str>, Diagnostic> {
    ImDocument::parse(text).map_err(|error| syntax(text, &error))
}

fn syntax(text: &str, error: &TomlError) -> Diagnostic {
    let message = error
        .message()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(": ");
    let diagnostic = Diagnostic::new(DiagnosticKind::Syntax, message);
    match error.span() {
        Some(span) => {
            let (line, column) = line_column(text, span.start);
            diagnostic.with_location(line, Some(column))
        }
        None => diagnostic,
    }
}

fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let before = text.get(..offset).unwrap_or(text);
    let line = before.matches('\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .map_or(1, |line| line.chars().count() + 1);
    (line, column)
}

fn internal(message: &str) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Internal, message)
}

/// A lone CR is not a TOML line ending; normalizing it would repair invalid
/// input, so it is reported before the parser ever sees it.
fn bare_carriage_return(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    bytes.iter().enumerate().find_map(|(index, byte)| {
        (*byte == b'\r' && bytes.get(index + 1) != Some(&b'\n')).then_some(index)
    })
}
