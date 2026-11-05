//! In-memory text documents, updated incrementally over UTF-16 positions.
//!
//! The handler pattern follows `crates/dot-code/server/src/documents.rs`: one
//! immutable `LineIndex` per snapshot, rebuilt after each change, so a position
//! can never be resolved against a different revision of the text.

use std::collections::HashMap;
use std::ops::Range;

use line_index::{LineCol, LineIndex, TextSize, WideEncoding, WideLineCol};
use lsp_types::{Position, TextDocumentContentChangeEvent, Uri};

#[derive(Default)]
pub struct Documents(HashMap<Uri, Document>);

impl Documents {
    pub fn open(&mut self, uri: Uri, document: Document) {
        self.0.insert(uri, document);
    }

    pub fn close(&mut self, uri: &Uri) -> Option<Document> {
        self.0.remove(uri)
    }

    pub fn get(&self, uri: &Uri) -> Option<&Document> {
        self.0.get(uri)
    }

    pub fn get_mut(&mut self, uri: &Uri) -> Option<&mut Document> {
        self.0.get_mut(uri)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Uri, &Document)> {
        self.0.iter()
    }
}

#[derive(Debug)]
pub struct Document {
    text: String,
    language: String,
    pub version: i32,
    index: LineIndex,
    desynchronised: bool,
}

impl Clone for Document {
    fn clone(&self) -> Self {
        Self {
            text: self.text.clone(),
            language: self.language.clone(),
            version: self.version,
            index: LineIndex::new(&self.text),
            desynchronised: self.desynchronised,
        }
    }
}

impl Document {
    pub fn new(text: String, language: String, version: i32) -> Self {
        Self {
            index: LineIndex::new(&text),
            text,
            language,
            version,
            desynchronised: false,
        }
    }

    /// Whether an incremental change could not be applied. The buffer no
    /// longer matches the editor until a full-text change or `didOpen`.
    pub fn is_desynchronised(&self) -> bool {
        self.desynchronised
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn language(&self) -> &str {
        &self.language
    }

    /// Applies an incremental change; a change without a range replaces the text.
    ///
    /// Invalid ranges leave the document unchanged and mark it desynchronised,
    /// which is the only safe answer when an editor sends a position that cannot
    /// be resolved. A full-text change clears the flag.
    pub fn change(&mut self, change: TextDocumentContentChangeEvent) -> bool {
        match change.range {
            None => {
                self.text = change.text;
                self.desynchronised = false;
            }
            Some(_) if self.desynchronised => return false,
            Some(range) => {
                let Some(range) = self.range(range) else {
                    self.desynchronised = true;
                    return false;
                };
                self.text.replace_range(range, &change.text);
            }
        }
        self.index = LineIndex::new(&self.text);
        true
    }

    pub fn offset(&self, position: Position) -> Option<usize> {
        let line = self.index.to_utf8(
            WideEncoding::Utf16,
            WideLineCol {
                line: position.line,
                col: position.character,
            },
        )?;
        let end = usize::from(self.index.line(line.line)?.end());
        let offset = usize::from(self.index.offset(line)?).min(end);
        self.text.is_char_boundary(offset).then_some(offset)
    }

    pub fn range(&self, range: lsp_types::Range) -> Option<Range<usize>> {
        Some(self.offset(range.start)?..self.offset(range.end)?)
    }

    pub fn position(&self, offset: usize) -> Position {
        let offset = offset.min(self.text.len());
        let line = u32::try_from(offset)
            .ok()
            .and_then(|offset| self.index.try_line_col(TextSize::from(offset)))
            .unwrap_or(LineCol { line: 0, col: 0 });
        let wide = self
            .index
            .to_wide(WideEncoding::Utf16, line)
            .unwrap_or(WideLineCol {
                line: line.line,
                col: line.col,
            });
        Position::new(wide.line, wide.col)
    }

    pub fn lsp_range(&self, range: &Range<usize>) -> lsp_types::Range {
        lsp_types::Range::new(self.position(range.start), self.position(range.end))
    }

    pub fn line_range(&self, first: usize, last: usize) -> Range<usize> {
        let start = self.line_start(first);
        let end = self.line_start(last.saturating_add(1));
        start..end.max(start)
    }

    /// The text of one zero-based line, without its line ending.
    pub fn line_text(&self, line: usize) -> &str {
        let start = self.line_start(line);
        let end = self.line_start(line.saturating_add(1)).min(self.text.len());
        self.text[start..end].trim_end_matches(['\r', '\n'])
    }

    pub fn line_start(&self, line: usize) -> usize {
        u32::try_from(line)
            .ok()
            .and_then(|line| self.index.line(line))
            .map_or(self.text.len(), |line| usize::from(line.start()))
    }

    pub fn line_count(&self) -> usize {
        self.text.bytes().filter(|byte| *byte == b'\n').count() + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start: (u32, u32), end: (u32, u32)) -> lsp_types::Range {
        lsp_types::Range::new(Position::new(start.0, start.1), Position::new(end.0, end.1))
    }

    fn change(range: Option<lsp_types::Range>, text: &str) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range,
            range_length: None,
            text: text.to_owned(),
        }
    }

    #[test]
    fn incremental_changes_use_utf16_positions() {
        let mut document = Document::new("a😀b\nsecond\n".into(), "json".into(), 1);
        assert!(document.change(change(Some(range((0, 1), (0, 3))), "X")));
        assert_eq!(document.text(), "aXb\nsecond\n");
        assert!(document.change(change(Some(range((1, 0), (1, 3))), "S")));
        assert_eq!(document.text(), "aXb\nSond\n");
        assert_eq!(document.position(1), Position::new(0, 1));
        assert_eq!(document.position(5), Position::new(1, 1));
    }

    #[test]
    fn invalid_ranges_leave_the_document_alone_and_desynchronise_it() {
        let mut document = Document::new("a😀\n".into(), "json".into(), 1);
        assert!(!document.change(change(Some(range((9, 0), (9, 1))), "x")));
        assert!(!document.change(change(Some(range((0, 2), (0, 3))), "x")));
        assert_eq!(document.text(), "a😀\n");
        assert!(document.is_desynchronised());
        assert!(!document.change(change(Some(range((0, 0), (0, 1))), "x")));
        assert_eq!(document.text(), "a😀\n");
        assert!(document.change(change(None, "recovered\n")));
        assert!(!document.is_desynchronised());
        assert_eq!(document.text(), "recovered\n");
    }

    #[test]
    fn a_change_without_a_range_replaces_the_whole_text() {
        let mut document = Document::new("old".into(), "json".into(), 1);
        assert!(document.change(change(None, "new\n")));
        assert_eq!(document.text(), "new\n");
        assert_eq!(document.version, 1);
    }

    #[test]
    fn line_ranges_are_byte_ranges() {
        let document = Document::new("one\ntwo\nthree".into(), "json".into(), 1);
        assert_eq!(document.line_range(1, 2), 4..13);
        assert_eq!(document.line_text(1), "two");
        assert_eq!(document.line_count(), 3);
    }
}
