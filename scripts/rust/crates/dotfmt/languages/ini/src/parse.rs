use crate::dialect::Dialect;

const LAYOUT: [char; 2] = [' ', '\t'];

#[derive(Debug)]
pub struct Document<'a> {
    pub bom: bool,
    pub lines: Vec<Line<'a>>,
}

#[derive(Debug)]
pub struct Line<'a> {
    pub kind: LineKind<'a>,
}

#[derive(Debug)]
pub enum LineKind<'a> {
    Blank,
    Comment(Comment<'a>),
    Section(&'a str),
    Entry(Entry<'a>),
    Continuation(&'a str),
    Raw(&'a str),
}

#[derive(Debug)]
pub struct Comment<'a> {
    pub marker: char,
    pub body: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Separator {
    Equals,
    Space,
}

#[derive(Clone, Copy, Debug)]
pub struct Entry<'a> {
    pub key: &'a str,
    pub value: &'a str,
    pub trailing: &'a str,
    pub separator: Separator,
}

pub fn parse(text: &str, dialect: Dialect) -> Document<'_> {
    let (bom, text) = match text.strip_prefix('\u{feff}') {
        Some(text) => (true, text),
        None => (false, text),
    };
    let mut physical: Vec<&str> = text.split('\n').collect();
    if text.ends_with('\n') {
        let continued = physical.len() >= 2
            && supports_continuation(dialect)
            && continues(physical[physical.len() - 2]);
        if !continued {
            physical.pop();
        }
    }

    let mut lines = Vec::with_capacity(physical.len());
    let mut continuing = false;
    for raw in physical {
        if continuing {
            lines.push(Line {
                kind: LineKind::Continuation(raw),
            });
            continuing = supports_continuation(dialect) && continues(raw);
            continue;
        }
        if raw.trim_matches(LAYOUT).is_empty() {
            lines.push(Line {
                kind: LineKind::Blank,
            });
            continue;
        }
        if dialect == Dialect::Generic && raw.starts_with([' ', '\t']) {
            lines.push(Line {
                kind: LineKind::Continuation(raw),
            });
            continue;
        }
        if let Some(comment) = comment(raw) {
            lines.push(Line {
                kind: LineKind::Comment(comment),
            });
            continue;
        }
        if let Some(header) = section(raw, dialect) {
            lines.push(Line {
                kind: LineKind::Section(header),
            });
            continue;
        }
        if let Some(entry) = entry(raw, dialect) {
            continuing = supports_continuation(dialect) && continues(raw);
            lines.push(Line {
                kind: LineKind::Entry(entry),
            });
            continue;
        }
        lines.push(Line {
            kind: LineKind::Raw(raw),
        });
    }
    Document { bom, lines }
}

fn comment(raw: &str) -> Option<Comment<'_>> {
    let body = raw.trim_start_matches(LAYOUT);
    let marker = body.chars().next().filter(|ch| matches!(ch, '#' | ';'))?;
    Some(Comment {
        marker,
        body: &body[marker.len_utf8()..],
    })
}

fn section(raw: &str, dialect: Dialect) -> Option<&str> {
    let trimmed = raw.trim_matches(LAYOUT);
    if dialect == Dialect::Ssh {
        let word = trimmed.split_whitespace().next()?;
        let keyword = word.split_once('=').map_or(word, |(keyword, _)| keyword);
        if keyword.eq_ignore_ascii_case("host") || keyword.eq_ignore_ascii_case("match") {
            return Some(trimmed);
        }
        return None;
    }
    (trimmed.starts_with('[') && trimmed.ends_with(']')).then_some(trimmed)
}

fn entry<'a>(raw: &'a str, dialect: Dialect) -> Option<Entry<'a>> {
    if dialect == Dialect::Ssh {
        return ssh_entry(raw);
    }
    let at = find_unquoted_equals(raw)?;
    let raw_value = raw[at + 1..].trim_start_matches(LAYOUT);
    let value = raw_value.trim_end_matches(LAYOUT);
    Some(Entry {
        key: raw[..at].trim_matches(LAYOUT),
        value,
        trailing: &raw_value[value.len()..],
        separator: Separator::Equals,
    })
}

fn ssh_entry(raw: &str) -> Option<Entry<'_>> {
    let body = raw.trim_start_matches(LAYOUT);
    if body.is_empty() {
        return None;
    }
    let at = body.find([' ', '\t', '='])?;
    let key = body[..at].trim_matches(LAYOUT);
    if key.is_empty() {
        return None;
    }
    let mut rest = body[at..].trim_start_matches(LAYOUT);
    let equals = rest.starts_with('=');
    if equals {
        rest = &rest[1..];
    }
    let raw_value = rest.trim_start_matches(LAYOUT);
    let value = raw_value.trim_end_matches(LAYOUT);
    let trailing = &raw_value[value.len()..];
    if equals {
        return Some(Entry {
            key,
            value,
            trailing,
            separator: if value.is_empty() {
                Separator::Equals
            } else {
                Separator::Space
            },
        });
    }
    if value.is_empty() {
        return None;
    }
    Some(Entry {
        key,
        value,
        trailing,
        separator: Separator::Space,
    })
}

fn find_unquoted_equals(text: &str) -> Option<usize> {
    let mut quote = None;
    let mut escaped = false;
    for (index, byte) in text.bytes().enumerate() {
        if let Some(current) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == current {
                quote = None;
            }
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = Some(byte),
            b'=' => return Some(index),
            _ => {}
        }
    }
    None
}

fn supports_continuation(dialect: Dialect) -> bool {
    matches!(dialect, Dialect::Generic | Dialect::Git | Dialect::Systemd)
}

fn continues(raw: &str) -> bool {
    raw.as_bytes()
        .iter()
        .rev()
        .take_while(|byte| **byte == b'\\')
        .count()
        % 2
        == 1
}

pub fn trim_layout(text: &str) -> &str {
    text.trim_matches(LAYOUT)
}

pub fn trim_end_layout(text: &str) -> &str {
    text.trim_end_matches(LAYOUT)
}

pub fn comment_body(body: &str, format_comments: bool) -> String {
    let body = body.trim_end_matches(LAYOUT);
    let Some(first) = body.chars().next() else {
        return String::new();
    };
    if format_comments && !first.is_whitespace() && !matches!(first, '#' | ';' | '!') {
        return format!(" {body}");
    }
    body.to_owned()
}
