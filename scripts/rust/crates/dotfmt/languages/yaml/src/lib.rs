#![forbid(unsafe_code)]

pub mod config;
mod print;
mod repair;
mod source;
mod syntax;

use config::{Config, LineEndings};
use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};
pub use repair::repair;
use source::Source;

pub fn format(input: &str, config: &Config) -> Result<String, Diagnostic> {
    let (bom, input) = match input.strip_prefix('\u{feff}') {
        Some(input) => (true, input),
        None => (false, input),
    };
    let text = input.replace("\r\n", "\n").replace('\r', "\n");
    let expected = syntax::read(&text)?;
    let source = Source::new(&text);
    let stream = syntax::parse(&source, &expected)?;
    let mut output = print::print(&source, &stream, config);
    if let Some(block) = stream.trailing_block_scalar() {
        // The source spelling decides whether the document may end without a
        // newline. A whitespace-only last line is only outside the scalar when
        // it is shorter than the scalar's content indentation; an indented
        // line is content, and a line wider than the indentation carries spaces
        // rather than a newline.
        let last_line = text.rsplit('\n').next().unwrap_or_default();
        let whitespace = last_line.chars().all(char::is_whitespace);
        let mut keep_newline = whitespace && !last_line.is_empty();
        if keep_newline {
            let indent = match &block.kind {
                syntax::Kind::Block { lines, .. } => {
                    lines.as_ref().and_then(|lines| lines.indent).unwrap_or(0)
                }
                _ => 0,
            };
            let width = last_line.chars().count();
            keep_newline = width <= indent;
        }
        if !text.ends_with('\n') && !keep_newline {
            output.pop();
        }
    } else {
        output.truncate(output.trim_end_matches('\n').len());
        if config.final_newline && !output.is_empty() {
            output.push('\n');
        }
    }
    let formatted =
        syntax::read(&output).map_err(|_| internal("formatting would produce invalid YAML"))?;
    let changed = (0..expected.len().max(formatted.len())).find(|index| {
        expected.get(*index).map(|read| &read.shape)
            != formatted.get(*index).map(|read| &read.shape)
    });
    if let Some(index) = changed {
        let at = expected
            .get(index)
            .or(expected.last())
            .map_or(0, |read| source.byte(read.start));
        return Err(internal("formatting would change values")
            .with_location(source.line(at) + 1, Some(source.column(at) + 1)));
    }
    if config.line_endings == LineEndings::Windows {
        output = output.replace('\n', "\r\n");
    }
    if bom {
        output.insert(0, '\u{feff}');
    }
    Ok(output)
}

fn internal(message: &str) -> Diagnostic {
    Diagnostic::new(DiagnosticKind::Internal, message)
}
