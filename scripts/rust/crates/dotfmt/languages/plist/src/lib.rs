#![forbid(unsafe_code)]

pub mod config;
mod model;
mod parse;
mod print;
mod source;
mod verify;

use config::Config;
use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};

const BOM: &[u8] = b"\xef\xbb\xbf";

/// The result of formatting a property list.
#[derive(Debug)]
pub enum Output {
    /// Formatted XML, still to be written by the caller.
    Text(String),
    /// The input was not an XML property list (binary, OpenStep ASCII or empty);
    /// the caller must leave it untouched.
    Verbatim,
}

/// Formats an XML property list, preserving comments, CDATA, entity spellings,
/// text bytes and key order.
///
/// Fails with [`DiagnosticKind::Syntax`] when the input is XML but does not
/// parse or is not a property list, and with [`DiagnosticKind::Internal`] when
/// the formatted text would no longer mean the same thing as its input; in both
/// cases the caller keeps the file.
pub fn format(input: &[u8], config: &Config) -> Result<Output, Diagnostic> {
    let (bom, input) = match input.strip_prefix(BOM) {
        Some(input) => (true, input),
        None => (false, input),
    };
    if verbatim(input) {
        return Ok(Output::Verbatim);
    }
    let text = std::str::from_utf8(input).map_err(|error| {
        let (line, column) = source::line_column(input, error.valid_up_to());
        Diagnostic::new(
            DiagnosticKind::Syntax,
            "error parsing: the property list is not valid UTF-8",
        )
        .with_location(line, Some(column))
    })?;
    let document = parse::parse(text)?;
    let expected = verify::Fingerprint::of(&document);
    let mut output = print::print(&document, config);
    let reparsed =
        parse::parse(&output).map_err(|_| internal("formatting would produce invalid XML"))?;
    let actual = verify::Fingerprint::of(&reparsed);
    compare(&expected, &actual)?;
    if bom {
        output.insert(0, '\u{feff}');
    }
    Ok(Output::Text(output))
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

/// Binary (`bplist00`), OpenStep ASCII and empty inputs are not XML. Anything
/// that does not start with `<` is left alone rather than guessed at.
fn verbatim(input: &[u8]) -> bool {
    if input.starts_with(b"bplist") {
        return true;
    }
    !matches!(
        input
            .iter()
            .copied()
            .find(|byte| !byte.is_ascii_whitespace()),
        Some(b'<')
    )
}

fn internal(message: &str) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Internal, message)
}
