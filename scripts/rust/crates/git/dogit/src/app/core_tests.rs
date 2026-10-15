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
        prompt: Prompt::Compose,
        title: "Commit".into(),
        value: "keep this draft".into(),
    });
    app.key(
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
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
    assert!(app.entries.is_empty());
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
fn operation_completion_preserves_selected_record_with_and_without_search() {
    let fixture = Fixture::new();
    for query in ["", "keep"] {
        let mut app = fixture.app();
        app.saved.operations = ["newer", "keep selected", "older"]
            .into_iter()
            .enumerate()
            .map(|(timestamp, action)| OperationRecord {
                action: action.into(),
                outcome: action.into(),
                detail: format!("{action} output"),
                failed: false,
                timestamp: timestamp as u64,
            })
            .collect();
        app.view.workspace = Workspace::Operations;
        app.query = query.into();
        app.rebuild();
        app.view.selected = app
            .entries
            .iter()
            .position(|entry| entry.target == Target::Operation(1))
            .unwrap();
        app.record("Fetch", Ok("Fetched".into()));
        assert_eq!(app.selected(), Some(&Target::Operation(2)));
        assert_eq!(app.text, ["keep selected output"]);
        assert_eq!(app.entries[app.view.selected].item.label, "keep selected");
    }
}

#[test]
fn tab_cycles_workspaces_in_both_directions_and_wraps_without_dialogs() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    for expected in Workspace::ALL
        .into_iter()
        .cycle()
        .skip(1)
        .take(Workspace::ALL.len())
    {
        app.key(
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &mut None,
            &mut None,
        )
        .unwrap();
        assert_eq!(app.view.workspace, expected);
        assert!(app.dialog.is_none());
    }
    for expected in Workspace::ALL.into_iter().rev() {
        app.key(
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            &mut None,
            &mut None,
        )
        .unwrap();
        assert_eq!(app.view.workspace, expected);
        assert!(app.dialog.is_none());
    }
    app.key(
        KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT),
        &mut None,
        &mut None,
    )
    .unwrap();
    assert_eq!(app.view.workspace, Workspace::Operations);
}

#[test]
fn number_keys_focus_existing_panes_without_changing_workspace_or_position() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    for workspace in Workspace::ALL {
        app.view.workspace = workspace;
        app.view.selected = 3;
        app.view.diff_cursor = 7;
        app.view.horizontal = 8;
        app.key(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &mut None,
            &mut None,
        )
        .unwrap();
        assert_eq!(app.view.focus, Focus::Canvas);
        for number in '3'..='9' {
            app.key(
                KeyEvent::new(KeyCode::Char(number), KeyModifiers::NONE),
                &mut None,
                &mut None,
            )
            .unwrap();
            assert_eq!(app.view.focus, Focus::Canvas);
            assert_eq!(app.view.workspace, workspace);
            assert!(app.dialog.is_none());
        }
        app.key(
            KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE),
            &mut None,
            &mut None,
        )
        .unwrap();
        assert_eq!(app.view.focus, Focus::Navigator);
        assert_eq!(app.view.workspace, workspace);
        assert_eq!(app.view.selected, 3);
        assert_eq!(app.view.diff_cursor, 7);
        assert_eq!(app.view.horizontal, 8);
    }
}

#[test]
fn explicit_shifted_number_events_select_workspaces() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    for (number, workspace) in ('1'..='7').zip(Workspace::ALL) {
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
fn shifted_numbers_and_symbols_remain_text_in_compose_and_search() {
    let fixture = Fixture::new();
    for prompt in [Prompt::Compose, Prompt::Search] {
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
            prompt: Prompt::Compose,
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
