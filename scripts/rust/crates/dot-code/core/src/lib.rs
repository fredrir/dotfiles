#![forbid(unsafe_code)]

use std::ops::Range;

pub use serde_json::{Error as SettingsError, Value};

/// Replaces a byte range of the original source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
}

impl Edit {
    pub fn insert(offset: usize, text: String) -> Self {
        Self {
            range: offset..offset,
            text,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub title: &'static str,
    pub kind: &'static str,
    pub edits: Vec<Edit>,
    /// Byte offset into the edited source where an inline rename starts.
    pub rename: Option<usize>,
    pub disabled: Option<&'static str>,
}

impl Action {
    pub fn new(title: &'static str, kind: &'static str, edits: Vec<Edit>) -> Self {
        Self {
            title,
            kind,
            edits,
            rename: None,
            disabled: None,
        }
    }

    pub fn disabled(title: &'static str, kind: &'static str, reason: &'static str) -> Self {
        Self {
            disabled: Some(reason),
            ..Self::new(title, kind, Vec::new())
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Invoked,
    Automatic,
}

pub struct Request<'a> {
    pub source: &'a str,
    pub language: &'a str,
    pub selection: Range<usize>,
    pub trigger: Trigger,
    pub only: &'a [String],
}

impl Request<'_> {
    pub fn wants(&self, kind: &str) -> bool {
        self.only.is_empty() || self.only.iter().any(|parent| contains(parent, kind))
    }

    /// Explicit requests for a kind surface disabled actions with their reason.
    pub fn explains(&self) -> bool {
        self.trigger == Trigger::Invoked && !self.only.is_empty()
    }
}

/// Code action kinds are hierarchical: `refactor` contains `refactor.extract`.
pub fn contains(parent: &str, kind: &str) -> bool {
    parent.is_empty()
        || kind
            .strip_prefix(parent)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
}

/// Applies non-overlapping edits, in any order, to `source`.
pub fn apply(source: &str, edits: &[Edit]) -> String {
    let mut sorted: Vec<&Edit> = edits.iter().collect();
    sorted.sort_by_key(|edit| (edit.range.start, edit.range.end));
    let mut output = String::with_capacity(source.len());
    let mut cursor = 0;
    for edit in sorted {
        output.push_str(&source[cursor..edit.range.start]);
        output.push_str(&edit.text);
        cursor = edit.range.end;
    }
    output.push_str(&source[cursor..]);
    output
}

/// A language-aware capability the server routes documents to.
pub trait Feature {
    /// The settings key under `dotCode` this feature reads.
    fn section(&self) -> &'static str;
    fn languages(&self) -> &'static [&'static str];
    fn kinds(&self) -> &'static [&'static str];
    fn configure(&mut self, settings: Value) -> Result<(), SettingsError>;
    fn code_actions(&mut self, request: &Request) -> Vec<Action>;
}
