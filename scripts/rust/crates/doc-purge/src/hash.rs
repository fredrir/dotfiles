use std::ops::Range;

use crate::lang::Dialect;
use crate::scan::{Bail, Logical, Out, char_width, identifier_byte, line_end, starts_with};

pub fn scan(source: &[u8], dialect: Dialect) -> Result<Out, Bail> {
    match dialect {
        Dialect::Python => python(source),
        Dialect::Toml => toml(source),
        Dialect::Yaml => yaml(source),
        Dialect::Shell => Shell::new(source).run(),
        Dialect::Ruby => Ruby::new(source).run(),
        _ => Ok(Out::default()),
    }
}

fn indent_of(source: &[u8], at: usize) -> (usize, usize) {
    let mut begin = at;
    while begin > 0 && source[begin - 1] != b'\n' {
        begin -= 1;
    }
    (begin, at - begin)
}

struct Pending {
    start: usize,
    indent: usize,
    text_lo: usize,
    text_hi: usize,
    texts: usize,
    other: bool,
}

impl Pending {
    fn begin(source: &[u8], at: usize) -> Pending {
        let (_, indent) = indent_of(source, at);
        Pending {
            start: at,
            indent,
            text_lo: at,
            text_hi: at,
            texts: 0,
            other: false,
        }
    }

    fn close(self, end: usize) -> Logical {
        let sole_text = (self.texts > 0 && !self.other).then_some(self.text_lo..self.text_hi);
        Logical {
            start: self.start,
            end,
            indent: self.indent,
            sole_text,
        }
    }
}

fn string_prefix(word: &[u8]) -> bool {
    !word.is_empty()
        && word.len() <= 2
        && word
            .iter()
            .all(|byte| matches!(byte, b'r' | b'R' | b'b' | b'B' | b'u' | b'U' | b'f' | b'F'))
}

fn python(source: &[u8]) -> Result<Out, Bail> {
    let mut out = Out::default();
    let mut at = 0usize;
    let mut depth = 0usize;
    let mut line: Option<Pending> = None;
    while at < source.len() {
        let byte = source[at];
        if byte == b'\n' {
            at += 1;
            if depth == 0
                && let Some(pending) = line.take()
            {
                out.logical.push(pending.close(at));
            }
            continue;
        }
        if byte.is_ascii_whitespace() {
            at += 1;
            continue;
        }
        if byte == b'#' {
            let end = line_end(source, at);
            out.comment(at..end, at + 1..end);
            at = end;
            continue;
        }
        if byte == b'\\' {
            at += 2;
            continue;
        }
        let pending = line.get_or_insert_with(|| Pending::begin(source, at));
        match byte {
            b'(' | b'[' | b'{' => {
                depth += 1;
                at += 1;
                pending.other = true;
            }
            b')' | b']' | b'}' => {
                depth = depth.saturating_sub(1);
                at += 1;
                pending.other = true;
            }
            b'\'' | b'"' => {
                let end = python_string(source, at, at, &mut out)?;
                if pending.texts == 0 {
                    pending.text_lo = at;
                }
                pending.texts += 1;
                pending.text_hi = end;
                at = end;
            }
            _ if byte.is_ascii_alphabetic() || byte == b'_' => {
                let start = at;
                while at < source.len() && identifier_byte(source[at]) {
                    at += char_width(source, at);
                }
                let word = &source[start..at];
                if string_prefix(word) && matches!(source.get(at), Some(b'\'') | Some(b'"')) {
                    let end = python_string(source, at, start, &mut out)?;
                    if pending.texts == 0 {
                        pending.text_lo = start;
                    }
                    pending.texts += 1;
                    pending.text_hi = end;
                    at = end;
                } else {
                    pending.other = true;
                }
            }
            _ => {
                at += char_width(source, at);
                pending.other = true;
            }
        }
    }
    if let Some(pending) = line.take() {
        out.logical.push(pending.close(source.len()));
    }
    Ok(out)
}

fn python_string(
    source: &[u8],
    quote_at: usize,
    token_at: usize,
    out: &mut Out,
) -> Result<usize, Bail> {
    let prefix = &source[token_at..quote_at];
    let raw = prefix.iter().any(|byte| matches!(byte, b'r' | b'R'));
    let quote = source[quote_at];
    let triple = [quote; 3];
    if starts_with(source, quote_at, &triple) {
        let start = quote_at + 3;
        let mut at = start;
        while at < source.len() {
            if source[at] == b'\\' {
                at += 1 + char_width(source, (at + 1).min(source.len().saturating_sub(1)));
                continue;
            }
            if starts_with(source, at, &triple) {
                out.text(start..at, !raw, true, 0);
                return Ok(at + 3);
            }
            at += char_width(source, at);
        }
        return Err(Bail("unterminated string"));
    }
    let start = quote_at + 1;
    let mut at = start;
    while at < source.len() {
        let byte = source[at];
        if byte == b'\\' {
            at += 1 + char_width(source, (at + 1).min(source.len().saturating_sub(1)));
            continue;
        }
        if byte == quote {
            out.text(start..at, !raw, true, quote);
            return Ok(at + 1);
        }
        if byte == b'\n' {
            return Err(Bail("unterminated string"));
        }
        at += char_width(source, at);
    }
    Err(Bail("unterminated string"))
}

fn toml(source: &[u8]) -> Result<Out, Bail> {
    let mut out = Out::default();
    let mut at = 0usize;
    while at < source.len() {
        let byte = source[at];
        match byte {
            b'#' => {
                let end = line_end(source, at);
                out.comment(at..end, at + 1..end);
                at = end;
            }
            b'"' | b'\'' => {
                let triple = [byte; 3];
                let escapes = byte == b'"';
                if starts_with(source, at, &triple) {
                    let start = at + 3;
                    let mut cursor = start;
                    loop {
                        if cursor >= source.len() {
                            return Err(Bail("unterminated string"));
                        }
                        if escapes && source[cursor] == b'\\' {
                            cursor += 2;
                            continue;
                        }
                        if starts_with(source, cursor, &triple) {
                            out.text(start..cursor, escapes, true, 0);
                            at = cursor + 3;
                            break;
                        }
                        cursor += char_width(source, cursor);
                    }
                } else {
                    let start = at + 1;
                    let mut cursor = start;
                    loop {
                        if cursor >= source.len() || source[cursor] == b'\n' {
                            return Err(Bail("unterminated string"));
                        }
                        if escapes && source[cursor] == b'\\' {
                            cursor += 2;
                            continue;
                        }
                        if source[cursor] == byte {
                            out.text(start..cursor, escapes, true, byte);
                            at = cursor + 1;
                            break;
                        }
                        cursor += char_width(source, cursor);
                    }
                }
            }
            _ => at += char_width(source, at),
        }
    }
    Ok(out)
}

fn yaml(source: &[u8]) -> Result<Out, Bail> {
    let mut out = Out::default();
    let mut at = 0usize;
    let mut block: Option<(usize, usize)> = None;
    while at < source.len() {
        let begin = at;
        let end = line_end(source, at);
        let content = &source[begin..end];
        let indent = content.iter().take_while(|byte| **byte == b' ').count();
        let blank = content.iter().all(|byte| byte.is_ascii_whitespace());
        if let Some((owner, start)) = block {
            if blank || indent > owner {
                at = (end + 1).min(source.len());
                continue;
            }
            out.text(start..begin, false, false, 0);
            block = None;
        }
        if !blank {
            let mut cursor = begin + indent;
            let mut scalar: Option<usize> = None;
            while cursor < end {
                let byte = source[cursor];
                if byte == b'#' && (cursor == begin + indent || source[cursor - 1] == b' ') {
                    out.comment(cursor..end, cursor + 1..end);
                    break;
                }
                match byte {
                    b'\'' | b'"' => {
                        cursor = yaml_quoted(source, cursor, &mut out)?;
                        scalar = None;
                        continue;
                    }
                    b':' if cursor + 1 >= end || source[cursor + 1] == b' ' => {
                        scalar = Some(cursor + 1);
                    }
                    b'-' if cursor == begin + indent
                        && (cursor + 1 >= end || source[cursor + 1] == b' ') =>
                    {
                        scalar = Some(cursor + 1);
                    }
                    _ => {}
                }
                cursor += char_width(source, cursor);
            }
            if let Some(from) = scalar {
                let value = yaml_value(source, from, end);
                if let Some(value) = value {
                    let head = source[value.start];
                    if head == b'|' || head == b'>' {
                        block = Some((indent, (end + 1).min(source.len())));
                    } else if !matches!(head, b'&' | b'*' | b'!' | b'{' | b'[') {
                        out.text(value, false, false, 0);
                    }
                }
            }
        }
        at = (end + 1).min(source.len());
        if end >= source.len() {
            break;
        }
    }
    if let Some((_, start)) = block {
        out.text(start..source.len(), false, false, 0);
    }
    Ok(out)
}

fn yaml_value(source: &[u8], from: usize, end: usize) -> Option<Range<usize>> {
    let mut start = from;
    while start < end && (source[start] == b' ' || source[start] == b'\t') {
        start += 1;
    }
    let mut stop = end;
    let mut cursor = start;
    while cursor < end {
        if source[cursor] == b'#' && cursor > start && source[cursor - 1] == b' ' {
            stop = cursor - 1;
            break;
        }
        cursor += 1;
    }
    while stop > start && source[stop - 1].is_ascii_whitespace() {
        stop -= 1;
    }
    (stop > start).then_some(start..stop)
}

fn yaml_quoted(source: &[u8], at: usize, out: &mut Out) -> Result<usize, Bail> {
    let quote = source[at];
    let start = at + 1;
    let mut cursor = start;
    while cursor < source.len() {
        let byte = source[cursor];
        if quote == b'"' && byte == b'\\' {
            cursor += 2;
            continue;
        }
        if byte == quote {
            if quote == b'\'' && source.get(cursor + 1) == Some(&b'\'') {
                cursor += 2;
                continue;
            }
            out.text(start..cursor, quote == b'"', false, quote);
            return Ok(cursor + 1);
        }
        cursor += char_width(source, cursor);
    }
    Err(Bail("unterminated scalar"))
}

struct Here {
    tag: Vec<u8>,
    strip: bool,
}

struct Shell<'a> {
    source: &'a [u8],
    at: usize,
    out: Out,
    fresh: bool,
    heredocs: Vec<Here>,
}

impl<'a> Shell<'a> {
    fn new(source: &'a [u8]) -> Shell<'a> {
        Shell {
            source,
            at: 0,
            out: Out::default(),
            fresh: true,
            heredocs: Vec::new(),
        }
    }

    fn byte(&self, at: usize) -> u8 {
        self.source.get(at).copied().unwrap_or(0)
    }

    fn run(mut self) -> Result<Out, Bail> {
        self.code(None, 0)?;
        Ok(self.out)
    }

    fn code(&mut self, stop: Option<u8>, depth: usize) -> Result<(), Bail> {
        if depth > 32 {
            return Err(Bail("nested too deeply to read"));
        }
        while self.at < self.source.len() {
            let byte = self.byte(self.at);
            if Some(byte) == stop {
                self.at += 1;
                return Ok(());
            }
            match byte {
                b'\n' => {
                    self.at += 1;
                    self.fresh = true;
                    if !self.heredocs.is_empty() {
                        self.take_heredocs()?;
                    }
                }
                b' ' | b'\t' | b'\r' => {
                    self.at += 1;
                    self.fresh = true;
                }
                b'#' if self.fresh => {
                    let start = self.at;
                    let end = line_end(self.source, start);
                    self.out.comment(start..end, start + 1..end);
                    self.at = end;
                }
                b'\\' => {
                    self.at += 2;
                    self.fresh = false;
                }
                b'\'' => self.single()?,
                b'"' => self.double()?,
                b'`' => {
                    self.at += 1;
                    while self.at < self.source.len() && self.byte(self.at) != b'`' {
                        if self.byte(self.at) == b'\\' {
                            self.at += 1;
                        }
                        self.at += char_width(self.source, self.at);
                    }
                    if self.at >= self.source.len() {
                        return Err(Bail("unterminated command substitution"));
                    }
                    self.at += 1;
                    self.fresh = false;
                }
                b'$' => self.dollar(depth)?,
                b'<' if self.byte(self.at + 1) == b'<' => self.heredoc_head()?,
                b';' | b'&' | b'|' | b'(' | b')' | b'{' | b'}' | b'<' | b'>' => {
                    self.at += 1;
                    self.fresh = true;
                }
                _ => {
                    self.at += char_width(self.source, self.at);
                    self.fresh = false;
                }
            }
        }
        if stop.is_some() {
            return Err(Bail("unterminated substitution"));
        }
        Ok(())
    }

    fn single(&mut self) -> Result<(), Bail> {
        let start = self.at + 1;
        let mut at = start;
        while at < self.source.len() && self.byte(at) != b'\'' {
            at += char_width(self.source, at);
        }
        if at >= self.source.len() {
            return Err(Bail("unterminated string"));
        }
        self.out.text(start..at, false, true, b'\'');
        self.at = at + 1;
        self.fresh = false;
        Ok(())
    }

    fn double(&mut self) -> Result<(), Bail> {
        let start = self.at + 1;
        let mut at = start;
        while at < self.source.len() {
            match self.byte(at) {
                b'\\' => at += 2,
                b'"' => {
                    self.out.text(start..at, true, true, b'"');
                    self.at = at + 1;
                    self.fresh = false;
                    return Ok(());
                }
                _ => at += char_width(self.source, at),
            }
        }
        Err(Bail("unterminated string"))
    }

    fn dollar(&mut self, depth: usize) -> Result<(), Bail> {
        match self.byte(self.at + 1) {
            b'\'' => {
                let start = self.at + 2;
                let mut at = start;
                while at < self.source.len() {
                    match self.byte(at) {
                        b'\\' => at += 2,
                        b'\'' => {
                            self.out.text(start..at, true, true, b'\'');
                            self.at = at + 1;
                            self.fresh = false;
                            return Ok(());
                        }
                        _ => at += char_width(self.source, at),
                    }
                }
                Err(Bail("unterminated string"))
            }
            b'{' => {
                let mut at = self.at + 2;
                let mut braces = 1usize;
                while at < self.source.len() && braces > 0 {
                    match self.byte(at) {
                        b'{' => braces += 1,
                        b'}' => braces -= 1,
                        b'\\' => at += 1,
                        _ => {}
                    }
                    at += 1;
                }
                if braces > 0 {
                    return Err(Bail("unterminated expansion"));
                }
                self.at = at;
                self.fresh = false;
                Ok(())
            }
            b'(' if self.byte(self.at + 2) == b'(' => {
                let mut at = self.at + 3;
                while at < self.source.len() && !starts_with(self.source, at, b"))") {
                    at += 1;
                }
                if at >= self.source.len() {
                    return Err(Bail("unterminated arithmetic"));
                }
                self.at = at + 2;
                self.fresh = false;
                Ok(())
            }
            b'(' => {
                self.at += 2;
                self.fresh = true;
                self.code(Some(b')'), depth + 1)?;
                self.fresh = false;
                Ok(())
            }
            _ => {
                self.at += 1;
                self.fresh = false;
                Ok(())
            }
        }
    }

    fn heredoc_head(&mut self) -> Result<(), Bail> {
        let mut at = self.at + 2;
        if self.byte(at) == b'<' {
            self.at = at + 1;
            self.fresh = true;
            return Ok(());
        }
        let strip = self.byte(at) == b'-';
        if strip {
            at += 1;
        }
        while matches!(self.byte(at), b' ' | b'\t') {
            at += 1;
        }
        let mut tag = Vec::new();
        match self.byte(at) {
            quote @ (b'\'' | b'"') => {
                at += 1;
                while at < self.source.len() && self.byte(at) != quote {
                    tag.push(self.byte(at));
                    at += 1;
                }
                if at >= self.source.len() {
                    return Err(Bail("unterminated heredoc tag"));
                }
                at += 1;
            }
            _ => {
                while at < self.source.len() && identifier_byte(self.byte(at)) {
                    tag.push(self.byte(at));
                    at += 1;
                }
            }
        }
        if tag.is_empty() {
            self.at += 2;
            self.fresh = true;
            return Ok(());
        }
        self.heredocs.push(Here { tag, strip });
        self.at = at;
        self.fresh = false;
        Ok(())
    }

    fn take_heredocs(&mut self) -> Result<(), Bail> {
        let queued: Vec<Here> = self.heredocs.drain(..).collect();
        for here in queued {
            let body = self.at;
            loop {
                let begin = self.at;
                let end = line_end(self.source, begin);
                let mut probe = begin;
                if here.strip {
                    while probe < end && self.byte(probe) == b'\t' {
                        probe += 1;
                    }
                }
                if &self.source[probe..end] == here.tag.as_slice() {
                    self.out.text(body..begin, false, true, 0);
                    self.at = (end + 1).min(self.source.len());
                    break;
                }
                if end >= self.source.len() {
                    return Err(Bail("unterminated heredoc"));
                }
                self.at = end + 1;
            }
        }
        self.fresh = true;
        Ok(())
    }
}

struct Ruby<'a> {
    source: &'a [u8],
    at: usize,
    out: Out,
    value: bool,
    heredocs: Vec<Here>,
}

impl<'a> Ruby<'a> {
    fn new(source: &'a [u8]) -> Ruby<'a> {
        Ruby {
            source,
            at: 0,
            out: Out::default(),
            value: false,
            heredocs: Vec::new(),
        }
    }

    fn byte(&self, at: usize) -> u8 {
        self.source.get(at).copied().unwrap_or(0)
    }

    fn run(mut self) -> Result<Out, Bail> {
        while self.at < self.source.len() {
            let byte = self.byte(self.at);
            let starts_line = self.at == 0 || self.byte(self.at - 1) == b'\n';
            if starts_line && starts_with(self.source, self.at, b"=begin") {
                let start = self.at;
                let mut at = self.at;
                loop {
                    at = line_end(self.source, at);
                    if at >= self.source.len() {
                        return Err(Bail("unterminated block comment"));
                    }
                    at += 1;
                    if starts_with(self.source, at, b"=end") {
                        let end = line_end(self.source, at);
                        self.out.comment(start..end, start + 6..at);
                        self.at = end;
                        break;
                    }
                }
                continue;
            }
            match byte {
                b'\n' => {
                    self.at += 1;
                    self.value = false;
                    if !self.heredocs.is_empty() {
                        self.take_heredocs()?;
                    }
                }
                b'#' => {
                    let start = self.at;
                    let end = line_end(self.source, start);
                    self.out.comment(start..end, start + 1..end);
                    self.at = end;
                }
                b'\'' => self.plain(b'\'')?,
                b'"' => self.plain(b'"')?,
                b'`' => self.plain(b'`')?,
                b'%' if !self.value && self.percent()? => {}
                b'?' if !self.value
                    && self.byte(self.at + 1).is_ascii_alphanumeric()
                    && !identifier_byte(self.byte(self.at + 2)) =>
                {
                    self.at += 2;
                    self.value = true;
                }
                b'<' if self.byte(self.at + 1) == b'<' && self.heredoc_head()? => {}
                b'/' if !self.value => {
                    if !self.regex() {
                        self.at += 1;
                        self.value = false;
                    }
                }
                _ if identifier_byte(byte) => {
                    while self.at < self.source.len() && identifier_byte(self.byte(self.at)) {
                        self.at += char_width(self.source, self.at);
                    }
                    self.value = true;
                }
                b')' | b']' | b'}' => {
                    self.at += 1;
                    self.value = true;
                }
                _ => {
                    self.at += char_width(self.source, self.at);
                    if !byte.is_ascii_whitespace() {
                        self.value = false;
                    }
                }
            }
        }
        Ok(self.out)
    }

    fn plain(&mut self, quote: u8) -> Result<(), Bail> {
        let start = self.at + 1;
        let mut at = start;
        while at < self.source.len() {
            match self.byte(at) {
                b'\\' => at += 2,
                byte if byte == quote => {
                    self.out.text(start..at, true, true, quote);
                    self.at = at + 1;
                    self.value = true;
                    return Ok(());
                }
                _ => at += char_width(self.source, at),
            }
        }
        Err(Bail("unterminated string"))
    }

    fn percent(&mut self) -> Result<bool, Bail> {
        let mut at = self.at + 1;
        let kind = self.byte(at);
        if kind.is_ascii_alphabetic() {
            if !matches!(kind, b'q' | b'Q' | b'w' | b'W' | b'i' | b'I' | b'r' | b's') {
                return Ok(false);
            }
            at += 1;
        }
        let open = self.byte(at);
        let close = match open {
            b'(' => b')',
            b'[' => b']',
            b'{' => b'}',
            b'<' => b'>',
            byte if byte.is_ascii_punctuation() => byte,
            _ => return Ok(false),
        };
        let start = at + 1;
        let mut cursor = start;
        let mut depth = 1usize;
        while cursor < self.source.len() {
            let byte = self.byte(cursor);
            if byte == b'\\' {
                cursor += 2;
                continue;
            }
            if byte == open && open != close {
                depth += 1;
            } else if byte == close {
                depth -= 1;
                if depth == 0 {
                    if kind != b'r' {
                        self.out.text(start..cursor, true, true, 0);
                    }
                    self.at = cursor + 1;
                    self.value = true;
                    return Ok(true);
                }
            }
            cursor += char_width(self.source, cursor);
        }
        Err(Bail("unterminated literal"))
    }

    fn regex(&mut self) -> bool {
        let mut at = self.at + 1;
        let mut class = false;
        while at < self.source.len() {
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
                    while at < self.source.len() && identifier_byte(self.byte(at)) {
                        at += 1;
                    }
                    self.at = at;
                    self.value = true;
                    return true;
                }
                b'\n' => return false,
                _ => at += char_width(self.source, at),
            }
        }
        false
    }

    fn heredoc_head(&mut self) -> Result<bool, Bail> {
        let mut at = self.at + 2;
        let strip = matches!(self.byte(at), b'~' | b'-');
        if strip {
            at += 1;
        }
        let mut tag = Vec::new();
        match self.byte(at) {
            quote @ (b'\'' | b'"') => {
                at += 1;
                while at < self.source.len() && self.byte(at) != quote {
                    tag.push(self.byte(at));
                    at += 1;
                }
                if at >= self.source.len() {
                    return Err(Bail("unterminated heredoc tag"));
                }
                at += 1;
            }
            byte if byte.is_ascii_uppercase() || byte == b'_' => {
                while at < self.source.len() && identifier_byte(self.byte(at)) {
                    tag.push(self.byte(at));
                    at += 1;
                }
            }
            _ => return Ok(false),
        }
        if tag.is_empty() {
            return Ok(false);
        }
        self.heredocs.push(Here { tag, strip });
        self.at = at;
        self.value = true;
        Ok(true)
    }

    fn take_heredocs(&mut self) -> Result<(), Bail> {
        let queued: Vec<Here> = self.heredocs.drain(..).collect();
        for here in queued {
            let body = self.at;
            loop {
                let begin = self.at;
                let end = line_end(self.source, begin);
                let mut probe = begin;
                if here.strip {
                    while probe < end && self.byte(probe).is_ascii_whitespace() {
                        probe += 1;
                    }
                }
                if &self.source[probe..end] == here.tag.as_slice() {
                    self.out.text(body..begin, true, true, 0);
                    self.at = (end + 1).min(self.source.len());
                    break;
                }
                if end >= self.source.len() {
                    return Err(Bail("unterminated heredoc"));
                }
                self.at = end + 1;
            }
        }
        Ok(())
    }
}
