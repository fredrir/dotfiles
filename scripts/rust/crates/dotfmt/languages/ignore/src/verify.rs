use crate::dialect::Dialect;
use crate::parse::{self, Document, Line};
use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};

/// A semantic snapshot of a document. Whitespace that a dialect ignores is
/// excluded; pattern bytes, attribute bytes and line order are compared
/// exactly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fingerprint {
    items: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Item {
    Blank(usize),
    Comment(String),
    Pattern(String),
    Attributes { pattern: String, attrs: Vec<String> },
    Opaque(String),
}

impl Fingerprint {
    /// Fingerprint text after normalizing CRLF. Used by the guard and by
    /// tests that need to prove which edits are semantic.
    pub fn of(input: &str, dialect: Dialect, blank_lines: usize) -> Self {
        let input = input.strip_prefix('\u{feff}').unwrap_or(input);
        let text = input.replace("\r\n", "\n");
        Self::of_document(&parse::parse(&text, dialect), blank_lines)
    }

    pub(crate) fn of_document(document: &Document, blank_lines: usize) -> Self {
        let mut items = Vec::new();
        let mut pending_blanks = 0usize;
        let mut started = false;
        for line in &document.lines {
            match line {
                Line::Blank => pending_blanks += 1,
                Line::Comment { body, .. } => {
                    begin_item(&mut items, &mut pending_blanks, &mut started, blank_lines);
                    items.push(Item::Comment(comment_key(body)));
                }
                Line::Pattern { body, .. } => {
                    begin_item(&mut items, &mut pending_blanks, &mut started, blank_lines);
                    items.push(Item::Pattern((*body).to_owned()));
                }
                Line::Attributes { pattern, attrs, .. } => {
                    begin_item(&mut items, &mut pending_blanks, &mut started, blank_lines);
                    items.push(Item::Attributes {
                        pattern: (*pattern).to_owned(),
                        attrs: attrs
                            .iter()
                            .map(|attribute| (*attribute).to_owned())
                            .collect(),
                    });
                }
                Line::Opaque(line) => {
                    begin_item(&mut items, &mut pending_blanks, &mut started, blank_lines);
                    items.push(Item::Opaque((*line).to_owned()));
                }
            }
        }
        // Leading and trailing blank lines are layout, not content: they may
        // be added or removed by the printer. Internal runs are compared after
        // clamping to the configured maximum.
        Fingerprint { items }
    }
}

fn begin_item(
    items: &mut Vec<Item>,
    pending_blanks: &mut usize,
    started: &mut bool,
    blank_lines: usize,
) {
    if *started {
        let count = (*pending_blanks).min(blank_lines);
        if count > 0 {
            items.push(Item::Blank(count));
        }
    }
    *pending_blanks = 0;
    *started = true;
}

fn comment_key(body: &str) -> String {
    let rest = body.strip_prefix('#').unwrap_or(body);
    format!("#{}", rest.trim_matches([' ', '\t']))
}

/// Reject a layout that would change what the file matches.
pub fn check(expected: &Fingerprint, actual: &Fingerprint) -> Result<(), Diagnostic> {
    if expected == actual {
        return Ok(());
    }
    Err(Diagnostic::new(
        DiagnosticKind::Internal,
        "internal error: formatting would change what the file matches, so nothing was written",
    ))
}
