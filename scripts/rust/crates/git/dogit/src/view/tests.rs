use super::*;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn draw(
    width: u16,
    height: u16,
    state: &mut ViewState,
    data: &RenderData<'_>,
) -> (Buffer, Geometry) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    let mut geometry = Geometry::default();
    terminal
        .draw(|frame| geometry = render(frame, state, data))
        .unwrap();
    (terminal.backend().buffer().clone(), geometry)
}

fn draw_tabs_at(
    state: &mut ViewState,
    now: Instant,
    reduced_motion: bool,
) -> (Buffer, Vec<(Workspace, Rect)>) {
    let mut terminal = Terminal::new(TestBackend::new(120, 4)).unwrap();
    let mut tabs = Vec::new();
    terminal
        .draw(|frame| {
            tabs = chrome::render_tabs_at(frame, frame.area(), state, now, reduced_motion);
        })
        .unwrap();
    (terminal.backend().buffer().clone(), tabs)
}

fn text(buffer: &Buffer) -> String {
    buffer.content.iter().map(|cell| cell.symbol()).collect()
}

fn area_text(buffer: &Buffer, area: Rect) -> String {
    (area.y..area.bottom())
        .map(|y| {
            (area.x..area.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_terminal_background_except_badge(buffer: &Buffer) {
    for y in buffer.area.y..buffer.area.bottom() {
        for x in buffer.area.x..buffer.area.right() {
            let cell = &buffer[(x, y)];
            if cell.bg != Color::Reset {
                assert_eq!(
                    y,
                    buffer.area.bottom() - 1,
                    "unexpected background at {x},{y}"
                );
            }
            assert!(!cell.modifier.contains(Modifier::REVERSED));
        }
    }
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn diff_fixture() -> (tempfile::TempDir, ExactDiff) {
    let root = tempfile::tempdir().unwrap();
    git(root.path(), &["init", "-q"]);
    git(root.path(), &["config", "user.name", "Test"]);
    git(
        root.path(),
        &["config", "user.email", "test@example.invalid"],
    );
    fs::write(
        root.path().join("main.rs"),
        "fn main() {\n    old();\n    removed();\n}\n",
    )
    .unwrap();
    git(root.path(), &["add", "--", "main.rs"]);
    git(
        root.path(),
        &["-c", "commit.gpgsign=false", "commit", "-qm", "Initial"],
    );
    fs::write(root.path().join("main.rs"), "fn main() {\n    new();\n}\n").unwrap();
    let diff = ExactDiff::load(
        root.path(),
        Path::new("main.rs"),
        crate::diff::DiffSide::Worktree,
    )
    .unwrap();
    (root, diff)
}

fn commit(id: &str, parents: &[&str]) -> crate::git::Commit {
    crate::git::Commit {
        oid: id.into(),
        parents: parents.iter().map(|value| (*value).into()).collect(),
        author: "Test".into(),
        timestamp: 0,
        subject: id.into(),
        decorations: String::new(),
    }
}

#[test]
fn resize_preserves_selection_and_reveals_the_focused_pane() {
    let items = (0..80)
        .map(|index| NavItem {
            label: format!("file-{index}.rs"),
            ..NavItem::default()
        })
        .collect::<Vec<_>>();
    let data = RenderData {
        repo: "dogit",
        branch: "main",
        items: &items,
        canvas_title: "Selected patch",
        ..RenderData::default()
    };
    let mut state = ViewState {
        selected: 61,
        ..ViewState::default()
    };
    let (_, wide) = draw(140, 35, &mut state, &data);
    assert!(wide.divider_x.is_some());
    assert_eq!(state.selected, 61);
    assert!(wide.nav_start <= 61 && wide.nav_start + usize::from(wide.nav.height) > 61);
    let (_, narrow) = draw(60, 18, &mut state, &data);
    assert!(narrow.canvas.is_empty());
    assert!(!narrow.nav.is_empty());
    assert_eq!(state.selected, 61);
    state.focus = Focus::Canvas;
    let (_, canvas) = draw(60, 18, &mut state, &data);
    assert!(canvas.nav.is_empty());
    assert!(!canvas.canvas.is_empty());
    let (_, restored) = draw(140, 35, &mut state, &data);
    assert_eq!(restored.nav.width, 34);
    assert_eq!(state.selected, 61);
}

#[test]
fn very_small_terminals_and_dialogs_do_not_panic_or_escape_the_buffer() {
    let items = vec![NavItem {
        label: "directory with a long name".into(),
        kind: ItemKind::Folder,
        ..NavItem::default()
    }];
    let data = RenderData {
        repo: "repository",
        branch: "feature/long-branch",
        items: &items,
        ..RenderData::default()
    };
    for width in [0, 1, 19, 20, 21, 40, 79, 80, 160] {
        for height in [0, 1, 6, 7, 8, 10, 30] {
            for focus in [Focus::Canvas, Focus::Navigator] {
                let mut state = ViewState {
                    focus,
                    nav_width: u16::MAX,
                    overlay: Some(Overlay::Confirm {
                        title: "Discard selected file".into(),
                        detail: "src/main.rs\nOnly unstaged changes".into(),
                        destructive: true,
                        scroll: 0,
                    }),
                    ..ViewState::default()
                };
                let (buffer, _) = draw(width, height, &mut state, &data);
                assert_eq!(
                    buffer.content.len(),
                    usize::from(width) * usize::from(height)
                );
            }
        }
    }
}

#[test]
fn exact_line_ids_follow_visible_unified_and_split_rows() {
    let (_root, diff) = diff_fixture();
    let data = RenderData {
        diff: Some(&diff),
        ..RenderData::default()
    };
    let mut state = ViewState {
        focus: Focus::Canvas,
        split: true,
        ..ViewState::default()
    };
    let (_, split) = draw(160, 30, &mut state, &data);
    let rows = diff_rows(&diff, true);
    assert_eq!(
        split.line_ids,
        rows.iter().map(|row| row.line).collect::<Vec<_>>()
    );
    assert_eq!(
        split.right_line_ids,
        rows.iter().map(|row| row.right_line).collect::<Vec<_>>()
    );
    assert!(
        rows.iter()
            .any(|row| row.line.is_some() && row.right_line.is_none())
    );
    let cached_unified = diff_rows(&diff, false);
    let stale_mode = RenderData {
        diff: Some(&diff),
        diff_rows: Some((&cached_unified, false)),
        ..RenderData::default()
    };
    let (_, resized) = draw(160, 30, &mut state, &stale_mode);
    assert_eq!(resized.right_line_ids, split.right_line_ids);
    let cached_split = RenderData {
        diff: Some(&diff),
        diff_rows: Some((&rows, true)),
        ..RenderData::default()
    };
    let (_, cached) = draw(160, 30, &mut state, &cached_split);
    assert_eq!(cached.line_ids, split.line_ids);
    let (_, unified) = draw(90, 30, &mut state, &data);
    let rows = diff_rows(&diff, false);
    assert_eq!(
        unified.line_ids,
        rows.iter().map(|row| row.line).collect::<Vec<_>>()
    );
    assert!(unified.right_line_ids.iter().all(Option::is_none));
    let changes = diff
        .hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .filter(|line| line.kind != LineKind::Context)
        .map(|line| line.id)
        .collect::<BTreeSet<_>>();
    let visible = split
        .line_ids
        .iter()
        .chain(&split.right_line_ids)
        .filter_map(|id| *id)
        .collect::<BTreeSet<_>>();
    assert!(
        changes.is_subset(&visible),
        "every change must remain individually selectable"
    );
}

#[test]
fn selected_diff_lines_retain_their_semantic_color_and_terminal_background() {
    let (_root, diff) = diff_fixture();
    let added = diff
        .hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .find(|line| line.kind == LineKind::Addition)
        .unwrap();
    let marked = BTreeSet::from([added.id]);
    let data = RenderData {
        diff: Some(&diff),
        selected_lines: &marked,
        ..RenderData::default()
    };
    let mut state = ViewState {
        focus: Focus::Canvas,
        ..ViewState::default()
    };
    let (buffer, geometry) = draw(130, 30, &mut state, &data);
    let row = geometry
        .line_ids
        .iter()
        .position(|id| *id == Some(added.id))
        .unwrap() as u16;
    let mut cells =
        (geometry.canvas.x..geometry.canvas.right()).map(|x| &buffer[(x, geometry.canvas.y + row)]);
    assert!(cells.any(|cell| cell.symbol() == "n"
        && cell.fg == palette().foreground(Role::DiffAdded).ratatui()
        && cell.modifier.contains(Modifier::BOLD)));
    assert!(buffer.content.iter().all(|cell| cell.bg == Color::Reset));
    assert!(
        buffer
            .content
            .iter()
            .all(|cell| !cell.modifier.contains(Modifier::DIM))
    );
    assert!(!text(&buffer).contains('·'));
}

#[test]
fn graph_preserves_lanes_and_connects_merge_and_join() {
    let rows = graph_rows(&[
        commit("merge", &["left", "right"]),
        commit("left", &["root"]),
        commit("right", &["root"]),
        commit("root", &[]),
    ]);
    assert_eq!(rows, ["●─┐ ", "● │ ", "├─● ", "● "]);
    let disconnected = graph_rows(&[
        commit("one", &["base"]),
        commit("other", &["else"]),
        commit("base", &[]),
        commit("else", &[]),
    ]);
    assert_eq!(disconnected, ["● ", "│ ● ", "● │ ", "  ● "]);
}

#[test]
fn structural_output_cannot_change_background_or_inject_terminal_controls() {
    let raw = "\x1b]52;c;ZXhmaWw=\x07\x1b[48;2;255;0;0m\x1b[38;2;10;240;30maddition\x1b[0m\x1b[7m plain\x1b[?25l\x1bPignored\x1b\\";
    let parsed = ansi_line(raw);
    assert_eq!(
        parsed
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>(),
        "addition plain"
    );
    assert_eq!(
        parsed.spans[0].style.fg,
        Some(palette().foreground(Role::DiffAdded).ratatui())
    );
    assert!(
        parsed
            .spans
            .iter()
            .all(|span| !span.style.add_modifier.contains(Modifier::REVERSED))
    );
    let data = RenderData {
        structural: Some(raw),
        ..RenderData::default()
    };
    let (buffer, _) = draw(110, 25, &mut ViewState::default(), &data);
    assert!(buffer.content.iter().all(|cell| cell.bg == Color::Reset));
    assert!(!text(&buffer).contains("ignored"));
    assert!(!text(&buffer).contains("52;c"));
}

#[test]
fn command_search_keeps_selected_result_visible() {
    let items = (0..30)
        .map(|index| NavItem {
            label: format!("Action {index}"),
            detail: "key".into(),
            ..NavItem::default()
        })
        .collect();
    let mut state = ViewState {
        overlay: Some(Overlay::Commands {
            query: "Action".into(),
            items,
            selected: 24,
        }),
        ..ViewState::default()
    };
    let (buffer, _) = draw(120, 35, &mut state, &RenderData::default());
    assert!(text(&buffer).contains("Action 24"));
    assert!(!text(&buffer).contains("Action 0 "));
}

#[test]
fn loading_and_directory_selection_do_not_claim_the_working_tree_is_clean() {
    let items = vec![NavItem {
        label: "src".into(),
        kind: ItemKind::Folder,
        ..NavItem::default()
    }];
    let data = RenderData {
        items: &items,
        ..RenderData::default()
    };
    let (buffer, _) = draw(120, 30, &mut ViewState::default(), &data);
    assert!(!text(&buffer).contains("clean working tree"));
    let data = RenderData {
        canvas_subtitle: "Loading",
        ..RenderData::default()
    };
    let (buffer, _) = draw(120, 30, &mut ViewState::default(), &data);
    assert!(!text(&buffer).contains("clean working tree"));
}

#[test]
fn composer_keeps_staged_patch_visible_and_places_empty_body_cursor_correctly() {
    let (_root, diff) = diff_fixture();
    let data = RenderData {
        diff: Some(&diff),
        ..RenderData::default()
    };
    let mut state = ViewState {
        overlay: Some(Overlay::Compose {
            summary: "Review changes".into(),
            body: String::new(),
            body_started: false,
        }),
        ..ViewState::default()
    };
    let mut terminal = Terminal::new(TestBackend::new(130, 36)).unwrap();
    terminal
        .draw(|frame| {
            render(frame, &mut state, &data);
        })
        .unwrap();
    let summary_cursor = terminal.get_cursor_position().unwrap();
    let contents = text(terminal.backend().buffer());
    assert!(contents.contains("fn main()"));
    assert!(contents.contains("Ctrl+S  Create commit"));
    state.overlay = Some(Overlay::Compose {
        summary: "Review changes".into(),
        body: String::new(),
        body_started: true,
    });
    terminal
        .draw(|frame| {
            render(frame, &mut state, &data);
        })
        .unwrap();
    let body_cursor = terminal.get_cursor_position().unwrap();
    assert_eq!(body_cursor.y, summary_cursor.y + 2);
    assert!(body_cursor.x < summary_cursor.x);
}

#[test]
fn sixteen_color_terminals_keep_semantic_colors_distinct() {
    let foregrounds = [
        Role::Plain,
        Role::Accent,
        Role::DiffAdded,
        Role::DiffRemoved,
        Role::Warning,
    ]
    .map(|role| style_at_depth(role, ColorDepth::Ansi16).fg.unwrap());
    for (index, color) in foregrounds.iter().enumerate() {
        assert!(!foregrounds[..index].contains(color));
    }
}

#[test]
fn confirmation_exposes_the_last_scoped_file_and_consequences_when_scrolled() {
    let paths = (0..30)
        .map(|index| format!("src/file-{index:02}.rs"))
        .collect::<Vec<_>>();
    let detail = format!(
        "Discard unstaged content in 30 files:\n{}\n\nStaged content is preserved.",
        paths.join("\n")
    );
    let mut state = ViewState {
        overlay: Some(Overlay::Confirm {
            title: "Discard selected changes".into(),
            detail: detail.clone(),
            destructive: true,
            scroll: 0,
        }),
        ..ViewState::default()
    };
    let (first, geometry) = draw(120, 18, &mut state, &RenderData::default());
    assert!(text(&first).contains("src/file-00.rs"));
    assert!(!text(&first).contains("src/file-29.rs"));
    assert!(text(&first).contains(" / 33"));
    let maximum = confirmation_scroll_max(&detail, geometry.body);
    assert!(maximum > 0 && maximum < 33);
    state.overlay = Some(Overlay::Confirm {
        title: "Discard selected changes".into(),
        detail,
        destructive: true,
        scroll: maximum,
    });
    let (last, _) = draw(120, 18, &mut state, &RenderData::default());
    assert!(text(&last).contains("src/file-29.rs"));
    assert!(text(&last).contains("Staged content is preserved."));
    assert!(text(&last).contains("Enter to confirm"));
    if let Some(Overlay::Confirm { scroll, .. }) = &mut state.overlay {
        *scroll -= 1;
    }
    let (up, _) = draw(120, 18, &mut state, &RenderData::default());
    assert_ne!(text(&last), text(&up), "Up must move immediately after End");
}

#[test]
fn empty_workspaces_leave_the_content_panes_blank() {
    for workspace in Workspace::ALL {
        for width in [60, 120] {
            for focus in [Focus::Navigator, Focus::Canvas] {
                let mut state = ViewState {
                    workspace,
                    focus,
                    ..ViewState::default()
                };
                let (buffer, geometry) = draw(width, 25, &mut state, &RenderData::default());
                assert!(area_text(&buffer, geometry.nav).trim().is_empty());
                assert!(area_text(&buffer, geometry.canvas).trim().is_empty());
                assert!(!text(&buffer).contains("dogit"));
            }
        }
    }
    let (buffer, _) = draw(19, 6, &mut ViewState::default(), &RenderData::default());
    assert!(!text(&buffer).contains("dogit"));
    assert!(!text(&buffer).contains("room"));
}

#[test]
fn every_active_tab_remains_visible_and_clickable_at_narrow_widths() {
    for width in [20, 39, 60, 79, 80, 100, 140] {
        for workspace in Workspace::ALL {
            let mut state = ViewState {
                workspace,
                ..ViewState::default()
            };
            let (buffer, geometry) = draw(width, 25, &mut state, &RenderData::default());
            let active = geometry
                .tabs
                .iter()
                .find(|(tab, _)| *tab == workspace)
                .expect("the active workspace must always have a tab target");
            assert!(area_text(&buffer, active.1).contains(workspace.label()));
            for (index, (_, rect)) in geometry.tabs.iter().enumerate() {
                assert!(!rect.is_empty());
                assert!(rect.x >= buffer.area.x && rect.right() <= buffer.area.right());
                assert!(rect.bottom() <= geometry.body.y);
                for (_, other) in &geometry.tabs[..index] {
                    assert!(rect.intersection(*other).is_empty());
                }
            }
        }
    }
}

#[test]
fn metadata_stays_below_content_and_does_not_consume_its_first_row() {
    let (_root, diff) = diff_fixture();
    let items = [
        NavItem {
            label: "main.rs".into(),
            ..NavItem::default()
        },
        NavItem {
            label: "lib.rs".into(),
            ..NavItem::default()
        },
    ];
    let data = RenderData {
        items: &items,
        nav_title: "Working",
        canvas_title: "main.rs",
        canvas_subtitle: "Index preview",
        diff: Some(&diff),
        ..RenderData::default()
    };
    for height in [14, 30] {
        let (buffer, geometry) = draw(120, height, &mut ViewState::default(), &data);
        let first_row = area_text(
            &buffer,
            Rect::new(geometry.nav.x, geometry.nav.y, geometry.nav.width, 1),
        );
        assert!(first_row.contains("main.rs"));
        assert!(!area_text(&buffer, geometry.nav).contains("2 files"));
        assert!(!area_text(&buffer, geometry.canvas).contains("Index preview"));
        let metadata_y = (0..height)
            .find(|y| area_text(&buffer, Rect::new(0, *y, 120, 1)).contains("Index preview"))
            .unwrap();
        let metadata = area_text(&buffer, Rect::new(0, metadata_y, 120, 1));
        assert!(metadata.contains("2 files"));
        assert!(metadata.contains("+1") && metadata.contains("-2"));
        assert!(metadata_y >= geometry.nav.bottom());
        assert!(metadata_y >= geometry.canvas.bottom());
        assert_eq!(
            metadata_y + 2,
            height,
            "metadata and footer should share the bottom border"
        );
    }
}

#[test]
fn tab_rule_spans_the_terminal_with_one_localized_active_segment() {
    for workspace in [
        Workspace::Changes,
        Workspace::Stashes,
        Workspace::Operations,
    ] {
        let mut state = ViewState {
            workspace,
            ..ViewState::default()
        };
        let (buffer, geometry) = draw(120, 25, &mut state, &RenderData::default());
        let active = geometry
            .tabs
            .iter()
            .find(|(item, _)| *item == workspace)
            .unwrap()
            .1;
        let y = active.bottom() - 1;
        let accented = (0..buffer.area.width)
            .filter(|x| buffer[(*x, y)].fg == foreground(Role::Accent))
            .collect::<Vec<_>>();
        assert!(!accented.is_empty());
        assert!(accented[0] >= active.x && *accented.last().unwrap() < active.right());
        assert_eq!(
            accented.len(),
            usize::from(accented.last().unwrap() - accented[0] + 1)
        );
        assert!((0..buffer.area.width).all(|x| !buffer[(x, y)].symbol().trim().is_empty()));
    }
}

#[test]
fn repository_identity_stays_bottom_right_when_feedback_is_long() {
    let operation = OperationView {
        label: "A deliberately long completed action description ".repeat(6),
        detail: "Action details must not replace repository identity".into(),
        ..OperationView::default()
    };
    let data = RenderData {
        repo: "atlas",
        branch: "feature/canvas",
        operation: Some(&operation),
        ..RenderData::default()
    };
    for width in [60, 120] {
        let (buffer, geometry) = draw(width, 25, &mut ViewState::default(), &data);
        let footer = area_text(&buffer, Rect::new(0, 24, width, 1));
        assert!(footer.contains("atlas"));
        assert!(footer.contains("feature/canvas"));
        assert!(!text(&buffer).contains("dogit"));
        assert!(geometry.body.bottom() <= 24);
        let badge_cells = (0..width)
            .filter(|x| buffer[(*x, 24)].bg != Color::Reset)
            .collect::<Vec<_>>();
        assert!(!badge_cells.is_empty());
        assert!(badge_cells[0] >= width / 2);
        assert_eq!(
            badge_cells.len(),
            usize::from(badge_cells.last().unwrap() - badge_cells[0] + 1)
        );
        let badge_text = badge_cells
            .iter()
            .map(|x| buffer[(*x, 24)].symbol())
            .collect::<String>();
        assert_eq!(badge_text.trim(), "atlas");
        assert_terminal_background_except_badge(&buffer);
    }
}

#[test]
fn overlays_and_structural_output_preserve_the_terminal_background() {
    let data = RenderData {
        repo: "atlas",
        branch: "main",
        structural: Some("\x1b[48;2;255;0;0m\x1b[7mexternal background\x1b[0m"),
        ..RenderData::default()
    };
    let overlays = [
        Overlay::Input {
            title: "Search".into(),
            value: "main".into(),
            detail: String::new(),
        },
        Overlay::Commands {
            query: "sta".into(),
            items: vec![NavItem {
                label: "Stage".into(),
                ..NavItem::default()
            }],
            selected: 0,
        },
        Overlay::Compose {
            summary: "Commit subject".into(),
            body: "Commit body".into(),
            body_started: true,
        },
        Overlay::Confirm {
            title: "Discard selected file".into(),
            detail: "src/main.rs".into(),
            destructive: true,
            scroll: 0,
        },
    ];
    for overlay in overlays {
        let mut state = ViewState {
            overlay: Some(overlay),
            ..ViewState::default()
        };
        let (buffer, _) = draw(120, 35, &mut state, &data);
        assert_terminal_background_except_badge(&buffer);
    }
}

#[test]
fn replacing_repository_identity_clears_its_previous_badge_background() {
    let mut terminal = Terminal::new(TestBackend::new(120, 25)).unwrap();
    let mut state = ViewState::default();
    let mut data = RenderData {
        repo: "a-much-longer-repository-name",
        branch: "main",
        ..RenderData::default()
    };
    terminal
        .draw(|frame| {
            render(frame, &mut state, &data);
        })
        .unwrap();
    data.repo = "atlas";
    terminal
        .draw(|frame| {
            render(frame, &mut state, &data);
        })
        .unwrap();
    let (fresh, _) = draw(120, 25, &mut ViewState::default(), &data);
    assert_eq!(terminal.backend().buffer(), &fresh);
    data.repo = "";
    terminal
        .draw(|frame| {
            render(frame, &mut state, &data);
        })
        .unwrap();
    assert!(
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .all(|cell| cell.bg == Color::Reset)
    );
}

#[test]
fn repository_badges_are_stable_distinct_and_readable() {
    fn rgb(color: Color) -> [u8; 3] {
        match color {
            Color::Rgb(red, green, blue) => [red, green, blue],
            Color::Indexed(index) => ui_theme::Color::Ansi(index).rgb().unwrap(),
            Color::Reset => panic!("repository badges require explicit contrasting colors"),
            other => {
                let index = match other {
                    Color::Black => 0,
                    Color::Red => 1,
                    Color::Green => 2,
                    Color::Yellow => 3,
                    Color::Blue => 4,
                    Color::Magenta => 5,
                    Color::Cyan => 6,
                    Color::Gray => 7,
                    Color::DarkGray => 8,
                    Color::LightRed => 9,
                    Color::LightGreen => 10,
                    Color::LightYellow => 11,
                    Color::LightBlue => 12,
                    Color::LightMagenta => 13,
                    Color::LightCyan => 14,
                    Color::White => 15,
                    _ => unreachable!(),
                };
                ui_theme::Color::Ansi(index).rgb().unwrap()
            }
        }
    }
    fn luminance(color: Color) -> f64 {
        let channels = rgb(color).map(|value| {
            let value = f64::from(value) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        });
        channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722
    }
    let atlas = chrome::repository_style("atlas");
    assert_eq!(atlas, chrome::repository_style("atlas"));
    assert_ne!(atlas.bg, chrome::repository_style("dogit").bg);
    for name in ["atlas", "dogit", "dotfiles", "client", "server", "配置"] {
        let appearance = chrome::repository_style(name);
        let foreground = luminance(appearance.fg.unwrap());
        let background = luminance(appearance.bg.unwrap());
        let contrast = (foreground.max(background) + 0.05) / (foreground.min(background) + 0.05);
        assert!(
            contrast >= 4.5,
            "{name} has insufficient badge contrast: {contrast}"
        );
    }
}

#[test]
fn divider_hover_has_a_five_cell_target_without_changing_pane_geometry() {
    let mut state = ViewState::default();
    let (plain, before) = draw(120, 30, &mut state, &RenderData::default());
    let x = before.divider_x.unwrap();
    let target = before.divider_hit.unwrap();
    assert_eq!(target.width, 5);
    assert_eq!(target.x + 2, x);
    assert!(target.y <= before.body.y && target.bottom() >= before.body.bottom());
    state.divider_hovered = true;
    state.divider_hover_row = Some(before.body.y + 2);
    let (hovered, after) = draw(120, 30, &mut state, &RenderData::default());
    assert_eq!(before.nav, after.nav);
    assert_eq!(before.canvas, after.canvas);
    assert_eq!(before.divider_hit, after.divider_hit);
    assert!(
        (after.body.y..after.body.bottom()).all(|y| hovered[(x, y)].fg == foreground(Role::Accent))
    );
    assert!(
        (after.body.y..after.body.bottom()).any(|y| {
            hovered[(x - 1, y)].symbol() != plain[(x - 1, y)].symbol()
                && hovered[(x + 1, y)].symbol() != plain[(x + 1, y)].symbol()
        }),
        "hover must expose a visible drag grip"
    );
    assert_terminal_background_except_badge(&hovered);
    let (_, compact) = draw(60, 30, &mut state, &RenderData::default());
    assert!(compact.divider_x.is_none() && compact.divider_hit.is_none());
}

#[test]
fn current_worktree_is_distinct_without_a_current_text_label() {
    let items = [
        NavItem {
            label: "atlas".into(),
            kind: ItemKind::Worktree,
            status: "current".into(),
            ..NavItem::default()
        },
        NavItem {
            label: "history".into(),
            kind: ItemKind::Worktree,
            status: "locked".into(),
            ..NavItem::default()
        },
    ];
    let data = RenderData {
        items: &items,
        ..RenderData::default()
    };
    let mut state = ViewState {
        workspace: Workspace::Worktrees,
        selected: 1,
        ..ViewState::default()
    };
    let (buffer, geometry) = draw(120, 25, &mut state, &data);
    let nav = area_text(&buffer, geometry.nav);
    assert!(nav.contains("atlas") && nav.contains("history") && nav.contains("locked"));
    assert!(!nav.contains("current"));
    let current = &buffer[(geometry.nav.x + 2, geometry.nav.y)];
    assert_eq!(current.symbol(), "a");
    assert_eq!(current.fg, foreground(Role::Accent));
    assert!(current.modifier.contains(Modifier::BOLD));
    let selected = &buffer[(geometry.nav.x + 2, geometry.nav.y + 1)];
    assert_ne!(
        current.fg, selected.fg,
        "current worktree and selection have different meanings"
    );
}

#[test]
fn resizing_during_a_tab_transition_keeps_its_indicator_in_the_visible_strip() {
    let mut state = ViewState::default();
    draw(120, 25, &mut state, &RenderData::default());
    assert!(!state.animating());
    state.workspace = Workspace::Operations;
    draw(120, 25, &mut state, &RenderData::default());
    let (buffer, geometry) = draw(39, 25, &mut state, &RenderData::default());
    let active = geometry
        .tabs
        .iter()
        .find(|(workspace, _)| *workspace == Workspace::Operations)
        .unwrap()
        .1;
    let underline = (active.x..active.right())
        .any(|x| buffer[(x, active.bottom() - 1)].fg == foreground(Role::Accent));
    assert!(
        underline,
        "resizing must re-anchor the moving indicator to the visible active tab"
    );
    assert!(!state.animating());
    assert!(area_text(&buffer, active).contains("Operations"));
}

#[test]
fn tab_text_and_underline_transition_together_and_finish_quickly() {
    let now = Instant::now();
    let mut state = ViewState::default();
    let (initial, tabs) = draw_tabs_at(&mut state, now, false);
    let old = tabs
        .iter()
        .find(|(workspace, _)| *workspace == Workspace::Changes)
        .unwrap()
        .1;
    let new = tabs
        .iter()
        .find(|(workspace, _)| *workspace == Workspace::Operations)
        .unwrap()
        .1;
    state.workspace = Workspace::Operations;
    let (start, _) = draw_tabs_at(&mut state, now, false);
    assert_eq!(
        initial, start,
        "label and indicator must both begin at the previous visual state"
    );
    assert!(state.animating());
    let (middle, _) = draw_tabs_at(&mut state, now + Duration::from_millis(30), false);
    let old_color = middle[(old.x + 1, old.y)].fg;
    let new_color = middle[(new.x + 1, new.y)].fg;
    assert_ne!(old_color, initial[(old.x + 1, old.y)].fg);
    assert_ne!(new_color, initial[(new.x + 1, new.y)].fg);
    let line_x = (0..120).find(|x| middle[(*x, 2)].symbol() == "━").unwrap();
    assert!(line_x > old.x && line_x < new.x);
    if let (Color::Rgb(plain, _, _), Color::Rgb(accent, _, _), Color::Rgb(current, _, _)) =
        (foreground(Role::Plain), foreground(Role::Accent), new_color)
    {
        let color_progress =
            (f64::from(current) - f64::from(plain)) / (f64::from(accent) - f64::from(plain));
        let position_progress = f64::from(line_x - old.x) / f64::from(new.x - old.x);
        assert!(
            (color_progress - position_progress).abs() < 0.04,
            "text and underline must use the same eased progress"
        );
    }
    let (finished, _) = draw_tabs_at(&mut state, now + Duration::from_millis(90), false);
    assert!(!state.animating());
    assert_eq!(finished[(new.x + 1, new.y)].fg, foreground(Role::Accent));
    assert_eq!(finished[(old.x + 1, old.y)].fg, foreground(Role::Plain));
    assert_eq!(finished[(new.x, 2)].symbol(), "━");
    let (idle, _) = draw_tabs_at(&mut state, now + Duration::from_secs(1), false);
    assert_eq!(finished, idle);
}

#[test]
fn interrupted_tab_motion_preserves_colors_and_reduced_motion_snaps_both() {
    let now = Instant::now();
    let mut state = ViewState::default();
    draw_tabs_at(&mut state, now, false);
    state.workspace = Workspace::Operations;
    draw_tabs_at(&mut state, now, false);
    let at = now + Duration::from_millis(25);
    let (before, _) = draw_tabs_at(&mut state, at, false);
    state.workspace = Workspace::History;
    let (after, _) = draw_tabs_at(&mut state, at, false);
    assert_eq!(
        before, after,
        "a new destination must not jump the current label colors or underline"
    );
    let (snapped, tabs) = draw_tabs_at(&mut state, at, true);
    assert!(!state.animating());
    for (workspace, rect) in tabs {
        assert_eq!(
            snapped[(rect.x + 1, rect.y)].fg,
            foreground(if workspace == Workspace::History {
                Role::Accent
            } else {
                Role::Plain
            })
        );
        assert_eq!(
            snapped[(rect.x, 2)].symbol() == "━",
            workspace == Workspace::History
        );
    }
}

#[test]
fn divider_grip_follows_pointer_row_and_stays_inside_the_frame() {
    let mut state = ViewState {
        divider_hovered: true,
        ..ViewState::default()
    };
    let (_, geometry) = draw(120, 30, &mut state, &RenderData::default());
    let x = geometry.divider_x.unwrap();
    let target = geometry.divider_hit.unwrap();
    for pointer in [geometry.body.y + 1, geometry.body.y + 5, 0, u16::MAX] {
        state.divider_hover_row = Some(pointer);
        let (buffer, _) = draw(120, 30, &mut state, &RenderData::default());
        let row = pointer.clamp(target.y, target.bottom() - 1);
        assert_eq!(buffer[(x, row)].symbol(), "╫");
        assert_eq!(buffer[(x - 1, row)].symbol(), "‹");
        assert_eq!(buffer[(x + 1, row)].symbol(), "›");
        assert_eq!(
            buffer
                .content
                .iter()
                .filter(|cell| cell.symbol() == "╫")
                .count(),
            1
        );
    }
    state.divider_hover_row = None;
    let (buffer, _) = draw(120, 30, &mut state, &RenderData::default());
    assert!(!text(&buffer).contains("╫"));
}
