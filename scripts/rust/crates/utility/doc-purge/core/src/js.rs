use crate::lang::Dialect;
use crate::scan::{Bail, Out, char_width, starts_with};

const DEPTH: usize = 64;
const BOM: &[u8] = b"\xef\xbb\xbf";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Prev {
    /// A value (identifier, literal, closing delimiter) just ended, so `/`
    /// starts division.
    Value,
    /// An expression may start here, so `/` starts a regular expression.
    Operator,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Last {
    Start,
    Value,
    Keyword,
    Control,
    ControlClose,
    ParenClose,
    BracketClose,
    BlockClose,
    ObjectClose,
    Arrow,
    AngleClose,
    BlockKeyword,
    Semicolon,
    Comma,
    Colon,
    Open,
    Dot,
    Operator,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Brace {
    Block,
    ExpressionBlock,
    Object,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Paren {
    Normal,
    Control(&'static [u8]),
    Function(bool),
}

struct Js<'a> {
    source: &'a [u8],
    at: usize,
    dialect: Dialect,
    out: Out,
    prev: Prev,
    last: Last,
    parens: Vec<Paren>,
    braces: Vec<Brace>,
    pending_control: Option<&'static [u8]>,
    pending_function: Option<bool>,
    pending_async: Option<bool>,
    pending_class: Option<(bool, usize)>,
    next_body: Option<Brace>,
    type_args_depth: usize,
    last_type_operator: bool,
    line_start: bool,
    token_line_start: bool,
}

pub fn scan(source: &[u8], dialect: Dialect) -> Result<Out, Bail> {
    let mut scanner = Js {
        source,
        at: 0,
        dialect,
        out: Out::default(),
        prev: Prev::Operator,
        last: Last::Start,
        parens: Vec::new(),
        braces: Vec::new(),
        pending_control: None,
        pending_function: None,
        pending_async: None,
        pending_class: None,
        next_body: None,
        type_args_depth: 0,
        last_type_operator: false,
        line_start: true,
        token_line_start: true,
    };
    scanner.hashbang();
    scanner.code(0, false)?;
    Ok(scanner.out)
}

impl Js<'_> {
    fn len(&self) -> usize {
        self.source.len()
    }

    fn byte(&self, at: usize) -> u8 {
        self.source.get(at).copied().unwrap_or(0)
    }

    fn markup(&self) -> bool {
        matches!(
            self.dialect,
            Dialect::JavaScript | Dialect::Mjs | Dialect::Jsx | Dialect::Tsx
        )
    }

    fn html_comments(&self) -> bool {
        self.dialect == Dialect::JavaScript
    }

    fn typescript(&self) -> bool {
        matches!(self.dialect, Dialect::TypeScript | Dialect::Tsx)
    }

    fn hashbang(&mut self) {
        let mut at = 0usize;
        if starts_with(self.source, at, BOM) {
            at += BOM.len();
        }
        if !starts_with(self.source, at, b"#!") {
            return;
        }
        let end = js_line_end(self.source, at + 2);
        self.out.comment(at..end, at + 2..end);
        self.at = end;
        self.line_start = true;
    }

    fn code(&mut self, depth: usize, stop_at_brace: bool) -> Result<(), Bail> {
        if depth > DEPTH {
            return Err(Bail("nested too deeply to read"));
        }
        let mut braces = 0usize;
        while self.at < self.len() {
            let byte = self.byte(self.at);
            if byte == b'\n' {
                self.at += 1;
                self.pending_async = None;
                self.line_start = true;
                continue;
            }
            if byte == b'\r' {
                self.at += if self.byte(self.at + 1) == b'\n' {
                    2
                } else {
                    1
                };
                self.pending_async = None;
                self.line_start = true;
                continue;
            }
            if starts_with(self.source, self.at, "\u{2028}".as_bytes())
                || starts_with(self.source, self.at, "\u{2029}".as_bytes())
            {
                self.at += 3;
                self.pending_async = None;
                self.line_start = true;
                continue;
            }
            if let Some(width) = self.space_width() {
                self.at += width;
                continue;
            }
            if self.byte(self.at) == b'/' && self.byte(self.at + 1) == b'/' {
                self.line_comment(self.at + 2);
                continue;
            }
            if self.byte(self.at) == b'/' && self.byte(self.at + 1) == b'*' {
                self.block_comment()?;
                continue;
            }
            if self.html_comments() && starts_with(self.source, self.at, b"<!--") {
                self.line_comment(self.at + 4);
                continue;
            }
            if self.html_comments() && self.line_start && starts_with(self.source, self.at, b"-->")
            {
                self.line_comment(self.at + 3);
                continue;
            }

            self.token_line_start = self.line_start;
            self.line_start = false;

            match byte {
                b'/' => {
                    if self.prev == Prev::Operator && self.regex() {
                        continue;
                    }
                    self.at += 1;
                    self.operator(Last::Operator);
                }
                b'"' | b'\'' => self.quoted(byte, true, true, false)?,
                b'`' => self.template(depth)?,
                b'<' if self.typescript() && self.type_args_depth > 0 => {
                    self.type_args_depth += 1;
                    self.at += 1;
                    self.operator(Last::Operator);
                }
                b'<' if self.typescript() && self.starts_type_arguments() => {
                    self.type_args_depth = 1;
                    self.at += 1;
                    self.operator(Last::Operator);
                }
                b'<' if self.markup() => {
                    if self.prev == Prev::Operator
                        && (self.flow_cast() || self.generic_parameters())
                    {
                        self.at += 1;
                        self.operator(Last::Operator);
                    } else if self.prev == Prev::Operator && self.opens_element() {
                        self.element(depth + 1)?;
                    } else {
                        self.at += 1;
                        self.operator(Last::Operator);
                    }
                }
                b'<' => {
                    self.at += 1;
                    self.operator(Last::Operator);
                }
                b'>' if self.type_args_depth > 0 => {
                    self.type_args_depth -= 1;
                    self.at += 1;
                    if self.type_args_depth == 0 {
                        self.value(Last::AngleClose);
                    } else {
                        self.operator(Last::Operator);
                    }
                }
                b'>' => {
                    self.at += 1;
                    self.operator(Last::Operator);
                }
                b'=' if self.byte(self.at + 1) == b'>' => {
                    self.at += 2;
                    self.prev = Prev::Operator;
                    self.last = Last::Arrow;
                }
                b'(' => {
                    let kind = if let Some(expression) = self.pending_function.take() {
                        Paren::Function(expression)
                    } else if let Some(control) = self.pending_control.take() {
                        Paren::Control(control)
                    } else {
                        Paren::Normal
                    };
                    self.parens.push(kind);
                    self.at += 1;
                    self.operator(Last::Open);
                }
                b')' => {
                    let kind = self.parens.pop().unwrap_or(Paren::Normal);
                    self.at += 1;
                    match kind {
                        Paren::Control(_) => {
                            self.prev = Prev::Operator;
                            self.last = Last::ControlClose;
                        }
                        Paren::Function(expression) => {
                            self.next_body = Some(if expression {
                                Brace::ExpressionBlock
                            } else {
                                Brace::Block
                            });
                            self.value(Last::ParenClose);
                        }
                        Paren::Normal => self.value(Last::ParenClose),
                    }
                }
                b'[' => {
                    self.at += 1;
                    self.operator(Last::Open);
                }
                b']' => {
                    self.at += 1;
                    self.value(Last::BracketClose);
                }
                b'{' => {
                    let kind = if self.next_body.is_some() && self.type_brace() {
                        Brace::Object
                    } else {
                        self.pending_body().unwrap_or_else(|| self.brace_kind())
                    };
                    self.braces.push(kind);
                    braces += 1;
                    self.at += 1;
                    self.operator(Last::Open);
                }
                b'}' => {
                    if braces == 0 && stop_at_brace {
                        self.at += 1;
                        self.value(Last::Value);
                        return Ok(());
                    }
                    braces = braces.saturating_sub(1);
                    let kind = self.braces.pop().unwrap_or(Brace::Object);
                    self.at += 1;
                    match kind {
                        Brace::Block => {
                            self.prev = Prev::Operator;
                            self.last = Last::BlockClose;
                        }
                        Brace::ExpressionBlock | Brace::Object => self.value(Last::ObjectClose),
                    }
                }
                b'+' | b'-' => {
                    if self.byte(self.at + 1) == byte {
                        self.at += 2;
                    } else {
                        self.at += 1;
                        self.operator(Last::Operator);
                    }
                }
                b'.' => {
                    if self.byte(self.at + 1).is_ascii_digit() {
                        self.at += 1;
                        while self.at < self.len() && self.byte(self.at).is_ascii_digit() {
                            self.at += 1;
                        }
                        self.value(Last::Value);
                    } else if starts_with(self.source, self.at, b"...") {
                        self.at += 3;
                        self.operator(Last::Operator);
                    } else {
                        self.at += 1;
                        self.operator(Last::Dot);
                    }
                }
                b';' => {
                    self.at += 1;
                    self.next_body = None;
                    self.pending_class = None;
                    self.pending_function = None;
                    self.operator(Last::Semicolon);
                }
                b',' => {
                    self.at += 1;
                    self.operator(Last::Comma);
                }
                b':' => {
                    self.at += 1;
                    self.operator(Last::Colon);
                }
                b'#' => {
                    self.at += 1;
                    while self.at < self.len() && self.word_byte() {
                        self.at += char_width(self.source, self.at);
                    }
                    self.value(Last::Value);
                }
                _ if self.word_byte() => self.word(),
                _ => {
                    self.at += char_width(self.source, self.at);
                    self.operator(Last::Operator);
                }
            }
        }
        if stop_at_brace {
            return Err(Bail("unclosed brace"));
        }
        Ok(())
    }

    fn value(&mut self, last: Last) {
        self.prev = Prev::Value;
        self.pending_async = None;
        self.last_type_operator = false;
        self.last = last;
    }

    fn operator(&mut self, last: Last) {
        self.prev = Prev::Operator;
        self.pending_control = None;
        self.pending_async = None;
        self.last_type_operator = false;
        self.last = last;
    }

    fn starts_type_arguments(&self) -> bool {
        if self.prev != Prev::Value || self.at == 0 {
            return false;
        }
        let previous = self.source[self.at - 1];
        if !(previous.is_ascii_alphanumeric()
            || matches!(previous, b'_' | b'$' | b']' | b')')
            || previous >= 0x80)
        {
            return false;
        }
        let Some(end) = angle_end(self.source, self.at) else {
            return false;
        };
        let after = skip_trivia(self.source, end);
        self.byte(after) == b'/' && !matches!(self.byte(after + 1), b'/' | b'*')
    }

    fn type_brace(&self) -> bool {
        self.last == Last::Arrow
            || self.last_type_operator
            || matches!(
                self.last,
                Last::Colon | Last::Comma | Last::Open | Last::Operator
            )
    }

    fn pending_body(&mut self) -> Option<Brace> {
        if let Some(kind) = self.next_body.take() {
            return Some(kind);
        }
        if let Some((expression, depth)) = self.pending_class
            && depth == self.parens.len()
        {
            self.pending_class = None;
            return Some(if expression {
                Brace::ExpressionBlock
            } else {
                Brace::Block
            });
        }
        None
    }

    fn declaration_position(&self) -> bool {
        match self.last {
            Last::Start
            | Last::Semicolon
            | Last::BlockClose
            | Last::ControlClose
            | Last::BlockKeyword => true,
            Last::Open => matches!(self.braces.last(), Some(Brace::Block)),
            Last::Value
            | Last::ParenClose
            | Last::BracketClose
            | Last::ObjectClose
            | Last::AngleClose => self.token_line_start,
            _ => false,
        }
    }

    fn brace_kind(&self) -> Brace {
        match self.last {
            Last::Arrow => Brace::ExpressionBlock,
            Last::Control
            | Last::ControlClose
            | Last::BlockKeyword
            | Last::Semicolon
            | Last::BlockClose
            | Last::Start => Brace::Block,
            Last::Keyword => Brace::Object,
            Last::Value | Last::ParenClose | Last::BracketClose | Last::ObjectClose => Brace::Block,
            Last::Open => {
                if matches!(self.braces.last(), Some(Brace::Block)) {
                    Brace::Block
                } else {
                    Brace::Object
                }
            }
            Last::Comma | Last::Colon | Last::Dot | Last::Operator | Last::AngleClose => {
                Brace::Object
            }
        }
    }

    fn space_width(&self) -> Option<usize> {
        let byte = self.byte(self.at);
        if matches!(byte, b' ' | b'\t' | 0x0b | 0x0c) {
            return Some(1);
        }
        if byte < 0x80 {
            return None;
        }
        let width = char_width(self.source, self.at);
        let text = std::str::from_utf8(&self.source[self.at..self.at + width]).ok()?;
        let letter = text.chars().next()?;
        (letter.is_whitespace() || letter == '\u{feff}').then_some(width)
    }

    fn word_byte(&self) -> bool {
        let byte = self.byte(self.at);
        byte.is_ascii_alphanumeric()
            || matches!(byte, b'_' | b'$')
            || (byte >= 0x80 && self.space_width().is_none())
    }

    fn word(&mut self) {
        let start = self.at;
        while self.at < self.len() && self.word_byte() {
            self.at += char_width(self.source, self.at);
        }
        let word = &self.source[start..self.at];
        self.last_type_operator = matches!(
            word,
            b"keyof"
                | b"typeof"
                | b"readonly"
                | b"infer"
                | b"unique"
                | b"extends"
                | b"implements"
                | b"abstract"
                | b"new"
                | b"asserts"
                | b"is"
        );
        if let Some(pending) = self.pending_control {
            if pending == b"for" && word == b"await" {
                self.prev = Prev::Operator;
                self.last = Last::Keyword;
                return;
            }
            self.pending_control = None;
        }
        if self.last == Last::Dot {
            self.prev = Prev::Value;
            self.last = Last::Value;
            return;
        }
        if word == b"of" && self.is_for_of_operator() {
            self.prev = Prev::Operator;
            self.last = Last::Operator;
            return;
        }
        let async_pending = self.pending_async.take();
        if word == b"async" {
            self.pending_async = Some(self.declaration_position());
        } else if word == b"function" && function_keyword(self.source, self.at) {
            let declaration = async_pending.unwrap_or_else(|| self.declaration_position());
            self.pending_function = Some(!declaration);
        } else if word == b"class" && class_keyword(self.source, self.at) {
            self.pending_class = Some((!self.declaration_position(), self.parens.len()));
        }
        if is_control_keyword(word) {
            self.pending_control = Some(control_keyword(word));
            self.prev = Prev::Operator;
            self.last = Last::Control;
            return;
        }
        if is_block_keyword(word) {
            self.prev = Prev::Operator;
            self.last = Last::BlockKeyword;
            return;
        }
        if is_expression_keyword(word) {
            self.prev = Prev::Operator;
            self.last = Last::Keyword;
            return;
        }
        self.prev = Prev::Value;
        self.last = Last::Value;
    }

    fn is_for_of_operator(&self) -> bool {
        let in_for = self
            .parens
            .last()
            .is_some_and(|kind| *kind == Paren::Control(b"for"));
        let after_binding = self.prev == Prev::Value
            || matches!(
                self.last,
                Last::BracketClose | Last::ObjectClose | Last::BlockClose
            );
        if !in_for || !after_binding {
            return false;
        }
        let next = skip_trivia(self.source, self.at);
        !matches!(
            self.source.get(next).copied(),
            None | Some(b'=' | b';' | b',' | b')')
        )
    }

    fn line_comment(&mut self, body: usize) {
        let end = js_line_end(self.source, body);
        self.out.comment(self.at..end, body..end);
        self.at = end;
    }

    fn block_comment(&mut self) -> Result<(), Bail> {
        let start = self.at;
        let mut at = start + 2;
        let mut multiline = false;
        while at < self.len() {
            match self.byte(at) {
                b'*' if self.byte(at + 1) == b'/' => {
                    self.out.comment(start..at + 2, start + 2..at);
                    self.at = at + 2;
                    if multiline {
                        self.pending_async = None;
                        self.line_start = true;
                    }
                    return Ok(());
                }
                b'\n' => {
                    multiline = true;
                    at += 1;
                }
                b'\r' => {
                    multiline = true;
                    at += if self.byte(at + 1) == b'\n' { 2 } else { 1 };
                }
                _ if starts_with(self.source, at, "\u{2028}".as_bytes())
                    || starts_with(self.source, at, "\u{2029}".as_bytes()) =>
                {
                    multiline = true;
                    at += 3;
                }
                _ => at += char_width(self.source, at),
            }
        }
        Err(Bail("unterminated block comment"))
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
                self.escape();
                continue;
            }
            if byte == quote {
                self.out.text(start..self.at, escapes, folds, quote);
                self.at += 1;
                self.line_start = false;
                self.value(Last::Value);
                return Ok(());
            }
            if !multiline && (byte == b'\n' || byte == b'\r') {
                return Err(Bail("unterminated string"));
            }
            self.at += char_width(self.source, self.at);
        }
        Err(Bail("unterminated string"))
    }

    fn escape(&mut self) {
        self.at += 1;
        if self.at >= self.len() {
            return;
        }
        match self.byte(self.at) {
            b'\r' if self.byte(self.at + 1) == b'\n' => self.at += 2,
            _ => self.at += char_width(self.source, self.at),
        }
    }

    fn template(&mut self, depth: usize) -> Result<(), Bail> {
        if depth > DEPTH {
            return Err(Bail("nested too deeply to read"));
        }
        self.at += 1;
        let mut start = self.at;
        while self.at < self.len() {
            let byte = self.byte(self.at);
            if byte == b'\\' {
                self.escape();
                continue;
            }
            if byte == b'`' {
                self.out.text(start..self.at, true, true, b'`');
                self.at += 1;
                self.line_start = false;
                self.value(Last::Value);
                return Ok(());
            }
            if byte == b'$' && self.byte(self.at + 1) == b'{' {
                self.out.text(start..self.at, true, true, b'`');
                self.at += 2;
                self.prev = Prev::Operator;
                self.last = Last::Open;
                self.line_start = false;
                self.code(depth + 1, true)?;
                start = self.at;
                continue;
            }
            self.at += char_width(self.source, self.at);
        }
        Err(Bail("unterminated template literal"))
    }

    fn regex(&mut self) -> bool {
        let mut at = self.at + 1;
        if at >= self.len() || matches!(self.byte(at), b'/' | b'*') {
            return false;
        }
        let mut class = false;
        while at < self.len() {
            match self.byte(at) {
                b'\\' => {
                    at += 1;
                    if at < self.len() {
                        at += char_width(self.source, at);
                    }
                }
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
                    while at < self.len() && self.regex_flag(at) {
                        at += char_width(self.source, at);
                    }
                    self.at = at;
                    self.line_start = false;
                    self.value(Last::Value);
                    return true;
                }
                b'\n' | b'\r' => return false,
                _ if starts_with(self.source, at, "\u{2028}".as_bytes())
                    || starts_with(self.source, at, "\u{2029}".as_bytes()) =>
                {
                    return false;
                }
                _ => at += char_width(self.source, at),
            }
        }
        false
    }

    fn is_space_at(&self, at: usize) -> bool {
        let byte = self.byte(at);
        if matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c) {
            return true;
        }
        if byte < 0x80 {
            return false;
        }
        let width = char_width(self.source, at);
        let Some(text) = std::str::from_utf8(&self.source[at..at + width]).ok() else {
            return false;
        };
        let Some(letter) = text.chars().next() else {
            return false;
        };
        letter.is_whitespace() || letter == '\u{feff}'
    }

    fn regex_flag(&self, at: usize) -> bool {
        let byte = self.byte(at);
        if byte < 0x80 {
            return byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$');
        }
        !self.is_space_at(at)
    }

    fn flow_cast(&self) -> bool {
        if !matches!(
            self.dialect,
            Dialect::JavaScript | Dialect::Mjs | Dialect::Jsx
        ) {
            return false;
        }
        let Some(end) = angle_end(self.source, self.at) else {
            return false;
        };
        if end >= 2 && self.source[end - 2] == b'/' {
            return false;
        }
        if let Some(name) = tag_name(self.source, self.at + 1, end - 1)
            && has_closing_tag_on_line(self.source, end, name)
        {
            return false;
        }
        let after = skip_trivia(self.source, end);
        let next = self.byte(after);
        let expression_start = ident_start(next)
            || matches!(
                next,
                b'(' | b'[' | b'{' | b'\'' | b'"' | b'`' | b'/' | b'!' | b'-' | b'+' | b'~' | b'<'
            );
        expression_start && !has_closing_element(self.source, end, None)
    }

    fn opens_element(&self) -> bool {
        let next = self.byte(self.at + 1);
        next == b'>' || ident_start(next)
    }

    fn element(&mut self, depth: usize) -> Result<(), Bail> {
        if depth > DEPTH {
            return Err(Bail("nested too deeply to read"));
        }
        self.at += 1;
        self.line_start = false;
        let mut expected = Vec::new();
        if self.byte(self.at) == b'>' {
            self.at += 1;
        } else {
            expected = self.element_name();
            loop {
                self.skip_space();
                if self.at >= self.len() {
                    return Err(Bail("unterminated element"));
                }
                match self.byte(self.at) {
                    b'/' if self.byte(self.at + 1) == b'>' => {
                        self.at += 2;
                        self.line_start = false;
                        self.value(Last::Value);
                        return Ok(());
                    }
                    b'>' => {
                        self.at += 1;
                        self.line_start = false;
                        break;
                    }
                    b'{' => {
                        self.at += 1;
                        self.prev = Prev::Operator;
                        self.last = Last::Open;
                        self.line_start = false;
                        self.code(depth + 1, true)?;
                    }
                    b'<' if self.markup() => self.type_arguments()?,
                    b'"' | b'\'' => {
                        self.quoted(self.byte(self.at), false, true, true)?;
                        self.line_start = false;
                    }
                    b'=' => {
                        self.at += 1;
                        self.skip_space();
                        match self.byte(self.at) {
                            b'"' | b'\'' => {
                                self.quoted(self.byte(self.at), false, true, true)?;
                                self.line_start = false;
                            }
                            b'{' => {
                                self.at += 1;
                                self.prev = Prev::Operator;
                                self.last = Last::Open;
                                self.line_start = false;
                                self.code(depth + 1, true)?;
                            }
                            b'<' if self.markup() && self.opens_element() => {
                                self.element(depth + 1)?;
                            }
                            _ => self.bare_attribute_value(),
                        }
                    }
                    _ => self.attribute_name(),
                }
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
                    self.last = Last::Open;
                    self.line_start = false;
                    self.code(depth + 1, true)?;
                }
                b'<' if self.byte(self.at + 1) == b'/' => {
                    self.at += 2;
                    self.skip_space();
                    let start = self.at;
                    while self.at < self.len()
                        && !self.is_space_at(self.at)
                        && self.byte(self.at) != b'>'
                    {
                        self.at += char_width(self.source, self.at);
                    }
                    if self.source[start..self.at] != expected {
                        return Err(Bail("mismatched closing element"));
                    }
                    self.skip_space();
                    if self.byte(self.at) != b'>' {
                        return Err(Bail("unterminated element"));
                    }
                    self.at += 1;
                    self.line_start = false;
                    self.value(Last::Value);
                    return Ok(());
                }
                b'<' if self.opens_element() => self.element(depth + 1)?,
                b'\n' => {
                    self.at += 1;
                    self.line_start = true;
                }
                b'\r' => {
                    self.at += if self.byte(self.at + 1) == b'\n' {
                        2
                    } else {
                        1
                    };
                    self.line_start = true;
                }
                _ => {
                    self.at += char_width(self.source, self.at);
                    self.line_start = false;
                }
            }
        }
    }

    fn element_name(&mut self) -> Vec<u8> {
        let start = self.at;
        while self.at < self.len() && element_name_byte(self.byte(self.at)) {
            self.at += char_width(self.source, self.at);
        }
        self.line_start = false;
        self.source[start..self.at].to_vec()
    }

    fn attribute_name(&mut self) {
        let start = self.at;
        while self.at < self.len() && attribute_name_byte(self.byte(self.at)) {
            self.at += char_width(self.source, self.at);
        }
        if self.at == start {
            self.at += char_width(self.source, self.at);
        }
        self.line_start = false;
    }

    fn bare_attribute_value(&mut self) {
        while self.at < self.len()
            && !self.is_space_at(self.at)
            && !matches!(self.byte(self.at), b'>' | b'/' | b'{')
        {
            self.at += char_width(self.source, self.at);
        }
        self.line_start = false;
    }

    fn skip_space(&mut self) {
        loop {
            if self.at >= self.len() {
                return;
            }
            match self.byte(self.at) {
                b'\n' => {
                    self.at += 1;
                    self.line_start = true;
                }
                b'\r' => {
                    self.at += if self.byte(self.at + 1) == b'\n' {
                        2
                    } else {
                        1
                    };
                    self.line_start = true;
                }
                _ => {
                    let Some(width) = self.space_width() else {
                        return;
                    };
                    self.at += width;
                }
            }
        }
    }

    fn type_arguments(&mut self) -> Result<(), Bail> {
        let mut angle = 1usize;
        self.at += 1;
        self.line_start = false;
        while self.at < self.len() {
            match self.byte(self.at) {
                b'/' if self.byte(self.at + 1) == b'/' => {
                    self.line_comment(self.at + 2);
                }
                b'/' if self.byte(self.at + 1) == b'*' => self.block_comment()?,
                b'\'' | b'"' => self.quoted(self.byte(self.at), true, true, false)?,
                b'`' => self.template(1)?,
                b'=' if self.byte(self.at + 1) == b'>' => self.at += 2,
                b'<' => {
                    angle += 1;
                    self.at += 1;
                }
                b'>' => {
                    angle -= 1;
                    self.at += 1;
                    if angle == 0 {
                        self.line_start = false;
                        return Ok(());
                    }
                }
                b'\n' => {
                    self.at += 1;
                    self.line_start = true;
                }
                b'\r' => {
                    self.at += if self.byte(self.at + 1) == b'\n' {
                        2
                    } else {
                        1
                    };
                    self.line_start = true;
                }
                _ => self.at += char_width(self.source, self.at),
            }
        }
        Err(Bail("unterminated type arguments"))
    }

    fn generic_parameters(&self) -> bool {
        if !matches!(
            self.dialect,
            Dialect::JavaScript | Dialect::Mjs | Dialect::Jsx | Dialect::Tsx
        ) {
            return false;
        }
        let Some(end) = angle_end(self.source, self.at) else {
            return false;
        };
        let Some(name) = type_parameter_name(self.source, self.at + 1, end - 1) else {
            return false;
        };
        let marked = has_type_parameter_marker(self.source, self.at + 1, end - 1);
        let paren = skip_trivia(self.source, end);
        if self.byte(paren) != b'(' {
            return false;
        }
        let Some(close) = matching_paren(self.source, paren) else {
            return false;
        };
        let after = skip_trivia(self.source, close + 1);
        if !starts_with(self.source, after, b"=>") && !matches!(self.byte(after), b':' | b'{') {
            return false;
        }
        if marked {
            return true;
        }
        if !has_type_annotation(self.source, paren + 1, close) {
            return false;
        }
        if has_closing_tag_on_line(self.source, end, name) {
            return false;
        }
        !has_closing_element(self.source, end, Some(name))
    }
}

fn js_line_end(source: &[u8], from: usize) -> usize {
    let mut at = from;
    while at < source.len() {
        match source[at] {
            b'\n' | b'\r' => return at,
            _ if starts_with(source, at, "\u{2028}".as_bytes())
                || starts_with(source, at, "\u{2029}".as_bytes()) =>
            {
                return at;
            }
            _ => at += char_width(source, at),
        }
    }
    at
}

fn is_control_keyword(word: &[u8]) -> bool {
    matches!(
        word,
        b"if" | b"while" | b"for" | b"with" | b"switch" | b"catch"
    )
}

fn control_keyword(word: &[u8]) -> &'static [u8] {
    match word {
        b"if" => b"if",
        b"while" => b"while",
        b"for" => b"for",
        b"with" => b"with",
        b"switch" => b"switch",
        _ => b"catch",
    }
}

fn is_block_keyword(word: &[u8]) -> bool {
    matches!(
        word,
        b"else" | b"do" | b"try" | b"finally" | b"export" | b"declare" | b"abstract"
    )
}

fn is_expression_keyword(word: &[u8]) -> bool {
    matches!(
        word,
        b"return"
            | b"throw"
            | b"case"
            | b"delete"
            | b"void"
            | b"typeof"
            | b"new"
            | b"instanceof"
            | b"in"
            | b"of"
            | b"yield"
            | b"await"
            | b"default"
            | b"extends"
            | b"function"
            | b"class"
    )
}

fn ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$') || byte >= 0x80
}

fn ident_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$') || byte >= 0x80
}

fn element_name_byte(byte: u8) -> bool {
    ident_continue(byte) || matches!(byte, b'.' | b'-' | b':')
}

fn attribute_name_byte(byte: u8) -> bool {
    ident_continue(byte) || matches!(byte, b'.' | b'-' | b':')
}

fn function_keyword(source: &[u8], after: usize) -> bool {
    let next = source.get(skip_trivia(source, after)).copied();
    !matches!(
        next,
        Some(b':' | b',' | b'}' | b'=' | b';' | b')' | b']' | b'.')
    )
}

fn class_keyword(source: &[u8], after: usize) -> bool {
    let next = source.get(skip_trivia(source, after)).copied();
    !matches!(
        next,
        Some(b':' | b',' | b'}' | b'=' | b';' | b')' | b']' | b'.' | b'(')
    )
}

fn skip_trivia(source: &[u8], mut at: usize) -> usize {
    loop {
        if at >= source.len() {
            return at;
        }
        match source[at] {
            b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c => {
                at += 1;
            }
            b'/' if source.get(at + 1) == Some(&b'/') => {
                at = js_line_end(source, at + 2);
            }
            b'/' if source.get(at + 1) == Some(&b'*') => {
                let mut cursor = at + 2;
                while cursor + 1 < source.len()
                    && !(source[cursor] == b'*' && source[cursor + 1] == b'/')
                {
                    cursor += 1;
                }
                if cursor + 1 >= source.len() {
                    return source.len();
                }
                at = cursor + 2;
            }
            _ if source[at] >= 0x80 => {
                let width = char_width(source, at);
                let Some(text) = std::str::from_utf8(&source[at..at + width]).ok() else {
                    return at;
                };
                let Some(letter) = text.chars().next() else {
                    return at;
                };
                if letter.is_whitespace() || letter == '\u{feff}' {
                    at += width;
                } else {
                    return at;
                }
            }
            _ => return at,
        }
    }
}

fn tag_name(source: &[u8], start: usize, end: usize) -> Option<&[u8]> {
    let mut at = start;
    while at < end && source[at].is_ascii_whitespace() {
        at += 1;
    }
    let begin = at;
    while at < end && element_name_byte(source[at]) {
        at += char_width(source, at);
    }
    (at > begin).then_some(&source[begin..at])
}

fn type_parameter_name(source: &[u8], start: usize, end: usize) -> Option<&[u8]> {
    let mut at = skip_trivia(source, start);
    if at >= end || !ident_start(source[at]) {
        return None;
    }
    let begin = at;
    at += char_width(source, at);
    while at < end && ident_continue(source[at]) {
        at += char_width(source, at);
    }
    let word = &source[begin..at];
    if matches!(word, b"const" | b"in" | b"out") {
        at = skip_trivia(source, at);
        if at >= end || !ident_start(source[at]) {
            return None;
        }
        let begin = at;
        at += char_width(source, at);
        while at < end && ident_continue(source[at]) {
            at += char_width(source, at);
        }
        return Some(&source[begin..at]);
    }
    Some(word)
}

fn semicolon_starts_statement(source: &[u8], mut at: usize) -> bool {
    while at < source.len() && matches!(source[at], b' ' | b'\t') {
        at += 1;
    }
    statement_word(source, at)
}

fn newline_starts_statement(source: &[u8], mut at: usize) -> bool {
    while at < source.len() && matches!(source[at], b' ' | b'\t') {
        at += 1;
    }
    statement_word(source, at)
}

fn statement_word(source: &[u8], at: usize) -> bool {
    let begin = at;
    let mut cursor = at;
    while cursor < source.len() && ident_continue(source[cursor]) {
        cursor += char_width(source, cursor);
    }
    matches!(
        &source[begin..cursor],
        b"const"
            | b"let"
            | b"var"
            | b"function"
            | b"class"
            | b"export"
            | b"import"
            | b"if"
            | b"for"
            | b"while"
            | b"return"
            | b"type"
            | b"interface"
            | b"declare"
            | b"enum"
            | b"namespace"
    )
}

fn has_closing_tag_on_line(source: &[u8], from: usize, name: &[u8]) -> bool {
    let mut at = from;
    while at < source.len() {
        if matches!(source[at], b'\n' | b'\r')
            || source[at..].starts_with("\u{2028}".as_bytes())
            || source[at..].starts_with("\u{2029}".as_bytes())
        {
            return false;
        }
        if source[at] == b'<' && source.get(at + 1) == Some(&b'/') {
            let start = at + 2;
            let end = start + name.len();
            if source.get(start..end) == Some(name)
                && matches!(source.get(end), Some(b'>' | b' ' | b'\t' | b'\r' | b'\n'))
            {
                return true;
            }
        }
        at += char_width(source, at);
    }
    false
}

fn has_closing_element(source: &[u8], from: usize, name: Option<&[u8]>) -> bool {
    let mut at = from;
    let mut depth = 0usize;
    while at < source.len() {
        match source[at] {
            b'(' | b'[' | b'{' => {
                depth += 1;
                at += 1;
            }
            b')' | b']' | b'}' => {
                depth = depth.saturating_sub(1);
                at += 1;
            }
            b';' if depth == 0 && semicolon_starts_statement(source, at + 1) => return false,
            b'\n' if depth == 0 && newline_starts_statement(source, at + 1) => return false,
            b'<' if depth == 0 && source.get(at + 1) == Some(&b'/') => {
                let start = at + 2;
                let matched = match name {
                    Some(name) => {
                        let end = start + name.len();
                        source.get(start..end) == Some(name)
                            && matches!(source.get(end), Some(b'>' | b' ' | b'\t' | b'\r' | b'\n'))
                    }
                    None => true,
                };
                if matched {
                    return true;
                }
                at += 1;
            }
            _ => at += char_width(source, at),
        }
    }
    false
}

fn matching_paren(source: &[u8], open: usize) -> Option<usize> {
    let mut at = open + 1;
    let mut depth = 1usize;
    let mut prev = b'(';
    while at < source.len() {
        match source[at] {
            b'/' if source.get(at + 1) == Some(&b'/') => {
                at = js_line_end(source, at + 2);
            }
            b'/' if source.get(at + 1) == Some(&b'*') => {
                let mut cursor = at + 2;
                while cursor + 1 < source.len()
                    && !(source[cursor] == b'*' && source[cursor + 1] == b'/')
                {
                    cursor += 1;
                }
                if cursor + 1 >= source.len() {
                    return None;
                }
                at = cursor + 2;
            }
            b'/' if regex_allowed(prev) => {
                if let Some(end) = regex_end(source, at) {
                    at = end;
                    prev = b'/';
                } else {
                    prev = b'/';
                    at += 1;
                }
            }
            b'\'' | b'"' => {
                at = skip_quoted(source, at, source[at])?;
                prev = b'"';
            }
            b'`' => {
                at = skip_template(source, at)?;
                prev = b'`';
            }
            b'(' => {
                depth += 1;
                prev = b'(';
                at += 1;
            }
            b')' => {
                depth -= 1;
                at += 1;
                if depth == 0 {
                    return Some(at - 1);
                }
                prev = b')';
            }
            b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c => at += 1,
            _ => {
                prev = source[at];
                at += char_width(source, at);
            }
        }
    }
    None
}

fn regex_allowed(prev: u8) -> bool {
    matches!(
        prev,
        b'=' | b'('
            | b'['
            | b'{'
            | b','
            | b':'
            | b'?'
            | b'+'
            | b'-'
            | b'*'
            | b'/'
            | b'%'
            | b'&'
            | b'|'
            | b'^'
            | b'!'
            | b'~'
            | b'<'
            | b'>'
            | b';'
    )
}

fn regex_end(source: &[u8], start: usize) -> Option<usize> {
    let mut at = start + 1;
    let mut class = false;
    while at < source.len() {
        match source[at] {
            b'\\' => {
                at += 1;
                if at < source.len() {
                    at += char_width(source, at);
                }
            }
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
                while at < source.len() {
                    let byte = source[at];
                    if byte < 0x80 {
                        if !byte.is_ascii_alphanumeric() && !matches!(byte, b'_' | b'$') {
                            break;
                        }
                        at += 1;
                    } else {
                        let width = char_width(source, at);
                        let text = std::str::from_utf8(&source[at..at + width]).ok()?;
                        if text.chars().next()?.is_whitespace() {
                            break;
                        }
                        at += width;
                    }
                }
                return Some(at);
            }
            b'\n' | b'\r' => return None,
            _ if starts_with(source, at, "\u{2028}".as_bytes())
                || starts_with(source, at, "\u{2029}".as_bytes()) =>
            {
                return None;
            }
            _ => at += char_width(source, at),
        }
    }
    None
}

fn has_type_annotation(source: &[u8], start: usize, end: usize) -> bool {
    let mut at = start;
    let mut prev = b'(';
    while at < end {
        match source[at] {
            b'/' if source.get(at + 1) == Some(&b'/') => {
                at = js_line_end(source, at + 2).min(end);
            }
            b'/' if source.get(at + 1) == Some(&b'*') => {
                let mut cursor = at + 2;
                while cursor + 1 < end && !(source[cursor] == b'*' && source[cursor + 1] == b'/') {
                    cursor += 1;
                }
                at = (cursor + 2).min(end);
            }
            b'/' if regex_allowed(prev) => {
                if let Some(next) = regex_end(source, at) {
                    at = next.min(end);
                    prev = b'/';
                } else {
                    prev = b'/';
                    at += 1;
                }
            }
            b'\'' | b'"' => {
                at = skip_quoted(source, at, source[at]).unwrap_or(end).min(end);
                prev = b'"';
            }
            b'`' => {
                at = skip_template(source, at).unwrap_or(end).min(end);
                prev = b'`';
            }
            b':' => return true,
            b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c => at += 1,
            _ => {
                prev = source[at];
                at += char_width(source, at);
            }
        }
    }
    false
}

fn has_type_parameter_marker(source: &[u8], start: usize, end: usize) -> bool {
    let mut at = skip_trivia(source, start);
    if at >= end {
        return false;
    }
    if let Some(word) = word_at(source, at, end)
        && matches!(word, b"const" | b"in" | b"out")
    {
        at = skip_trivia(source, at + word.len());
        if at >= end {
            return false;
        }
    }
    let Some(name) = word_at(source, at, end) else {
        return false;
    };
    at = skip_trivia(source, at + name.len());
    match source.get(at).copied() {
        Some(b',' | b'=') => true,
        Some(_) if word_at(source, at, end) == Some(b"extends") => {
            let after = skip_trivia(source, at + b"extends".len());
            after < end && !matches!(source[after], b'=' | b'>' | b'/')
        }
        _ => false,
    }
}

fn word_at(source: &[u8], at: usize, end: usize) -> Option<&[u8]> {
    if at >= end || !ident_start(source[at]) {
        return None;
    }
    let begin = at;
    let mut cursor = at + char_width(source, at);
    while cursor < end && ident_continue(source[cursor]) {
        cursor += char_width(source, cursor);
    }
    Some(&source[begin..cursor])
}

fn angle_end(source: &[u8], start: usize) -> Option<usize> {
    let mut at = start + 1;
    let mut angle = 1usize;
    while at < source.len() {
        match source[at] {
            b'/' if source.get(at + 1) == Some(&b'/') => {
                at = js_line_end(source, at + 2);
            }
            b'/' if source.get(at + 1) == Some(&b'*') => {
                let mut cursor = at + 2;
                while cursor + 1 < source.len()
                    && !(source[cursor] == b'*' && source[cursor + 1] == b'/')
                {
                    cursor += 1;
                }
                if cursor + 1 >= source.len() {
                    return None;
                }
                at = cursor + 2;
            }
            b'\'' | b'"' => {
                at = skip_quoted(source, at, source[at])?;
            }
            b'`' => {
                at = skip_template(source, at)?;
            }
            b'=' if source.get(at + 1) == Some(&b'>') => at += 2,
            b'<' => {
                angle += 1;
                at += 1;
            }
            b'>' => {
                angle -= 1;
                at += 1;
                if angle == 0 {
                    return Some(at);
                }
            }
            b'\n' | b'\r' => at += 1,
            _ => at += char_width(source, at),
        }
    }
    None
}

fn skip_quoted(source: &[u8], start: usize, quote: u8) -> Option<usize> {
    let mut at = start + 1;
    while at < source.len() {
        match source[at] {
            b'\\' => {
                at += 1;
                if at < source.len() {
                    at += char_width(source, at);
                }
            }
            byte if byte == quote => return Some(at + 1),
            b'\n' | b'\r' => return None,
            _ => at += char_width(source, at),
        }
    }
    None
}

fn skip_template(source: &[u8], start: usize) -> Option<usize> {
    let mut at = start + 1;
    while at < source.len() {
        match source[at] {
            b'\\' => {
                at += 1;
                if at < source.len() {
                    at += char_width(source, at);
                }
            }
            b'`' => return Some(at + 1),
            _ => at += char_width(source, at),
        }
    }
    None
}
