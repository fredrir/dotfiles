use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Debug, Default)]
struct Snapshot {
    text: String,
    cursor: usize,
    anchor: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditMotion {
    Left,
    Right,
    WordLeft,
    WordRight,
    LineStart,
    LineEnd,
    Start,
    End,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditGroup {
    Typing,
    Backspace,
    Delete,
}

/// A UTF-8 editor whose cursor and selection stay on grapheme boundaries.
#[derive(Clone, Debug, Default)]
pub struct EditorBuffer {
    current: Snapshot,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    group: Option<EditGroup>,
    column: Option<usize>,
    killed: String,
}

#[derive(Clone, Debug)]
pub struct EditorRow {
    pub range: Range<usize>,
}

/// Shared wrapping for cursor movement and rendering, measured in terminal cells.
#[derive(Clone, Debug)]
pub struct EditorLayout {
    pub rows: Vec<EditorRow>,
    width: usize,
}

impl EditorLayout {
    pub fn new(text: &str, width: usize) -> Self {
        let width = width.max(2);
        let mut rows = Vec::new();
        let mut start = 0;
        let mut column = 0;
        for (index, grapheme) in text.grapheme_indices(true) {
            if grapheme == "\n" {
                rows.push(EditorRow {
                    range: start..index,
                });
                start = index + 1;
                column = 0;
                continue;
            }
            let length = cell_width(grapheme, column);
            if column > 0 && column + length > width {
                rows.push(EditorRow {
                    range: start..index,
                });
                start = index;
                column = 0;
            }
            column += cell_width(grapheme, column);
        }
        rows.push(EditorRow {
            range: start..text.len(),
        });
        // Reserve a real cursor cell after an exactly full final row.
        if column >= width {
            rows.push(EditorRow {
                range: text.len()..text.len(),
            });
        }
        Self { rows, width }
    }

    pub fn position(&self, text: &str, cursor: usize) -> (usize, usize) {
        let row = self
            .rows
            .iter()
            .rposition(|row| row.range.start <= cursor)
            .unwrap_or(0);
        let range = &self.rows[row].range;
        let column = text[range.start..cursor.min(range.end)]
            .graphemes(true)
            .fold(0, |column, grapheme| column + cell_width(grapheme, column));
        (row, column.min(self.width - 1))
    }

    pub fn index(&self, text: &str, row: usize, column: usize) -> usize {
        let range = &self.rows[row.min(self.rows.len() - 1)].range;
        let mut used = 0;
        for (index, grapheme) in text[range.clone()].grapheme_indices(true) {
            let width = cell_width(grapheme, used);
            if used + width > column {
                return range.start + index;
            }
            used += width;
        }
        range.end
    }
}

pub fn cell_width(grapheme: &str, column: usize) -> usize {
    if grapheme == "\t" {
        4 - column % 4
    } else {
        grapheme.width()
    }
}

impl EditorBuffer {
    pub fn new(text: &str) -> Self {
        let mut editor = Self::default();
        editor.paste(text);
        editor.undo.clear();
        editor
    }
    pub fn as_str(&self) -> &str {
        &self.current.text
    }
    pub fn cursor(&self) -> usize {
        self.current.cursor
    }
    pub fn selection(&self) -> Option<Range<usize>> {
        self.current
            .anchor
            .filter(|anchor| *anchor != self.cursor())
            .map(|anchor| anchor.min(self.cursor())..anchor.max(self.cursor()))
    }
    pub fn insert(&mut self, character: char) {
        if !character.is_control() || matches!(character, '\n' | '\t') {
            self.replace(&character.to_string(), Some(EditGroup::Typing));
        }
    }
    pub fn paste(&mut self, text: &str) {
        let text: String = text
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .chars()
            .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
            .collect();
        if !text.is_empty() {
            self.replace(&text, None);
        }
    }
    fn remember(&mut self, group: Option<EditGroup>) {
        if group.is_none() || self.group != group || self.selection().is_some() {
            self.undo.push(self.current.clone());
            bound_history(&mut self.undo);
        }
        self.redo.clear();
        self.group = group;
        self.column = None;
    }
    fn replace(&mut self, text: &str, group: Option<EditGroup>) {
        self.remember(group);
        let range = self.selection().unwrap_or(self.cursor()..self.cursor());
        self.current.text.replace_range(range.clone(), text);
        self.current.cursor = range.start + text.len();
        self.normalize_cursor();
        self.current.anchor = None;
    }
    fn normalize_cursor(&mut self) {
        self.current.cursor = self
            .current
            .text
            .grapheme_indices(true)
            .map(|(index, _)| index)
            .find(|index| *index >= self.cursor())
            .unwrap_or(self.current.text.len());
    }
    fn previous(&self) -> usize {
        self.as_str()[..self.cursor()]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(index, _)| index)
    }
    fn next(&self) -> usize {
        self.cursor()
            + self.as_str()[self.cursor()..]
                .graphemes(true)
                .next()
                .map_or(0, str::len)
    }
    fn word_left(&self) -> usize {
        let mut boundary = self.cursor();
        let mut word = false;
        for (index, grapheme) in self.as_str()[..self.cursor()].grapheme_indices(true).rev() {
            let space = grapheme.chars().all(char::is_whitespace);
            if word && space {
                break;
            }
            word |= !space;
            boundary = index;
        }
        boundary
    }
    fn word_right(&self) -> usize {
        let mut boundary = self.cursor();
        let mut word = false;
        for (index, grapheme) in self.as_str()[self.cursor()..].grapheme_indices(true) {
            let space = grapheme.chars().all(char::is_whitespace);
            if word && space {
                break;
            }
            word |= !space;
            boundary = self.cursor() + index + grapheme.len();
        }
        boundary
    }
    pub fn move_cursor(&mut self, motion: EditMotion, select: bool, width: usize) {
        use EditMotion::*;
        self.group = None;
        let previous = self.cursor();
        let destination = match motion {
            Left if !select && self.selection().is_some() => self.selection().unwrap().start,
            Right if !select && self.selection().is_some() => self.selection().unwrap().end,
            Left => self.previous(),
            Right => self.next(),
            WordLeft => self.word_left(),
            WordRight => self.word_right(),
            LineStart => self.as_str()[..previous]
                .rfind('\n')
                .map_or(0, |index| index + 1),
            LineEnd => self.as_str()[previous..]
                .find('\n')
                .map_or(self.as_str().len(), |index| previous + index),
            Start => 0,
            End => self.as_str().len(),
            Up | Down => {
                let layout = EditorLayout::new(self.as_str(), width);
                let (row, column) = layout.position(self.as_str(), previous);
                let column = *self.column.get_or_insert(column);
                let row = if motion == Up {
                    row.saturating_sub(1)
                } else {
                    (row + 1).min(layout.rows.len() - 1)
                };
                layout.index(self.as_str(), row, column)
            }
        };
        if !matches!(motion, Up | Down) {
            self.column = None;
        }
        self.current.anchor = if select {
            Some(self.current.anchor.unwrap_or(previous))
        } else {
            None
        };
        self.current.cursor = destination;
    }
    pub fn backspace(&mut self) {
        self.erase(
            self.previous()..self.cursor(),
            Some(EditGroup::Backspace),
            false,
        );
    }
    pub fn delete(&mut self) {
        self.erase(self.cursor()..self.next(), Some(EditGroup::Delete), false);
    }
    pub fn kill(&mut self, motion: EditMotion) {
        let start = self.cursor();
        let selection = self.selection();
        self.move_cursor(motion, false, usize::MAX);
        let end = self.cursor();
        self.current.cursor = start;
        let mut range = selection.unwrap_or(start.min(end)..start.max(end));
        if range.is_empty() && motion == EditMotion::LineEnd {
            range.end = self.next();
        }
        self.current.anchor = None;
        self.erase(range, None, true);
    }
    fn erase(&mut self, range: Range<usize>, group: Option<EditGroup>, kill: bool) {
        let range = self.selection().unwrap_or(range);
        if range.is_empty() {
            return;
        }
        self.remember(group);
        if kill {
            self.killed = self.as_str()[range.clone()].to_owned();
        }
        self.current.text.replace_range(range.clone(), "");
        self.current.cursor = range.start;
        self.normalize_cursor();
        self.current.anchor = None;
    }
    pub fn yank(&mut self) {
        let text = self.killed.clone();
        if !text.is_empty() {
            self.replace(&text, None);
        }
    }
    pub fn undo(&mut self) {
        if let Some(previous) = self.undo.pop() {
            self.redo
                .push(std::mem::replace(&mut self.current, previous));
            self.group = None;
            self.column = None;
        }
    }
    pub fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.current, next));
            self.group = None;
            self.column = None;
        }
    }
}

fn bound_history(history: &mut Vec<Snapshot>) {
    let mut bytes: usize = history.iter().map(|snapshot| snapshot.text.len()).sum();
    while history.len() > 1 && (history.len() > 100 || bytes > 1024 * 1024) {
        bytes -= history.remove(0).text.len();
    }
}
