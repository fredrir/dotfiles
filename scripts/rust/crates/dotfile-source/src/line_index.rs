use crate::decode_utf8;

/// One-based normative source coordinate: line and Unicode-scalar column.
///
/// Column one is the first scalar of a line. Combining scalars and astral
/// scalars each advance the column once. For diagnostics over malformed
/// UTF-8, each invalid byte advances the column once; those pseudo-scalar
/// columns exist only on diagnostics and never enter semantic anchors or a
/// generated lock.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct LineCol {
    pub line: u64,
    pub column: u64,
}

/// Zero-based LSP position encoding negotiated at the protocol boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PositionEncoding {
    /// Character offsets count UTF-8 bytes from the line start.
    Utf8,
    /// Character offsets count UTF-16 code units from the line start.
    Utf16,
}

/// One shared line index built by scanning raw bytes.
///
/// LF ends a line. CRLF is one newline sequence spanning two bytes. Bare CR
/// does not end a line. Invalid UTF-8 does not prevent newline indexing.
/// All coordinate conversions are fallible and derive from this index; no
/// consumer performs independent line scanning (ADR 0002).
#[derive(Clone, Debug)]
pub struct LineIndex {
    /// Byte offset of the first byte of every line. Always starts with 0.
    line_starts: Vec<u64>,
}

impl LineIndex {
    pub fn new(bytes: &[u8]) -> Self {
        let mut line_starts = vec![0];
        for (index, byte) in bytes.iter().enumerate() {
            if *byte == b'\n' {
                line_starts.push(index as u64 + 1);
            }
        }
        Self { line_starts }
    }

    pub fn line_count(&self) -> u64 {
        self.line_starts.len() as u64
    }

    /// The zero-based index of the line containing `offset`, and the byte at
    /// which that line starts. An offset at the LF byte of a line belongs to
    /// that line; the offset immediately after LF belongs to the next line.
    fn line_of(&self, offset: u64) -> (usize, u64) {
        let index = self.line_starts.partition_point(|start| *start <= offset) - 1;
        (index, self.line_starts[index])
    }

    /// Maps a byte offset to its one-based normative coordinate.
    ///
    /// Offsets at the CR and LF bytes of a CRLF sequence map to the preceding
    /// line's exclusive end coordinate. Each valid scalar advances the column
    /// once; each invalid byte advances it once.
    pub fn line_col(&self, bytes: &[u8], offset: u64) -> LineCol {
        let offset = offset.min(bytes.len() as u64);
        let (index, start) = self.line_of(offset);
        let mut column = 1;
        let mut position = start as usize;
        let end = offset as usize;
        while position < end {
            if bytes[position] == b'\r'
                && bytes.get(position + 1) == Some(&b'\n')
                && position + 1 >= end
            {
                // The CR of a line-terminating CRLF is not a content scalar.
                break;
            }
            match decode_utf8(bytes, position) {
                Some((_, width)) => position += width,
                None => position += 1,
            }
            column += 1;
        }
        LineCol {
            line: index as u64 + 1,
            column,
        }
    }

    /// The byte offset of a one-based normative coordinate.
    ///
    /// The reverse conversion of a line's exclusive end coordinate selects
    /// the boundary immediately before a terminating CR. Coordinates beyond
    /// a line's exclusive end are rejected.
    pub fn offset(&self, bytes: &[u8], line: u64, column: u64) -> Option<u64> {
        if line == 0 || column == 0 || line > self.line_count() {
            return None;
        }
        let mut position = self.line_starts[(line - 1) as usize] as usize;
        let mut current = 1;
        loop {
            if current == column {
                return Some(position as u64);
            }
            let byte = *bytes.get(position)?;
            if byte == b'\n' {
                return None;
            }
            if byte == b'\r' && bytes.get(position + 1) == Some(&b'\n') {
                return None;
            }
            match decode_utf8(bytes, position) {
                Some((_, width)) => position += width,
                None => position += 1,
            }
            current += 1;
        }
    }

    /// Zero-based LSP position of a byte offset. UTF-8 character offsets
    /// count bytes from the line start; UTF-16 offsets count code units. An
    /// astral scalar counts as four UTF-8 bytes and two UTF-16 code units.
    pub fn lsp_position(
        &self,
        bytes: &[u8],
        offset: u64,
        encoding: PositionEncoding,
    ) -> (u64, u64) {
        let offset = offset.min(bytes.len() as u64);
        let (index, start) = self.line_of(offset);
        let end = offset as usize;
        let mut position = start as usize;
        let mut character = 0u64;
        while position < end {
            if bytes[position] == b'\r'
                && bytes.get(position + 1) == Some(&b'\n')
                && position + 1 >= end
            {
                break;
            }
            match decode_utf8(bytes, position) {
                Some((scalar, width)) => {
                    character += match encoding {
                        PositionEncoding::Utf8 => width as u64,
                        PositionEncoding::Utf16 => scalar.len_utf16() as u64,
                    };
                    position += width;
                }
                None => {
                    character += 1;
                    position += 1;
                }
            }
        }
        (index as u64, character)
    }

    /// The byte offset of a zero-based LSP position.
    ///
    /// The position must identify a scalar boundary and must not exceed the
    /// line content; invalid positions are rejected rather than silently
    /// retargeted. A position at a line's exclusive end selects the boundary
    /// immediately before a terminating CR.
    pub fn offset_at_lsp(
        &self,
        bytes: &[u8],
        line: u64,
        character: u64,
        encoding: PositionEncoding,
    ) -> Option<u64> {
        if line >= self.line_count() {
            return None;
        }
        let mut position = self.line_starts[line as usize] as usize;
        let mut current = 0u64;
        loop {
            if current == character {
                return Some(position as u64);
            }
            let byte = *bytes.get(position)?;
            if byte == b'\n' {
                return None;
            }
            if byte == b'\r' && bytes.get(position + 1) == Some(&b'\n') {
                return None;
            }
            let (advance, width) = match decode_utf8(bytes, position) {
                Some((scalar, width)) => (
                    match encoding {
                        PositionEncoding::Utf8 => width as u64,
                        PositionEncoding::Utf16 => scalar.len_utf16() as u64,
                    },
                    width,
                ),
                None => (1, 1),
            };
            current += advance;
            if current > character {
                // The requested character offset lands mid-scalar.
                return None;
            }
            position += width;
        }
    }

    /// Whether `offset` may begin or end a semantic anchor: it must not lie
    /// inside a valid UTF-8 scalar or inside a CRLF sequence.
    pub fn is_anchor_boundary(&self, bytes: &[u8], offset: u64) -> bool {
        let offset = offset as usize;
        if offset > bytes.len() {
            return false;
        }
        if offset > 0 && offset < bytes.len() {
            let before = bytes[offset - 1];
            let at = bytes[offset];
            if before == b'\r' && at == b'\n' {
                return false;
            }
            if at & 0xC0 == 0x80 {
                // A continuation byte can only follow a valid multi-byte
                // leading byte for the offset to be mid-scalar.
                if matches!(before, 0xC2..=0xF4) {
                    return false;
                }
                if offset >= 2 && matches!(bytes[offset - 2], 0xE0..=0xF4) && before & 0xC0 == 0x80
                {
                    return false;
                }
                if offset >= 3
                    && matches!(bytes[offset - 3], 0xF0..=0xF4)
                    && bytes[offset - 2] & 0xC0 == 0x80
                    && before & 0xC0 == 0x80
                {
                    return false;
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(index: &LineIndex, bytes: &[u8], offset: u64) -> (u64, u64) {
        let LineCol { line, column } = index.line_col(bytes, offset);
        (line, column)
    }

    #[test]
    fn empty_input() {
        let index = LineIndex::new(b"");
        assert_eq!(index.line_count(), 1);
        assert_eq!(col(&index, b"", 0), (1, 1));
        assert_eq!(index.offset(b"", 1, 1), Some(0));
        assert_eq!(index.offset(b"", 1, 2), None);
    }

    #[test]
    fn ascii_lines() {
        let bytes = b"abc\nde\n";
        let index = LineIndex::new(bytes);
        assert_eq!(index.line_count(), 3);
        assert_eq!(col(&index, bytes, 0), (1, 1));
        assert_eq!(col(&index, bytes, 2), (1, 3));
        // The LF byte maps to the preceding line's exclusive end.
        assert_eq!(col(&index, bytes, 3), (1, 4));
        assert_eq!(col(&index, bytes, 4), (2, 1));
        assert_eq!(col(&index, bytes, 6), (2, 3));
        assert_eq!(col(&index, bytes, 7), (3, 1));
        assert_eq!(index.offset(bytes, 1, 4), Some(3));
        assert_eq!(index.offset(bytes, 2, 1), Some(4));
        assert_eq!(index.offset(bytes, 1, 5), None);
    }

    #[test]
    fn crlf_is_one_newline() {
        let bytes = b"ab\r\ncd\r\n";
        let index = LineIndex::new(bytes);
        assert_eq!(index.line_count(), 3);
        // Both the CR and LF bytes map to the exclusive end of line one.
        assert_eq!(col(&index, bytes, 2), (1, 3));
        assert_eq!(col(&index, bytes, 3), (1, 3));
        assert_eq!(col(&index, bytes, 4), (2, 1));
        // Reverse conversion selects the boundary immediately before CR.
        assert_eq!(index.offset(bytes, 1, 3), Some(2));
        assert_eq!(index.offset(bytes, 2, 1), Some(4));
        assert!(!index.is_anchor_boundary(bytes, 3));
        assert!(index.is_anchor_boundary(bytes, 2));
        assert!(index.is_anchor_boundary(bytes, 4));
    }

    #[test]
    fn bare_cr_does_not_end_a_line() {
        let bytes = b"a\rb";
        let index = LineIndex::new(bytes);
        assert_eq!(index.line_count(), 1);
        assert_eq!(col(&index, bytes, 1), (1, 2));
        assert_eq!(col(&index, bytes, 2), (1, 3));
        assert_eq!(index.offset(bytes, 1, 3), Some(2));
    }

    #[test]
    fn combining_and_astral_scalars_advance_once() {
        let bytes = "e\u{301}\u{1f600}x".as_bytes();
        let index = LineIndex::new(bytes);
        assert_eq!(col(&index, bytes, 1), (1, 2));
        assert_eq!(col(&index, bytes, 3), (1, 3));
        assert_eq!(col(&index, bytes, 7), (1, 4));
        assert_eq!(col(&index, bytes, 8), (1, 5));
        assert_eq!(index.offset(bytes, 1, 4), Some(7));
        assert!(!index.is_anchor_boundary(bytes, 2));
        assert!(!index.is_anchor_boundary(bytes, 5));
        assert!(index.is_anchor_boundary(bytes, 3));
    }

    #[test]
    fn invalid_utf8_advances_once_per_byte() {
        let bytes = b"a\xff\xfe\nb";
        let index = LineIndex::new(bytes);
        assert_eq!(col(&index, bytes, 1), (1, 2));
        assert_eq!(col(&index, bytes, 2), (1, 3));
        assert_eq!(col(&index, bytes, 3), (1, 4));
        assert_eq!(col(&index, bytes, 4), (2, 1));
    }

    #[test]
    fn bom_is_one_scalar() {
        let bytes = b"\xef\xbb\xbfa\n";
        let index = LineIndex::new(bytes);
        assert_eq!(col(&index, bytes, 0), (1, 1));
        assert_eq!(col(&index, bytes, 3), (1, 2));
        assert_eq!(col(&index, bytes, 4), (1, 3));
        assert_eq!(index.offset(bytes, 1, 2), Some(3));
    }

    #[test]
    fn lsp_positions() {
        let bytes = "a\u{1f600}b\r\nc".as_bytes();
        let index = LineIndex::new(bytes);
        assert_eq!(index.lsp_position(bytes, 0, PositionEncoding::Utf8), (0, 0));
        assert_eq!(index.lsp_position(bytes, 1, PositionEncoding::Utf8), (0, 1));
        assert_eq!(index.lsp_position(bytes, 5, PositionEncoding::Utf8), (0, 5));
        assert_eq!(
            index.lsp_position(bytes, 1, PositionEncoding::Utf16),
            (0, 1)
        );
        assert_eq!(
            index.lsp_position(bytes, 5, PositionEncoding::Utf16),
            (0, 3)
        );
        // The CR/LF bytes map to the line's exclusive end.
        assert_eq!(index.lsp_position(bytes, 6, PositionEncoding::Utf8), (0, 6));
        assert_eq!(index.lsp_position(bytes, 7, PositionEncoding::Utf8), (0, 6));
        assert_eq!(index.lsp_position(bytes, 8, PositionEncoding::Utf8), (1, 0));

        assert_eq!(
            index.offset_at_lsp(bytes, 0, 5, PositionEncoding::Utf8),
            Some(5)
        );
        assert_eq!(
            index.offset_at_lsp(bytes, 0, 3, PositionEncoding::Utf16),
            Some(5)
        );
        // Mid-scalar and mid-CRLF positions are rejected.
        assert_eq!(
            index.offset_at_lsp(bytes, 0, 2, PositionEncoding::Utf8),
            None
        );
        assert_eq!(
            index.offset_at_lsp(bytes, 0, 2, PositionEncoding::Utf16),
            None
        );
        // Beyond the line content.
        assert_eq!(
            index.offset_at_lsp(bytes, 0, 7, PositionEncoding::Utf8),
            None
        );
        // The exclusive end selects the boundary before CR.
        assert_eq!(
            index.offset_at_lsp(bytes, 0, 6, PositionEncoding::Utf8),
            Some(6)
        );
    }

    #[test]
    fn round_trips_every_scalar_boundary() {
        let bytes = "a\u{301}\n\u{1f600}x\r\nz\u{7f}".as_bytes();
        let index = LineIndex::new(bytes);
        for offset in 0..=bytes.len() as u64 {
            if !index.is_anchor_boundary(bytes, offset) {
                continue;
            }
            let LineCol { line, column } = index.line_col(bytes, offset);
            assert_eq!(
                index.offset(bytes, line, column),
                Some(offset),
                "offset {offset}"
            );
            for encoding in [PositionEncoding::Utf8, PositionEncoding::Utf16] {
                let (lsp_line, character) = index.lsp_position(bytes, offset, encoding);
                assert_eq!(
                    index.offset_at_lsp(bytes, lsp_line, character, encoding),
                    Some(offset),
                    "offset {offset} ({encoding:?})"
                );
            }
        }
    }
}
