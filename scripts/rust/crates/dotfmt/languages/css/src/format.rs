use std::path::Path;

use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};
use malva::format_text;

use crate::apply::{block_comment_end, sort_apply, string_end};
use crate::config::{Config, LineEndings};
use crate::dialect::Dialect;

/// Malva parses nested blocks and functions recursively. Keep the recursion
/// well inside the default thread stack so adversarial input fails with a
/// diagnostic instead of aborting the process.
const MAX_NESTING_DEPTH: usize = 32;

/// Formats CSS, SCSS, Sass, or Less source.
///
/// A leading BOM is preserved, CRLF input is normalized to the configured
/// line ending, and the final newline follows [`Config::final_newline`].
/// Configured Tailwind sorters rewrite `@apply` class lists after printing.
pub fn format(
    path: &Path,
    input: &str,
    config: &Config,
    dialect: Dialect,
) -> Result<String, Diagnostic> {
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
    let dialect = dialect.resolve(config.dialect, path);
    let nesting = nesting_violation(&text, dialect).or_else(|| {
        (dialect == Dialect::Sass)
            .then(|| sass_nesting_violation(&text))
            .flatten()
    });
    if let Some((line, column)) = nesting {
        return Err(Diagnostic::new(
            DiagnosticKind::Syntax,
            format!("nesting deeper than {MAX_NESTING_DEPTH} levels is not supported"),
        )
        .with_location(line, Some(column)));
    }
    let first = format_text(&text, dialect.syntax(), &config.malva_options())
        .map_err(|error| syntax(&text, error))?;
    // Malva has a few non-idempotent pretty-printer corners on malformed Less.
    // An unstable result is worse than no formatting: keep the normalized input.
    let mut output = match format_text(&first, dialect.syntax(), &config.malva_options()) {
        Ok(second) if second == first => first,
        _ => text.clone(),
    };
    if config.tailwind_enabled()
        && let Some(sorter) = &config.tailwind_sorter
    {
        output = sort_apply(&output, sorter, dialect);
    }
    if format_text(&output, dialect.syntax(), &config.malva_options()).is_err() {
        return Err(Diagnostic::new(
            DiagnosticKind::Internal,
            "formatting would produce invalid CSS",
        ));
    }
    output.truncate(output.trim_end_matches('\n').len());
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

fn syntax(text: &str, error: malva::Error) -> Diagnostic {
    let diagnostic = Diagnostic::new(DiagnosticKind::Syntax, error.to_string());
    let malva::Error::Parser(parser_error, _, _) = error;
    let (line, column) = line_column(text, parser_error.span.start);
    diagnostic.with_location(line, Some(column))
}

fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let before = text.get(..offset).unwrap_or(text);
    let line = before.matches('\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .map_or(1, |line| line.encode_utf16().count() + 1);
    (line, column)
}

fn bare_carriage_return(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    bytes.iter().enumerate().find_map(|(index, byte)| {
        (*byte == b'\r' && bytes.get(index + 1) != Some(&b'\n')).then_some(index)
    })
}

fn nesting_violation(text: &str, dialect: Dialect) -> Option<(usize, usize)> {
    let mut cursor = 0;
    let mut depth = 0usize;
    let mut parentheses = 0usize;
    let mut brackets = 0usize;
    while cursor < text.len() {
        let rest = &text[cursor..];
        let character = rest.chars().next().expect("cursor is in bounds");
        if rest.starts_with("/*") {
            cursor = block_comment_end(text, cursor)?;
            continue;
        }
        if rest.starts_with("//") && dialect != Dialect::Css && parentheses == 0 && brackets == 0 {
            cursor = text[cursor..]
                .find('\n')
                .map_or(text.len(), |offset| cursor + offset);
            continue;
        }
        if character == '"' || character == '\'' {
            cursor = string_end(text, cursor);
            continue;
        }
        if character == '\\' {
            cursor += character.len_utf8();
            if let Some(escaped) = text[cursor..].chars().next() {
                cursor += escaped.len_utf8();
            }
            continue;
        }
        match character {
            '{' | '(' | '[' => {
                depth += 1;
                if depth > MAX_NESTING_DEPTH {
                    return Some(line_column(text, cursor));
                }
                match character {
                    '(' => parentheses += 1,
                    '[' => brackets += 1,
                    _ => {}
                }
            }
            '}' | ')' | ']' => {
                depth = depth.saturating_sub(1);
                match character {
                    ')' => parentheses = parentheses.saturating_sub(1),
                    ']' => brackets = brackets.saturating_sub(1),
                    _ => {}
                }
            }
            _ => {}
        }
        cursor += character.len_utf8();
    }
    None
}

fn sass_nesting_violation(text: &str) -> Option<(usize, usize)> {
    let mut levels = vec![0usize];
    let mut in_block_comment = false;
    let mut offset = 0usize;
    for raw_line in text.split('\n') {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
        let content = &line[indent..];
        if in_block_comment {
            if content.contains("*/") {
                in_block_comment = false;
            }
            offset += raw_line.len() + 1;
            continue;
        }
        if content.is_empty() || content.starts_with("//") {
            offset += raw_line.len() + 1;
            continue;
        }
        if content.starts_with("/*") {
            if !content.contains("*/") {
                in_block_comment = true;
            }
            offset += raw_line.len() + 1;
            continue;
        }
        while levels.last().is_some_and(|level| *level > indent) {
            levels.pop();
        }
        if levels.last().is_some_and(|level| *level < indent) {
            levels.push(indent);
            if levels.len() - 1 > MAX_NESTING_DEPTH {
                return Some(line_column(text, offset + indent));
            }
        }
        offset += raw_line.len() + 1;
    }
    None
}
