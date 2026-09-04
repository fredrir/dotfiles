use crate::{
    AcceptTarget, Directory, DirectoryStatus, Entry, EntryKind, InputKind, Selection,
    SelectionPolicy,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PromptEdit {
    Insert(char),
    Backspace,
    Kill,
    WordBack,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Prompt {
    text: String,
    kind: InputKind,
}

pub(crate) struct State<L> {
    directory: Directory<L>,
    selection_policy: SelectionPolicy,
    rows: Vec<usize>,
    cursor: usize,
    offset: usize,
    focus_anchor: Option<L>,
    prompt: Option<Prompt>,
    error: Option<String>,
    #[cfg(test)]
    filter_rebuilds: usize,
}

impl<L: Clone + Eq> State<L> {
    pub(crate) fn new(directory: Directory<L>, selection_policy: SelectionPolicy) -> Self {
        let rows = (0..directory.entries.len()).collect();
        let focus_anchor = directory
            .entries
            .first()
            .map(|entry| entry.location.clone());
        Self {
            directory,
            selection_policy,
            rows,
            cursor: 0,
            offset: 0,
            focus_anchor,
            prompt: None,
            error: None,
            #[cfg(test)]
            filter_rebuilds: 1,
        }
    }

    pub(crate) fn directory(&self) -> &Directory<L> {
        &self.directory
    }

    pub(crate) fn rows(&self) -> &[usize] {
        &self.rows
    }

    pub(crate) fn cursor(&self) -> usize {
        self.cursor
    }

    pub(crate) fn offset(&self) -> usize {
        self.offset
    }

    pub(crate) fn prompt(&self) -> Option<&str> {
        self.prompt.as_ref().map(|prompt| prompt.text.as_str())
    }

    pub(crate) fn input_kind(&self) -> Option<InputKind> {
        self.prompt.as_ref().map(|prompt| prompt.kind)
    }

    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub(crate) fn focused(&self) -> Option<&Entry<L>> {
        self.rows
            .get(self.cursor)
            .and_then(|index| self.directory.entries.get(*index))
    }

    pub(crate) fn selection(&self) -> Option<Selection<L>> {
        match self.selection_policy.accept_target {
            AcceptTarget::CurrentDirectory => self.current_directory_selection(),
            AcceptTarget::HighlightedEntry => {
                if self.search_has_no_matches() || self.directory.status != DirectoryStatus::Present
                {
                    return None;
                }
                self.focused()
                    .filter(|entry| self.selection_policy.permits(entry.kind))
                    .map(entry_selection)
            }
            AcceptTarget::HighlightedEntryOrCurrentDirectory => {
                if self.search_has_no_matches() || self.directory.status != DirectoryStatus::Present
                {
                    return None;
                }
                self.focused()
                    .filter(|entry| self.selection_policy.permits(entry.kind))
                    .map(entry_selection)
                    .or_else(|| {
                        self.directory
                            .entries
                            .is_empty()
                            .then(|| self.present_directory_selection())
                    })
            }
        }
    }

    pub(crate) fn begin_prompt(&mut self, kind: InputKind) {
        self.set_prompt(String::new(), kind);
    }

    pub(crate) fn set_prompt(&mut self, text: String, kind: InputKind) {
        let prompt = Prompt { text, kind };
        if self.prompt.as_ref() == Some(&prompt) {
            return;
        }
        self.prompt = Some(prompt);
        self.rebuild_rows();
        self.error = None;
    }

    pub(crate) fn edit_prompt(
        &mut self,
        edit: PromptEdit,
        classify: impl FnOnce(&str) -> InputKind,
    ) {
        let Some(prompt) = &self.prompt else {
            return;
        };
        let mut text = prompt.text.clone();
        match edit {
            PromptEdit::Insert(character) => text.push(character),
            PromptEdit::Backspace => {
                text.pop();
            }
            PromptEdit::Kill => text.clear(),
            PromptEdit::WordBack => remove_last_word(&mut text),
        }
        let kind = classify(&text);
        self.set_prompt(text, kind);
    }

    pub(crate) fn cancel_prompt(&mut self) {
        if self.prompt.is_none() {
            return;
        }
        self.prompt = None;
        self.rebuild_rows();
        self.error = None;
    }

    pub(crate) fn set_error(&mut self, error: impl Into<String>) {
        self.error = Some(error.into());
    }

    pub(crate) fn move_by(&mut self, amount: isize) {
        if amount.is_negative() {
            self.cursor = self.cursor.saturating_sub(amount.unsigned_abs());
        } else {
            self.cursor = self
                .cursor
                .saturating_add(amount as usize)
                .min(self.rows.len().saturating_sub(1));
        }
        self.remember_focus();
        self.error = None;
    }

    pub(crate) fn page_by(&mut self, pages: isize, viewport_rows: usize) {
        let distance = pages.unsigned_abs().saturating_mul(viewport_rows.max(1));
        let amount = if pages.is_negative() {
            isize::try_from(distance)
                .unwrap_or(isize::MAX)
                .saturating_neg()
        } else {
            isize::try_from(distance).unwrap_or(isize::MAX)
        };
        self.move_by(amount);
    }

    pub(crate) fn first(&mut self) {
        self.cursor = 0;
        self.remember_focus();
        self.error = None;
    }

    pub(crate) fn last(&mut self) {
        self.cursor = self.rows.len().saturating_sub(1);
        self.remember_focus();
        self.error = None;
    }

    pub(crate) fn focus_location(&mut self, location: &L) -> bool {
        let Some(cursor) = self
            .rows
            .iter()
            .position(|index| self.directory.entries[*index].location == *location)
        else {
            return false;
        };
        self.cursor = cursor;
        self.focus_anchor = Some(location.clone());
        true
    }

    pub(crate) fn replace_directory(&mut self, directory: Directory<L>, restore_focus: Option<&L>) {
        self.directory = directory;
        self.prompt = None;
        self.error = None;
        self.cursor = 0;
        self.offset = 0;
        self.focus_anchor = restore_focus.cloned().or_else(|| {
            self.directory
                .entries
                .first()
                .map(|entry| entry.location.clone())
        });
        self.rebuild_rows();
    }

    pub(crate) fn settle(&mut self, viewport_rows: usize) -> bool {
        let previous = self.offset;
        if self.rows.is_empty() {
            self.cursor = 0;
            self.offset = 0;
            return previous != self.offset;
        }

        self.cursor = self.cursor.min(self.rows.len() - 1);
        let viewport_rows = viewport_rows.max(1);
        if self.cursor < self.offset {
            self.offset = self.cursor;
        } else if self.cursor >= self.offset.saturating_add(viewport_rows) {
            self.offset = self.cursor + 1 - viewport_rows;
        }
        let maximum = self.rows.len().saturating_sub(viewport_rows);
        self.offset = self.offset.min(maximum);
        previous != self.offset
    }

    fn current_directory_selection(&self) -> Option<Selection<L>> {
        match self.directory.status {
            DirectoryStatus::Present => Some(self.directory_selection()),
            DirectoryStatus::Missing if self.selection_policy.allow_missing_directory => {
                Some(self.directory_selection())
            }
            DirectoryStatus::Missing => None,
            DirectoryStatus::Unreadable(_) => None,
        }
    }

    fn present_directory_selection(&self) -> Selection<L> {
        self.directory_selection()
    }

    fn directory_selection(&self) -> Selection<L> {
        Selection {
            location: self.directory.location.clone(),
            kind: EntryKind::Directory,
            label: self.directory.label.clone(),
        }
    }

    fn search_has_no_matches(&self) -> bool {
        matches!(
            &self.prompt,
            Some(Prompt {
                text,
                kind: InputKind::Search,
            }) if !text.is_empty() && self.rows.is_empty()
        )
    }

    fn remember_focus(&mut self) {
        if let Some(location) = self.focused().map(|entry| entry.location.clone()) {
            self.focus_anchor = Some(location);
        }
    }

    fn rebuild_rows(&mut self) {
        self.rows.clear();
        match &self.prompt {
            Some(Prompt {
                text,
                kind: InputKind::Search,
            }) if !text.is_empty() => {
                let needle = text.to_lowercase();
                self.rows.extend(
                    self.directory
                        .entries
                        .iter()
                        .enumerate()
                        .filter(|(_, entry)| entry.name.to_lowercase().contains(&needle))
                        .map(|(index, _)| index),
                );
            }
            _ => self.rows.extend(0..self.directory.entries.len()),
        }
        self.cursor = self
            .focus_anchor
            .as_ref()
            .and_then(|location| {
                self.rows
                    .iter()
                    .position(|index| self.directory.entries[*index].location == *location)
            })
            .unwrap_or_else(|| self.cursor.min(self.rows.len().saturating_sub(1)));
        self.offset = self.offset.min(self.rows.len().saturating_sub(1));
        #[cfg(test)]
        {
            self.filter_rebuilds += 1;
        }
    }
}

fn entry_selection<L: Clone>(entry: &Entry<L>) -> Selection<L> {
    Selection {
        location: entry.location.clone(),
        kind: entry.kind,
        label: entry.name.clone(),
    }
}

fn remove_last_word(text: &mut String) {
    let trimmed = text.trim_end_matches(char::is_whitespace).len();
    text.truncate(trimmed);
    if text.is_empty() {
        return;
    }
    if let Some((index, character)) = text
        .char_indices()
        .rev()
        .find(|(_, character)| *character == '/' || character.is_whitespace())
    {
        let keep = match character {
            '/' if index == 0 => character.len_utf8(),
            '/' => index,
            _ => index + character.len_utf8(),
        };
        text.truncate(keep);
    } else {
        text.clear();
    }
}

#[cfg(test)]
#[path = "../tests/unit/state_tests.rs"]
mod tests;
