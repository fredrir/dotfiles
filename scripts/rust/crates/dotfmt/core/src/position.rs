//! UTF-16 line/character positions, matching the editor and LSP default.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

impl Position {
    pub const fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

impl Range {
    pub const fn new(start: Position, end: Position) -> Self {
        Self { start, end }
    }
}

/// Line starts for one immutable snapshot of a document.
#[derive(Clone, Debug, Default)]
pub struct LineIndex {
    line_starts: Vec<usize>,
    len: usize,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut line_starts = vec![0];
        for (offset, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(offset + 1);
            }
        }
        Self {
            line_starts,
            len: text.len(),
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// UTF-16 position of a byte offset.
    pub fn position(&self, text: &str, offset: usize) -> Position {
        let offset = offset.min(self.len);
        let line = self
            .line_starts
            .partition_point(|&start| start <= offset)
            .saturating_sub(1);
        let start = self.line_starts[line];
        let character = text
            .get(start..offset)
            .unwrap_or_default()
            .encode_utf16()
            .count();
        Position::new(line as u32, character as u32)
    }

    /// Byte offset of a UTF-16 position, clamped to the line and its end.
    pub fn offset(&self, text: &str, position: Position) -> usize {
        let Some(&start) = self.line_starts.get(position.line as usize) else {
            return self.len;
        };
        let end = self
            .line_starts
            .get(position.line as usize + 1)
            .copied()
            .unwrap_or(self.len);
        let mut units = 0u32;
        for (offset, character) in text[start..end].char_indices() {
            if units >= position.character {
                return start + offset;
            }
            units += character.len_utf16() as u32;
            if units > position.character {
                return start + offset;
            }
        }
        if end > start && text.as_bytes().get(end - 1) == Some(&b'\n') {
            let end = end - 1;
            if end > start && text.as_bytes().get(end - 1) == Some(&b'\r') {
                end - 1
            } else {
                end
            }
        } else {
            end
        }
    }

    pub fn range(&self, text: &str, start: usize, end: usize) -> Range {
        Range::new(self.position(text, start), self.position(text, end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_count_utf16_units_and_clamp_to_lines() {
        let text = "a😀b\nsecond\r\nthird";
        let index = LineIndex::new(text);
        assert_eq!(index.position(text, 0), Position::new(0, 0));
        assert_eq!(index.position(text, 1), Position::new(0, 1));
        assert_eq!(index.position(text, 5), Position::new(0, 3));
        assert_eq!(index.position(text, 6), Position::new(0, 4));
        assert_eq!(index.position(text, 7), Position::new(1, 0));
        assert_eq!(index.offset(text, Position::new(0, 3)), 5);
        assert_eq!(index.offset(text, Position::new(1, 99)), 13);
        assert_eq!(index.offset(text, Position::new(2, 2)), 17);
        assert_eq!(index.offset(text, Position::new(99, 0)), text.len());
    }

    #[test]
    fn positions_round_trip_on_ascii() {
        let text = "abc\ndef\nghi";
        let index = LineIndex::new(text);
        for offset in 0..=text.len() {
            let position = index.position(text, offset);
            assert_eq!(index.offset(text, position), offset);
        }
    }
}
