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
    if start == end && hunk.text.is_empty() {
        return;
    }
    // Adjacent changed lines are one diff hunk. Split every multi-line hunk so
    // a requested range only receives the lines it contains and cannot
    // silently rewrite neighbouring statements.
    let added: Vec<&str> = hunk.text.split_inclusive('\n').collect();
    if hunk.removed > 1 || added.len() > 1 {
        let paired = hunk.removed.min(added.len());
        for (index, line) in added.iter().take(paired).enumerate() {
            let line_start = starts.get(hunk.start + index).copied().unwrap_or(len);
            let line_end = starts.get(hunk.start + index + 1).copied().unwrap_or(len);
            edits.push(ByteEdit::new(line_start, line_end, (*line).to_owned()));
        }
        for index in paired..hunk.removed {
            let line_start = starts.get(hunk.start + index).copied().unwrap_or(len);
            let line_end = starts.get(hunk.start + index + 1).copied().unwrap_or(len);
            edits.push(ByteEdit::new(line_start, line_end, String::new()));
        }
        if added.len() > paired {
            let insert_at = starts
                .get(hunk.start + hunk.removed)
                .copied()
                .unwrap_or(len);
            for line in &added[paired..] {
                edits.push(ByteEdit::new(insert_at, insert_at, (*line).to_owned()));
            }
        }
    } else {
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

/// Keeps the edits fully contained in at least one requested byte range.
///
/// Range formatting must never rewrite bytes outside the selection, so an edit
/// that merely touches a range boundary is discarded until [`minimal`] has
/// split the hunk small enough to fit.
pub fn filter(edits: Vec<ByteEdit>, ranges: &[Range<usize>]) -> Vec<ByteEdit> {
    edits
        .into_iter()
        .filter(|edit| ranges.iter().any(|range| contains(edit, range)))
        .collect()
}

fn contains(edit: &ByteEdit, range: &Range<usize>) -> bool {
    if edit.start == edit.end {
        range.start <= edit.start && edit.start <= range.end
    } else {
        range.start <= edit.start && edit.end <= range.end
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
    fn adjacent_changed_lines_become_separate_edits() {
        let edits = minimal("a\nb\n", "A\nB\n");
        assert_eq!(
            edits,
            vec![
                ByteEdit::new(0, 2, "A\n".to_owned()),
                ByteEdit::new(2, 4, "B\n".to_owned()),
            ]
        );
    }

    #[test]
    fn filtering_a_split_edit_does_not_touch_the_next_statement() {
        let edits = minimal("a\nb\n", "A\nB\n");
        let range = 0usize..2;
        let filtered = filter(edits, std::slice::from_ref(&range));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0], ByteEdit::new(0, 2, "A\n".to_owned()));
    }

    #[test]
    fn every_multiline_replacement_becomes_one_edit_per_line() {
        let edits = minimal("one\ntwo\nthree\n", "ONE\nTWO\nTHREE\n");
        assert_eq!(
            edits,
            vec![
                ByteEdit::new(0, 4, "ONE\n".to_owned()),
                ByteEdit::new(4, 8, "TWO\n".to_owned()),
                ByteEdit::new(8, 14, "THREE\n".to_owned()),
            ]
        );
    }

    #[test]
    fn unequal_multiline_hunks_split_into_line_edits() {
        assert_eq!(
            minimal("one\ntwo\n", "ONE\nTWO\nTHREE\n"),
            vec![
                ByteEdit::new(0, 4, "ONE\n".to_owned()),
                ByteEdit::new(4, 8, "TWO\n".to_owned()),
                ByteEdit::new(8, 8, "THREE\n".to_owned()),
            ]
        );
        assert_eq!(
            minimal("one\ntwo\nthree\n", "ONE\nTWO\n"),
            vec![
                ByteEdit::new(0, 4, "ONE\n".to_owned()),
                ByteEdit::new(4, 8, "TWO\n".to_owned()),
                ByteEdit::new(8, 14, String::new()),
            ]
        );
    }

    #[test]
    fn a_range_inside_a_multiline_hunk_keeps_only_its_own_line() {
        let edits = minimal("one\ntwo\nthree\n", "ONE\nTWO\nTHREE\n");
        let range = 4..8;
        let filtered = filter(edits, std::slice::from_ref(&range));
        assert_eq!(filtered, vec![ByteEdit::new(4, 8, "TWO\n".to_owned())]);
    }

    #[test]
    fn partially_overlapping_edits_are_not_kept() {
        let edits = || vec![ByteEdit::new(4, 8, "TWO\n".to_owned())];
        let partial = 5..7;
        let clipped = 4..7;
        let whole = 4..8;
        assert!(filter(edits(), std::slice::from_ref(&partial)).is_empty());
        assert!(filter(edits(), std::slice::from_ref(&clipped)).is_empty());
        assert_eq!(filter(edits(), std::slice::from_ref(&whole)).len(), 1);
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
        let edits = minimal("a\nb\nc\nd\n", "a\nB\nc\nD\n");
        let range = 2..6;
        let filtered = filter(edits, std::slice::from_ref(&range));
        assert_eq!(filtered, vec![ByteEdit::new(2, 4, "B\n".to_owned())]);
    }

    #[test]
    fn a_zero_width_range_touches_an_insertion_at_its_position() {
        let edits = minimal("a\n", "a\nb\n");
        let range = 2..2;
        assert_eq!(filter(edits, std::slice::from_ref(&range)).len(), 1);
    }
}
