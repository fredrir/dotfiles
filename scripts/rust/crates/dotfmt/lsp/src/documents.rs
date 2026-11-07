
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

    pub fn uri_for_identity(&self, key: &crate::uri::Key) -> Option<Uri> {
        self.0
            .keys()
            .filter(|uri| crate::uri::key(uri) == *key)
            .min_by_key(|uri| uri.as_str())
            .cloned()
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

    pub fn is_desynchronised(&self) -> bool {
        self.desynchronised
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn language(&self) -> &str {
        &self.language
    }

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

    /// Converts a position to an offset, clamping `character` to the end of the line's
    /// content. The line break belongs to no line's characters, so a position past the
    /// end of a line must never consume it.
    pub fn offset(&self, position: Position) -> Option<usize> {
        let line = self.index.line(position.line)?;
        let start = usize::from(line.start());
        let line_text = &self.text[start..usize::from(line.end())];
        let content = line_text.trim_end_matches(['\r', '\n']);
        let wide = self.index.to_wide(
            WideEncoding::Utf16,
            LineCol {
                line: position.line,
                col: u32::try_from(content.len()).ok()?,
            },
        )?;
        let line_col = self.index.to_utf8(
            WideEncoding::Utf16,
            WideLineCol {
                line: position.line,
                col: position.character.min(wide.col),
            },
        )?;
        let offset = start + (line_col.col as usize).min(content.len());
        self.text.is_char_boundary(offset).then_some(offset)
    }

    pub fn range(&self, range: lsp_types::Range) -> Option<Range<usize>> {
        let start = self.offset(range.start)?;
        let end = self.offset(range.end)?;
        (start <= end).then_some(start..end)
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

    fn applied(text: &str, range: lsp_types::Range, replacement: &str) -> Option<String> {
        let mut document = Document::new(text.into(), "json".into(), 1);
        let applied = document.change(change(Some(range), replacement));
        applied.then(|| document.text().to_owned())
    }

    #[test]
    fn past_eol_characters_stop_before_the_line_break() {
        let text = "const a = 1;\nconst b = 2;\n";
        assert_eq!(
            applied(text, range((0, 12), (0, 500)), "!").as_deref(),
            Some("const a = 1;!\nconst b = 2;\n")
        );
        assert_eq!(
            applied(text, range((0, 13), (0, 13)), "!").as_deref(),
            Some("const a = 1;!\nconst b = 2;\n")
        );
        assert_eq!(
            applied(text, range((0, 0), (0, 12)), "const c = 0;").as_deref(),
            Some("const c = 0;\nconst b = 2;\n")
        );
        assert_eq!(
            applied(text, range((0, 0), (0, u32::MAX)), "const c = 0;").as_deref(),
            Some("const c = 0;\nconst b = 2;\n")
        );
    }

    #[test]
    fn past_eol_characters_stop_before_crlf() {
        let text = "const a = 1;\r\nconst b = 2;\r\n";
        assert_eq!(
            applied(text, range((0, 12), (0, 14)), "!").as_deref(),
            Some("const a = 1;!\r\nconst b = 2;\r\n")
        );
        assert_eq!(
            applied(text, range((0, 13), (0, 500)), "!").as_deref(),
            Some("const a = 1;!\r\nconst b = 2;\r\n")
        );
        assert_eq!(
            applied(text, range((0, 0), (0, 500)), "const c = 0;").as_deref(),
            Some("const c = 0;\r\nconst b = 2;\r\n")
        );
    }

    #[test]
    fn a_multiline_range_past_the_end_line_stops_before_its_line_break() {
        let text = "one\ntwo\nthree";
        assert_eq!(
            applied(text, range((0, 0), (1, 500)), "two").as_deref(),
            Some("two\nthree")
        );
        assert_eq!(
            applied(text, range((0, 0), (2, 500)), "all").as_deref(),
            Some("all")
        );
    }

    #[test]
    fn line_ranges_are_byte_ranges() {
        let document = Document::new("one\ntwo\nthree".into(), "json".into(), 1);
        assert_eq!(document.line_range(1, 2), 4..13);
        assert_eq!(document.line_text(1), "two");
        assert_eq!(document.line_count(), 3);
    }
}
