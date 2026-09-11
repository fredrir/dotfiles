use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use similar::{Algorithm, ChangeTag, TextDiff};
use ui_theme::{ColorMode, Palette, Role};
use unicode_width::UnicodeWidthChar;

const MAX_BYTES: usize = 256 * 1024;
const MAX_LINES: usize = 4096;
const MAX_LINE_COLUMNS: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Context,
    Removed,
    Added,
}

#[derive(Clone, Debug)]
pub struct DiffLine {
    pub kind: Kind,
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
    pub text: String,
    pub missing_newline: bool,
}

#[derive(Clone, Debug, Default)]
struct Pair {
    left: Option<usize>,
    right: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct DiffDocument {
    pub lines: Vec<DiffLine>,
    pub truncated: bool,
    pub binary: bool,
    pub added: usize,
    pub removed: usize,
    pub left_bytes: usize,
    pub right_bytes: usize,
    pairs: Vec<Pair>,
    unified_hunks: Vec<usize>,
    paired_hunks: Vec<usize>,
    max_columns: usize,
}

impl DiffDocument {
    pub fn new(left: &str, right: &str) -> Self {
        let binary = left.as_bytes().contains(&0) || right.as_bytes().contains(&0);
        let (left_preview, left_cut) = bounded(left);
        let (right_preview, right_cut) = bounded(right);
        let mut document = Self {
            lines: Vec::new(),
            truncated: left_cut || right_cut,
            binary,
            added: 0,
            removed: 0,
            left_bytes: left.len(),
            right_bytes: right.len(),
            pairs: Vec::new(),
            unified_hunks: Vec::new(),
            paired_hunks: Vec::new(),
            max_columns: 0,
        };
        if binary {
            return document;
        }
        let diff = TextDiff::configure()
            .algorithm(Algorithm::Myers)
            .timeout(Duration::from_millis(40))
            .diff_lines(left_preview, right_preview);
        for change in diff.iter_all_changes() {
            let kind = match change.tag() {
                ChangeTag::Equal => Kind::Context,
                ChangeTag::Delete => {
                    document.removed += 1;
                    Kind::Removed
                }
                ChangeTag::Insert => {
                    document.added += 1;
                    Kind::Added
                }
            };
            let (text, columns, cut) = clean_line(change.value());
            document.truncated |= cut;
            document.max_columns = document.max_columns.max(columns);
            document.lines.push(DiffLine {
                kind,
                old_line: change.old_index().map(|index| index + 1),
                new_line: change.new_index().map(|index| index + 1),
                text,
                missing_newline: !change.value().ends_with('\n')
                    && left_preview.ends_with('\n') != right_preview.ends_with('\n'),
            });
        }
        document.build_pairs();
        document.unified_hunks =
            hunk_starts(document.lines.iter().map(|line| line.kind != Kind::Context));
        document.paired_hunks = hunk_starts(document.pairs.iter().map(|pair| {
            pair.left
                .or(pair.right)
                .is_some_and(|index| document.lines[index].kind != Kind::Context)
        }));
        document
    }

    fn build_pairs(&mut self) {
        let mut index = 0;
        while index < self.lines.len() {
            if self.lines[index].kind == Kind::Context {
                self.pairs.push(Pair {
                    left: Some(index),
                    right: Some(index),
                });
                index += 1;
                continue;
            }
            let mut removed = Vec::new();
            let mut added = Vec::new();
            while index < self.lines.len() && self.lines[index].kind != Kind::Context {
                match self.lines[index].kind {
                    Kind::Removed => removed.push(index),
                    Kind::Added => added.push(index),
                    Kind::Context => unreachable!(),
                }
                index += 1;
            }
            self.pairs
                .extend((0..removed.len().max(added.len())).map(|row| Pair {
                    left: removed.get(row).copied(),
                    right: added.get(row).copied(),
                }));
        }
    }

    pub fn row_count(&self, mode: ViewMode) -> usize {
        match mode {
            ViewMode::Unified => self.lines.len(),
            ViewMode::SideBySide => self.pairs.len(),
        }
    }

    pub fn hunks(&self, mode: ViewMode) -> &[usize] {
        match mode {
            ViewMode::Unified => &self.unified_hunks,
            ViewMode::SideBySide => &self.paired_hunks,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ViewMode {
    #[default]
    Unified,
    SideBySide,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Left,
    Right,
    PreviousHunk,
    NextHunk,
    ToggleMode,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ViewState {
    pub mode: ViewMode,
    pub offset: usize,
    pub horizontal: usize,
    layout: ViewMode,
    narrow: bool,
}

impl ViewState {
    pub fn new(mode: ViewMode) -> Self {
        Self {
            mode,
            layout: mode,
            ..Self::default()
        }
    }

    pub fn effective_mode(&self) -> ViewMode {
        self.layout
    }

    /// Call on resize and before input/render; adapts rows without changing the requested mode.
    pub fn fit_width(&mut self, document: &DiffDocument, width: u16, height: u16) {
        self.narrow = width < 44;
        self.settle(document, height);
    }

    fn settle(&mut self, document: &DiffDocument, height: u16) {
        let layout = if self.narrow {
            ViewMode::Unified
        } else {
            self.mode
        };
        if self.layout != layout {
            let line = match self.layout {
                ViewMode::Unified => self.offset,
                ViewMode::SideBySide => document
                    .pairs
                    .get(self.offset)
                    .and_then(|pair| pair.left.or(pair.right))
                    .unwrap_or(0),
            };
            self.offset = match layout {
                ViewMode::Unified => line,
                ViewMode::SideBySide => document
                    .pairs
                    .iter()
                    .position(|pair| pair.left == Some(line) || pair.right == Some(line))
                    .unwrap_or(0),
            };
            self.layout = layout;
        }
        let page = usize::from(height.saturating_sub(2)).max(1);
        self.offset = self
            .offset
            .min(document.row_count(layout).saturating_sub(page));
    }

    pub fn apply(&mut self, action: Action, document: &DiffDocument, height: u16) {
        self.settle(document, height);
        let page = usize::from(height.saturating_sub(2)).max(1);
        let last = document.row_count(self.layout).saturating_sub(page);
        match action {
            Action::Up => self.offset = self.offset.saturating_sub(1),
            Action::Down => self.offset = self.offset.saturating_add(1).min(last),
            Action::PageUp => self.offset = self.offset.saturating_sub(page),
            Action::PageDown => self.offset = self.offset.saturating_add(page).min(last),
            Action::Home => self.offset = 0,
            Action::End => self.offset = last,
            Action::Left => self.horizontal = self.horizontal.saturating_sub(8),
            Action::Right => {
                self.horizontal = self
                    .horizontal
                    .saturating_add(8)
                    .min(document.max_columns.saturating_sub(1))
            }
            Action::PreviousHunk => {
                self.offset = document
                    .hunks(self.layout)
                    .iter()
                    .copied()
                    .rev()
                    .find(|row| *row < self.offset)
                    .unwrap_or(0)
                    .min(last);
            }
            Action::NextHunk => {
                if let Some(row) = document
                    .hunks(self.layout)
                    .iter()
                    .copied()
                    .find(|row| *row > self.offset)
                {
                    self.offset = row.min(last);
                }
            }
            Action::ToggleMode => {
                self.mode = match self.mode {
                    ViewMode::Unified => ViewMode::SideBySide,
                    ViewMode::SideBySide => ViewMode::Unified,
                };
                self.settle(document, height);
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent, document: &DiffDocument, height: u16) -> bool {
        if key.kind == KeyEventKind::Release {
            return false;
        }
        let action = match key.code {
            KeyCode::Up | KeyCode::Char('k') => Action::Up,
            KeyCode::Down | KeyCode::Char('j') => Action::Down,
            KeyCode::PageUp => Action::PageUp,
            KeyCode::PageDown => Action::PageDown,
            KeyCode::Home | KeyCode::Char('g') => Action::Home,
            KeyCode::End | KeyCode::Char('G') => Action::End,
            KeyCode::Left | KeyCode::Char('h') => Action::Left,
            KeyCode::Right | KeyCode::Char('l') => Action::Right,
            KeyCode::Char('[') => Action::PreviousHunk,
            KeyCode::Char(']') => Action::NextHunk,
            KeyCode::Char('v') => Action::ToggleMode,
            _ => return false,
        };
        self.apply(action, document, height);
        true
    }
}

pub struct DiffView<'a> {
    pub document: &'a DiffDocument,
    pub state: &'a ViewState,
    pub palette: &'a Palette,
    pub color: bool,
    pub left_label: &'a str,
    pub right_label: &'a str,
}

impl Widget for DiffView<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        let mode = if self.color {
            ColorMode::Always
        } else {
            ColorMode::Never
        };
        let style = |role| self.palette.ratatui(mode, true, role);
        let (left_label, _, _) = clean_line(self.left_label);
        let (right_label, _, _) = clean_line(self.right_label);
        if self.document.binary {
            Paragraph::new(format!(
                "Binary values · {}: {} bytes · {}: {} bytes",
                left_label, self.document.left_bytes, right_label, self.document.right_bytes
            ))
            .style(style(Role::Warning))
            .render(area, buffer);
            return;
        }
        let mut state = self.state.clone();
        state.fit_width(self.document, area.width, area.height);
        let display_mode = state.effective_mode();
        let offset = state.offset;
        let heading = format!(
            "{} → {}  +{} −{}  {}",
            left_label,
            right_label,
            self.document.added,
            self.document.removed,
            match display_mode {
                ViewMode::Unified => "unified",
                ViewMode::SideBySide => "side by side",
            },
        );
        Paragraph::new(heading)
            .style(style(Role::DiffHeader))
            .render(Rect::new(area.x, area.y, area.width, 1), buffer);
        let content = Rect::new(
            area.x,
            area.y.saturating_add(1),
            area.width,
            area.height.saturating_sub(2),
        );
        if self.document.lines.is_empty() {
            Paragraph::new("Both values are empty")
                .style(style(Role::Muted))
                .render(content, buffer);
        } else {
            match display_mode {
                ViewMode::Unified => {
                    let lines = self
                        .document
                        .lines
                        .iter()
                        .skip(offset)
                        .take(usize::from(content.height))
                        .map(|line| {
                            let marker = match line.kind {
                                Kind::Context => ' ',
                                Kind::Removed => '−',
                                Kind::Added => '+',
                            };
                            let old = line
                                .old_line
                                .map(|value| value.to_string())
                                .unwrap_or_default();
                            let new = line
                                .new_line
                                .map(|value| value.to_string())
                                .unwrap_or_default();
                            Line::from(vec![
                                Span::styled(
                                    format!("{old:>4} {new:>4} {marker} "),
                                    style(role(line.kind)),
                                ),
                                Span::styled(
                                    window(
                                        &line_text(line),
                                        self.state.horizontal,
                                        usize::from(content.width.saturating_sub(12)),
                                    ),
                                    style(role(line.kind)),
                                ),
                            ])
                        })
                        .collect::<Vec<_>>();
                    Paragraph::new(lines).render(content, buffer);
                }
                ViewMode::SideBySide => {
                    let half = content.width.saturating_sub(1) / 2;
                    for (row, pair) in self
                        .document
                        .pairs
                        .iter()
                        .skip(offset)
                        .take(usize::from(content.height))
                        .enumerate()
                    {
                        let y = content.y + row as u16;
                        for (index, x, width, left) in [
                            (pair.left, content.x, half, true),
                            (
                                pair.right,
                                content.x + half + 1,
                                content.width - half - 1,
                                false,
                            ),
                        ] {
                            if let Some(index) = index {
                                let line = &self.document.lines[index];
                                let number = if left { line.old_line } else { line.new_line }
                                    .map(|value| value.to_string())
                                    .unwrap_or_default();
                                let marker = match line.kind {
                                    Kind::Context => ' ',
                                    Kind::Removed => '−',
                                    Kind::Added => '+',
                                };
                                let text = format!(
                                    "{number:>4} {marker} {}",
                                    window(
                                        &line_text(line),
                                        self.state.horizontal,
                                        usize::from(width.saturating_sub(7))
                                    )
                                );
                                Paragraph::new(text)
                                    .style(style(role(line.kind)))
                                    .render(Rect::new(x, y, width, 1), buffer);
                            }
                        }
                        Paragraph::new("│")
                            .style(style(Role::Border))
                            .render(Rect::new(content.x + half, y, 1, 1), buffer);
                    }
                }
            }
        }
        if area.height > 1 {
            let footer = if self.document.truncated {
                "Preview truncated · choices apply to the full value"
            } else {
                "j/k scroll · PgUp/PgDn page · [/] change · v layout"
            };
            Paragraph::new(footer)
                .style(style(if self.document.truncated {
                    Role::Warning
                } else {
                    Role::Muted
                }))
                .render(Rect::new(area.x, area.bottom() - 1, area.width, 1), buffer);
        }
    }
}

fn bounded(value: &str) -> (&str, bool) {
    let mut end = value.len().min(MAX_BYTES);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    if let Some((index, _)) = value[..end].match_indices('\n').nth(MAX_LINES - 1) {
        end = index + 1;
    }
    (&value[..end], end < value.len())
}

fn clean_line(value: &str) -> (String, usize, bool) {
    let mut text = String::new();
    let mut width = 0;
    let mut truncated = false;
    for character in value.trim_end_matches(['\r', '\n']).chars() {
        let character = match character {
            '\t' => {
                let spaces = 4 - width % 4;
                if width + spaces > MAX_LINE_COLUMNS {
                    truncated = true;
                    break;
                }
                text.push_str(&" ".repeat(spaces));
                width += spaces;
                continue;
            }
            character if character.is_control() => '�',
            character => character,
        };
        let columns = character.width().unwrap_or(0);
        if width + columns > MAX_LINE_COLUMNS {
            truncated = true;
            break;
        }
        text.push(character);
        width += columns;
    }
    (text, width, truncated)
}

fn role(kind: Kind) -> Role {
    match kind {
        Kind::Context => Role::DiffContext,
        Kind::Removed => Role::DiffRemoved,
        Kind::Added => Role::DiffAdded,
    }
}

fn line_text(line: &DiffLine) -> std::borrow::Cow<'_, str> {
    if line.missing_newline {
        format!("{}  [no final newline]", line.text).into()
    } else {
        line.text.as_str().into()
    }
}

fn hunk_starts(changed: impl Iterator<Item = bool>) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut previous = None;
    for (index, changed) in changed.enumerate() {
        if changed {
            if previous.is_none_or(|last| index > last + 6) {
                starts.push(index.saturating_sub(2));
            }
            previous = Some(index);
        }
    }
    starts
}

fn window(text: &str, offset: usize, width: usize) -> String {
    let mut result = String::new();
    let mut column = 0;
    let end = offset.saturating_add(width);
    for character in text.chars() {
        let columns = character.width().unwrap_or(0);
        if column + columns > end {
            break;
        }
        if column >= offset {
            result.push(character);
        }
        column += columns;
    }
    result
}
