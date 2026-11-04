use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};

use crate::model::{Document, Entry, Item, Value};

pub(crate) fn parse(text: &str) -> Result<Document<'_>, Diagnostic> {
    let bytes = text.as_bytes();
    let mut items: Vec<Item<'_>> = Vec::new();
    let mut pos = 0;
    while pos < text.len() {
        let (end, next) = line_bounds(text, pos);
        let line = &text[pos..end];
        if is_blank(line) {
            match items.last_mut() {
                Some(Item::Blank { count }) => *count += 1,
                _ => items.push(Item::Blank { count: 1 }),
            }
            pos = next;
            continue;
        }
        let first = pos + (line.len() - line.trim_start_matches([' ', '\t']).len());
        if bytes[first] == b'#' {
            let comment = &text[first..end];
            reject_bare_cr(text, comment, first)?;
            items.push(Item::Comment { text: comment });
            pos = next;
            continue;
        }
        if let Some((entry, after)) = try_entry(text, pos, end, next)? {
            items.push(Item::Entry(entry));
            pos = after;
            continue;
        }
        reject_bare_cr(text, line, pos)?;
        items.push(Item::Verbatim { raw: line });
        pos = next;
    }
    Ok(Document { items })
}

fn is_blank(line: &str) -> bool {
    line.bytes().all(|byte| matches!(byte, b' ' | b'\t'))
}

fn line_bounds(text: &str, from: usize) -> (usize, usize) {
    match text[from..].find('\n') {
        Some(offset) => {
            let newline = from + offset;
            let end = if newline > from && text.as_bytes()[newline - 1] == b'\r' {
                newline - 1
            } else {
                newline
            };
            (end, newline + 1)
        }
        None => (text.len(), text.len()),
    }
}

/// Returns the entry and the offset just past its last line, or `None` when
/// the line is not shaped like an entry at all and must stay verbatim.
fn try_entry<'a>(
    text: &'a str,
    pos: usize,
    end: usize,
    next: usize,
) -> Result<Option<(Entry<'a>, usize)>, Diagnostic> {
    let bytes = text.as_bytes();
    let lead_start = pos;
    let mut at = pos;
    while at < end && matches!(bytes[at], b' ' | b'\t') {
        at += 1;
    }
    let lead_ws = &text[lead_start..at];

    let mut export = None;
    let mut export_gap = "";
    if text[at..end].starts_with("export") {
        let after = at + "export".len();
        if after < end && matches!(bytes[after], b' ' | b'\t') {
            export = Some(&text[at..after]);
            let gap_start = after;
            at = after;
            while at < end && matches!(bytes[at], b' ' | b'\t') {
                at += 1;
            }
            export_gap = &text[gap_start..at];
        }
    }

    let key_start = at;
    while at < end && !matches!(bytes[at], b'=' | b' ' | b'\t' | b'\r' | b'\n') {
        at += 1;
    }
    if at == key_start {
        return Ok(None);
    }
    let key = &text[key_start..at];
    if at == end {
        return Ok(None);
    }
    if bytes[at] != b'=' {
        if !matches!(bytes[at], b' ' | b'\t') {
            return Ok(None);
        }
        while at < end && matches!(bytes[at], b' ' | b'\t') {
            at += 1;
        }
        if at == end || bytes[at] != b'=' {
            return Ok(None);
        }
    }
    at += 1;

    let gap_start = at;
    while at < end && matches!(bytes[at], b' ' | b'\t') {
        at += 1;
    }
    let after_eq = &text[gap_start..at];
    if at == end {
        return Ok(Some((
            Entry {
                lead_ws,
                export,
                export_gap,
                key,
                value: Value::Bare(""),
                trailing_gap: after_eq,
                comment: None,
            },
            next,
        )));
    }

    let first = bytes[at];
    if first == b'#' && !after_eq.is_empty() {
        let comment = &text[at..end];
        reject_bare_cr(text, comment, at)?;
        return Ok(Some((
            Entry {
                lead_ws,
                export,
                export_gap,
                key,
                value: Value::Bare(""),
                trailing_gap: after_eq,
                comment: Some(comment),
            },
            next,
        )));
    }

    let (value, trailing_gap, comment, after) = match first {
        b'\'' => quoted(text, at, end, next, b'\'', false)?,
        b'"' => quoted_double(text, at)?,
        b'`' => quoted(text, at, end, next, b'`', true)?,
        _ => bare(text, at, end, next)?,
    };
    Ok(Some((
        Entry {
            lead_ws,
            export,
            export_gap,
            key,
            value,
            trailing_gap,
            comment,
        },
        after,
    )))
}

fn quoted<'a>(
    text: &'a str,
    at: usize,
    end: usize,
    next: usize,
    quote: u8,
    escapes: bool,
) -> Result<(Value<'a>, &'a str, Option<&'a str>, usize), Diagnostic> {
    let bytes = text.as_bytes();
    let mut index = at + 1;
    let mut close = None;
    while index < end {
        if escapes && bytes[index] == b'\\' {
            index += 2;
            continue;
        }
        if bytes[index] == quote {
            close = Some(index);
            break;
        }
        index += 1;
    }
    let Some(close) = close else {
        return Err(syntax(text, at, "unterminated quoted value"));
    };

    let value = if quote == b'\'' {
        Value::Single(&text[at..=close])
    } else {
        Value::Backtick(&text[at..=close])
    };
    let mut after = close + 1;
    let gap_start = after;
    while after < end && matches!(bytes[after], b' ' | b'\t') {
        after += 1;
    }
    let gap = &text[gap_start..after];
    if after == end {
        return Ok((value, gap, None, next));
    }
    if bytes[after] == b'#' {
        if gap.is_empty() {
            return Err(syntax(
                text,
                after,
                "a trailing comment must be separated from the value by whitespace",
            ));
        }
        let comment = &text[after..end];
        reject_bare_cr(text, comment, after)?;
        return Ok((value, gap, Some(comment), next));
    }
    Err(syntax(text, after, "unexpected text after a quoted value"))
}

fn quoted_double<'a>(
    text: &'a str,
    at: usize,
) -> Result<(Value<'a>, &'a str, Option<&'a str>, usize), Diagnostic> {
    let bytes = text.as_bytes();
    let mut index = at + 1;
    let close = loop {
        if index >= text.len() {
            return Err(syntax(text, at, "unterminated double-quoted value"));
        }
        match bytes[index] {
            b'\\' => index += 2,
            b'"' => break index,
            _ => index += 1,
        }
    };
    let value = Value::Double(&text[at..=close]);

    let mut after = close + 1;
    let gap_start = after;
    while after < text.len() && matches!(bytes[after], b' ' | b'\t') {
        after += 1;
    }
    let gap = &text[gap_start..after];
    if after == text.len() {
        return Ok((value, gap, None, text.len()));
    }
    match bytes[after] {
        b'\n' => Ok((value, gap, None, after + 1)),
        b'\r' if bytes.get(after + 1) == Some(&b'\n') => Ok((value, gap, None, after + 2)),
        b'#' if !gap.is_empty() => {
            let (end, next) = line_bounds(text, after);
            let comment = &text[after..end];
            reject_bare_cr(text, comment, after)?;
            Ok((value, gap, Some(comment), next))
        }
        b'#' => Err(syntax(
            text,
            after,
            "a trailing comment must be separated from the value by whitespace",
        )),
        _ => Err(syntax(
            text,
            after,
            "unexpected text after a double-quoted value",
        )),
    }
}

fn bare<'a>(
    text: &'a str,
    at: usize,
    end: usize,
    next: usize,
) -> Result<(Value<'a>, &'a str, Option<&'a str>, usize), Diagnostic> {
    let bytes = text.as_bytes();
    let mut index = at;
    let mut comment_start = None;
    while index < end {
        if bytes[index] == b'\r' {
            return Err(syntax(
                text,
                index,
                "a carriage return outside a double-quoted value must be followed by a line feed",
            ));
        }
        if bytes[index] == b'#' && index > at && matches!(bytes[index - 1], b' ' | b'\t') {
            comment_start = Some(index);
            break;
        }
        index += 1;
    }

    let value_end = comment_start.unwrap_or(end);
    let mut raw_end = value_end;
    while raw_end > at && matches!(bytes[raw_end - 1], b' ' | b'\t') {
        raw_end -= 1;
    }
    let value = Value::Bare(&text[at..raw_end]);
    let gap = &text[raw_end..value_end];
    let comment = match comment_start {
        Some(start) => {
            let comment = &text[start..end];
            reject_bare_cr(text, comment, start)?;
            Some(comment)
        }
        None => None,
    };
    Ok((value, gap, comment, next))
}

fn reject_bare_cr(full: &str, text: &str, start: usize) -> Result<(), Diagnostic> {
    match text.find('\r') {
        Some(at) => Err(syntax(
            full,
            start + at,
            "a carriage return outside a double-quoted value must be followed by a line feed",
        )),
        None => Ok(()),
    }
}

fn syntax(text: &str, offset: usize, message: &str) -> Diagnostic {
    let before = text.get(..offset).unwrap_or(text);
    let line = before.matches('\n').count() + 1;
    let column = before
        .rsplit('\n')
        .next()
        .map_or(1, |line| line.chars().count() + 1);
    Diagnostic::new(DiagnosticKind::Syntax, message).with_location(line, Some(column))
}
