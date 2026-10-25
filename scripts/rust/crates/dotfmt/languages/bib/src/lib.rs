#![forbid(unsafe_code)]

pub mod config;
mod parse;
mod print;
mod verify;

use config::{Config, LineEndings};
use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};

/// Format a BibTeX or BibLaTeX bibliography without changing any value bytes.
pub fn format(input: &str, config: &Config) -> Result<String, Diagnostic> {
    let (bom, input) = match input.strip_prefix('\u{feff}') {
        Some(input) => (true, input),
        None => (false, input),
    };
    let text = input.replace("\r\n", "\n").replace('\r', "\n");
    let mut output = if text.trim().is_empty() {
        String::new()
    } else {
        format_once(&text, config)?
    };
    if config.final_newline && !output.is_empty() {
        output.push('\n');
    }
    if config.line_endings == LineEndings::Windows {
        output = output.replace('\n', "\r\n");
    }
    if bom {
        output.insert(0, '\u{feff}');
    }
    Ok(output)
}

fn format_once(text: &str, config: &Config) -> Result<String, Diagnostic> {
    let document = parse::parse(text)?;
    let expected = verify::of(&document, text, config.wrap);
    let mut output = print::print(&document, text, config);
    output.truncate(output.trim_end_matches('\n').len());
    let parsed =
        parse::parse(&output).map_err(|_| internal("formatting would produce invalid BibTeX"))?;
    if expected != verify::of(&parsed, &output, config.wrap) {
        return Err(internal("formatting would change the bibliography"));
    }
    Ok(output)
}

fn internal(message: &str) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Internal, message)
}
