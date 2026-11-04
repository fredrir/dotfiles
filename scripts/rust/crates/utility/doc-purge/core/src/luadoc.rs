use crate::edit::{Cut, Kind};
use crate::scan::{Comment, identifier_byte};

struct Cursor<'a> {
    source: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn next(&self) -> usize {
        let mut at = self.at;
        while self.source.get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        at
    }

    fn peek(&self) -> Option<u8> {
        self.source.get(self.next()).copied()
    }

    fn take(&mut self, token: &[u8]) -> bool {
        let at = self.next();
        if self.source[at..].starts_with(token) {
            self.at = at + token.len();
            true
        } else {
            false
        }
    }

    fn name(&mut self) -> Option<&'a [u8]> {
        let start = self.next();
        if self.take(b"...") {
            return Some(&self.source[start..self.at]);
        }
        if !self.source.get(start).is_some_and(|b| identifier_byte(*b)) {
            return None;
        }
        self.at = start + 1;
        while self
            .source
            .get(self.at)
            .is_some_and(|b| identifier_byte(*b) || matches!(b, b'.' | b'*' | b'-'))
        {
            self.at += 1;
        }
        Some(&self.source[start..self.at])
    }

    fn quoted(&mut self) -> Option<()> {
        self.at = self.next();
        let quote = *self.source.get(self.at)?;
        self.at += 1;
        while let Some(&byte) = self.source.get(self.at) {
            self.at += 1;
            if byte == quote {
                return Some(());
            }
            if byte == b'\\' && quote != b'`' {
                self.source.get(self.at)?;
                self.at += 1;
            }
        }
        None
    }

    fn long_literal(&mut self) -> Option<bool> {
        let start = self.next();
        let mut open = start + 1;
        while self.source.get(open) == Some(&b'=') {
            open += 1;
        }
        if self.source.get(open) != Some(&b'[') {
            return Some(false);
        }
        let width = open - start + 1;
        let close = self.source[open + 1..].windows(width).position(|part| {
            part[0] == b']'
                && part[width - 1] == b']'
                && part[1..width - 1].iter().all(|b| *b == b'=')
        })?;
        self.at = open + 1 + close + width;
        Some(true)
    }

    fn group(&mut self) -> Option<()> {
        self.at = self.next();
        let mut closings = Vec::new();
        while let Some(&byte) = self.source.get(self.at) {
            match byte {
                b'\'' | b'"' | b'`' => {
                    self.quoted()?;
                    continue;
                }
                b'(' => closings.push(b')'),
                b'[' => {
                    if self.long_literal()? {
                        continue;
                    }
                    closings.push(b']');
                }
                b'{' => closings.push(b'}'),
                b'<' => closings.push(b'>'),
                b')' | b']' | b'}' | b'>' if closings.pop() != Some(byte) => {
                    return None;
                }
                _ => {}
            }
            self.at += 1;
            if closings.is_empty() {
                return Some(());
            }
        }
        None
    }

    fn ty(&mut self, depth: usize) -> Option<()> {
        if depth > 64 {
            return None;
        }
        loop {
            match self.peek()? {
                b'\'' | b'"' | b'`' => self.quoted()?,
                b'(' | b'{' => self.group()?,
                b'[' => {
                    if !self.long_literal()? {
                        self.group()?;
                    }
                }
                b'-' => {
                    self.take(b"-");
                    let start = self.at;
                    while self.source.get(self.at).is_some_and(u8::is_ascii_digit) {
                        self.at += 1;
                    }
                    if self.at == start {
                        return None;
                    }
                }
                _ => {
                    let mut name = self.name()?;
                    if name == b"async" {
                        name = self.name()?;
                        if name != b"fun" {
                            return None;
                        }
                    }
                    if name == b"fun" {
                        if self.peek() == Some(b'<') {
                            self.group()?;
                        }
                        if self.peek() != Some(b'(') {
                            return None;
                        }
                        self.group()?;
                        if self.take(b":") {
                            self.function_returns(depth + 1)?;
                        }
                    }
                }
            }
            while self.peek() == Some(b'<') {
                self.group()?;
            }
            while self.take(b"[]") {}
            self.take(b"?");
            if !self.take(b"|") && !self.take(b"&") {
                return Some(());
            }
        }
    }

    fn function_returns(&mut self, depth: usize) -> Option<()> {
        if self.peek() == Some(b'(') {
            return self.group();
        }
        loop {
            let start = self.at;
            if self.name().is_none() || !self.take(b":") {
                self.at = start;
            }
            self.ty(depth)?;
            if !self.take(b",") {
                return Some(());
            }
        }
    }

    fn types(&mut self, named: bool) -> Option<()> {
        loop {
            self.ty(0)?;
            if named {
                self.name();
            }
            if !self.take(b",") {
                return Some(());
            }
        }
    }

    fn annotation(&mut self) -> Option<()> {
        match self.name()? {
            b"param" => {
                self.name()?;
                self.take(b"?");
                self.ty(0)
            }
            b"field" => {
                let start = self.at;
                if !matches!(
                    self.name(),
                    Some(b"private" | b"protected" | b"public" | b"package")
                ) {
                    self.at = start;
                }
                if self.peek() == Some(b'[') {
                    self.group()?;
                } else {
                    self.name()?;
                }
                self.take(b"?");
                self.ty(0)
            }
            b"return" => self.types(true),
            b"type" => self.types(false),
            b"vararg" | b"overload" => self.ty(0),
            kind @ (b"alias" | b"class") => {
                if self.peek() == Some(b'(') {
                    self.group()?;
                }
                self.name()?;
                if self.peek() == Some(b'<') {
                    self.group()?;
                }
                if kind == b"alias" {
                    self.ty(0)
                } else if self.take(b":") {
                    self.types(false)
                } else {
                    Some(())
                }
            }
            _ => None,
        }
    }
}

pub fn description(source: &[u8], comment: &Comment) -> Option<Cut> {
    if !source[comment.span.clone()].starts_with(b"---@") {
        return None;
    }
    let body = &source[comment.body.clone()];
    let body = body.strip_suffix(b"\r").unwrap_or(body);
    let annotation = body.strip_prefix(b"-@")?;
    let mut cursor = Cursor {
        source: annotation,
        at: 0,
    };
    cursor.annotation()?;
    let tail = annotation.get(cursor.at..)?;
    if tail.iter().all(u8::is_ascii_whitespace)
        || !tail
            .first()
            .is_some_and(|b| b.is_ascii_whitespace() || *b == b'#')
    {
        return None;
    }
    Some(Cut {
        span: comment.body.start + 2 + cursor.at..comment.body.start + body.len(),
        with: None,
        kind: Kind::Comment,
        lines: false,
    })
}
