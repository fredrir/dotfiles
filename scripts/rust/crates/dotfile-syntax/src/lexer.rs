//! The hand-written byte lexer.
//!
//! The comment boundary, compound keywords, path precedence, sigil
//! adjacency, interpolation, and strict invalid-byte behavior are part of
//! language conformance, so the lexer operates directly on bytes and reports
//! exact ranges. It emits significant tokens plus `tokens + 1` trivia gaps;
//! replaying both reproduces the input exactly, including invalid regions.

use dotfile_source::{
    ByteRange, Diagnostic, DiagnosticSink, LineIndex, RepoPath, Severity, SourceText, Stage,
    decode_utf8,
};

use crate::token::{EscapeData, Gap, Lexed, StringData, StringSegment, Token, TokenKind};

const BOM: &[u8] = b"\xef\xbb\xbf";

/// Lexes one file into tokens, gaps, side tables, and diagnostics.
pub fn lex(path: &RepoPath, source: &SourceText) -> Lexed {
    let line_index = LineIndex::new(source.as_bytes());
    let mut sink = DiagnosticSink::new(path, source, &line_index);
    let (tokens, gaps, strings) = lex_into(source, &mut sink);
    Lexed {
        tokens,
        gaps,
        strings,
        diagnostics: sink.finish(),
        line_index,
    }
}

/// Lexes into an existing diagnostic sink so the parser shares the retained
/// diagnostic limit. Returns the token stream without diagnostics.
pub(crate) fn lex_into<'s>(
    source: &'s SourceText,
    sink: &mut DiagnosticSink<'s>,
) -> (Vec<Token>, Vec<Gap>, Vec<Option<StringData>>) {
    let mut lexer = Lexer {
        bytes: source.as_bytes(),
        pos: 0,
        gap_start: 0,
        tokens: Vec::new(),
        gaps: Vec::new(),
        strings: Vec::new(),
        line_has_code: false,
        sink,
    };
    lexer.run();
    (lexer.tokens, lexer.gaps, lexer.strings)
}

struct Lexer<'a, 'b> {
    bytes: &'b [u8],
    pos: usize,
    gap_start: usize,
    tokens: Vec<Token>,
    gaps: Vec<Gap>,
    strings: Vec<Option<StringData>>,
    /// Whether a significant token was already emitted on the current line.
    /// A `#` starts a comment when no code precedes it on the line or when
    /// it immediately follows horizontal whitespace.
    line_has_code: bool,
    sink: &'a mut DiagnosticSink<'b>,
}

impl Lexer<'_, '_> {
    fn run(&mut self) {
        if self.bytes.starts_with(BOM) {
            // One leading BOM is preamble trivia owned by the first gap.
            self.pos = 3;
        }
        loop {
            self.consume_trivia();
            if self.pos >= self.bytes.len() {
                break;
            }
            if self.lex_token() {
                continue;
            }
        }
        self.push_gap(self.bytes.len());
    }

    /// Consumes horizontal whitespace and comments into the current gap.
    /// Returns without consuming anything that could start a token.
    fn consume_trivia(&mut self) {
        loop {
            while self.pos < self.bytes.len() && matches!(self.bytes[self.pos], b' ' | b'\t') {
                self.pos += 1;
            }
            if self.pos < self.bytes.len() && self.bytes[self.pos] == b'#' && self.comment_starts()
            {
                let start = self.pos;
                while self.pos < self.bytes.len() && self.bytes[self.pos] != b'\n' {
                    self.pos += 1;
                }
                self.validate_utf8(start, self.pos);
                continue;
            }
            break;
        }
    }

    fn comment_starts(&self) -> bool {
        !self.line_has_code || self.pos == 0 || matches!(self.bytes[self.pos - 1], b' ' | b'\t')
    }

    /// Lexes one significant token at `self.pos`. Returns `false` when the
    /// byte was diagnosed and skipped into the surrounding gap instead of
    /// producing a token (bare CR, stray control, NUL).
    fn lex_token(&mut self) -> bool {
        let start = self.pos;
        let byte = self.bytes[self.pos];
        match byte {
            b'\n' => {
                self.pos += 1;
                self.emit(TokenKind::Newline, start, self.pos);
                self.line_has_code = false;
            }
            b'\r' => {
                if self.bytes.get(self.pos + 1) == Some(&b'\n') {
                    self.pos += 2;
                    self.emit(TokenKind::Newline, start, self.pos);
                    self.line_has_code = false;
                } else {
                    self.encoding_error(
                        start,
                        start + 1,
                        "bare carriage return",
                        "use LF or CRLF line endings",
                    );
                    self.pos += 1;
                    return false;
                }
            }
            b'{' => self.single(TokenKind::LeftBrace),
            b'}' => self.single(TokenKind::RightBrace),
            b'[' => self.single(TokenKind::LeftBracket),
            b']' => self.single(TokenKind::RightBracket),
            b'=' => self.single(TokenKind::Eq),
            b',' => self.single(TokenKind::Comma),
            b'/' => self.single(TokenKind::Slash),
            b'"' => {
                let data = self.lex_string(start);
                self.emit_string(TokenKind::String, start, self.pos, data);
            }
            b'@' => self.lex_at(),
            b'$' => {
                self.pos += 1;
                self.emit(TokenKind::Dollar, start, self.pos);
                if !self.at_word_start() {
                    self.sigil_error(start, self.pos, "$", "a binding name");
                }
            }
            b'?' => {
                self.pos += 1;
                self.emit(TokenKind::Question, start, self.pos);
                let adjacent = self.pos < self.bytes.len()
                    && (self.bytes[self.pos] == b'@' || is_word_start(self.bytes[self.pos]));
                if !adjacent {
                    self.sigil_error(start, self.pos, "?", "@, a name, or a path");
                }
            }
            b'.' => {
                if self.bytes.get(self.pos + 1) == Some(&b'/') {
                    self.lex_pathref(start);
                } else {
                    self.lex_word(start);
                }
            }
            b'#' => {
                // A `#` that is not the first non-whitespace character on
                // the line and does not follow horizontal whitespace is a
                // lexical error, not a comment.
                while self.pos < self.bytes.len() && self.bytes[self.pos] != b'\n' {
                    self.pos += 1;
                }
                self.token_error(
                    start,
                    self.pos,
                    "`#` does not start a comment here",
                    "start the line with `#` or put horizontal whitespace before it",
                );
                return false;
            }
            _ if is_word_start(byte) => self.lex_word(start),
            0x00..=0x1f => {
                self.encoding_error(
                    start,
                    start + 1,
                    "literal control character outside a string",
                    "remove the control character",
                );
                self.pos += 1;
                return false;
            }
            b'+' | b'-' => {
                // Word-continuation bytes cannot start a token.
                self.pos += 1;
                while self.pos < self.bytes.len() && is_id_cont(self.bytes[self.pos]) {
                    self.pos += 1;
                }
                self.emit(TokenKind::Error, start, self.pos);
                self.token_error(
                    start,
                    self.pos,
                    "a word cannot start with `+` or `-`",
                    "start the name with a letter, digit, `.`, or `_`",
                );
            }
            _ => match decode_utf8(self.bytes, self.pos) {
                Some((scalar, width)) => {
                    if scalar == '\u{feff}' {
                        self.encoding_error(
                            start,
                            start + width,
                            "misplaced byte order mark",
                            "remove the BOM; it is only accepted at byte offset zero",
                        );
                    } else if is_c1(scalar) {
                        self.encoding_error(
                            start,
                            start + width,
                            "literal C1 control character outside a string",
                            "remove the control character",
                        );
                    } else if width == 1 {
                        self.token_error(
                            start,
                            start + width,
                            "unexpected character outside a string or comment",
                            "quote the value or remove the character",
                        );
                    } else {
                        self.token_error(
                            start,
                            start + width,
                            "non-ASCII character outside a string or comment",
                            "quote the value or remove the character",
                        );
                    }
                    self.pos += width;
                    return false;
                }
                None => {
                    self.encoding_error(
                        start,
                        start + 1,
                        "invalid UTF-8",
                        "input must be strict UTF-8",
                    );
                    self.pos += 1;
                    return false;
                }
            },
        }
        true
    }

    fn single(&mut self, kind: TokenKind) {
        let start = self.pos;
        self.pos += 1;
        self.emit(kind, start, self.pos);
    }

    fn lex_word(&mut self, start: usize) {
        self.pos += 1;
        while self.pos < self.bytes.len() && is_id_cont(self.bytes[self.pos]) {
            self.pos += 1;
        }
        self.emit(TokenKind::Word, start, self.pos);
    }

    /// Lexes `@`, the compound keywords `@let`/`@extend`, and attributes or
    /// sigil blocks. A compound keyword requires exact spelling, no space
    /// after `@`, and at least one horizontal space after the keyword; the
    /// following word is then required by the parser.
    fn lex_at(&mut self) {
        let start = self.pos;
        self.pos += 1;
        let word_start = self.pos;
        if self.pos < self.bytes.len() && is_word_start(self.bytes[self.pos]) {
            let mut end = self.pos + 1;
            while end < self.bytes.len() && is_id_cont(self.bytes[end]) {
                end += 1;
            }
            let word = &self.bytes[word_start..end];
            let followed_by_space =
                end < self.bytes.len() && matches!(self.bytes[end], b' ' | b'\t');
            if followed_by_space && word == b"let" {
                self.pos = end;
                self.emit(TokenKind::AtLet, start, end);
                return;
            }
            if followed_by_space && word == b"extend" {
                self.pos = end;
                self.emit(TokenKind::AtExtend, start, end);
                return;
            }
            self.emit(TokenKind::At, start, word_start);
            return;
        }
        self.emit(TokenKind::At, start, word_start);
        self.sigil_error(start, word_start, "@", "a name");
    }

    /// Lexes a source path reference: the complete bare or quoted path is
    /// one token; `./` is never emitted separately.
    fn lex_pathref(&mut self, start: usize) {
        self.pos += 2; // "./"
        if self.pos < self.bytes.len() && self.bytes[self.pos] == b'"' {
            let string_start = self.pos;
            let data = self.lex_string(string_start);
            if data.has_interpolation() {
                self.token_error(
                    string_start,
                    self.pos,
                    "interpolation is not allowed in a quoted path",
                    "write the path literally",
                );
            }
            let decoded = data.decoded();
            self.validate_path(&decoded, start, self.pos);
            self.emit_string(TokenKind::PathRef, start, self.pos, data);
            return;
        }
        // Bare path: PATH_SAFE segments separated by single slashes.
        if self.pos >= self.bytes.len() || !is_path_safe(self.bytes[self.pos]) {
            self.emit(TokenKind::PathRef, start, self.pos);
            self.token_error(
                start,
                self.pos,
                "incomplete path reference",
                "follow `./` with a path component",
            );
            return;
        }
        loop {
            while self.pos < self.bytes.len() && is_path_safe(self.bytes[self.pos]) {
                self.pos += 1;
            }
            if self.pos < self.bytes.len() && self.bytes[self.pos] == b'/' {
                let slash = self.pos;
                let mut slashes = self.pos;
                while slashes < self.bytes.len() && self.bytes[slashes] == b'/' {
                    slashes += 1;
                }
                let continues = slashes < self.bytes.len() && is_path_safe(self.bytes[slashes]);
                if self.pos + 1 == slashes && continues {
                    self.pos = slashes;
                    continue;
                }
                self.pos = slashes;
                if continues {
                    self.token_error(
                        slash,
                        slashes,
                        "empty path component",
                        "remove the repeated slash",
                    );
                    continue;
                }
                self.token_error(
                    slash,
                    slashes,
                    "path reference has a trailing slash",
                    "remove the trailing slash",
                );
            }
            break;
        }
        let raw = std::str::from_utf8(&self.bytes[start + 2..self.pos]).unwrap_or("");
        // The slash-run checks above already reported empty components and a
        // trailing slash; report `.` and `..` components here.
        for component in raw.split('/') {
            if component == "." || component == ".." {
                self.token_error(
                    start,
                    self.pos,
                    "path reference has a `.` or `..` component",
                    "use plain relative components",
                );
                break;
            }
        }
        self.emit(TokenKind::PathRef, start, self.pos);
    }

    /// Validates a decoded path reference: non-empty, relative, no empty,
    /// `.` or `..` component, no backslash or control character, and no
    /// leading or trailing slash.
    fn validate_path(&mut self, decoded: &str, start: usize, end: usize) {
        let error = |lexer: &mut Self, summary: &'static str| {
            lexer.token_error(start, end, summary, "use a plain relative repository path");
        };
        if decoded.is_empty() {
            error(self, "path reference is empty");
            return;
        }
        if decoded.starts_with('/') {
            error(self, "path reference is absolute");
            return;
        }
        if decoded.ends_with('/') {
            error(self, "path reference has a trailing slash");
            return;
        }
        for component in decoded.split('/') {
            if component.is_empty() {
                error(self, "path reference has an empty component");
                return;
            }
            if component == "." || component == ".." {
                error(self, "path reference has a `.` or `..` component");
                return;
            }
        }
        if decoded
            .chars()
            .any(|scalar| scalar == '\\' || is_forbidden_in_path(scalar))
        {
            error(
                self,
                "path reference contains a backslash or control character",
            );
        }
    }

    /// Lexes one string token. Records decoded segments, escapes, and
    /// interpolation subspans in the side table.
    fn lex_string(&mut self, start: usize) -> StringData {
        self.pos += 1; // opening quote
        let mut segments: Vec<StringSegment> = Vec::new();
        let mut escapes: Vec<EscapeData> = Vec::new();
        let mut literal = String::new();
        let mut literal_start = self.pos;
        // Position just past the last byte that produced literal content.
        // Literal segment ranges are `literal_start..literal_end`, which is
        // always ordered because both cursors only move forward.
        let mut literal_end = self.pos;
        let mut terminated = false;
        macro_rules! flush_literal {
            () => {
                if !literal.is_empty() {
                    segments.push(StringSegment::Literal {
                        text: std::mem::take(&mut literal),
                        range: self.range(literal_start, literal_end),
                    });
                }
            };
        }
        loop {
            if self.pos >= self.bytes.len() {
                self.unterminated(start, self.pos);
                break;
            }
            let byte = self.bytes[self.pos];
            match byte {
                b'"' => {
                    self.pos += 1;
                    terminated = true;
                    break;
                }
                b'\n' | b'\r' => {
                    self.unterminated(start, self.pos);
                    break;
                }
                b'\\' => {
                    let escape_start = self.pos;
                    self.pos += 1;
                    if self.pos >= self.bytes.len() {
                        self.unterminated(start, self.pos);
                        break;
                    }
                    let escape = self.bytes[self.pos];
                    let simple = match escape {
                        b'"' => Some('"'),
                        b'\\' => Some('\\'),
                        b'n' => Some('\n'),
                        b'r' => Some('\r'),
                        b't' => Some('\t'),
                        b'b' => Some('\u{8}'),
                        b'f' => Some('\u{c}'),
                        b'$' => Some('$'),
                        _ => None,
                    };
                    if let Some(decoded) = simple {
                        self.pos += 1;
                        escapes.push(EscapeData {
                            range: self.range(escape_start, self.pos),
                            decoded,
                        });
                        literal.push(decoded);
                        literal_end = self.pos;
                        continue;
                    }
                    if escape == b'u' {
                        let before = literal.len();
                        self.lex_unicode_escape(escape_start, &mut literal, &mut escapes);
                        if literal.len() > before {
                            literal_end = self.pos;
                        }
                        continue;
                    }
                    self.pos += 1;
                    self.token_error(
                        escape_start,
                        self.pos,
                        "unlisted escape sequence",
                        "use one of \\\" \\\\ \\n \\r \\t \\b \\f \\$ \\u{...}",
                    );
                }
                b'$' => {
                    if self.bytes.get(self.pos + 1) == Some(&b'{') {
                        flush_literal!();
                        self.lex_interpolation(&mut segments);
                        literal_start = self.pos;
                    } else {
                        literal.push('$');
                        self.pos += 1;
                        literal_end = self.pos;
                    }
                }
                0x00..=0x1f => {
                    self.token_error(
                        self.pos,
                        self.pos + 1,
                        "literal control character in a string",
                        "use an escape sequence",
                    );
                    self.pos += 1;
                }
                _ => match decode_utf8(self.bytes, self.pos) {
                    Some((scalar, width)) => {
                        if is_c1(scalar) {
                            self.token_error(
                                self.pos,
                                self.pos + width,
                                "literal C1 control character in a string",
                                "use a \\u{...} escape",
                            );
                        } else {
                            literal.push(scalar);
                            literal_end = self.pos;
                        }
                        self.pos += width;
                    }
                    None => {
                        self.encoding_error(
                            self.pos,
                            self.pos + 1,
                            "invalid UTF-8",
                            "input must be strict UTF-8",
                        );
                        self.pos += 1;
                    }
                },
            }
        }
        flush_literal!();
        if segments.is_empty() {
            segments.push(StringSegment::Literal {
                text: String::new(),
                range: self.range(literal_end, literal_end),
            });
        }
        StringData {
            segments,
            escapes,
            terminated,
        }
    }

    /// Lexes `\u{...}` after the backslash and `u` were consumed.
    fn lex_unicode_escape(
        &mut self,
        escape_start: usize,
        literal: &mut String,
        escapes: &mut Vec<EscapeData>,
    ) {
        self.pos += 1; // 'u'
        if self.bytes.get(self.pos) != Some(&b'{') {
            self.pos += 1;
            self.token_error(
                escape_start,
                self.pos,
                "unlisted escape sequence",
                "write Unicode escapes as \\u{...}",
            );
            return;
        }
        self.pos += 1; // '{'
        let digits_start = self.pos;
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_hexdigit() {
            self.pos += 1;
        }
        let digits = self.pos - digits_start;
        let closed = self.bytes.get(self.pos) == Some(&b'}');
        if closed {
            self.pos += 1;
        }
        let range = self.range(escape_start, self.pos);
        if digits == 0 || digits > 6 || !closed {
            self.token_error(
                escape_start,
                self.pos,
                "malformed Unicode escape",
                "use one to six hex digits inside \\u{...}",
            );
            return;
        }
        let text = std::str::from_utf8(&self.bytes[digits_start..digits_start + digits]).unwrap();
        let value = u32::from_str_radix(text, 16).unwrap();
        match char::from_u32(value) {
            Some(decoded) => {
                escapes.push(EscapeData { range, decoded });
                literal.push(decoded);
            }
            None => {
                self.token_error(
                    escape_start,
                    self.pos,
                    "Unicode escape is not a scalar value",
                    "use a value up to U+10FFFF and outside the surrogate range",
                );
            }
        }
    }

    /// Lexes `${binding}` after `$` was seen; `self.pos` is at `$`.
    fn lex_interpolation(&mut self, segments: &mut Vec<StringSegment>) {
        let start = self.pos;
        self.pos += 2; // "${"
        let name_start = self.pos;
        if self.pos < self.bytes.len() && is_binding_start(self.bytes[self.pos]) {
            self.pos += 1;
            while self.pos < self.bytes.len() && is_binding_cont(self.bytes[self.pos]) {
                self.pos += 1;
            }
        }
        let name_end = self.pos;
        let closed = self.bytes.get(self.pos) == Some(&b'}');
        if closed {
            self.pos += 1;
        }
        if name_start == name_end || !closed {
            self.token_error(
                start,
                self.pos,
                "malformed interpolation",
                "write ${binding} with a binding name and a closing brace",
            );
            return;
        }
        let name = std::str::from_utf8(&self.bytes[name_start..name_end])
            .unwrap_or("")
            .to_owned();
        segments.push(StringSegment::Interpolation {
            name,
            range: self.range(start, self.pos),
            name_range: self.range(name_start, name_end),
        });
    }

    fn at_word_start(&self) -> bool {
        self.pos < self.bytes.len() && is_word_start(self.bytes[self.pos])
    }

    /// Validates one comment range as strict UTF-8.
    fn validate_utf8(&mut self, start: usize, end: usize) {
        let mut position = start;
        while position < end {
            match decode_utf8(self.bytes, position) {
                Some((_, width)) => position += width,
                None => {
                    self.encoding_error(
                        position,
                        position + 1,
                        "invalid UTF-8",
                        "input must be strict UTF-8",
                    );
                    position += 1;
                }
            }
        }
    }

    fn range(&self, start: usize, end: usize) -> ByteRange {
        ByteRange::new(start as u64, end as u64, self.bytes.len() as u64).unwrap()
    }

    fn emit(&mut self, kind: TokenKind, start: usize, end: usize) {
        self.push_gap(start);
        self.tokens.push(Token {
            kind,
            range: self.range(start, end),
        });
        self.strings.push(None);
        self.gap_start = end;
        self.line_has_code = true;
    }

    fn emit_string(&mut self, kind: TokenKind, start: usize, end: usize, data: StringData) {
        self.push_gap(start);
        self.tokens.push(Token {
            kind,
            range: self.range(start, end),
        });
        self.strings.push(Some(data));
        self.gap_start = end;
        self.line_has_code = true;
    }

    fn push_gap(&mut self, end: usize) {
        self.gaps.push(Gap {
            range: self.range(self.gap_start, end),
        });
    }

    fn unterminated(&mut self, start: usize, end: usize) {
        self.token_error(
            start,
            end,
            "unterminated string",
            "close the string on the same line",
        );
    }

    fn sigil_error(
        &mut self,
        start: usize,
        end: usize,
        sigil: &'static str,
        subject: &'static str,
    ) {
        let span = self.sink.span(self.range(start, end));
        self.sink.push(
            Diagnostic::new(
                "lex/token",
                Stage::Lex,
                Severity::Error,
                "sigil is not adjacent to its subject",
                "write the sigil immediately before its subject",
                span,
            )
            .with_expected("subject", subject)
            .with_actual("sigil", sigil),
        );
    }

    fn token_error(
        &mut self,
        start: usize,
        end: usize,
        summary: &'static str,
        remedy: &'static str,
    ) {
        let span = self.sink.span(self.range(start, end));
        self.sink.push(Diagnostic::new(
            "lex/token",
            Stage::Lex,
            Severity::Error,
            summary,
            remedy,
            span,
        ));
    }

    fn encoding_error(
        &mut self,
        start: usize,
        end: usize,
        summary: &'static str,
        remedy: &'static str,
    ) {
        let span = self.sink.span(self.range(start, end));
        self.sink.push(Diagnostic::new(
            "lex/encoding",
            Stage::Lex,
            Severity::Error,
            summary,
            remedy,
            span,
        ));
    }
}

pub(crate) fn is_word_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte.is_ascii_digit() || byte == b'.' || byte == b'_'
}

pub(crate) fn is_id_cont(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'+' | b'-')
}

fn is_binding_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_binding_cont(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn is_path_safe(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'+' | b'-' | b'%' | b'@' | b'=')
}

fn is_c1(scalar: char) -> bool {
    ('\u{80}'..='\u{9f}').contains(&scalar)
}

fn is_forbidden_in_path(scalar: char) -> bool {
    scalar.is_control()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex_str(input: &str) -> Lexed {
        let path = RepoPath::new("fixture.dotfile").unwrap();
        lex(&path, &SourceText::from(input))
    }

    fn kinds(lexed: &Lexed) -> Vec<TokenKind> {
        lexed.tokens.iter().map(|token| token.kind).collect()
    }

    #[test]
    fn empty_and_newline_only_files() {
        let lexed = lex_str("");
        assert!(lexed.tokens.is_empty());
        assert_eq!(lexed.gaps.len(), 1);
        lexed.check_invariants(0);

        let lexed = lex_str("\n\n");
        assert_eq!(kinds(&lexed), [TokenKind::Newline, TokenKind::Newline]);
        assert_eq!(lexed.gaps.len(), 3);
        lexed.check_invariants(2);
        assert_eq!(lexed.replay("\n\n".as_bytes()), b"\n\n");
    }

    #[test]
    fn replay_covers_every_byte() {
        let input = " \t# comment\nfoo = \"bar\" # trailing\n@let v = \"${v}\"\n";
        let lexed = lex_str(input);
        lexed.check_invariants(input.len() as u64);
        assert_eq!(lexed.replay(input.as_bytes()), input.as_bytes());
    }

    #[test]
    fn comment_rules() {
        let lexed = lex_str("# whole line\nfoo # trailing\n");
        assert!(lexed.diagnostics.is_empty());

        let lexed = lex_str("foo#bar\n");
        assert_eq!(lexed.diagnostics.len(), 1);
        assert_eq!(lexed.diagnostics[0].code, "lex/token");
        assert_eq!(
            lexed.diagnostics[0].summary,
            "`#` does not start a comment here"
        );
    }

    #[test]
    fn compound_keywords() {
        let lexed = lex_str("@let v = \"x\"\n@extend a/b {}\n@letfoo = 1\n");
        assert_eq!(
            kinds(&lexed),
            [
                TokenKind::AtLet,
                TokenKind::Word,
                TokenKind::Eq,
                TokenKind::String,
                TokenKind::Newline,
                TokenKind::AtExtend,
                TokenKind::Word,
                TokenKind::Slash,
                TokenKind::Word,
                TokenKind::LeftBrace,
                TokenKind::RightBrace,
                TokenKind::Newline,
                TokenKind::At,
                TokenKind::Word,
                TokenKind::Eq,
                TokenKind::Word,
                TokenKind::Newline,
            ]
        );
    }

    #[test]
    fn sigil_adjacency() {
        for (input, count) in [
            ("@ foo", 1),
            ("$ 1", 1),
            ("? foo", 1),
            ("? @font", 1),
            ("@let", 0),
        ] {
            let lexed = lex_str(input);
            let adjacency_errors = lexed
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.summary == "sigil is not adjacent to its subject")
                .count();
            assert_eq!(adjacency_errors, count, "{input:?}");
        }
    }

    #[test]
    fn paths() {
        let lexed = lex_str("./.zshrc\n./\"presets/Mocha Islands/settings.json\"\n");
        assert!(lexed.diagnostics.is_empty());
        assert_eq!(
            kinds(&lexed),
            [
                TokenKind::PathRef,
                TokenKind::Newline,
                TokenKind::PathRef,
                TokenKind::Newline
            ]
        );
        let data = lexed.strings[2].as_ref().unwrap();
        assert_eq!(data.decoded(), "presets/Mocha Islands/settings.json");

        for input in [
            "./",
            "./foo/",
            "./foo//bar",
            "./../x",
            "./.",
            "./\"a/../b\"",
        ] {
            let lexed = lex_str(input);
            assert!(!lexed.diagnostics.is_empty(), "{input:?}");
        }
    }

    #[test]
    fn strings_and_escapes() {
        let lexed = lex_str("\"a\\n\\u{1f600}${v}b\"\n");
        assert!(lexed.diagnostics.is_empty(), "{:?}", lexed.diagnostics);
        let data = lexed.strings[0].as_ref().unwrap();
        assert_eq!(data.decoded(), "a\n\u{1f600}${v}b");
        assert_eq!(data.escapes.len(), 2);

        for input in [
            "\"\\q\"",
            "\"\u{7}\"",
            "\"unterminated",
            "\"${}\"",
            "\"${1x}\"",
        ] {
            let lexed = lex_str(input);
            assert!(!lexed.diagnostics.is_empty(), "{input:?}");
        }
    }

    #[test]
    fn unicode_escape_boundaries() {
        for valid in [
            "\"\\u{0}\"",
            "\"\\u{10FFFF}\"",
            "\"\\u{0abcde}\"",
            "\"\\u{0ABCDE}\"",
        ] {
            assert!(lex_str(valid).diagnostics.is_empty(), "{valid:?}");
        }
        for invalid in [
            "\"\\u{}\"",
            "\"\\u{1234567}\"",
            "\"\\u{D800}\"",
            "\"\\u{DFFF}\"",
            "\"\\u{110000}\"",
            "\"\\u{41\"",
            "\"\\u{abcdef}\"",
            "\"\\u{ABCDEF}\"",
        ] {
            assert!(!lex_str(invalid).diagnostics.is_empty(), "{invalid:?}");
        }
    }

    #[test]
    fn invalid_bytes() {
        let lexed = lex_str("a\x0cb");
        assert_eq!(lexed.diagnostics.len(), 1);
        assert_eq!(lexed.diagnostics[0].code, "lex/encoding");

        let lexed = lex_str("a\rb");
        assert_eq!(lexed.diagnostics.len(), 1);
        assert_eq!(lexed.diagnostics[0].summary, "bare carriage return");

        let mut bytes = b"a".to_vec();
        bytes.push(0xff);
        let path = RepoPath::new("fixture.dotfile").unwrap();
        let lexed = lex(&path, &SourceText::from_bytes(bytes));
        assert_eq!(lexed.diagnostics.len(), 1);
        assert_eq!(lexed.diagnostics[0].summary, "invalid UTF-8");
    }

    #[test]
    fn misplaced_bom() {
        let path = RepoPath::new("fixture.dotfile").unwrap();
        let lexed = lex(
            &path,
            &SourceText::from_bytes(b"\xef\xbb\xbf\xef\xbb\xbfx".to_vec()),
        );
        assert_eq!(lexed.diagnostics.len(), 1);
        assert_eq!(lexed.diagnostics[0].summary, "misplaced byte order mark");
        // The first BOM is preamble trivia owned by the first gap, and the
        // diagnosed misplaced BOM is skipped into the same gap.
        assert_eq!(lexed.gaps[0].range, ByteRange::new(0, 6, 8).unwrap());
    }
}
