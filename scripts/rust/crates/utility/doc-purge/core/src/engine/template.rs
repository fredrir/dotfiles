
use std::ops::Range;

use crate::engine::generic;
use crate::scan::{Bail, Out, char_width, ends_line, identifier_byte, line_end, starts_with};
use crate::syntax::{At, BlockComment, Engine, LineComment, Region, Syntax};

const BOM: &[u8] = b"\xef\xbb\xbf";

const MAX_DEPTH: usize = 64;

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
    let mut at = 0;
    while at < source.len() {
        let line = matching_line(source, syntax, at, content_start);
        let block = matching_block(source, syntax, at, content_start);
        let region = matching_region(source, syntax, at, content_start);
        let line_len = line.map_or(0, |line| line.marker.len());
        let block_len = block.map_or(0, |(_, len)| len);
        let comment_len = line_len.max(block_len);
        let region_len = region.map_or(0, |(_, len)| len);
        if comment_len == 0 && region_len == 0 {
            at += char_width(source, at);
            continue;
        }
        if comment_len > 0 && comment_len >= region_len {
            if let Some((block, open_len)) = block.filter(|_| block_len >= line_len) {
                at = block_comment(source, &mut out, block, open_len, at)?;
            } else if let Some(line) = line {
                at = line_comment(source, &mut out, line, at);
            } else {
                at += char_width(source, at);
            }
            continue;
        }
        if let Some((region, open_len)) = region {
            let hit = Hit {
                region,
                start: at,
                open_len,
            };
            at = enter_region(source, syntax, &mut out, hit, content_start, depth)?;
        } else {
            at += char_width(source, at);
        }
    }
    Ok(out)
}

fn matching_line(
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

fn matching_block(
    source: &[u8],
    syntax: &'static Syntax,
    at: usize,
    content_start: usize,
) -> Option<(&'static BlockComment, usize)> {
    let mut best: Option<(&'static BlockComment, usize)> = None;
    for block in syntax.block {
        if !at_allowed(source, block.at, at, content_start) {
            continue;
        }
        let Some(end) = needle_at(source, at, block.open.as_bytes()) else {
            continue;
        };
        let len = end - at;
        if best.is_none_or(|(_, best_len)| len > best_len) {
            best = Some((block, len));
        }
    }
    best
}

fn matching_region(
    source: &[u8],
    syntax: &'static Syntax,
    at: usize,
    content_start: usize,
) -> Option<(&'static Region, usize)> {
    let mut best: Option<(&'static Region, usize)> = None;
    for region in syntax.regions {
        if region.once && at != content_start {
            continue;
        }
        let Some(end) = needle_at(source, at, region.open.as_bytes()) else {
            continue;
        };
        let len = end - at;
        if region.needs_attr.is_some() {
            continue;
        }
        if best.is_none_or(|(_, best_len)| len > best_len) {
            best = Some((region, len));
        }
    }
    best
}

fn line_comment(source: &[u8], out: &mut Out, line: &'static LineComment, at: usize) -> usize {
    let end = line_comment_end(source, line, at);
    out.comment(at..end, at + line.marker.len()..end);
    end
}

fn block_comment(
    source: &[u8],
    out: &mut Out,
    block: &'static BlockComment,
    open_len: usize,
    at: usize,
) -> Result<usize, Bail> {
    let open_end = at + open_len;
    match block_end(source, block, open_end)? {
        BlockEnd::Closed { end, body_end } => {
            out.comment(at..end, open_end..body_end);
            Ok(end)
        }
        BlockEnd::Eof => {
            out.comment(at..source.len(), open_end..source.len());
            Ok(source.len())
        }
    }
}

enum BlockEnd {
    Closed { end: usize, body_end: usize },
    Eof,
}

fn block_end(source: &[u8], block: &BlockComment, open_end: usize) -> Result<BlockEnd, Bail> {
    let open = block.open.as_bytes();
    let close = block.close.as_bytes();
    if close.is_empty() {
        return if block.to_eof_ok {
            Ok(BlockEnd::Eof)
        } else {
            Err(Bail("unterminated block comment"))
        };
    }
    let mut at = open_end;
    let mut depth = 1usize;
    while at < source.len() {
        if block.nested
            && let Some(end) = needle_at(source, at, open)
        {
            depth += 1;
            at = end;
            continue;
        }
        if let Some(end) = needle_at(source, at, close) {
            depth -= 1;
            if depth == 0 {
                return Ok(BlockEnd::Closed { end, body_end: at });
            }
            at = end;
            continue;
        }
        at += char_width(source, at);
    }
    if block.to_eof_ok {
        Ok(BlockEnd::Eof)
    } else {
        Err(Bail("unterminated block comment"))
    }
}

struct Hit {
    region: &'static Region,
    start: usize,
    open_len: usize,
}

fn enter_region(
    source: &[u8],
    syntax: &'static Syntax,
    out: &mut Out,
    hit: Hit,
    content_start: usize,
    depth: usize,
) -> Result<usize, Bail> {
    let region = hit.region;
    let body_start = hit.start + hit.open_len;
    if region.engine == Engine::Verbatim {
        let (_, close_end) = next_close(source, region, body_start, content_start)
            .ok_or(Bail("unterminated region"))?;
        return Ok(close_end);
    }
    let resolved = resolve(region.engine)?;
    let mut search = body_start;
    while let Some((close_start, close_end)) = next_close(source, region, search, content_start) {
        match scan_embedded(
            &source[body_start..close_start],
            region.engine,
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
            && let Some(end) = needle_at(source, at, region.close.as_bytes())
        {
            return Some((at, end));
        }
        at += char_width(source, at);
    }
    None
}

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

fn resolve(engine: Engine) -> Result<Option<&'static Syntax>, Bail> {
    match engine {
        Engine::Verbatim | Engine::Generic | Engine::Curly | Engine::Dash | Engine::Template => {
            Ok(None)
        }
        Engine::Markup => Err(Bail("engine dispatch needs the dialect")),
        Engine::Elixir => Ok(Some(named("elixir")?)),
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
        Engine::Template => scan_at_depth(source, host, depth + 1),
        Engine::Markup => Err(Bail("engine dispatch needs the dialect")),
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
        name: "template-test",
        extensions: &[],
        filenames: &[],
        shebangs: &[],
        family: Family::Template,
        engine: Engine::Template,
        line: &[],
        block: &[],
        quotes: &[],
        heredocs: &[],
        doc: DocStyle::None,
        directives: &[],
        regions: &[],
    };

    const JINJA_COMMENT: BlockComment = BlockComment {
        open: "{#",
        close: "#}",
        nested: false,
        at: At::Anywhere,
        important: None,
        to_eof_ok: false,
    };

    const JINJA_EXPR: Region = Region {
        open: "{{",
        close: "}}",
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
    fn host_text_is_not_scanned_without_host_markers() {
        const SYNTAX: Syntax = Syntax {
            block: &[JINJA_COMMENT],
            regions: &[JINJA_EXPR],
            ..BASE
        };
        let source = b"# // <!-- --> {# note #} {{ x }}";
        let found = spans(source, &SYNTAX);
        assert_eq!(found.len(), 1);
        assert_eq!(&source[found[0].clone()], b"{# note #}");
    }

    #[test]
    fn verbatim_regions_emit_nothing() {
        const RAW: Region = Region {
            open: "{% raw %}",
            close: "{% endraw %}",
            engine: Engine::Verbatim,
            needs_attr: None,
            close_at: At::Anywhere,
            once: false,
        };
        const TAG: Region = Region {
            open: "{%",
            close: "%}",
            engine: Engine::Verbatim,
            needs_attr: None,
            close_at: At::Anywhere,
            once: false,
        };
        const SYNTAX: Syntax = Syntax {
            block: &[JINJA_COMMENT],
            regions: &[RAW, TAG],
            ..BASE
        };
        let source =
            b"{% raw %}{# hidden #}{% endraw %}{% set x = \"{# also hidden #}\" %}{# shown #}";
        let found = spans(source, &SYNTAX);
        assert_eq!(found.len(), 1);
        assert_eq!(&source[found[0].clone()], b"{# shown #}");
    }

    #[test]
    fn liquid_comment_tag_is_one_block_comment() {
        const COMMENT: BlockComment = BlockComment {
            open: "{% comment %}",
            close: "{% endcomment %}",
            nested: false,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        };
        const TAG: Region = Region {
            open: "{%",
            close: "%}",
            engine: Engine::Verbatim,
            needs_attr: None,
            close_at: At::Anywhere,
            once: false,
        };
        const SYNTAX: Syntax = Syntax {
            block: &[COMMENT],
            regions: &[TAG],
            ..BASE
        };
        let source = b"a {% comment %} hidden {% endcomment %} b";
        let found = spans(source, &SYNTAX);
        assert_eq!(found.len(), 1);
        assert_eq!(
            &source[found[0].clone()],
            b"{% comment %} hidden {% endcomment %}"
        );
        let trimmed = b"{%- comment -%} hidden {%- endcomment -%}";
        let found = spans(trimmed, &SYNTAX);
        assert_eq!(found.len(), 1);
        assert_eq!(&trimmed[found[0].clone()], &trimmed[..]);
    }

    #[test]
    fn handlebars_long_comment_beats_its_short_prefix() {
        const LONG: BlockComment = BlockComment {
            open: "{{!--",
            close: "--}}",
            nested: false,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        };
        const SHORT: BlockComment = BlockComment {
            open: "{{!",
            close: "}}",
            nested: false,
            at: At::Anywhere,
            important: None,
            to_eof_ok: false,
        };
        const SYNTAX: Syntax = Syntax {
            block: &[LONG, SHORT],
            regions: &[JINJA_EXPR],
            ..BASE
        };
        let source = b"{{!-- renders {{foo}} --}}{{! short }}";
        let found = spans(source, &SYNTAX);
        assert_eq!(found.len(), 2);
        assert_eq!(&source[found[0].clone()], b"{{!-- renders {{foo}} --}}");
        assert_eq!(&source[found[1].clone()], b"{{! short }}");
    }

    #[test]
    fn an_unterminated_region_bails() {
        let error = scan(b"{{ x", &BASE_WITH_EXPR).err().expect("bails");
        assert_eq!(error.0, "unterminated region");
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
        const HASH: LineComment = LineComment {
            marker: "#",
            at: At::Anywhere,
            continues_on_backslash: false,
            doc: false,
            owns_indent_block: false,
        };
        const SYNTAX: Syntax = Syntax {
            line: &[HASH],
            regions: &[REGION],
            ..BASE
        };
        let source = b"<% x = 1 # note %>after";
        assert_eq!(spans(source, &SYNTAX), vec![9..16]);
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
        let source = b"<%= \"100%> done\" %>";
        assert_eq!(spans(source, &SYNTAX), Vec::new());
    }

    #[test]
    fn a_pug_comment_owns_its_indented_block() {
        const PUG_COMMENT: LineComment = LineComment {
            marker: "//-",
            at: At::LineStart,
            continues_on_backslash: false,
            doc: false,
            owns_indent_block: true,
        };
        const SYNTAX: Syntax = Syntax {
            line: &[PUG_COMMENT],
            ..BASE
        };
        let source = b"//- note\n  body\n    deeper\n  more\nnext\n";
        let found = spans(source, &SYNTAX);
        assert_eq!(found.len(), 1);
        assert_eq!(
            &source[found[0].clone()],
            b"//- note\n  body\n    deeper\n  more"
        );
    }

    #[test]
    fn whitespace_control_spellings_open_the_same_region() {
        const RAW: Region = Region {
            open: "{% raw %}",
            close: "{% endraw %}",
            engine: Engine::Verbatim,
            needs_attr: None,
            close_at: At::Anywhere,
            once: false,
        };
        const TAG: Region = Region {
            open: "{%",
            close: "%}",
            engine: Engine::Verbatim,
            needs_attr: None,
            close_at: At::Anywhere,
            once: false,
        };
        const SYNTAX: Syntax = Syntax {
            block: &[JINJA_COMMENT],
            regions: &[RAW, TAG],
            ..BASE
        };
        let source = b"{%- raw -%}{# hidden #}{%- endraw -%}{# shown #}";
        let found = spans(source, &SYNTAX);
        assert_eq!(found.len(), 1);
        assert_eq!(&source[found[0].clone()], b"{# shown #}");
    }

    const BASE_WITH_EXPR: Syntax = Syntax {
        name: "jinja-test",
        extensions: &[],
        filenames: &[],
        shebangs: &[],
        family: Family::Template,
        engine: Engine::Template,
        line: &[],
        block: &[],
        quotes: &[],
        heredocs: &[],
        doc: DocStyle::None,
        directives: &[],
        regions: &[JINJA_EXPR],
    };
}
