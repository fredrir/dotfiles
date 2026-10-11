//! Lexical positions the parser does not expose: command substitutions,
//! `$0` references and plain words, located by byte offset.

use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Substitution {
    pub range: Range<usize>,
    pub inner: Range<usize>,
    pub quoted: bool,
    pub in_param: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZeroForm {
    /// `$0`, `$0:h`, `${0}`, `${0:A:h}`
    Name { modifiers: String },
    /// `${(%):-%x}` (`file`) or `${(%):-%N}`
    Prompt { file: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZeroRef {
    pub range: Range<usize>,
    pub form: ZeroForm,
    pub in_param: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    pub range: Range<usize>,
}

#[derive(Debug, Default)]
pub struct Scan {
    pub substitutions: Vec<Substitution>,
    pub zero_refs: Vec<ZeroRef>,
    /// Zero references the rewriter cannot reproduce, such as `${0:#pattern}`.
    pub unsupported_zero: Vec<usize>,
    pub words: Vec<Word>,
    /// Newlines between the elements of a closed `name=(...)`, outside comments.
    pub array_newlines: Vec<usize>,
    /// Each `(` of a `(((` zsh reads as a subshell around `((`, as in `((( a )) && b)`.
    pub subshell_parens: Vec<usize>,
    /// Parameters `${name=word}`, `${name:=word}` and `${name::=word}`
    /// assign, subscripts kept; outside command substitutions.
    pub assigned: Vec<String>,
    /// The text inside each `$(( ))`, outside command substitutions.
    pub arithmetic: Vec<Range<usize>>,
    pub unterminated: bool,
}

pub fn scan(text: &str) -> Scan {
    let mut scanner = Scanner {
        bytes: text.as_bytes(),
        pos: 0,
        out: Scan::default(),
        pending_heredocs: Vec::new(),
        param_depth: 0,
        substitution_depth: 0,
    };
    scanner.normal(Stop::Eof);
    scanner.out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stop {
    Eof,
    Paren,
}

struct Heredoc {
    delimiter: Vec<u8>,
    strip_tabs: bool,
    expand: bool,
}

struct Scanner<'a> {
    bytes: &'a [u8],
    pos: usize,
    out: Scan,
    pending_heredocs: Vec<Heredoc>,
    param_depth: usize,
    /// Command substitutions around the cursor, which run in a subshell.
    substitution_depth: usize,
}

const MODIFIERS: &[u8] = b"AahtreQlu";

impl Scanner<'_> {
    fn peek(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    fn starts_with(&self, prefix: &[u8]) -> bool {
        self.bytes[self.pos..].starts_with(prefix)
    }

    fn at_word_start(&self) -> bool {
        word_start(self.bytes, self.pos)
    }

    /// Whether the `(` at the cursor opens `name=(` or `name+=(`.
    fn opens_array(&self) -> bool {
        let Some(head) = self.bytes[..self.pos].strip_suffix(b"=") else {
            return false;
        };
        let head = head.strip_suffix(b"+").unwrap_or(head);
        let name = head
            .iter()
            .rev()
            .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'_')
            .count();
        let start = head.len() - name;
        name > 0 && !head[start].is_ascii_digit() && word_start(head, start)
    }

    /// Whether the `(` at the cursor starts `(((` at a word, where zsh reads
    /// `((` as math only when the paren closing it is followed by another.
    fn opens_subshell(&self) -> bool {
        if !self.starts_with(b"(((")
            || !self.at_word_start()
            || self.bytes[..self.pos].ends_with(b"(")
        {
            return false;
        }
        let mut depth = 0usize;
        for (offset, byte) in self.bytes[self.pos + 2..].iter().enumerate() {
            match byte {
                b'(' => depth += 1,
                b')' if depth > 0 => depth -= 1,
                b')' => return self.bytes.get(self.pos + 3 + offset) != Some(&b')'),
                _ => {}
            }
        }
        false
    }

    /// Unquoted shell text until `stop`; returns whether the stop was found.
    fn normal(&mut self, stop: Stop) -> bool {
        let mut parens = 0usize;
        let mut array: Option<(usize, Vec<usize>)> = None;
        let mut comment = false;
        while let Some(byte) = self.peek(0) {
            match byte {
                b')' if stop == Stop::Paren && parens == 0 => {
                    self.pos += 1;
                    return true;
                }
                b'(' => {
                    if self.opens_subshell() {
                        self.out.subshell_parens.push(self.pos);
                    }
                    parens += 1;
                    if stop == Stop::Eof && array.is_none() && self.opens_array() {
                        array = Some((parens, Vec::new()));
                    }
                    self.pos += 1;
                }
                b')' => {
                    if let Some((_, newlines)) = array.take_if(|(depth, _)| *depth == parens) {
                        self.out.array_newlines.extend(newlines);
                    }
                    parens = parens.saturating_sub(1);
                    self.pos += 1;
                }
                b'\\' => self.pos += 2,
                b'\'' => self.single_quote(),
                b'"' => self.double_quote(),
                b'`' => self.backtick(false),
                b'$' => self.dollar(false),
                b'#' if self.at_word_start() => {
                    comment = true;
                    while let Some(byte) = self.peek(0) {
                        if byte == b'\n' {
                            break;
                        }
                        self.pos += 1;
                    }
                }
                b'<' if self.starts_with(b"<<<") => self.pos += 3,
                b'<' if self.starts_with(b"<<") => self.heredoc_start(),
                b'\n' => {
                    if let Some((depth, newlines)) = &mut array
                        && *depth == parens
                        && !comment
                    {
                        newlines.push(self.pos);
                    }
                    comment = false;
                    self.pos += 1;
                    self.heredoc_bodies();
                }
                _ if self.at_word_start() && is_word_byte(byte) => self.word(),
                _ => self.pos += 1,
            }
        }
        if stop != Stop::Eof {
            self.out.unterminated = true;
        }
        stop == Stop::Eof
    }

    fn word(&mut self) {
        let start = self.pos;
        let mut end = start;
        while end < self.bytes.len() && is_word_byte(self.bytes[end]) {
            end += 1;
        }
        let boundary = self.bytes.get(end).is_none_or(|byte| {
            matches!(
                byte,
                b' ' | b'\t' | b'\n' | b';' | b'&' | b'|' | b')' | b'}'
            )
        });
        if boundary {
            self.out.words.push(Word { range: start..end });
        }
        self.pos = end;
    }

    fn single_quote(&mut self) {
        self.pos += 1;
        while let Some(byte) = self.peek(0) {
            self.pos += 1;
            if byte == b'\'' {
                return;
            }
        }
        self.out.unterminated = true;
    }

    fn ansi_quote(&mut self) {
        self.pos += 2;
        while let Some(byte) = self.peek(0) {
            match byte {
                b'\\' => self.pos += 2,
                b'\'' => {
                    self.pos += 1;
                    return;
                }
                _ => self.pos += 1,
            }
        }
        self.out.unterminated = true;
    }

    fn double_quote(&mut self) {
        self.pos += 1;
        while let Some(byte) = self.peek(0) {
            match byte {
                b'"' => {
                    self.pos += 1;
                    return;
                }
                b'\\' => self.pos += 2,
                b'`' => self.backtick(true),
                b'$' => self.dollar(true),
                _ => self.pos += 1,
            }
        }
        self.out.unterminated = true;
    }

    fn backtick(&mut self, quoted: bool) {
        let start = self.pos;
        self.pos += 1;
        while let Some(byte) = self.peek(0) {
            match byte {
                b'\\' => self.pos += 2,
                b'`' => {
                    self.pos += 1;
                    self.out.substitutions.push(Substitution {
                        range: start..self.pos,
                        inner: start + 1..self.pos - 1,
                        quoted,
                        in_param: self.param_depth > 0,
                    });
                    return;
                }
                _ => self.pos += 1,
            }
        }
        self.out.unterminated = true;
    }

    fn dollar(&mut self, quoted: bool) {
        let start = self.pos;
        match self.peek(1) {
            Some(b'\'') if !quoted => self.ansi_quote(),
            Some(b'(') if self.peek(2) == Some(b'(') => self.arithmetic(),
            Some(b'(') => {
                self.pos += 2;
                let depth = std::mem::take(&mut self.param_depth);
                self.substitution_depth += 1;
                let closed = self.normal(Stop::Paren);
                self.substitution_depth -= 1;
                self.param_depth = depth;
                if closed {
                    self.out.substitutions.push(Substitution {
                        range: start..self.pos,
                        inner: start + 2..self.pos - 1,
                        quoted,
                        in_param: self.param_depth > 0,
                    });
                }
            }
            Some(b'{') => self.parameter(quoted),
            Some(b'0') if !self.bytes.get(self.pos + 2).is_some_and(u8::is_ascii_digit) => {
                self.pos += 2;
                let modifiers = self.modifiers();
                self.out.zero_refs.push(ZeroRef {
                    range: start..self.pos,
                    form: ZeroForm::Name { modifiers },
                    in_param: self.param_depth > 0,
                });
            }
            _ => self.pos += 1,
        }
    }

    fn modifiers(&mut self) -> String {
        let mut modifiers = String::new();
        while self.peek(0) == Some(b':')
            && self.peek(1).is_some_and(|byte| MODIFIERS.contains(&byte))
        {
            modifiers.push(self.bytes[self.pos + 1] as char);
            self.pos += 2;
        }
        modifiers
    }

    fn parameter(&mut self, quoted: bool) {
        let start = self.pos;
        self.pos += 2;
        for (prompt, file) in [(&b"(%):-%x}"[..], true), (b"(%):-%N}", false)] {
            if self.starts_with(prompt) {
                self.pos += prompt.len();
                self.out.zero_refs.push(ZeroRef {
                    range: start..self.pos,
                    form: ZeroForm::Prompt { file },
                    in_param: self.param_depth > 0,
                });
                return;
            }
        }
        if self.peek(0) == Some(b'0') && !self.peek(1).is_some_and(|byte| byte.is_ascii_digit()) {
            self.pos += 1;
            let modifiers = self.modifiers();
            if self.peek(0) == Some(b'}') {
                self.pos += 1;
                self.out.zero_refs.push(ZeroRef {
                    range: start..self.pos,
                    form: ZeroForm::Name { modifiers },
                    in_param: self.param_depth > 0,
                });
                return;
            }
            self.out.unsupported_zero.push(start);
        }
        if let Some(name) = self.assigning().filter(|_| self.substitution_depth == 0) {
            self.out.assigned.push(name);
        }
        self.param_depth += 1;
        while let Some(byte) = self.peek(0) {
            match byte {
                b'}' => {
                    self.pos += 1;
                    self.param_depth -= 1;
                    return;
                }
                b'\\' => self.pos += 2,
                b'\'' if !quoted => self.single_quote(),
                b'"' => self.double_quote(),
                b'`' => self.backtick(quoted),
                b'$' => self.dollar(quoted),
                _ => self.pos += 1,
            }
        }
        self.param_depth -= 1;
        self.out.unterminated = true;
    }

    /// The parameter a `${...}` past its brace assigns: flags, then a name
    /// and subscript followed by `=`, `:=` or `::=`.
    fn assigning(&self) -> Option<String> {
        let rest = &self.bytes[self.pos..];
        let mut index = match rest.first() {
            Some(b'(') => rest.iter().position(|byte| *byte == b')')? + 1,
            _ => 0,
        };
        while matches!(rest.get(index), Some(b'^' | b'=' | b'~')) {
            index += 1;
        }
        let start = index;
        while rest
            .get(index)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            index += 1;
        }
        if index == start || rest[start].is_ascii_digit() {
            return None;
        }
        if rest.get(index) == Some(&b'[') {
            index += rest[index..].iter().position(|byte| *byte == b']')? + 1;
        }
        let operator = &rest[index..];
        [&b"="[..], b":=", b"::="]
            .iter()
            .any(|assign| operator.starts_with(assign))
            .then(|| String::from_utf8_lossy(&rest[start..index]).into_owned())
    }

    fn arithmetic(&mut self) {
        self.pos += 3;
        let start = self.pos;
        let mut depth = 2usize;
        while let Some(byte) = self.peek(0) {
            self.pos += 1;
            match byte {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        if self.substitution_depth == 0 {
                            self.out.arithmetic.push(start..self.pos - 2);
                        }
                        return;
                    }
                }
                _ => {}
            }
        }
        self.out.unterminated = true;
    }

    fn heredoc_start(&mut self) {
        self.pos += 2;
        let strip_tabs = self.peek(0) == Some(b'-');
        if strip_tabs {
            self.pos += 1;
        }
        while matches!(self.peek(0), Some(b' ' | b'\t')) {
            self.pos += 1;
        }
        let mut delimiter = Vec::new();
        let mut expand = true;
        while let Some(byte) = self.peek(0) {
            match byte {
                b' ' | b'\t' | b'\n' | b';' | b'&' | b'|' | b'<' | b'>' | b'(' | b')' => break,
                b'\'' | b'"' => {
                    expand = false;
                    self.pos += 1;
                    while let Some(inner) = self.peek(0) {
                        self.pos += 1;
                        if inner == byte {
                            break;
                        }
                        delimiter.push(inner);
                    }
                }
                b'\\' => {
                    expand = false;
                    if let Some(next) = self.peek(1) {
                        delimiter.push(next);
                    }
                    self.pos += 2;
                }
                _ => {
                    delimiter.push(byte);
                    self.pos += 1;
                }
            }
        }
        self.pending_heredocs.push(Heredoc {
            delimiter,
            strip_tabs,
            expand,
        });
    }

    fn heredoc_bodies(&mut self) {
        for heredoc in std::mem::take(&mut self.pending_heredocs) {
            loop {
                let line_start = self.pos;
                let mut line_end = line_start;
                while line_end < self.bytes.len() && self.bytes[line_end] != b'\n' {
                    line_end += 1;
                }
                if line_start >= self.bytes.len() {
                    self.out.unterminated = true;
                    return;
                }
                let mut line = &self.bytes[line_start..line_end];
                if heredoc.strip_tabs {
                    while let Some(rest) = line.strip_prefix(b"\t") {
                        line = rest;
                    }
                }
                if line == heredoc.delimiter.as_slice() {
                    self.pos = (line_end + 1).min(self.bytes.len());
                    break;
                }
                if heredoc.expand {
                    self.heredoc_line(line_end);
                }
                self.pos = (line_end + 1).min(self.bytes.len());
            }
        }
    }

    fn heredoc_line(&mut self, end: usize) {
        while self.pos < end {
            match self.bytes[self.pos] {
                b'\\' => self.pos += 2,
                b'`' => self.backtick(true),
                b'$' => self.dollar(true),
                _ => self.pos += 1,
            }
        }
    }
}

fn word_start(bytes: &[u8], pos: usize) -> bool {
    pos == 0
        || matches!(
            bytes[pos - 1],
            b' ' | b'\t' | b'\n' | b';' | b'&' | b'|' | b'(' | b')' | b'{' | b'}'
        )
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

#[cfg(test)]
#[path = "../tests/unit/scan_tests.rs"]
mod tests;
