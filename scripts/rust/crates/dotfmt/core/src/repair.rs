//! Fixes made to input that nearly parses, each kept with where it was made.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Repair {
    Comma,
    MissingComma,
    Comment,
    Quote,
    Key,
    Literal,
    Number,
    Surrogate,
    Control,
    Space,
    Utf8,
    Fence,
    Separator,
    Colon,
    Bracket,
    Indent,
}

impl Repair {
    pub const ALL: [Repair; 16] = [
        Repair::Comma,
        Repair::MissingComma,
        Repair::Comment,
        Repair::Quote,
        Repair::Key,
        Repair::Literal,
        Repair::Number,
        Repair::Surrogate,
        Repair::Control,
        Repair::Space,
        Repair::Utf8,
        Repair::Fence,
        Repair::Separator,
        Repair::Colon,
        Repair::Bracket,
        Repair::Indent,
    ];
}

/// One fix; `line` and `column` are 1-based, and `column` counts bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Fix {
    pub repair: Repair,
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Repairs {
    fixes: Vec<Fix>,
}

impl Repairs {
    pub fn add(&mut self, fix: Fix) {
        self.fixes.push(fix);
    }

    pub fn of(&self, repair: Repair) -> usize {
        self.fixes.iter().filter(|fix| fix.repair == repair).count()
    }

    pub fn total(&self) -> usize {
        self.fixes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fixes.is_empty()
    }

    pub fn fixes(&self) -> &[Fix] {
        &self.fixes
    }
}

impl FromIterator<Fix> for Repairs {
    fn from_iter<I: IntoIterator<Item = Fix>>(fixes: I) -> Self {
        Self {
            fixes: fixes.into_iter().collect(),
        }
    }
}

impl Extend<Fix> for Repairs {
    fn extend<I: IntoIterator<Item = Fix>>(&mut self, fixes: I) {
        self.fixes.extend(fixes);
    }
}

/// Replaces `start..end` of a source with `text`; equal bounds insert.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

impl Edit {
    pub fn insert(at: usize, text: impl Into<String>) -> Self {
        Self::replace(at, at, text)
    }

    pub fn delete(start: usize, end: usize) -> Self {
        Self::replace(start, end, "")
    }

    pub fn replace(start: usize, end: usize, text: impl Into<String>) -> Self {
        Self {
            start,
            end,
            text: text.into(),
        }
    }
}

/// Edits made against one source, applied in offset order; insertions at one
/// offset keep the order they were made in and land before a replacement there.
pub fn apply(source: &[u8], edits: &[Edit]) -> Vec<u8> {
    let mut ordered: Vec<&Edit> = edits.iter().collect();
    ordered.sort_by_key(|edit| (edit.start, edit.end));
    let mut output = Vec::with_capacity(source.len() + edits.len());
    let mut at = 0;
    for edit in ordered {
        let start = edit.start.clamp(at, source.len());
        output.extend_from_slice(&source[at..start]);
        output.extend_from_slice(edit.text.as_bytes());
        at = edit.end.clamp(start, source.len());
    }
    output.extend_from_slice(&source[at..]);
    output
}

/// Byte offsets to lines and columns, walking forward from the last answer.
pub struct Locator<'a> {
    source: &'a [u8],
    at: usize,
    line: usize,
    start: usize,
}

impl<'a> Locator<'a> {
    pub fn new(source: &'a [u8]) -> Self {
        Self {
            source,
            at: 0,
            line: 1,
            start: 0,
        }
    }

    pub fn locate(&mut self, offset: usize) -> (usize, usize) {
        let offset = offset.min(self.source.len());
        if offset < self.at {
            *self = Self::new(self.source);
        }
        for (index, byte) in self.source[self.at..offset].iter().enumerate() {
            if *byte == b'\n' {
                self.line += 1;
                self.start = self.at + index + 1;
            }
        }
        self.at = offset;
        (self.line, offset - self.start + 1)
    }

    pub fn fix(&mut self, repair: Repair, offset: usize) -> Fix {
        let (line, column) = self.locate(offset);
        Fix {
            repair,
            line,
            column,
        }
    }
}

pub struct Repaired {
    pub text: String,
    pub repairs: Repairs,
}

const ATTEMPTS: usize = 256;

/// Where a parse stopped, as a byte offset, and what it said.
pub struct Failure {
    pub at: usize,
    pub message: String,
}

/// Where a separator missing before the token at `at` goes: after the token
/// before it, or right before `at` when a comment could sit between the two.
pub fn separator(text: &str, at: usize, comment: &str) -> usize {
    let end = text[..at].trim_end().len();
    let line = text[..end].rfind('\n').map_or(0, |newline| newline + 1);
    if end == 0 || text[line..end].contains(comment) {
        return at;
    }
    end
}

/// Parses, fixes what the error points at, and parses again until the source
/// reads or the next fixes fail to move the error forward. The text answered
/// is the furthest that read.
pub fn settle(
    source: &str,
    mut parse: impl FnMut(&str) -> Option<Failure>,
    mut fix: impl FnMut(&str, &Failure) -> Vec<(Repair, Edit)>,
) -> Repaired {
    let mut text = source.to_string();
    let mut fixes = Vec::new();
    let mut failed = parse(&text);
    for _ in 0..ATTEMPTS {
        let Some(failure) = &failed else {
            break;
        };
        let at = failure.at;
        let step = fix(&text, failure);
        let edits: Vec<Edit> = step.iter().map(|(_, edit)| edit.clone()).collect();
        if edits.is_empty() {
            break;
        }
        let Ok(next) = String::from_utf8(apply(text.as_bytes(), &edits)) else {
            break;
        };
        let shift: isize = edits
            .iter()
            .filter(|edit| edit.start <= at)
            .map(|edit| edit.text.len() as isize - (edit.end - edit.start) as isize)
            .sum();
        let after = parse(&next);
        if after
            .as_ref()
            .is_some_and(|after| after.at as isize - shift <= at as isize)
        {
            break;
        }
        let mut locator = Locator::new(text.as_bytes());
        let mut starts: Vec<(usize, Repair)> = step
            .iter()
            .map(|(repair, edit)| (edit.start, *repair))
            .collect();
        starts.sort_by_key(|(start, _)| *start);
        fixes.extend(
            starts
                .into_iter()
                .map(|(start, repair)| locator.fix(repair, start)),
        );
        text = next;
        failed = after;
    }
    fixes.sort_by_key(|fix: &Fix| (fix.line, fix.column));
    Repaired {
        text,
        repairs: fixes.into_iter().collect(),
    }
}
