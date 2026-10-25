#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Field(String),
    Dot,
    Recurse,
    Var(String),
    Ident(String),
    Format(String),
    Str(Part),
    Num,
    Punct(&'static str),
    Comment,
}

// A string is split around each `\(...)` interpolation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Whole,
    Start,
    Middle,
    End,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: Kind,
    pub start: usize,
    pub end: usize,
    // A string still open where the text ends.
    pub open: bool,
}

const KEYWORDS: [&str; 16] = [
    "def", "if", "then", "elif", "else", "end", "as", "reduce", "foreach", "try", "catch", "label",
    "import", "include", "and", "or",
];
const OPERATORS: [&str; 32] = [
    "?//", "//=", "|=", "+=", "-=", "*=", "/=", "%=", "==", "!=", "<=", ">=", "//", "[", "]", "{",
    "}", "(", ")", "|", ",", ":", ";", "=", "<", ">", "+", "-", "*", "/", "%", "?",
];

impl Token {
    pub fn ident(&self) -> Option<&str> {
        match &self.kind {
            Kind::Ident(name) => Some(name),
            _ => None,
        }
    }

    pub fn is_ident(&self, name: &str) -> bool {
        self.ident() == Some(name)
    }

    pub fn is_punct(&self, punct: &str) -> bool {
        matches!(&self.kind, Kind::Punct(known) if *known == punct)
    }

    // Whether a postfix such as `.name` or `[0]` can follow.
    pub fn ends_term(&self) -> bool {
        match &self.kind {
            Kind::Field(_) | Kind::Dot | Kind::Recurse | Kind::Var(_) | Kind::Format(_) => true,
            Kind::Num => true,
            Kind::Ident(name) => !is_keyword(name),
            Kind::Str(Part::Whole | Part::End) => !self.open,
            Kind::Punct(punct) => matches!(*punct, ")" | "]" | "}" | "?"),
            _ => false,
        }
    }
}

pub fn is_keyword(name: &str) -> bool {
    KEYWORDS.contains(&name)
}

pub fn lex(text: &str) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    // Open parentheses inside each unfinished interpolation.
    let mut interpolations: Vec<usize> = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let start = at;
        let byte = bytes[at];
        let kind = match byte {
            b' ' | b'\t' | b'\n' | b'\r' => {
                at += 1;
                continue;
            }
            b'#' => {
                at = text[at..].find('\n').map_or(bytes.len(), |end| at + end);
                Kind::Comment
            }
            b'"' => {
                let (token, nested) = string(text, at, at + 1, Part::Whole);
                at = token.end;
                if nested {
                    interpolations.push(0);
                }
                tokens.push(token);
                continue;
            }
            b'.' if bytes.get(at + 1) == Some(&b'.') => {
                at += 2;
                Kind::Recurse
            }
            b'.' if bytes.get(at + 1).is_some_and(u8::is_ascii_digit) => {
                at = number(bytes, at + 1);
                Kind::Num
            }
            b'.' if bytes.get(at + 1).is_some_and(|next| ident_start(*next)) => {
                at = ident_end(bytes, at + 1, false);
                Kind::Field(text[start + 1..at].to_string())
            }
            b'.' => {
                at += 1;
                Kind::Dot
            }
            b'$' => {
                at = ident_end(bytes, at + 1, true);
                Kind::Var(text[start + 1..at].to_string())
            }
            b'@' => {
                at = ident_end(bytes, at + 1, false);
                Kind::Format(text[start + 1..at].to_string())
            }
            _ if ident_start(byte) => {
                at = ident_end(bytes, at, true);
                Kind::Ident(text[start..at].to_string())
            }
            _ if byte.is_ascii_digit() => {
                at = number(bytes, at);
                Kind::Num
            }
            b')' if interpolations.last() == Some(&0) => {
                interpolations.pop();
                let (token, nested) = string(text, at, at + 1, Part::End);
                at = token.end;
                if nested {
                    interpolations.push(0);
                }
                tokens.push(token);
                continue;
            }
            _ => {
                let Some(punct) = OPERATORS
                    .iter()
                    .find(|punct| text[at..].starts_with(**punct))
                else {
                    at += text[at..].chars().next().map_or(1, char::len_utf8);
                    continue;
                };
                at += punct.len();
                if let Some(depth) = interpolations.last_mut() {
                    match *punct {
                        "(" => *depth += 1,
                        ")" => *depth = depth.saturating_sub(1),
                        _ => {}
                    }
                }
                Kind::Punct(punct)
            }
        };
        tokens.push(Token {
            kind,
            start,
            end: at,
            open: false,
        });
    }
    tokens
}

// The string piece from `start`, scanning its body from `body`; true when it opens an interpolation.
fn string(text: &str, start: usize, body: usize, first: Part) -> (Token, bool) {
    let bytes = text.as_bytes();
    let mut at = body;
    let token = |end: usize, part: Part, open: bool| Token {
        kind: Kind::Str(part),
        start,
        end,
        open,
    };
    while at < bytes.len() {
        match bytes[at] {
            b'\\' if bytes.get(at + 1) == Some(&b'(') => {
                let part = if first == Part::Whole {
                    Part::Start
                } else {
                    Part::Middle
                };
                return (token(at + 2, part, false), true);
            }
            b'\\' => at = (at + 2).min(bytes.len()),
            b'"' => return (token(at + 1, first, false), false),
            _ => at += 1,
        }
    }
    (token(bytes.len(), first, true), false)
}

fn ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

// Module paths such as `mod::name` are one identifier, including a trailing `::` still being typed.
fn ident_end(bytes: &[u8], mut at: usize, modules: bool) -> usize {
    loop {
        while bytes
            .get(at)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            at += 1;
        }
        if modules && bytes.get(at..at + 2) == Some(b"::") {
            at += 2;
            continue;
        }
        return at;
    }
}

fn number(bytes: &[u8], mut at: usize) -> usize {
    let digits = |at: &mut usize| {
        while bytes.get(*at).is_some_and(u8::is_ascii_digit) {
            *at += 1;
        }
    };
    digits(&mut at);
    if bytes.get(at) == Some(&b'.') && bytes.get(at + 1) != Some(&b'.') {
        at += 1;
        digits(&mut at);
    }
    if bytes
        .get(at)
        .is_some_and(|byte| matches!(byte, b'e' | b'E'))
    {
        at += 1;
        if bytes
            .get(at)
            .is_some_and(|byte| matches!(byte, b'+' | b'-'))
        {
            at += 1;
        }
        digits(&mut at);
    }
    at
}

#[cfg(test)]
#[path = "../../tests/unit/jq/lexer_tests.rs"]
mod tests;
