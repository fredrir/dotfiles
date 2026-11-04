//! The Curly-family reference scanner.
//!
//! Kept as a second, independent implementation of the descriptor rules that
//! `engine/generic.rs` implements table-driven: `core/tests/equivalence.rs`
//! asserts the two agree byte for byte over every Curly dialect, so sharing code
//! between them would make the comparison vacuous. Everything language-specific
//! is read from the [`Syntax`]; there is no `Dialect` left in this file.

use std::ops::Range;

use crate::edit::line_start;
use crate::scan::{Bail, Out, char_width, identifier_byte, line_end, starts_with};
use crate::syntax::{At, BlockComment, Escape, LineComment, Quote, Syntax, TextGlyphs};

const BOM: &[u8] = b"\xef\xbb\xbf";

/// Longest C++ raw string delimiter tag, in bytes: `R"tag(… )tag"`.
const RAW_TAG_LIMIT: usize = 16;

pub fn scan(source: &[u8], syntax: &'static Syntax) -> Result<Out, Bail> {
    let mut scanner = Curly {
        source,
        syntax,
        at: 0,
        out: Out::default(),
    };
    scanner.code()?;
    Ok(scanner.out)
}

struct Curly<'a> {
    source: &'a [u8],
    syntax: &'static Syntax,
    at: usize,
    out: Out,
}

/// A quote that matched, and everything the close has to repeat.
struct Hit {
    quote: &'static Quote,
    /// Bytes from `at` to the end of the opening delimiter, prefixes included.
    open: usize,
    hashes: usize,
}

/// What a matched quote turned out to be.
enum Shape {
    /// `'a` in Rust: a tick, not a literal.
    Tick,
    /// `R"tag(… )tag"`: the close is built from a tag read at the open.
    Tagged,
    /// `"…"`, `"""…"""`, `r#"…"#`: an ordinary delimited literal.
    Quoted,
}

impl Curly<'_> {
    fn len(&self) -> usize {
        self.source.len()
    }

    fn byte(&self, at: usize) -> u8 {
        self.source.get(at).copied().unwrap_or(0)
    }

    fn code(&mut self) -> Result<(), Bail> {
        while self.at < self.len() {
            if let Some(line) = self.matching_line() {
                self.line_comment(line);
                continue;
            }
            if let Some(block) = self.matching_block() {
                self.block_comment(block)?;
                continue;
            }
            if self.quote()? {
                continue;
            }
            self.at += char_width(self.source, self.at);
        }
        Ok(())
    }

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
            at = line_end(self.source, at);
            if at < self.len() {
                at += if self.source[at] == b'\r' && self.source.get(at + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
            }
        }
        at
    }

    fn matching_line(&self) -> Option<&'static LineComment> {
        let mut best: Option<&'static LineComment> = None;
        for line in self.syntax.line {
            let longer = best.is_none_or(|best| best.marker.len() < line.marker.len());
            if !longer || !starts_with(self.source, self.at, line.marker.as_bytes()) {
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
            let mut next = end + 1;
            if self.byte(end) == b'\r' && self.byte(next) == b'\n' {
                next += 1;
            }
            end = line_end(self.source, next);
        }
        self.out.comment(start..end, start + line.marker.len()..end);
        self.at = end;
    }

    fn matching_block(&self) -> Option<&'static BlockComment> {
        let mut best: Option<&'static BlockComment> = None;
        for block in self.syntax.block {
            let longer = best.is_none_or(|best| best.open.len() < block.open.len());
            if !longer || !starts_with(self.source, self.at, block.open.as_bytes()) {
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
            } else if starts_with(self.source, at, block.close.as_bytes()) {
                depth -= 1;
                at += block.close.len();
                if depth == 0 {
                    self.out
                        .comment(start..at, start + block.open.len()..at - block.close.len());
                    self.at = at;
                    return Ok(());
                }
            } else {
                at += 1;
            }
        }
        if block.to_eof_ok {
            self.out
                .comment(start..self.len(), start + block.open.len()..self.len());
            self.at = self.len();
            return Ok(());
        }
        Err(Bail("unterminated block comment"))
    }

    fn quote(&mut self) -> Result<bool, Bail> {
        let Some(hit) = self.matching_quote() else {
            return Ok(false);
        };
        match self.shape(&hit) {
            Shape::Tick => {
                self.at += hit.open;
                Ok(true)
            }
            Shape::Tagged => self.raw_tag(hit),
            Shape::Quoted => {
                self.quoted(hit)?;
                Ok(true)
            }
        }
    }

    fn matching_quote(&self) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        for quote in self.syntax.quotes {
            let Some((open, hashes)) = self.opening_of(quote) else {
                continue;
            };
            if best.as_ref().is_some_and(|best| best.open >= open) {
                continue;
            }
            best = Some(Hit {
                quote,
                open,
                hashes,
            });
        }
        best
    }

    /// Bytes from `at` to the end of `quote`'s opening, and the `#`s it consumed.
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
            // The hashes may sit inside the prefix (`#` for Swift) or between the
            // prefix and the delimiter (`r##` for Rust); both count.
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
            return Some((
                at + open.len() - self.at,
                if quote.hashes { hashes } else { 0 },
            ));
        }
        None
    }

    fn shape(&self, hit: &Hit) -> Shape {
        if hit.quote.raw_tag {
            return Shape::Tagged;
        }
        if self.ticks(hit) {
            return Shape::Tick;
        }
        Shape::Quoted
    }

    /// Rust's `'a` lifetime is not a character literal.
    ///
    /// A single-quoted, single-line literal opens only when it is followed by a
    /// backslash or when the character after that character is the closing
    /// quote: `'x'` and `'\n'` are literals, `&'a str` is not. This is the rule
    /// this file has always applied to Rust, and the only reading of `'` that
    /// keeps lifetimes out of `Bail("unterminated string")`.
    fn ticks(&self, hit: &Hit) -> bool {
        let quote = hit.quote;
        if hit.hashes != 0
            || hit.open != quote.open.len()
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

    fn quoted(&mut self, hit: Hit) -> Result<(), Bail> {
        let quote = hit.quote;
        let close = close_of(quote);
        let start = self.at + hit.open;
        self.at = start;
        while self.at < self.len() {
            if quote.escape == Escape::Backslash && self.byte(self.at) == b'\\' {
                self.at += 1;
                if self.at < self.len() {
                    self.at += char_width(self.source, self.at);
                }
                continue;
            }
            if let Some(end) = self.closed_at(self.at, close, hit.hashes) {
                if quote.escape == Escape::Doubling
                    && let Some(doubled) = self.closed_at(end, close, hit.hashes)
                {
                    self.at = doubled;
                    continue;
                }
                self.out_text(quote, start..self.at, hit.hashes);
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
    fn closed_at(&self, at: usize, close: &'static str, hashes: usize) -> Option<usize> {
        let close = close.as_bytes();
        if !starts_with(self.source, at, close) {
            return None;
        }
        let end = at + close.len();
        if !(0..hashes).all(|offset| self.byte(end + offset) == b'#') {
            return None;
        }
        Some(end + hashes)
    }

    fn raw_tag(&mut self, hit: Hit) -> Result<bool, Bail> {
        let source = self.source;
        let open_end = self.at + hit.open;
        let mut tag_end = open_end;
        while tag_end < self.len()
            && tag_end - open_end <= RAW_TAG_LIMIT
            && self.byte(tag_end) != b'('
        {
            tag_end += 1;
        }
        if self.byte(tag_end) != b'(' {
            // `R` was an identifier after all: fall back to scanning it as one
            // and let the quote after it open an ordinary string.
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
                self.out_text(hit.quote, start..cursor, 0);
                self.at = cursor + tag.len() + 2;
                return Ok(true);
            }
            cursor += 1;
        }
        Err(Bail("unterminated raw string"))
    }

    fn out_text(&mut self, quote: &'static Quote, body: Range<usize>, hashes: usize) {
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
