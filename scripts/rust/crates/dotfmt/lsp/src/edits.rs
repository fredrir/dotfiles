
use std::ops::Range;

use lsp_types::TextEdit;
use similar::{ChangeTag, DiffOp, TextDiff};

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

pub fn ranged(before: &str, after: &str, ranges: &[Range<usize>]) -> Vec<ByteEdit> {
    if before == after || ranges.is_empty() {
        return Vec::new();
    }
    let map = LineMap::new(before, after);
    let mut edits = Vec::new();
    for requested in merge_ranges(ranges) {
        let original = expanded_lines(before, requested.start, requested.end);
        let Some(start) = map.map(original.start) else {
            continue;
        };
        let Some(end) = map.map(original.end) else {
            continue;
        };
        let start_at_line = at_line_start(before, original.start);
        let end_at_line = original.end == before.len() || at_line_start(before, original.end);
        let formatted_start = if start_at_line {
            line_start_at(after, start)
        } else {
            start.min(after.len())
        };
        let formatted_end = if end_at_line {
            line_start_at(after, end)
        } else {
            line_end_at(after, end)
        };
        let formatted_end = formatted_end.max(formatted_start);
        let new_text = after[formatted_start..formatted_end].to_owned();
        if before[original.clone()] == new_text {
            continue;
        }
        edits.push(ByteEdit::new(original.start, original.end, new_text));
    }
    edits
}

const MAX_CHAR_DIFF: usize = 1_048_576;

struct LineMap<'a> {
    before: &'a str,
    after: &'a str,
    old_starts: Vec<usize>,
    new_starts: Vec<usize>,
    ops: Vec<DiffOp>,
}

impl<'a> LineMap<'a> {
    fn new(before: &'a str, after: &'a str) -> Self {
        let diff = TextDiff::from_lines(before, after);
        Self {
            before,
            after,
            old_starts: line_starts(before),
            new_starts: line_starts(after),
            ops: diff.ops().to_vec(),
        }
    }

    fn map(&self, offset: usize) -> Option<usize> {
        let line = line_index(&self.old_starts, offset);
        for op in &self.ops {
            let old = op.old_range();
            let new = op.new_range();
            if line < old.start || line >= old.end {
                continue;
            }
            if let DiffOp::Equal { .. } = op {
                let target_line = new.start + (line - old.start);
                let old_base = self.old_starts[line];
                let new_base = *self.new_starts.get(target_line)?;
                return Some(new_base + offset.saturating_sub(old_base));
            }
            let old_start = self.old_starts[old.start];
            let old_end = self
                .old_starts
                .get(old.end)
                .copied()
                .unwrap_or(self.before.len());
            let new_start = self.new_starts[new.start];
            let new_end = self
                .new_starts
                .get(new.end)
                .copied()
                .unwrap_or(self.after.len());
            let local = offset
                .saturating_sub(old_start)
                .min(old_end.saturating_sub(old_start));
            let map = CharMap::new(
                &self.before[old_start..old_end],
                &self.after[new_start..new_end],
            )?;
            return Some(new_start + map.map(local)?);
        }
        (offset == self.before.len()).then_some(self.after.len())
    }
}

fn line_index(starts: &[usize], offset: usize) -> usize {
    match starts.binary_search(&offset) {
        Ok(index) => index,
        Err(index) => index.saturating_sub(1),
    }
}

struct CharMap {
    offsets: Vec<usize>,
}

impl CharMap {
    fn new(before: &str, after: &str) -> Option<Self> {
        if before.len().max(after.len()) > MAX_CHAR_DIFF {
            return None;
        }
        let diff = TextDiff::from_chars(before, after);
        let mut offsets = vec![0; before.len() + 1];
        let mut old = 0usize;
        let mut new = 0usize;
        for change in diff.iter_all_changes() {
            let text = change.value();
            let len = text.len();
            match change.tag() {
                ChangeTag::Equal => {
                    for index in 0..=len {
                        offsets[old + index] = new + index;
                    }
                    old += len;
                    new += len;
                }
                ChangeTag::Delete => {
                    for index in 0..len {
                        offsets[old + index] = new;
                    }
                    old += len;
                }
                ChangeTag::Insert => {
                    new += len;
                }
            }
        }
        offsets[before.len()] = after.len();
        Some(Self { offsets })
    }

    fn map(&self, offset: usize) -> Option<usize> {
        self.offsets.get(offset).copied()
    }
}

fn merge_ranges(ranges: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = ranges.to_vec();
    ranges.sort_by_key(|range| (range.start, range.end));
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && range.start <= last.end
        {
            last.end = last.end.max(range.end);
            continue;
        }
        merged.push(range);
    }
    merged
}

fn at_line_start(text: &str, offset: usize) -> bool {
    offset == 0 || text.as_bytes().get(offset - 1) == Some(&b'\n')
}

fn line_start_at(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    if at_line_start(text, offset) {
        return offset;
    }
    text[..offset].rfind('\n').map_or(0, |index| index + 1)
}

fn line_end_at(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    text[offset..]
        .find('\n')
        .map_or(text.len(), |index| offset + index + 1)
}

fn expanded_lines(text: &str, start: usize, end: usize) -> Range<usize> {
    let start = start.min(text.len());
    let end = end.min(text.len()).max(start);
    let start = if start == 0 || text.as_bytes().get(start - 1) == Some(&b'\n') {
        start
    } else {
        text[..start].rfind('\n').map_or(0, |offset| offset + 1)
    };
    let end = if end == text.len()
        || text.as_bytes().get(end) == Some(&b'\n')
        || (end > 0 && text.as_bytes().get(end - 1) == Some(&b'\n'))
    {
        end
    } else {
        text[end..]
            .find('\n')
            .map_or(text.len(), |offset| end + offset + 1)
    };
    start..end
}

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
    #![allow(clippy::single_range_in_vec_init)]

    use super::*;

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
    fn ranged_replaces_only_the_selected_line() {
        let edits = ranged("one\ntwo\nthree\n", "ONE\nTWO\nTHREE\n", &[4..8]);
        assert_eq!(edits, vec![ByteEdit::new(4, 8, "TWO\n".to_owned())]);
    }

    #[test]
    fn ranged_maps_a_statement_after_earlier_line_count_changes() {
        let before = "function one(){return 1;}\nfunction two(){return 2;}\n";
        let after = "function one() {\n  return 1;\n}\nfunction two() {\n  return 2;\n}\n";
        let start = before.find("function two").unwrap();
        let end = before.len();
        let edits = ranged(before, after, &[start..end]);
        assert_eq!(edits.len(), 1);
        assert_eq!(
            &before[edits[0].start..edits[0].end],
            "function two(){return 2;}\n"
        );
        assert_eq!(edits[0].new_text, "function two() {\n  return 2;\n}\n");
    }

    #[test]
    fn ranged_never_replaces_unselected_statements() {
        let before = "function one(){return 1;}\nfunction two(){return 2;}\n";
        let after = "function one() {\n  return 1;\n}\nfunction two() {\n  return 2;\n}\n";
        let start = before.find("function two").unwrap();
        let edits = ranged(before, after, &[start..before.len() - 1]);
        let mut output = before.to_owned();
        for edit in edits.into_iter().rev() {
            output.replace_range(edit.start..edit.end, &edit.new_text);
        }
        assert!(output.starts_with("function one(){return 1;}\n"));
        assert!(output.contains("function two() {\n  return 2;\n}"));
    }

    #[test]
    fn ranged_merges_overlapping_ranges() {
        let edits = ranged("a\nb\nc\n", "A\nB\nC\n", &[0..2, 1..4]);
        assert_eq!(edits, vec![ByteEdit::new(0, 4, "A\nB\n".to_owned())]);
    }

    #[test]
    fn ranged_returns_empty_for_equal_text() {
        assert!(ranged("same\n", "same\n", &[0..5]).is_empty());
    }

    #[test]
    fn ranged_does_not_full_format_from_an_empty_range_at_eof() {
        assert!(ranged("a\n", "a\nb\n", &[2..2]).is_empty());
    }
}
