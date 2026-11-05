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
