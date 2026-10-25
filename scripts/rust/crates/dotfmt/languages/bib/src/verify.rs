use crate::config::NameCase;
use crate::parse::{Assign, Block, Comment, Document, Entry, PartKind, Preamble, Trivia, Value};

#[derive(Debug, PartialEq, Eq)]
pub struct Fingerprint {
    items: Vec<Item>,
    trailer: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct Item {
    leading: Vec<String>,
    block: BlockFp,
}

#[derive(Debug, PartialEq, Eq)]
enum BlockFp {
    Entry(EntryFp),
    Assign(AssignFp),
    Preamble(PreambleFp),
    Comment(String),
}

#[derive(Debug, PartialEq, Eq)]
struct EntryFp {
    kind: String,
    open: u8,
    close: u8,
    key: String,
    fields: Vec<FieldFp>,
    trailing: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct FieldFp {
    name: String,
    value: Vec<(PartKind, String)>,
    leading: Vec<String>,
    trailing: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct AssignFp {
    open: u8,
    close: u8,
    name: String,
    value: Vec<(PartKind, String)>,
    leading: Vec<String>,
    trailing: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct PreambleFp {
    open: u8,
    close: u8,
    inner: String,
}

pub fn of(document: &Document, text: &str, collapse: bool) -> Fingerprint {
    Fingerprint {
        items: document
            .items
            .iter()
            .map(|item| Item {
                leading: trivia(&item.leading, text),
                block: block(&item.block, text, collapse),
            })
            .collect(),
        trailer: trivia(&document.trailer, text),
    }
}

fn block(block: &Block, text: &str, collapse: bool) -> BlockFp {
    match block {
        Block::Entry(entry) => BlockFp::Entry(entry_fp(entry, text, collapse)),
        Block::Assign(assign) => BlockFp::Assign(assign_fp(assign, text)),
        Block::Preamble(preamble) => BlockFp::Preamble(preamble_fp(preamble, text)),
        Block::Comment(comment) => BlockFp::Comment(comment.span.slice(text).to_owned()),
    }
}

fn entry_fp(entry: &Entry, text: &str, collapse: bool) -> EntryFp {
    EntryFp {
        kind: entry.kind.slice(text).to_ascii_lowercase(),
        open: entry.open,
        close: entry.close,
        key: entry.key.slice(text).trim().to_owned(),
        fields: entry
            .fields
            .iter()
            .map(|field| FieldFp {
                name: field.name.slice(text).to_ascii_lowercase(),
                value: value(&field.value, text, collapse),
                leading: comments(&field.leading, text),
                trailing: field.trailing.map(|comment| comment.text(text).to_owned()),
            })
            .collect(),
        trailing: comments(&entry.trailing, text),
    }
}

fn assign_fp(assign: &Assign, text: &str) -> AssignFp {
    AssignFp {
        open: assign.open,
        close: assign.close,
        name: assign.name.slice(text).to_ascii_lowercase(),
        value: value(&assign.value, text, false),
        leading: comments(&assign.leading, text),
        trailing: comments(&assign.trailing, text),
    }
}

fn preamble_fp(preamble: &Preamble, text: &str) -> PreambleFp {
    PreambleFp {
        open: preamble.open,
        close: preamble.close,
        inner: preamble.inner.slice(text).trim().to_owned(),
    }
}

fn value(value: &Value, text: &str, collapse: bool) -> Vec<(PartKind, String)> {
    value
        .parts
        .iter()
        .map(|part| {
            let text = part.span.slice(text);
            let text = if collapse {
                text.split_whitespace().collect::<Vec<_>>().join(" ")
            } else {
                text.to_owned()
            };
            (part.kind, text)
        })
        .collect()
}

fn comments(comments: &[Comment], text: &str) -> Vec<String> {
    comments
        .iter()
        .map(|comment| comment.text(text).to_owned())
        .collect()
}

fn trivia(trivia: &Trivia, text: &str) -> Vec<String> {
    trivia
        .span
        .slice(text)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

pub fn case(name: &str, case: NameCase) -> String {
    match case {
        NameCase::Preserve => name.to_owned(),
        NameCase::Lower => name.to_ascii_lowercase(),
        NameCase::Upper => name.to_ascii_uppercase(),
    }
}
