//! Significant tokens, trivia gaps, and string/path side tables.

use dotfile_source::{ByteRange, Diagnostic, LineIndex};

/// The significant token kinds of the generic grammar. Horizontal whitespace
/// and comments are not tokens; they live in the trivia gaps between tokens.
/// `NL` is significant. End of input is implicit and never stored.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TokenKind {
    Word,
    String,
    PathRef,
    At,
    Dollar,
    Question,
    AtLet,
    AtExtend,
    Eq,
    LeftBrace,
    RightBrace,
    LeftBracket,
    RightBracket,
    Comma,
    Slash,
    Newline,
    /// A run of bytes that could not be lexed as a valid token. Error tokens
    /// let the parser recover without losing a single byte.
    Error,
}

impl TokenKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Word => "Word",
            Self::String => "String",
            Self::PathRef => "PathRef",
            Self::At => "At",
            Self::Dollar => "Dollar",
            Self::Question => "Question",
            Self::AtLet => "AtLet",
            Self::AtExtend => "AtExtend",
            Self::Eq => "Eq",
            Self::LeftBrace => "LeftBrace",
            Self::RightBrace => "RightBrace",
            Self::LeftBracket => "LeftBracket",
            Self::RightBracket => "RightBracket",
            Self::Comma => "Comma",
            Self::Slash => "Slash",
            Self::Newline => "Newline",
            Self::Error => "Error",
        }
    }
}

/// One significant token: a kind and its raw byte range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub range: ByteRange,
}

/// The raw trivia bytes between two adjacent significant tokens.
///
/// Gaps own horizontal whitespace, comments, the optional BOM preamble, and
/// any invalid bytes the lexer skipped (bare CR, stray controls). There are
/// exactly `tokens + 1` gaps; replaying gaps and token byte slices in order
/// reproduces the original input exactly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Gap {
    pub range: ByteRange,
}

/// One escape inside a string token: its source span (including the
/// backslash) and the scalar it decodes to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EscapeData {
    pub range: ByteRange,
    pub decoded: char,
}

/// One segment of a string token. Literal text is escape-decoded;
/// interpolations record the binding name and both spans.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StringSegment {
    Literal {
        text: String,
        range: ByteRange,
    },
    Interpolation {
        name: String,
        range: ByteRange,
        name_range: ByteRange,
    },
}

/// The side table for one `String` or quoted `PathRef` token: decoded
/// segments, escapes, and whether the closing quote was present.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StringData {
    pub segments: Vec<StringSegment>,
    pub escapes: Vec<EscapeData>,
    pub terminated: bool,
}

impl StringData {
    /// The decoded literal text with interpolations rendered as `${name}`.
    pub fn decoded(&self) -> String {
        let mut text = String::new();
        for segment in &self.segments {
            match segment {
                StringSegment::Literal { text: literal, .. } => text.push_str(literal),
                StringSegment::Interpolation { name, .. } => {
                    text.push_str("${");
                    text.push_str(name);
                    text.push('}');
                }
            }
        }
        text
    }

    pub fn has_interpolation(&self) -> bool {
        self.segments
            .iter()
            .any(|segment| matches!(segment, StringSegment::Interpolation { .. }))
    }
}

/// The complete result of lexing one file: significant tokens, trivia gaps,
/// the string side table indexed parallel to tokens, diagnostics, and the
/// shared line index.
#[derive(Clone, Debug)]
pub struct Lexed {
    pub tokens: Vec<Token>,
    pub gaps: Vec<Gap>,
    pub strings: Vec<Option<StringData>>,
    pub diagnostics: Vec<Diagnostic>,
    pub line_index: LineIndex,
}

impl Lexed {
    /// Structural invariants every lexer run must satisfy.
    pub fn check_invariants(&self, source_len: u64) {
        assert_eq!(self.gaps.len(), self.tokens.len() + 1, "gap count");
        assert_eq!(self.strings.len(), self.tokens.len(), "side table");
        let mut cursor = 0;
        for (index, token) in self.tokens.iter().enumerate() {
            let gap = self.gaps[index].range;
            assert_eq!(
                gap.start(),
                cursor,
                "gap {index} starts at previous token end"
            );
            assert_eq!(gap.end(), token.range.start(), "gap {index} meets token");
            cursor = token.range.end();
            if matches!(token.kind, TokenKind::String | TokenKind::PathRef) {
                // Only string-bearing tokens carry side data.
            } else {
                assert!(
                    self.strings[index].is_none(),
                    "side data on non-string token"
                );
            }
        }
        let tail = self.gaps[self.tokens.len()].range;
        assert_eq!(tail.start(), cursor, "tail gap starts at last token end");
        assert_eq!(tail.end(), source_len, "tail gap reaches end of input");
    }

    /// Replays gaps and token byte slices; equals the original input.
    pub fn replay(&self, source: &[u8]) -> Vec<u8> {
        let mut output = Vec::with_capacity(source.len());
        for (index, token) in self.tokens.iter().enumerate() {
            output.extend_from_slice(slice(source, self.gaps[index].range));
            output.extend_from_slice(slice(source, token.range));
        }
        output.extend_from_slice(slice(source, self.gaps[self.tokens.len()].range));
        output
    }

    /// A deterministic text dump of tokens and gaps for golden fixtures.
    pub fn dump(&self, source: &[u8]) -> String {
        dump_tokens(&self.tokens, &self.gaps, &self.strings, source)
    }
}

/// A deterministic text dump of a token stream and its gaps for golden
/// fixtures.
pub fn dump_tokens(
    tokens: &[Token],
    gaps: &[Gap],
    strings: &[Option<StringData>],
    source: &[u8],
) -> String {
    let mut output = String::new();
    for (index, token) in tokens.iter().enumerate() {
        dump_gap(&mut output, source, gaps[index].range);
        output.push_str(token.kind.name());
        output.push(' ');
        output.push_str(&token.range.to_string());
        output.push(' ');
        output.push_str(&escape_dump(slice(source, token.range)));
        if let Some(data) = &strings[index] {
            output.push_str(" decoded ");
            output.push_str(&escape_dump(data.decoded().as_bytes()));
            if !data.terminated {
                output.push_str(" unterminated");
            }
        }
        output.push('\n');
    }
    dump_gap(&mut output, source, gaps[tokens.len()].range);
    output
}

fn dump_gap(output: &mut String, source: &[u8], range: ByteRange) {
    output.push_str("gap ");
    output.push_str(&range.to_string());
    output.push(' ');
    output.push_str(&escape_dump(slice(source, range)));
    output.push('\n');
}

pub(crate) fn slice(source: &[u8], range: ByteRange) -> &[u8] {
    &source[range.start() as usize..range.end() as usize]
}

/// Escapes control characters and quotes for deterministic dumps; other
/// scalars are emitted directly as UTF-8.
pub fn escape_dump(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() + 2);
    output.push('"');
    let mut position = 0;
    while position < bytes.len() {
        match bytes[position] {
            b'"' => {
                output.push_str("\\\"");
                position += 1;
            }
            b'\\' => {
                output.push_str("\\\\");
                position += 1;
            }
            b'\n' => {
                output.push_str("\\n");
                position += 1;
            }
            b'\r' => {
                output.push_str("\\r");
                position += 1;
            }
            b'\t' => {
                output.push_str("\\t");
                position += 1;
            }
            byte if byte < 0x20 || byte == 0x7f => {
                output.push_str(&format!("\\u{{{:x}}}", byte));
                position += 1;
            }
            _ => match dotfile_source::decode_utf8(bytes, position) {
                Some((scalar, width)) => {
                    output.push(scalar);
                    position += width;
                }
                None => {
                    output.push_str("\\u{fffd}");
                    position += 1;
                }
            },
        }
    }
    output.push('"');
    output
}
