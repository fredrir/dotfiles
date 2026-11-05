#![forbid(unsafe_code)]

pub mod config;
pub mod dialect;
mod parse;
mod print;
mod verify;

use std::path::Path;

use config::{Config, LineEndings};
use dialect::Dialect;
use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};

pub fn format(path: &Path, input: &[u8], config: &Config) -> Result<String, Diagnostic> {
    let input = std::str::from_utf8(input).map_err(|_| {
        Diagnostic::new(DiagnosticKind::Syntax, "input is not valid UTF-8").with_path(path)
    })?;
    let dialect = Dialect::Auto.resolve(config.dialect, path);
    format_with_dialect(input, config, dialect)
}

pub fn format_with_dialect(
    input: &str,
    config: &Config,
    dialect: Dialect,
) -> Result<String, Diagnostic> {
    let dialect = match dialect {
        Dialect::Auto => match config.dialect {
            Dialect::Auto => Dialect::Generic,
            configured => configured,
        },
        explicit => explicit,
    };
    let (bom, text) = match input.strip_prefix('\u{feff}') {
        Some(text) => (true, text),
        None => (false, input),
    };
    if text.trim().is_empty() {
        return Ok(input.to_owned());
    }
    let text = text.replace("\r\n", "\n");
    let mut output = format_body(&text, bom, config, dialect)?;
    if config.final_newline
        && !output.is_empty()
        && !output.ends_with('\n')
        && !output.ends_with('\r')
    {
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

fn format_body(
    text: &str,
    bom: bool,
    config: &Config,
    dialect: Dialect,
) -> Result<String, Diagnostic> {
    let source = with_bom(bom, text);
    let document = parse::parse(&source, dialect);
    let expected = verify::Fingerprint::of(&document, config, dialect);
    let body = print::print(&document, config, dialect);

    let guarded = with_bom(bom, &body);
    let reparsed = parse::parse(&guarded, dialect);
    if expected != verify::Fingerprint::of(&reparsed, config, dialect) {
        return Err(internal("formatting would change INI content"));
    }
    let second = print::print(&reparsed, config, dialect);
    if with_bom(bom, &second) != guarded {
        return Err(internal("formatting is not idempotent"));
    }
    Ok(body)
}

fn with_bom(bom: bool, text: &str) -> String {
    if bom {
        format!("\u{feff}{text}")
    } else {
        text.to_owned()
    }
}

fn internal(message: &str) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Internal, message)
}
