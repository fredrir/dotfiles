use std::ops::Range;

use crate::lang::Dialect;

#[derive(Clone, Debug)]
pub struct Comment {
    pub span: Range<usize>,
    pub body: Range<usize>,
}

#[derive(Clone, Debug)]
pub struct Text {
    pub body: Range<usize>,
    pub escapes: bool,
    pub folds: bool,
    pub quote: u8,
}

#[derive(Clone, Debug)]
pub struct Logical {
    pub start: usize,
    pub end: usize,
    pub indent: usize,
    pub sole_text: Option<Range<usize>>,
}

#[derive(Default)]
pub struct Out {
    pub comments: Vec<Comment>,
    pub texts: Vec<Text>,
    pub logical: Vec<Logical>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bail(pub &'static str);

impl Out {
    pub fn comment(&mut self, span: Range<usize>, body: Range<usize>) {
        self.comments.push(Comment { span, body });
    }

    pub fn text(&mut self, body: Range<usize>, escapes: bool, folds: bool, quote: u8) {
        if !body.is_empty() {
            self.texts.push(Text {
                body,
                escapes,
                folds,
                quote,
            });
        }
    }
}

pub fn scan(source: &[u8], dialect: Dialect) -> Result<Out, Bail> {
    crate::engine::scan(source, dialect.syntax())
}

pub fn char_width(source: &[u8], at: usize) -> usize {
    let lead = source[at];
    let width = if lead < 0x80 {
        1
    } else if lead >> 5 == 0b110 {
        2
    } else if lead >> 4 == 0b1110 {
        3
    } else if lead >> 3 == 0b11110 {
        4
    } else {
        1
    };
    width.min(source.len() - at)
}

/// The end of the line `from` sits on: the first `\r` or `\n`, or the end of
/// the source. Stopping at `\r` matters: a file with bare-CR line endings has
/// no `\n` at all, and a scanner that only looked for `\n` would stretch every
/// line comment to the end of the file.
pub fn line_end(source: &[u8], from: usize) -> usize {
    let mut at = from;
    while at < source.len() && !matches!(source[at], b'\n' | b'\r') {
        at += 1;
    }
    at
}

/// Whether `byte` ends a line. Every scanner shares this so that bare-CR and
/// CRLF sources agree with `\n` sources about where lines break.
pub fn ends_line(byte: u8) -> bool {
    matches!(byte, b'\n' | b'\r')
}

pub fn starts_with(source: &[u8], at: usize, needle: &[u8]) -> bool {
    source.len() >= at + needle.len() && &source[at..at + needle.len()] == needle
}

pub fn identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}
