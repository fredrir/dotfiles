use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;
use ui_diff_view::{Action, DiffDocument, DiffView, Kind, ViewMode, ViewState};
use ui_theme::Palette;

fn render(document: &DiffDocument, state: &ViewState, width: u16, height: u16) -> String {
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    DiffView {
        document,
        state,
        palette: &Palette::default(),
        color: false,
        left_label: "repo",
        right_label: "live",
    }
    .render(area, &mut buffer);
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn comparison_preserves_line_numbers_and_both_replacement_values() {
    let document = DiffDocument::new("first\nold\nlast\n", "first\nnew\nlast\n");
    assert_eq!((document.added, document.removed), (1, 1));
    let removed = document
        .lines
        .iter()
        .find(|line| line.kind == Kind::Removed)
        .unwrap();
    assert_eq!(removed.old_line, Some(2));
    assert_eq!(removed.new_line, None);
    for mode in [ViewMode::Unified, ViewMode::SideBySide] {
        let shown = render(&document, &ViewState::new(mode), 90, 10);
        for value in ["repo", "live", "old", "new", "last"] {
            assert!(shown.contains(value), "missing {value}: {shown}");
        }
        assert!(!shown.contains('\u{1b}'));
    }
}

#[test]
fn a_final_newline_change_is_visible() {
    let document = DiffDocument::new("same value", "same value\n");
    assert!(document.lines.iter().any(|line| line.missing_newline));
    let shown = render(&document, &ViewState::default(), 90, 8);
    assert!(shown.contains("no final newline"), "{shown}");
}

#[test]
fn scrolling_and_hunk_navigation_reach_late_changes() {
    let left = (0..80)
        .map(|index| format!("line {index}\n"))
        .collect::<String>();
    let right = left
        .replace("line 3\n", "early change\n")
        .replace("line 70\n", "late change\n");
    let document = DiffDocument::new(&left, &right);
    let mut state = ViewState::default();
    state.apply(Action::NextHunk, &document, 10);
    state.apply(Action::NextHunk, &document, 10);
    assert!(render(&document, &state, 80, 10).contains("late change"));
    state.apply(Action::End, &document, 10);
    assert!(render(&document, &state, 80, 10).contains("line 79"));
    state.apply(Action::ToggleMode, &document, 10);
    assert!(render(&document, &state, 80, 10).contains("line 79"));
    state.apply(Action::Home, &document, 10);
    assert_eq!(state.offset, 0);
}

#[test]
fn navigation_responds_immediately_after_the_viewport_grows() {
    let source = (0..80)
        .map(|index| format!("line {index}\n"))
        .collect::<String>();
    let document = DiffDocument::new(&source, &source);
    let mut state = ViewState::default();
    state.apply(Action::End, &document, 8);
    state.apply(Action::Up, &document, 20);
    assert!(render(&document, &state, 80, 20).contains("line 61"));
    assert!(!render(&document, &state, 80, 20).contains("line 79"));
}

#[test]
fn split_preference_and_current_line_survive_narrow_navigation_and_widening() {
    let source = (0..40)
        .map(|index| format!("line {index}\n"))
        .collect::<String>();
    let document = DiffDocument::new(&source, &source.replace("line", "changed"));
    let mut state = ViewState::new(ViewMode::SideBySide);
    state.fit_width(&document, 40, 10);
    assert_eq!(state.mode, ViewMode::SideBySide);
    assert_eq!(state.effective_mode(), ViewMode::Unified);
    state.apply(Action::End, &document, 10);
    let narrow = render(&document, &state, 40, 10);
    assert!(narrow.contains("changed 32"));
    assert!(narrow.contains("changed 39"));

    state.fit_width(&document, 90, 10);
    assert_eq!(state.mode, ViewMode::SideBySide);
    assert_eq!(state.effective_mode(), ViewMode::SideBySide);
    let wide = render(&document, &state, 90, 10);
    for text in ["side by side", "line 32", "changed 32", "changed 39"] {
        assert!(wide.contains(text), "missing {text}: {wide}");
    }
}

#[test]
fn repeated_narrow_layout_toggles_never_reinterpret_unified_row_offsets() {
    let source = (0..40)
        .map(|index| format!("line {index}\n"))
        .collect::<String>();
    let document = DiffDocument::new(&source, &source.replace("line", "changed"));
    let mut state = ViewState::new(ViewMode::SideBySide);
    state.fit_width(&document, 40, 10);
    state.apply(Action::End, &document, 10);
    let final_page = render(&document, &state, 40, 10);
    for _ in 0..4 {
        state.handle_key(
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE),
            &document,
            10,
        );
        assert_eq!(render(&document, &state, 40, 10), final_page);
        state.apply(Action::Up, &document, 10);
        let previous_page = render(&document, &state, 40, 10);
        assert!(previous_page.contains("changed 31"));
        assert!(!previous_page.contains("changed 39"));
        state.apply(Action::Down, &document, 10);
        assert_eq!(render(&document, &state, 40, 10), final_page);
    }
    assert_eq!(state.mode, ViewMode::SideBySide);
}

#[test]
fn navigation_keys_do_not_consume_confirmation_or_cancellation() {
    let document = DiffDocument::new("old", "new");
    let mut state = ViewState::default();
    for code in [KeyCode::Enter, KeyCode::Esc, KeyCode::Char('q')] {
        assert!(!state.handle_key(KeyEvent::new(code, KeyModifiers::NONE), &document, 10));
    }
    assert!(state.handle_key(
        KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE),
        &document,
        10
    ));
    assert_eq!(state.mode, ViewMode::SideBySide);
}

#[test]
fn binary_and_large_values_are_explicit_and_preview_work_is_bounded() {
    let binary = DiffDocument::new("old\0value", "new\0value");
    assert!(binary.binary);
    assert!(render(&binary, &ViewState::default(), 80, 5).contains("Binary values"));
    let large = "a long configuration line\n".repeat(50_000);
    let document = DiffDocument::new(&large, "replacement\n");
    assert!(document.truncated);
    assert!(document.lines.len() < 5000);
    assert!(
        render(&document, &ViewState::default(), 80, 8).contains("choices apply to the full value")
    );
}

#[test]
fn terminal_controls_are_sanitized_and_small_views_do_not_panic() {
    let document = DiffDocument::new("\u{1b}[2J\t界", "\u{7}new");
    assert!(
        document
            .lines
            .iter()
            .all(|line| !line.text.chars().any(char::is_control))
    );
    for width in [0, 1, 10, 43, 44] {
        for height in [0, 1, 2, 3] {
            render(
                &document,
                &ViewState::new(ViewMode::SideBySide),
                width,
                height,
            );
        }
    }
}

#[test]
fn arbitrary_labels_cannot_inject_terminal_controls() {
    for source in ["text", "binary\0value"] {
        let document = DiffDocument::new(source, source);
        let area = Rect::new(0, 0, 100, 4);
        let mut buffer = Buffer::empty(area);
        DiffView {
            document: &document,
            state: &ViewState::default(),
            palette: &Palette::default(),
            color: false,
            left_label: "repo\u{1b}[2J",
            right_label: "live\r\nnext\u{7}",
        }
        .render(area, &mut buffer);
        let shown = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(!shown.chars().any(char::is_control), "{shown:?}");
        assert!(shown.contains("repo"));
        assert!(shown.contains("live"));
    }
}
