#![forbid(unsafe_code)]

mod comment;
pub mod config;
mod model;
mod parse;
mod print;
mod verify;

use config::Config;
use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};

pub fn format(input: &str, config: &Config) -> Result<String, Diagnostic> {
    let (bom, text) = match input.strip_prefix('\u{feff}') {
        Some(text) => (true, text),
        None => (false, input),
    };
    let document = parse::parse(text)?;
    if document.is_blank() {
        return Ok(input.to_owned());
    }

    let expected = verify::Fingerprint::of(&document, config);
    let mut output = print::print(&document, config, bom);
    let reparsed =
        parse::parse(&output).map_err(|_| internal("formatting would produce invalid dotenv"))?;
    verify::check(&expected, &verify::Fingerprint::of(&reparsed, config))?;
    if print::print(&reparsed, config, bom) != output {
        return Err(internal("laying it out again does not settle"));
    }

    if bom {
        output.insert(0, '\u{feff}');
    }
    Ok(output)
}

pub fn format_bytes(input: &[u8], config: &Config) -> Result<String, Diagnostic> {
    let text = std::str::from_utf8(input)
        .map_err(|_| Diagnostic::new(DiagnosticKind::Syntax, "not UTF-8"))?;
    format(text, config)
}

fn internal(message: &str) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Internal, message)
}
