//! The dash-family scanners whose run/terminator shapes are not table data.
//!
//! `Engine::Dash` is named by exactly the descriptors that need one of three
//! rules the generic engine cannot express:
//!
//! * Lua long brackets: `--[=*[…]=*]` and `[=*[…]=*]`, where the `=` count in
//!   the opener selects the closer.
//! * Haskell's `--`: a comment only when the dash run cannot continue as an
//!   operator, i.e. the byte after the run is not a Haskell symbol.
//! * SQL dollar quoting: `$tag$ … $tag$`, where the tag in the opener selects
//!   the closer.
//!
//! The marker and quote tables still decide *what* is a comment or a literal
//! and which glyphs a body feeds to the sweep; only the run shape lives here.

use std::ops::Range;

use crate::edit::line_start;
use crate::scan::{Bail, Out, char_width, identifier_byte, line_end, starts_with};
use crate::syntax::{At, BlockComment, Escape, LineComment, Quote, Syntax};

/// Haskell 2010 §2.2 symbol characters. A `--` whose dash run is followed by
/// one of these is part of a longer operator (`-->`, `|--`), not a comment.
const SYMBOLS: &[u8] = b"!#$%&*+./<=>?@\\^|-~:";

const BOM: &[u8] = b"\xef\xbb\xbf";

/// Lex `source` with the dash engine named by `syntax`.
pub fn scan(source: &[u8], syntax: &'static Syntax) -> Result<Out, Bail> {
    match syntax.name {
        "lua" => lua(source, syntax),
        "haskell" => haskell(source, syntax),
        "sql" => sql(source, syntax),
        // `Engine::Dash` is only reached through one of the descriptors above.
        // Anything else is a wiring bug, and an empty scan would hide it.
        _ => Err(Bail("dash engine has no scanner for this descriptor")),
    }
}

fn allowed(source: &[u8], at: usize, kind: At) -> bool {
    match kind {
        At::Anywhere => true,
        At::LineStart => source[line_start(source, at)..at]
            .iter()
            .all(|byte| matches!(byte, b' ' | b'\t')),
        At::WordStart => at == 0 || !identifier_byte(source[at - 1]),
        At::StartOfFile => at == content_start(source),
    }
}

/// Byte 0, or the first byte after a BOM and an optional shebang line.
fn content_start(source: &[u8]) -> usize {
    let mut at = if source.starts_with(BOM) {
        BOM.len()
    } else {
        0
    };
    if source.get(at) == Some(&b'#') && source.get(at + 1) == Some(&b'!') {
        at = line_end(source, at);
        if at < source.len() {
            at += if source[at] == b'\r' && source.get(at + 1) == Some(&b'\n') {
                2
            } else {
                1
            };
        }
    }
    at
}

fn matching_line(
    source: &[u8],
    syntax: &'static Syntax,
    at: usize,
) -> Option<&'static LineComment> {
    let mut best: Option<&'static LineComment> = None;
    for line in syntax.line {
        if best.is_some_and(|best| best.marker.len() >= line.marker.len()) {
            continue;
        }
        if !starts_with(source, at, line.marker.as_bytes()) {
            continue;
        }
        if !allowed(source, at, line.at) {
            continue;
        }
        best = Some(line);
    }
    best
}

fn matching_block(
    source: &[u8],
    syntax: &'static Syntax,
    at: usize,
) -> Option<&'static BlockComment> {
    let mut best: Option<&'static BlockComment> = None;
    for block in syntax.block {
        if best.is_some_and(|best| best.open.len() >= block.open.len()) {
            continue;
        }
        if !starts_with(source, at, block.open.as_bytes()) {
            continue;
        }
        if !allowed(source, at, block.at) {
            continue;
        }
        best = Some(block);
    }
    best
}

fn line_comment(source: &[u8], at: usize, line: &'static LineComment, out: &mut Out) -> usize {
    let start = at;
    let mut end = line_end(source, at + line.marker.len());
    while line.continues_on_backslash && end < source.len() {
        let mut back = end;
        if back > start && source[back - 1] == b'\r' {
            back -= 1;
        }
        if back == start || source[back - 1] != b'\\' {
            break;
        }
        let mut next = end + 1;
        if source[end] == b'\r' && source.get(next) == Some(&b'\n') {
            next += 1;
        }
        end = line_end(source, next);
    }
    out.comment(start..end, start + line.marker.len()..end);
    end
}

fn block_comment(
    source: &[u8],
    at: usize,
    block: &'static BlockComment,
    out: &mut Out,
) -> Result<usize, Bail> {
    let start = at;
    let body = start + block.open.len();
    let mut cursor = body;
    let mut depth = 1usize;
    while cursor < source.len() {
        if block.nested && starts_with(source, cursor, block.open.as_bytes()) {
            depth += 1;
            cursor += block.open.len();
            continue;
        }
        if starts_with(source, cursor, block.close.as_bytes()) {
            cursor += block.close.len();
            depth -= 1;
            if depth == 0 {
                out.comment(start..cursor, body..cursor - block.close.len());
                return Ok(cursor);
            }
            continue;
        }
        cursor += 1;
    }
    if block.to_eof_ok {
        out.comment(start..source.len(), body..source.len());
        return Ok(source.len());
    }
    Err(Bail("unterminated block comment"))
}

/// The longest quote placed at `at`, with the byte just past its opener
/// (prefixes included).
fn matching_quote(
    source: &[u8],
    syntax: &'static Syntax,
    at: usize,
) -> Option<(&'static Quote, usize)> {
    let mut best: Option<(&'static Quote, usize)> = None;
    for quote in syntax.quotes {
        let Some(start) = opening(source, at, quote) else {
            continue;
        };
        if best.is_some_and(|(_, best)| best >= start) {
            continue;
        }
        best = Some((quote, start));
    }
    best
}

fn opening(source: &[u8], at: usize, quote: &Quote) -> Option<usize> {
    let open = quote.open.as_bytes();
    if starts_with(source, at, open) {
        return Some(at + open.len());
    }
    for prefix in quote.prefixes {
        let prefix = prefix.as_bytes();
        if starts_with(source, at, prefix) && starts_with(source, at + prefix.len(), open) {
            return Some(at + prefix.len() + open.len());
        }
    }
    None
}

fn close_of(quote: &Quote) -> &'static str {
    // `close: "''"` with `Escape::Doubling` is the descriptor shorthand for
    // "the delimiter is one `'` and a doubled `''` continues" (SQL's string
    // and quoted-name form). A literal two-byte closer would never match the
    // single quote that ends such a literal.
    if quote.close.is_empty() || (quote.escape == Escape::Doubling && doubled_close(quote)) {
        quote.open
    } else {
        quote.close
    }
}

fn doubled_close(quote: &Quote) -> bool {
    let open = quote.open.as_bytes();
    let close = quote.close.as_bytes();
    close.len() == open.len() * 2 && close[..open.len()] == *open && close[open.len()..] == *open
}

fn quoted(
    source: &[u8],
    quote: &'static Quote,
    start: usize,
    out: &mut Out,
) -> Result<usize, Bail> {
    let close = close_of(quote);
    let mut cursor = start;
    while cursor < source.len() {
        if quote.escape == Escape::Backslash && source[cursor] == b'\\' {
            cursor += 1;
            if cursor < source.len() {
                cursor += char_width(source, cursor);
            }
            continue;
        }
        if starts_with(source, cursor, close.as_bytes()) {
            let end = cursor + close.len();
            if quote.escape == Escape::Doubling && starts_with(source, end, close.as_bytes()) {
                cursor = end + close.len();
                continue;
            }
            out_text(out, quote, start..cursor);
            return Ok(end);
        }
        if !quote.multiline && matches!(source[cursor], b'\n' | b'\r') {
            return Err(Bail("unterminated string"));
        }
        cursor += char_width(source, cursor);
    }
    if quote.text.is_none() {
        // SQL's `"…"` is a quoted identifier, not swept text. A missing
        // closer leaves the rest as identifier bytes; keeping them is safer
        // than refusing the file.
        return Ok(source.len());
    }
    Err(Bail("unterminated string"))
}

fn out_text(out: &mut Out, quote: &Quote, body: Range<usize>) {
    if let Some(glyphs) = quote.text {
        out.text(body, glyphs.escapes, glyphs.folds, glyphs.delim);
    }
}

fn lua(source: &[u8], syntax: &'static Syntax) -> Result<Out, Bail> {
    let mut out = Out::default();
    let mut at = 0usize;
    while at < source.len() {
        // `--` followed by a long bracket, at the level the opener spells.
        if let Some(block) = syntax
            .block
            .iter()
            .find(|block| block.open.ends_with("[[") && block.close == "]]")
            && allowed(source, at, block.at)
            && let Some(prefix) = block.open.strip_suffix("[[")
            && starts_with(source, at, prefix.as_bytes())
            && let Some(level) = long_level(source, at + prefix.len())
        {
            let start = at;
            let body = at + prefix.len() + level + 2;
            let Some(close) = long_close(source, body, level) else {
                return Err(Bail("unterminated long comment"));
            };
            let end = close + level + 2;
            out.comment(start..end, body..close);
            at = end;
            continue;
        }
        if let Some(line) = matching_line(source, syntax, at) {
            at = line_comment(source, at, line, &mut out);
            continue;
        }
        if let Some(end) = lua_long_string(source, syntax, at, &mut out)? {
            at = end;
            continue;
        }
        if let Some((quote, start)) = matching_quote(source, syntax, at) {
            at = quoted(source, quote, start, &mut out)?;
            continue;
        }
        at += char_width(source, at);
    }
    Ok(out)
}

/// `[=*[ … ]=*]`: a long string. The descriptor's `[[` quote marks the form;
/// the level is read from the opener by [`long_level`].
fn lua_long_string(
    source: &[u8],
    syntax: &'static Syntax,
    at: usize,
    out: &mut Out,
) -> Result<Option<usize>, Bail> {
    let Some(quote) = syntax.quotes.iter().find(|quote| quote.open == "[[") else {
        return Ok(None);
    };
    let Some(level) = long_level(source, at) else {
        return Ok(None);
    };
    let body = at + level + 2;
    let Some(close) = long_close(source, body, level) else {
        return Err(Bail("unterminated long string"));
    };
    out_text(out, quote, body..close);
    Ok(Some(close + level + 2))
}

fn long_level(source: &[u8], at: usize) -> Option<usize> {
    if source.get(at) != Some(&b'[') {
        return None;
    }
    let mut cursor = at + 1;
    let mut level = 0usize;
    while source.get(cursor) == Some(&b'=') {
        level += 1;
        cursor += 1;
    }
    (source.get(cursor) == Some(&b'[')).then_some(level)
}

fn long_close(source: &[u8], from: usize, level: usize) -> Option<usize> {
    let mut closing = Vec::with_capacity(level + 2);
    closing.push(b']');
    closing.extend(std::iter::repeat_n(b'=', level));
    closing.push(b']');
    let mut cursor = from;
    while cursor < source.len() {
        if starts_with(source, cursor, &closing) {
            return Some(cursor);
        }
        cursor += 1;
    }
    None
}

fn haskell(source: &[u8], syntax: &'static Syntax) -> Result<Out, Bail> {
    let mut out = Out::default();
    let mut at = 0usize;
    while at < source.len() {
        if let Some(line) = matching_line(source, syntax, at) {
            // `--` is a comment only when the run of dashes cannot be the
            // prefix of a longer operator.
            let mut cursor = at;
            while source.get(cursor) == Some(&b'-') {
                cursor += 1;
            }
            let tail = source.get(cursor).copied().unwrap_or(b'\n');
            if !SYMBOLS.contains(&tail) {
                at = line_comment(source, at, line, &mut out);
                continue;
            }
            at = cursor;
            continue;
        }
        if let Some(block) = matching_block(source, syntax, at) {
            // `{-# … #-}` pragmas arrive here too. `keep.rs` retains them
            // through `syntax.directives`, whose needles start with `#`.
            at = block_comment(source, at, block, &mut out)?;
            continue;
        }
        if let Some((quote, start)) = matching_quote(source, syntax, at) {
            at = quoted(source, quote, start, &mut out)?;
            continue;
        }
        if source[at] == b'\'' && (at == 0 || !identifier_byte(source[at - 1])) {
            // Not in `syntax.quotes`: `foo'` is a legal Haskell identifier, so
            // a `'` quote would swallow every primed name. A char literal can
            // never contain `--` or `{-`, so skipping it is safe for comments.
            let mut cursor = at + 1;
            loop {
                if cursor >= source.len() || source[cursor] == b'\n' {
                    at += 1;
                    break;
                }
                match source[cursor] {
                    b'\\' => cursor += 1 + char_width(source, (cursor + 1).min(source.len() - 1)),
                    b'\'' => {
                        at = cursor + 1;
                        break;
                    }
                    _ => cursor += char_width(source, cursor),
                }
            }
            continue;
        }
        at += char_width(source, at);
    }
    Ok(out)
}

fn sql(source: &[u8], syntax: &'static Syntax) -> Result<Out, Bail> {
    let mut out = Out::default();
    let dollar = syntax.quotes.iter().find(|quote| quote.open == "$$");
    let mut at = 0usize;
    while at < source.len() {
        if let Some(line) = matching_line(source, syntax, at) {
            at = line_comment(source, at, line, &mut out);
            continue;
        }
        if let Some(block) = matching_block(source, syntax, at) {
            at = block_comment(source, at, block, &mut out)?;
            continue;
        }
        if source[at] == b'$'
            && let Some(quote) = dollar
            && let Some((end, body)) = dollar_quote(source, at)?
        {
            out_text(&mut out, quote, body);
            at = end;
            continue;
        }
        if let Some((quote, start)) = matching_quote(source, syntax, at) {
            at = quoted(source, quote, start, &mut out)?;
            continue;
        }
        at += char_width(source, at);
    }
    Ok(out)
}

/// `$tag$ … $tag$`: the tag read from the opener selects the closer. Returns
/// `(end of the literal, body range)`; `None` means this `$` does not open a
/// dollar quote.
fn dollar_quote(source: &[u8], at: usize) -> Result<Option<(usize, Range<usize>)>, Bail> {
    let mut cursor = at + 1;
    while cursor < source.len() && identifier_byte(source[cursor]) {
        cursor += 1;
    }
    if source.get(cursor) != Some(&b'$') {
        return Ok(None);
    }
    let tag = &source[at..cursor + 1];
    let start = cursor + 1;
    let mut probe = start;
    let close = loop {
        if probe >= source.len() {
            return Err(Bail("unterminated dollar quote"));
        }
        if starts_with(source, probe, tag) {
            break probe;
        }
        probe += 1;
    };
    Ok(Some((close + tag.len(), start..close)))
}
