use super::*;
use std::sync::atomic::Ordering;

struct Fixture {
    directory: tempfile::TempDir,
    repo: Repo,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        git(directory.path(), &["init", "--initial-branch=main"]);
        for (name, value) in [
            ("user.name", "Dogit Test"),
            ("user.email", "dogit@example.invalid"),
            ("commit.gpgsign", "false"),
            ("core.autocrlf", "false"),
            ("core.hooksPath", ".git/hooks"),
        ] {
            git(directory.path(), &["config", name, value]);
        }
        std::fs::write(directory.path().join("file.txt"), "original\n").unwrap();
        git(directory.path(), &["add", "--all"]);
        git(directory.path(), &["commit", "--message", "initial"]);
        let repo = Repo::open(directory.path()).unwrap();
        Self { directory, repo }
    }

    fn app(&self) -> App {
        let mut app = App::new(self.repo.clone(), Start::Changes);
        app.snapshot = self.repo.snapshot(300).unwrap();
        app.files = self.repo.files().unwrap();
        app.rebuild();
        app
    }

    fn write(&self, content: &str) {
        std::fs::write(self.directory.path().join("file.txt"), content).unwrap();
    }
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn collect_until(app: &mut App, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done(app) {
        assert!(Instant::now() < deadline, "worker did not finish");
        app.collect();
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn results_use_action_titles_and_errors_remain_until_dismissed() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    for (label, expected) in [
        ("commit", "Committed!"),
        ("Push", "Pushed!"),
        ("Create branch", "Branch created!"),
    ] {
        app.report_result(label, Ok("noisy git output".into()));
        let notification = app.notification.as_ref().unwrap();
        assert_eq!(notification.title, expected);
        assert!(notification.detail.is_empty());
        assert!(!notification.failed);
    }
    app.notification.as_mut().unwrap().started = Instant::now() - Duration::from_secs(6);
    app.collect();
    assert!(app.notification.is_none());

    app.report_result(
        "push",
        Err("remote rejected the update\nretry after pulling".into()),
    );
    let notification = app.notification.as_mut().unwrap();
    assert_eq!(notification.title, "Push failed");
    assert_eq!(
        notification.detail,
        "remote rejected the update\nretry after pulling"
    );
    notification.started = Instant::now() - Duration::from_secs(60);
    app.collect();
    assert!(app.notification.as_ref().unwrap().failed);
    app.key(
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        &mut None,
        &mut None,
    )
    .unwrap();
    assert!(app.notification.is_none());
    assert!(!app.quit);
}

#[test]
fn background_refresh_preserves_slow_preview_and_explicit_refresh_supersedes_it() {
    let fixture = Fixture::new();
    fixture.write("modified\n");
    let mut app = fixture.app();
    let (started, token) = crossbeam_channel::bounded(1);
    let (release, blocked) = crossbeam_channel::bounded(1);
    app.content_worker.submit(move |cancel| {
        started.send(cancel).unwrap();
        let _ = blocked.recv();
        Err("superseded".into())
    });
    app.content_pending = true;
    let cancel = token.recv_timeout(Duration::from_secs(5)).unwrap();
    app.refresh_background();
    collect_until(&mut app, |app| !app.snapshot_pending);
    assert!(!cancel.load(Ordering::Acquire));
    app.refresh_background();
    app.refresh(); // A forced refresh must survive an already pending timer read.
    collect_until(&mut app, |app| !app.snapshot_pending);
    assert!(cancel.load(Ordering::Acquire));
    release.send(()).unwrap();
}

#[test]
fn preview_keys_track_worktree_content_and_moving_refs_but_cache_commits() {
    let fixture = Fixture::new();
    fixture.write("modified\n");
    let app = fixture.app();
    let request = app.preview_request();
    let before =
        PreviewKey::capture(&app.repo, request.clone(), &app.snapshot, &app.files).unwrap();
    let mut commit = request.clone();
    commit.workspace = Workspace::History;
    commit.target = Some(Target::Commit(app.snapshot.status.oid.clone().unwrap()));
    let immutable =
        PreviewKey::capture(&app.repo, commit.clone(), &app.snapshot, &app.files).unwrap();
    fixture.write("replaced\n"); // Same path, length, and Git status.
    assert_ne!(
        before,
        PreviewKey::capture(&app.repo, request, &app.snapshot, &app.files).unwrap()
    );
    assert_eq!(
        immutable,
        PreviewKey::capture(&app.repo, commit, &app.snapshot, &app.files).unwrap()
    );

    let mut compare = app.preview_request();
    compare.target = Some(Target::RevisionAll);
    compare.workspace = Workspace::Compare;
    compare.comparison = Some(("HEAD".into(), "refs/heads/main".into()));
    let before =
        PreviewKey::capture(&app.repo, compare.clone(), &app.snapshot, &app.files).unwrap();
    git(fixture.directory.path(), &["add", "--all"]);
    git(fixture.directory.path(), &["commit", "--message", "next"]);
    let snapshot = fixture.repo.snapshot(300).unwrap();
    assert_ne!(
        before,
        PreviewKey::capture(&app.repo, compare, &snapshot, &app.files).unwrap()
    );
}

#[test]
fn selection_reset_clears_previous_plain_text_immediately() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.text = vec!["previous file".into()];
    app.content_line_count = 1;
    app.load_content(true);
    assert!(app.text.is_empty());
    assert_eq!(app.content_line_count, 0);
}

#[cfg(unix)]
#[test]
fn preview_displays_symlink_without_opening_target() {
    let directory = tempfile::tempdir().unwrap();
    let link = directory.path().join("link");
    std::os::unix::fs::symlink("/dev/zero", &link).unwrap();
    assert_eq!(
        preview_file(&link).unwrap().unwrap(),
        ["Symbolic link → /dev/zero"]
    );
}

#[test]
fn logical_line_anchor_survives_an_earlier_hunk() {
    let fixture = Fixture::new();
    let mut lines = (0..80)
        .map(|index| format!("line {index}\n"))
        .collect::<Vec<_>>();
    fixture.write(&lines.concat());
    git(fixture.directory.path(), &["add", "--all"]);
    git(
        fixture.directory.path(),
        &["commit", "--message", "many lines"],
    );
    lines[65] = "selected change\n".into();
    fixture.write(&lines.concat());
    let mut app = fixture.app();
    app.diff = Some(Arc::new(
        ExactDiff::load(&app.repo.root, Path::new("file.txt"), DiffSide::Worktree).unwrap(),
    ));
    app.update_rows();
    let id = app
        .diff
        .as_ref()
        .unwrap()
        .hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .find(|line| line.content == b"selected change\n")
        .unwrap()
        .id;
    app.view.diff_cursor = app
        .row_map
        .iter()
        .position(|row| row.line == Some(id))
        .unwrap();
    let anchor = app.line_anchor().unwrap();
    lines[5] = "earlier change\n".into();
    fixture.write(&lines.concat());
    app.diff = Some(Arc::new(
        ExactDiff::load(&app.repo.root, Path::new("file.txt"), DiffSide::Worktree).unwrap(),
    ));
    app.row_map.clear();
    app.update_rows();
    app.restore_line_anchor(&anchor);
    assert_eq!(
        app.line_anchor().unwrap().line.content,
        b"selected change\n"
    );
    assert_ne!(app.line_anchor().unwrap().line.id, id);
}

#[test]
fn busy_prompt_keeps_draft_and_confirmation_scroll_remains_available() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.operation = Some(PendingOperation {
        label: "Fetch".into(),
        started: Instant::now(),
    });
    app.dialog = Some(Dialog::Input {
        prompt: Prompt::Branch,
        title: "Commit".into(),
        value: "keep this draft".into(),
    });
    app.key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut None,
        &mut None,
    )
    .unwrap();
    assert!(matches!(&app.dialog, Some(Dialog::Input { value, .. }) if value == "keep this draft"));
    app.dialog = Some(Dialog::Confirm {
        action: Action::Discard,
        target: None,
        title: "Discard".into(),
        detail: "file\n".repeat(30),
        discard: None,
        abort: None,
        scroll: 0,
    });
    app.key(
        KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
        &mut None,
        &mut None,
    )
    .unwrap();
    assert!(matches!(
        app.dialog,
        Some(Dialog::Confirm { scroll: 8, .. })
    ));
    app.key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut None,
        &mut None,
    )
    .unwrap();
    assert!(matches!(
        app.dialog,
        Some(Dialog::Confirm { scroll: 8, .. })
    ));
}

#[test]
fn horizontal_keys_and_split_preserve_canvas_focus_and_position() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.view.focus = Focus::Canvas;
    app.view.diff_cursor = 7;
    app.key(
        KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        &mut None,
        &mut None,
    )
    .unwrap();
    assert_eq!(app.view.horizontal, 8);
    app.key(
        KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE),
        &mut None,
        &mut None,
    )
    .unwrap();
    assert!(app.view.split);
    assert_eq!(app.view.diff_cursor, 7);
    assert_eq!(app.view.focus, Focus::Canvas);
    assert_eq!(app.view.horizontal, 8);
}

#[test]
fn split_toggle_preserves_the_exact_line_near_the_end_of_a_large_replacement() {
    let fixture = Fixture::new();
    fixture.write(
        &(0..80)
            .map(|index| format!("old value {index}\n"))
            .collect::<String>(),
    );
    git(fixture.directory.path(), &["add", "--all"]);
    git(fixture.directory.path(), &["commit", "-qm", "old block"]);
    fixture.write(
        &(0..80)
            .map(|index| format!("replacement value {index}\n"))
            .collect::<String>(),
    );
    let mut app = fixture.app();
    app.diff = Some(Arc::new(
        ExactDiff::load(&app.repo.root, Path::new("file.txt"), DiffSide::Worktree).unwrap(),
    ));
    app.geometry = Some(Geometry {
        canvas: ratatui::layout::Rect::new(30, 5, 100, 20),
        ..Default::default()
    });
    app.view.focus = Focus::Canvas;
    app.view.split = false;
    app.update_rows();
    let id = app
        .diff
        .as_ref()
        .unwrap()
        .hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .find(|line| line.content == b"replacement value 79\n")
        .unwrap()
        .id;
    app.view.diff_cursor = app
        .row_map
        .iter()
        .position(|row| row.line == Some(id))
        .unwrap();
    assert!(app.view.diff_cursor > 100);
    app.marked.insert(id);
    app.key(
        KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE),
        &mut None,
        &mut None,
    )
    .unwrap();
    assert!(app.row_split);
    let row = &app.row_map[app.view.diff_cursor];
    assert!(row.line == Some(id) || row.right_line == Some(id));
    assert_eq!(app.view.focus, Focus::Canvas);
    assert!(app.marked.contains(&id));
}

#[test]
fn pasted_search_updates_results_and_shift_down_selects_intervening_lines() {
    let fixture = Fixture::new();
    fixture.write("first\nsecond\nthird\nfourth\n");
    let mut app = fixture.app();
    app.dialog = Some(Dialog::Input {
        prompt: Prompt::Search,
        title: "Search".into(),
        value: String::new(),
    });
    app.paste("missing");
    assert_eq!(app.query, "missing");
    assert_eq!(app.entries.len(), 1);
    assert_eq!(app.selected(), Some(&Target::Path(PathBuf::new(), true)));
    assert_eq!(app.nav_summary.items, 0);
    app.dialog = None;
    app.diff = Some(Arc::new(
        ExactDiff::load(&app.repo.root, Path::new("file.txt"), DiffSide::Worktree).unwrap(),
    ));
    app.update_rows();
    app.geometry = Some(Geometry {
        canvas: ratatui::layout::Rect::new(30, 5, 80, 20),
        ..Default::default()
    });
    app.view.diff_cursor = 1;
    app.view.focus = Focus::Canvas;
    for _ in 0..4 {
        app.key(
            KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT),
            &mut None,
            &mut None,
        )
        .unwrap();
    }
    let expected = app.row_map[1..=5]
        .iter()
        .flat_map(|row| [row.line, row.right_line])
        .flatten()
        .filter(|id| app.changed_lines.contains(id))
        .collect::<BTreeSet<_>>();
    assert_eq!(app.marked, expected);
    assert!(app.marked.len() >= 4);
}

#[test]
fn plain_numbers_select_every_workspace_without_opening_dialogs() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    for (number, expected) in ('1'..='6').zip(Workspace::ALL) {
        app.key(
            KeyEvent::new(KeyCode::Char(number), KeyModifiers::NONE),
            &mut None,
            &mut None,
        )
        .unwrap();
        assert_eq!(app.view.workspace, expected);
        assert!(app.dialog.is_none());
    }
    for number in ['0', '7', '8', '9'] {
        app.key(
            KeyEvent::new(KeyCode::Char(number), KeyModifiers::NONE),
            &mut None,
            &mut None,
        )
        .unwrap();
        assert_eq!(app.view.workspace, Workspace::Compare);
        assert!(app.dialog.is_none());
    }
}

#[test]
fn tab_and_backtab_cycle_panes_without_changing_workspace_or_position() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    for workspace in Workspace::ALL {
        app.view.workspace = workspace;
        app.view.selected = 3;
        app.view.diff_cursor = 7;
        app.view.horizontal = 8;
        for key in [
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT),
        ] {
            for expected in [Focus::Canvas, Focus::Navigator] {
                app.key(key, &mut None, &mut None).unwrap();
                assert_eq!(app.view.focus, expected);
                assert_eq!(app.view.workspace, workspace);
                assert_eq!(app.view.selected, 3);
                assert_eq!(app.view.diff_cursor, 7);
                assert_eq!(app.view.horizontal, 8);
                assert!(app.dialog.is_none());
            }
        }
    }
}

#[test]
fn function_keys_are_unbound_and_control_r_refreshes() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.snapshot_pending = true;
    app.refresh_forced = false;
    for number in 1..=12 {
        app.key(
            KeyEvent::new(KeyCode::F(number), KeyModifiers::NONE),
            &mut None,
            &mut None,
        )
        .unwrap();
        assert!(!app.refresh_forced);
        assert!(app.dialog.is_none());
        assert_eq!(app.view.workspace, Workspace::Changes);
        assert_eq!(app.mouse_capture, ui_terminal::MouseCapture::Enabled);
    }
    app.key(
        KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
        &mut None,
        &mut None,
    )
    .unwrap();
    assert!(app.refresh_forced);
    assert_eq!(app.mouse_capture, ui_terminal::MouseCapture::Enabled);
}

#[test]
fn explicit_shifted_number_events_select_workspaces() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    for (number, workspace) in ('1'..='6').zip(Workspace::ALL) {
        app.key(
            KeyEvent::new(KeyCode::Char(number), KeyModifiers::SHIFT),
            &mut None,
            &mut None,
        )
        .unwrap();
        assert_eq!(app.view.workspace, workspace);
        assert!(app.dialog.is_none());
    }
}

#[test]
fn punctuation_is_not_guessed_to_be_a_workspace_shortcut() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.view.focus = Focus::Canvas;
    for modifier in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
        for character in "!@#$%^&\"¤".chars() {
            app.key(
                KeyEvent::new(KeyCode::Char(character), modifier),
                &mut None,
                &mut None,
            )
            .unwrap();
            assert_eq!(app.view.workspace, Workspace::Changes);
            assert_eq!(app.view.focus, Focus::Canvas);
            assert!(app.dialog.is_none());
        }
    }
}

#[test]
fn shifted_numbers_and_symbols_remain_text_in_prompts_and_search() {
    let fixture = Fixture::new();
    for prompt in [Prompt::Branch, Prompt::Search] {
        let mut app = fixture.app();
        app.dialog = Some(Dialog::Input {
            prompt,
            title: "Input".into(),
            value: String::new(),
        });
        let mut expected = String::new();
        for modifier in [KeyModifiers::NONE, KeyModifiers::SHIFT] {
            for character in "!@#$%^&\"¤/0123456789".chars() {
                expected.push(character);
                app.key(
                    KeyEvent::new(KeyCode::Char(character), modifier),
                    &mut None,
                    &mut None,
                )
                .unwrap();
                assert_eq!(app.view.workspace, Workspace::Changes);
                assert!(
                    matches!(&app.dialog, Some(Dialog::Input { value, .. }) if value == &expected)
                );
            }
        }
    }
}

#[test]
fn other_modifiers_cannot_activate_number_navigation() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.view.workspace = Workspace::History;
    app.view.focus = Focus::Canvas;
    for modifier in [
        KeyModifiers::CONTROL,
        KeyModifiers::ALT,
        KeyModifiers::SUPER,
        KeyModifiers::META,
        KeyModifiers::HYPER,
        KeyModifiers::SHIFT | KeyModifiers::SUPER,
        KeyModifiers::SHIFT | KeyModifiers::CONTROL,
    ] {
        for character in "0123456789!@#$%^&".chars() {
            app.key(
                KeyEvent::new(KeyCode::Char(character), modifier),
                &mut None,
                &mut None,
            )
            .unwrap();
            assert_eq!(app.view.workspace, Workspace::History);
            assert_eq!(app.view.focus, Focus::Canvas);
            assert!(app.dialog.is_none());
        }
    }
}

#[test]
fn slash_opens_search_without_being_guessed_to_be_shift_seven() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.query = "existing filter".into();
    app.key(
        KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
        &mut None,
        &mut None,
    )
    .unwrap();
    assert!(
        matches!(&app.dialog, Some(Dialog::Input { prompt: Prompt::Search, value, .. }) if value == "existing filter")
    );
    assert_eq!(app.view.workspace, Workspace::Changes);
}

#[test]
fn escape_closes_modal_before_quitting_from_canvas() {
    let fixture = Fixture::new();
    for dialog in [
        Dialog::Actions {
            query: String::new(),
            selected: 0,
        },
        Dialog::Input {
            prompt: Prompt::Branch,
            title: "Commit".into(),
            value: "draft".into(),
        },
        Dialog::Confirm {
            action: Action::Discard,
            target: None,
            title: "Discard".into(),
            detail: String::new(),
            discard: None,
            abort: None,
            scroll: 0,
        },
    ] {
        let mut app = fixture.app();
        app.view.focus = Focus::Canvas;
        app.dialog = Some(dialog);
        app.key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut None,
            &mut None,
        )
        .unwrap();
        assert!(app.dialog.is_none());
        assert!(!app.quit);
        app.key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut None,
            &mut None,
        )
        .unwrap();
        assert!(app.quit);
    }
}

#[test]
fn escape_unwinds_selection_search_and_commit_layers_and_keeps_mutations_running() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.view.workspace = Workspace::History;
    app.history_commit = app.snapshot.status.oid.clone();
    app.query = "file".into();
    app.marked.insert(42);
    for layer in 0..3 {
        app.key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &mut None,
            &mut None,
        )
        .unwrap();
        assert!(!app.quit);
        assert!(app.marked.is_empty());
        if layer >= 1 {
            assert!(app.query.is_empty());
        }
        if layer == 2 {
            assert!(app.history_commit.is_none());
        }
    }
    app.operation = Some(PendingOperation {
        label: "Commit".into(),
        started: Instant::now(),
    });
    app.key(
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        &mut None,
        &mut None,
    )
    .unwrap();
    assert!(!app.quit);
    assert!(app.operation.is_some());
    app.operation = None;
    app.key(
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        &mut None,
        &mut None,
    )
    .unwrap();
    assert!(app.quit);
}

#[test]
fn divider_hover_uses_broader_hit_area_and_drag_preserves_grab_offset() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.geometry = Some(Geometry {
        body: ratatui::layout::Rect::new(1, 7, 122, 24),
        divider_x: Some(35),
        divider_hit: Some(ratatui::layout::Rect::new(33, 7, 5, 24)),
        ..Default::default()
    });
    let mouse = |kind, column| MouseEvent {
        kind,
        column,
        row: 12,
        modifiers: KeyModifiers::NONE,
    };
    app.dirty = false;
    app.mouse(mouse(MouseEventKind::Moved, 32));
    assert!(!app.view.divider_hovered);
    assert!(!app.dirty);
    app.mouse(mouse(MouseEventKind::Moved, 33));
    assert!(app.view.divider_hovered);
    assert_eq!(app.view.divider_hover_row, Some(12));
    assert!(app.dirty);
    app.dirty = false;
    app.mouse(mouse(MouseEventKind::Moved, 37));
    assert!(!app.dirty);
    app.mouse(MouseEvent {
        row: 14,
        ..mouse(MouseEventKind::Moved, 37)
    });
    assert_eq!(app.view.divider_hover_row, Some(14));
    assert!(app.dirty);
    app.mouse(mouse(MouseEventKind::Down(MouseButton::Left), 37));
    assert!(app.dragging);
    assert_eq!(app.drag_offset, 2);
    app.mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 37));
    assert_eq!(app.view.nav_width, 34);
    app.mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 47));
    assert_eq!(app.view.nav_width, 44);
    assert!(app.view.divider_hovered);
    app.mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 0));
    assert_eq!(app.view.nav_width, 22);
    app.mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 120));
    assert_eq!(app.view.nav_width, 90);
    app.dirty = false;
    app.mouse(mouse(MouseEventKind::Moved, 80));
    assert!(app.view.divider_hovered);
    assert!(!app.dirty);
    app.mouse(mouse(MouseEventKind::Up(MouseButton::Left), 80));
    assert!(!app.dragging);
    assert!(!app.view.divider_hovered);
    assert_eq!(app.view.divider_hover_row, None);
}

fn populate_navigation(app: &mut App, count: usize) {
    app.entries = (0..count)
        .map(|index| Entry {
            target: Target::Path(PathBuf::from(format!("file-{index}")), false),
            item: nav(&format!("file-{index}"), "", "", NavKind::File),
        })
        .collect();
    app.nav_items = app.entries.iter().map(|entry| entry.item.clone()).collect();
}

#[test]
fn navigator_steps_wrap_but_home_end_and_pages_keep_their_boundaries() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    populate_navigation(&mut app, 20);
    for (key, expected) in [
        (KeyCode::Up, 19),
        (KeyCode::Down, 0),
        (KeyCode::Char('k'), 19),
        (KeyCode::Char('j'), 0),
        (KeyCode::Home, 0),
        (KeyCode::PageUp, 0),
        (KeyCode::End, 19),
        (KeyCode::PageDown, 19),
    ] {
        app.key(KeyEvent::new(key, KeyModifiers::NONE), &mut None, &mut None)
            .unwrap();
        assert_eq!(app.view.selected, expected);
    }
    app.entries.clear();
    app.key(
        KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        &mut None,
        &mut None,
    )
    .unwrap();
    assert_eq!(app.view.selected, 0);
}

#[test]
fn action_palette_wraps_and_keeps_letter_keys_for_search() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let last = app.available_actions("").len() - 1;
    app.dialog = Some(Dialog::Actions {
        query: String::new(),
        selected: 0,
    });
    for (key, modifiers, expected) in [
        (KeyCode::Up, KeyModifiers::NONE, last),
        (KeyCode::Down, KeyModifiers::NONE, 0),
        (KeyCode::Char('k'), KeyModifiers::CONTROL, last),
        (KeyCode::Char('j'), KeyModifiers::CONTROL, 0),
        (KeyCode::End, KeyModifiers::NONE, last),
        (KeyCode::PageDown, KeyModifiers::NONE, last),
        (KeyCode::Home, KeyModifiers::NONE, 0),
    ] {
        app.key(KeyEvent::new(key, modifiers), &mut None, &mut None)
            .unwrap();
        assert!(
            matches!(app.dialog, Some(Dialog::Actions { selected, .. }) if selected == expected)
        );
    }
    for character in ['j', 'k'] {
        app.key(
            KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
            &mut None,
            &mut None,
        )
        .unwrap();
    }
    assert!(matches!(&app.dialog, Some(Dialog::Actions { query, .. }) if query == "jk"));
}

#[test]
fn wheel_scrolls_the_pointed_viewport_without_changing_focus_selection_or_preview() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    populate_navigation(&mut app, 100);
    app.nav_items[0].label = "n".repeat(100);
    app.text = vec!["x".repeat(160); 100];
    app.content_line_count = app.text.len();
    app.view.focus = Focus::Canvas;
    app.view.selected = 2;
    app.view.diff_cursor = 3;
    app.geometry = Some(Geometry {
        nav: ratatui::layout::Rect::new(1, 5, 34, 8),
        canvas: ratatui::layout::Rect::new(36, 5, 80, 8),
        ..Default::default()
    });
    let wheel = |kind, column| MouseEvent {
        kind,
        column,
        row: 8,
        modifiers: KeyModifiers::NONE,
    };
    app.mouse(wheel(MouseEventKind::ScrollDown, 10));
    assert_eq!(app.view.offset, 1);
    assert!(app.view.nav_manual_scroll);
    assert_eq!(app.view.focus, Focus::Canvas);
    assert_eq!(app.view.selected, 2);
    app.view.focus = Focus::Navigator;
    app.mouse(wheel(MouseEventKind::ScrollDown, 50));
    assert_eq!(app.view.diff_offset, 1);
    assert!(app.view.canvas_manual_scroll);
    assert_eq!(app.view.focus, Focus::Navigator);
    assert_eq!(app.view.diff_cursor, 3);
    assert!(!app.content_pending);
    app.mouse(wheel(MouseEventKind::ScrollRight, 50));
    assert_eq!(app.view.horizontal, 1);
    assert_eq!(app.view.nav_horizontal, 0);
    app.mouse(wheel(MouseEventKind::ScrollRight, 10));
    assert_eq!(app.view.nav_horizontal, 1);
    assert_eq!(app.view.horizontal, 1);
    app.mouse(wheel(MouseEventKind::ScrollUp, 10));
    app.mouse(wheel(MouseEventKind::ScrollUp, 50));
    assert_eq!(app.view.offset, 0);
    assert_eq!(app.view.diff_offset, 0);
    assert_eq!(app.view.selected, 2);
    assert_eq!(app.view.diff_cursor, 3);
    app.view.nav_horizontal = u16::MAX as usize;
    app.view.horizontal = u16::MAX as usize;
    app.mouse(wheel(MouseEventKind::ScrollRight, 10));
    app.mouse(wheel(MouseEventKind::ScrollRight, 50));
    assert_eq!(app.view.nav_horizontal, 69);
    assert_eq!(app.view.horizontal, 84);
}

#[test]
fn manual_viewport_scroll_survives_render_until_keyboard_navigation_resumes() {
    let fixture = Fixture::new();
    fixture.write(
        &(0..100)
            .map(|index| format!("line {index}\n"))
            .collect::<String>(),
    );
    let mut app = fixture.app();
    populate_navigation(&mut app, 100);
    app.view.selected = 0;
    app.diff = Some(Arc::new(
        ExactDiff::load(&app.repo.root, Path::new("file.txt"), DiffSide::Worktree).unwrap(),
    ));
    app.update_rows();
    app.view.offset = 10;
    app.view.diff_offset = 10;
    app.view.nav_manual_scroll = true;
    app.view.canvas_manual_scroll = true;
    app.view.focus = Focus::Canvas;
    let backend = ratatui::backend::TestBackend::new(120, 30);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    let data = RenderData {
        items: &app.nav_items,
        diff: app.diff.as_deref(),
        ..Default::default()
    };
    terminal
        .draw(|frame| {
            view::render(frame, &mut app.view, &data);
        })
        .unwrap();
    assert_eq!(app.view.offset, 10);
    assert_eq!(app.view.diff_offset, 10);
    assert_eq!(app.view.selected, 0);
    assert_eq!(app.view.diff_cursor, 0);
    app.move_cursor(1, false);
    assert!(!app.view.canvas_manual_scroll);
    assert_eq!(app.view.diff_offset, 1);
}

#[test]
fn changes_tree_unifies_sides_aggregates_folders_and_keeps_staged_deletions() {
    let fixture = Fixture::new();
    let root = fixture.directory.path();
    std::fs::create_dir(root.join("src")).unwrap();
    for name in ["staged", "mixed", "working", "deleted"] {
        std::fs::write(root.join("src").join(name), "original\n").unwrap();
    }
    git(root, &["add", "--all"]);
    git(root, &["commit", "-m", "tree"]);
    for name in ["staged", "mixed", "working"] {
        std::fs::write(root.join("src").join(name), "modified\n").unwrap();
    }
    git(root, &["add", "src/staged", "src/mixed"]);
    git(root, &["rm", "src/deleted"]);
    std::fs::write(root.join("src/mixed"), "more work\n").unwrap();
    std::fs::write(root.join("src/new"), "new\n").unwrap();
    let mut app = fixture.app();
    let states = |app: &App, path: &str| {
        app.entries
            .iter()
            .find(
                |entry| matches!(&entry.target, Target::Path(value, _) if value == Path::new(path)),
            )
            .and_then(|entry| entry.item.file_state)
            .unwrap()
    };
    assert!(states(&app, "src/staged").staged);
    assert!(!states(&app, "src/staged").unstaged);
    assert!(states(&app, "src/mixed").staged && states(&app, "src/mixed").unstaged);
    assert!(states(&app, "src/working").unstaged);
    assert!(states(&app, "src").staged && states(&app, "src").unstaged);
    assert_eq!(
        (
            app.nav_summary.items,
            app.nav_summary.staged,
            app.nav_summary.unstaged
        ),
        (5, 3, 3)
    );
    app.collapsed.insert(PathBuf::from("src"));
    app.rebuild();
    assert_eq!(app.entries.len(), 1);
    assert_eq!(
        (
            app.nav_summary.items,
            app.nav_summary.staged,
            app.nav_summary.unstaged
        ),
        (5, 3, 3)
    );
    app.query = "mixed".into();
    app.rebuild();
    assert_eq!(app.entries.len(), 2);
    assert!(states(&app, "src").expanded);
    assert!(app.collapsed.contains(Path::new("src")));
    app.query.clear();
    app.collapsed.clear();
    app.all_files = true;
    app.rebuild();
    assert!(states(&app, "src/deleted").staged);
    assert_eq!(
        app.entries
            .iter()
            .filter(|entry| entry.target == Target::Path("src/mixed".into(), false))
            .count(),
        1
    );
    assert_eq!(app.nav_summary.items, 6);
}

#[test]
fn staging_preserves_selected_path_and_auto_selects_available_diff_side() {
    let fixture = Fixture::new();
    fixture.write("changed\n");
    std::fs::write(fixture.directory.path().join("later"), "unrelated\n").unwrap();
    let mut app = fixture.app();
    let target = Target::Path("file.txt".into(), false);
    app.view.selected = app
        .entries
        .iter()
        .position(|entry| entry.target == target)
        .unwrap();
    assert_eq!(app.selected(), Some(&target));
    app.action(Action::StageFile, &mut None, &mut None).unwrap();
    collect_until(&mut app, |app| {
        app.operation.is_none() && !app.snapshot_pending && !app.content_pending
    });
    assert_eq!(app.selected(), Some(&target));
    assert_eq!(app.side, DiffSide::Index);
    assert!(app.selected_file_state().unwrap().staged);
    assert!(!app.selected_file_state().unwrap().unstaged);
    app.action(Action::UnstageFile, &mut None, &mut None)
        .unwrap();
    collect_until(&mut app, |app| {
        app.operation.is_none() && !app.snapshot_pending && !app.content_pending
    });
    assert_eq!(app.selected(), Some(&target));
    assert_eq!(app.side, DiffSide::Worktree);
    assert!(app.selected_file_state().unwrap().unstaged);
}

#[test]
fn tree_arrows_navigate_compacted_directories_and_return_to_visible_parent() {
    let fixture = Fixture::new();
    let root = fixture.directory.path();
    std::fs::create_dir_all(root.join("src/nested")).unwrap();
    std::fs::write(root.join("src/nested/new"), "new\n").unwrap();
    fixture.write("root change\n");
    let mut app = fixture.app();
    app.view.selected = app
        .entries
        .iter()
        .position(|entry| entry.target == Target::Path("src/nested".into(), true))
        .unwrap();
    app.collapse_selected();
    assert!(app.collapsed.contains(Path::new("src/nested")));
    app.expand_selected();
    assert_eq!(
        app.selected(),
        Some(&Target::Path("src/nested".into(), true))
    );
    app.expand_selected();
    assert_eq!(
        app.selected(),
        Some(&Target::Path("src/nested/new".into(), false))
    );
    app.collapse_selected();
    assert_eq!(
        app.selected(),
        Some(&Target::Path("src/nested".into(), true))
    );
    app.collapse_selected();
    app.collapse_selected();
    assert_eq!(app.selected(), Some(&Target::Path(PathBuf::new(), true)));
}

#[test]
fn partial_mutation_rejects_a_preview_from_the_previous_diff_side() {
    let fixture = Fixture::new();
    fixture.write("staged\n");
    git(fixture.directory.path(), &["add", "file.txt"]);
    fixture.write("working\n");
    let mut app = fixture.app();
    app.diff = Some(Arc::new(
        ExactDiff::load(&app.repo.root, Path::new("file.txt"), DiffSide::Worktree).unwrap(),
    ));
    app.update_rows();
    app.view.focus = Focus::Canvas;
    app.side = DiffSide::Index;
    app.action(Action::Unstage, &mut None, &mut None).unwrap();
    assert!(app.operation.is_none());
    assert!(app.content_pending);
    assert!(app.diff.is_none());
    collect_until(&mut app, |app| !app.content_pending);
    assert_eq!(app.diff.as_ref().unwrap().side, DiffSide::Index);
    let output = Command::new("git")
        .current_dir(&app.repo.root)
        .args(["show", ":file.txt"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"staged\n");
}

#[test]
fn conflicted_folder_preserves_access_to_other_staged_changes() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.snapshot.status.entries = vec![
        crate::git::StatusEntry {
            path: "src/conflicted".into(),
            original_path: None,
            index: 'U',
            worktree: 'U',
            unmerged: true,
            untracked: false,
            submodule: None,
        },
        crate::git::StatusEntry {
            path: "src/staged".into(),
            original_path: None,
            index: 'M',
            worktree: '.',
            unmerged: false,
            untracked: false,
            submodule: None,
        },
    ];
    app.rebuild();
    app.view.selected = 0;
    let state = app.selected_file_state().unwrap();
    assert!(state.conflicted && state.staged && state.unstaged);
    app.side = DiffSide::Index;
    app.align_diff_side();
    assert_eq!(app.side, DiffSide::Index);
    assert_eq!((app.nav_summary.staged, app.nav_summary.unstaged), (1, 1));
    app.view.selected = 1;
    app.align_diff_side();
    assert_eq!(app.side, DiffSide::Worktree);
    assert!(!app.selected_file_state().unwrap().staged);
}

#[test]
fn clicking_diff_scope_changes_only_preview_and_clears_staging_marks() {
    let fixture = Fixture::new();
    fixture.write("staged\n");
    git(fixture.directory.path(), &["add", "file.txt"]);
    fixture.write("working\n");
    let mut app = fixture.app();
    let selected = app.selected().cloned();
    let entries: Vec<_> = app
        .entries
        .iter()
        .map(|entry| entry.target.clone())
        .collect();
    let backend = ratatui::backend::TestBackend::new(120, 30);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    let data = RenderData {
        items: &app.nav_items,
        diff_scope: Some(view::DiffScope {
            side: DiffSide::Worktree,
            staged: true,
            unstaged: true,
        }),
        ..Default::default()
    };
    terminal
        .draw(|frame| {
            app.geometry = Some(view::render(frame, &mut app.view, &data));
        })
        .unwrap();
    let target = app
        .geometry
        .as_ref()
        .unwrap()
        .diff_sides
        .iter()
        .find(|(side, _)| *side == DiffSide::Index)
        .unwrap()
        .1;
    app.marked.insert(2);
    app.mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: target.x,
        row: target.y,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.side, DiffSide::Index);
    assert_eq!(app.selected(), selected.as_ref());
    assert_eq!(
        app.entries
            .iter()
            .map(|entry| entry.target.clone())
            .collect::<Vec<_>>(),
        entries
    );
    assert!(app.marked.is_empty());
    assert!(app.content_pending);
}

#[test]
fn directory_preview_refreshes_after_same_status_child_edit_without_directory_change() {
    let fixture = Fixture::new();
    let root = fixture.directory.path();
    let directory = root.join("src");
    let child = directory.join("tracked.txt");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(&child, "original\n").unwrap();
    git(root, &["add", "src/tracked.txt"]);
    git(root, &["commit", "-m", "directory"]);
    std::fs::write(&child, "firstxxx\n").unwrap();
    let mut app = fixture.app();
    app.presentation_config = Some(DiffPresentationConfig::default());
    app.view.selected = app
        .entries
        .iter()
        .position(|entry| entry.target == Target::Path("src".into(), true))
        .unwrap();
    app.load_content(true);
    collect_until(&mut app, |app| !app.content_pending);
    assert!(
        app.diff
            .as_ref()
            .unwrap()
            .raw
            .windows(b"firstxxx".len())
            .any(|bytes| bytes == b"firstxxx")
    );
    let old_key = app.loaded_preview.clone();
    let directory_stamp = path_stamp(&directory, false).unwrap();
    let old_status = app.snapshot.status.clone();
    std::fs::write(&child, "secondxx\n").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&child)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(2))
        .unwrap();
    assert_eq!(directory_stamp, path_stamp(&directory, false).unwrap());
    assert_eq!(old_status, fixture.repo.snapshot(300).unwrap().status);
    app.refresh_background();
    collect_until(&mut app, |app| {
        !app.snapshot_pending && !app.content_pending
    });
    assert_eq!(app.selected(), Some(&Target::Path("src".into(), true)));
    assert_ne!(old_key, app.loaded_preview);
    assert!(
        app.diff
            .as_ref()
            .unwrap()
            .raw
            .windows(b"secondxx".len())
            .any(|bytes| bytes == b"secondxx")
    );
}

#[test]
fn forced_refresh_queued_during_background_scan_rereads_presentation_config() {
    let fixture = Fixture::new();
    git(fixture.directory.path(), &["config", "core.pager", "cat"]);
    git(fixture.directory.path(), &["config", "pager.diff", "cat"]);
    let expected = crate::presentation::load_config(fixture.directory.path(), None).unwrap();
    let mut stale = expected.clone();
    stale.right_format.push_str("stale cached configuration");
    stale.split = !expected.split;
    let mut app = fixture.app();
    app.presentation_config = Some(stale.clone());
    app.view.split = stale.split;
    let data = Data {
        snapshot: app.snapshot.clone(),
        files: app.files.clone(),
        preview: Some(
            PreviewKey::capture(&app.repo, app.preview_request(), &app.snapshot, &app.files)
                .unwrap(),
        ),
        force: false,
        presentation_config: None,
    };
    let (started, ready) = crossbeam_channel::bounded(1);
    let (release, blocked) = crossbeam_channel::bounded(1);
    app.snapshot_worker.submit(move |_| {
        started.send(()).unwrap();
        blocked.recv().unwrap();
        Ok(data)
    });
    app.snapshot_pending = true;
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    app.refresh();
    assert!(app.refresh_forced);
    assert_eq!(app.presentation_config.as_ref(), Some(&stale));
    release.send(()).unwrap();
    collect_until(&mut app, |app| {
        !app.snapshot_pending && !app.content_pending
    });
    assert_eq!(app.presentation_config.as_ref(), Some(&expected));
    assert_eq!(app.view.split, expected.split);
    assert!(!app.refresh_forced);
    assert_eq!(
        app.loaded_preview.as_ref().unwrap().request.presentation,
        expected
    );
}

#[test]
fn wrapped_continuation_reflow_preserves_cursor_viewport_marks_and_exact_staging() {
    let fixture = Fixture::new();
    let source = format!("const value = \"{}\";\tEND", "åΩ界 abcdef ".repeat(35));
    let working = format!("original\n{source}\nneighbor\n");
    fixture.write(&working);
    let mut app = fixture.app();
    app.view.selected = app
        .entries
        .iter()
        .position(|entry| entry.target == Target::Path("file.txt".into(), false))
        .unwrap();
    app.presentation_config = Some(DiffPresentationConfig::default());
    app.diff = Some(Arc::new(
        ExactDiff::load(&app.repo.root, Path::new("file.txt"), DiffSide::Worktree).unwrap(),
    ));
    app.geometry = Some(Geometry {
        canvas: ratatui::layout::Rect::new(30, 5, 80, 12),
        ..Default::default()
    });
    app.view.focus = Focus::Canvas;
    app.view.split = true;
    app.split_override = Some(true);
    app.update_rows();
    let id = app
        .diff
        .as_ref()
        .unwrap()
        .hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .find(|line| line.content == format!("{source}\n").as_bytes())
        .unwrap()
        .id;
    let continuations: Vec<_> = app
        .row_map
        .iter()
        .enumerate()
        .filter(|(_, row)| row.right_line == Some(id) && row.right_offset > 0)
        .map(|(index, _)| index)
        .collect();
    assert!(continuations.len() > 5);
    app.view.diff_cursor = continuations[4];
    app.view.diff_offset = continuations[2];
    app.view.canvas_manual_scroll = true;
    let cursor = app.row_anchor(app.view.diff_cursor).unwrap();
    let viewport = app.row_anchor(app.view.diff_offset).unwrap();
    app.mark_current(true);
    app.move_cursor(1, true);
    assert_eq!(app.marked, BTreeSet::from([id]));
    app.view.diff_cursor = continuations[4];
    app.view.diff_offset = continuations[2];
    app.view.canvas_manual_scroll = true;
    app.geometry.as_mut().unwrap().canvas.width = 120;
    app.update_rows();
    assert_eq!(app.row_width, 120);
    assert_eq!(app.view.diff_cursor, app.row_for_anchor(cursor).unwrap());
    assert_eq!(app.view.diff_offset, app.row_for_anchor(viewport).unwrap());
    assert_eq!(
        app.row_anchor(app.view.diff_cursor)
            .unwrap()
            .source
            .unwrap()
            .0,
        id
    );
    assert_eq!(
        app.row_anchor(app.view.diff_offset)
            .unwrap()
            .source
            .unwrap()
            .0,
        id
    );
    assert!(app.view.canvas_manual_scroll);
    assert_eq!(app.diff_counts, (2, 0));
    assert_eq!(app.marked, BTreeSet::from([id]));
    app.action(Action::ToggleSplit, &mut None, &mut None)
        .unwrap();
    assert!(!app.row_split);
    assert_eq!(
        app.row_anchor(app.view.diff_cursor)
            .unwrap()
            .source
            .unwrap()
            .0,
        id
    );
    app.action(Action::ToggleSplit, &mut None, &mut None)
        .unwrap();
    collect_until(&mut app, |app| !app.content_pending);
    assert!(app.row_split);
    assert_eq!(
        app.row_anchor(app.view.diff_cursor)
            .unwrap()
            .source
            .unwrap()
            .0,
        id
    );
    assert_eq!(app.marked, BTreeSet::from([id]));
    app.action(Action::Stage, &mut None, &mut None).unwrap();
    collect_until(&mut app, |app| {
        app.operation.is_none() && !app.snapshot_pending && !app.content_pending
    });
    let index = Command::new("git")
        .current_dir(&app.repo.root)
        .args(["show", ":file.txt"])
        .output()
        .unwrap();
    assert!(index.status.success());
    assert_eq!(index.stdout, format!("original\n{source}\n").as_bytes());
    assert_eq!(
        std::fs::read_to_string(app.repo.root.join("file.txt")).unwrap(),
        working
    );
}

#[test]
fn unlimited_split_wrapping_prevents_horizontal_keyboard_and_mouse_scrolling() {
    let fixture = Fixture::new();
    fixture.write(&format!("original\n{}\n", "long content ".repeat(60)));
    let mut app = fixture.app();
    app.diff = Some(Arc::new(
        ExactDiff::load(&app.repo.root, Path::new("file.txt"), DiffSide::Worktree).unwrap(),
    ));
    app.geometry = Some(Geometry {
        canvas: ratatui::layout::Rect::new(30, 5, 90, 12),
        ..Default::default()
    });
    app.presentation_config = Some(DiffPresentationConfig::default());
    app.view.focus = Focus::Canvas;
    app.view.split = true;
    app.view.horizontal = 30;
    app.update_rows();
    assert_eq!(app.view.horizontal, 0);
    app.key(
        KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        &mut None,
        &mut None,
    )
    .unwrap();
    app.mouse(MouseEvent {
        kind: MouseEventKind::ScrollRight,
        column: 40,
        row: 8,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.view.horizontal, 0);
    app.presentation_config.as_mut().unwrap().split_wrap =
        crate::presentation::WrapPolicy::Disabled;
    app.update_rows();
    app.key(
        KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        &mut None,
        &mut None,
    )
    .unwrap();
    assert_eq!(app.view.horizontal, 8);
    app.mouse(MouseEvent {
        kind: MouseEventKind::ScrollRight,
        column: 40,
        row: 8,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.view.horizontal, 9);
}

#[test]
fn changes_tree_uses_one_shared_root_and_compacts_directory_chains() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    for (paths, expected) in [
        (
            vec![
                "scripts/rust/crates/dclip/Cargo.toml",
                "scripts/rust/crates/dclip/src/mode.rs",
            ],
            vec![
                ("scripts/rust/crates/dclip", 0),
                ("Cargo.toml", 1),
                ("src", 1),
                ("mode.rs", 2),
            ],
        ),
        (
            vec!["src/one/two/file.rs", "src/other/deep/test.rs"],
            vec![
                ("src", 0),
                ("one/two", 1),
                ("file.rs", 2),
                ("other/deep", 1),
                ("test.rs", 2),
            ],
        ),
        (
            vec!["src/only/one.rs"],
            vec![("src/only", 0), ("one.rs", 1)],
        ),
    ] {
        app.snapshot.status.entries = paths
            .iter()
            .map(|path| crate::git::StatusEntry {
                path: path.into(),
                original_path: None,
                index: '.',
                worktree: 'M',
                unmerged: false,
                untracked: false,
                submodule: None,
            })
            .collect();
        app.rebuild();
        assert_eq!(
            app.entries
                .iter()
                .map(|entry| (entry.item.label.as_str(), entry.item.depth))
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            app.entries
                .iter()
                .filter(|entry| entry.item.depth == 0)
                .count(),
            1
        );
        assert_eq!(app.nav_summary.items, paths.len());
        for path in paths {
            assert!(
                app.entries
                    .iter()
                    .any(|entry| entry.target == Target::Path(path.into(), false))
            );
        }
    }
    app.query = "one.rs".into();
    app.rebuild();
    assert_eq!(app.entries[0].item.label, "src/only");
    app.query = "missing".into();
    app.rebuild();
    assert_eq!(app.entries.len(), 1);
    assert_eq!(app.entries[0].target, Target::Path(PathBuf::new(), true));
}

#[test]
fn space_stages_mixed_content_then_unstages_without_changing_the_worktree() {
    let fixture = Fixture::new();
    fixture.write("staged version\n");
    git(fixture.directory.path(), &["add", "file.txt"]);
    fixture.write("working version\n");
    let mut app = fixture.app();
    let target = app.selected().cloned();
    assert!(
        app.selected_file_state().unwrap().staged && app.selected_file_state().unwrap().unstaged
    );
    for staged in [true, false] {
        app.key(
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            &mut None,
            &mut None,
        )
        .unwrap();
        collect_until(&mut app, |app| {
            app.operation.is_none() && !app.snapshot_pending && !app.content_pending
        });
        assert_eq!(app.selected(), target.as_ref());
        let state = app.selected_file_state().unwrap();
        assert_eq!((state.staged, state.unstaged), (staged, !staged));
        assert_eq!(
            std::fs::read_to_string(fixture.directory.path().join("file.txt")).unwrap(),
            "working version\n"
        );
    }
}

#[test]
fn repository_root_previews_and_space_toggles_all_changed_files() {
    let fixture = Fixture::new();
    fixture.write("root change\n");
    std::fs::create_dir_all(fixture.directory.path().join("nested/deep")).unwrap();
    std::fs::write(
        fixture.directory.path().join("nested/deep/new"),
        "new content\n",
    )
    .unwrap();
    let mut app = fixture.app();
    app.view.selected = 0;
    assert_eq!(app.selected(), Some(&Target::Path(PathBuf::new(), true)));
    app.load_content(true);
    collect_until(&mut app, |app| !app.content_pending);
    let diff = app.diff.as_ref().unwrap();
    let patch = String::from_utf8_lossy(&diff.raw);
    assert!(patch.contains("root change") && patch.contains("new content"));
    for staged in [true, false] {
        app.key(
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            &mut None,
            &mut None,
        )
        .unwrap();
        collect_until(&mut app, |app| {
            app.operation.is_none() && !app.snapshot_pending && !app.content_pending
        });
        assert_eq!(app.nav_summary.items, 2);
        assert_eq!(
            (app.nav_summary.staged, app.nav_summary.unstaged),
            if staged { (2, 0) } else { (0, 2) }
        );
        assert_eq!(app.selected(), Some(&Target::Path(PathBuf::new(), true)));
    }
}

#[test]
fn space_on_a_filtered_root_only_stages_matching_changes() {
    let fixture = Fixture::new();
    fixture.write("visible change\n");
    std::fs::write(
        fixture.directory.path().join("hidden.txt"),
        "hidden change\n",
    )
    .unwrap();
    let mut app = fixture.app();
    app.query = "file.txt".into();
    app.rebuild();
    app.view.selected = 0;
    app.key(
        KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        &mut None,
        &mut None,
    )
    .unwrap();
    collect_until(&mut app, |app| {
        app.operation.is_none() && !app.snapshot_pending && !app.content_pending
    });
    let snapshot = fixture.repo.snapshot(300).unwrap();
    assert!(
        snapshot
            .status
            .entries
            .iter()
            .find(|entry| entry.path == Path::new("file.txt"))
            .unwrap()
            .staged()
    );
    assert!(
        snapshot
            .status
            .entries
            .iter()
            .find(|entry| entry.path == Path::new("hidden.txt"))
            .unwrap()
            .untracked
    );
}

#[test]
fn reopening_changes_starts_at_the_root_and_command_arrows_reach_both_edges() {
    let fixture = Fixture::new();
    fixture.write("changed\n");
    std::fs::write(fixture.directory.path().join("last.txt"), "new\n").unwrap();
    for _ in 0..2 {
        let mut app = fixture.app();
        assert_eq!(app.view.selected, 0);
        assert_eq!(app.selected(), Some(&Target::Path(PathBuf::new(), true)));
        for (key, expected) in [(KeyCode::Down, app.entries.len() - 1), (KeyCode::Up, 0)] {
            app.view.nav_manual_scroll = true;
            app.key(
                KeyEvent::new(key, KeyModifiers::SUPER),
                &mut None,
                &mut None,
            )
            .unwrap();
            assert_eq!(app.view.selected, expected);
            assert!(!app.view.nav_manual_scroll);
            assert!(app.operation.is_none());
        }
        app.key(
            KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
            &mut None,
            &mut None,
        )
        .unwrap();
    }
}

#[test]
fn shift_space_toggles_all_current_changes_independent_of_cursor_and_search() {
    let fixture = Fixture::new();
    fixture.write("staged\n");
    git(fixture.directory.path(), &["add", "file.txt"]);
    fixture.write("mixed\n");
    let mut app = fixture.app();
    app.query = "file.txt".into();
    app.rebuild();
    app.view.selected = 1;
    // This change is absent from both the filtered tree and its last snapshot.
    std::fs::write(fixture.directory.path().join("hidden.txt"), "hidden\n").unwrap();
    let target = app.selected().cloned();
    for staged in [true, false] {
        app.key(
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::SHIFT),
            &mut None,
            &mut None,
        )
        .unwrap();
        collect_until(&mut app, |app| {
            app.operation.is_none() && !app.snapshot_pending && !app.content_pending
        });
        let status = fixture.repo.status().unwrap();
        assert_eq!(status.entries.len(), 2);
        assert!(
            status
                .entries
                .iter()
                .all(|entry| entry.staged() == staged && entry.unstaged() != staged)
        );
        assert_eq!(app.selected(), target.as_ref());
        assert_eq!(
            std::fs::read_to_string(fixture.directory.path().join("file.txt")).unwrap(),
            "mixed\n"
        );
        assert_eq!(
            std::fs::read_to_string(fixture.directory.path().join("hidden.txt")).unwrap(),
            "hidden\n"
        );
    }
    app.key(
        KeyEvent::new_with_kind(
            KeyCode::Char(' '),
            KeyModifiers::SHIFT,
            KeyEventKind::Repeat,
        ),
        &mut None,
        &mut None,
    )
    .unwrap();
    assert!(app.operation.is_none());
    app.dialog = Some(Dialog::Input {
        prompt: Prompt::Search,
        title: "Search".into(),
        value: String::new(),
    });
    app.key(
        KeyEvent::new(KeyCode::Char(' '), KeyModifiers::SHIFT),
        &mut None,
        &mut None,
    )
    .unwrap();
    assert!(matches!(app.dialog, Some(Dialog::Input { ref value, .. }) if value == " "));
    assert!(app.operation.is_none());
}

#[test]
fn inline_commit_accepts_normal_text_editing_without_activating_workbench_shortcuts() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.action(Action::Compose, &mut None, &mut None).unwrap();
    app.paste("Hello wrld\r\n\r\nBody å e\u{301} 👩‍💻");
    let keys = [
        KeyEvent::new(KeyCode::Home, KeyModifiers::CONTROL),
        KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
        KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL),
        KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL),
        KeyEvent::new(KeyCode::Char(' '), KeyModifiers::SHIFT),
        KeyEvent::new(
            KeyCode::Char('@'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        ),
    ];
    for key in keys {
        app.key(key, &mut None, &mut None).unwrap();
    }
    let Some(Dialog::Compose { editor }) = &app.dialog else {
        panic!("inline editor closed");
    };
    assert_eq!(editor.as_str(), "Hello world\n\nBody å e\u{301} 👩‍💻 @");
    assert_eq!(app.view.workspace, Workspace::Changes);
    assert!(app.operation.is_none());
    app.key(
        KeyEvent::new_with_kind(
            KeyCode::Char('!'),
            KeyModifiers::SHIFT,
            KeyEventKind::Release,
        ),
        &mut None,
        &mut None,
    )
    .unwrap();
    let Some(Dialog::Compose { editor }) = &app.dialog else {
        panic!();
    };
    assert!(!editor.as_str().ends_with('!'));
    app.key(
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
        &mut None,
        &mut None,
    )
    .unwrap();
    assert!(app.composing(), "Ctrl+S must not commit");
    assert!(app.operation.is_none());
    app.operation = Some(PendingOperation {
        label: "Fetch".into(),
        started: Instant::now(),
    });
    app.key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut None,
        &mut None,
    )
    .unwrap();
    assert!(matches!(app.dialog, Some(Dialog::Compose { .. })));
    app.operation = None;
    app.key(
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        &mut None,
        &mut None,
    )
    .unwrap();
    assert!(!app.composing());
}

#[test]
fn inline_editing_supports_selection_word_motion_delete_and_terminal_shortcut_variants() {
    let mut editor = ui_widgets::EditorBuffer::new("one two three");
    for key in [
        KeyEvent::new(KeyCode::Left, KeyModifiers::ALT),
        KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
        KeyEvent::new(KeyCode::Char('X'), KeyModifiers::SHIFT),
        KeyEvent::new(KeyCode::Left, KeyModifiers::SUPER),
        KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL),
    ] {
        editor::edit_key(&mut editor, key, 80);
    }
    assert_eq!(editor.as_str(), "one two X");
    editor::edit_key(
        &mut editor,
        KeyEvent::new(KeyCode::Right, KeyModifiers::SUPER),
        80,
    );
    assert_eq!(editor.cursor(), editor.as_str().len());
    editor::edit_key(
        &mut editor,
        KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT),
        80,
    );
    editor::edit_key(
        &mut editor,
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT),
        80,
    );
    assert_eq!(editor.as_str(), "one two ");
}

#[test]
fn inline_enter_commits_but_modified_enter_and_paste_keep_multiline_drafts() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.action(Action::Compose, &mut None, &mut None).unwrap();
    app.key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut None,
        &mut None,
    )
    .unwrap();
    assert!(app.composing());
    assert!(app.operation.is_none());
    app.paste("Subject");
    app.key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
        &mut None,
        &mut None,
    )
    .unwrap();
    app.key(
        KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
        &mut None,
        &mut None,
    )
    .unwrap();
    app.paste("Body\nMore");
    app.key(
        KeyEvent::new_with_kind(KeyCode::Enter, KeyModifiers::NONE, KeyEventKind::Repeat),
        &mut None,
        &mut None,
    )
    .unwrap();
    let Some(Dialog::Compose { editor }) = &app.dialog else {
        panic!("draft submitted early")
    };
    assert_eq!(editor.as_str(), "Subject\n\nBody\nMore");
    assert!(app.operation.is_none());
    app.key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut None,
        &mut None,
    )
    .unwrap();
    assert!(!app.composing());
    assert!(app.operation.is_some());
}

#[test]
fn diff_s_and_u_switch_views_without_staging_files_or_marked_lines() {
    let fixture = Fixture::new();
    fixture.write("staged\n");
    git(fixture.directory.path(), &["add", "file.txt"]);
    fixture.write("working\n");
    let index = std::fs::read(fixture.directory.path().join(".git/index")).unwrap();
    let mut app = fixture.app();
    app.view.focus = Focus::Canvas;
    let selected = app.selected().cloned();
    for (key, side) in [
        ('s', DiffSide::Index),
        ('u', DiffSide::Worktree),
        ('s', DiffSide::Index),
        ('s', DiffSide::Index),
        ('u', DiffSide::Worktree),
    ] {
        app.marked.insert(1);
        app.key(
            KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE),
            &mut None,
            &mut None,
        )
        .unwrap();
        collect_until(&mut app, |app| !app.content_pending);
        assert_eq!(app.side, side);
        assert_eq!(app.diff.as_ref().unwrap().side, side);
        assert_eq!(app.selected(), selected.as_ref());
        assert!(app.operation.is_none());
        assert_eq!(
            std::fs::read(fixture.directory.path().join(".git/index")).unwrap(),
            index
        );
        assert_eq!(
            std::fs::read_to_string(fixture.directory.path().join("file.txt")).unwrap(),
            "working\n"
        );
    }
}
