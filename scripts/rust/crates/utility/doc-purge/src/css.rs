use crate::lang::Dialect;
use crate::scan::{Bail, Out, char_width, line_end, starts_with};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Url {
    Raw,
    Sass,
}

pub fn scan(source: &[u8], dialect: Dialect) -> Result<Out, Bail> {
    let mut scanner = Css {
        source,
        at: 0,
        dialect,
        out: Out::default(),
        declaration_start: true,
        custom: false,
        custom_depth: 0,
    };
    scanner.code()?;
    Ok(scanner.out)
}

struct Css<'a> {
    source: &'a [u8],
    at: usize,
    dialect: Dialect,
    out: Out,
    declaration_start: bool,
    custom: bool,
    custom_depth: usize,
}

impl Css<'_> {
    fn len(&self) -> usize {
        self.source.len()
    }

    fn byte(&self, at: usize) -> u8 {
        self.source.get(at).copied().unwrap_or(0)
    }

    fn line_comments(&self) -> bool {
        self.dialect != Dialect::Css
    }

    fn indented(&self) -> bool {
        self.dialect == Dialect::Sass
    }

    fn scss(&self) -> bool {
        matches!(self.dialect, Dialect::Scss | Dialect::Sass)
    }

    fn tracks_custom(&self) -> bool {
        self.scss()
    }

    fn block_to_eof(&self) -> bool {
        self.dialect == Dialect::Stylus
    }

    fn url_kind(&self) -> Url {
        if self.scss() { Url::Sass } else { Url::Raw }
    }

    fn ident_start(&self, byte: u8) -> bool {
        byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'-') || byte >= 0x80
    }

    fn ident_byte(&self, byte: u8) -> bool {
        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-') || byte >= 0x80
    }

    fn code(&mut self) -> Result<(), Bail> {
        while self.at < self.len() {
            let byte = self.byte(self.at);
            match byte {
                b'/' if self.byte(self.at + 1) == b'*' => self.block_comment()?,
                b'/' if self.byte(self.at + 1) == b'/' && self.line_comments() && !self.custom => {
                    self.line_comment();
                }
                b'"' | b'\'' => self.quoted(byte)?,
                b'`' if self.dialect == Dialect::Less => self.less_js()?,
                b'\\' => {
                    self.escape();
                    if !self.custom {
                        self.declaration_start = false;
                    }
                }
                b'@' if self.dialect == Dialect::Stylus && self.stylus_css() => {
                    self.stylus_literal()?;
                }
                b'u' | b'U' if self.url_ahead() => self.url()?,
                b'{' if self.custom => {
                    self.custom_depth += 1;
                    self.at += 1;
                }
                b'}' if self.custom && self.custom_depth > 0 => {
                    self.custom_depth -= 1;
                    self.at += 1;
                }
                b';' if self.custom && self.custom_depth == 0 => {
                    self.custom = false;
                    self.at += 1;
                    self.declaration_start = true;
                }
                b'}' if self.custom => {
                    self.custom = false;
                    self.at += 1;
                    self.declaration_start = true;
                }
                b'\n' if self.custom && self.indented() => {
                    self.custom = false;
                    self.at += 1;
                    self.declaration_start = true;
                }
                _ if self.ident_start(byte) => self.word(),
                _ => self.advance(byte),
            }
        }
        Ok(())
    }

    fn advance(&mut self, byte: u8) {
        self.at += char_width(self.source, self.at);
        if self.custom {
            return;
        }
        match byte {
            b'{' | b'}' | b';' => self.declaration_start = true,
            b'\n' if self.indented() => self.declaration_start = true,
            _ if byte.is_ascii_whitespace() => {}
            _ => self.declaration_start = false,
        }
    }

    fn word(&mut self) {
        if self.tracks_custom() && self.declaration_start && self.custom_ahead() {
            self.at += 2;
            while self.at < self.len() && self.ident_byte(self.byte(self.at)) {
                self.at += char_width(self.source, self.at);
            }
            self.custom = true;
            self.custom_depth = 0;
            self.declaration_start = false;
            return;
        }
        while self.at < self.len() && self.ident_byte(self.byte(self.at)) {
            self.at += char_width(self.source, self.at);
        }
        self.declaration_start = false;
    }

    fn custom_ahead(&self) -> bool {
        if self.byte(self.at) != b'-' || self.byte(self.at + 1) != b'-' {
            return false;
        }
        let mut at = self.at + 2;
        let name = at;
        while at < self.len() && (self.ident_byte(self.byte(at)) || self.byte(at) == b'*') {
            at += char_width(self.source, at);
        }
        if at == name {
            return false;
        }
        while at < self.len() && self.byte(at).is_ascii_whitespace() {
            at += 1;
        }
        self.byte(at) == b':' && self.byte(at + 1) != b':'
    }

    fn line_comment(&mut self) {
        if self.indented() && self.line_starts_at(self.at) {
            self.indented_silent();
        } else {
            let start = self.at;
            let end = line_end(self.source, start + 2);
            self.out.comment(start..end, start + 2..end);
            self.at = end;
        }
    }

    fn indented_silent(&mut self) {
        let start = self.at;
        let base = self.indent_at(start);
        let mut end = line_end(self.source, start + 2);
        let mut cursor = end;
        while cursor < self.len() {
            if self.byte(cursor) != b'\n' {
                break;
            }
            cursor += 1;
            let next = line_end(self.source, cursor);
            let indent = self.indent_at(cursor);
            let blank = self.source[cursor..next]
                .iter()
                .all(|byte| byte.is_ascii_whitespace());
            if blank {
                cursor = next;
                continue;
            }
            if indent > base {
                end = next;
                cursor = next;
                continue;
            }
            break;
        }
        self.out.comment(start..end, start + 2..end);
        self.at = end;
    }

    fn line_starts_at(&self, at: usize) -> bool {
        let mut begin = at;
        while begin > 0 && self.source[begin - 1] != b'\n' {
            begin -= 1;
        }
        self.source[begin..at]
            .iter()
            .all(|byte| matches!(byte, b' ' | b'\t' | b'\r'))
    }

    fn indent_at(&self, at: usize) -> usize {
        let mut begin = at;
        while begin > 0 && self.source[begin - 1] != b'\n' {
            begin -= 1;
        }
        let mut cursor = begin;
        while cursor < self.len() && matches!(self.byte(cursor), b' ' | b'\t') {
            cursor += 1;
        }
        cursor - begin
    }

    fn block_comment(&mut self) -> Result<(), Bail> {
        if self.indented() && self.line_starts_at(self.at) {
            self.indented_block();
            return Ok(());
        }
        match self.closed_block() {
            Ok(()) => Ok(()),
            Err(_) if self.block_to_eof() => {
                let start = self.at;
                self.out.comment(start..self.len(), start + 2..self.len());
                self.at = self.len();
                Ok(())
            }
            Err(bail) => Err(bail),
        }
    }

    fn indented_block(&mut self) {
        let start = self.at;
        let base = self.indent_at(start);
        let first = line_end(self.source, start + 2);
        if let Some(close) = close_at(self.source, start + 2, first) {
            let end = close + 2;
            self.out.comment(start..end, start + 2..end - 2);
            self.at = end;
            return;
        }
        let mut end = first;
        let mut cursor = first;
        while cursor < self.len() {
            if self.byte(cursor) != b'\n' {
                break;
            }
            cursor += 1;
            let next = line_end(self.source, cursor);
            let indent = self.indent_at(cursor);
            let blank = self.source[cursor..next]
                .iter()
                .all(|byte| byte.is_ascii_whitespace());
            if blank {
                cursor = next;
                continue;
            }
            if let Some(close) = close_at(self.source, cursor, next) {
                let before = &self.source[cursor..close];
                if indent > base || before.iter().all(|byte| byte.is_ascii_whitespace()) {
                    let close = close + 2;
                    self.out.comment(start..close, start + 2..close - 2);
                    self.at = close;
                    return;
                }
                break;
            }
            if indent > base {
                end = next;
                cursor = next;
                continue;
            }
            break;
        }
        self.out.comment(start..end, start + 2..end);
        self.at = end;
    }

    fn quoted(&mut self, quote: u8) -> Result<(), Bail> {
        self.at += 1;
        let mut start = self.at;
        while self.at < self.len() {
            let byte = self.byte(self.at);
            if byte == b'\\' {
                self.escape_line();
                continue;
            }
            if self.scss() && byte == b'#' && self.byte(self.at + 1) == b'{' {
                self.out.text(start..self.at, true, true, quote);
                self.interpolation(b'#')?;
                start = self.at;
                continue;
            }
            if byte == quote {
                self.out.text(start..self.at, true, true, quote);
                self.at += 1;
                if !self.custom {
                    self.declaration_start = false;
                }
                return Ok(());
            }
            if matches!(byte, b'\n' | b'\r') {
                return Err(Bail("unterminated string"));
            }
            self.at += char_width(self.source, self.at);
        }
        Err(Bail("unterminated string"))
    }

    fn skip_string(&mut self, quote: u8) -> Result<(), Bail> {
        self.at += 1;
        while self.at < self.len() {
            let byte = self.byte(self.at);
            if byte == b'\\' {
                self.escape_line();
                continue;
            }
            if self.scss() && byte == b'#' && self.byte(self.at + 1) == b'{' {
                self.interpolation(b'#')?;
                continue;
            }
            if byte == quote {
                self.at += 1;
                return Ok(());
            }
            if matches!(byte, b'\n' | b'\r') {
                return Err(Bail("unterminated string"));
            }
            self.at += char_width(self.source, self.at);
        }
        Err(Bail("unterminated string"))
    }

    fn interpolation(&mut self, prefix: u8) -> Result<(), Bail> {
        self.at += 2;
        let mut depth = 1usize;
        while self.at < self.len() {
            let byte = self.byte(self.at);
            match byte {
                b'\\' => self.escape(),
                b'"' | b'\'' => self.skip_string(byte)?,
                b'/' if self.byte(self.at + 1) == b'*' => self.skip_block()?,
                b'#' if self.byte(self.at + 1) == b'{' && prefix == b'#' => {
                    self.at += 2;
                    depth += 1;
                }
                b'@' if self.byte(self.at + 1) == b'{' && prefix == b'@' => {
                    self.at += 2;
                    depth += 1;
                }
                b'{' => {
                    self.at += 1;
                    depth += 1;
                }
                b'}' => {
                    self.at += 1;
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                _ => self.at += char_width(self.source, self.at),
            }
        }
        Err(Bail("unterminated interpolation"))
    }

    fn skip_block(&mut self) -> Result<(), Bail> {
        self.at += 2;
        while self.at < self.len() {
            if starts_with(self.source, self.at, b"*/") {
                self.at += 2;
                return Ok(());
            }
            self.at += char_width(self.source, self.at);
        }
        Err(Bail("unterminated block comment"))
    }

    fn escape(&mut self) {
        self.at += 1;
        if self.at < self.len() {
            self.at += char_width(self.source, self.at);
        }
    }

    fn escape_line(&mut self) {
        self.at += 1;
        if self.at >= self.len() {
            return;
        }
        match self.byte(self.at) {
            b'\r' if self.byte(self.at + 1) == b'\n' => self.at += 2,
            b'\r' | b'\n' => self.at += 1,
            _ => self.at += char_width(self.source, self.at),
        }
    }

    fn less_js(&mut self) -> Result<(), Bail> {
        self.at += 1;
        while self.at < self.len() {
            if self.byte(self.at) == b'`' {
                self.at += 1;
                self.declaration_start = false;
                return Ok(());
            }
            self.at += char_width(self.source, self.at);
        }
        Err(Bail("unterminated javascript"))
    }

    fn stylus_css(&self) -> bool {
        if !starts_with(self.source, self.at, b"@css") {
            return false;
        }
        let mut at = self.at + 4;
        while at < self.len() && matches!(self.byte(at), b' ' | b'\t') {
            at += 1;
        }
        self.byte(at) == b'{'
    }

    fn closed_block(&mut self) -> Result<(), Bail> {
        let start = self.at;
        let mut at = start + 2;
        while at < self.len() {
            if starts_with(self.source, at, b"*/") {
                at += 2;
                self.out.comment(start..at, start + 2..at - 2);
                self.at = at;
                return Ok(());
            }
            at += char_width(self.source, at);
        }
        Err(Bail("unterminated block comment"))
    }

    fn stylus_literal(&mut self) -> Result<(), Bail> {
        self.at += 4;
        while self.at < self.len() && matches!(self.byte(self.at), b' ' | b'\t') {
            self.at += 1;
        }
        self.at += 1;
        let mut depth = 1usize;
        while self.at < self.len() {
            let byte = self.byte(self.at);
            match byte {
                b'/' if self.byte(self.at + 1) == b'*' => self.closed_block()?,
                b'"' | b'\'' => self.quoted(byte)?,
                b'\\' => self.escape(),
                b'u' | b'U' if self.url_ahead() => self.url()?,
                b'{' => {
                    depth += 1;
                    self.at += 1;
                }
                b'}' => {
                    depth -= 1;
                    self.at += 1;
                    if depth == 0 {
                        self.declaration_start = false;
                        return Ok(());
                    }
                }
                _ => self.at += char_width(self.source, self.at),
            }
        }
        self.declaration_start = false;
        Ok(())
    }

    fn url_ahead(&self) -> bool {
        matches!(self.byte(self.at), b'u' | b'U')
            && matches!(self.byte(self.at + 1), b'r' | b'R')
            && matches!(self.byte(self.at + 2), b'l' | b'L')
            && self.byte(self.at + 3) == b'('
    }

    fn url(&mut self) -> Result<(), Bail> {
        self.at += 4;
        let mut probe = self.at;
        while probe < self.len() && css_whitespace(self.byte(probe)) {
            probe += 1;
        }
        if matches!(self.byte(probe), b'"' | b'\'') {
            return Ok(());
        }
        if self.url_kind() == Url::Sass && !self.sass_url(probe) {
            return Ok(());
        }
        while self.at < self.len() {
            match self.byte(self.at) {
                b'\\' => self.escape(),
                b'#' if self.scss() && self.byte(self.at + 1) == b'{' => {
                    self.interpolation(b'#')?;
                }
                b'@' if self.dialect == Dialect::Less && self.byte(self.at + 1) == b'{' => {
                    self.interpolation(b'@')?;
                }
                b')' => {
                    self.at += 1;
                    if !self.custom {
                        self.declaration_start = false;
                    }
                    return Ok(());
                }
                _ => self.at += char_width(self.source, self.at),
            }
        }
        Err(Bail("unterminated url"))
    }

    fn sass_url(&self, probe: usize) -> bool {
        let mut at = probe;
        let mut seen = false;
        while at < self.len() {
            match self.byte(at) {
                b'\\' => {
                    at += 1;
                    if at < self.len() {
                        at += char_width(self.source, at);
                    }
                    seen = true;
                }
                b'#' if self.byte(at + 1) == b'{' => {
                    let Some(end) = self.interpolation_end(at, b'#') else {
                        return false;
                    };
                    at = end;
                    seen = true;
                }
                b')' => return seen || at > probe,
                byte if css_whitespace(byte) => {
                    let mut after = at;
                    while after < self.len() && css_whitespace(self.byte(after)) {
                        after += 1;
                    }
                    return self.byte(after) == b')';
                }
                b'"' | b'\'' | b'(' | b'$' => return false,
                byte if byte < 0x20 || byte == 0x7f => return false,
                _ => {
                    seen = true;
                    at += char_width(self.source, at);
                }
            }
        }
        false
    }

    fn interpolation_end(&self, from: usize, prefix: u8) -> Option<usize> {
        let mut at = from + 2;
        let mut depth = 1usize;
        while at < self.len() {
            match self.byte(at) {
                b'\\' => {
                    at += 1;
                    if at < self.len() {
                        at += char_width(self.source, at);
                    }
                }
                b'"' | b'\'' => at = self.string_end(at)?,
                b'/' if self.byte(at + 1) == b'*' => {
                    at += 2;
                    while at < self.len() {
                        if starts_with(self.source, at, b"*/") {
                            at += 2;
                            break;
                        }
                        at += 1;
                    }
                }
                b'#' if self.byte(at + 1) == b'{' && prefix == b'#' => {
                    at += 2;
                    depth += 1;
                }
                b'@' if self.byte(at + 1) == b'{' && prefix == b'@' => {
                    at += 2;
                    depth += 1;
                }
                b'{' => {
                    at += 1;
                    depth += 1;
                }
                b'}' => {
                    at += 1;
                    depth -= 1;
                    if depth == 0 {
                        return Some(at);
                    }
                }
                _ => at += char_width(self.source, at),
            }
        }
        None
    }

    fn string_end(&self, at: usize) -> Option<usize> {
        let quote = self.byte(at);
        let mut at = at + 1;
        while at < self.len() {
            match self.byte(at) {
                b'\\' => {
                    at += 1;
                    if at < self.len() {
                        at += char_width(self.source, at);
                    }
                }
                b'#' if self.scss() && self.byte(at + 1) == b'{' => {
                    at = self.interpolation_end(at, b'#')?;
                }
                b'\n' | b'\r' => return None,
                byte if byte == quote => return Some(at + 1),
                _ => at += char_width(self.source, at),
            }
        }
        None
    }
}

fn css_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0c)
}

fn close_at(source: &[u8], from: usize, to: usize) -> Option<usize> {
    let mut at = from;
    while at + 1 < to {
        if source[at] == b'*' && source[at + 1] == b'/' {
            return Some(at);
        }
        at += 1;
    }
    None
}
