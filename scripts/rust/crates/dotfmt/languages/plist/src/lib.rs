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

#[derive(Debug)]
pub enum Output {
    Text(String),
    Verbatim,
}

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

fn compare(expected: &verify::Fingerprint, actual: &verify::Fingerprint) -> Result<(), Diagnostic> {
    if !expected.matches(actual) {
        return Err(internal("formatting would change values"));
    }
    if !expected.comments_match(actual) {
        return Err(internal("formatting would change comments"));
    }
    Ok(())
}

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
