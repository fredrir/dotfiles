use std::ops::Range;

use crate::edit::{Cut, Kind};
use crate::glyphs;
use crate::scan::{Logical, Out};

pub fn cuts(source: &[u8], out: &Out, aggressive: bool) -> Vec<Cut> {
    let mut found = Vec::new();
    for (index, line) in out.logical.iter().enumerate() {
        let Some(span) = line.sole_text.clone() else {
            continue;
        };
        let owner = if index == 0 {
            None
        } else {
            let previous = &out.logical[index - 1];
            if opens_a_body(source, out, previous) && line.indent > previous.indent {
                Some(previous.indent)
            } else if out.logical[..index]
                .iter()
                .all(|earlier| earlier.sole_text.is_some())
            {
                None
            } else {
                continue;
            }
        };
        let empties = match owner {
            None => out.logical.len() == 1,
            Some(indent) => out
                .logical
                .get(index + 1)
                .is_none_or(|next| next.indent <= indent),
        };
        let with = if empties && !aggressive {
            let delim = delimiter(source, out, &span);
            let Some(sentence) = retained(source, out, &span, delim) else {
                continue;
            };
            Some(format!(
                "{}{delim}{sentence}{delim}{}",
                " ".repeat(line.indent),
                line_ending(source, &span)
            ))
        } else {
            None
        };
        if let Some(replacement) = &with {
            let target = crate::edit::whole_lines(source, &span).unwrap_or_else(|| span.clone());
            if replacement.as_bytes() == &source[target] {
                continue;
            }
        }
        found.push(Cut {
            span,
            with,
            kind: Kind::Doc,
            lines: true,
        });
    }
    found
}

fn opens_a_body(source: &[u8], out: &Out, line: &Logical) -> bool {
    let mut end = line.end;
    for comment in &out.comments {
        if comment.span.start >= line.start && comment.span.start < end {
            end = comment.span.start;
        }
    }
    let text = String::from_utf8_lossy(&source[line.start..end.max(line.start)]);
    if !text.trim_end().ends_with(':') {
        return false;
    }
    let head = text.trim_start();
    head.starts_with("def ") || head.starts_with("async def ") || head.starts_with("class ")
}

fn delimiter(source: &[u8], out: &Out, span: &Range<usize>) -> &'static str {
    for text in &out.texts {
        if text.body.start < span.start || text.body.end > span.end {
            continue;
        }
        let start = text.body.start;
        let Some(&quote) = start.checked_sub(1).and_then(|at| source.get(at)) else {
            break;
        };
        if !matches!(quote, b'"' | b'\'') {
            break;
        }
        let mut count = 0usize;
        while start > count && source[start - count - 1] == quote {
            count += 1;
        }
        return match (quote, count) {
            (b'"', 3..) => "\"\"\"",
            (b'\'', 3..) => "'''",
            (b'"', 1) => "\"",
            (b'\'', 1) => "'",
            _ => "\"\"\"",
        };
    }
    "\"\"\""
}

fn line_ending(source: &[u8], span: &Range<usize>) -> &'static str {
    let Some(widened) = crate::edit::whole_lines(source, span) else {
        return "";
    };
    let end = widened.end;
    if end <= span.end {
        return "";
    }
    if end >= 2 && source[end - 2..end] == *b"\r\n" {
        "\r\n"
    } else if source[end - 1] == b'\n' {
        "\n"
    } else if source[end - 1] == b'\r' {
        "\r"
    } else if end >= 3 && source[end - 3..end] == *"\u{2028}".as_bytes() {
        "\u{2028}"
    } else if end >= 3 && source[end - 3..end] == *"\u{2029}".as_bytes() {
        "\u{2029}"
    } else {
        ""
    }
}

fn retained(source: &[u8], out: &Out, span: &Range<usize>, delim: &str) -> Option<String> {
    let mut joined = String::new();
    for text in &out.texts {
        if text.body.start >= span.start && text.body.end <= span.end {
            joined.push_str(&String::from_utf8_lossy(&source[text.body.clone()]));
        }
    }
    let sentence = glyphs::clean(&first_sentence(&joined));
    let sentence = sentence.split_whitespace().collect::<Vec<_>>().join(" ");
    let quote = delim.as_bytes()[0] as char;
    if sentence.is_empty()
        || sentence.contains(delim)
        || sentence.contains('\\')
        || sentence.ends_with(quote)
    {
        return None;
    }
    Some(sentence)
}

fn first_sentence(content: &str) -> String {
    let trimmed = content.trim();
    for (at, letter) in trimmed.char_indices() {
        if letter != '.' {
            continue;
        }
        let rest = &trimmed[at + 1..];
        if rest.is_empty() || rest.starts_with(char::is_whitespace) {
            return trimmed[..at + 1].to_string();
        }
    }
    if let Some(at) = trimmed.find("\n\n") {
        return trimmed[..at].to_string();
    }
    match trimmed.find('\n') {
        Some(at) => trimmed[..at].to_string(),
        None => trimmed.to_string(),
    }
}
