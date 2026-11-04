//! The markup engine: host comments around regions lexed by another engine.
//!
//! Outside a region the descriptor's own `line`/`block`/`quotes` tables are run
//! by [`crate::engine::generic`]. A region opener is only considered at a byte
//! the host scanner did not already claim for a comment, and the longest match
//! wins across line comments, block comments and region openers (`<%#` before
//! `<%`, `{% comment %}` before `{%`, `<!--` before a tag).
//!
//! A region's closing marker is found by running the *embedded* engine over the
//! candidate body: a `%>` inside a string or a block comment is rejected, while
//! a line comment is truncated at `%>` so the close always wins.

use std::ops::Range;

use crate::engine::generic;
use crate::scan::{Bail, Out, char_width, ends_line, identifier_byte, line_end, starts_with};
use crate::syntax::{At, Engine, LineComment, Region, Syntax};

const BOM: &[u8] = b"\xef\xbb\xbf";

/// A guard against a pathological nesting depth turning the recursive region
/// scan into a stack overflow.
const MAX_DEPTH: usize = 64;

/// Bytes a template dialect may insert next to a whitespace-control delimiter.
const TRIM: &[u8] = b"-~";

pub fn scan(source: &[u8], syntax: &'static Syntax) -> Result<Out, Bail> {
    scan_at_depth(source, syntax, 0)
}

fn scan_at_depth(source: &[u8], syntax: &'static Syntax, depth: usize) -> Result<Out, Bail> {
    if depth > MAX_DEPTH {
        return Err(Bail("too many nested regions"));
    }
    let content_start = content_start(source);
    let mut out = Out::default();
    let mut cursor = 0;
    while cursor < source.len() {
        let Some(hit) = next_region(source, syntax, cursor, content_start) else {
            let mut host = generic::scan(&source[cursor..], syntax)?;
            shift_out(&mut host, cursor);
            append(&mut out, host);
            break;
        };
        match host_until(source, syntax, cursor, hit.start, content_start)? {
            Host::Clear(mut host) => {
                shift_out(&mut host, cursor);
                append(&mut out, host);
            }
            Host::Blocked(mut host, resume) => {
                shift_out(&mut host, cursor);
                append(&mut out, host);
                cursor = resume;
                continue;
            }
        }
        cursor = enter_region(source, syntax, &mut out, hit, content_start, depth)?;
    }
    Ok(out)
}

struct Hit {
    region: &'static Region,
    start: usize,
    /// Bytes of `region.open` that matched, trim byte included.
    open_len: usize,
}

/// Scan the host stretch that ends at `stop`. A region opener inside a host
/// comment is not an opener: the caller skips to the end of the comment.
enum Host {
    Clear(Out),
    Blocked(Out, usize),
}

fn host_until(
    source: &[u8],
    syntax: &'static Syntax,
    cursor: usize,
    stop: usize,
    content_start: usize,
) -> Result<Host, Bail> {
    match generic::scan(&source[cursor..stop], syntax) {
        Ok(host) => {
            if let Some(resume) =
                truncated_line_comment_end(source, syntax, cursor, stop, &host, content_start)
            {
                return blocked(source, syntax, cursor, resume);
            }
            Ok(Host::Clear(host))
        }
        Err(error) => match crossing_comment_end(source, syntax, cursor, stop) {
            Ok(Some(resume)) => blocked(source, syntax, cursor, resume),
            Ok(None) => Err(error),
            Err(error) => Err(error),
        },
    }
}

/// Re-scan a stretch that a comment crosses, so the comment is emitted whole.
fn blocked(
    source: &[u8],
    syntax: &'static Syntax,
    cursor: usize,
    resume: usize,
) -> Result<Host, Bail> {
    let host = generic::scan(&source[cursor..resume], syntax)?;
    Ok(Host::Blocked(host, resume))
}

/// A line comment that the host stretch truncates at `stop`: its marker is
/// before `stop` and its real end is after it. Block comments that cross `stop`
/// make `generic::scan` bail instead.
fn truncated_line_comment_end(
    source: &[u8],
    syntax: &'static Syntax,
    cursor: usize,
    stop: usize,
    host: &Out,
    content_start: usize,
) -> Option<usize> {
    for comment in &host.comments {
        let start = cursor + comment.span.start;
        let end = cursor + comment.span.end;
        if end != stop || start >= stop {
            continue;
        }
        let line = longest_line_at(source, syntax, start, content_start)?;
        let resume = line_comment_end(source, line, start);
        if resume > stop {
            return Some(resume);
        }
    }
    None
}

/// The end of a block comment that starts before `stop` and closes after it.
/// Extending a line at a time keeps the scan inside the host language until the
/// comment is complete.
fn crossing_comment_end(
    source: &[u8],
    syntax: &'static Syntax,
    cursor: usize,
    stop: usize,
) -> Result<Option<usize>, Bail> {
    let mut end = line_end(source, stop).max(stop);
    loop {
        match generic::scan(&source[cursor..end], syntax) {
            Ok(host) => {
                let relative = stop - cursor;
                for comment in &host.comments {
                    if comment.span.start < relative && comment.span.end > relative {
                        return Ok(Some(cursor + comment.span.end));
                    }
                }
                return Ok(None);
            }
            Err(_) if end >= source.len() => return Ok(None),
            Err(_) => {
                let next = past_terminator(source, end);
                if next <= end {
                    return Ok(None);
                }
                end = next;
            }
        }
    }
}

fn next_region(
    source: &[u8],
    syntax: &'static Syntax,
    from: usize,
    content_start: usize,
) -> Option<Hit> {
    let mut at = from;
    while at < source.len() {
        if let Some((region, open_len)) = matching_region(source, syntax, at, content_start) {
            let comment_len = longest_comment_at(source, syntax, at, content_start);
            // Longest match wins; on a tie the comment wins so the tail of a
            // comment is never emitted as markup.
            if comment_len < open_len {
                return Some(Hit {
                    region,
                    start: at,
                    open_len,
                });
            }
        }
        at += char_width(source, at);
    }
    None
}

fn matching_region(
    source: &[u8],
    syntax: &'static Syntax,
    at: usize,
    content_start: usize,
) -> Option<(&'static Region, usize)> {
    let mut best: Option<(&'static Region, usize)> = None;
    for region in syntax.regions {
        // `Region` has no open-anchor field. `once` is the only descriptor data
        // Astro frontmatter has for "only at the top of the file", so it is the
        // anchor as well as the one-region-per-file limit.
        if region.once && at != content_start {
            continue;
        }
        let Some(end) = needle_at(source, at, region.open.as_bytes()) else {
            continue;
        };
        let len = end - at;
        // ERB/EEx escape a literal opener as `<%%`; the escape is host text.
        if region.open == "<%" && source.get(at + len) == Some(&b'%') {
            continue;
        }
        if opens_tag(region.open) {
            if !source
                .get(at + len)
                .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n' | b'>' | b'/'))
            {
                continue;
            }
            if let Some((attr, value)) = region.needs_attr
                && !opening_tag_has(source, at, len, attr, value)
            {
                continue;
            }
        } else if region.needs_attr.is_some() {
            continue;
        }
        if best.is_none_or(|(_, best_len)| len > best_len) {
            best = Some((region, len));
        }
    }
    best
}

fn longest_comment_at(
    source: &[u8],
    syntax: &'static Syntax,
    at: usize,
    content_start: usize,
) -> usize {
    let mut longest = 0;
    for line in syntax.line {
        if starts_with(source, at, line.marker.as_bytes())
            && at_allowed(source, line.at, at, content_start)
        {
            longest = longest.max(line.marker.len());
        }
    }
    for block in syntax.block {
        if starts_with(source, at, block.open.as_bytes())
            && at_allowed(source, block.at, at, content_start)
        {
            longest = longest.max(block.open.len());
        }
    }
    longest
}

fn enter_region(
    source: &[u8],
    syntax: &'static Syntax,
    out: &mut Out,
    hit: Hit,
    content_start: usize,
    depth: usize,
) -> Result<usize, Bail> {
    let body_start = if opens_tag(hit.region.open) {
        tag_end(source, hit.start + hit.open_len).ok_or(Bail("unterminated tag"))? + 1
    } else {
        hit.start + hit.open_len
    };
    if hit.region.engine == Engine::Verbatim {
        let (_, close_end) = next_close(source, hit.region, body_start, content_start)
            .ok_or(Bail("unterminated region"))?;
        return Ok(close_end);
    }
    let resolved = resolve(hit.region.engine)?;
    let mut search = body_start;
    while let Some((close_start, close_end)) = next_close(source, hit.region, search, content_start)
    {
        // Candidate scanning is what makes the close string-aware and what lets
        // it win over a line comment opened in the body: a close inside a
        // string/block comment leaves the body scan "unterminated", a close
        // after a line-comment marker leaves it complete at the slice end.
        match scan_embedded(
            &source[body_start..close_start],
            hit.region.engine,
            syntax,
            resolved,
            depth,
        ) {
            Ok(mut part) => {
                shift_out(&mut part, body_start);
                append(out, part);
                return Ok(close_end);
            }
            Err(_) => search = close_start + char_width(source, close_start),
        }
    }
    Err(Bail("unterminated region"))
}

fn next_close(
    source: &[u8],
    region: &Region,
    from: usize,
    content_start: usize,
) -> Option<(usize, usize)> {
    let mut at = from;
    while at < source.len() {
        if at_allowed(source, region.close_at, at, content_start)
            && let Some(end) = close_end(source, region.close.as_bytes(), at)
        {
            return Some((at, end));
        }
        at += char_width(source, at);
    }
    None
}

fn close_end(source: &[u8], close: &[u8], at: usize) -> Option<usize> {
    if close.starts_with(b"</") {
        return tag_close_end(source, at, close);
    }
    needle_at(source, at, close)
}

/// Literal match, or one whitespace-control byte either side of the delimited
/// body (`{%- raw -%}` against `{% raw %}`).
fn needle_at(source: &[u8], at: usize, needle: &[u8]) -> Option<usize> {
    if starts_with(source, at, needle) {
        return Some(at + needle.len());
    }
    trimmed_needle_end(source, at, needle)
}

fn trimmed_needle_end(source: &[u8], at: usize, needle: &[u8]) -> Option<usize> {
    const LEAD: usize = 2;
    const TRAIL: usize = 2;
    if needle.len() < LEAD + TRAIL {
        return None;
    }
    let lead = &needle[..LEAD];
    if !(lead == b"{{" || lead == b"{%" || lead == b"{#") {
        return None;
    }
    let trail = &needle[needle.len() - TRAIL..];
    if !(trail == b"}}" || trail == b"%}" || trail == b"#}") {
        return None;
    }
    if !starts_with(source, at, lead) {
        return None;
    }
    let mut cursor = at + LEAD;
    if source.get(cursor).is_some_and(|byte| TRIM.contains(byte)) {
        cursor += 1;
    }
    let core = &needle[LEAD..needle.len() - TRAIL];
    if !starts_with(source, cursor, core) {
        return None;
    }
    cursor += core.len();
    if source.get(cursor).is_some_and(|byte| TRIM.contains(byte)) {
        cursor += 1;
    }
    if !starts_with(source, cursor, trail) {
        return None;
    }
    Some(cursor + TRAIL)
}

fn opens_tag(open: &str) -> bool {
    let bytes = open.as_bytes();
    bytes.len() > 1 && bytes[0] == b'<' && bytes[1].is_ascii_alphabetic()
}

fn opening_tag_has(source: &[u8], at: usize, open_len: usize, attr: &str, value: &str) -> bool {
    let Some(end) = tag_end(source, at + open_len) else {
        return false;
    };
    let tag = &source[at..end];
    contains(tag, attr.as_bytes()) && contains(tag, value.as_bytes())
}

/// The `>` that ends the opening tag, skipping quoted attribute values.
fn tag_end(source: &[u8], from: usize) -> Option<usize> {
    let mut at = from;
    while at < source.len() {
        match source[at] {
            b'"' | b'\'' => {
                let quote = source[at];
                at += 1;
                while at < source.len() && source[at] != quote {
                    at += 1;
                }
                if at < source.len() {
                    at += 1;
                }
            }
            b'>' => return Some(at),
            _ => at += char_width(source, at),
        }
    }
    None
}

/// `</SCRIPT >` closes `</script>`. Tag closes are case-insensitive and admit
/// whitespace before `>` as the HTML parser does; the descriptor cannot say it.
fn tag_close_end(source: &[u8], at: usize, close: &[u8]) -> Option<usize> {
    if close.len() < 3 || close.last() != Some(&b'>') {
        return None;
    }
    if source.get(at) != Some(&b'<') || source.get(at + 1) != Some(&b'/') {
        return None;
    }
    let name = &close[2..close.len() - 1];
    let mut cursor = at + 2;
    if source.len() < cursor + name.len() {
        return None;
    }
    if !source[cursor..cursor + name.len()].eq_ignore_ascii_case(name) {
        return None;
    }
    cursor += name.len();
    while source
        .get(cursor)
        .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
    {
        cursor += 1;
    }
    if source.get(cursor) == Some(&b'>') {
        Some(cursor + 1)
    } else {
        None
    }
}

/// The descriptor an embedded engine needs. `Engine::Markup` nests with the
/// host descriptor; the three `SPEC-DELTA-1` resolvers are looked up by
/// descriptor name so this file does not depend on `lang.rs` variant spellings.
fn resolve(engine: Engine) -> Result<Option<&'static Syntax>, Bail> {
    match engine {
        Engine::Verbatim | Engine::Generic | Engine::Curly | Engine::Dash | Engine::Markup => {
            Ok(None)
        }
        Engine::Template => Err(Bail("engine dispatch needs the dialect")),
        Engine::Elixir => Ok(Some(named("elixir")?)),
        // HEEx/EEX name `Engine::Indented` for their Elixir bodies. The
        // descriptor has no field that says the embedded language is Elixir;
        // this is the resolver SPEC-DELTA-1 asks the markup engine to carry.
        Engine::Indented => Ok(Some(named("elixir")?)),
        Engine::Script => Ok(Some(named("javascript")?)),
        Engine::Style => Ok(Some(named("css")?)),
        Engine::Ruby => Ok(Some(named("ruby")?)),
        Engine::Shell => Ok(Some(named("shell")?)),
        Engine::Perl => Ok(Some(named("perl")?)),
    }
}

fn scan_embedded(
    source: &[u8],
    engine: Engine,
    host: &'static Syntax,
    resolved: Option<&'static Syntax>,
    depth: usize,
) -> Result<Out, Bail> {
    match engine {
        Engine::Verbatim => Ok(Out::default()),
        Engine::Generic => generic::scan(source, host),
        Engine::Curly => crate::curly::scan(source, host),
        Engine::Dash => crate::engine::dash::scan(source, host),
        Engine::Markup => scan_at_depth(source, host, depth + 1),
        Engine::Template => Err(Bail("engine dispatch needs the dialect")),
        Engine::Elixir | Engine::Indented => {
            let elixir = resolved.ok_or(Bail("engine dispatch needs the dialect"))?;
            match elixir.engine {
                Engine::Generic | Engine::Elixir => generic::scan(source, elixir),
                _ => crate::engine::scan(source, elixir),
            }
        }
        Engine::Script | Engine::Style | Engine::Ruby | Engine::Shell | Engine::Perl => {
            let embedded = resolved.ok_or(Bail("engine dispatch needs the dialect"))?;
            crate::engine::scan(source, embedded)
        }
    }
}

fn named(name: &str) -> Result<&'static Syntax, Bail> {
    for dialect in crate::lang::Dialect::ALL {
        let syntax = dialect.syntax();
        if syntax.name == name {
            return Ok(syntax);
        }
    }
    Err(Bail("engine dispatch needs the dialect"))
}

fn longest_line_at(
    source: &[u8],
    syntax: &'static Syntax,
    at: usize,
    content_start: usize,
) -> Option<&'static LineComment> {
    let mut best: Option<&'static LineComment> = None;
    for line in syntax.line {
        if best.is_some_and(|best| best.marker.len() >= line.marker.len()) {
            continue;
        }
        if starts_with(source, at, line.marker.as_bytes())
            && at_allowed(source, line.at, at, content_start)
        {
            best = Some(line);
        }
    }
    best
}

fn at_allowed(source: &[u8], at: At, position: usize, content_start: usize) -> bool {
    match at {
        At::Anywhere => true,
        At::LineStart => source[line_start(source, position)..position]
            .iter()
            .all(|byte| matches!(byte, b' ' | b'\t')),
        At::WordStart => position == 0 || !identifier_byte(source[position - 1]),
        At::StartOfFile => position == content_start,
    }
}

fn content_start(source: &[u8]) -> usize {
    let mut at = if source.starts_with(BOM) {
        BOM.len()
    } else {
        0
    };
    if source.get(at) == Some(&b'#') && source.get(at + 1) == Some(&b'!') {
        at = past_terminator(source, line_end(source, at));
    }
    at
}

fn line_start(source: &[u8], at: usize) -> usize {
    let mut cursor = at.min(source.len());
    while cursor > 0 && !ends_line(source[cursor - 1]) {
        cursor -= 1;
    }
    cursor
}

fn line_comment_end(source: &[u8], line: &LineComment, at: usize) -> usize {
    let mut end = line_end(source, at + line.marker.len());
    while line.continues_on_backslash && end < source.len() {
        let mut back = end;
        if back > at && source[back - 1] == b'\r' {
            back -= 1;
        }
        if back == at || source[back - 1] != b'\\' {
            break;
        }
        end = line_end(source, past_terminator(source, end));
    }
    if line.owns_indent_block {
        end = indent_block_end(source, at, end);
    }
    end
}

/// The end of the last line under the marker whose indent is greater than the
/// marker line's indent. Blank lines inside the block are transparent.
fn indent_block_end(source: &[u8], at: usize, marker_line_end: usize) -> usize {
    let start = line_start(source, at);
    let indent = indent_width(source, start, at);
    let mut cursor = marker_line_end;
    let mut end = marker_line_end;
    while cursor < source.len() {
        cursor = past_terminator(source, cursor);
        if cursor >= source.len() {
            break;
        }
        let next_end = line_end(source, cursor);
        if blank_line(source, cursor, next_end) {
            cursor = next_end;
            continue;
        }
        if indent_width(source, cursor, next_end) > indent {
            end = next_end;
        } else {
            break;
        }
        cursor = next_end;
    }
    end
}

fn indent_width(source: &[u8], start: usize, end: usize) -> usize {
    source[start..end]
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count()
}

fn blank_line(source: &[u8], start: usize, end: usize) -> bool {
    source[start..end]
        .iter()
        .all(|byte| matches!(byte, b' ' | b'\t'))
}

fn past_terminator(source: &[u8], at: usize) -> usize {
    if source.get(at) == Some(&b'\r') && source.get(at + 1) == Some(&b'\n') {
        return at + 2;
    }
    at + 1
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn shift_out(out: &mut Out, base: usize) {
    for comment in &mut out.comments {
        shift(&mut comment.span, base);
        shift(&mut comment.body, base);
    }
    for text in &mut out.texts {
        shift(&mut text.body, base);
    }
    for logical in &mut out.logical {
        logical.start += base;
        logical.end += base;
        if let Some(sole) = &mut logical.sole_text {
            shift(sole, base);
        }
    }
}

fn shift(range: &mut Range<usize>, base: usize) {
    *range = base + range.start..base + range.end;
}

fn append(out: &mut Out, mut part: Out) {
    out.comments.append(&mut part.comments);
    out.texts.append(&mut part.texts);
    out.logical.append(&mut part.logical);
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use super::scan;
    use crate::syntax::{
        At, BlockComment, DocStyle, Engine, Escape, Family, LineComment, Quote, Region, Syntax,
    };

    const BASE: Syntax = Syntax {
        name: "markup-test",
        extensions: &[],
        filenames: &[],
        shebangs: &[],
        family: Family::Markup,
        engine: Engine::Markup,
        line: &[],
        block: &[],
        quotes: &[],
        heredocs: &[],
        doc: DocStyle::None,
        directives: &[],
        regions: &[],
    };

    const HTML_COMMENT: BlockComment = BlockComment {
        open: "<!--",
        close: "-->",
        nested: false,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    };

    const SCRIPT: Region = Region {
        open: "<script",
        close: "</script>",
        engine: Engine::Verbatim,
        needs_attr: None,
        close_at: At::Anywhere,
        once: false,
    };

    fn spans(source: &[u8], syntax: &'static Syntax) -> Vec<Range<usize>> {
        scan(source, syntax)
            .expect("source lexes")
            .comments
            .into_iter()
            .map(|comment| comment.span)
            .collect()
    }

    #[test]
    fn host_comments_are_purged_and_region_bodies_are_not_scanned() {
        const SYNTAX: Syntax = Syntax {
            block: &[HTML_COMMENT],
            regions: &[SCRIPT],
            ..BASE
        };
        let source = b"a <!-- one --> <script><!-- two --></script> b <!-- three -->";
        let found = spans(source, &SYNTAX);
        assert_eq!(found.len(), 2);
        assert_eq!(&source[found[0].clone()], b"<!-- one -->");
        assert_eq!(
            &source[found[1].clone()],
            b"<!-- three -->",
            "the script body's <!-- --> is verbatim"
        );
    }

    #[test]
    fn an_unterminated_region_bails() {
        const SYNTAX: Syntax = Syntax {
            regions: &[SCRIPT],
            ..BASE
        };
        let error = scan(b"<script>still open", &SYNTAX).err().expect("bails");
        assert_eq!(error.0, "unterminated region");
    }

    #[test]
    fn nested_regions_close_at_their_own_close() {
        const TEMPLATE: Region = Region {
            open: "<template",
            close: "</template>",
            engine: Engine::Markup,
            needs_attr: None,
            close_at: At::Anywhere,
            once: false,
        };
        const SYNTAX: Syntax = Syntax {
            block: &[HTML_COMMENT],
            regions: &[TEMPLATE],
            ..BASE
        };
        let source = b"<template><template><!-- inner --></template><!-- outer --></template>";
        let found = spans(source, &SYNTAX);
        assert_eq!(found.len(), 2);
        assert_eq!(&source[found[0].clone()], b"<!-- inner -->");
        assert_eq!(&source[found[1].clone()], b"<!-- outer -->");
    }

    #[test]
    fn a_close_inside_a_string_does_not_close_the_region() {
        const REGION: Region = Region {
            open: "<%",
            close: "%>",
            engine: Engine::Generic,
            needs_attr: None,
            close_at: At::Anywhere,
            once: false,
        };
        const SYNTAX: Syntax = Syntax {
            block: &[HTML_COMMENT],
            quotes: &[Quote {
                open: "\"",
                close: "",
                escape: Escape::Backslash,
                multiline: false,
                prefixes: &[],
                hashes: false,
                raw_tag: false,
                text: None,
            }],
            regions: &[REGION],
            ..BASE
        };
        let source = b"<%= \"100%> done\" %> <!-- after -->";
        let found = spans(source, &SYNTAX);
        assert_eq!(found.len(), 1);
        assert_eq!(&source[found[0].clone()], b"<!-- after -->");
    }

    #[test]
    fn a_region_close_wins_over_a_line_comment() {
        const REGION: Region = Region {
            open: "<%",
            close: "%>",
            engine: Engine::Generic,
            needs_attr: None,
            close_at: At::Anywhere,
            once: false,
        };
        const SLASH: LineComment = LineComment {
            marker: "//",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: false,
            owns_indent_block: false,
        };
        const SYNTAX: Syntax = Syntax {
            line: &[SLASH],
            block: &[HTML_COMMENT],
            regions: &[REGION],
            ..BASE
        };
        let source = b"<% x = 1 // note %> <!-- after -->";
        let found = spans(source, &SYNTAX);
        assert_eq!(found.len(), 2);
        assert_eq!(&source[found[0].clone()], b"// note ");
        assert_eq!(&source[found[1].clone()], b"<!-- after -->");
    }

    #[test]
    fn longest_match_puts_the_comment_before_the_region() {
        const EEX_COMMENT: BlockComment = BlockComment {
            open: "<%#",
            close: "%>",
            nested: false,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        };
        const REGION: Region = Region {
            open: "<%",
            close: "%>",
            engine: Engine::Verbatim,
            needs_attr: None,
            close_at: At::Anywhere,
            once: false,
        };
        const SYNTAX: Syntax = Syntax {
            block: &[EEX_COMMENT],
            regions: &[REGION],
            ..BASE
        };
        let source = b"<%# note %>";
        assert_eq!(spans(source, &SYNTAX), vec![0..source.len()]);
        assert_eq!(spans(b"<%= x %>", &SYNTAX), Vec::new());
        assert_eq!(
            spans(b"<%% escaped, not a region", &SYNTAX),
            Vec::new(),
            "<%% is a literal opener, so there is no unterminated region"
        );
    }

    #[test]
    fn an_astro_frontmatter_closes_only_at_a_line_start_and_only_once() {
        const FRONTMATTER: Region = Region {
            open: "---",
            close: "---",
            engine: Engine::Verbatim,
            needs_attr: None,
            close_at: At::LineStart,
            once: true,
        };
        const SYNTAX: Syntax = Syntax {
            block: &[HTML_COMMENT],
            regions: &[FRONTMATTER],
            ..BASE
        };
        let source =
            b"---\nconst x = 1; --- still frontmatter\n<!-- hidden -->\n---\n<!-- after -->\n";
        let found = spans(source, &SYNTAX);
        assert_eq!(found.len(), 1);
        assert_eq!(&source[found[0].clone()], b"<!-- after -->");
        assert_eq!(
            spans(b"x\n---\n<!-- body -->\n", &SYNTAX),
            vec![6..6 + b"<!-- body -->".len()],
            "a once region is anchored to the start of the file"
        );
    }

    #[test]
    fn a_region_opener_inside_a_comment_is_not_opened() {
        const SYNTAX: Syntax = Syntax {
            block: &[HTML_COMMENT],
            regions: &[SCRIPT],
            ..BASE
        };
        let source = b"<!-- <script> -->";
        assert_eq!(spans(source, &SYNTAX), vec![0..source.len()]);
    }

    #[test]
    fn needs_attr_is_a_presence_test_on_the_opening_tag() {
        const MODULE: Region = Region {
            open: "<script",
            close: "</script>",
            engine: Engine::Verbatim,
            needs_attr: Some(("type", "module")),
            close_at: At::Anywhere,
            once: false,
        };
        const SYNTAX: Syntax = Syntax {
            block: &[HTML_COMMENT],
            regions: &[MODULE],
            ..BASE
        };
        let source = b"<script src=\"x\"><!-- plain --></script><script type=\"module\"><!-- module --></script>";
        let found = spans(source, &SYNTAX);
        assert_eq!(found.len(), 1);
        assert_eq!(&source[found[0].clone()], b"<!-- plain -->");
    }

    #[test]
    fn a_tag_close_is_case_insensitive_and_admits_whitespace() {
        const SYNTAX: Syntax = Syntax {
            block: &[HTML_COMMENT],
            regions: &[SCRIPT],
            ..BASE
        };
        let source = b"<script>x</SCRIPT ><!-- after -->";
        let found = spans(source, &SYNTAX);
        assert_eq!(found.len(), 1);
        assert_eq!(&source[found[0].clone()], b"<!-- after -->");
    }
}
