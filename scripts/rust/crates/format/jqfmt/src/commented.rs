//! Lossless tokens for JSON with comments and trailing commas.

use crate::parse::{self, Options, Problem};
use crate::render::{Indent, Layout};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Open,
    Close,
    Colon,
    Comma,
    Scalar,
    LineComment,
    BlockComment,
}

impl Kind {
    fn comment(self) -> bool {
        matches!(self, Self::LineComment | Self::BlockComment)
    }
}

struct Token<'a> {
    text: &'a str,
    start: usize,
    kind: Kind,
    new_line: bool,
}

pub fn format(input: &[u8], layout: Layout) -> Result<String, Problem> {
    let text = std::str::from_utf8(input)
        .map_err(|error| problem(input, error.valid_up_to(), "invalid UTF-8"))?;
    let tokens = tokenize(text)?;
    validate(input, &tokens)?;
    Ok(write(&tokens, layout))
}

fn problem(input: &[u8], at: usize, message: &str) -> Problem {
    let head = &input[..at];
    Problem {
        line: 1 + head.iter().filter(|byte| **byte == b'\n').count(),
        column: 1 + head.iter().rev().take_while(|byte| **byte != b'\n').count(),
        message: message.to_string(),
    }
}

fn tokenize(text: &str) -> Result<Vec<Token<'_>>, Problem> {
    let input = text.as_bytes();
    let mut tokens = Vec::new();
    let mut at = 0;
    let mut new_line = false;
    while at < input.len() {
        match input[at] {
            b' ' | b'\t' => {
                at += 1;
                continue;
            }
            b'\r' | b'\n' => {
                new_line = true;
                at += 1;
                continue;
            }
            _ => {}
        }
        let start = at;
        let kind = match input[at] {
            b'{' | b'[' => Kind::Open,
            b'}' | b']' => Kind::Close,
            b':' => Kind::Colon,
            b',' => Kind::Comma,
            b'"' => {
                at += 1;
                while at < input.len() && input[at] != b'"' {
                    if input[at] == b'\\' {
                        at += 1;
                    }
                    at += 1;
                }
                if at >= input.len() {
                    return Err(problem(input, start, "unterminated string"));
                }
                Kind::Scalar
            }
            b'/' if input.get(at + 1) == Some(&b'/') => {
                at += 2;
                while at < input.len() && !matches!(input[at], b'\r' | b'\n') {
                    at += 1;
                }
                tokens.push(Token {
                    text: &text[start..at],
                    start,
                    kind: Kind::LineComment,
                    new_line,
                });
                new_line = false;
                continue;
            }
            b'/' if input.get(at + 1) == Some(&b'*') => {
                at += 2;
                while at + 1 < input.len() && &input[at..at + 2] != b"*/" {
                    at += 1;
                }
                if at + 1 >= input.len() {
                    return Err(problem(input, start, "unterminated comment"));
                }
                at += 1;
                Kind::BlockComment
            }
            _ => {
                while at + 1 < input.len()
                    && !matches!(
                        input[at + 1],
                        b' ' | b'\t'
                            | b'\r'
                            | b'\n'
                            | b'{'
                            | b'}'
                            | b'['
                            | b']'
                            | b':'
                            | b','
                            | b'/'
                            | b'"'
                    )
                {
                    at += 1;
                }
                Kind::Scalar
            }
        };
        at += 1;
        tokens.push(Token {
            text: &text[start..at],
            start,
            kind,
            new_line,
        });
        new_line = false;
    }
    Ok(tokens)
}

fn validate(input: &[u8], tokens: &[Token<'_>]) -> Result<(), Problem> {
    // Keep byte offsets intact so diagnostics still point into the source.
    let mut json = input.to_vec();
    let mut previous = None;
    let mut comma = None;
    for token in tokens {
        let start = token.start;
        if token.kind.comment() {
            for byte in &mut json[start..start + token.text.len()] {
                if !matches!(byte, b'\r' | b'\n') {
                    *byte = b' ';
                }
            }
            continue;
        }
        if token.kind == Kind::Close {
            if let Some(at) = comma.take() {
                json[at] = b' ';
            }
        } else {
            comma = if token.kind == Kind::Comma
                && matches!(previous, Some(Kind::Scalar | Kind::Close))
            {
                Some(start)
            } else {
                None
            };
        }
        if token.kind == Kind::Scalar
            && token
                .text
                .as_bytes()
                .first()
                .is_some_and(|byte| matches!(byte, b'-' | b'+' | b'.' | b'0'..=b'9'))
            && !json_number(token.text.as_bytes())
        {
            return Err(problem(input, start, "invalid JSON number"));
        }
        previous = Some(token.kind);
    }
    let parsed = parse::parse(&json, Options::default())?;
    if parsed.value.is_none() && !input.is_empty() {
        return Err(problem(input, 0, "expected a value"));
    }
    Ok(())
}

fn json_number(mut bytes: &[u8]) -> bool {
    if bytes.first() == Some(&b'-') {
        bytes = &bytes[1..];
    }
    match bytes.first() {
        Some(b'0') => bytes = &bytes[1..],
        Some(b'1'..=b'9') => {
            bytes = &bytes[bytes
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .count()..];
        }
        _ => return false,
    }
    if bytes.first() == Some(&b'.') {
        bytes = &bytes[1..];
        let count = bytes
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if count == 0 {
            return false;
        }
        bytes = &bytes[count..];
    }
    if matches!(bytes.first(), Some(b'e' | b'E')) {
        bytes = &bytes[1..];
        if matches!(bytes.first(), Some(b'+' | b'-')) {
            bytes = &bytes[1..];
        }
        let count = bytes
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if count == 0 {
            return false;
        }
        bytes = &bytes[count..];
    }
    bytes.is_empty()
}

fn write(tokens: &[Token<'_>], layout: Layout) -> String {
    let mut out = String::new();
    let mut depth: usize = 0;
    let mut previous: Option<Kind> = None;
    let pretty = layout.indent != Indent::Compact;
    let mut after_comma = false;
    for token in tokens {
        if token.kind == Kind::Close {
            depth = depth.saturating_sub(1);
        }
        let line = previous == Some(Kind::LineComment)
            || (pretty
                && (previous == Some(Kind::Open) && token.kind != Kind::Close
                    || after_comma && (!token.kind.comment() || token.new_line)
                    || token.kind == Kind::Close && previous != Some(Kind::Open)
                    || token.kind.comment() && token.new_line
                    || previous == Some(Kind::BlockComment) && token.new_line));
        if line && !out.is_empty() {
            out.push('\n');
            match layout.indent {
                Indent::Spaces(width) => out.push_str(&" ".repeat(width * depth)),
                Indent::Tabs => out.push_str(&"\t".repeat(depth)),
                Indent::Compact => {}
            }
        } else if !out.is_empty()
            && (token.kind.comment()
                || previous.is_some_and(Kind::comment)
                || pretty && previous == Some(Kind::Colon))
        {
            out.push(' ');
        }
        if line || !token.kind.comment() {
            after_comma = token.kind == Kind::Comma;
        }
        out.push_str(token.text);
        if token.kind == Kind::Open {
            depth += 1;
        }
        previous = Some(token.kind);
    }
    if layout.final_newline && !out.is_empty() {
        out.push('\n');
    }
    out
}
