use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};
use oxc_diagnostics::OxcDiagnostic;

/// One-based line and UTF-16 column for a byte offset.
pub(crate) fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let before = text.get(..offset).unwrap_or(text);
    let line = before.matches('\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .map_or(1, |line| line.encode_utf16().count() + 1);
    (line, column)
}

/// Converts an oxc parser diagnostic into the formatter's fatal diagnostic.
pub(crate) fn syntax_diagnostic(text: &str, error: &OxcDiagnostic) -> Diagnostic {
    let diagnostic = Diagnostic::new(DiagnosticKind::Syntax, error.to_string());
    let offset = error
        .labels
        .as_ref()
        .first()
        .map(|label| label.offset() as usize);
    match offset {
        Some(offset) => {
            let (line, column) = line_column(text, offset);
            diagnostic.with_location(line, Some(column))
        }
        None => diagnostic,
    }
}

/// A conservative guard against parser/formatter stack exhaustion.
///
/// oxc is recursive; a few adversarial inputs (30k nested JSX or 100k parens)
/// overflow even the 32 MiB worker stack. This scanner ignores strings and
/// comments and counts both bracket and JSX-ish angle nesting. It may reject
/// pathological generic/angle expressions, which is preferable to aborting the
/// process.
pub(crate) fn nesting_exceeded(text: &str) -> bool {
    const LIMIT: usize = 8192;
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut angles = 0usize;
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            index += 2;
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            index += 2;
            while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/') {
                index += 1;
            }
            index = (index + 2).min(bytes.len());
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            let quote = byte;
            index += 1;
            while index < bytes.len() {
                if bytes[index] == b'\\' {
                    index += 2;
                    continue;
                }
                if bytes[index] == quote {
                    index += 1;
                    break;
                }
                index += 1;
            }
            continue;
        }
        if byte == b'`' {
            index += 1;
            while index < bytes.len() {
                if bytes[index] == b'\\' {
                    index += 2;
                    continue;
                }
                if bytes[index] == b'`' {
                    index += 1;
                    break;
                }
                index += 1;
            }
            continue;
        }
        match byte {
            b'(' | b'[' | b'{' => {
                depth += 1;
                if depth > LIMIT {
                    return true;
                }
            }
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b'<' if bytes
                .get(index + 1)
                .is_some_and(|next| next.is_ascii_alphabetic() || matches!(next, b'/' | b'>')) =>
            {
                if bytes.get(index + 1) == Some(&b'/') {
                    angles = angles.saturating_sub(1);
                } else {
                    angles += 1;
                    if angles > LIMIT {
                        return true;
                    }
                }
            }
            _ => {}
        }
        index += 1;
    }
    false
}
