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

pub fn minimal(before: &str, after: &str) -> Vec<ByteEdit> {
    if before == after {
        return Vec::new();
    }
    // `similar`'s line diff is quadratic in the worst case. A multi-megabyte
    // buffer can otherwise block the editor for minutes; one whole-document
    // edit is a correct and cheap fallback (the client applies it atomically).
    if line_count(before).saturating_mul(line_count(after)) > MAX_DIFF_CELLS
        || before.len().saturating_add(after.len()) > MAX_DIFF_BYTES
    {
        return vec![ByteEdit::new(0, before.len(), after.to_owned())];
    }
    let starts = line_starts(before);
    let mut edits = Vec::new();
    for hunk in hunks(before, after) {
        push(&mut edits, &starts, before.len(), hunk);
    }
    edits
}

fn line_count(text: &str) -> usize {
    text.bytes().filter(|byte| *byte == b'\n').count() + 1
}

/// Line hunks of the minimal line diff from `before` to `after`.
///
/// A hunk replaces `removed` lines starting at `start` with `text`; a hunk
/// without removed lines inserts `text` at the start of line `start`.
fn hunks(before: &str, after: &str) -> Vec<Hunk> {
    if before == after {
        return Vec::new();
    }
    let diff = TextDiff::from_lines(before, after);
    let mut hunks = Vec::new();
    let mut old_line = 0;
    let mut hunk: Option<Hunk> = None;

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => {
                if let Some(hunk) = hunk.take() {
                    hunks.push(hunk);
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
        hunks.push(hunk);
    }
    hunks
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

/// Formats only the lines that the requested ranges cover completely.
///
/// The document is not mapped through the line diff, because a diff hunk of
/// two reformatted documents groups lines that have nothing to do with each
/// other once a construct changes its line count. Instead the lines of both
/// documents are aligned by their whitespace stripped content, which keeps
/// constructs together, and only complete alignment steps are applied:
///
/// * a step that keeps the line count replaces whole lines, so its lines can
///   be selected individually;
/// * a step that reflows lines (a different number of lines before and after)
///   is applied as a whole and only when the selection covers all of it, since
///   applying part of a reflow would mix up lines of different constructs.
///
/// The trade-off is that a selection inside a reflowed block is left alone
/// instead of being partly rewritten, which keeps every byte outside the
/// selection untouched.
pub fn ranged(before: &str, after: &str, ranges: &[Range<usize>]) -> Vec<ByteEdit> {
    if before == after {
        return Vec::new();
    }
    let selections: Vec<Range<usize>> = merge_ranges(ranges)
        .into_iter()
        .filter(|range| !range.is_empty())
        .map(|range| expanded_lines(before, range.start, range.end))
        .filter(|range| !range.is_empty())
        .collect();
    if selections.is_empty() {
        return Vec::new();
    }
    let len = before.len();
    let starts = line_starts(before);
    let old_lines = line_texts(before);
    let new_lines = line_texts(after);
    let covered = |span: &Range<usize>| {
        selections
            .iter()
            .any(|selection| covers(selection, span, len))
    };
    let mut edits = Vec::new();
    for step in alignment(before, after) {
        let span = lines_span(&starts, len, &step.old);
        let raw = new_lines[step.new.clone()].concat();
        let new_text = with_line_endings(before, &raw);
        match (step.old.len(), step.new.len()) {
            (0, 0) => {}
            (0, _) => {
                if covered(&span) {
                    edits.push(ByteEdit::new(span.start, span.end, new_text));
                }
            }
            (1, 1) => {
                if old_lines[step.old.start] != new_text && covered(&span) {
                    edits.push(ByteEdit::new(span.start, span.end, new_text));
                }
            }
            (removed, added) if removed == added => {
                for (offset, line) in new_lines[step.new.clone()].iter().enumerate() {
                    let line = with_line_endings(before, line);
                    let span = lines_span(
                        &starts,
                        len,
                        &(step.old.start + offset..step.old.start + offset + 1),
                    );
                    if old_lines[step.old.start + offset] != line && covered(&span) {
                        edits.push(ByteEdit::new(span.start, span.end, line));
                    }
                }
            }
            _ => {
                if covered(&span) {
                    edits.push(ByteEdit::new(span.start, span.end, new_text));
                }
            }
        }
    }
    edits
}

/// Lines of `before` and `after` that correspond to each other.
///
/// A plain line diff cannot be used: once a construct changes its line count,
/// minimal diffs are free to line up blank lines and `}` of unrelated
/// constructs, which splits a construct over several hunks. Aligning on the
/// whitespace stripped content of every line instead keeps constructs
/// together, so a step is either a set of individual lines or one construct.
fn alignment(before: &str, after: &str) -> Vec<Step> {
    let old = keys(before);
    let new = keys(after);
    if old.len().saturating_mul(new.len()) > MAX_ALIGN_CELLS {
        // Too large to align line by line; returning no edit is safe, unlike
        // guessing which formatted lines belong to the selection.
        return Vec::new();
    }
    let mut steps = Vec::new();
    let mut old_line = 0;
    let mut new_line = 0;
    let mut group: Option<Step> = None;
    let flush = |steps: &mut Vec<Step>, group: &mut Option<Step>| {
        if let Some(group) = group.take() {
            steps.push(group);
        }
    };
    for op in pairs(&old, &new) {
        match op {
            Op::Pair {
                old: line,
                new: other,
            } => {
                if old[line] == new[other] {
                    flush(&mut steps, &mut group);
                    steps.push(Step {
                        old: line..line + 1,
                        new: other..other + 1,
                    });
                } else {
                    // A rewritten line stays inside the construct it belongs to.
                    let group = open(&mut group, old_line, new_line);
                    group.old.end = line + 1;
                    group.new.end = other + 1;
                }
                old_line = line + 1;
                new_line = other + 1;
            }
            Op::Old(line) => {
                let group = open(&mut group, old_line, new_line);
                group.old.end = line + 1;
                old_line = line + 1;
            }
            Op::New(other) => {
                let group = open(&mut group, old_line, new_line);
                group.new.end = other + 1;
                new_line = other + 1;
            }
        }
    }
    flush(&mut steps, &mut group);
    trim(steps, &old, &new)
}

/// Splits blank lines that only exist on one side of a step out of it, so a
/// construct is not grouped with the removal of a surrounding blank line.
fn trim(steps: Vec<Step>, old: &[String], new: &[String]) -> Vec<Step> {
    let mut trimmed = Vec::with_capacity(steps.len());
    for step in steps {
        let mut old_lines = step.old;
        let mut new_lines = step.new;
        let mut leading = Vec::new();
        let mut trailing = Vec::new();
        loop {
            let old_blank = old_lines.start < old_lines.end && old[old_lines.start].is_empty();
            let new_blank = new_lines.start < new_lines.end && new[new_lines.start].is_empty();
            if old_blank == new_blank {
                break;
            }
            if old_blank {
                if old_lines.len() <= 1 {
                    break;
                }
                leading.push(Step {
                    old: old_lines.start..old_lines.start + 1,
                    new: new_lines.start..new_lines.start,
                });
                old_lines.start += 1;
            } else {
                if new_lines.len() <= 1 {
                    break;
                }
                leading.push(Step {
                    old: old_lines.start..old_lines.start,
                    new: new_lines.start..new_lines.start + 1,
                });
                new_lines.start += 1;
            }
        }
        loop {
            let mut old_blank = false;
            let mut new_blank = false;
            if old_lines.end > old_lines.start {
                old_blank = old[old_lines.end - 1].is_empty();
            }
            if new_lines.end > new_lines.start {
                new_blank = new[new_lines.end - 1].is_empty();
            }
            if old_blank == new_blank {
                break;
            }
            if old_blank {
                if old_lines.len() <= 1 {
                    break;
                }
                old_lines.end -= 1;
                trailing.push(Step {
                    old: old_lines.end..old_lines.end + 1,
                    new: new_lines.end..new_lines.end,
                });
            } else {
                if new_lines.len() <= 1 {
                    break;
                }
                new_lines.end -= 1;
                trailing.push(Step {
                    old: old_lines.end..old_lines.end,
                    new: new_lines.end..new_lines.end + 1,
                });
            }
        }
        trimmed.extend(leading);
        if !old_lines.is_empty() || !new_lines.is_empty() {
            trimmed.push(Step {
                old: old_lines,
                new: new_lines,
            });
        }
        trimmed.extend(trailing.into_iter().rev());
    }
    trimmed
}

fn open(group: &mut Option<Step>, old_line: usize, new_line: usize) -> &mut Step {
    group.get_or_insert(Step {
        old: old_line..old_line,
        new: new_line..new_line,
    })
}

enum Op {
    Pair { old: usize, new: usize },
    Old(usize),
    New(usize),
}

/// The formatter emits LF, but ranged edits must not mix line endings into a
/// CRLF document.
fn with_line_endings(before: &str, text: &str) -> String {
    if document_uses_crlf(before) && !text.contains('\r') {
        text.replace('\n', "\r\n")
    } else {
        text.to_owned()
    }
}

/// A lone CRLF inside a template literal must not turn every replacement in an
/// otherwise LF document into CRLF.
fn document_uses_crlf(text: &str) -> bool {
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count().saturating_sub(crlf);
    crlf > lf
}

const MAX_ALIGN_CELLS: usize = 16_000_000;
/// Above this many line pairs the minimal line diff is too expensive; emit one
/// whole-document edit instead.
const MAX_DIFF_CELLS: usize = 16_000_000;
/// Also cap by bytes, so a few very long lines cannot make the diff expensive.
const MAX_DIFF_BYTES: usize = 16 * 1024 * 1024;
const GAP: f32 = 0.6;

/// Needleman-Wunsch alignment of the line keys, so a construct that is
/// rewritten with a different number of lines stays one contiguous run.
fn pairs(old: &[String], new: &[String]) -> Vec<Op> {
    let (rows, columns) = (old.len(), new.len());
    let width = columns + 1;
    let mut back = vec![0u8; (rows + 1) * width];
    let mut previous = vec![0.0f32; width];
    let mut current = vec![0.0f32; width];
    for (column, score) in previous.iter_mut().enumerate().skip(1) {
        *score = -(column as f32) * GAP;
        back[column] = 2;
    }
    for row in 1..=rows {
        current[0] = -(row as f32) * GAP;
        back[row * width] = 1;
        for column in 1..=columns {
            let diagonal = previous[column - 1] + similarity(&old[row - 1], &new[column - 1]);
            let up = previous[column] - GAP;
            let left = current[column - 1] - GAP;
            let (best, direction) = if diagonal >= up && diagonal >= left {
                (diagonal, 0)
            } else if left >= up {
                (left, 2)
            } else {
                (up, 1)
            };
            current[column] = best;
            back[row * width + column] = direction;
        }
        std::mem::swap(&mut previous, &mut current);
    }
    let mut ops = Vec::new();
    let (mut row, mut column) = (rows, columns);
    while row > 0 || column > 0 {
        match back[row * width + column] {
            0 => {
                ops.push(Op::Pair {
                    old: row - 1,
                    new: column - 1,
                });
                row -= 1;
                column -= 1;
            }
            1 => {
                ops.push(Op::Old(row - 1));
                row -= 1;
            }
            _ => {
                ops.push(Op::New(column - 1));
                column -= 1;
            }
        }
    }
    ops.reverse();
    ops
}

/// Score for lining up two whitespace stripped lines. Equal lines are the
/// anchors of the alignment, rewritten lines are only lined up when they are
/// clearly the same construct.
fn similarity(old: &str, new: &str) -> f32 {
    if old == new {
        if old.is_empty() {
            // Blank lines may line up, but they must not outweigh a construct.
            return 0.2;
        }
        return 1.0 + (old.chars().count().min(40) as f32) / 40.0;
    }
    let prefix = old
        .chars()
        .zip(new.chars())
        .take_while(|(left, right)| left == right)
        .count();
    if prefix < 3 {
        return -1.4;
    }
    let ratio = (2 * prefix) as f32 / (old.chars().count() + new.chars().count()).max(1) as f32;
    if ratio < 0.4 { -1.4 } else { 0.4 + 0.6 * ratio }
}

#[derive(Clone, Debug)]
struct Step {
    old: Range<usize>,
    new: Range<usize>,
}

fn keys(text: &str) -> Vec<String> {
    line_texts(text)
        .into_iter()
        .map(|line| line.chars().filter(|c| !c.is_whitespace()).collect())
        .collect()
}

/// Lines of `text`, aligned with `line_starts`, so a text that ends with a
/// newline has a trailing empty line.
fn line_texts(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split_inclusive('\n').collect();
    if text.ends_with('\n') {
        lines.push("");
    }
    lines
}

fn lines_span(starts: &[usize], len: usize, lines: &Range<usize>) -> Range<usize> {
    let start = starts.get(lines.start).copied().unwrap_or(len);
    let end = starts.get(lines.end).copied().unwrap_or(len);
    start..end
}

fn covers(selection: &Range<usize>, span: &Range<usize>, len: usize) -> bool {
    if span.is_empty() {
        if span.start < selection.end {
            return span.start >= selection.start;
        }
        // A trailing insertion (e.g. the final newline) belongs to a selection
        // that runs to the end of the document, so selecting everything still
        // yields the full formatting.
        return span.start == selection.end && span.start == len;
    }
    span.start >= selection.start && span.end <= selection.end
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
    let start = if at_line_start(text, start) {
        start
    } else {
        line_start_at(text, start)
    };
    // The line holding `end` counts as selected unless `end` sits on the start
    // of the next line, so a selection never reaches one line too far.
    let end = if end == start || at_line_start(text, end) {
        end
    } else {
        line_end_at(text, end)
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

    /// Mirrors a user selecting whole lines, like the server does for
    /// `textDocument/rangeFormatting` and `rangesFormatting`.
    fn lines(text: &str, first: usize, last: usize) -> Range<usize> {
        let starts = line_starts(text);
        let start = starts.get(first).copied().unwrap_or(text.len());
        let end = starts.get(last).copied().unwrap_or(text.len());
        start..end
    }

    #[test]
    fn huge_documents_fall_back_to_one_whole_document_edit() {
        let before = "const value = 1;\n".repeat(5_000);
        let after = "const value = 2;\n".repeat(5_000);
        let edits = minimal(&before, &after);
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].start, 0);
        assert_eq!(edits[0].end, before.len());
        assert_eq!(edits[0].new_text, after);
        assert_eq!(apply(&before, &edits), after);
    }

    fn apply(text: &str, edits: &[ByteEdit]) -> String {
        let mut output = String::with_capacity(text.len());
        let mut cursor = 0;
        for edit in edits {
            assert!(
                edit.start >= cursor,
                "edits must be sorted and non-overlapping: {} < {cursor}",
                edit.start
            );
            assert!(edit.end >= edit.start, "edit ends before it starts");
            output.push_str(&text[cursor..edit.start]);
            output.push_str(&edit.new_text);
            cursor = edit.end;
        }
        output.push_str(&text[cursor..]);
        output
    }

    /// The lines outside `selected` (exclusive line span) must stay byte equal.
    fn assert_outside_unchanged(before: &str, after: &str, selected: Range<usize>) {
        let old: Vec<&str> = before.split_inclusive('\n').collect();
        let new: Vec<&str> = after.split_inclusive('\n').collect();
        assert!(
            old.len() >= selected.end && new.len() >= selected.end,
            "selection out of bounds"
        );
        assert_eq!(
            old[..selected.start],
            new[..selected.start],
            "lines before the selection changed"
        );
        let tail = old.len() - selected.end;
        assert_eq!(
            old[old.len() - tail..],
            new[new.len() - tail..],
            "lines after the selection changed"
        );
    }

    const REPRO_BEFORE: &str = "// header comment\nconst a   =   {b:1,c:2};\n\nfunction f( x:number ){return x+1}\n\nconst y=[1,2,3,].map((v)=>v*2)\n\n";
    const REPRO_AFTER: &str = "// header comment\nconst a = { b: 1, c: 2 };\n\nfunction f(x: number) {\n  return x + 1;\n}\n\nconst y = [1, 2, 3].map((v) => v * 2);\n";

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
    fn ranged_formats_only_the_selected_function() {
        let selection = lines(REPRO_BEFORE, 2, 4);
        let edits = ranged(REPRO_BEFORE, REPRO_AFTER, std::slice::from_ref(&selection));
        assert!(!edits.is_empty());
        for edit in &edits {
            assert!(
                edit.start >= selection.start && edit.end <= selection.end,
                "edit outside the selection: {edit:?}"
            );
        }
        let output = apply(REPRO_BEFORE, &edits);
        assert_outside_unchanged(REPRO_BEFORE, &output, 2..4);
        assert_eq!(
            output,
            "// header comment\nconst a   =   {b:1,c:2};\n\nfunction f(x: number) {\n  return x + 1;\n}\n\nconst y=[1,2,3,].map((v)=>v*2)\n\n"
        );
        assert_eq!(output.matches("const a").count(), 1);
        assert!(!output.contains("const a = { b: 1, c: 2 };"));
    }

    #[test]
    fn ranged_formats_disjoint_selections_once() {
        let ranges = [lines(REPRO_BEFORE, 0, 2), lines(REPRO_BEFORE, 4, 6)];
        let edits = ranged(REPRO_BEFORE, REPRO_AFTER, &ranges);
        let output = apply(REPRO_BEFORE, &edits);
        assert_eq!(
            output,
            "// header comment\nconst a = { b: 1, c: 2 };\n\nfunction f( x:number ){return x+1}\n\nconst y = [1, 2, 3].map((v) => v * 2);\n\n"
        );
        assert_eq!(output.matches("const a").count(), 1);
        assert_eq!(output.matches("function f").count(), 1);
    }

    #[test]
    fn ranged_deduplicates_overlapping_and_nested_ranges() {
        let ranges = [
            lines(REPRO_BEFORE, 2, 4),
            lines(REPRO_BEFORE, 2, 4),
            lines(REPRO_BEFORE, 1, 4),
        ];
        let edits = ranged(REPRO_BEFORE, REPRO_AFTER, &ranges);
        let output = apply(REPRO_BEFORE, &edits);
        assert_eq!(
            output,
            "// header comment\nconst a = { b: 1, c: 2 };\n\nfunction f(x: number) {\n  return x + 1;\n}\n\nconst y=[1,2,3,].map((v)=>v*2)\n\n"
        );
    }

    #[test]
    fn ranged_formats_a_statement_after_earlier_line_count_changes() {
        let before = "function one(){return 1;}\n\nfunction two(){return 2;}\n";
        let after = "function one() {\n  return 1;\n}\n\nfunction two() {\n  return 2;\n}\n";
        let selection = before.find("function two").unwrap()..before.len();
        let edits = ranged(before, after, std::slice::from_ref(&selection));
        let output = apply(before, &edits);
        assert_eq!(
            output,
            "function one(){return 1;}\n\nfunction two() {\n  return 2;\n}\n"
        );
        assert_outside_unchanged(before, &output, 2..3);
    }

    #[test]
    fn ranged_never_replaces_unselected_statements() {
        let before = "function one(){return 1;}\n\nfunction two(){return 2;}\n";
        let after = "function one() {\n  return 1;\n}\n\nfunction two() {\n  return 2;\n}\n";
        let selection = before.find("function two").unwrap()..before.len() - 1;
        let edits = ranged(before, after, std::slice::from_ref(&selection));
        assert!(!edits.is_empty());
        let output = apply(before, &edits);
        assert!(output.starts_with("function one(){return 1;}\n"));
        assert!(output.contains("function two() {\n  return 2;\n}"));
    }

    #[test]
    fn ranged_skips_a_reflow_hunk_that_is_only_partially_selected() {
        // Reflowing two adjacent statements is one hunk whose line count
        // changes; half of it would mix lines of both statements, so a
        // selection that covers only one line gets no edit at all.
        let before = "function one(){return 1;}\nfunction two(){return 2;}\n";
        let after = "function one() {\n  return 1;\n}\nfunction two() {\n  return 2;\n}\n";
        assert!(ranged(before, after, &[lines(before, 1, 2)]).is_empty());
        let edits = ranged(before, after, &[lines(before, 0, 2)]);
        assert_eq!(apply(before, &edits), after);
    }

    #[test]
    fn ranged_formats_a_single_line_inside_a_multiline_construct() {
        let before = "const config = {\n  name: \"x\",\n  size  :2,\n  parts: [1, 2],\n};\n";
        let after = "const config = {\n  name: \"x\",\n  size: 2,\n  parts: [1, 2],\n};\n";
        let selection = lines(before, 2, 3);
        let edits = ranged(before, after, std::slice::from_ref(&selection));
        assert_eq!(
            edits,
            vec![ByteEdit::new(
                selection.start,
                selection.end,
                "  size: 2,\n".to_owned()
            )]
        );
        let output = apply(before, &edits);
        assert_eq!(output, after);
    }

    #[test]
    fn ranged_formats_a_single_jsx_child_line() {
        let before = "const view = (\n  <div className=\"x\">\n    <span   id=\"a\">hi</span>\n    text\n  </div>\n);\n";
        let after = "const view = (\n  <div className=\"x\">\n    <span id=\"a\">hi</span>\n    text\n  </div>\n);\n";
        let selection = lines(before, 2, 3);
        let edits = ranged(before, after, std::slice::from_ref(&selection));
        let output = apply(before, &edits);
        assert_eq!(output, after);
        assert_outside_unchanged(before, &output, 2..3);
    }

    #[test]
    fn ranged_formats_only_the_selected_line_of_a_reindent() {
        let before = "function a() {\n    const b = 1;\n    return b;\n}\n";
        let after = "function a() {\n  const b = 1;\n  return b;\n}\n";
        // Re-indenting keeps the line count, so a single selected line is a
        // self contained replacement that cannot delete or duplicate lines.
        let selection = lines(before, 1, 2);
        let edits = ranged(before, after, std::slice::from_ref(&selection));
        assert_eq!(
            edits,
            vec![ByteEdit::new(
                selection.start,
                selection.end,
                "  const b = 1;\n".to_owned()
            )]
        );
        let output = apply(before, &edits);
        assert_eq!(output, "function a() {\n  const b = 1;\n    return b;\n}\n");
        assert_outside_unchanged(before, &output, 1..2);
    }

    #[test]
    fn ranged_merges_overlapping_ranges() {
        let edits = ranged("a\nb\nc\n", "A\nB\nC\n", &[0..2, 1..4, 0..2]);
        assert_eq!(apply("a\nb\nc\n", &edits), "A\nB\nc\n");
        assert_eq!(
            edits,
            vec![
                ByteEdit::new(0, 2, "A\n".to_owned()),
                ByteEdit::new(2, 4, "B\n".to_owned()),
            ]
        );
    }

    #[test]
    fn ranged_returns_empty_for_equal_text() {
        assert!(ranged("same\n", "same\n", &[0..5]).is_empty());
    }

    #[test]
    fn ranged_returns_empty_for_a_selection_without_changes() {
        assert!(ranged(REPRO_BEFORE, REPRO_AFTER, &[lines(REPRO_BEFORE, 0, 1)]).is_empty());
    }

    #[test]
    fn ranged_keeps_crlf_line_endings() {
        let before = "const a = 1;\r\nconst b  =2;\r\nconst c = 3;\r\n";
        let after = "const a = 1;\r\nconst b = 2;\r\nconst c = 3;\r\n";
        let selection = lines(before, 1, 2);
        let edits = ranged(before, after, std::slice::from_ref(&selection));
        assert_eq!(
            edits,
            vec![ByteEdit::new(14, 28, "const b = 2;\r\n".to_owned())]
        );
        let output = apply(before, &edits);
        assert_eq!(output, after);
    }

    #[test]
    fn ranged_keeps_crlf_when_the_formatter_returns_lf() {
        let before = "const a = 1;\r\nconst b  =2;\r\nconst c = 3;\r\n";
        let after = "const a = 1;\nconst b = 2;\nconst c = 3;\n";
        let selection = lines(before, 1, 2);
        let edits = ranged(before, after, std::slice::from_ref(&selection));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "const b = 2;\r\n");
        assert_eq!(
            apply(before, &edits),
            "const a = 1;\r\nconst b = 2;\r\nconst c = 3;\r\n"
        );
    }

    #[test]
    fn ranged_keeps_lf_when_crlf_only_appears_inside_a_literal() {
        let before = "a  =1;\nconst t = `x\r\ny`;\n";
        let after = "a = 1;\nconst t = `x\r\ny`;\n";
        let selection = lines(before, 0, 1);
        let edits = ranged(before, after, std::slice::from_ref(&selection));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "a = 1;\n");
    }

    #[test]
    fn ranged_applies_eof_insertions_only_for_selections_reaching_the_end() {
        let before = "a\nb\nc\n";
        let after = "a\nb\nc\n\n";
        assert_eq!(
            ranged(before, after, &[0..before.len()]),
            vec![ByteEdit::new(before.len(), before.len(), "\n".to_owned())]
        );
        assert!(ranged(before, after, &[0..4]).is_empty());
    }

    #[test]
    fn ranged_never_deletes_the_trailing_newline_of_a_selection() {
        let before = "a  =1;\nb  =2;\n";
        let after = "a = 1;\nb = 2;\n";
        let edits = ranged(before, after, &[0..7]);
        assert_eq!(edits, vec![ByteEdit::new(0, 7, "a = 1;\n".to_owned())]);
    }

    #[test]
    fn ranged_does_not_full_format_from_an_empty_range_at_eof() {
        assert!(ranged("a\n", "a\nb\n", &[2..2]).is_empty());
    }

    #[test]
    fn ranged_over_the_whole_document_is_the_full_formatting() {
        let whole = |before: &str| 0..before.len();
        assert_eq!(
            apply(
                REPRO_BEFORE,
                &ranged(REPRO_BEFORE, REPRO_AFTER, &[whole(REPRO_BEFORE)])
            ),
            REPRO_AFTER
        );
        for (before, after) in [
            ("a\nb\nc\n", "a\nb\nc\n\n"),
            ("a\nb", "a\nb\n"),
            ("\n\na  =1;\n\n", "\n\na = 1;\n"),
            ("x()\n", "x()\n"),
        ] {
            let edits = ranged(before, after, &[0..before.len()]);
            assert_eq!(apply(before, &edits), after, "{before:?} -> {after:?}");
            assert_eq!(apply(before, &minimal(before, after)), after);
        }
    }
}
