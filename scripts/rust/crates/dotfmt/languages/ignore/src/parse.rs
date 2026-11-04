//! A lossless line model for ignore files.
//!
//! A line is split into byte slices; pattern and attribute bytes are never
//! decoded or rewritten. Trailing whitespace is kept separate because the
//! dialect decides whether it belongs to the pattern.

use crate::dialect::Dialect;

#[derive(Debug)]
pub struct Document<'a> {
    pub lines: Vec<Line<'a>>,
}

#[derive(Debug)]
pub enum Line<'a> {
    Blank,
    Comment {
        indent: &'a str,
        body: &'a str,
        trailing: &'a str,
    },
    Pattern {
        body: &'a str,
        trailing: &'a str,
    },
    Attributes {
        indent: &'a str,
        pattern: &'a str,
        attrs: Vec<&'a str>,
        trailing: &'a str,
    },
    /// A line whose fields cannot be split safely. It is emitted byte for
    /// byte and contributes an opaque fingerprint item.
    Opaque(&'a str),
}

pub fn parse(text: &str, dialect: Dialect) -> Document<'_> {
    let dialect = dialect.base();
    let mut lines = Vec::new();
    if text.is_empty() {
        return Document { lines };
    }
    let mut raw: Vec<&str> = text.split('\n').collect();
    if text.ends_with('\n') {
        raw.pop();
    }
    for line in raw {
        lines.push(match dialect {
            Dialect::Attributes => parse_attributes(line),
            _ => parse_ignore_like(line, dialect),
        });
    }
    Document { lines }
}

fn parse_ignore_like(line: &str, dialect: Dialect) -> Line<'_> {
    if blank_ignore_like(line, dialect) {
        return Line::Blank;
    }
    let trim = if trims_tabs(dialect) {
        Trim::Whitespace
    } else {
        Trim::Spaces
    };
    if line.starts_with('#') {
        return match split_trailing(line, trim) {
            Split::Content { content, trailing } => Line::Comment {
                indent: "",
                body: content,
                trailing,
            },
            Split::Escaped => Line::Opaque(line),
        };
    }
    match split_trailing(line, trim) {
        Split::Content { content, trailing } => Line::Pattern {
            body: content,
            trailing,
        },
        Split::Escaped => Line::Opaque(line),
    }
}

fn parse_attributes(line: &str) -> Line<'_> {
    if line.bytes().all(|byte| matches!(byte, b' ' | b'\t')) {
        return Line::Blank;
    }
    let Split::Content { content, trailing } = split_trailing(line, Trim::Whitespace) else {
        return Line::Opaque(line);
    };
    let rest = content.trim_start_matches([' ', '\t']);
    let indent = &content[..content.len() - rest.len()];
    if rest.starts_with('#') {
        return Line::Comment {
            indent,
            body: rest,
            trailing,
        };
    }
    if rest.starts_with('"') {
        let Some(end) = quoted_end(rest) else {
            return Line::Opaque(line);
        };
        let pattern = &rest[..end];
        let after = &rest[end..];
        if !after.is_empty() && !starts_with_space(after) {
            return Line::Opaque(line);
        }
        return Line::Attributes {
            indent,
            pattern,
            attrs: split_attrs(after),
            trailing,
        };
    }
    let (pattern, after) = match rest.find([' ', '\t']) {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, ""),
    };
    // Git's handling of backslashes inside a bare attribute pattern is not
    // defined well enough to normalise the spacing after it.
    if pattern.ends_with('\\') {
        return Line::Opaque(line);
    }
    Line::Attributes {
        indent,
        pattern,
        attrs: split_attrs(after),
        trailing,
    }
}

fn blank_ignore_like(line: &str, dialect: Dialect) -> bool {
    if trims_tabs(dialect) {
        line.bytes().all(|byte| matches!(byte, b' ' | b'\t'))
    } else {
        line.bytes().all(|byte| byte == b' ')
    }
}

/// `ignore` and Prettier trim tabs as well as spaces. Git only trims spaces;
/// VS Code is treated as Git-like, so a line it may consider a pattern is
/// never discarded.
fn trims_tabs(dialect: Dialect) -> bool {
    matches!(dialect, Dialect::Ignore | Dialect::Prettier)
}

fn starts_with_space(text: &str) -> bool {
    text.starts_with(' ') || text.starts_with('\t')
}

fn split_attrs(text: &str) -> Vec<&str> {
    text.split([' ', '\t'])
        .filter(|attribute| !attribute.is_empty())
        .collect()
}

fn quoted_end(pattern: &str) -> Option<usize> {
    let bytes = pattern.as_bytes();
    let mut index = 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'"' => return Some(index + 1),
            _ => index += 1,
        }
    }
    None
}

#[derive(Clone, Copy)]
pub(crate) enum Trim {
    Spaces,
    Whitespace,
}

pub(crate) enum Split<'a> {
    Content {
        content: &'a str,
        trailing: &'a str,
    },
    /// The trailing run is protected by a backslash. Removing it could change
    /// the pattern in at least one dialect, so the line stays byte-exact.
    Escaped,
}

pub(crate) fn split_trailing<'a>(line: &'a str, trim: Trim) -> Split<'a> {
    let bytes = line.as_bytes();
    let mut end = bytes.len();
    while end > 0 {
        let byte = bytes[end - 1];
        let trimmable = match trim {
            Trim::Spaces => byte == b' ',
            Trim::Whitespace => byte == b' ' || byte == b'\t',
        };
        if !trimmable {
            break;
        }
        end -= 1;
    }
    if end == bytes.len() {
        return Split::Content {
            content: line,
            trailing: "",
        };
    }
    if end > 0 && bytes[end - 1] == b'\\' {
        return Split::Escaped;
    }
    Split::Content {
        content: &line[..end],
        trailing: &line[end..],
    }
}
