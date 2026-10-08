use dotfmt_core::syntax::{QuoteMode, scan};
use unicode_width::UnicodeWidthStr;

use crate::conf::lines;
use crate::config::Config;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    Blank,
    Comment,
    Open,
    Close,
    Entry,
    Bare,
}

pub struct Line<'a> {
    pub class: Class,
    #[cfg_attr(not(test), allow(dead_code))]
    pub number: usize,
    pub body: &'a str,
    pub block: &'a str,
    pub depth: usize,
    pub key: &'a str,
    pub value: &'a str,
}

impl Line<'_> {
    fn signature(&self) -> (Class, &str, &str, &str) {
        (self.class, self.block, self.key, self.value)
    }
}

#[derive(Debug)]
pub struct Problem {
    pub line: usize,
    pub message: String,
}

pub fn parse(text: &str) -> Result<Vec<Line<'_>>, Problem> {
    let mut found = Vec::new();
    let mut open: Vec<&str> = Vec::new();
    let mut number = 0;
    let mut compact_braces = 0;
    for raw in lines(text) {
        number += 1;
        let mut rest = raw;
        loop {
            let patterns = open.last().is_some_and(|name| {
                matches!(
                    *name,
                    "include"
                        | "exclude"
                        | "included_files"
                        | "included-files"
                        | "excluded_files"
                        | "excluded-files"
                        | "whitelist"
                        | "blacklist"
                )
            });
            let body = rest.trim_start_matches([' ', '\t']);
            let structural = body
                .split_once('#')
                .map_or(trim(body), |(head, _)| trim(head));
            let compact = compact_braces > 0 || (!patterns && compact_token(structural));
            let body = if patterns || compact {
                body
            } else {
                trim(body)
            };
            // Compact brace tokens may be data or configuration syntax. Keep
            // their contents intact, including assignments and multiline spans.
            if compact {
                let at = closing_brace(body, false, &mut compact_braces);
                let (part, tail) = at.map_or((body, ""), |at| body.split_at(at));
                found.push(Line {
                    class: Class::Bare,
                    number,
                    body: part,
                    block: open.last().copied().unwrap_or(""),
                    depth: open.len(),
                    key: part,
                    value: "",
                });
                if tail.is_empty() {
                    break;
                }
                rest = tail;
                continue;
            }
            let (class, key, value, tail) = if body.starts_with('#') || body.is_empty() {
                let (class, key, value) = classify(body);
                (class, key, value, "")
            } else if structural.starts_with('}') {
                let tail = &body[1..];
                let (comment, tail) = trailing_comment(tail);
                (Class::Close, "", comment, tail)
            } else if !patterns
                && let Some(at) = structural.find('{')
                && !structural[..at].contains(['=', '\'', '"'])
            {
                let name = trim(&structural[..at]);
                let (comment, tail) = trailing_comment(&body[at + 1..]);
                (Class::Open, name, comment, tail)
            } else {
                let at = closing_brace(body, patterns, &mut 0);
                let (part, tail) = at.map_or((body, ""), |at| body.split_at(at));
                let (class, key, value) = if patterns {
                    (Class::Bare, part, "")
                } else {
                    classify(trim(part))
                };
                let depth = open.len();
                found.push(Line {
                    class,
                    number,
                    body: if patterns { part } else { trim(part) },
                    block: open.last().copied().unwrap_or(""),
                    depth,
                    key,
                    value,
                });
                if tail.is_empty() {
                    break;
                }
                rest = tail;
                continue;
            };
            let (block, depth) = match class {
                Class::Open => {
                    let depth = open.len();
                    open.push(key);
                    (key, depth)
                }
                Class::Close => {
                    let Some(name) = open.pop() else {
                        return Err(Problem::at(number, "unexpected }"));
                    };
                    (name, open.len())
                }
                _ => (open.last().copied().unwrap_or(""), open.len()),
            };
            found.push(Line {
                class,
                number,
                body,
                block,
                depth,
                key,
                value,
            });
            if tail.trim().is_empty() {
                break;
            }
            rest = tail;
        }
    }
    if let Some(name) = open.last() {
        return Err(Problem::at(number, format!("missing }} for {name}")));
    }
    Ok(found)
}

// Whitespace before an opener and a line-ending opener are structural.
// Other brace-bearing tokens remain opaque: `lib{a,b}`, `${HOME}`, `json{}`.
fn compact_token(text: &str) -> bool {
    let Some(at) = text.find('{') else {
        return false;
    };
    let name = &text[..at];
    !name.is_empty()
        && !name.ends_with([' ', '\t'])
        && !name.contains(['=', '\'', '"'])
        && at + 1 < text.len()
}

fn trailing_comment(tail: &str) -> (&str, &str) {
    if tail.trim_start().starts_with('#') {
        (trim(tail), "")
    } else {
        ("", tail)
    }
}

// A structural close is outside quoted values and balanced literal braces.
// Pattern blocks additionally require whitespace before an inline close,
// preserving braces that are part of a filename and escaped trailing spaces.
fn closing_brace(text: &str, patterns: bool, braces: &mut usize) -> Option<usize> {
    let mut separated = false;
    let quotes = if patterns {
        QuoteMode::Start
    } else {
        QuoteMode::Anywhere
    };
    for token in scan(text, quotes) {
        let ch = token.character;
        let separator = ch.is_whitespace() && token.is_structural();
        if token.is_structural() {
            if ch == '#' && !patterns {
                break;
            } else if ch == '{' && !patterns {
                *braces += 1;
            } else if ch == '}' {
                if *braces > 0 {
                    *braces -= 1;
                } else if !patterns || separated {
                    return Some(token.span.start);
                }
            }
        }
        separated = separator;
    }
    None
}

pub fn format(text: &str, config: &Config) -> Result<String, Problem> {
    let parsed = parse(text)?;
    // A file of nothing but blank lines is left exactly as it is. There is no
    // layout to impose on it, and a formatter that can turn a file into zero
    // bytes is a formatter nobody can leave on save.
    if parsed.iter().all(|line| line.class == Class::Blank) {
        return Ok(text.to_string());
    }
    let arranged = arrange(&parsed, config);
    let widths = columns(&arranged, config);
    let rendered: Vec<_> = arranged
        .iter()
        .zip(&widths)
        .map(|(item, width)| render(item, *width, config))
        .collect();
    let comments = comment_columns(&arranged, &rendered, config.align);
    let mut out = String::with_capacity(text.len());
    for ((item, text), column) in arranged.iter().zip(&rendered).zip(comments) {
        out.push_str(text);
        let comment = inline_comment(item);
        if !comment.is_empty() {
            let gap = column.map_or(1, |column| {
                column.saturating_sub(display_width(text)).max(1)
            });
            out.extend(std::iter::repeat_n(' ', gap));
            out.push_str(comment);
        }
        out.push('\n');
    }
    if !config.final_newline {
        out.pop();
    }
    Ok(out)
}

pub fn signature(text: &str) -> Result<Vec<(Class, String, String, String)>, Problem> {
    Ok(parse(text)?
        .iter()
        .filter(|line| line.class != Class::Blank)
        .map(|line| {
            let (class, block, key, value) = line.signature();
            let value = if class == Class::Entry {
                let (value, comment) = assignment_parts(line);
                if comment.is_empty() {
                    value.to_string()
                } else {
                    with_comment(value.to_string(), comment)
                }
            } else {
                value.to_string()
            };
            (class, block.to_string(), key.to_string(), value)
        })
        .collect())
}

impl Problem {
    fn at(line: usize, message: impl Into<String>) -> Problem {
        Problem {
            line,
            message: message.into(),
        }
    }
}

enum Item<'a> {
    Blank,
    Text(&'a Line<'a>),
    Compact(&'a Line<'a>, &'a Line<'a>),
}

fn trim(text: &str) -> &str {
    text.trim_matches([' ', '\t'])
}

fn classify(body: &str) -> (Class, &str, &str) {
    if body.is_empty() {
        return (Class::Blank, "", "");
    }
    if body.starts_with('#') {
        return (Class::Comment, body, "");
    }
    if body == "}" {
        return (Class::Close, "", "");
    }
    if let Some(name) = body.strip_suffix('{') {
        return (Class::Open, trim(name), "");
    }
    // Retain the original value here; assignment_parts separates only a
    // whitespace-delimited, unquoted trailing comment when rendering.
    if let Some((key, value)) = body.split_once('=') {
        return (Class::Entry, trim(key), trim(value));
    }
    (Class::Bare, body, "")
}

fn arrange<'a>(parsed: &'a [Line<'a>], config: &Config) -> Vec<Item<'a>> {
    let mut out = Vec::new();
    let mut pending = 0;
    for line in parsed {
        if line.class == Class::Blank {
            // A blank with nothing above it separates nothing, so it is only
            // counted once something has been written.
            if !out.is_empty() {
                pending += 1;
            }
            continue;
        }
        // The gap above a `}` is the gap at the end of a block, which is not a
        // gap between two things.
        if line.class != Class::Close {
            for _ in 0..pending.min(config.blank_lines) {
                out.push(Item::Blank);
            }
        }
        pending = 0;
        if line.class == Class::Close
            && let Some(Item::Text(opener)) = out.last()
            && opener.class == Class::Open
            && opener.depth == line.depth
            && opener.value.is_empty()
        {
            let opener = *opener;
            out.pop();
            out.push(Item::Compact(opener, line));
        } else {
            out.push(Item::Text(line));
        }
    }
    out
}

fn columns(arranged: &[Item], config: &Config) -> Vec<Option<usize>> {
    let mut widths = vec![None; arranged.len()];
    if !config.align {
        return widths;
    }
    let mut start = 0;
    while start < arranged.len() {
        if !groupable(&arranged[start]) {
            start += 1;
            continue;
        }
        let mut end = start;
        while end < arranged.len() && groupable(&arranged[end]) {
            end += 1;
        }
        // Only entries have a key, so only entries set the width — a group of
        // bare lines is written out as it stands.
        let width = arranged[start..end]
            .iter()
            .filter_map(|item| match item {
                Item::Text(line) if line.class == Class::Entry => {
                    Some(line.key.chars().count().min(config.align_max))
                }
                _ => None,
            })
            .max();
        widths[start..end].fill(width);
        start = end;
    }
    widths
}

fn groupable(item: &Item) -> bool {
    match item {
        Item::Blank | Item::Compact(..) => false,
        Item::Text(line) => {
            line.depth > 0 && matches!(line.class, Class::Entry | Class::Bare | Class::Comment)
        }
    }
}

fn render(item: &Item<'_>, width: Option<usize>, config: &Config) -> String {
    let line = match item {
        Item::Blank => return String::new(),
        Item::Text(line) | Item::Compact(line, _) => line,
    };
    let indent = if line.depth == 0 {
        String::new()
    } else {
        " ".repeat(config.indent * line.depth)
    };
    if matches!(item, Item::Compact(..)) {
        return if line.key.is_empty() {
            format!("{indent}{{}}")
        } else {
            format!("{indent}{} {{}}", line.key)
        };
    }
    match line.class {
        // Always the spaced form. `blocks.py` is read with `open_suffix="{"`
        // everywhere except `packages.py`, which uses `" {"`, and `name {` is
        // the only spelling both readers parse the same way.
        Class::Open if line.key.is_empty() => with_comment(format!("{indent}{{"), line.value),
        Class::Open => with_comment(format!("{indent}{} {{", line.key), line.value),
        Class::Close => with_comment(format!("{indent}}}"), line.value),
        Class::Entry => {
            // The `=` sits two columns past the widest key in the group, and a
            // key at or past the cap takes its one space and overflows rather
            // than dragging the column out after it.
            let pad = match width {
                Some(width) => (width + 2).saturating_sub(line.key.chars().count()).max(1),
                None => 1,
            };
            let mut text = format!("{indent}{}{}=", line.key, " ".repeat(pad));
            let (value, _) = assignment_parts(line);
            if !value.is_empty() {
                // Inserting whitespace before an unseparated leading hash
                // would turn its literal value into a trailing comment.
                if !value.starts_with('#') {
                    text.push(' ');
                }
                text.push_str(value);
            }
            text
        }
        // Interior whitespace is never edited: `config/hosts.dotfile` holds
        // values like `32 GB (2×16 GB) DDR5-6000 CL30`, and collapsing the
        // runs in one would be editing data rather than laying it out.
        _ => format!("{indent}{}", line.body),
    }
}

fn with_comment(mut text: String, comment: &str) -> String {
    if !comment.is_empty() {
        text.push(' ');
        text.push_str(comment);
    }
    text
}

// Inspect the untrimmed assignment tail so `key=#literal` stays data, while
// `key= # comment` is recognized. Track only unescaped layout whitespace:
// `value\  # comment` contains one escaped space that belongs to the value.
// Require a gap after the marker too, keeping part numbers like `board #42`
// and tags like `value #tag` literal even when whitespace precedes their hash.
fn assignment_parts<'a>(line: &Line<'a>) -> (&'a str, &'a str) {
    let Some((_, tail)) = line.body.split_once('=') else {
        return (line.value, "");
    };
    let mut gap = None;
    for token in scan(tail, QuoteMode::Anywhere) {
        if token.character == '#'
            && token.is_structural()
            && tail[token.span.end..]
                .chars()
                .next()
                .is_none_or(char::is_whitespace)
            && let Some(start) = gap
        {
            return (
                tail[..start].trim_start_matches([' ', '\t']),
                &tail[token.span.start..],
            );
        }
        if matches!(token.character, ' ' | '\t') && token.is_structural() {
            gap.get_or_insert(token.span.start);
        } else {
            gap = None;
        }
    }
    (line.value, "")
}

fn inline_comment<'a>(item: &Item<'a>) -> &'a str {
    match item {
        Item::Text(line) if line.class == Class::Entry => assignment_parts(line).1,
        Item::Compact(_, closer) => closer.value,
        _ => "",
    }
}

fn comment_depth(item: &Item<'_>) -> Option<usize> {
    match item {
        Item::Text(line) if matches!(line.class, Class::Entry | Class::Bare | Class::Comment) => {
            Some(line.depth)
        }
        Item::Compact(opener, _) => Some(opener.depth),
        _ => None,
    }
}

fn comment_columns(arranged: &[Item<'_>], rendered: &[String], align: bool) -> Vec<Option<usize>> {
    let mut columns = vec![None; arranged.len()];
    if !align {
        return columns;
    }
    let mut start = 0;
    while start < arranged.len() {
        let Some(depth) = comment_depth(&arranged[start]) else {
            start += 1;
            continue;
        };
        let mut end = start + 1;
        while end < arranged.len() && comment_depth(&arranged[end]) == Some(depth) {
            end += 1;
        }
        let column = (start..end)
            .filter(|&index| !inline_comment(&arranged[index]).is_empty())
            .map(|index| display_width(&rendered[index]) + 1)
            .max();
        columns[start..end].fill(column);
        start = end;
    }
    columns
}

fn display_width(text: &str) -> usize {
    text.split('\t')
        .enumerate()
        .fold(0, |column, (index, part)| {
            let column = if index == 0 {
                column
            } else {
                column + 8 - column % 8
            };
            column + UnicodeWidthStr::width(part)
        })
}
