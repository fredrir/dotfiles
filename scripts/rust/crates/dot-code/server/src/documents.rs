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

    pub fn close(&mut self, uri: &Uri) {
        self.0.remove(uri);
    }

    pub fn get(&self, uri: &Uri) -> Option<&Document> {
        self.0.get(uri)
    }

    pub fn get_mut(&mut self, uri: &Uri) -> Option<&mut Document> {
        self.0.get_mut(uri)
    }
}

pub struct Document {
    text: String,
    language: String,
    pub version: i32,
    index: LineIndex,
}

impl Document {
    pub fn new(text: String, language: String, version: i32) -> Self {
        Self {
            index: LineIndex::new(&text),
            text,
            language,
            version,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn language(&self) -> &str {
        &self.language
    }

    /// Applies an incremental change; a change without a range replaces the text.
    pub fn change(&mut self, change: TextDocumentContentChangeEvent) {
        match change.range {
            None => self.text = change.text,
            Some(range) => {
                let Some(range) = self.range(range) else {
                    return;
                };
                self.text.replace_range(range, &change.text);
            }
        }
        self.index = LineIndex::new(&self.text);
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
}

/// The UTF-16 position of `offset` in text that has no index yet.
pub fn position_in(text: &str, offset: usize) -> Position {
    let before = &text[..offset];
    let line_start = before.rfind('\n').map_or(0, |index| index + 1);
    Position::new(
        before.matches('\n').count() as u32,
        before[line_start..].encode_utf16().count() as u32,
    )
}
