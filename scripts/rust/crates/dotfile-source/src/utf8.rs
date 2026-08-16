/// Decodes one UTF-8 scalar starting at `bytes[position]`.
///
/// Returns `Some((scalar, width))` for a valid encoding and `None` for any
/// malformed sequence: invalid leading or continuation bytes, overlong
/// forms, surrogates, values above U+10FFFF, and truncated sequences. The
/// lexer and line index treat each undecodable byte as one invalid unit so
/// error ranges and pseudo-scalar columns stay exact.
pub fn decode_utf8(bytes: &[u8], position: usize) -> Option<(char, usize)> {
    let first = *bytes.get(position)?;
    if first < 0x80 {
        return Some((first as char, 1));
    }
    let (width, mut value) = match first {
        0xC2..=0xDF => (2, u32::from(first & 0x1F)),
        0xE0..=0xEF => (3, u32::from(first & 0x0F)),
        0xF0..=0xF4 => (4, u32::from(first & 0x07)),
        _ => return None,
    };
    for index in 1..width {
        let continuation = *bytes.get(position + index)?;
        if continuation & 0xC0 != 0x80 {
            return None;
        }
        value = (value << 6) | u32::from(continuation & 0x3F);
    }
    // Reject overlong encodings; the range and surrogate checks are covered
    // by char::from_u32 together with the leading-byte ranges above.
    let minimum = match width {
        2 => 0x80,
        3 => 0x800,
        _ => 0x10000,
    };
    if value < minimum {
        return None;
    }
    char::from_u32(value).map(|scalar| (scalar, width))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_valid_scalars() {
        assert_eq!(decode_utf8(b"a", 0), Some(('a', 1)));
        assert_eq!(decode_utf8("\u{7f}".as_bytes(), 0), Some(('\u{7f}', 1)));
        assert_eq!(decode_utf8("\u{80}".as_bytes(), 0), Some(('\u{80}', 2)));
        assert_eq!(decode_utf8("\u{7ff}".as_bytes(), 0), Some(('\u{7ff}', 2)));
        assert_eq!(decode_utf8("\u{800}".as_bytes(), 0), Some(('\u{800}', 3)));
        assert_eq!(decode_utf8("\u{ffff}".as_bytes(), 0), Some(('\u{ffff}', 3)));
        assert_eq!(
            decode_utf8("\u{10000}".as_bytes(), 0),
            Some(('\u{10000}', 4))
        );
        assert_eq!(
            decode_utf8("\u{10ffff}".as_bytes(), 0),
            Some(('\u{10ffff}', 4))
        );
    }

    #[test]
    fn rejects_malformed_sequences() {
        // Lone continuation byte.
        assert_eq!(decode_utf8(&[0x80], 0), None);
        // Invalid leading byte.
        assert_eq!(decode_utf8(&[0xC0, 0x80], 0), None);
        assert_eq!(decode_utf8(&[0xC1, 0x80], 0), None);
        // Overlong encodings.
        assert_eq!(decode_utf8(&[0xE0, 0x80, 0x80], 0), None);
        assert_eq!(decode_utf8(&[0xF0, 0x80, 0x80, 0x80], 0), None);
        // Surrogates.
        assert_eq!(decode_utf8(&[0xED, 0xA0, 0x80], 0), None);
        assert_eq!(decode_utf8(&[0xED, 0xBF, 0xBF], 0), None);
        // Above U+10FFFF.
        assert_eq!(decode_utf8(&[0xF4, 0x90, 0x80, 0x80], 0), None);
        assert_eq!(decode_utf8(&[0xF5, 0x80, 0x80, 0x80], 0), None);
        // Truncated and broken sequences.
        assert_eq!(decode_utf8(&[0xC2], 0), None);
        assert_eq!(decode_utf8(&[0xC2, b'a'], 0), None);
        assert_eq!(decode_utf8(&[0xE1, 0x80], 0), None);
    }
}
