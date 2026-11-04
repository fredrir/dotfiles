use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};

use crate::model::{Document, Entry, Item, Value};

pub(crate) fn parse(text: &str) -> Result<Document<'_>, Diagnostic> {
    let bytes = text.as_bytes();
    let mut items: Vec<Item<'_>> = Vec::new();
    let mut pos = 0;
    // A shell joins a backslash-newline pair, so `#` on the next physical
    // line may be part of the previous word instead of a comment. Such a line
    // stays verbatim: dropping its indentation or spacing its `#` would
    // change the joined word.
    let mut continued = false;
    while pos < text.len() {
        let (end, next) = line_bounds(text, pos);
        let line = &text[pos..end];
        if is_blank(line) {
            if continued {
                items.push(Item::Blank {
                    count: 1,
                    protected: true,
                });
            } else {
                match items.last_mut() {
                    Some(Item::Blank { count, .. }) => *count += 1,
                    _ => items.push(Item::Blank {
                        count: 1,
                        protected: false,
                    }),
                }
            }
            continued = false;
            pos = next;
            continue;
        }
        let first = pos + (line.len() - line.trim_start_matches([' ', '\t']).len());
        if bytes[first] == b'#' {
            let comment = &text[first..end];
            reject_bare_cr(text, comment, first)?;
            if continued {
                items.push(Item::Verbatim { raw: line });
            } else {
                items.push(Item::Comment { text: comment });
            }
            continued = ends_with_continuation(line);
            pos = next;
            continue;
        }
        if let Some((entry, after)) = try_entry(text, pos, end, next)? {
            continued = last_line_ends_with_continuation(text, pos, after);
            items.push(Item::Entry(entry));
            pos = after;
            continue;
        }
        reject_bare_cr(text, line, pos)?;
        items.push(Item::Verbatim { raw: line });
        continued = ends_with_continuation(line);
        pos = next;
    }
    Ok(Document { items })
}

fn is_blank(line: &str) -> bool {
    line.bytes().all(|byte| matches!(byte, b' ' | b'\t'))
}

fn ends_with_continuation(line: &str) -> bool {
    line.bytes().rev().take_while(|byte| *byte == b'\\').count() % 2 == 1
}

fn last_line_ends_with_continuation(text: &str, start: usize, after: usize) -> bool {
    let mut end = after;
    if end > start && text.as_bytes()[end - 1] == b'\n' {
        end -= 1;
        if end > start && text.as_bytes()[end - 1] == b'\r' {
            end -= 1;
        }
    }
    let line_start = text[start..end]
        .rfind('\n')
        .map_or(start, |offset| start + offset + 1);
    ends_with_continuation(&text[line_start..end])
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
    let before_eq_start = at;
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
    let before_eq_gap = &text[before_eq_start..at];
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
                before_eq_gap,
                after_eq_gap: "",
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
                before_eq_gap,
                after_eq_gap: "",
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
            before_eq_gap,
            after_eq_gap: after_eq,
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
        if bytes[index] == b'#'
            && index > at
            && matches!(bytes[index - 1], b' ' | b'\t')
            && !is_escaped(bytes, index - 1)
        {
            comment_start = Some(index);
            break;
        }
        index += 1;
    }

    let value_end = comment_start.unwrap_or(end);
    let raw_end = unescaped_value_end(bytes, at, value_end);
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

/// Whether the byte at `index` is preceded by an odd number of backslashes.
/// In a bare value, that makes the byte part of the word rather than layout.
fn is_escaped(bytes: &[u8], index: usize) -> bool {
    let mut backslashes = 0;
    let mut cursor = index;
    while cursor > 0 && bytes[cursor - 1] == b'\\' {
        backslashes += 1;
        cursor -= 1;
    }
    backslashes % 2 == 1
}

/// End of the value after dropping trailing layout whitespace. A first
/// whitespace byte escaped by a preceding backslash belongs to the value, so
/// it (and anything before it) stays even when the rest of the run is layout.
fn unescaped_value_end(bytes: &[u8], at: usize, end: usize) -> usize {
    let mut start = end;
    while start > at && matches!(bytes[start - 1], b' ' | b'\t') {
        start -= 1;
    }
    if start < end && is_escaped(bytes, start) {
        start + 1
    } else {
        start
    }
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
