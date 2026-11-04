//! The table-driven engine: one loop that asks the descriptor what to do at the
//! current byte.
//!
//! At each byte the longest matching [`LineComment`] or [`BlockComment`] wins;
//! on a tie the line comment goes first, as SPEC §3 orders them. Then comes the
//! longest matching [`Quote`], otherwise the scanner advances one character.
//! Everything the scanner knows about a language comes from the [`Syntax`] it
//! was handed.
//!
//! `heredocs`, `doc`, `directives`, `regions` and `owns_indent_block` are
//! deliberately not consulted here: heredocs belong to `Engine::Shell`, doc
//! strings to `Engine::Indented`, indent-owned comments to the template engines
//! and regions to `Engine::Markup`/`Engine::Template`. A descriptor that needs
//! them must name one of those engines.

use std::ops::Range;

use crate::edit::line_start;
use crate::scan::{Bail, Out, char_width, identifier_byte, line_end, starts_with};
use crate::syntax::{At, BlockComment, Escape, LineComment, Quote, Syntax, TextGlyphs};

const BOM: &[u8] = b"\xef\xbb\xbf";

/// Longest C++ raw string delimiter tag, in bytes, as `curly.rs` has always
/// allowed: `R"tag(… )tag"`.
const RAW_TAG_LIMIT: usize = 16;

pub fn scan(source: &[u8], syntax: &'static Syntax) -> Result<Out, Bail> {
    let mut scanner = Generic {
        source,
        syntax,
        at: 0,
        out: Out::default(),
    };
    scanner.code()?;
    Ok(scanner.out)
}

struct Generic<'a> {
    source: &'a [u8],
    syntax: &'static Syntax,
    at: usize,
    out: Out,
}

/// A quote that matched: how many bytes open it, and how many `#`s its close is
/// suffixed with.
struct Opening {
    quote: &'static Quote,
    /// Bytes from `at` to the end of the opening delimiter, prefixes included.
    len: usize,
    hashes: usize,
}

impl Generic<'_> {
    fn len(&self) -> usize {
        self.source.len()
    }

    fn byte(&self, at: usize) -> u8 {
        self.source.get(at).copied().unwrap_or(0)
    }

    fn code(&mut self) -> Result<(), Bail> {
        while self.at < self.len() {
            let line = self.matching_line();
            let block = self.matching_block();
            // A line marker can be a proper prefix of a block opener: Nim `#`
            // and `#[`, MATLAB `%` and `%{`, Lua `--` and `--[[`. The longer
            // marker is the comment that actually starts here, so the block
            // wins; equal lengths keep the line comment first.
            if let Some(block) = block
                && line.is_none_or(|line| block.open.len() > line.marker.len())
            {
                self.block_comment(block)?;
                continue;
            }
            if let Some(line) = line {
                self.line_comment(line);
                continue;
            }
            if self.quote()? {
                continue;
            }
            self.at += char_width(self.source, self.at);
        }
        Ok(())
    }

    /// Whether a marker is allowed to start here.
    fn allowed(&self, at: At) -> bool {
        match at {
            At::Anywhere => true,
            At::LineStart => self.source[line_start(self.source, self.at)..self.at]
                .iter()
                .all(|byte| matches!(byte, b' ' | b'\t')),
            At::WordStart => self.at == 0 || !identifier_byte(self.source[self.at - 1]),
            At::StartOfFile => self.at == self.content_start(),
        }
    }

    /// Byte 0, or the first byte after a BOM and an optional shebang line.
    fn content_start(&self) -> usize {
        let mut at = if self.source.starts_with(BOM) {
            BOM.len()
        } else {
            0
        };
        if self.source.get(at) == Some(&b'#') && self.source.get(at + 1) == Some(&b'!') {
            at = past_terminator(self.source, line_end(self.source, at));
        }
        at
    }

    /// The longest line comment marker placed at `self.at`, if any.
    fn matching_line(&self) -> Option<&'static LineComment> {
        let mut best: Option<&'static LineComment> = None;
        for line in self.syntax.line {
            if best.is_some_and(|best| best.marker.len() >= line.marker.len()) {
                continue;
            }
            if !starts_with(self.source, self.at, line.marker.as_bytes()) {
                continue;
            }
            if !self.allowed(line.at) {
                continue;
            }
            best = Some(line);
        }
        best
    }

    fn line_comment(&mut self, line: &'static LineComment) {
        let start = self.at;
        let mut end = line_end(self.source, self.at + line.marker.len());
        while line.continues_on_backslash && end < self.len() {
            let mut back = end;
            if back > start && self.byte(back - 1) == b'\r' {
                back -= 1;
            }
            if back == start || self.byte(back - 1) != b'\\' {
                break;
            }
            end = line_end(self.source, past_terminator(self.source, end));
        }
        self.out.comment(start..end, start + line.marker.len()..end);
        self.at = end;
    }

    /// The longest block comment opener placed at `self.at`, if any.
    fn matching_block(&self) -> Option<&'static BlockComment> {
        let mut best: Option<&'static BlockComment> = None;
        for block in self.syntax.block {
            if best.is_some_and(|best| best.open.len() >= block.open.len()) {
                continue;
            }
            if !starts_with(self.source, self.at, block.open.as_bytes()) {
                continue;
            }
            if !self.allowed(block.at) {
                continue;
            }
            best = Some(block);
        }
        best
    }

    fn block_comment(&mut self, block: &'static BlockComment) -> Result<(), Bail> {
        let start = self.at;
        let mut at = self.at + block.open.len();
        let mut depth = 1usize;
        while at < self.len() {
            if block.nested && starts_with(self.source, at, block.open.as_bytes()) {
                depth += 1;
                at += block.open.len();
                continue;
            }
            if starts_with(self.source, at, block.close.as_bytes()) {
                at += block.close.len();
                depth -= 1;
                if depth == 0 {
                    self.out
                        .comment(start..at, start + block.open.len()..at - block.close.len());
                    self.at = at;
                    return Ok(());
                }
                continue;
            }
            at += 1;
        }
        if block.to_eof_ok {
            self.out
                .comment(start..self.len(), start + block.open.len()..self.len());
            self.at = self.len();
            return Ok(());
        }
        Err(Bail("unterminated block comment"))
    }

    /// The longest quote placed at `self.at`, if any.
    fn matching_quote(&self) -> Option<Opening> {
        let mut best: Option<Opening> = None;
        for quote in self.syntax.quotes {
            let Some((len, hashes)) = self.opening_of(quote) else {
                continue;
            };
            if best.as_ref().is_some_and(|best| best.len >= len) {
                continue;
            }
            best = Some(Opening { quote, len, hashes });
        }
        best
    }

    /// The length of `quote`'s opening at `self.at`, prefixes included, and the
    /// number of `#` bytes that opening consumed.
    fn opening_of(&self, quote: &'static Quote) -> Option<(usize, usize)> {
        let open = quote.open.as_bytes();
        if starts_with(self.source, self.at, open) {
            return Some((open.len(), 0));
        }
        for prefix in quote.prefixes {
            let prefix = prefix.as_bytes();
            if prefix.len() + open.len() > self.len() - self.at {
                continue;
            }
            if !starts_with(self.source, self.at, prefix) {
                continue;
            }
            let mut at = self.at + prefix.len();
            // Rust puts the hashes between `r` and `"`; Swift puts its whole
            // prefix in front of them. Counting the hashes in the prefix as well
            // as the run keeps `r#"` (1) and `##"` (2) both right.
            let mut hashes = prefix.iter().filter(|byte| **byte == b'#').count();
            if quote.hashes {
                while self.byte(at) == b'#' {
                    hashes += 1;
                    at += 1;
                }
            }
            if !starts_with(self.source, at, open) {
                continue;
            }
            let hashes = if quote.hashes { hashes } else { 0 };
            return Some((at + open.len() - self.at, hashes));
        }
        None
    }

    fn quote(&mut self) -> Result<bool, Bail> {
        let Some(opening) = self.matching_quote() else {
            return Ok(false);
        };
        if self.tick(&opening) {
            self.at += opening.len;
            return Ok(true);
        }
        if opening.quote.raw_tag {
            return self.raw_tag(&opening);
        }
        self.delimited(&opening)?;
        Ok(true)
    }

    /// Rust's `'a` lifetime is not a character literal.
    ///
    /// A single-quoted, single-line literal opens only when it is followed by a
    /// backslash or when the character after that character is the closing
    /// quote: `'x'` and `'\n'` are literals, `&'a str` is not. This is the rule
    /// `curly.rs` has always applied to Rust, and the only reading of `'` that
    /// keeps lifetimes out of `Bail("unterminated string")`.
    fn tick(&self, opening: &Opening) -> bool {
        let quote = opening.quote;
        if opening.hashes != 0
            || opening.len != quote.open.len()
            || quote.open != "'"
            || close_of(quote) != "'"
            || quote.multiline
            || quote.escape == Escape::Doubling
        {
            return false;
        }
        let next = self.byte(self.at + 1);
        // Nothing that follows a line terminator can be a one-line literal, so a
        // `'` before one is a tick too. Without this, `'a'\r'b'` reads as a
        // literal spanning the `\r` and the whole file bails.
        if next == 0 || matches!(next, b'\n' | b'\r') {
            return true;
        }
        if next == b'\\' {
            return false;
        }
        let after = self.at + 1 + char_width(self.source, self.at + 1);
        self.byte(after) != b'\''
    }

    fn delimited(&mut self, opening: &Opening) -> Result<(), Bail> {
        let quote = opening.quote;
        let start = self.at + opening.len;
        self.at = start;
        while self.at < self.len() {
            if quote.escape == Escape::Backslash && self.byte(self.at) == b'\\' {
                self.at += 1;
                if self.at < self.len() {
                    self.at += char_width(self.source, self.at);
                }
                continue;
            }
            if let Some(end) = self.close_at(quote, self.at, opening.hashes) {
                if quote.escape == Escape::Doubling
                    && let Some(doubled) = self.close_at(quote, end, opening.hashes)
                {
                    self.at = doubled;
                    continue;
                }
                self.text(quote, start..self.at, opening.hashes);
                self.at = end;
                return Ok(());
            }
            if !quote.multiline && matches!(self.byte(self.at), b'\n' | b'\r') {
                return Err(Bail(unterminated(quote)));
            }
            self.at += char_width(self.source, self.at);
        }
        Err(Bail(unterminated(quote)))
    }

    /// Where the closing delimiter that starts at `at` ends, hashes included.
    fn close_at(&self, quote: &'static Quote, at: usize, hashes: usize) -> Option<usize> {
        let close = close_of(quote).as_bytes();
        if !starts_with(self.source, at, close) {
            return None;
        }
        let end = at + close.len();
        if !(0..hashes).all(|offset| self.byte(end + offset) == b'#') {
            return None;
        }
        Some(end + hashes)
    }

    fn raw_tag(&mut self, opening: &Opening) -> Result<bool, Bail> {
        let source = self.source;
        let open_end = self.at + opening.len;
        let mut tag_end = open_end;
        while tag_end < self.len()
            && tag_end - open_end <= RAW_TAG_LIMIT
            && self.byte(tag_end) != b'('
        {
            tag_end += 1;
        }
        if self.byte(tag_end) != b'(' {
            // `R` was an identifier after all: `curly.rs` falls back to scanning
            // it as one and lets the quote after it open a normal string.
            return Ok(false);
        }
        let tag = &source[open_end..tag_end];
        let start = tag_end + 1;
        let mut cursor = start;
        while cursor < self.len() {
            if self.byte(cursor) == b')'
                && starts_with(source, cursor + 1, tag)
                && self.byte(cursor + tag.len() + 1) == b'"'
            {
                self.text(opening.quote, start..cursor, 0);
                self.at = cursor + tag.len() + 2;
                return Ok(true);
            }
            cursor += 1;
        }
        Err(Bail("unterminated raw string"))
    }

    fn text(&mut self, quote: &'static Quote, body: Range<usize>, hashes: usize) {
        if let Some(glyphs) = quote.text {
            self.out.text(
                body,
                glyphs.escapes,
                glyphs.folds,
                folding_delim(quote, glyphs, hashes),
            );
        }
    }
}

/// The byte a folded typographic quote must not become inside this literal.
///
/// `TextGlyphs::delim` is the descriptor's answer, except for a hash-suffixed
/// literal that carries no hashes: that one closes on the bare delimiter, so
/// folding a typographic quote to the delimiter there would end the string
/// early. Rust `r"..."` is that case; `r#"..."#` closes on `"#`, so folding
/// stays safe and the descriptor's `delim` is kept.
fn folding_delim(quote: &Quote, glyphs: TextGlyphs, hashes: usize) -> u8 {
    if quote.hashes && hashes == 0 {
        close_of(quote).as_bytes()[0]
    } else {
        glyphs.delim
    }
}

/// The bytes that close `quote`; `close: ""` means "same as open".
fn close_of(quote: &Quote) -> &'static str {
    if quote.close.is_empty() {
        quote.open
    } else {
        quote.close
    }
}

/// The reason an unterminated literal reports.
///
/// `Bail` is wire-visible through the LSP, so each form keeps the spelling it
/// has always used instead of collapsing to one message.
fn unterminated(quote: &Quote) -> &'static str {
    if quote.hashes || quote.raw_tag {
        "unterminated raw string"
    } else if quote.open == "`" && quote.text.is_none() {
        "unterminated quoted name"
    } else if quote.open == "'" && !quote.multiline && quote.text.is_none() {
        "unterminated character literal"
    } else if quote.open.len() > 1 {
        "unterminated text block"
    } else {
        "unterminated string"
    }
}

/// The byte after the line terminator `at` sits on: `\r\n` moves two, a lone
/// `\r` or `\n` moves one.
fn past_terminator(source: &[u8], at: usize) -> usize {
    if source.get(at) == Some(&b'\r') && source.get(at + 1) == Some(&b'\n') {
        return at + 2;
    }
    at + 1
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use super::scan;
    use crate::syntax::{
        At, BlockComment, DocStyle, Engine, Escape, Family, LineComment, Quote, Syntax, TextGlyphs,
    };

    fn comments(source: &[u8], syntax: &'static Syntax) -> Vec<(Range<usize>, Range<usize>)> {
        scan(source, syntax)
            .expect("source lexes")
            .comments
            .iter()
            .map(|comment| (comment.span.clone(), comment.body.clone()))
            .collect()
    }

    fn texts(source: &[u8], syntax: &'static Syntax) -> Vec<Range<usize>> {
        scan(source, syntax)
            .expect("source lexes")
            .texts
            .iter()
            .map(|text| text.body.clone())
            .collect()
    }

    const SLASH: LineComment = LineComment {
        marker: "//",
        at: At::Anywhere,
        continues_on_backslash: false,
        doc: false,
        owns_indent_block: false,
    };

    const LINE_ONLY: Syntax = Syntax {
        name: "line-only",
        extensions: &[],
        filenames: &[],
        shebangs: &[],
        family: Family::Curly,
        engine: Engine::Generic,
        line: &[SLASH],
        block: &[],
        quotes: &[],
        heredocs: &[],
        doc: DocStyle::None,
        directives: &[],
        regions: &[],
    };

    #[test]
    fn at_a_marker_is_found_anywhere() {
        let source = b"code // note\n";
        assert_eq!(comments(source, &LINE_ONLY), vec![(5..12, 7..12)]);
    }

    #[test]
    fn at_line_start_matches_only_when_nothing_but_indent_precedes() {
        const LINE_START: Syntax = Syntax {
            name: "line-start",
            extensions: &[],
            filenames: &[],
            shebangs: &[],
            family: Family::Hash,
            engine: Engine::Generic,
            line: &[LineComment {
                marker: "#",
                at: At::LineStart,
                continues_on_backslash: false,
                doc: false,
                owns_indent_block: false,
            }],
            block: &[],
            quotes: &[],
            heredocs: &[],
            doc: DocStyle::None,
            directives: &[],
            regions: &[],
        };
        let source = b"  # one\ncode # two\n";
        assert_eq!(comments(source, &LINE_START), vec![(2..7, 3..7)]);
    }

    #[test]
    fn at_word_start_needs_a_fresh_word() {
        const WORD_START: Syntax = Syntax {
            name: "word-start",
            extensions: &[],
            filenames: &[],
            shebangs: &[],
            family: Family::Hash,
            engine: Engine::Generic,
            line: &[
                LineComment {
                    marker: "-",
                    at: At::WordStart,
                    continues_on_backslash: false,
                    doc: false,
                    owns_indent_block: false,
                },
                LineComment {
                    marker: "--",
                    at: At::WordStart,
                    continues_on_backslash: false,
                    doc: false,
                    owns_indent_block: false,
                },
            ],
            block: &[],
            quotes: &[],
            heredocs: &[],
            doc: DocStyle::None,
            directives: &[],
            regions: &[],
        };
        let source = b"a#b -- note\n";
        assert_eq!(comments(source, &WORD_START), vec![(4..11, 6..11)]);
    }

    const NESTED: Syntax = Syntax {
        name: "nested",
        extensions: &[],
        filenames: &[],
        shebangs: &[],
        family: Family::Curly,
        engine: Engine::Generic,
        line: &[],
        block: &[BlockComment {
            open: "/*",
            close: "*/",
            nested: true,
            at: At::Anywhere,
            important: Some('*'),
            to_eof_ok: false,
        }],
        quotes: &[],
        heredocs: &[],
        doc: DocStyle::None,
        directives: &[],
        regions: &[],
    };

    const FLAT: Syntax = Syntax {
        name: "flat",
        extensions: &[],
        filenames: &[],
        shebangs: &[],
        family: Family::Curly,
        engine: Engine::Generic,
        line: &[],
        block: &[BlockComment {
            open: "/*",
            close: "*/",
            nested: false,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        }],
        quotes: &[],
        heredocs: &[],
        doc: DocStyle::None,
        directives: &[],
        regions: &[],
    };

    #[test]
    fn a_nested_block_runs_to_its_matching_close() {
        let source = b"a/* x /* y */ z */b\n";
        assert_eq!(comments(source, &NESTED), vec![(1..18, 3..16)]);
    }

    #[test]
    fn a_flat_block_stops_at_the_first_close() {
        let source = b"a/* x /* y */ z */b\n";
        assert_eq!(comments(source, &FLAT), vec![(1..13, 3..11)]);
    }

    #[test]
    fn an_unterminated_block_bails() {
        assert_eq!(
            scan(b"/* note\n", &FLAT).err().expect("bails").0,
            "unterminated block comment"
        );
    }

    /// Nim `#` / `#[`, Lua `--` / `--[[`: the line marker is a proper prefix of
    /// the block opener, so the block comment has to win at the shared byte.
    const PREFIXED_BLOCK: Syntax = Syntax {
        name: "prefixed-block",
        extensions: &[],
        filenames: &[],
        shebangs: &[],
        family: Family::Hash,
        engine: Engine::Generic,
        line: &[LineComment {
            marker: "#",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: false,
            owns_indent_block: false,
        }],
        block: &[BlockComment {
            open: "#[",
            close: "]#",
            nested: true,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        }],
        quotes: &[],
        heredocs: &[],
        doc: DocStyle::None,
        directives: &[],
        regions: &[],
    };

    #[test]
    fn a_block_opener_that_extends_a_line_marker_wins() {
        // `a=#[n]#1`: the `#` is not a line comment, so `1` must survive the
        // block span instead of being eaten to end of line.
        assert_eq!(comments(b"a=#[n]#1\n", &PREFIXED_BLOCK), vec![(2..7, 4..5)]);
        assert_eq!(comments(b"# note\n", &PREFIXED_BLOCK), vec![(0..6, 1..6)]);
    }

    #[test]
    fn an_unterminated_prefixed_block_still_bails() {
        assert_eq!(
            scan(b"#[ note\n", &PREFIXED_BLOCK).err().expect("bails").0,
            "unterminated block comment"
        );
    }

    #[test]
    fn a_prefixed_block_that_may_run_to_eof_covers_the_rest() {
        const TO_EOF: Syntax = Syntax {
            block: &[BlockComment {
                open: "#[",
                close: "]#",
                nested: true,
                at: At::Anywhere,
                important: None,
                to_eof_ok: true,
            }],
            ..PREFIXED_BLOCK
        };
        let source = b"#[ note\nstill note\n";
        assert_eq!(comments(source, &TO_EOF), vec![(0..19, 2..19)]);
    }

    /// MATLAB `%` / `%{`: the block opener is confined to a line start, but the
    /// line marker is allowed anywhere, so it must not eat the opener line and
    /// leave the body behind.
    const LINE_START_PREFIX: Syntax = Syntax {
        name: "line-start-prefix",
        extensions: &[],
        filenames: &[],
        shebangs: &[],
        family: Family::Dash,
        engine: Engine::Generic,
        line: &[LineComment {
            marker: "%",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: false,
            owns_indent_block: false,
        }],
        block: &[BlockComment {
            open: "%{",
            close: "%}",
            nested: false,
            at: At::LineStart,
            important: None,
            to_eof_ok: false,
        }],
        quotes: &[],
        heredocs: &[],
        doc: DocStyle::None,
        directives: &[],
        regions: &[],
    };

    #[test]
    fn a_line_start_block_opener_beats_the_line_marker_on_its_line() {
        let source = b"%{\nnote\n%}\nx = 1\n";
        assert_eq!(comments(source, &LINE_START_PREFIX), vec![(0..10, 2..8)]);
    }

    #[test]
    fn the_longer_line_marker_still_beats_a_shorter_block_opener() {
        const LONG_LINE: Syntax = Syntax {
            line: &[LineComment {
                marker: "##",
                at: At::Anywhere,
                continues_on_backslash: false,
                doc: true,
                owns_indent_block: false,
            }],
            block: &[BlockComment {
                open: "#",
                close: "!",
                nested: false,
                at: At::Anywhere,
                important: None,
                to_eof_ok: false,
            }],
            ..PREFIXED_BLOCK
        };
        // At `##x!` both markers start; `##` (2 bytes) beats `#` (1 byte), so
        // the `!` stays code instead of closing a block.
        assert_eq!(comments(b"##x!\n", &LONG_LINE), vec![(0..4, 2..4)]);
    }

    #[test]
    fn a_block_that_may_run_to_eof_does() {
        const TO_EOF: Syntax = Syntax {
            name: "to-eof",
            extensions: &[],
            filenames: &[],
            shebangs: &[],
            family: Family::Css,
            engine: Engine::Generic,
            line: &[],
            block: &[BlockComment {
                open: "/*",
                close: "*/",
                nested: false,
                at: At::Anywhere,
                important: None,
                to_eof_ok: true,
            }],
            quotes: &[],
            heredocs: &[],
            doc: DocStyle::None,
            directives: &[],
            regions: &[],
        };
        let source = b"/* note\nstill note\n";
        assert_eq!(comments(source, &TO_EOF), vec![(0..19, 2..19)]);
    }

    const CONTINUED: Syntax = Syntax {
        name: "continued",
        extensions: &[],
        filenames: &[],
        shebangs: &[],
        family: Family::Curly,
        engine: Engine::Generic,
        line: &[LineComment {
            marker: "//",
            at: At::Anywhere,
            continues_on_backslash: true,
            doc: false,
            owns_indent_block: false,
        }],
        block: &[],
        quotes: &[],
        heredocs: &[],
        doc: DocStyle::None,
        directives: &[],
        regions: &[],
    };

    #[test]
    fn a_trailing_backslash_continues_the_line_comment() {
        let source = b"// one \\\ntwo\ncode\n";
        assert_eq!(comments(source, &CONTINUED), vec![(0..12, 2..12)]);
    }

    #[test]
    fn a_continued_line_comment_survives_crlf() {
        let source = b"// one \\\r\ntwo\r\ncode\r\n";
        let found = comments(source, &CONTINUED);
        assert_eq!(found, vec![(0..13, 2..13)]);
        assert_eq!(&source[found[0].0.clone()], b"// one \\\r\ntwo");
    }

    #[test]
    fn a_line_without_the_backslash_ends_the_comment() {
        let source = b"// one\ntwo\n";
        assert_eq!(comments(source, &CONTINUED), vec![(0..6, 2..6)]);
    }

    const SQLISH: Syntax = Syntax {
        name: "sqlish",
        extensions: &[],
        filenames: &[],
        shebangs: &[],
        family: Family::Dash,
        engine: Engine::Generic,
        line: &[LineComment {
            marker: "--",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: false,
            owns_indent_block: false,
        }],
        block: &[],
        quotes: &[Quote {
            open: "'",
            close: "",
            escape: Escape::Doubling,
            multiline: true,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: Some(TextGlyphs {
                escapes: false,
                folds: true,
                delim: b'\'',
            }),
        }],
        heredocs: &[],
        doc: DocStyle::None,
        directives: &[],
        regions: &[],
    };

    #[test]
    fn a_doubled_close_delimiter_stays_inside_the_string() {
        let source = b"SELECT 'it''s -- no' -- yes\n";
        assert_eq!(comments(source, &SQLISH), vec![(21..27, 23..27)]);
        assert_eq!(texts(source, &SQLISH), vec![8..19]);
    }

    const RUSTISH: Syntax = Syntax {
        name: "rustish",
        extensions: &[],
        filenames: &[],
        shebangs: &[],
        family: Family::Curly,
        engine: Engine::Generic,
        line: &[SLASH],
        block: &[],
        quotes: &[
            // The plain/byte spelling is listed first so it wins the tie against
            // the hash-capable spelling at a bare `"`.
            Quote {
                open: "\"",
                close: "",
                escape: Escape::Backslash,
                multiline: true,
                prefixes: &["b"],
                hashes: false,
                raw_tag: false,
                text: Some(TextGlyphs {
                    escapes: true,
                    folds: true,
                    delim: b'"',
                }),
            },
            Quote {
                open: "\"",
                close: "",
                escape: Escape::None,
                multiline: true,
                prefixes: &["r", "br", "rb"],
                hashes: true,
                raw_tag: false,
                text: Some(TextGlyphs {
                    escapes: false,
                    folds: true,
                    delim: 0,
                }),
            },
        ],
        heredocs: &[],
        doc: DocStyle::None,
        directives: &[],
        regions: &[],
    };

    #[test]
    fn a_hash_suffixed_raw_close_needs_every_hash() {
        let source = b"r##\"a \"# b\"## // x\n";
        assert_eq!(comments(source, &RUSTISH), vec![(14..18, 16..18)]);
        assert_eq!(texts(source, &RUSTISH), vec![4..10]);
    }

    const CPPISH: Syntax = Syntax {
        name: "cppish",
        extensions: &[],
        filenames: &[],
        shebangs: &[],
        family: Family::Curly,
        engine: Engine::Generic,
        line: &[SLASH],
        block: &[],
        quotes: &[Quote {
            open: "\"",
            close: "",
            escape: Escape::Backslash,
            multiline: true,
            prefixes: &["R"],
            hashes: false,
            raw_tag: true,
            text: Some(TextGlyphs {
                escapes: false,
                folds: true,
                delim: 0,
            }),
        }],
        heredocs: &[],
        doc: DocStyle::None,
        directives: &[],
        regions: &[],
    };

    #[test]
    fn a_raw_tag_close_repeats_the_tag() {
        let source = b"R\"xy(a )xy b)xy\" // x\n";
        assert_eq!(comments(source, &CPPISH), vec![(17..21, 19..21)]);
        assert_eq!(texts(source, &CPPISH), vec![5..12]);
    }

    #[test]
    fn an_unterminated_string_bails() {
        assert_eq!(
            scan(b"\"note\n", &RUSTISH).err().expect("bails").0,
            "unterminated string"
        );
    }

    #[test]
    fn a_quote_that_spans_lines_does_not_bail() {
        let source = b"\"note\nstill note\" // x\n";
        assert_eq!(comments(source, &RUSTISH), vec![(18..22, 20..22)]);
    }

    const PREFIXED: Syntax = Syntax {
        name: "prefixed",
        extensions: &[],
        filenames: &[],
        shebangs: &[],
        family: Family::Hash,
        engine: Engine::Generic,
        line: &[SLASH],
        block: &[],
        quotes: &[Quote {
            open: "\"",
            close: "",
            escape: Escape::Backslash,
            multiline: false,
            prefixes: &["f", "rb"],
            hashes: false,
            raw_tag: false,
            text: None,
        }],
        heredocs: &[],
        doc: DocStyle::None,
        directives: &[],
        regions: &[],
    };

    #[test]
    fn a_prefix_is_matched_only_when_the_quote_is_next() {
        assert_eq!(
            comments(b"f\"x // y\" // z\n", &PREFIXED),
            vec![(10..14, 12..14)]
        );
        assert_eq!(comments(b"fx // z\n", &PREFIXED), vec![(3..7, 5..7)]);
    }

    #[test]
    fn a_quote_without_text_glyphs_records_no_text() {
        let source = b"f\"x // y\" // z\n";
        assert_eq!(texts(source, &PREFIXED), Vec::new());
    }

    const TICK: Syntax = Syntax {
        name: "tick",
        extensions: &[],
        filenames: &[],
        shebangs: &[],
        family: Family::Curly,
        engine: Engine::Generic,
        line: &[SLASH],
        block: &[],
        quotes: &[Quote {
            open: "'",
            close: "",
            escape: Escape::Backslash,
            multiline: false,
            prefixes: &[],
            hashes: false,
            raw_tag: false,
            text: None,
        }],
        heredocs: &[],
        doc: DocStyle::None,
        directives: &[],
        regions: &[],
    };

    #[test]
    fn a_single_quote_without_a_close_is_a_tick_not_a_literal() {
        let source = b"fn f<'a>(x: &'a str) -> &'a str { x } // note\n";
        assert_eq!(comments(source, &TICK), vec![(38..45, 40..45)]);
        assert_eq!(scan(b"'a'\n", &TICK).unwrap().texts.len(), 0);
    }

    #[test]
    fn at_start_of_file_matches_byte_zero_after_a_bom() {
        const FRONTMATTER: LineComment = LineComment {
            marker: "---",
            at: At::StartOfFile,
            continues_on_backslash: false,
            doc: false,
            owns_indent_block: false,
        };
        const ASTROISH: Syntax = Syntax {
            name: "astroish",
            extensions: &[],
            filenames: &[],
            shebangs: &[],
            family: Family::Template,
            engine: Engine::Generic,
            line: &[FRONTMATTER],
            block: &[],
            quotes: &[],
            heredocs: &[],
            doc: DocStyle::None,
            directives: &[],
            regions: &[],
        };
        assert_eq!(comments(b"---\nx\n", &ASTROISH), vec![(0..3, 3..3)]);
        assert_eq!(
            comments(b"\xef\xbb\xbf---\n", &ASTROISH),
            vec![(3..6, 6..6)]
        );
        assert_eq!(comments(b"x\n---\n", &ASTROISH), Vec::new());
    }
}
