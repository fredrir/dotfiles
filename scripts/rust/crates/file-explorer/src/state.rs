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
mod tests {
    use super::*;

    fn entry(name: &str, kind: EntryKind) -> Entry<String> {
        Entry {
            location: format!("/root/{name}"),
            name: name.to_string(),
            kind,
        }
    }

    fn directory(status: DirectoryStatus) -> Directory<String> {
        Directory {
            location: "/root".to_string(),
            parent: Some("/".to_string()),
            label: "/root".to_string(),
            entries: vec![
                entry("Alpha", EntryKind::Directory),
                entry("beta.txt", EntryKind::File),
                entry("gamma", EntryKind::Other),
                entry("omega", EntryKind::Directory),
            ],
            status,
        }
    }

    fn state(target: AcceptTarget) -> State<String> {
        State::new(
            directory(DirectoryStatus::Present),
            SelectionPolicy::new(target),
        )
    }

    #[test]
    fn starts_with_all_rows_and_first_entry_focused() {
        let state = state(AcceptTarget::HighlightedEntry);
        assert_eq!(state.rows(), &[0, 1, 2, 3]);
        assert_eq!(state.cursor(), 0);
        assert_eq!(state.offset(), 0);
        assert_eq!(
            state.focused().map(|entry| entry.name.as_str()),
            Some("Alpha")
        );
    }

    #[test]
    fn filters_case_insensitively_and_preserves_entry_indexes() {
        let mut state = state(AcceptTarget::HighlightedEntry);
        state.set_prompt("A".to_string(), InputKind::Search);
        assert_eq!(state.rows(), &[0, 1, 2, 3]);
        state.set_prompt("AM".to_string(), InputKind::Search);
        assert_eq!(state.rows(), &[2]);
        assert_eq!(
            state.focused().map(|entry| entry.name.as_str()),
            Some("gamma")
        );
    }

    #[test]
    fn location_input_never_filters_entries() {
        let mut state = state(AcceptTarget::HighlightedEntry);
        state.set_prompt("/nowhere".to_string(), InputKind::Location);
        assert_eq!(state.rows(), &[0, 1, 2, 3]);
        assert_eq!(state.input_kind(), Some(InputKind::Location));
    }

    #[test]
    fn row_cache_changes_only_when_prompt_or_directory_changes() {
        let mut state = state(AcceptTarget::HighlightedEntry);
        assert_eq!(state.filter_rebuilds, 1);
        state.move_by(2);
        state.settle(2);
        assert_eq!(state.filter_rebuilds, 1);
        state.set_prompt("a".to_string(), InputKind::Search);
        assert_eq!(state.filter_rebuilds, 2);
        state.set_prompt("a".to_string(), InputKind::Search);
        assert_eq!(state.filter_rebuilds, 2);
        state.set_prompt("a".to_string(), InputKind::Location);
        assert_eq!(state.filter_rebuilds, 3);
        state.replace_directory(directory(DirectoryStatus::Present), None);
        assert_eq!(state.filter_rebuilds, 4);
    }

    #[test]
    fn changing_and_cancelling_filter_preserves_focus_by_location() {
        let mut state = state(AcceptTarget::HighlightedEntry);
        state.move_by(3);
        state.set_prompt("meg".to_string(), InputKind::Search);
        assert_eq!(
            state.focused().map(|entry| entry.name.as_str()),
            Some("omega")
        );
        assert_eq!(state.cursor(), 0);
        state.cancel_prompt();
        assert_eq!(
            state.focused().map(|entry| entry.name.as_str()),
            Some("omega")
        );
        assert_eq!(state.cursor(), 3);
    }

    #[test]
    fn focus_identity_survives_a_temporary_filter_with_no_matches() {
        let mut state = state(AcceptTarget::HighlightedEntry);
        state.move_by(3);
        state.set_prompt("absent".to_string(), InputKind::Search);
        assert!(state.focused().is_none());
        state.cancel_prompt();
        assert_eq!(
            state.focused().map(|entry| entry.name.as_str()),
            Some("omega")
        );
    }

    #[test]
    fn focus_falls_back_to_valid_row_when_location_does_not_match() {
        let mut state = state(AcceptTarget::HighlightedEntry);
        state.move_by(3);
        state.set_prompt("beta".to_string(), InputKind::Search);
        assert_eq!(state.cursor(), 0);
        assert_eq!(
            state.focused().map(|entry| entry.name.as_str()),
            Some("beta.txt")
        );
    }

    #[test]
    fn movement_clamps_at_both_ends() {
        let mut state = state(AcceptTarget::HighlightedEntry);
        state.move_by(-20);
        assert_eq!(state.cursor(), 0);
        state.move_by(20);
        assert_eq!(state.cursor(), 3);
        state.move_by(isize::MIN);
        assert_eq!(state.cursor(), 0);
    }

    #[test]
    fn paging_uses_the_current_viewport_height() {
        let mut state = state(AcceptTarget::HighlightedEntry);
        state.page_by(1, 3);
        assert_eq!(state.cursor(), 3);
        state.page_by(-1, 2);
        assert_eq!(state.cursor(), 1);
        state.page_by(-1, 0);
        assert_eq!(state.cursor(), 0);
    }

    #[test]
    fn first_last_and_settle_keep_focus_visible() {
        let mut state = state(AcceptTarget::HighlightedEntry);
        state.last();
        assert!(state.settle(2));
        assert_eq!(state.offset(), 2);
        state.first();
        assert!(state.settle(2));
        assert_eq!(state.offset(), 0);
        assert!(!state.settle(2));
    }

    #[test]
    fn zero_height_viewport_still_settles_safely() {
        let mut state = state(AcceptTarget::HighlightedEntry);
        state.last();
        state.settle(0);
        assert_eq!(state.offset(), 3);
    }

    #[test]
    fn replacing_with_parent_listing_can_restore_child_focus() {
        let mut state = state(AcceptTarget::HighlightedEntry);
        state.set_prompt("beta".to_string(), InputKind::Search);
        state.set_error("old error");
        let child = "/root/omega".to_string();
        state.replace_directory(directory(DirectoryStatus::Present), Some(&child));
        assert_eq!(
            state.focused().map(|entry| entry.name.as_str()),
            Some("omega")
        );
        assert_eq!(state.prompt(), None);
        assert_eq!(state.error(), None);
        assert_eq!(state.offset(), 0);
    }

    #[test]
    fn current_directory_accepts_present_and_missing_but_not_unreadable() {
        let present = state(AcceptTarget::CurrentDirectory);
        assert_eq!(present.selection().unwrap().location, "/root");

        let missing = State::new(
            directory(DirectoryStatus::Missing),
            SelectionPolicy::new(AcceptTarget::CurrentDirectory).allow_missing_directory(true),
        );
        assert_eq!(missing.selection().unwrap().location, "/root");

        let unreadable = State::new(
            directory(DirectoryStatus::Unreadable("denied".to_string())),
            SelectionPolicy::new(AcceptTarget::CurrentDirectory).allow_missing_directory(true),
        );
        assert!(unreadable.selection().is_none());
    }

    #[test]
    fn highlighted_selection_returns_opaque_entry_location_and_kind() {
        let mut state = state(AcceptTarget::HighlightedEntry);
        state.move_by(1);
        assert_eq!(
            state.selection(),
            Some(Selection {
                location: "/root/beta.txt".to_string(),
                kind: EntryKind::File,
                label: "beta.txt".to_string(),
            })
        );
    }

    #[test]
    fn selection_policy_can_reject_entry_kinds() {
        let mut state = State::new(
            directory(DirectoryStatus::Present),
            SelectionPolicy::new(AcceptTarget::HighlightedEntry)
                .selectable(|kind| kind == EntryKind::File),
        );
        assert!(state.selection().is_none());
        state.move_by(1);
        assert_eq!(state.selection().unwrap().kind, EntryKind::File);
    }

    #[test]
    fn a_search_with_no_matches_cannot_be_accepted_as_an_entry() {
        for target in [
            AcceptTarget::HighlightedEntry,
            AcceptTarget::HighlightedEntryOrCurrentDirectory,
        ] {
            let mut state = state(target);
            state.set_prompt("absent".to_string(), InputKind::Search);
            assert!(state.rows().is_empty());
            assert!(state.selection().is_none());
        }
    }

    #[test]
    fn entry_or_directory_falls_back_only_for_a_present_genuinely_empty_directory() {
        let empty = |status| Directory {
            location: "/empty".to_string(),
            parent: Some("/".to_string()),
            label: "/empty".to_string(),
            entries: Vec::new(),
            status,
        };
        let present = State::new(
            empty(DirectoryStatus::Present),
            SelectionPolicy::new(AcceptTarget::HighlightedEntryOrCurrentDirectory),
        );
        assert_eq!(present.selection().unwrap().location, "/empty");
        let missing = State::new(
            empty(DirectoryStatus::Missing),
            SelectionPolicy::new(AcceptTarget::HighlightedEntryOrCurrentDirectory),
        );
        assert!(missing.selection().is_none());
        let unreadable = State::new(
            empty(DirectoryStatus::Unreadable("denied".to_string())),
            SelectionPolicy::new(AcceptTarget::HighlightedEntryOrCurrentDirectory),
        );
        assert!(unreadable.selection().is_none());
    }

    #[test]
    fn filter_no_match_does_not_look_like_an_empty_directory() {
        let mut state = state(AcceptTarget::HighlightedEntryOrCurrentDirectory);
        state.set_prompt("absent".to_string(), InputKind::Search);
        assert!(!state.directory().entries.is_empty());
        assert!(state.rows().is_empty());
        assert!(state.selection().is_none());
    }

    #[test]
    fn prompt_editing_handles_unicode_kill_and_kind_changes() {
        let mut state = state(AcceptTarget::HighlightedEntry);
        state.begin_prompt(InputKind::Search);
        state.edit_prompt(PromptEdit::Insert('æ'), |_| InputKind::Search);
        assert_eq!(state.prompt(), Some("æ"));
        state.edit_prompt(PromptEdit::Backspace, |_| InputKind::Search);
        assert_eq!(state.prompt(), Some(""));
        state.edit_prompt(PromptEdit::Insert('/'), |_| InputKind::Location);
        assert_eq!(state.input_kind(), Some(InputKind::Location));
        state.edit_prompt(PromptEdit::Kill, |_| InputKind::Search);
        assert_eq!(state.prompt(), Some(""));
    }

    #[test]
    fn word_back_removes_search_words_and_path_components() {
        let mut state = state(AcceptTarget::HighlightedEntry);
        state.set_prompt("hello world".to_string(), InputKind::Search);
        state.edit_prompt(PromptEdit::WordBack, |_| InputKind::Search);
        assert_eq!(state.prompt(), Some("hello "));
        state.set_prompt("/one/two".to_string(), InputKind::Location);
        state.edit_prompt(PromptEdit::WordBack, |_| InputKind::Location);
        assert_eq!(state.prompt(), Some("/one"));
        state.edit_prompt(PromptEdit::WordBack, |_| InputKind::Location);
        assert_eq!(state.prompt(), Some("/"));
    }

    #[test]
    fn prompt_and_movement_clear_transient_errors() {
        let mut state = state(AcceptTarget::HighlightedEntry);
        state.set_error("failure");
        assert_eq!(state.error(), Some("failure"));
        state.move_by(1);
        assert_eq!(state.error(), None);
        state.set_error("failure");
        state.begin_prompt(InputKind::Search);
        assert_eq!(state.error(), None);
    }

    #[test]
    fn focus_location_reports_whether_an_entry_was_found() {
        let mut state = state(AcceptTarget::HighlightedEntry);
        assert!(state.focus_location(&"/root/gamma".to_string()));
        assert_eq!(state.cursor(), 2);
        assert!(!state.focus_location(&"/root/absent".to_string()));
        assert_eq!(state.cursor(), 2);
    }
}
