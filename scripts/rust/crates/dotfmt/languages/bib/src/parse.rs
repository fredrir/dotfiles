//! A lossless BibTeX/BibLaTeX parser.
//!
//! The parser records byte spans rather than decoded values. The printer only
//! writes separators, indentation, and case-normalised names around those
//! spans, so a value can never be silently rewritten.

use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn slice(self, text: &str) -> &str {
        &text[self.start..self.end]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartKind {
    Braced,
    Quoted,
    Bare,
}

#[derive(Clone, Copy, Debug)]
pub struct Part {
    pub kind: PartKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Value {
    pub span: Span,
    pub parts: Vec<Part>,
}

#[derive(Clone, Copy, Debug)]
pub struct Comment {
    pub span: Span,
}

impl Comment {
    pub fn text(self, text: &str) -> &str {
        self.span.slice(text).trim_end()
    }
}

#[derive(Debug)]
pub struct Field {
    pub name: Span,
    pub value: Value,
    pub leading: Vec<Comment>,
    pub trailing: Option<Comment>,
}

#[derive(Debug)]
pub struct Entry {
    pub open: u8,
    pub close: u8,
    pub kind: Span,
    pub key: Span,
    pub fields: Vec<Field>,
    pub trailing: Vec<Comment>,
}

#[derive(Debug)]
pub struct Assign {
    pub open: u8,
    pub close: u8,
    pub name: Span,
    pub value: Value,
    pub leading: Vec<Comment>,
    pub trailing: Vec<Comment>,
}

#[derive(Debug)]
pub struct Preamble {
    pub open: u8,
    pub close: u8,
    pub inner: Span,
}

#[derive(Debug)]
pub struct CommentBlock {
    pub span: Span,
}

#[derive(Debug)]
pub enum Block {
    Entry(Entry),
    Assign(Assign),
    Preamble(Preamble),
    Comment(CommentBlock),
}

#[derive(Debug)]
pub struct Trivia {
    pub span: Span,
}

#[derive(Debug)]
pub struct Item {
    pub leading: Trivia,
    pub block: Block,
}

#[derive(Debug)]
pub struct Document {
    pub items: Vec<Item>,
    pub trailer: Trivia,
}

pub fn parse(text: &str) -> Result<Document, Diagnostic> {
    Parser { text, pos: 0 }.document()
}

struct Parser<'a> {
    text: &'a str,
    pos: usize,
}

impl Parser<'_> {
    fn document(mut self) -> Result<Document, Diagnostic> {
        let mut items = Vec::new();
        let mut leading_start = 0;
        while let Some(start) = self.next_block()? {
            let leading = Trivia {
                span: Span {
                    start: leading_start,
                    end: start,
                },
            };
            self.pos = start;
            let block = self.block()?;
            items.push(Item { leading, block });
            leading_start = self.pos;
        }
        Ok(Document {
            items,
            trailer: Trivia {
                span: Span {
                    start: leading_start,
                    end: self.text.len(),
                },
            },
        })
    }

    fn next_block(&mut self) -> Result<Option<usize>, Diagnostic> {
        let bytes = self.text.as_bytes();
        while self.pos < bytes.len() {
            match bytes[self.pos] {
                b'%' => self.pos = line_end(bytes, self.pos),
                b'@' if self.looks_like_block(self.pos) => return Ok(Some(self.pos)),
                b'@' => {
                    // BibTeX reports both forms, so a stray `@` outside a block is
                    // not prose that can be silently skipped.
                    let message = if self.block_ident(self.pos).is_some() {
                        "expected '{' or '(' after the block type"
                    } else {
                        "expected a block type after '@'"
                    };
                    return Err(self.error(self.pos, message));
                }
                _ => self.pos += 1,
            }
        }
        Ok(None)
    }

    /// The block type after the `@` at `at`, with the offset just past it.
    fn block_ident(&self, at: usize) -> Option<(usize, &str)> {
        let bytes = self.text.as_bytes();
        let mut i = at + 1;
        while bytes.get(i).is_some_and(|byte| byte.is_ascii_whitespace()) {
            i += 1;
        }
        let start = i;
        while bytes.get(i).is_some_and(|byte| is_ident_byte(*byte)) {
            i += 1;
        }
        (start < i).then(|| (i, &self.text[start..i]))
    }

    fn looks_like_block(&self, at: usize) -> bool {
        let Some((mut i, kind)) = self.block_ident(at) else {
            return false;
        };
        let bytes = self.text.as_bytes();
        while bytes.get(i).is_some_and(|byte| byte.is_ascii_whitespace()) {
            i += 1;
        }
        bytes
            .get(i)
            .is_some_and(|byte| *byte == b'{' || *byte == b'(')
            || kind.eq_ignore_ascii_case("comment")
    }

    fn block(&mut self) -> Result<Block, Diagnostic> {
        let start = self.pos;
        self.pos += 1;
        self.skip_whitespace();
        let kind_start = self.pos;
        let kind = self.ident();
        if kind.is_empty() {
            return Err(self.error(start, "expected a bibliography block"));
        }
        let kind = kind.to_ascii_lowercase();
        self.skip_whitespace();
        let open = self.peek().filter(|byte| *byte == b'{' || *byte == b'(');
        if kind == "comment" {
            let Some(open) = open else {
                // A brace-less @comment runs to the next `@`. The whitespace that
                // separates it from the next block (or ends the file) is trivia, not
                // comment text, so the printed separator cannot grow on every pass.
                let end = self.text[self.pos..]
                    .find('@')
                    .map_or(self.text.len(), |offset| self.pos + offset);
                let end = trim_end(self.text, start, end);
                self.pos = end;
                return Ok(Block::Comment(CommentBlock {
                    span: Span { start, end },
                }));
            };
            self.pos += 1;
            let end = if open == b'{' {
                self.balanced(self.pos, b'{', b'}')
                    .ok_or_else(|| self.error(start, "unterminated @comment"))?
            } else {
                let close_end = self
                    .text
                    .get(self.pos..)
                    .and_then(|rest| rest.find(')'))
                    .map(|offset| self.pos + offset + 1)
                    .ok_or_else(|| self.error(start, "unterminated @comment"))?;
                let line = line_end(self.text.as_bytes(), close_end);
                if self.text[close_end..line].contains('@') {
                    close_end
                } else {
                    line
                }
            };
            self.pos = end;
            return Ok(Block::Comment(CommentBlock {
                span: Span { start, end },
            }));
        }
        let Some(open) = open else {
            return Err(self.error(start, "expected '{' or '(' after the block type"));
        };
        self.pos += 1;
        let close = matching(open);
        match kind.as_str() {
            "preamble" => self.preamble(start, open, close),
            "string" => self.assign(start, open, close),
            _ => self.entry(start, kind_start, open, close),
        }
    }

    fn preamble(&mut self, start: usize, open: u8, close: u8) -> Result<Block, Diagnostic> {
        let inner_start = self.pos;
        let mut comments = Vec::new();
        self.skip_trivia(&mut comments);
        if self.peek() == Some(close) {
            self.reject_preamble_comments(&comments)?;
            let inner_end = self.pos;
            self.pos += 1;
            return Ok(Block::Preamble(Preamble {
                open,
                close,
                inner: Span {
                    start: inner_start,
                    end: inner_end,
                },
            }));
        }
        let value = self.value(&mut comments)?;
        let _ = value;
        let before_end = comments.len();
        self.skip_trivia(&mut comments);
        self.reject_preamble_comments(&comments[before_end..])?;
        if self.peek() != Some(close) {
            return Err(self.error(self.pos, "expected the end of @preamble"));
        }
        let inner_end = self.pos;
        self.pos += 1;
        let _ = start;
        Ok(Block::Preamble(Preamble {
            open,
            close,
            inner: Span {
                start: inner_start,
                end: inner_end,
            },
        }))
    }

    fn assign(&mut self, start: usize, open: u8, close: u8) -> Result<Block, Diagnostic> {
        let mut leading = Vec::new();
        self.skip_trivia(&mut leading);
        let name_start = self.pos;
        self.name()?;
        let name_span = Span {
            start: name_start,
            end: self.pos,
        };
        self.skip_trivia(&mut leading);
        if self.peek() != Some(b'=') {
            return Err(self.error(self.pos, "expected '=' in @string"));
        }
        self.pos += 1;
        let mut value_comments = Vec::new();
        let value = self.value(&mut value_comments)?;
        leading.append(&mut value_comments);
        let mut trailing = Vec::new();
        self.after_value(&mut trailing)?;
        if self.peek() == Some(b',') {
            self.pos += 1;
        }
        self.skip_trivia(&mut trailing);
        if self.peek() != Some(close) {
            return Err(self.error(self.pos, "expected the end of @string"));
        }
        self.pos += 1;
        let _ = start;
        Ok(Block::Assign(Assign {
            open,
            close,
            name: name_span,
            value,
            leading,
            trailing,
        }))
    }

    fn entry(
        &mut self,
        start: usize,
        kind_start: usize,
        open: u8,
        close: u8,
    ) -> Result<Block, Diagnostic> {
        let key_region = self.pos;
        self.scan_key(open, close);
        let start_of_key = trim_start(self.text, key_region, self.pos);
        let key = Span {
            start: start_of_key,
            end: trim_end(self.text, start_of_key, self.pos),
        };
        let key_text = key.slice(self.text);
        if key_text.chars().any(char::is_whitespace) || (open == b'{' && key_text.contains('}')) {
            return Err(self.error(key_region, "invalid entry key"));
        }
        if self.peek() == Some(close) {
            self.pos += 1;
            return Ok(Block::Entry(Entry {
                open,
                close,
                kind: Span {
                    start: kind_start,
                    end: kind_start + self.ident_len(kind_start),
                },
                key,
                fields: Vec::new(),
                trailing: Vec::new(),
            }));
        }
        if self.peek() != Some(b',') {
            let message = if self.peek().is_none() {
                "unterminated entry"
            } else {
                "expected ',' after the entry key"
            };
            return Err(self.error(self.pos, message));
        }
        self.pos += 1;
        let mut fields = Vec::new();
        let mut leading = Vec::new();
        let trailing;
        loop {
            self.skip_trivia(&mut leading);
            if self.peek() == Some(close) {
                trailing = leading;
                self.pos += 1;
                break;
            }
            let field_start = self.pos;
            self.name()?;
            let name_span = Span {
                start: field_start,
                end: self.pos,
            };
            self.skip_trivia(&mut leading);
            if self.peek() != Some(b'=') {
                return Err(self.error(self.pos, "expected '=' after a field name"));
            }
            self.pos += 1;
            let mut value_comments = Vec::new();
            let value = self.value(&mut value_comments)?;
            leading.append(&mut value_comments);
            let mut field_trailing = None;
            self.skip_horizontal();
            if self.peek() == Some(b'%') {
                let comment = self.comment();
                field_trailing = Some(comment);
            }
            let mut between = Vec::new();
            self.skip_trivia(&mut between);
            let mut comma = false;
            if self.peek() == Some(b',') {
                comma = true;
                self.pos += 1;
                self.skip_horizontal();
                if field_trailing.is_none() && self.peek() == Some(b'%') {
                    field_trailing = Some(self.comment());
                }
                self.skip_trivia(&mut between);
            }
            fields.push(Field {
                name: name_span,
                value,
                leading: std::mem::take(&mut leading),
                trailing: field_trailing,
            });
            if self.peek() == Some(close) {
                trailing = between;
                self.pos += 1;
                break;
            }
            if !comma {
                return Err(self.error(self.pos, "expected ',' or the end of the entry"));
            }
            leading = between;
        }
        let _ = start;
        Ok(Block::Entry(Entry {
            open,
            close,
            kind: Span {
                start: kind_start,
                end: kind_start + self.ident_len(kind_start),
            },
            key,
            fields,
            trailing,
        }))
    }

    /// Scan the entry key body; the caller trims the span.
    ///
    /// Braced entries stop at `,` or `}`, as BibTeX does. A parenthesised key may
    /// contain `)`, so it ends at the first comma and only a comma-less entry ends at
    /// its final `)`.
    fn scan_key(&mut self, open: u8, close: u8) {
        if open == b'{' {
            while self
                .peek()
                .is_some_and(|byte| byte != b',' && byte != close)
            {
                self.pos += 1;
            }
            return;
        }
        let bytes = self.text.as_bytes();
        let mut comma = None;
        let mut last_close = None;
        while self.pos < bytes.len() {
            match bytes[self.pos] {
                b',' => {
                    comma = Some(self.pos);
                    break;
                }
                b')' => {
                    last_close = Some(self.pos);
                    self.pos += 1;
                }
                b'@' if self.looks_like_block(self.pos) => break,
                _ => self.pos += 1,
            }
        }
        if let Some(comma) = comma {
            self.pos = comma;
        } else if let Some(last_close) = last_close {
            self.pos = last_close;
        }
    }

    fn value(&mut self, comments: &mut Vec<Comment>) -> Result<Value, Diagnostic> {
        let mut parts = Vec::new();
        self.skip_trivia(comments);
        let start = self.pos;
        loop {
            let part_start = self.pos;
            let kind = match self.peek() {
                Some(b'{') => {
                    self.pos = self
                        .balanced(self.pos + 1, b'{', b'}')
                        .ok_or_else(|| self.error(part_start, "unbalanced braces in a value"))?;
                    PartKind::Braced
                }
                Some(b'"') => {
                    self.pos = self
                        .quoted(self.pos + 1)
                        .ok_or_else(|| self.error(part_start, "unterminated quoted value"))?;
                    PartKind::Quoted
                }
                Some(byte) if is_value_byte(byte) => {
                    self.bare();
                    let token = &self.text[part_start..self.pos];
                    let invalid = token.bytes().any(|byte| {
                        matches!(byte, b'{' | b'}' | b'"' | b'\'' | b'(' | b')' | b'=')
                    });
                    if invalid
                        || (token.starts_with(|c: char| c.is_ascii_digit())
                            && !token.bytes().all(|byte| byte.is_ascii_digit()))
                    {
                        return Err(self.error(part_start, "invalid bare value"));
                    }
                    PartKind::Bare
                }
                _ => {
                    return Err(self.error(self.pos, "expected a field value"));
                }
            };
            parts.push(Part {
                kind,
                span: Span {
                    start: part_start,
                    end: self.pos,
                },
            });
            let after_part = self.pos;
            let mut between = Vec::new();
            self.skip_trivia(&mut between);
            if self.peek() != Some(b'#') {
                self.pos = after_part;
                break;
            }
            // BibTeX rejects a comment between concatenated parts; printing it once as
            // a leading comment would move it, and keeping it in the value span would
            // emit it twice.
            self.reject_concat_comments(&between)?;
            self.pos += 1;
            let mut after_hash = Vec::new();
            self.skip_trivia(&mut after_hash);
            self.reject_concat_comments(&after_hash)?;
        }
        let end = parts.last().map_or(start, |part| part.span.end);
        Ok(Value {
            span: Span { start, end },
            parts,
        })
    }

    fn reject_concat_comments(&self, comments: &[Comment]) -> Result<(), Diagnostic> {
        if let Some(comment) = comments.first() {
            return Err(self.error(
                comment.span.start,
                "comments are not allowed between concatenated value parts",
            ));
        }
        Ok(())
    }

    /// A comment between an `@preamble` value and its closing delimiter would swallow
    /// that delimiter once printed without the newline that ended the comment.
    fn reject_preamble_comments(&self, comments: &[Comment]) -> Result<(), Diagnostic> {
        if let Some(comment) = comments.first() {
            return Err(self.error(
                comment.span.start,
                "comments are not allowed before the end of @preamble",
            ));
        }
        Ok(())
    }

    fn after_value(&mut self, trailing: &mut Vec<Comment>) -> Result<(), Diagnostic> {
        self.skip_horizontal();
        if self.peek() == Some(b'%') {
            trailing.push(self.comment());
        }
        self.skip_trivia(trailing);
        Ok(())
    }

    fn name(&mut self) -> Result<&str, Diagnostic> {
        let start = self.pos;
        while self.peek().is_some_and(is_ident_byte) {
            self.pos += 1;
        }
        if start == self.pos {
            return Err(self.error(start, "expected a field name"));
        }
        Ok(&self.text[start..self.pos])
    }

    fn ident(&mut self) -> &str {
        let start = self.pos;
        while self.peek().is_some_and(is_ident_byte) {
            self.pos += 1;
        }
        &self.text[start..self.pos]
    }

    fn ident_len(&self, start: usize) -> usize {
        self.text[start..]
            .bytes()
            .take_while(|byte| is_ident_byte(*byte))
            .count()
    }

    fn bare(&mut self) {
        while self.peek().is_some_and(|byte| {
            !byte.is_ascii_whitespace() && !matches!(byte, b'#' | b',' | b'}' | b')' | b'%')
        }) {
            self.pos += 1;
        }
    }

    fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }

    fn skip_horizontal(&mut self) {
        while self
            .peek()
            .is_some_and(|byte| byte == b' ' || byte == b'\t')
        {
            self.pos += 1;
        }
    }

    fn skip_trivia(&mut self, comments: &mut Vec<Comment>) {
        loop {
            self.skip_whitespace();
            if self.peek() != Some(b'%') {
                break;
            }
            comments.push(self.comment());
        }
    }

    fn comment(&mut self) -> Comment {
        let start = self.pos;
        self.pos = line_end(self.text.as_bytes(), self.pos);
        Comment {
            span: Span {
                start,
                end: trim_end(self.text, start, self.pos),
            },
        }
    }

    fn balanced(&self, from: usize, open: u8, close: u8) -> Option<usize> {
        let bytes = self.text.as_bytes();
        let mut depth = 1usize;
        let mut i = from;
        while i < bytes.len() {
            if bytes[i] == open {
                depth += 1;
            } else if bytes[i] == close {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            i += 1;
        }
        None
    }

    fn quoted(&self, from: usize) -> Option<usize> {
        let bytes = self.text.as_bytes();
        let mut depth = 0usize;
        let mut i = from;
        while i < bytes.len() {
            match bytes[i] {
                b'{' => depth += 1,
                b'}' => depth = depth.saturating_sub(1),
                b'"' if depth == 0 => return Some(i + 1),
                _ => {}
            }
            i += 1;
        }
        None
    }

    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.pos).copied()
    }

    fn error(&self, offset: usize, message: &str) -> Diagnostic {
        let before = self.text.get(..offset).unwrap_or(self.text);
        let line = before.matches('\n').count() + 1;
        let column = before
            .rsplit('\n')
            .next()
            .map_or(1, |line| line.chars().count() + 1);
        Diagnostic::new(DiagnosticKind::Syntax, message).with_location(line, Some(column))
    }
}

fn matching(open: u8) -> u8 {
    if open == b'{' { b'}' } else { b')' }
}

fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b':' | b'.' | b'+' | b'/')
}

fn is_value_byte(byte: u8) -> bool {
    !byte.is_ascii_whitespace() && !matches!(byte, b'#' | b',' | b'}' | b')')
}

fn line_end(bytes: &[u8], from: usize) -> usize {
    bytes[from..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(bytes.len(), |offset| from + offset)
}

fn trim_start(text: &str, start: usize, end: usize) -> usize {
    let bytes = text.as_bytes();
    let mut start = start;
    while start < end && bytes[start].is_ascii_whitespace() {
        start += 1;
    }
    start
}

fn trim_end(text: &str, start: usize, end: usize) -> usize {
    let bytes = text.as_bytes();
    let mut end = end;
    while end > start && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    end
}
