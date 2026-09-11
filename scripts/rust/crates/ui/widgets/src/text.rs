use ui_terminal::text::sanitize;
use ui_theme::{Role, Style};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub role: Role,
}

impl Default for Span {
    fn default() -> Self {
        Self::new("", Role::Plain)
    }
}

impl Span {
    pub fn new(text: impl Into<String>, role: Role) -> Self {
        Self {
            text: text.into(),
            role,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Line {
    pub spans: Vec<Span>,
}

impl Line {
    pub fn plain(text: impl Into<String>) -> Self {
        Self::styled(text, Role::Plain)
    }
    pub fn styled(text: impl Into<String>, role: Role) -> Self {
        Self {
            spans: vec![Span::new(text, role)],
        }
    }
    pub fn from_spans(spans: impl IntoIterator<Item = Span>) -> Self {
        Self {
            spans: spans.into_iter().collect(),
        }
    }
    pub fn width(&self) -> usize {
        self.spans
            .iter()
            .map(|span| UnicodeWidthStr::width(span.text.as_str()))
            .sum()
    }

    pub fn fitted(&self, limit: usize) -> Self {
        let sanitized: Vec<_> = self
            .spans
            .iter()
            .map(|span| Span::new(sanitize(&span.text), span.role))
            .collect();
        let width: usize = sanitized
            .iter()
            .map(|span| UnicodeWidthStr::width(span.text.as_str()))
            .sum();
        if width <= limit {
            return Self { spans: sanitized };
        }
        if limit == 0 {
            return Self::default();
        }
        let mut result = Self::default();
        let mut used = 0;
        let mut role = Role::Plain;
        'spans: for span in sanitized {
            role = span.role;
            let mut text = String::new();
            for character in span.text.chars() {
                let width = UnicodeWidthChar::width(character).unwrap_or(0);
                if used + width > limit - 1 {
                    if !text.is_empty() {
                        result.spans.push(Span::new(text, role));
                    }
                    break 'spans;
                }
                text.push(character);
                used += width;
            }
            if !text.is_empty() {
                result.spans.push(Span::new(text, role));
            }
        }
        result.spans.push(Span::new("…", role));
        result
    }

    pub fn paint(&self, style: &Style, limit: usize) -> String {
        self.fitted(limit)
            .spans
            .into_iter()
            .map(|span| style.paint(span.role, &span.text))
            .collect()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PromptBuffer {
    text: String,
}

impl PromptBuffer {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
    pub fn as_str(&self) -> &str {
        &self.text
    }
    pub fn insert(&mut self, character: char) {
        if !character.is_control() {
            self.text.push(character);
        }
    }
    pub fn backspace(&mut self) {
        self.text.pop();
    }
    pub fn clear(&mut self) {
        self.text.clear();
    }
    pub fn word_back(&mut self) {
        self.text
            .truncate(self.text.trim_end_matches(char::is_whitespace).len());
        if let Some((index, character)) = self
            .text
            .char_indices()
            .rev()
            .find(|(_, character)| *character == '/' || character.is_whitespace())
        {
            let keep = match character {
                '/' if index == 0 => 1,
                '/' => index,
                _ => index + character.len_utf8(),
            };
            self.text.truncate(keep);
        } else {
            self.text.clear();
        }
    }
    pub fn into_string(self) -> String {
        self.text
    }
}
