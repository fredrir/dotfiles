//! Byte-preserving lexical primitives shared by configuration and block formatting.
//!
//! Callers decide whether braces, comments, and whitespace are syntax or data.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuoteMode {
    Anywhere,
    Start,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceSpan {
    pub start: usize,
    pub end: usize,
}

impl SourceSpan {
    pub fn slice(self, source: &str) -> &str {
        &source[self.start..self.end]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scanned {
    pub character: char,
    pub span: SourceSpan,
    pub quoted: bool,
    pub escaped: bool,
}

impl Scanned {
    pub fn is_structural(self) -> bool {
        !self.quoted && !self.escaped
    }
}

pub struct Scanner<'a> {
    characters: std::str::CharIndices<'a>,
    quotes: QuoteMode,
    quote: Option<char>,
    escaped: bool,
}

pub fn scan(text: &str, quotes: QuoteMode) -> Scanner<'_> {
    Scanner {
        characters: text.char_indices(),
        quotes,
        quote: None,
        escaped: false,
    }
}

impl Iterator for Scanner<'_> {
    type Item = Scanned;

    fn next(&mut self) -> Option<Self::Item> {
        let (start, character) = self.characters.next()?;
        let escaped = self.escaped;
        let mut quoted = self.quote.is_some();
        if escaped {
            self.escaped = false;
        } else if character == '\\' {
            self.escaped = true;
        } else if self.quote == Some(character) {
            self.quote = None;
        } else if self.quote.is_none()
            && matches!(character, '\'' | '"')
            && (self.quotes == QuoteMode::Anywhere || start == 0)
        {
            self.quote = Some(character);
            quoted = true;
        }
        Some(Scanned {
            character,
            span: SourceSpan {
                start,
                end: start + character.len_utf8(),
            },
            quoted,
            escaped,
        })
    }
}
