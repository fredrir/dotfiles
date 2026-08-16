/// The raw bytes of one source file.
///
/// `SourceText` performs no validation: malformed UTF-8, misplaced BOMs, bare
/// CR, and control characters are lexer inputs with exact ranges, not
/// construction errors. Validation lives in `dotfile-syntax`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceText {
    bytes: Vec<u8>,
}

impl SourceText {
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn len(&self) -> u64 {
        self.bytes.len() as u64
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// The exact byte slice of a range previously checked against this text.
    pub fn slice(&self, range: crate::ByteRange) -> &[u8] {
        &self.bytes[range.start() as usize..range.end() as usize]
    }

    /// The range decoded as UTF-8, when it holds valid UTF-8.
    pub fn slice_str(&self, range: crate::ByteRange) -> Option<&str> {
        std::str::from_utf8(self.slice(range)).ok()
    }
}

impl From<&str> for SourceText {
    fn from(text: &str) -> Self {
        Self::from_bytes(text.as_bytes().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slices_checked_ranges() {
        let text = SourceText::from("abc");
        let range = crate::ByteRange::new(1, 3, 3).unwrap();
        assert_eq!(text.slice(range), b"bc");
    }
}
