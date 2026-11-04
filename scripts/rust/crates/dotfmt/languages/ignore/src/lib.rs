#![forbid(unsafe_code)]

pub mod config;
pub mod dialect;
mod parse;
mod print;
pub mod verify;

use std::path::Path;

use config::{Config, LineEndings};
use dialect::Dialect;
use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};

/// Format an ignore-family file without changing what it matches.
///
/// `path` selects the dialect unless the configured dialect is explicit.
pub fn format(path: &Path, input: &[u8], config: &Config) -> Result<String, Diagnostic> {
    let text = std::str::from_utf8(input)
        .map_err(|_| Diagnostic::new(DiagnosticKind::Syntax, "not UTF-8").with_path(path))?;
    let dialect = Dialect::Auto.resolve(config.dialect, Dialect::detect(path));
    settle(text, config, dialect).map_err(|error| {
        if error.path.is_some() {
            error
        } else {
            error.with_path(path)
        }
    })
}

/// Format using an explicit dialect, for `--dialect` and unit tests.
///
/// `Dialect::Auto` falls back to the configured dialect and then to
/// [`Dialect::Git`].
pub fn format_with_dialect(
    input: &str,
    config: &Config,
    dialect: Dialect,
) -> Result<String, Diagnostic> {
    let dialect = dialect.resolve(config.dialect, Dialect::Git);
    settle(input, config, dialect)
}

fn settle(input: &str, config: &Config, dialect: Dialect) -> Result<String, Diagnostic> {
    let output = run(input, config, dialect)?;
    if run(&output, config, dialect)? != output {
        return Err(internal(
            "internal error: formatting does not settle, so nothing was written",
        ));
    }
    Ok(output)
}

fn run(input: &str, config: &Config, dialect: Dialect) -> Result<String, Diagnostic> {
    let (bom, input) = match input.strip_prefix('\u{feff}') {
        Some(input) => (true, input),
        None => (false, input),
    };
    // Only CRLF is a line ending here. A lone CR is pattern data in every
    // dialect and is left where it is.
    let text = input.replace("\r\n", "\n");
    let blank_only = is_blank(&text);
    let mut output = if blank_only {
        text
    } else {
        let document = parse::parse(&text, dialect);
        let expected = verify::Fingerprint::of_document(&document, config.blank_lines);
        let output = print::print(&document, config, dialect);
        let reparsed = parse::parse(&output, dialect);
        let actual = verify::Fingerprint::of_document(&reparsed, config.blank_lines);
        verify::check(&expected, &actual)?;
        output
    };
    // Appending LF after a final lone CR would turn that CR into data-losing
    // CRLF on the next pass. Preserving the pattern wins over the trailer.
    if !blank_only && config.final_newline && !output.is_empty() && !output.ends_with('\r') {
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

fn is_blank(text: &str) -> bool {
    text.bytes()
        .all(|byte| matches!(byte, b' ' | b'\t' | b'\n'))
}

fn internal(message: &str) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Internal, message)
}
