use crate::edit::{self, Cut, Edited, Kind};
use crate::glyphs;
use crate::keep::{self, Kept};
use crate::lang::{Dialect, Family};
use crate::luadoc;
use crate::scan;
use crate::{docstring, scan::Comment};

#[derive(Default, Clone, Copy)]
pub struct Saved {
    pub shebangs: usize,
    pub important: usize,
    pub directives: usize,
    pub licenses: usize,
}

impl Saved {
    pub fn any(&self) -> bool {
        self.shebangs + self.important + self.directives + self.licenses > 0
    }

    pub fn add(&mut self, other: Saved) {
        self.shebangs += other.shebangs;
        self.important += other.important;
        self.directives += other.directives;
        self.licenses += other.licenses;
    }
}

pub enum Outcome {
    Changed(Box<Edited>, Saved),
    Untouched(Saved),
    Skipped(&'static str),
}

fn leading(source: &[u8], comments: &[Comment], dialect: Dialect) -> usize {
    let mut cursor = if dialect.family() == Family::Css {
        after_charset(source)
    } else {
        0
    };
    let mut count = 0usize;
    for comment in comments {
        if source[cursor..comment.span.start]
            .iter()
            .any(|byte| !byte.is_ascii_whitespace())
        {
            break;
        }
        cursor = comment.span.end;
        count += 1;
    }
    count
}

fn after_charset(source: &[u8]) -> usize {
    let mut at = 0usize;
    while at < source.len() && source[at].is_ascii_whitespace() {
        at += 1;
    }
    if source.len() < at + 8 || !source[at..at + 8].eq_ignore_ascii_case(b"@charset") {
        return 0;
    }
    at += 8;
    while at < source.len() && source[at].is_ascii_whitespace() {
        at += 1;
    }
    let quote = match source.get(at) {
        Some(b'"') => b'"',
        Some(b'\'') => b'\'',
        _ => return 0,
    };
    at += 1;
    let mut closed = false;
    while at < source.len() {
        match source[at] {
            b'\\' => at += 2,
            byte if byte == quote => {
                at += 1;
                closed = true;
                break;
            }
            b'\n' => return 0,
            _ => at += 1,
        }
    }
    if !closed {
        return 0;
    }
    while at < source.len() && source[at].is_ascii_whitespace() {
        at += 1;
    }
    if source.get(at) == Some(&b';') {
        at + 1
    } else {
        0
    }
}

fn wordy(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-') || byte >= 0x80
}

fn merging(left: u8, right: u8) -> bool {
    if wordy(left) && (wordy(right) || right == b'(') {
        return true;
    }
    if matches!(left, b'#' | b'@' | b'$' | b'%') && wordy(right) {
        return true;
    }
    if left == b'+' && (right.is_ascii_digit() || right == b'.') {
        return true;
    }
    if left == b'.' && right.is_ascii_digit() {
        return true;
    }
    if left.is_ascii_digit() && matches!(right, b'.' | b'%') {
        return true;
    }
    false
}

fn needs_style_space(source: &[u8], start: usize, end: usize, padded: bool) -> bool {
    if start == 0 || end >= source.len() {
        return false;
    }
    let left = source[start - 1];
    let right = source[end];
    if left.is_ascii_whitespace() || right.is_ascii_whitespace() {
        return false;
    }
    padded || merging(left, right)
}

pub fn purge(source: &[u8], dialect: Dialect) -> Outcome {
    let out = match scan::scan(source, dialect) {
        Ok(out) => out,
        Err(bail) => return Outcome::Skipped(bail.0),
    };
    let mut saved = Saved::default();
    let mut cuts: Vec<Cut> = Vec::new();
    let heading = leading(source, &out.comments, dialect);
    for (index, comment) in out.comments.iter().enumerate() {
        match keep::kept(source, comment, dialect, index < heading) {
            Some(Kept::Shebang) => {
                saved.shebangs += 1;
                continue;
            }
            Some(Kept::Important) => {
                saved.important += 1;
                continue;
            }
            Some(Kept::Directive) => {
                saved.directives += 1;
                if dialect == Dialect::Lua
                    && let Some(cut) = luadoc::description(source, comment)
                {
                    cuts.push(cut);
                }
                continue;
            }
            Some(Kept::License) => {
                saved.licenses += 1;
                continue;
            }
            None => {}
        }
        let (span, with) = match edit::whole_lines(source, &comment.span) {
            Some(widened) => (widened, None),
            None => {
                let mut start = comment.span.start;
                while start > 0 && matches!(source[start - 1], b' ' | b'\t') {
                    start -= 1;
                }
                let mut end = comment.span.end;
                while end < source.len() && matches!(source[end], b' ' | b'\t') {
                    end += 1;
                }
                let padded = start < comment.span.start && end > comment.span.end;
                let space = if dialect.family() == Family::Css {
                    let style_padded = start < comment.span.start || end > comment.span.end;
                    needs_style_space(source, start, end, style_padded)
                } else {
                    padded
                };
                (start..end, space.then(|| " ".to_string()))
            }
        };
        cuts.push(Cut {
            span,
            with,
            kind: Kind::Comment,
            lines: false,
        });
    }
    cuts.extend(docstring::cuts(source, &out));
    for text in &out.texts {
        for swap in glyphs::sweep(source, text) {
            cuts.push(Cut {
                span: swap.span,
                with: (!swap.with.is_empty()).then_some(swap.with),
                kind: Kind::Glyph,
                lines: false,
            });
        }
    }
    let edited = edit::apply(source, cuts);
    if !edited.touched() {
        return Outcome::Untouched(saved);
    }
    if scan::scan(&edited.content, dialect).is_err() {
        return Outcome::Skipped("the purged file would not read back");
    }
    Outcome::Changed(Box::new(edited), saved)
}
