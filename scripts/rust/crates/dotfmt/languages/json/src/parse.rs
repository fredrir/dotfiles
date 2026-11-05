
use dotfmt_core::repair::{Edit, Locator, Repair, Repairs};
use indexmap::IndexMap;

use crate::number;
use crate::render;
use crate::value::Value;

const MAX_DEPTH: usize = 512;

const QUOTES: [(&[u8], &[u8]); 7] = [
    (b"\"", b"\""),
    (b"'", b"'"),
    (b"`", b"`"),
    ("\u{201c}".as_bytes(), "\u{201d}".as_bytes()),
    ("\u{201d}".as_bytes(), "\u{201d}".as_bytes()),
    ("\u{2018}".as_bytes(), "\u{2019}".as_bytes()),
    ("\u{2019}".as_bytes(), "\u{2019}".as_bytes()),
];

#[derive(Debug, PartialEq, Eq)]
pub struct Problem {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

#[cfg(test)]
impl Problem {
    pub fn said(&self) -> String {
        format!("{}:{}: {}", self.line, self.column, self.message)
    }
}

#[derive(Clone, Copy, Default)]
pub struct Options {
    pub repair: bool,
    pub commented: bool,
}

pub struct Parsed {
    pub value: Option<Value>,
    pub repairs: Repairs,
    pub edits: Vec<Edit>,
}

pub struct Streamed {
    pub values: Vec<Value>,
    pub repairs: Repairs,
}

pub fn parse(input: &[u8], options: Options) -> Result<Parsed, Problem> {
    let mut parser = Parser::new(input, options);
    parser.trivia()?;
    if parser.at == input.len() {
        return Ok(parser.parsed(None));
    }
    let value = parser.value()?;
    parser.trivia()?;
    parser.stray_closers()?;
    if parser.at != input.len() {
        return Err(parser.problem("more than one value"));
    }
    Ok(parser.parsed(Some(value)))
}

pub fn stream(input: &[u8], options: Options) -> Result<Streamed, Problem> {
    let mut parser = Parser::new(input, options);
    let mut values = Vec::new();
    loop {
        parser.trivia()?;
        parser.stray_closers()?;
        if parser.at == input.len() {
            return Ok(Streamed {
                values,
                repairs: parser.located(),
            });
        }
        values.push(parser.value()?);
    }
}

struct Parser<'a> {
    input: &'a [u8],
    at: usize,
    repair: bool,
    commented: bool,
    fixes: Vec<(Repair, usize)>,
    edits: Vec<Edit>,
    depth: usize,
    end: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a [u8], options: Options) -> Self {
        Parser {
            input,
            at: 0,
            repair: options.repair,
            commented: options.commented,
            fixes: Vec::new(),
            edits: Vec::new(),
            depth: 0,
            end: 0,
        }
    }

    fn parsed(mut self, value: Option<Value>) -> Parsed {
        Parsed {
            value,
            repairs: self.located(),
            edits: std::mem::take(&mut self.edits),
        }
    }

    fn located(&mut self) -> Repairs {
        self.fixes.sort_by_key(|(_, at)| *at);
        let mut locator = Locator::new(self.input);
        self.fixes
            .iter()
            .map(|(repair, at)| locator.fix(*repair, *at))
            .collect()
    }

    fn fix(&mut self, repair: Repair, at: usize) {
        self.fixes.push((repair, at));
    }

    fn fixed(&mut self, repair: Repair, at: usize, edit: Edit) {
        self.fix(repair, at);
        self.edits.push(edit);
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.at).copied()
    }

    fn eat(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.at += 1;
            self.end = self.at;
            return true;
        }
        false
    }

    fn problem(&self, message: impl Into<String>) -> Problem {
        self.problem_at(self.at, message)
    }

    fn problem_at(&self, at: usize, message: impl Into<String>) -> Problem {
        let (line, column) = Locator::new(self.input).locate(at);
        Problem {
            line,
            column,
            message: message.into(),
        }
    }

    fn unexpected(&self) -> Problem {
        self.problem(format!("unexpected {}", self.character()))
    }

    fn character(&self) -> String {
        match codepoint(&self.input[self.at..]) {
            Some((character, _)) => character.to_string(),
            None => format!("byte 0x{:02x}", self.input[self.at]),
        }
    }

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
            match byte {
                b'/' if matches!(self.input.get(self.at + 1), Some(b'/' | b'*')) => {
                    if !self.repair && !self.commented {
                        return Err(self.problem("comments are not JSON"));
                    }
                    let start = self.at;
                    self.comment()?;
                    if !self.commented {
                        self.fixed(Repair::Comment, start, Edit::replace(start, self.at, " "));
                    }
                }
                b'#' if self.repair => {
                    let start = self.at;
                    self.skip_line();
                    let edit = if self.commented {
                        Edit::replace(start, start + 1, "//")
                    } else {
                        Edit::delete(start, self.at)
                    };
                    self.fixed(Repair::Comment, start, edit);
                }
                b'`' | b'~' if self.repair && self.depth == 0 && self.fence() => {}
                0x80.. => {
                    let Some((character, width)) = codepoint(&self.input[self.at..]) else {
                        return Err(self.problem("invalid UTF-8"));
                    };
                    let bom = character == '\u{feff}';
                    if !bom && !character.is_whitespace() {
                        return Ok(());
                    }
                    if !self.repair {
                        return Err(self.problem("unusual whitespace"));
                    }
                    let start = self.at;
                    self.at += width;
                    let space = if bom { "" } else { " " };
                    self.fixed(Repair::Space, start, Edit::replace(start, self.at, space));
                }
                _ => return Ok(()),
            }
        }
    }

    fn comment(&mut self) -> Result<(), Problem> {
        self.at += 1;
        match self.peek() {
            Some(b'/') => {
                self.skip_line();
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

    fn skip_line(&mut self) {
        while self.peek().is_some_and(|byte| byte != b'\n') {
            self.at += 1;
        }
    }

    fn fence(&mut self) -> bool {
        let start = self.at;
        let line = self.input[..start]
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |newline| newline + 1);
        let indented = self.input[line..start]
            .iter()
            .all(|byte| matches!(byte, b' ' | b'\t'));
        let rest = &self.input[start..];
        if !indented || !(rest.starts_with(b"```") || rest.starts_with(b"~~~")) {
            return false;
        }
        self.skip_line();
        self.fixed(Repair::Fence, start, Edit::delete(start, self.at));
        true
    }

    fn stray_closers(&mut self) -> Result<(), Problem> {
        while self.repair && matches!(self.peek(), Some(b'}' | b']')) {
            self.fixed(Repair::Bracket, self.at, Edit::delete(self.at, self.at + 1));
            self.at += 1;
            self.trivia()?;
        }
        Ok(())
    }

    fn value(&mut self) -> Result<Value, Problem> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.problem("too deeply nested"));
        }
        let value = self.dispatch();
        self.depth -= 1;
        self.end = self.at;
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
                self.end = self.at;
                self.object()
            }
            b'[' => {
                self.at += 1;
                self.end = self.at;
                self.array()
            }
            b'-' | b'+' | b'.' | b'0'..=b'9' => self.number(),
            byte if byte.is_ascii_alphabetic() => self.literal(),
            _ if quote(&self.input[self.at..]).is_some() => self.text().map(Value::String),
            _ => Err(self.unexpected()),
        }
    }

    fn literal(&mut self) -> Result<Value, Problem> {
        let start = self.at;
        self.skip_word();
        let word = String::from_utf8_lossy(&self.input[start..self.at]).into_owned();
        match word.as_str() {
            "true" => return Ok(Value::Bool(true)),
            "false" => return Ok(Value::Bool(false)),
            "null" => return Ok(Value::Null),
            _ => {}
        }
        let (value, json) = match word.as_str() {
            "True" => (Value::Bool(true), "true"),
            "False" => (Value::Bool(false), "false"),
            "None" | "NaN" | "Infinity" | "undefined" => (Value::Null, "null"),
            _ => return Err(self.problem_at(start, format!("not a JSON literal: {word}"))),
        };
        if !self.repair {
            return Err(self.problem_at(start, format!("not a JSON literal: {word}")));
        }
        self.fixed(Repair::Literal, start, Edit::replace(start, self.at, json));
        Ok(value)
    }

    fn number(&mut self) -> Result<Value, Problem> {
        let start = self.at;
        if self.peek().is_some_and(|byte| matches!(byte, b'-' | b'+')) {
            self.at += 1;
            if self.repair && self.peek().is_some_and(|byte| byte.is_ascii_alphabetic()) {
                self.skip_word();
                let word = String::from_utf8_lossy(&self.input[start + 1..self.at]).into_owned();
                if matches!(word.as_str(), "Infinity" | "NaN") {
                    self.fixed(
                        Repair::Literal,
                        start,
                        Edit::replace(start, self.at, "null"),
                    );
                    return Ok(Value::Null);
                }
                let sign = char::from(self.input[start]);
                return Err(self.problem_at(start, format!("not a JSON literal: {sign}{word}")));
            }
        }
        if self.repair && matches!(self.input.get(self.at..self.at + 2), Some(b"0x" | b"0X")) {
            return self.hex(start);
        }
        self.skip_digits();
        if self.peek() == Some(b'.') {
            self.at += 1;
            self.skip_digits();
        }
        if self.peek().is_some_and(|byte| matches!(byte, b'e' | b'E')) {
            self.at += 1;
            if self.peek().is_some_and(|byte| matches!(byte, b'-' | b'+')) {
                self.at += 1;
            }
            self.skip_digits();
        }
        self.number_ends(start)?;
        let text = String::from_utf8_lossy(&self.input[start..self.at]).into_owned();
        let Some(literal) = number::canonical(&text) else {
            return Err(self.problem_at(start, format!("not a number: {text}")));
        };
        if self.commented && !number::strict(text.as_bytes()) {
            if !self.repair {
                return Err(self.problem_at(start, "invalid JSON number"));
            }
            self.fixed(
                Repair::Number,
                start,
                Edit::replace(start, self.at, literal.as_str()),
            );
        }
        Ok(Value::Number(literal))
    }

    fn hex(&mut self, start: usize) -> Result<Value, Problem> {
        self.at += 2;
        let digits = self.at;
        while self.peek().is_some_and(|byte| byte.is_ascii_hexdigit()) {
            self.at += 1;
        }
        self.number_ends(start)?;
        let text = String::from_utf8_lossy(&self.input[start..self.at]).into_owned();
        let magnitude = std::str::from_utf8(&self.input[digits..self.at])
            .ok()
            .and_then(|digits| u128::from_str_radix(digits, 16).ok());
        let Some(magnitude) = magnitude else {
            return Err(self.problem_at(start, format!("not a number: {text}")));
        };
        let sign = if self.input[start] == b'-' && magnitude > 0 {
            "-"
        } else {
            ""
        };
        let literal = format!("{sign}{magnitude}");
        self.fixed(
            Repair::Number,
            start,
            Edit::replace(start, self.at, literal.as_str()),
        );
        Ok(Value::Number(literal))
    }

    fn number_ends(&mut self, start: usize) -> Result<(), Problem> {
        if !self
            .peek()
            .is_some_and(|byte| is_word(byte) || byte == b'.')
        {
            return Ok(());
        }
        while self
            .peek()
            .is_some_and(|byte| is_word(byte) || byte == b'.')
        {
            self.at += 1;
        }
        let text = String::from_utf8_lossy(&self.input[start..self.at]).into_owned();
        Err(self.problem_at(start, format!("not a number: {text}")))
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
        let mut comma = None;
        loop {
            self.trivia()?;
            match self.peek() {
                Some(b'}') => {
                    self.trailing(comma)?;
                    self.eat(b'}');
                    return Ok(Value::Object(entries));
                }
                Some(b',') => {
                    self.stray_comma()?;
                    continue;
                }
                Some(b']') | None if self.close(b'}', comma) => {
                    return Ok(Value::Object(entries));
                }
                _ => {}
            }
            let key = self.key()?;
            self.colon()?;
            let value = self.value()?;
            entries.insert(key, value);
            comma = None;
            self.trivia()?;
            match self.peek() {
                Some(b',') => comma = Some(self.separator()),
                Some(b';') if self.repair => comma = Some(self.separator()),
                Some(b'}') => {
                    self.eat(b'}');
                    return Ok(Value::Object(entries));
                }
                Some(b']') | None if self.close(b'}', None) => {
                    return Ok(Value::Object(entries));
                }
                None => return Err(self.problem("expected another key-value pair")),
                Some(_) if self.repair => self.missing_comma(),
                Some(_) => return Err(self.problem("expected , or } between key-value pairs")),
            }
        }
    }

    fn array(&mut self) -> Result<Value, Problem> {
        let mut items: Vec<Value> = Vec::new();
        let mut comma = None;
        loop {
            self.trivia()?;
            match self.peek() {
                Some(b']') => {
                    self.trailing(comma)?;
                    self.eat(b']');
                    return Ok(Value::Array(items));
                }
                Some(b',') => {
                    self.stray_comma()?;
                    continue;
                }
                Some(b'}') | None if self.close(b']', comma) => {
                    return Ok(Value::Array(items));
                }
                _ => {}
            }
            items.push(self.value()?);
            comma = None;
            self.trivia()?;
            match self.peek() {
                Some(b',') => comma = Some(self.separator()),
                Some(b';') if self.repair => comma = Some(self.separator()),
                Some(b']') => {
                    self.eat(b']');
                    return Ok(Value::Array(items));
                }
                Some(b'}') | None if self.close(b']', None) => {
                    return Ok(Value::Array(items));
                }
                None => return Err(self.problem("expected another array element")),
                Some(_) if self.repair => self.missing_comma(),
                Some(_) => return Err(self.problem("expected , or ] between elements")),
            }
        }
    }

    fn separator(&mut self) -> usize {
        let at = self.at;
        if self.peek() == Some(b';') {
            self.fixed(Repair::Separator, at, Edit::replace(at, at + 1, ","));
        }
        self.at += 1;
        self.end = self.at;
        at
    }

    fn missing_comma(&mut self) {
        self.fixed(Repair::MissingComma, self.end, Edit::insert(self.end, ","));
    }

    fn stray_comma(&mut self) -> Result<(), Problem> {
        if !self.repair {
            return Err(self.problem("stray comma"));
        }
        self.fixed(Repair::Comma, self.at, Edit::delete(self.at, self.at + 1));
        self.at += 1;
        Ok(())
    }

    fn trailing(&mut self, comma: Option<usize>) -> Result<(), Problem> {
        if comma.is_some() && !self.commented && !self.repair {
            return Err(self.problem("stray comma"));
        }
        self.drop_trailing(comma);
        Ok(())
    }

    fn drop_trailing(&mut self, comma: Option<usize>) {
        if let Some(at) = comma
            && self.repair
            && !self.commented
        {
            self.fixed(Repair::Comma, at, Edit::delete(at, at + 1));
        }
    }

    fn close(&mut self, closer: u8, comma: Option<usize>) -> bool {
        if !self.repair {
            return false;
        }
        self.drop_trailing(comma);
        let text = char::from(closer).to_string();
        match self.peek() {
            Some(_) => {
                self.fixed(
                    Repair::Bracket,
                    self.at,
                    Edit::replace(self.at, self.at + 1, text),
                );
                self.at += 1;
                self.end = self.at;
            }
            None => self.fixed(Repair::Bracket, self.end, Edit::insert(self.end, text)),
        }
        true
    }

    fn colon(&mut self) -> Result<(), Problem> {
        self.trivia()?;
        if self.eat(b':') {
            return Ok(());
        }
        if self.repair && self.peek() == Some(b'=') {
            self.fixed(
                Repair::Separator,
                self.at,
                Edit::replace(self.at, self.at + 1, ":"),
            );
            self.eat(b'=');
            return Ok(());
        }
        if self.repair && self.starts_value() {
            self.fixed(Repair::Colon, self.end, Edit::insert(self.end, ":"));
            return Ok(());
        }
        Err(self.problem("expected : after a key"))
    }

    fn starts_value(&self) -> bool {
        self.peek().is_some_and(|byte| {
            matches!(byte, b'{' | b'[' | b'-' | b'+' | b'.')
                || byte.is_ascii_alphanumeric()
                || quote(&self.input[self.at..]).is_some()
        })
    }

    fn key(&mut self) -> Result<String, Problem> {
        match self.peek() {
            Some(_) if quote(&self.input[self.at..]).is_some() => self.text(),
            Some(byte) if self.repair && is_key(byte) => {
                let start = self.at;
                while self.peek().is_some_and(is_key) {
                    self.at += 1;
                }
                self.end = self.at;
                let key = String::from_utf8_lossy(&self.input[start..self.at]).into_owned();
                self.fixed(
                    Repair::Key,
                    start,
                    Edit::replace(start, self.at, render::string(&key)),
                );
                Ok(key)
            }
            Some(byte) if is_word(byte) => Err(self.problem("expected a key")),
            Some(_) => Err(self.problem(format!("expected a key, found {}", self.character()))),
            None => Err(self.problem("expected a key")),
        }
    }

    fn text(&mut self) -> Result<String, Problem> {
        let start = self.at;
        let (open, close) = quote(&self.input[start..]).unwrap_or(QUOTES[0]);
        let plain = open == b"\"";
        if !plain && !self.repair {
            if open == b"'" {
                return Err(self.problem("single-quoted strings are not JSON"));
            }
            return Err(self.unexpected());
        }
        let before = self.fixes.len();
        self.at += open.len();
        let text = self.string(close, start)?;
        if !plain {
            self.fix(Repair::Quote, start);
        }
        if !plain || self.fixes.len() > before {
            self.edits
                .push(Edit::replace(start, self.at, render::string(&text)));
        }
        self.end = self.at;
        Ok(text)
    }

    fn string(&mut self, close: &[u8], start: usize) -> Result<String, Problem> {
        let template = close == b"`";
        let mut text: Vec<u8> = Vec::new();
        loop {
            let Some(byte) = self.peek() else {
                return Err(self.problem("unterminated string"));
            };
            if self.input[self.at..].starts_with(close) {
                self.at += close.len();
                break;
            }
            match byte {
                b'\\' => self.escape(close, &mut text)?,
                b'$' if template && self.input.get(self.at + 1) == Some(&b'{') => {
                    return Err(self.problem("template literals are not JSON"));
                }
                0x00..=0x1f => {
                    if !self.repair {
                        return Err(self.problem("control character in a string"));
                    }
                    if !template {
                        self.fix(Repair::Control, self.at);
                    }
                    text.push(byte);
                    self.at += 1;
                }
                _ => {
                    let from = self.at;
                    self.at += 1;
                    while self.peek().is_some_and(|byte| {
                        byte != close[0] && byte != b'\\' && byte >= 0x20 && byte != b'$'
                    }) {
                        self.at += 1;
                    }
                    text.extend_from_slice(&self.input[from..self.at]);
                }
            }
        }
        match String::from_utf8(text) {
            Ok(text) => Ok(text),
            Err(error) if self.repair => {
                self.fix(Repair::Utf8, start);
                Ok(String::from_utf8_lossy(&error.into_bytes()).into_owned())
            }
            Err(_) => Err(self.problem_at(start, "invalid UTF-8 in a string")),
        }
    }

    fn escape(&mut self, close: &[u8], text: &mut Vec<u8>) -> Result<(), Problem> {
        let at = self.at;
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
            b'\'' if self.repair => {
                if close == b"\"" {
                    self.fix(Repair::Quote, at);
                }
                text.push(b'\'');
            }
            b'`' if close == b"`" => text.push(b'`'),
            b'$' if close == b"`" => text.push(b'$'),
            b'u' => self.unicode(text)?,
            _ => {
                return Err(self.problem(format!("invalid escape: \\{}", char::from(byte))));
            }
        }
        Ok(())
    }

    fn unicode(&mut self, text: &mut Vec<u8>) -> Result<(), Problem> {
        let at = self.at - 2;
        let first = self.hex4()?;
        let code = if (0xd800..=0xdbff).contains(&first) {
            if !self.paired() {
                return self.surrogate(at, text);
            }
            self.at += 2;
            let second = self.hex4()?;
            if !(0xdc00..=0xdfff).contains(&second) {
                return self.surrogate(at, text);
            }
            0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00)
        } else if (0xdc00..=0xdfff).contains(&first) {
            return self.surrogate(at, text);
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

    fn surrogate(&mut self, at: usize, text: &mut Vec<u8>) -> Result<(), Problem> {
        if !self.repair {
            return Err(self.problem("lone surrogate"));
        }
        self.fix(Repair::Surrogate, at);
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

fn quote(bytes: &[u8]) -> Option<(&'static [u8], &'static [u8])> {
    QUOTES.into_iter().find(|(open, _)| bytes.starts_with(open))
}

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

fn is_key(byte: u8) -> bool {
    is_word(byte) || byte == b'-' || byte == b'.'
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

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
