//! Reading JSON, and — under `--editor` — reading what somebody meant.
//!
//! One parser does both jobs. Strict mode refuses everything JSON refuses, with
//! a message that names the flag which would have taken it. Editor mode takes
//! the repairs the flag promises and keeps a count of them, so a run can say
//! what it changed rather than changing it quietly.
//!
//! Numbers are the one place where being strict is not enough: `1e2` is a
//! literal jq rewrites as `1E+2`, and a formatter that answered `100` would
//! rewrite numbers nobody asked about. `number::canonical` decides, here, so
//! the value tree only ever holds the text jq would print.

use indexmap::IndexMap;

use crate::number;
use crate::repair::{Repair, Repairs};
use crate::value::Value;

/// Deep enough for any file a person writes, shallow enough that a pathological
/// one is a message rather than a stack overflow.
const MAX_DEPTH: usize = 512;

#[derive(Debug, PartialEq, Eq)]
pub struct Problem {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl Problem {
    pub fn said(&self) -> String {
        format!("{}:{}: {}", self.line, self.column, self.message)
    }
}

#[derive(Clone, Copy, Default)]
pub struct Options {
    pub editor: bool,
}

pub struct Parsed {
    /// `None` for a body that holds no value at all, which is what jq makes of
    /// an empty file: nothing to write, and nothing wrong.
    pub value: Option<Value>,
    pub repairs: Repairs,
}

pub fn parse(input: &[u8], options: Options) -> Result<Parsed, Problem> {
    let mut parser = Parser {
        input,
        at: 0,
        editor: options.editor,
        repairs: Repairs::default(),
        depth: 0,
    };
    parser.trivia()?;
    if parser.at == input.len() {
        return Ok(Parsed {
            value: None,
            repairs: parser.repairs,
        });
    }
    let value = parser.value()?;
    parser.trivia()?;
    // jq reads `1 2` as a stream of two numbers. A formatter that did the same
    // would write two values into one file, which is not a file anybody wants.
    if parser.at != input.len() {
        return Err(parser.problem("more than one value"));
    }
    Ok(Parsed {
        value: Some(value),
        repairs: parser.repairs,
    })
}

struct Parser<'a> {
    input: &'a [u8],
    at: usize,
    editor: bool,
    repairs: Repairs,
    depth: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.input.get(self.at).copied()
    }

    fn eat(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.at += 1;
            return true;
        }
        false
    }

    fn problem(&self, message: impl Into<String>) -> Problem {
        self.problem_at(self.at, message)
    }

    /// Where the token starts rather than where it stopped being one: a reader
    /// wants the cursor on the literal, not after it.
    fn problem_at(&self, at: usize, message: impl Into<String>) -> Problem {
        let (line, column) = locate(self.input, at);
        Problem {
            line,
            column,
            message: message.into(),
        }
    }

    /// A mistake JSON makes and `--editor` does not have to: the message names
    /// the flag rather than leaving the reader to guess that one exists.
    fn repairable(&self, message: impl Into<String>) -> Problem {
        self.repairable_at(self.at, message)
    }

    fn repairable_at(&self, at: usize, message: impl Into<String>) -> Problem {
        let message = message.into();
        if self.editor {
            return self.problem_at(at, message);
        }
        self.problem_at(at, format!("{message}; --editor fixes this"))
    }

    /// Whitespace, and under `--editor` the things left where whitespace goes:
    /// comments, a byte order mark, and the spaces a word processor inserts.
    fn trivia(&mut self) -> Result<(), Problem> {
        loop {
            while self
                .peek()
                .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
            {
                self.at += 1;
            }
            let Some(byte) = self.peek() else {
                return Ok(());
            };
            if byte == b'/' {
                match self.input.get(self.at + 1) {
                    Some(b'/') | Some(b'*') => {
                        if !self.editor {
                            return Err(self.repairable("comments are not JSON"));
                        }
                        self.comment()?;
                        self.repairs.add(Repair::Comment);
                    }
                    _ => return Ok(()),
                }
                continue;
            }
            if byte >= 0x80 {
                let Some((character, width)) = codepoint(&self.input[self.at..]) else {
                    return Err(self.problem("invalid UTF-8"));
                };
                // U+FEFF is a byte order mark rather than a space, and it is
                // the thing most often found at the top of a file an editor is
                // asked to look at.
                if character.is_whitespace() || character == '\u{feff}' {
                    if !self.editor {
                        return Err(self.repairable("unusual whitespace"));
                    }
                    self.at += width;
                    self.repairs.add(Repair::Space);
                    continue;
                }
            }
            return Ok(());
        }
    }

    fn comment(&mut self) -> Result<(), Problem> {
        self.at += 1;
        match self.peek() {
            Some(b'/') => {
                while self.peek().is_some_and(|byte| byte != b'\n') {
                    self.at += 1;
                }
                Ok(())
            }
            Some(b'*') => {
                self.at += 1;
                match find(&self.input[self.at..], b"*/") {
                    Some(found) => {
                        self.at += found + 2;
                        Ok(())
                    }
                    None => {
                        self.at = self.input.len();
                        Err(self.problem("unterminated comment"))
                    }
                }
            }
            _ => Ok(()),
        }
    }

    fn value(&mut self) -> Result<Value, Problem> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.problem("too deeply nested"));
        }
        let value = self.dispatch();
        self.depth -= 1;
        value
    }

    fn dispatch(&mut self) -> Result<Value, Problem> {
        self.trivia()?;
        let Some(byte) = self.peek() else {
            return Err(self.problem("expected a value"));
        };
        match byte {
            b'{' => {
                self.at += 1;
                self.object()
            }
            b'[' => {
                self.at += 1;
                self.array()
            }
            b'"' => {
                self.at += 1;
                Ok(Value::String(self.string(b'"')?))
            }
            b'\'' => {
                if !self.editor {
                    return Err(self.repairable("single-quoted strings are not JSON"));
                }
                self.at += 1;
                self.repairs.add(Repair::Quote);
                Ok(Value::String(self.string(b'\'')?))
            }
            b'-' | b'+' | b'.' | b'0'..=b'9' => self.number(),
            byte if byte.is_ascii_alphabetic() => self.literal(),
            other => Err(self.problem(format!("unexpected {}", char::from(other)))),
        }
    }

    /// `true`, `false`, `null`, and under `--editor` the names Python and
    /// JavaScript give the same three things. `NaN` and `Infinity` are not
    /// JSON at all, and jq answers both with `null`, so that is what they
    /// become — with a count, because the value did change.
    fn literal(&mut self) -> Result<Value, Problem> {
        let start = self.at;
        self.skip_word();
        let word = &self.input[start..self.at];
        let word = String::from_utf8_lossy(word).into_owned();
        match word.as_str() {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            "null" => Ok(Value::Null),
            _ if self.editor => match word.as_str() {
                "True" | "False" => {
                    self.repairs.add(Repair::Literal);
                    Ok(Value::Bool(word == "True"))
                }
                "None" | "NaN" | "Infinity" => {
                    self.repairs.add(Repair::Literal);
                    Ok(Value::Null)
                }
                _ => Err(self.problem_at(start, format!("not a JSON literal: {word}"))),
            },
            _ => {
                let said = format!("not a JSON literal: {word}");
                if matches!(
                    word.as_str(),
                    "True" | "False" | "None" | "NaN" | "Infinity" | "undefined"
                ) {
                    return Err(self.repairable_at(start, said));
                }
                Err(self.problem_at(start, said))
            }
        }
    }

    fn number(&mut self) -> Result<Value, Problem> {
        let start = self.at;
        let signed = self.peek().is_some_and(|byte| matches!(byte, b'-' | b'+'));
        if signed {
            self.at += 1;
            // `-Infinity` names a value rather than a number, and it is the one
            // name that arrives with a sign in front of it.
            if self.editor && self.peek().is_some_and(|byte| byte.is_ascii_alphabetic()) {
                self.skip_word();
                let word = String::from_utf8_lossy(&self.input[start + 1..self.at]).into_owned();
                if matches!(word.as_str(), "Infinity" | "NaN") {
                    self.repairs.add(Repair::Literal);
                    return Ok(Value::Null);
                }
                let sign = char::from(self.input[start]);
                return Err(self.problem_at(start, format!("not a JSON literal: {sign}{word}")));
            }
        }
        self.skip_digits();
        if self.eat(b'.') {
            self.skip_digits();
        }
        if self.peek().is_some_and(|byte| matches!(byte, b'e' | b'E')) {
            self.at += 1;
            if self.peek().is_some_and(|byte| matches!(byte, b'-' | b'+')) {
                self.at += 1;
            }
            self.skip_digits();
        }
        // A number stops where the number stops: `1abc` and `1.2.3` are
        // mistakes, and reading them as a number followed by something else
        // would put a value in the file that was never written.
        if self
            .peek()
            .is_some_and(|byte| is_word(byte) || byte == b'.')
        {
            while self
                .peek()
                .is_some_and(|byte| is_word(byte) || byte == b'.')
            {
                self.at += 1;
            }
            let text = String::from_utf8_lossy(&self.input[start..self.at]).into_owned();
            return Err(self.problem_at(start, format!("not a number: {text}")));
        }
        let text = String::from_utf8_lossy(&self.input[start..self.at]).into_owned();
        match number::canonical(&text) {
            Some(literal) => Ok(Value::Number(literal)),
            None => Err(self.problem_at(start, format!("not a number: {text}"))),
        }
    }

    fn skip_digits(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.at += 1;
        }
    }

    fn skip_word(&mut self) {
        while self.peek().is_some_and(is_word) {
            self.at += 1;
        }
    }

    fn object(&mut self) -> Result<Value, Problem> {
        let mut entries: IndexMap<String, Value> = IndexMap::new();
        // Whether a comma has been read and a member is still owed, which is
        // how a trailing comma is told from the one between two members.
        let mut owed = false;
        loop {
            self.trivia()?;
            if self.peek() == Some(b'}') {
                if owed {
                    if !self.editor {
                        return Err(self.repairable("stray comma"));
                    }
                    self.repairs.add(Repair::Comma);
                }
                self.at += 1;
                return Ok(Value::Object(entries));
            }
            if self.peek() == Some(b',') {
                if !self.editor {
                    return Err(self.repairable("stray comma"));
                }
                self.at += 1;
                self.repairs.add(Repair::Comma);
                owed = false;
                continue;
            }
            let key = self.key()?;
            self.trivia()?;
            if !self.eat(b':') {
                return Err(self.problem("expected : after a key"));
            }
            let value = self.value()?;
            entries.insert(key, value);
            owed = false;
            self.trivia()?;
            match self.peek() {
                Some(b',') => {
                    self.at += 1;
                    owed = true;
                }
                Some(b'}') => {
                    self.at += 1;
                    return Ok(Value::Object(entries));
                }
                None => return Err(self.problem("expected another key-value pair")),
                Some(_) if self.editor => self.repairs.add(Repair::MissingComma),
                Some(_) => {
                    return Err(self.repairable("expected , or } between key-value pairs"));
                }
            }
        }
    }

    fn array(&mut self) -> Result<Value, Problem> {
        let mut items: Vec<Value> = Vec::new();
        let mut owed = false;
        loop {
            self.trivia()?;
            if self.peek() == Some(b']') {
                if owed {
                    if !self.editor {
                        return Err(self.repairable("stray comma"));
                    }
                    self.repairs.add(Repair::Comma);
                }
                self.at += 1;
                return Ok(Value::Array(items));
            }
            if self.peek() == Some(b',') {
                if !self.editor {
                    return Err(self.repairable("stray comma"));
                }
                self.at += 1;
                self.repairs.add(Repair::Comma);
                owed = false;
                continue;
            }
            items.push(self.value()?);
            owed = false;
            self.trivia()?;
            match self.peek() {
                Some(b',') => {
                    self.at += 1;
                    owed = true;
                }
                Some(b']') => {
                    self.at += 1;
                    return Ok(Value::Array(items));
                }
                None => return Err(self.problem("expected another array element")),
                Some(_) if self.editor => self.repairs.add(Repair::MissingComma),
                Some(_) => return Err(self.repairable("expected , or ] between elements")),
            }
        }
    }

    fn key(&mut self) -> Result<String, Problem> {
        match self.peek() {
            Some(b'"') => {
                self.at += 1;
                self.string(b'"')
            }
            Some(b'\'') if self.editor => {
                self.at += 1;
                self.repairs.add(Repair::Quote);
                self.string(b'\'')
            }
            Some(byte) if self.editor && is_word(byte) => {
                let start = self.at;
                self.skip_word();
                self.repairs.add(Repair::Key);
                Ok(String::from_utf8_lossy(&self.input[start..self.at]).into_owned())
            }
            Some(b'\'') => Err(self.repairable("single-quoted strings are not JSON")),
            // A name is the only thing `--editor` can make a key out of, so
            // anything else is answered without the offer.
            Some(byte) if is_word(byte) => Err(self.repairable("expected a key")),
            Some(byte) => Err(self.problem(format!("expected a key, found {}", char::from(byte)))),
            None => Err(self.problem("expected a key")),
        }
    }

    /// The opening quote has already been read. `quote` is what closes it, so a
    /// single-quoted string is read by the same code that reads a double-quoted
    /// one and the printer escapes the result either way.
    fn string(&mut self, quote: u8) -> Result<String, Problem> {
        let mut text: Vec<u8> = Vec::new();
        loop {
            let Some(byte) = self.peek() else {
                return Err(self.problem("unterminated string"));
            };
            match byte {
                _ if byte == quote => {
                    self.at += 1;
                    break;
                }
                b'\\' => self.escape(quote, &mut text)?,
                0x00..=0x1f => {
                    if !self.editor {
                        return Err(self.repairable("control character in a string"));
                    }
                    self.repairs.add(Repair::Control);
                    text.push(byte);
                    self.at += 1;
                }
                _ => {
                    let start = self.at;
                    while self
                        .peek()
                        .is_some_and(|byte| byte != quote && byte != b'\\' && byte >= 0x20)
                    {
                        self.at += 1;
                    }
                    text.extend_from_slice(&self.input[start..self.at]);
                }
            }
        }
        match String::from_utf8(text) {
            Ok(text) => Ok(text),
            // jq answers a byte it cannot read with U+FFFD rather than with an
            // error, and under `--editor` so does this: a file that is nearly
            // text is worth opening, and the count says the value changed.
            Err(error) if self.editor => {
                self.repairs.add(Repair::Utf8);
                Ok(String::from_utf8_lossy(&error.into_bytes()).into_owned())
            }
            Err(_) => Err(self.repairable("invalid UTF-8 in a string")),
        }
    }

    /// `quote` is the quote that opened this string, because an escape means
    /// different things in each of them. It is *not* counted here when the
    /// string is single quoted: that was already counted as the string.
    fn escape(&mut self, quote: u8, text: &mut Vec<u8>) -> Result<(), Problem> {
        self.at += 1;
        let Some(byte) = self.peek() else {
            return Err(self.problem("unterminated string"));
        };
        self.at += 1;
        match byte {
            b'"' => text.push(b'"'),
            b'\\' => text.push(b'\\'),
            b'/' => text.push(b'/'),
            b'b' => text.push(0x08),
            b'f' => text.push(0x0c),
            b'n' => text.push(b'\n'),
            b'r' => text.push(b'\r'),
            b't' => text.push(b'\t'),
            // JavaScript lets an apostrophe escape for the benefit of a single
            // quoted string, and takes the escape in a double quoted one too.
            b'\'' if self.editor => {
                if quote != b'\'' {
                    self.repairs.add(Repair::Quote);
                }
                text.push(b'\'');
            }
            b'u' => self.unicode(text)?,
            _ => {
                return Err(self.problem(format!("invalid escape: \\{}", char::from(byte))));
            }
        }
        Ok(())
    }

    fn unicode(&mut self, text: &mut Vec<u8>) -> Result<(), Problem> {
        let first = self.hex4()?;
        let code = if (0xd800..=0xdbff).contains(&first) {
            // A surrogate is half a character, and jq refuses half a character
            // rather than writing one out.
            if !self.paired() {
                return self.surrogate(text);
            }
            self.at += 2;
            let second = self.hex4()?;
            if !(0xdc00..=0xdfff).contains(&second) {
                return self.surrogate(text);
            }
            0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00)
        } else if (0xdc00..=0xdfff).contains(&first) {
            return self.surrogate(text);
        } else {
            first
        };
        let mut buffer = [0u8; 4];
        let character = char::from_u32(code).unwrap_or('\u{fffd}');
        text.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
        Ok(())
    }

    fn paired(&self) -> bool {
        self.input.get(self.at) == Some(&b'\\') && self.input.get(self.at + 1) == Some(&b'u')
    }

    fn surrogate(&mut self, text: &mut Vec<u8>) -> Result<(), Problem> {
        if !self.editor {
            return Err(self.repairable("lone surrogate"));
        }
        self.repairs.add(Repair::Surrogate);
        text.extend_from_slice("\u{fffd}".as_bytes());
        Ok(())
    }

    fn hex4(&mut self) -> Result<u32, Problem> {
        let mut value = 0;
        for _ in 0..4 {
            let Some(byte) = self.peek() else {
                return Err(self.problem("unterminated string"));
            };
            let Some(digit) = char::from(byte).to_digit(16) else {
                return Err(self.problem("invalid \\u escape"));
            };
            value = value * 16 + digit;
            self.at += 1;
        }
        Ok(value)
    }
}

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Decodes one character at the front of a slice, or answers nothing when those
/// bytes are not a character at all.
fn codepoint(bytes: &[u8]) -> Option<(char, usize)> {
    let width = match bytes.first()? {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return None,
    };
    let text = std::str::from_utf8(bytes.get(..width)?).ok()?;
    Some((text.chars().next()?, width))
}

fn locate(input: &[u8], at: usize) -> (usize, usize) {
    let head = &input[..at.min(input.len())];
    let line = 1 + head.iter().filter(|byte| **byte == b'\n').count();
    let column = head.iter().rev().take_while(|byte| **byte != b'\n').count() + 1;
    (line, column)
}
