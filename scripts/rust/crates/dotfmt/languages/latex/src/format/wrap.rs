use crate::config::Config;

const TABLES: &[&str] = &[
    "tabular",
    "tabular*",
    "tabularx",
    "tabulary",
    "longtable",
    "array",
    "matrix",
    "pmatrix",
    "bmatrix",
    "Bmatrix",
    "vmatrix",
    "Vmatrix",
    "cases",
    "align",
    "align*",
    "aligned",
    "split",
    "eqnarray",
    "eqnarray*",
    "alignat",
    "alignat*",
    "gather",
    "gather*",
    "multline",
    "multline*",
];

pub fn wrap_text(text: &str, config: &Config, marker: &str) -> String {
    let mut output: Vec<String> = Vec::new();
    let mut ignore = false;
    let mut table_depth = 0usize;
    for line in text.split('\n') {
        let tail = line.trim_end();
        let off = tail.ends_with("% tex-fmt: off");
        let on = tail.ends_with("% tex-fmt: on");
        let skip = tail.ends_with("% tex-fmt: skip");
        if ignore || off || on || skip || line.trim() == marker {
            output.push(line.to_owned());
        } else {
            let code = &line[..comment_index(line.as_bytes())];
            if table_depth == 0 {
                output.extend(wrap_line(line, config));
            } else {
                output.push(line.to_owned());
            }
            let (begins, ends) = environment_events(code);
            table_depth = table_depth.saturating_add(begins).saturating_sub(ends);
        }
        if off {
            ignore = true;
        } else if on {
            ignore = false;
        }
    }
    output.join("\n")
}

fn wrap_line(line: &str, config: &Config) -> Vec<String> {
    let indent_end = line.len() - line.trim_start_matches([' ', '\t']).len();
    let indent = &line[..indent_end];
    let body = &line[indent_end..];
    if body.is_empty() {
        return vec![line.to_owned()];
    }
    let indent_width = indent.chars().count();
    if indent_width + body.chars().count() <= config.width {
        return vec![line.to_owned()];
    }
    let comment = comment_index(body.as_bytes());
    let code = &body[..comment];
    let comment = &body[comment..];
    if !comment.is_empty() && (!code.trim().is_empty() || is_tooling_directive(comment)) {
        return vec![line.to_owned()];
    }
    let code_width = indent_width + code.chars().count();
    let mut lines = Vec::new();
    if code_width > config.width {
        let (first, rest) = wrap_body(indent, code, config);
        let mut first_line = first;
        if !comment.is_empty() {
            first_line.push_str(comment);
        }
        lines.push(first_line);
        if !rest.is_empty() {
            lines.extend(wrap_remaining(indent, rest, config));
        }
    } else if !comment.is_empty() {
        lines.extend(wrap_comment(indent, code, comment, config));
    } else {
        lines.extend(wrap_remaining(indent, code, config));
    }
    if lines.is_empty() {
        lines.push(line.to_owned());
    }
    lines
}

/// Whether a comment line is a tooling directive that only works unbroken:
/// a `%!TEX`/`% !BIB` magic comment, an `% arara:` recipe, or a `%&` format
/// line.
fn is_tooling_directive(comment: &str) -> bool {
    let trimmed = comment.trim_start();
    if trimmed.starts_with("%&") {
        return true;
    }
    let content = trimmed
        .trim_start_matches('%')
        .trim_start_matches([' ', '\t']);
    content.starts_with('!') || content.starts_with("arara:")
}

fn wrap_remaining(indent: &str, body: &str, config: &Config) -> Vec<String> {
    let mut lines = Vec::new();
    let mut rest = body.trim_start();
    loop {
        if indent.chars().count() + rest.chars().count() <= config.width {
            lines.push(format!("{indent}{rest}"));
            break;
        }
        let limit = config.width.saturating_sub(indent.chars().count()).max(1);
        let Some((prefix, suffix)) = split_break(rest, limit, config) else {
            lines.push(format!("{indent}{rest}"));
            break;
        };
        lines.push(format!("{indent}{prefix}"));
        rest = suffix.trim_start();
        if rest.is_empty() {
            break;
        }
    }
    lines
}

fn wrap_body<'a>(indent: &str, body: &'a str, config: &Config) -> (String, &'a str) {
    let limit = config.width.saturating_sub(indent.chars().count()).max(1);
    if let Some((prefix, suffix)) = split_break(body, limit, config) {
        (format!("{indent}{prefix}"), suffix)
    } else {
        (format!("{indent}{body}"), "")
    }
}

fn wrap_comment(indent: &str, code: &str, comment: &str, config: &Config) -> Vec<String> {
    let content = comment.strip_prefix('%').unwrap_or(comment).trim_start();
    let first_prefix = if code.is_empty() {
        format!("{indent}% ")
    } else {
        format!("{indent}{code} % ")
    };
    let first_available = config
        .width
        .saturating_sub(first_prefix.chars().count())
        .max(1);
    let continuation = format!("{indent}% ");
    let continuation_available = config
        .width
        .saturating_sub(continuation.chars().count())
        .max(1);
    let mut lines = Vec::new();
    let mut rest = content;
    let mut first = true;
    loop {
        let limit = if first {
            first_available
        } else {
            continuation_available
        };
        let Some((prefix, suffix)) = split_break(rest, limit, config) else {
            let line = if first {
                format!("{first_prefix}{rest}")
            } else {
                format!("{continuation}{rest}")
            };
            lines.push(line);
            break;
        };
        if first {
            lines.push(format!("{first_prefix}{prefix}"));
            first = false;
        } else {
            lines.push(format!("{continuation}{prefix}"));
        }
        rest = suffix.trim_start();
        if rest.is_empty() {
            break;
        }
    }
    lines
}

fn split_break<'a>(text: &'a str, limit: usize, config: &Config) -> Option<(String, &'a str)> {
    let minimum = config.wrap_min().min(limit);
    let mut previous = None;
    let mut candidate = None;
    let mut preferred = None;
    for (index, (byte, character)) in text.char_indices().enumerate() {
        if index >= limit {
            break;
        }
        if config.wrap_chars.contains(&character)
            && character != '\\'
            && previous != Some('\\')
            && index > 0
        {
            candidate = Some(byte);
            if index >= minimum {
                preferred = Some(byte);
            }
        }
        previous = Some(character);
    }
    let index = preferred.or(candidate)?;
    let character = text[index..].chars().next()?;
    let after = index + character.len_utf8();
    let prefix = if character == ' ' {
        text[..index].trim_end().to_owned()
    } else {
        text[..after].to_owned()
    };
    Some((prefix, &text[after..]))
}

fn comment_index(bytes: &[u8]) -> usize {
    for (position, byte) in bytes.iter().enumerate() {
        if *byte == b'%' && !escaped(bytes, position) {
            return position;
        }
    }
    bytes.len()
}

fn escaped(bytes: &[u8], position: usize) -> bool {
    let mut count = 0;
    let mut i = position;
    while i > 0 && bytes[i - 1] == b'\\' {
        count += 1;
        i -= 1;
    }
    count % 2 == 1
}

fn environment_events(code: &str) -> (usize, usize) {
    let bytes = code.as_bytes();
    let mut begins = 0;
    let mut ends = 0;
    for (position, byte) in bytes.iter().enumerate() {
        if *byte != b'\\' || !command_starts(bytes, position) {
            continue;
        }
        let (begin, rest) = if code[position..].starts_with("\\begin") {
            (true, position + 6)
        } else if code[position..].starts_with("\\end") {
            (false, position + 4)
        } else {
            continue;
        };
        let mut j = rest;
        while code.as_bytes().get(j) == Some(&b' ') || code.as_bytes().get(j) == Some(&b'\t') {
            j += 1;
        }
        if code.as_bytes().get(j) != Some(&b'{') {
            continue;
        }
        if let Some(close) = code[j + 1..].find('}') {
            let name = &code[j + 1..j + 1 + close];
            if TABLES.contains(&name) {
                if begin {
                    begins += 1;
                } else {
                    ends += 1;
                }
            }
        }
    }
    (begins, ends)
}

fn command_starts(bytes: &[u8], position: usize) -> bool {
    let mut count = 0;
    let mut i = position + 1;
    while i > 0 && bytes[i - 1] == b'\\' {
        count += 1;
        i -= 1;
    }
    count % 2 == 1
}
