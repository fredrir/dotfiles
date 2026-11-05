//! Minimal line edits between a buffer and its formatted replacement.

use std::ops::Range;

use lsp_types::TextEdit;
use similar::{ChangeTag, TextDiff};

use dotfmt_core::edit::Edit;

use crate::documents::Document;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ByteEdit {
    pub start: usize,
    pub end: usize,
    pub new_text: String,
}

impl ByteEdit {
    fn new(start: usize, end: usize, new_text: String) -> Self {
        Self {
            start,
            end,
            new_text,
        }
    }
}

/// A contiguous line replacement, before formatting it into UTF-16 ranges.
pub fn minimal(before: &str, after: &str) -> Vec<ByteEdit> {
    if before == after {
        return Vec::new();
    }
    let starts = line_starts(before);
    let diff = TextDiff::from_lines(before, after);
    let mut edits = Vec::new();
    let mut old_line = 0;
    let mut hunk: Option<Hunk> = None;

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => {
                if let Some(hunk) = hunk.take() {
                    push(&mut edits, &starts, before.len(), hunk);
                }
                old_line += 1;
            }
            ChangeTag::Delete => {
                let hunk = hunk.get_or_insert_with(|| Hunk {
                    start: old_line,
                    removed: 0,
                    text: String::new(),
                });
                hunk.removed += 1;
                old_line += 1;
            }
            ChangeTag::Insert => {
                let hunk = hunk.get_or_insert_with(|| Hunk {
                    start: old_line,
                    removed: 0,
                    text: String::new(),
                });
                hunk.text.push_str(change.value());
            }
        }
    }
    if let Some(hunk) = hunk {
        push(&mut edits, &starts, before.len(), hunk);
    }
    edits
}

struct Hunk {
    start: usize,
    removed: usize,
    text: String,
}

fn push(edits: &mut Vec<ByteEdit>, starts: &[usize], len: usize, hunk: Hunk) {
    let start = starts.get(hunk.start).copied().unwrap_or(len);
    let end = starts
        .get(hunk.start + hunk.removed)
        .copied()
        .unwrap_or(len);
    if start != end || !hunk.text.is_empty() {
        edits.push(ByteEdit::new(start, end, hunk.text));
    }
}

fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(
        text.bytes()
            .enumerate()
            .filter(|(_, byte)| *byte == b'\n')
            .map(|(offset, _)| offset + 1),
    );
    starts
}

/// Keeps the edits that touch at least one requested UTF-16 range.
pub fn filter(
    document: &Document,
    edits: Vec<ByteEdit>,
    ranges: &[lsp_types::Range],
) -> Vec<ByteEdit> {
    let byte_ranges: Vec<Range<usize>> = ranges
        .iter()
        .filter_map(|range| document.range(*range))
        .collect();
    edits
        .into_iter()
        .filter(|edit| byte_ranges.iter().any(|range| touches(edit, range)))
        .collect()
}

fn touches(edit: &ByteEdit, range: &Range<usize>) -> bool {
    if edit.start == edit.end {
        range.start <= edit.start && edit.start <= range.end
    } else {
        edit.start < range.end && edit.end > range.start
    }
}

/// Converts fixes from the dotfmt analysis pass into UTF-16 text edits.
pub fn fix_text_edits(document: &Document, edits: &[Edit]) -> Vec<TextEdit> {
    let mut edits: Vec<TextEdit> = edits
        .iter()
        .map(|edit| {
            let start = clamp(document.text(), edit.start);
            let end = clamp(document.text(), edit.end).max(start);
            TextEdit {
                range: document.lsp_range(&(start..end)),
                new_text: edit.new_text.clone(),
            }
        })
        .collect();
    edits.sort_by_key(|edit| (edit.range.start.line, edit.range.start.character));
    edits
}

fn clamp(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

pub fn text_edits(document: &Document, edits: &[ByteEdit]) -> Vec<TextEdit> {
    edits
        .iter()
        .map(|edit| TextEdit {
            range: document.lsp_range(&(edit.start..edit.end)),
            new_text: edit.new_text.clone(),
        })
        .collect()
}

/// The full-document replacement for a new buffer, or no edit when unchanged.
pub fn whole(document: &Document, text: &str) -> Vec<TextEdit> {
    if text == document.text() {
        return Vec::new();
    }
    vec![TextEdit {
        range: document.lsp_range(&(0..document.text().len())),
        new_text: text.to_owned(),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Position;

    fn document(text: &str) -> Document {
        Document::new(text.into(), "json".into(), 1)
    }

    #[test]
    fn returns_no_edits_for_equal_text() {
        assert!(minimal("a\n", "a\n").is_empty());
    }

    #[test]
    fn only_the_changed_lines_are_replaced() {
        let edits = minimal("a\nb\nc\n", "a\nB\nc\n");
        assert_eq!(edits, vec![ByteEdit::new(2, 4, "B\n".to_owned())]);
        let edits = text_edits(&document("a\nb\nc\n"), &edits);
        assert_eq!(edits[0].range.start, Position::new(1, 0));
        assert_eq!(edits[0].range.end, Position::new(2, 0));
    }

    #[test]
    fn handles_insertions_at_the_end_and_empty_buffers() {
        assert_eq!(
            minimal("", "x\n"),
            vec![ByteEdit::new(0, 0, "x\n".to_owned())]
        );
        assert_eq!(
            minimal("a", "a\n"),
            vec![ByteEdit::new(0, 1, "a\n".to_owned())]
        );
    }

    #[test]
    fn ranges_keep_only_touching_edits() {
        let document = document("a\nb\nc\nd\n");
        let edits = minimal("a\nb\nc\nd\n", "a\nB\nc\nD\n");
        let range = lsp_types::Range::new(Position::new(1, 0), Position::new(2, 0));
        let filtered = filter(&document, edits, &[range]);
        assert_eq!(filtered, vec![ByteEdit::new(2, 4, "B\n".to_owned())]);
    }

    #[test]
    fn a_zero_width_range_touches_an_insertion_at_its_position() {
        let document = document("a\n");
        let edits = minimal("a\n", "a\nb\n");
        let range = lsp_types::Range::new(Position::new(1, 0), Position::new(1, 0));
        assert_eq!(filter(&document, edits, &[range]).len(), 1);
    }
}
