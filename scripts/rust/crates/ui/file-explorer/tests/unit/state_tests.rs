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
