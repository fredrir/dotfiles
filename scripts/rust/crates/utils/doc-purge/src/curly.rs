use crate::lang::Dialect;
use crate::scan::{Bail, Out, char_width, identifier_byte, line_end, starts_with};

const DEPTH: usize = 64;

const OPERATOR_WORDS: &[&[u8]] = &[
    b"return",
    b"typeof",
    b"instanceof",
    b"in",
    b"of",
    b"new",
    b"delete",
    b"void",
    b"case",
    b"do",
    b"else",
    b"yield",
    b"await",
    b"throw",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Prev {
    Value,
    Operator,
}

struct Curly<'a> {
    source: &'a [u8],
    at: usize,
    dialect: Dialect,
    out: Out,
    prev: Prev,
}

pub fn scan(source: &[u8], dialect: Dialect) -> Result<Out, Bail> {
    let mut scanner = Curly {
        source,
        at: 0,
        dialect,
        out: Out::default(),
        prev: Prev::Operator,
    };
    scanner.code(0, false)?;
    Ok(scanner.out)
}

impl<'a> Curly<'a> {
    fn len(&self) -> usize {
        self.source.len()
    }

    fn byte(&self, at: usize) -> u8 {
        self.source.get(at).copied().unwrap_or(0)
    }

    fn nesting_blocks(&self) -> bool {
        matches!(
            self.dialect,
            Dialect::Rust | Dialect::Kotlin | Dialect::Swift
        )
    }

    fn has_block_comments(&self) -> bool {
        self.dialect != Dialect::Zig
    }

    fn multiline_strings(&self) -> bool {
        self.dialect == Dialect::Rust
    }

    fn continued_line_comments(&self) -> bool {
        matches!(self.dialect, Dialect::C | Dialect::Cpp)
    }

    fn scripty(&self) -> bool {
        matches!(
            self.dialect,
            Dialect::JavaScript | Dialect::TypeScript | Dialect::Jsx
        )
    }

    fn markup(&self) -> bool {
        matches!(self.dialect, Dialect::JavaScript | Dialect::Jsx)
    }

    fn code(&mut self, depth: usize, stop_at_brace: bool) -> Result<(), Bail> {
        if depth > DEPTH {
            return Err(Bail("nested too deeply to read"));
        }
        let mut braces = 0usize;
        while self.at < self.len() {
            let byte = self.byte(self.at);
            match byte {
                b'/' if self.byte(self.at + 1) == b'/' => self.line_comment(),
                b'/' if self.byte(self.at + 1) == b'*' && self.has_block_comments() => {
                    self.block_comment()?
                }
                b'/' if self.scripty() && self.prev == Prev::Operator && self.regex() => {}
                b'"' => self.double_quoted()?,
                b'\'' => self.single_quoted()?,
                b'`' => self.backtick(depth)?,
                b'\\' if self.dialect == Dialect::Zig && self.byte(self.at + 1) == b'\\' => {
                    self.zig_line_string()
                }
                b'@' | b'$' if self.dialect == Dialect::CSharp && self.sharp_prefix()? => {}
                b'#' if self.dialect == Dialect::Swift && self.swift_raw()? => {}
                b'<' if self.markup() && self.prev == Prev::Operator && self.opens_element() => {
                    self.element(depth + 1)?
                }
                b'{' => {
                    braces += 1;
                    self.at += 1;
                    self.prev = Prev::Operator;
                }
                b'}' => {
                    if braces == 0 && stop_at_brace {
                        self.at += 1;
                        self.prev = Prev::Value;
                        return Ok(());
                    }
                    braces = braces.saturating_sub(1);
                    self.at += 1;
                    self.prev = Prev::Value;
                }
                b')' | b']' => {
                    self.at += 1;
                    self.prev = Prev::Value;
                }
                _ if identifier_byte(byte) => self.word()?,
                _ => {
                    self.at += char_width(self.source, self.at);
                    if !byte.is_ascii_whitespace() {
                        self.prev = Prev::Operator;
                    }
                }
            }
        }
        if stop_at_brace {
            return Err(Bail("unclosed brace"));
        }
        Ok(())
    }

    fn word(&mut self) -> Result<(), Bail> {
        let start = self.at;
        if self.dialect == Dialect::Rust && self.raw_or_byte()? {
            return Ok(());
        }
        if self.dialect == Dialect::Cpp && self.cpp_raw()? {
            return Ok(());
        }
        while self.at < self.len() && identifier_byte(self.byte(self.at)) {
            self.at += char_width(self.source, self.at);
        }
        let word = &self.source[start..self.at];
        self.prev = if OPERATOR_WORDS.contains(&word) {
            Prev::Operator
        } else {
            Prev::Value
        };
        Ok(())
    }

    fn line_comment(&mut self) {
        let start = self.at;
        let mut at = self.at + 2;
        loop {
            at = line_end(self.source, at);
            if !self.continued_line_comments() || at >= self.len() {
                break;
            }
            let mut back = at;
            if back > start && self.byte(back - 1) == b'\r' {
                back -= 1;
            }
            if back > start && self.byte(back - 1) == b'\\' {
                at += 1;
                continue;
            }
            break;
        }
        self.out.comment(start..at, start + 2..at);
        self.at = at;
    }

    fn block_comment(&mut self) -> Result<(), Bail> {
        let start = self.at;
        let mut at = self.at + 2;
        let mut depth = 1usize;
        while at < self.len() {
            if self.nesting_blocks() && starts_with(self.source, at, b"/*") {
                depth += 1;
                at += 2;
                continue;
            }
            if starts_with(self.source, at, b"*/") {
                at += 2;
                depth -= 1;
                if depth == 0 {
                    self.out.comment(start..at, start + 2..at - 2);
                    self.at = at;
                    return Ok(());
                }
                continue;
            }
            at += 1;
        }
        Err(Bail("unterminated block comment"))
    }

    fn double_quoted(&mut self) -> Result<(), Bail> {
        if self.triples() && starts_with(self.source, self.at, b"\"\"\"") {
            return self.triple();
        }
        self.quoted(b'"', true, true, self.multiline_strings())
    }

    fn triples(&self) -> bool {
        matches!(
            self.dialect,
            Dialect::Java | Dialect::Kotlin | Dialect::Swift | Dialect::CSharp
        )
    }

    fn single_quoted(&mut self) -> Result<(), Bail> {
        match self.dialect {
            Dialect::JavaScript | Dialect::TypeScript | Dialect::Jsx => {
                self.quoted(b'\'', true, true, false)
            }
            Dialect::Swift => {
                self.at += 1;
                self.prev = Prev::Operator;
                Ok(())
            }
            Dialect::Rust => self.rust_quote(),
            _ => self.character(),
        }
    }

    fn rust_quote(&mut self) -> Result<(), Bail> {
        let next = self.byte(self.at + 1);
        if next == b'\\' {
            return self.character();
        }
        if next != 0 {
            let after = self.at + 1 + char_width(self.source, self.at + 1);
            if self.byte(after) == b'\'' {
                return self.character();
            }
        }
        self.at += 1;
        self.prev = Prev::Operator;
        Ok(())
    }

    fn character(&mut self) -> Result<(), Bail> {
        let mut at = self.at + 1;
        while at < self.len() {
            match self.byte(at) {
                b'\\' => at += 1 + char_width(self.source, (at + 1).min(self.len() - 1)),
                b'\'' => {
                    self.at = at + 1;
                    self.prev = Prev::Value;
                    return Ok(());
                }
                b'\n' => return Err(Bail("unterminated character literal")),
                _ => at += char_width(self.source, at),
            }
        }
        Err(Bail("unterminated character literal"))
    }

    fn quoted(
        &mut self,
        quote: u8,
        escapes: bool,
        folds: bool,
        multiline: bool,
    ) -> Result<(), Bail> {
        self.at += 1;
        let start = self.at;
        while self.at < self.len() {
            let byte = self.byte(self.at);
            if escapes && byte == b'\\' {
                self.at += 1;
                if self.at < self.len() {
                    self.at += char_width(self.source, self.at);
                }
                continue;
            }
            if byte == quote {
                self.out.text(start..self.at, escapes, folds, quote);
                self.at += 1;
                self.prev = Prev::Value;
                return Ok(());
            }
            if byte == b'\n' && !multiline {
                return Err(Bail("unterminated string"));
            }
            self.at += char_width(self.source, self.at);
        }
        Err(Bail("unterminated string"))
    }

    fn triple(&mut self) -> Result<(), Bail> {
        self.at += 3;
        let start = self.at;
        while self.at < self.len() {
            if self.byte(self.at) == b'\\' && self.dialect != Dialect::Kotlin {
                self.at += 1;
                if self.at < self.len() {
                    self.at += char_width(self.source, self.at);
                }
                continue;
            }
            if starts_with(self.source, self.at, b"\"\"\"") {
                self.out
                    .text(start..self.at, self.dialect != Dialect::Kotlin, true, 0);
                self.at += 3;
                self.prev = Prev::Value;
                return Ok(());
            }
            self.at += char_width(self.source, self.at);
        }
        Err(Bail("unterminated text block"))
    }

    fn backtick(&mut self, depth: usize) -> Result<(), Bail> {
        match self.dialect {
            Dialect::Go => self.quoted(b'`', false, true, true),
            Dialect::Kotlin => {
                self.at += 1;
                while self.at < self.len() && self.byte(self.at) != b'`' {
                    self.at += char_width(self.source, self.at);
                }
                if self.at >= self.len() {
                    return Err(Bail("unterminated quoted name"));
                }
                self.at += 1;
                self.prev = Prev::Value;
                Ok(())
            }
            Dialect::JavaScript | Dialect::TypeScript | Dialect::Jsx => self.template(depth),
            _ => {
                self.at += 1;
                self.prev = Prev::Operator;
                Ok(())
            }
        }
    }

    fn template(&mut self, depth: usize) -> Result<(), Bail> {
        self.at += 1;
        let mut start = self.at;
        while self.at < self.len() {
            let byte = self.byte(self.at);
            if byte == b'\\' {
                self.at += 1;
                if self.at < self.len() {
                    self.at += char_width(self.source, self.at);
                }
                continue;
            }
            if byte == b'`' {
                self.out.text(start..self.at, true, true, b'`');
                self.at += 1;
                self.prev = Prev::Value;
                return Ok(());
            }
            if byte == b'$' && self.byte(self.at + 1) == b'{' {
                self.out.text(start..self.at, true, true, b'`');
                self.at += 2;
                self.code(depth + 1, true)?;
                start = self.at;
                continue;
            }
            self.at += char_width(self.source, self.at);
        }
        Err(Bail("unterminated template literal"))
    }

    fn raw_or_byte(&mut self) -> Result<bool, Bail> {
        let mut at = self.at;
        if self.byte(at) == b'b' {
            at += 1;
        }
        if self.byte(at) != b'r' {
            if at > self.at && self.byte(at) == b'\'' {
                self.at = at;
                self.character()?;
                return Ok(true);
            }
            if at > self.at && self.byte(at) == b'"' {
                self.at = at;
                self.quoted(b'"', true, true, true)?;
                return Ok(true);
            }
            return Ok(false);
        }
        at += 1;
        let mut hashes = 0usize;
        while self.byte(at) == b'#' {
            hashes += 1;
            at += 1;
        }
        if self.byte(at) != b'"' {
            return Ok(false);
        }
        self.at = at + 1;
        let start = self.at;
        while self.at < self.len() {
            if self.byte(self.at) == b'"' && self.closing_hashes(self.at + 1, hashes) {
                self.out.text(
                    start..self.at,
                    false,
                    true,
                    if hashes == 0 { b'"' } else { 0 },
                );
                self.at += 1 + hashes;
                self.prev = Prev::Value;
                return Ok(true);
            }
            self.at += char_width(self.source, self.at);
        }
        Err(Bail("unterminated raw string"))
    }

    fn closing_hashes(&self, from: usize, hashes: usize) -> bool {
        (0..hashes).all(|offset| self.byte(from + offset) == b'#')
    }

    fn cpp_raw(&mut self) -> Result<bool, Bail> {
        if self.byte(self.at) != b'R' || self.byte(self.at + 1) != b'"' {
            return Ok(false);
        }
        let mut at = self.at + 2;
        let tag = at;
        while at < self.len() && self.byte(at) != b'(' && at - tag <= 16 {
            at += 1;
        }
        if self.byte(at) != b'(' {
            return Ok(false);
        }
        let mut closing = Vec::with_capacity(18);
        closing.push(b')');
        closing.extend_from_slice(&self.source[tag..at]);
        closing.push(b'"');
        let start = at + 1;
        let mut cursor = start;
        while cursor < self.len() {
            if starts_with(self.source, cursor, &closing) {
                self.out.text(start..cursor, false, true, 0);
                self.at = cursor + closing.len();
                self.prev = Prev::Value;
                return Ok(true);
            }
            cursor += 1;
        }
        Err(Bail("unterminated raw string"))
    }

    fn sharp_prefix(&mut self) -> Result<bool, Bail> {
        let mut at = self.at;
        let mut verbatim = false;
        while matches!(self.byte(at), b'@' | b'$') {
            verbatim |= self.byte(at) == b'@';
            at += 1;
        }
        if self.byte(at) != b'"' {
            return Ok(false);
        }
        if !verbatim {
            self.at = at;
            self.double_quoted()?;
            return Ok(true);
        }
        self.at = at + 1;
        let start = self.at;
        while self.at < self.len() {
            if self.byte(self.at) == b'"' {
                if self.byte(self.at + 1) == b'"' {
                    self.at += 2;
                    continue;
                }
                self.out.text(start..self.at, false, false, b'"');
                self.at += 1;
                self.prev = Prev::Value;
                return Ok(true);
            }
            self.at += char_width(self.source, self.at);
        }
        Err(Bail("unterminated verbatim string"))
    }

    fn swift_raw(&mut self) -> Result<bool, Bail> {
        let mut at = self.at;
        let mut hashes = 0usize;
        while self.byte(at) == b'#' {
            hashes += 1;
            at += 1;
        }
        if self.byte(at) != b'"' || hashes == 0 {
            return Ok(false);
        }
        self.at = at + 1;
        let start = self.at;
        while self.at < self.len() {
            if self.byte(self.at) == b'"' && self.closing_hashes(self.at + 1, hashes) {
                self.out.text(start..self.at, false, true, 0);
                self.at += 1 + hashes;
                self.prev = Prev::Value;
                return Ok(true);
            }
            self.at += char_width(self.source, self.at);
        }
        Err(Bail("unterminated raw string"))
    }

    fn zig_line_string(&mut self) {
        let start = self.at + 2;
        let end = line_end(self.source, start);
        self.out.text(start..end, false, true, 0);
        self.at = end;
        self.prev = Prev::Value;
    }

    fn regex(&mut self) -> bool {
        let mut at = self.at + 1;
        let mut class = false;
        while at < self.len() {
            match self.byte(at) {
                b'\\' => at += 2,
                b'[' => {
                    class = true;
                    at += 1;
                }
                b']' if class => {
                    class = false;
                    at += 1;
                }
                b'/' if !class => {
                    at += 1;
                    while at < self.len() && identifier_byte(self.byte(at)) {
                        at += 1;
                    }
                    self.at = at;
                    self.prev = Prev::Value;
                    return true;
                }
                b'\n' => return false,
                _ => at += char_width(self.source, at),
            }
        }
        false
    }

    fn opens_element(&self) -> bool {
        let next = self.byte(self.at + 1);
        next == b'>' || next == b'_' || next == b'$' || next.is_ascii_alphabetic()
    }

    fn element(&mut self, depth: usize) -> Result<(), Bail> {
        if depth > DEPTH {
            return Err(Bail("nested too deeply to read"));
        }
        self.at += 1;
        while self.at < self.len()
            && (identifier_byte(self.byte(self.at))
                || matches!(self.byte(self.at), b'.' | b'-' | b':'))
        {
            self.at += 1;
        }
        loop {
            if self.at >= self.len() {
                return Err(Bail("unterminated element"));
            }
            match self.byte(self.at) {
                b'/' if self.byte(self.at + 1) == b'>' => {
                    self.at += 2;
                    self.prev = Prev::Value;
                    return Ok(());
                }
                b'>' => {
                    self.at += 1;
                    break;
                }
                b'{' => {
                    self.at += 1;
                    self.prev = Prev::Operator;
                    self.code(depth + 1, true)?;
                }
                b'"' => self.quoted(b'"', false, true, false)?,
                b'\'' => self.quoted(b'\'', false, true, false)?,
                _ => self.at += char_width(self.source, self.at),
            }
        }
        loop {
            if self.at >= self.len() {
                return Err(Bail("unterminated element"));
            }
            match self.byte(self.at) {
                b'{' => {
                    self.at += 1;
                    self.prev = Prev::Operator;
                    self.code(depth + 1, true)?;
                }
                b'<' if self.byte(self.at + 1) == b'/' => {
                    self.at += 2;
                    while self.at < self.len() && self.byte(self.at) != b'>' {
                        self.at += 1;
                    }
                    if self.at >= self.len() {
                        return Err(Bail("unterminated element"));
                    }
                    self.at += 1;
                    self.prev = Prev::Value;
                    return Ok(());
                }
                b'<' => self.element(depth + 1)?,
                _ => self.at += char_width(self.source, self.at),
            }
        }
    }
}
