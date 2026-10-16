use super::changes_filter_tests::fixture;
use super::*;

fn key(app: &mut App, code: KeyCode) {
    app.key(
        KeyEvent::new(code, KeyModifiers::NONE),
        &mut None,
        &mut None,
    )
    .unwrap();
}

#[test]
fn arrow_navigation_wraps_at_root_and_last_canvas() {
    let (_temp, mut app) = fixture();
    key(&mut app, KeyCode::Left);
    assert_eq!(
        (app.view.workspace, app.view.focus),
        (Workspace::Compare, Focus::Canvas)
    );
    key(&mut app, KeyCode::Right);
    assert_eq!(
        (app.view.workspace, app.view.focus),
        (Workspace::Changes, Focus::Navigator)
    );
    key(&mut app, KeyCode::Right);
    assert_eq!(app.view.focus, Focus::Canvas);
    key(&mut app, KeyCode::Left);
    assert_eq!(
        (app.view.workspace, app.view.focus),
        (Workspace::Changes, Focus::Navigator)
    );
}

#[test]
fn every_tab_crosses_to_the_adjacent_tab_at_its_outer_edge() {
    let (_temp, mut app) = fixture();
    for (index, workspace) in Workspace::ALL.iter().enumerate() {
        app.workspace(*workspace);
        app.view.focus = Focus::Canvas;
        app.content_worker.invalidate();
        app.diff = None;
        app.text.clear();
        key(&mut app, KeyCode::Right);
        assert_eq!(app.view.workspace, Workspace::ALL[(index + 1) % 6]);
        assert_eq!(app.view.focus, Focus::Navigator);
        key(&mut app, KeyCode::Left);
        assert_eq!(app.view.workspace, *workspace);
        assert_eq!(app.view.focus, Focus::Canvas);
    }
}

#[test]
fn right_opens_commit_then_moves_to_diff_and_tab_reaches_metadata() {
    let (_temp, mut app) = fixture();
    app.workspace(Workspace::History);
    key(&mut app, KeyCode::Right);
    assert!(app.history_commit.is_some());
    assert_eq!(app.view.focus, Focus::Navigator);
    key(&mut app, KeyCode::Right);
    assert_eq!(app.view.focus, Focus::Canvas);
    key(&mut app, KeyCode::BackTab);
    assert_eq!(app.view.focus, Focus::CommitDetails);
    key(&mut app, KeyCode::Up);
    assert_eq!(app.view.commit_field, 2);
    key(&mut app, KeyCode::Down);
    assert_eq!(app.view.commit_field, 0);
    key(&mut app, KeyCode::Backspace);
    assert!(app.history_commit.is_none());
    assert_eq!(app.view.focus, Focus::Navigator);
}

#[test]
fn horizontal_scroll_gets_first_chance_before_changing_panes() {
    let (_temp, mut app) = fixture();
    app.view.focus = Focus::Canvas;
    app.text = vec!["x".repeat(200)];
    app.geometry = Some(Geometry {
        canvas: ratatui::layout::Rect::new(40, 2, 40, 20),
        ..Default::default()
    });
    key(&mut app, KeyCode::Right);
    assert_eq!(app.view.horizontal, 8);
    assert_eq!(app.view.workspace, Workspace::Changes);
    key(&mut app, KeyCode::Left);
    assert_eq!(app.view.horizontal, 0);
    assert_eq!(app.view.focus, Focus::Canvas);
    key(&mut app, KeyCode::Left);
    assert_eq!(app.view.focus, Focus::Navigator);
}

#[test]
fn inspect_scrolls_and_closes_without_changing_selection() {
    let (_temp, mut app) = fixture();
    app.dialog = Some(Dialog::Inspect {
        message: "long message\n".repeat(100),
        scroll: 0,
        loading: false,
    });
    let selected = app.view.selected;
    key(&mut app, KeyCode::End);
    assert!(matches!(app.dialog, Some(Dialog::Inspect { scroll, .. }) if scroll > 0));
    key(&mut app, KeyCode::Home);
    assert!(matches!(
        app.dialog,
        Some(Dialog::Inspect { scroll: 0, .. })
    ));
    key(&mut app, KeyCode::Esc);
    assert!(app.dialog.is_none());
    assert_eq!(app.view.selected, selected);
    assert!(!app.quit);
}
