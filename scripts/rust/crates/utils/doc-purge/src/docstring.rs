use std::ops::Range;

use crate::edit::{Cut, Kind};
use crate::glyphs;
use crate::scan::{Logical, Out};

pub fn cuts(source: &[u8], out: &Out) -> Vec<Cut> {
    let mut found = Vec::new();
    for (index, line) in out.logical.iter().enumerate() {
        let Some(span) = line.sole_text.clone() else {
            continue;
        };
        let owner = if index == 0 {
            None
        } else {
            let previous = &out.logical[index - 1];
            if !opens_a_body(source, out, previous) || line.indent <= previous.indent {
                continue;
            }
            Some(previous.indent)
        };
        if index > 0 && owner.is_none() {
            continue;
        }
        let empties = match owner {
            None => out.logical.len() == 1,
            Some(indent) => out
                .logical
                .get(index + 1)
                .is_none_or(|next| next.indent <= indent),
        };
        let with = if empties {
            let Some(sentence) = retained(source, out, &span) else {
                continue;
            };
            Some(format!(
                "{}\"\"\"{sentence}\"\"\"\n",
                " ".repeat(line.indent)
            ))
        } else {
            None
        };
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

fn retained(source: &[u8], out: &Out, span: &Range<usize>) -> Option<String> {
    let mut joined = String::new();
    for text in &out.texts {
        if text.body.start >= span.start && text.body.end <= span.end {
            joined.push_str(&String::from_utf8_lossy(&source[text.body.clone()]));
        }
    }
    let sentence = glyphs::clean(&first_sentence(&joined));
    let sentence = sentence.split_whitespace().collect::<Vec<_>>().join(" ");
    if sentence.is_empty()
        || sentence.contains("\"\"\"")
        || sentence.contains('\\')
        || sentence.ends_with('"')
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
