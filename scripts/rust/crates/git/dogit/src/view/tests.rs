use super::*;
use std::fs;
use std::path::Path;
use std::process::Command;

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
                assert!(
                    y >= buffer.area.bottom().saturating_sub(2)
                        || (y == 2
                            && [
                                style(Role::DiffAdded),
                                style(Role::DiffRemoved),
                                file_side_style(DiffSide::Index),
                                file_side_style(DiffSide::Worktree),
                            ]
                            .map(|appearance| chrome::badge_style(appearance, color_depth()).bg)
                            .contains(&Some(cell.bg))),
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
        ahead: 12,
        behind: 3,
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
        diff_rows: Some((&cached_unified, false, 123)),
        ..RenderData::default()
    };
    let (_, resized) = draw(160, 30, &mut state, &stale_mode);
    assert_eq!(resized.right_line_ids, split.right_line_ids);
    let cached_split = RenderData {
        diff: Some(&diff),
        diff_rows: Some((&rows, true, 123)),
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
        && !cell.modifier.contains(Modifier::BOLD)));
    assert_terminal_background_except_badge(&buffer);
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
    assert_terminal_background_except_badge(&buffer);
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
    let maximum = confirmation_scroll_max(&detail, geometry.overlay_bounds);
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
            let number = Workspace::ALL
                .iter()
                .position(|item| *item == workspace)
                .unwrap()
                + 1;
            assert!(
                area_text(&buffer, active.1).contains(&format!("{number} {}", workspace.label()))
            );
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
fn changes_counts_replace_hints_and_hide_zero_or_redundant_totals() {
    for (items, staged, unstaged, expected) in [
        (0, 0, 0, ""),
        (4, 4, 0, "4 Staged"),
        (4, 0, 4, "4 Unstaged"),
        (6, 2, 4, "6 Files  2 Staged  4 Unstaged"),
        (1, 1, 1, "1 File  1 Staged  1 Unstaged"),
    ] {
        for width in [60, 124] {
            for overlay in [
                None,
                Some(Overlay::Input {
                    title: "Search".into(),
                    value: "123".into(),
                }),
            ] {
                let data = RenderData {
                    nav_summary: Some(NavSummary {
                        items,
                        staged,
                        unstaged,
                        ..NavSummary::default()
                    }),
                    ..RenderData::default()
                };
                let (buffer, geometry) = draw(
                    width,
                    25,
                    &mut ViewState {
                        overlay,
                        nav_width: 60,
                        ..ViewState::default()
                    },
                    &data,
                );
                let footer = area_text(&buffer, geometry.nav_footer);
                assert_eq!(
                    footer.split_whitespace().collect::<Vec<_>>().join(" "),
                    expected.split_whitespace().collect::<Vec<_>>().join(" ")
                );
                assert!(!text(&buffer).contains("Shift+drag"));
                assert!(!text(&buffer).contains("Tab panes"));
                assert!(!area_text(&buffer, geometry.nav).contains("Staged"));
            }
        }
    }
}

#[test]
fn files_badge_shares_the_repository_identity_style() {
    let data = RenderData {
        repo: "dotfiles",
        nav_summary: Some(NavSummary {
            items: 6,
            staged: 2,
            unstaged: 1,
            ..NavSummary::default()
        }),
        ..RenderData::default()
    };
    let (buffer, geometry) = draw(120, 25, &mut ViewState::default(), &data);
    let footer = area_text(&buffer, Rect::new(0, 24, 120, 1));
    let files_x = footer.find("6 Files").unwrap() as u16;
    let repo_x = footer.find("Dotfiles").unwrap() as u16;
    let identity = chrome::identity_style("dotfiles");
    for x in files_x..files_x + "6 Files".len() as u16 {
        let cell = &buffer[(x, 24)];
        let badge = &buffer[(repo_x, 24)];
        assert_eq!(cell.bg, badge.bg, "{x}");
        assert_eq!(cell.fg, badge.fg, "{x}");
        assert_eq!(cell.bg, identity.bg.unwrap(), "{x}");
        assert_eq!(cell.fg, identity.fg.unwrap(), "{x}");
        assert!(cell.modifier.contains(Modifier::BOLD));
    }
    assert_ne!(buffer[(files_x, 24)].bg, Color::Reset);
    assert!(geometry.nav_footer.y == 24);
}

#[test]
fn every_badge_glyph_renders_bold() {
    let data = RenderData {
        nav_summary: Some(NavSummary {
            items: 6,
            staged: 2,
            unstaged: 4,
            ..NavSummary::default()
        }),
        ..RenderData::default()
    };
    for width in [60, 120] {
        let (buffer, geometry) = draw(width, 25, &mut ViewState::default(), &data);
        let footer = area_text(&buffer, geometry.nav_footer);
        assert!(footer.contains("2 Staged") && footer.contains("4 Unstaged"));
        for x in geometry.nav_footer.x..geometry.nav_footer.right() {
            let cell = &buffer[(x, geometry.nav_footer.y)];
            if !cell.symbol().trim().is_empty() {
                assert!(
                    cell.modifier.contains(Modifier::BOLD),
                    "{width}: {x} {:?}",
                    cell.symbol()
                );
            }
        }
    }
}

#[test]
fn pane_footer_has_no_separator_and_uses_the_freed_row_for_content() {
    let (buffer, geometry) = draw(120, 30, &mut ViewState::default(), &RenderData::default());
    assert_eq!(geometry.nav.bottom(), 29);
    for x in 1..119 {
        assert!(!["─", "├", "┤", "┼"].contains(&buffer[(x, 27)].symbol()));
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
        assert!(!area_text(&buffer, geometry.nav).contains("2 Files"));
        assert!(!area_text(&buffer, geometry.canvas).contains("Index preview"));
        let metadata_y = (0..height)
            .find(|y| area_text(&buffer, Rect::new(0, *y, 120, 1)).contains("Index preview"))
            .unwrap();
        let metadata = area_text(&buffer, Rect::new(0, metadata_y, 120, 1));
        assert!(area_text(&buffer, geometry.nav_footer).contains("2 Files"));
        assert!(!metadata.contains("+1") && !metadata.contains("-2"));
        let header = area_text(
            &buffer,
            Rect::new(geometry.canvas.x, geometry.body.y, geometry.canvas.width, 1),
        );
        assert!(header.contains("+1") && header.contains("-2"));
        assert!(metadata_y >= geometry.nav.bottom());
        assert!(metadata_y >= geometry.canvas.bottom());
        assert_eq!(
            metadata_y + 1,
            height,
            "metadata and footer should share the bottom border"
        );
    }
}

#[test]
fn tab_rule_spans_the_terminal_with_one_localized_active_segment() {
    for workspace in [Workspace::Changes, Workspace::Stashes, Workspace::Compare] {
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
        assert!(footer.contains("Atlas"));
        assert!(footer.contains("Feature/canvas"));
        assert!(!text(&buffer).contains("dogit"));
        assert!(geometry.body.bottom() <= 24);
        for (name, label) in [(data.branch, "Feature/canvas"), (data.repo, "Atlas")] {
            let label_x = footer.find(label).unwrap() as u16;
            let badge = Rect::new(label_x - 1, 24, label.len() as u16 + 2, 1);
            assert!(badge.x >= width / 2);
            assert_eq!(area_text(&buffer, badge), format!(" {label} "));
            let appearance = chrome::identity_style(name);
            for x in badge.x..badge.right() {
                let cell = &buffer[(x, badge.y)];
                assert_eq!(cell.bg, appearance.bg.unwrap());
                assert_eq!(cell.fg, appearance.fg.unwrap());
                assert!(cell.modifier.contains(Modifier::BOLD));
            }
            assert_eq!(buffer[(badge.right(), badge.y)].bg, Color::Reset);
        }
        assert_terminal_background_except_badge(&buffer);
    }
}

#[test]
fn result_toasts_match_diff_badges_and_preserve_the_footer() {
    for (failed, title, role) in [
        (false, "Committed!", Role::DiffAdded),
        (true, "Push failed", Role::DiffRemoved),
    ] {
        let toast = ToastView {
            title,
            detail: "remote rejected the update",
            failed,
            elapsed: std::time::Duration::from_secs(1),
            reduced_motion: false,
        };
        let data = RenderData {
            repo: "atlas",
            branch: "main",
            toast: Some(&toast),
            ..RenderData::default()
        };
        let (buffer, _) = draw(120, 25, &mut ViewState::default(), &data);
        let appearance = chrome::badge_style(style(role), color_depth());
        let title_row = (0..buffer.area.height)
            .find(|y| area_text(&buffer, Rect::new(0, *y, 120, 1)).contains(title))
            .unwrap();
        let title_cell = (0..buffer.area.width)
            .map(|x| &buffer[(x, title_row)])
            .find(|cell| cell.symbol() == &title[..1])
            .unwrap();
        assert_eq!(Some(title_cell.fg), appearance.fg);
        assert_eq!(Some(title_cell.bg), appearance.bg);
        assert!(title_cell.modifier.contains(Modifier::BOLD));
        assert!(text(&buffer).contains(title));
        assert_eq!(text(&buffer).contains("remote rejected"), failed);
        assert_eq!(text(&buffer).contains("Esc dismiss"), failed);
        let footer = area_text(&buffer, Rect::new(0, 24, 120, 1));
        assert!(footer.contains("Main") && footer.contains("Atlas"));
    }
}

#[test]
fn toast_motion_settles_expires_and_respects_reduced_motion() {
    use std::time::Duration;
    let mut toast = ToastView {
        title: "Pushed!",
        detail: "",
        failed: false,
        elapsed: Duration::ZERO,
        reduced_motion: false,
    };
    let baseline = Buffer::filled(Rect::new(0, 0, 80, 20), ratatui::buffer::Cell::new("·"));
    let paint = |toast: &ToastView<'_>| {
        let mut buffer = baseline.clone();
        toast::render(&mut buffer, baseline.area, toast);
        buffer
    };
    assert!(toast.animating());
    assert_eq!(paint(&toast), baseline);
    toast.elapsed = Duration::from_millis(70);
    let entering = paint(&toast);
    assert_ne!(entering, baseline);
    toast.elapsed = Duration::from_millis(250);
    assert!(!toast.animating());
    let settled = paint(&toast);
    assert_ne!(entering, settled);
    assert!(text(&settled).contains("Pushed!"));
    toast.elapsed = Duration::from_millis(4930);
    assert!(toast.animating());
    assert_ne!(paint(&toast), settled);
    toast.elapsed = Duration::from_secs(5);
    assert!(toast.expired());
    assert_eq!(paint(&toast), baseline);

    toast.reduced_motion = true;
    for elapsed in [Duration::ZERO, Duration::from_millis(4999)] {
        toast.elapsed = elapsed;
        assert!(!toast.animating());
        assert_eq!(paint(&toast), settled);
    }
    toast.failed = true;
    toast.elapsed = Duration::from_secs(60);
    assert!(!toast.expired());
    assert!(text(&paint(&toast)).contains("Esc dismiss"));
}

#[test]
fn toasts_clip_unicode_and_controls_in_small_or_offset_viewports() {
    let toast = ToastView {
        title: "Push 界 failed\u{1b}[2J",
        detail: "remote: 拒否された\nsecond line\twith controls",
        failed: true,
        elapsed: std::time::Duration::from_secs(1),
        reduced_motion: true,
    };
    for width in [0, 1, 4, 8, 16, 24, 80] {
        for height in [0, 1, 4, 6, 20] {
            let mut buffer = Buffer::empty(Rect::new(3, 5, width, height));
            let area = buffer.area;
            toast::render(&mut buffer, area, &toast);
            assert!(!text(&buffer).chars().any(char::is_control));
            if height > 0 {
                assert!(
                    area_text(&buffer, Rect::new(3, area.bottom() - 1, width, 1))
                        .trim()
                        .is_empty()
                );
            }
        }
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
        },
        Overlay::Commands {
            query: "sta".into(),
            items: vec![NavItem {
                label: "Stage".into(),
                ..NavItem::default()
            }],
            selected: 0,
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
        let (buffer, geometry) = draw(120, 35, &mut state, &data);
        assert_terminal_background_except_badge(&buffer);
        for y in geometry.body.y..geometry.body.bottom() {
            assert!((0..120).all(|x| !buffer[(x, y)].modifier.contains(Modifier::BOLD)));
        }
    }
}

#[test]
fn replacing_identity_badges_preserves_branch_color_and_clears_old_backgrounds() {
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
    let main_color = {
        let buffer = terminal.backend().buffer();
        let footer = area_text(buffer, Rect::new(0, 24, 120, 1));
        let cell = &buffer[(footer.find("Main").unwrap() as u16, 24)];
        assert_ne!(cell.bg, Color::Reset);
        (cell.fg, cell.bg)
    };
    data.repo = "atlas";
    for (ahead, behind) in [(3, 0), (0, 12), (3, 1234), (0, 0)] {
        data.ahead = ahead;
        data.behind = behind;
        terminal
            .draw(|frame| {
                render(frame, &mut state, &data);
            })
            .unwrap();
        let (fresh, _) = draw(120, 25, &mut ViewState::default(), &data);
        assert_eq!(terminal.backend().buffer(), &fresh);
        let footer = area_text(&fresh, Rect::new(0, 24, 120, 1));
        let label_x = |label: &str| footer[..footer.find(label).unwrap()].width() as u16;
        let cell = &fresh[(label_x("Main"), 24)];
        assert_eq!((cell.fg, cell.bg), main_color);
        assert!(!footer.contains("ahead") && !footer.contains("behind"));
        assert_eq!(footer.contains(''), ahead > 0);
        assert_eq!(footer.contains(''), behind > 0);
        let mut next_x = label_x("Main") - 1;
        assert_eq!(
            label_x("Atlas") - label_x("Main"),
            "Main".width() as u16 + 3
        );
        for (count, arrow, role) in [
            (ahead, "", Role::DiffAdded),
            (behind, "", Role::DiffRemoved),
        ] {
            if count == 0 {
                continue;
            }
            let label = format!("{count} {arrow}");
            let start = label_x(&label) - 1;
            let end = start + label.width() as u16 + 2;
            assert_eq!(end + 1, next_x);
            assert_eq!(fresh[(end, 24)].bg, Color::Reset);
            let appearance = chrome::badge_style(style(role), color_depth());
            for x in start..end {
                let cell = &fresh[(x, 24)];
                assert_eq!(Some(cell.fg), appearance.fg);
                assert_eq!(Some(cell.bg), appearance.bg);
                assert!(cell.modifier.contains(Modifier::BOLD));
            }
            next_x = start;
        }
    }
    data.repo = "";
    data.branch = "";
    data.ahead = 0;
    data.behind = 0;
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
fn badges_keep_light_backgrounds_dark_matching_text_and_readable_contrast() {
    fn rgb(color: Color) -> [u8; 3] {
        match color {
            Color::Rgb(red, green, blue) => [red, green, blue],
            Color::Indexed(index) => ui_theme::Color::Ansi(index).rgb().unwrap(),
            Color::Reset => panic!("identity badges require explicit contrasting colors"),
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
    let assert_contrast = |appearance: Style| {
        let foreground = luminance(appearance.fg.unwrap());
        let background = luminance(appearance.bg.unwrap());
        assert!(background > foreground, "badge text must always be darker");
        assert!(background >= 0.4, "badge background should be light");
        let contrast = (background + 0.05) / (foreground + 0.05);
        assert!(contrast >= 4.5, "{appearance:?} has contrast {contrast}");
    };
    let atlas = chrome::identity_style("atlas");
    assert_eq!(atlas, chrome::identity_style("atlas"));
    assert_ne!(atlas.bg, chrome::identity_style("dogit").bg);
    for name in [
        "atlas",
        "dogit",
        "dotfiles",
        "client",
        "server",
        "配置",
        "main",
        "develop",
        "feature/canvas",
    ] {
        let appearance = chrome::identity_style(name);
        assert_contrast(appearance);
    }
    for depth in [
        ColorDepth::TrueColor,
        ColorDepth::Ansi256,
        ColorDepth::Ansi16,
    ] {
        for (source, dominant) in [
            (Color::Rgb(0x83, 0xdc, 0xa2), Some(1)),
            (Color::Rgb(0x73, 0xc9, 0x91), Some(1)),
            (Color::Rgb(0xff, 0x8c, 0x87), Some(0)),
            (Color::Rgb(0xe5, 0xba, 0x7d), Some(0)),
            (Color::Rgb(0xed, 0xf3, 0xf8), None),
        ] {
            let appearance = chrome::badge_style(Style::default().fg(source), depth);
            assert_contrast(appearance);
            if depth != ColorDepth::Ansi16
                && let Some(dominant) = dominant
            {
                for color in [appearance.fg.unwrap(), appearance.bg.unwrap()] {
                    let channels = rgb(color);
                    assert!(
                        channels[dominant] > channels[(dominant + 2) % 3],
                        "{depth:?}: {source:?} lost its hue in {color:?}"
                    );
                }
            }
        }
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
    assert_eq!(target.y, before.body.y + 1);
    assert_eq!(target.bottom(), before.body.bottom() - 1);
    state.divider_hovered = true;
    state.divider_hover_row = Some(before.body.y + 2);
    let (hovered, after) = draw(120, 30, &mut state, &RenderData::default());
    assert_eq!(before.nav, after.nav);
    assert_eq!(before.canvas, after.canvas);
    assert_eq!(before.divider_hit, after.divider_hit);
    assert!((target.y..target.bottom()).all(|y| hovered[(x, y)].fg == foreground(Role::Accent)));
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
    assert!(!current.modifier.contains(Modifier::BOLD));
    let footer = geometry.nav_footer;
    for x in footer.x..footer.right() {
        let cell = &buffer[(x, footer.y)];
        if !cell.symbol().trim().is_empty() {
            assert!(cell.modifier.contains(Modifier::BOLD));
        }
    }
    let selected = &buffer[(geometry.nav.x + 2, geometry.nav.y + 1)];
    assert_ne!(
        current.fg, selected.fg,
        "current worktree and selection have different meanings"
    );
}

#[test]
fn resizing_keeps_the_active_tab_in_the_visible_strip() {
    let mut state = ViewState::default();
    draw(120, 25, &mut state, &RenderData::default());
    state.workspace = Workspace::Compare;
    draw(120, 25, &mut state, &RenderData::default());
    let (buffer, geometry) = draw(39, 25, &mut state, &RenderData::default());
    let active = geometry
        .tabs
        .iter()
        .find(|(workspace, _)| *workspace == Workspace::Compare)
        .unwrap()
        .1;
    let underline = (active.x..active.right())
        .any(|x| buffer[(x, active.bottom() - 1)].fg == foreground(Role::Accent));
    assert!(
        underline,
        "resizing must anchor the indicator to the visible active tab"
    );
    assert!(area_text(&buffer, active).contains("Compare"));
}

#[test]
fn tabs_switch_label_and_underline_on_the_first_frame() {
    let mut state = ViewState::default();
    draw(120, 25, &mut state, &RenderData::default());
    for workspace in Workspace::ALL {
        state.workspace = workspace;
        let (buffer, geometry) = draw(120, 25, &mut state, &RenderData::default());
        for (tab, rect) in geometry.tabs {
            assert_eq!(
                buffer[(rect.x + 1, rect.y)].fg,
                foreground(if tab == workspace {
                    Role::Accent
                } else {
                    Role::Plain
                })
            );
            assert_eq!(
                buffer[(rect.x, rect.bottom() - 1)].symbol() == "━",
                tab == workspace
            );
        }
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

#[test]
fn connected_tree_distinguishes_staged_unstaged_and_mixed_without_duplicate_paths() {
    let staged = FileState {
        staged: true,
        expanded: true,
        ..FileState::default()
    };
    let unstaged = FileState {
        unstaged: true,
        ..FileState::default()
    };
    let mixed = FileState {
        unstaged: true,
        ..staged
    };
    let mut items = vec![
        NavItem {
            label: "src".into(),
            kind: ItemKind::Folder,
            file_state: Some(mixed),
            ..NavItem::default()
        },
        NavItem {
            label: "lib".into(),
            kind: ItemKind::Folder,
            depth: 1,
            file_state: Some(staged),
            ..NavItem::default()
        },
        NavItem {
            label: "a.rs".into(),
            status: "A".into(),
            depth: 2,
            file_state: Some(staged),
            ..NavItem::default()
        },
        NavItem {
            label: "main.rs".into(),
            status: "M".into(),
            depth: 1,
            file_state: Some(mixed),
            ..NavItem::default()
        },
        NavItem {
            label: "tests".into(),
            kind: ItemKind::Folder,
            file_state: Some(unstaged),
            ..NavItem::default()
        },
    ];
    let prefixes = tree_prefixes(&items);
    assert_eq!(prefixes, ["", "├─", "│ └─", "└─", ""]);
    for (item, prefix) in items.iter_mut().zip(prefixes) {
        item.graph = prefix;
    }
    let data = RenderData {
        items: &items,
        ..RenderData::default()
    };
    let mut state = ViewState {
        selected: 3,
        nav_width: 44,
        ..ViewState::default()
    };
    let (buffer, geometry) = draw(140, 30, &mut state, &data);
    let text = area_text(&buffer, geometry.nav);
    assert_eq!(text.matches("main.rs").count(), 1);
    assert!(text.contains("▾ src") && text.contains("▸ tests"));
    for row in text.lines() {
        assert!(
            !row.split_whitespace()
                .any(|word| matches!(word, "S" | "U" | "M" | "A"))
        );
    }
    let staged_x = geometry.nav.x + 2 + items[2].graph.width() as u16;
    assert_eq!(
        buffer[(staged_x, geometry.nav.y + 2)].fg,
        file_side_style(DiffSide::Index).fg.unwrap()
    );
    assert_eq!(
        buffer[(geometry.nav.x, geometry.nav.y + 3)].fg,
        foreground(Role::Accent)
    );
    assert_terminal_background_except_badge(&buffer);
}

#[test]
fn diff_context_has_stable_title_path_and_scope_controls_outside_patch_hit_rows() {
    let (_root, diff) = diff_fixture();
    let data = RenderData {
        canvas_path: "src/main.rs",
        diff_scope: Some(DiffScope {
            side: DiffSide::Worktree,
            staged: true,
            unstaged: true,
        }),
        diff: Some(&diff),
        ..RenderData::default()
    };
    let (buffer, geometry) = draw(140, 30, &mut ViewState::default(), &data);
    assert!(!area_text(&buffer, geometry.canvas_footer).contains("Diff"));
    assert!(
        area_text(
            &buffer,
            Rect::new(geometry.canvas.x, geometry.body.y, geometry.canvas.width, 1)
        )
        .contains("src/main.rs")
    );
    assert_eq!(geometry.canvas.y, geometry.body.y + 2);
    assert_eq!(geometry.diff_sides.len(), 2);
    for (side, rect) in geometry.diff_sides {
        assert!(rect.intersection(geometry.canvas).is_empty());
        assert!(
            area_text(&buffer, rect).contains(if side == DiffSide::Index {
                "Staged"
            } else {
                "Unstaged"
            })
        );
    }
    assert_eq!(
        geometry.line_ids,
        diff_rows(&diff, false)
            .iter()
            .map(|row| row.line)
            .collect::<Vec<_>>()
    );
}

#[test]
fn configured_diff_has_one_number_column_and_preserves_syntax_styles() {
    let (_root, diff) = diff_fixture();
    let lines = diff
        .hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .map(|line| {
            (
                line.id,
                format!(
                    "\x1b[38;2;123;67;201m{}\x1b[0m",
                    String::from_utf8_lossy(&line.content).trim_end_matches(['\r', '\n'])
                ),
            )
        })
        .collect();
    let mut presentation = crate::presentation::DiffPresentation {
        lines,
        config: crate::presentation::DiffPresentationConfig {
            left_format: String::new(),
            right_format: "{np:>2}│".into(),
            keep_markers: false,
            hunk_headers: false,
            ..crate::presentation::DiffPresentationConfig::default()
        },
        renderer: crate::presentation::Renderer::Delta,
    };
    let mut state = ViewState::default();
    let data = RenderData {
        diff: Some(&diff),
        presentation: Some(&presentation),
        ..RenderData::default()
    };
    let (buffer, geometry) = draw(160, 30, &mut state, &data);
    let row = area_text(
        &buffer,
        Rect::new(
            geometry.canvas.x,
            geometry.canvas.y + 1,
            geometry.canvas.width,
            1,
        ),
    );
    assert_eq!(row.matches('│').count(), 1);
    assert!(row.contains("1│ fn main()"));
    assert!(!area_text(&buffer, geometry.canvas).contains("@@"));
    let x = geometry.canvas.x + 5;
    assert_eq!(
        buffer[(x, geometry.canvas.y + 1)].fg,
        ui_theme::Color::Rgb(123, 67, 201)
            .at_depth(color_depth())
            .ratatui()
    );
    state.split = true;
    let (buffer, geometry) = draw(160, 30, &mut state, &data);
    let row = area_text(
        &buffer,
        Rect::new(
            geometry.canvas.x,
            geometry.canvas.y + 1,
            geometry.canvas.width,
            1,
        ),
    );
    assert_eq!(
        row.matches("1│").count(),
        2,
        "split has exactly one number column on either side"
    );
    presentation.config.line_numbers = false;
    state.split = false;
    let data = RenderData {
        diff: Some(&diff),
        presentation: Some(&presentation),
        ..RenderData::default()
    };
    let (buffer, geometry) = draw(160, 30, &mut state, &data);
    let row = area_text(
        &buffer,
        Rect::new(
            geometry.canvas.x,
            geometry.canvas.y + 1,
            geometry.canvas.width,
            1,
        ),
    );
    assert_eq!(row.trim(), "fn main() {");
}

#[test]
fn syntax_spans_keep_configured_emphasis_and_tab_stops_without_terminal_commands() {
    let line = syntax_line(
        "a\x1b[1;3;38;2;123;67;201m\tb\x1b[48;2;45;25;25mX\x1b]52;c;secret\x07\x1b[2JZ",
        4,
    );
    let content = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert_eq!(content, "a   bXZ");
    assert_eq!(
        line.spans.last().unwrap().style.bg,
        Some(
            ui_theme::Color::Rgb(45, 25, 25)
                .at_depth(color_depth())
                .ratatui()
        )
    );
    assert!(!content.contains("secret") && !content.contains('\x1b'));
    assert!(
        line.spans
            .last()
            .unwrap()
            .style
            .add_modifier
            .contains(Modifier::ITALIC)
    );
    assert!(
        line.spans
            .iter()
            .all(|span| !span.style.add_modifier.contains(Modifier::BOLD))
    );
}

#[test]
fn wrapped_split_rows_keep_source_ids_and_reflow_when_cached_width_is_stale() {
    use crate::presentation::{DiffPresentationConfig, WrapPolicy};
    let (_root, mut diff) = diff_fixture();
    diff.hunks.truncate(1);
    diff.hunks[0].lines = vec![
        crate::diff::DiffLine {
            id: 0,
            kind: LineKind::Deletion,
            content: format!("{}\n", "old ".repeat(40)).into_bytes(),
            old_line: Some(1),
            new_line: None,
            no_newline: false,
        },
        crate::diff::DiffLine {
            id: 1,
            kind: LineKind::Addition,
            content: b"\n".to_vec(),
            old_line: None,
            new_line: Some(1),
            no_newline: false,
        },
        crate::diff::DiffLine {
            id: 2,
            kind: LineKind::Context,
            content: b"unchanged\n".to_vec(),
            old_line: Some(2),
            new_line: Some(2),
            no_newline: false,
        },
    ];
    let config = DiffPresentationConfig::default();
    let mut state = ViewState {
        split: true,
        ..ViewState::default()
    };
    let width = canvas_width(Rect::new(0, 0, 120, 30), &state);
    let layout = configured_diff_layout(&diff, width, true, None, &config);
    assert_eq!(layout.rows[1].line, Some(0));
    assert_eq!(
        layout.rows[1].right_line,
        Some(1),
        "empty added source line keeps its identity"
    );
    let continuation = &layout.rows[2];
    assert_eq!(continuation.line, Some(0));
    assert_eq!(
        continuation.right_line, None,
        "exhausted half has no invisible source identity"
    );
    assert!(continuation.left_offset > 0);
    assert_eq!(
        layout
            .rows
            .iter()
            .filter(|row| row.line.is_none() && row.right_line.is_none())
            .count(),
        1
    );
    assert_eq!(layout.rows.last().unwrap().right_line, Some(2));
    let selected_lines = BTreeSet::from([0]);
    let data = RenderData {
        diff: Some(&diff),
        diff_rows: Some((&layout.rows, true, width)),
        diff_paint: Some(&layout.paint),
        selected_lines: &selected_lines,
        ..RenderData::default()
    };
    let (buffer, geometry) = draw(120, 30, &mut state, &data);
    for y in geometry.canvas.y..geometry.canvas.bottom() {
        assert!(
            (geometry.canvas.x..geometry.canvas.right())
                .all(|x| !buffer[(x, y)].modifier.contains(Modifier::BOLD))
        );
    }
    assert_eq!(
        geometry.line_ids,
        layout.rows.iter().map(|row| row.line).collect::<Vec<_>>()
    );
    assert_eq!(
        geometry.right_line_ids,
        layout
            .rows
            .iter()
            .map(|row| row.right_line)
            .collect::<Vec<_>>()
    );
    let (_, resized) = draw(110, 30, &mut state, &data);
    let expected = configured_diff_layout(&diff, resized.canvas.width, true, None, &config);
    assert!(expected.rows.len() > layout.rows.len());
    assert_eq!(
        resized.line_ids,
        expected.rows.iter().map(|row| row.line).collect::<Vec<_>>()
    );
    assert_eq!(
        configured_diff_horizontal_limit(&diff, width, true, &config),
        0
    );
    let disabled = DiffPresentationConfig {
        split_wrap: WrapPolicy::Disabled,
        ..config
    };
    assert!(configured_diff_horizontal_limit(&diff, width, true, &disabled) > 0);
    assert_eq!(
        configured_diff_layout(&diff, width, true, None, &disabled)
            .rows
            .len(),
        3
    );
}

#[test]
fn predicted_canvas_width_matches_wide_and_compact_focus_geometry() {
    for width in [19, 20, 79, 80, 110, 160] {
        for focus in [Focus::Navigator, Focus::Canvas] {
            for nav_width in [22, 34, u16::MAX] {
                let mut state = ViewState {
                    focus,
                    nav_width,
                    ..ViewState::default()
                };
                let predicted = canvas_width(Rect::new(0, 0, width, 30), &state);
                let (_, geometry) = draw(width, 30, &mut state, &RenderData::default());
                assert_eq!(
                    predicted, geometry.canvas.width,
                    "width={width}, focus={focus:?}, navigator={nav_width}"
                );
            }
        }
    }
}

#[test]
fn compact_tree_uses_available_width_and_resizing_recovers_scrolled_names() {
    let mut items = vec![
        NavItem {
            label: "scripts/rust/crates/dclip".into(),
            kind: ItemKind::Folder,
            file_state: Some(FileState {
                expanded: true,
                ..FileState::default()
            }),
            ..NavItem::default()
        },
        NavItem {
            label: "Cargo.toml".into(),
            depth: 1,
            file_state: Some(FileState::default()),
            ..NavItem::default()
        },
        NavItem {
            label: "src".into(),
            kind: ItemKind::Folder,
            depth: 1,
            file_state: Some(FileState {
                expanded: true,
                ..FileState::default()
            }),
            ..NavItem::default()
        },
        NavItem {
            label: "mode.rs".into(),
            depth: 2,
            file_state: Some(FileState::default()),
            ..NavItem::default()
        },
    ];
    let prefixes = tree_prefixes(&items);
    for (item, prefix) in items.iter_mut().zip(prefixes) {
        item.graph = prefix;
    }
    for width in [22, 28, 36, 60] {
        let mut state = ViewState {
            nav_horizontal: 100,
            ..ViewState::default()
        };
        let area = Rect::new(0, 0, width, 5);
        let mut buffer = Buffer::empty(area);
        render_navigator(&mut buffer, area, &mut state, &items);
        let rendered = area_text(&buffer, area);
        assert!(rendered.contains("├─Cargo.toml"), "{rendered}");
        assert!(rendered.contains("└─src/"), "{rendered}");
        assert!(rendered.contains("  └─mode.rs"), "{rendered}");
        if width >= 30 {
            assert!(rendered.contains("scripts/rust/crates/dclip"), "{rendered}");
            assert_eq!(state.nav_horizontal, 0);
        }
    }
}

#[test]
fn all_workspaces_share_bottom_left_count_badges_without_outside_borders() {
    for (workspace, badge) in [
        (Workspace::Changes, "4 Staged"),
        (Workspace::Branches, "13 Branches"),
        (Workspace::History, "13 Commits"),
        (Workspace::Stashes, "13 Stashes"),
        (Workspace::Worktrees, "13 Worktrees"),
        (Workspace::Compare, "13 Files"),
    ] {
        let data = RenderData {
            nav_summary: Some(NavSummary {
                items: if workspace == Workspace::Changes {
                    4
                } else {
                    13
                },
                staged: 4,
                has_commits: true,
                ..NavSummary::default()
            }),
            ..RenderData::default()
        };
        for width in [60, 100, 140] {
            let (buffer, geometry) = draw(
                width,
                25,
                &mut ViewState {
                    workspace,
                    ..ViewState::default()
                },
                &data,
            );
            assert_eq!(geometry.body.y, 2);
            assert!(
                geometry
                    .tabs
                    .iter()
                    .all(|(_, rect)| rect.y == 0 && rect.bottom() == geometry.body.y)
            );
            let footer = area_text(&buffer, geometry.nav_footer);
            assert_eq!(geometry.nav_footer.y, 24);
            assert_eq!(geometry.nav_footer.x, 0);
            assert_eq!(geometry.nav.bottom(), 24);
            assert_eq!(footer.trim(), badge);
            assert!(footer.ends_with(&format!("{badge} ")), "{footer}");
            assert!(
                (geometry.nav_footer.x..geometry.nav_footer.right())
                    .any(|x| buffer[(x, geometry.nav_footer.y)].bg != Color::Reset)
            );
            for y in geometry.body.y..geometry.body.bottom() {
                assert_eq!(buffer[(0, y)].symbol(), " ");
                assert_eq!(buffer[(width - 1, y)].symbol(), " ");
            }
            if let Some(x) = geometry.divider_x {
                assert_eq!(buffer[(x, geometry.body.y)].symbol(), " ");
                assert_eq!(buffer[(x, geometry.body.bottom() - 1)].symbol(), " ");
                assert!(
                    (geometry.body.y + 1..geometry.body.bottom() - 1)
                        .all(|y| buffer[(x, y)].symbol() == "│")
                );
                assert!(!area_text(&buffer, geometry.canvas_footer).contains("Diff"));
            }
        }
    }
}

#[test]
fn directory_diff_headers_are_relative_and_preserve_quoted_file_identity() {
    let (root, _) = diff_fixture();
    let scope = Path::new("shared/wezterm");
    let names = [
        "core/init.lua",
        "keymap/motion keys.lua",
        "keymap/æ.lua",
        "keymap/tab\tname.lua",
        "deleted.lua",
    ];
    for name in names {
        let path = root.path().join(scope).join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "old\n").unwrap();
    }
    git(root.path(), &["add", "."]);
    git(
        root.path(),
        &["-c", "commit.gpgsign=false", "commit", "-qm", "Paths"],
    );
    for name in names {
        let path = root.path().join(scope).join(name);
        if name == "deleted.lua" {
            fs::remove_file(path).unwrap();
        } else {
            fs::write(path, "new\n").unwrap();
        }
    }
    let diff =
        ExactDiff::directory_cancellable(root.path(), scope, DiffSide::Worktree, None).unwrap();
    let raw = diff.raw.clone();
    let paths: BTreeSet<_> = diff
        .hunks
        .iter()
        .map(|hunk| hunk.file_path.clone())
        .collect();
    assert_eq!(
        paths,
        names.map(|name| scope.join(name)).into_iter().collect()
    );
    let data = RenderData {
        canvas_path: "shared/wezterm",
        directory_scope: Some(scope),
        diff: Some(&diff),
        ..RenderData::default()
    };
    let (buffer, geometry) = draw(150, 40, &mut ViewState::default(), &data);
    let canvas = area_text(
        &buffer,
        Rect::new(
            geometry.canvas.x,
            geometry.body.y,
            geometry.canvas.width,
            geometry.body.height,
        ),
    );
    assert_eq!(canvas.matches("shared/wezterm").count(), 1, "{canvas}");
    assert!(
        canvas.contains("core/init.lua") && canvas.contains("keymap/motion keys.lua"),
        "{canvas}"
    );
    assert!(
        canvas.contains("keymap/æ.lua") && canvas.contains("deleted.lua"),
        "{canvas}"
    );
    assert_eq!(diff.raw, raw);
}

#[test]
fn inline_composer_keeps_patch_visible_and_cursor_tracks_wrapping_scrolling_and_resize() {
    let (_root, diff) = diff_fixture();
    let data = RenderData {
        diff: Some(&diff),
        ..RenderData::default()
    };
    let mut state = ViewState::default();
    let value = format!(
        "Summary å\n\n{}\nlast 界e\u{301}👩💻",
        "A long body with words. ".repeat(35)
    );
    let mut editor = ui_widgets::EditorBuffer::new(&value);
    for width in [130, 80, 45, 110] {
        let mut terminal = Terminal::new(TestBackend::new(width, 36)).unwrap();
        let mut geometry = Geometry::default();
        for _ in 0..2 {
            state.overlay = Some(Overlay::Compose {
                value: editor.as_str().into(),
                cursor: editor.cursor(),
                selection: editor.selection(),
            });
            terminal
                .draw(|frame| {
                    geometry = render(frame, &mut state, &data);
                })
                .unwrap();
            let area = geometry.editor.unwrap();
            let cursor = terminal.get_cursor_position().unwrap();
            assert!(area.contains(cursor));
            let contents = area_text(terminal.backend().buffer(), area);
            assert!(contents.contains("last 界 e\u{301}👩💻"), "{contents}");
            if width == 130 {
                assert!(text(terminal.backend().buffer()).contains("fn main()"));
            }
            editor.move_cursor(ui_widgets::EditMotion::Left, false, usize::from(area.width));
        }
    }
    editor.move_cursor(ui_widgets::EditMotion::Start, false, 80);
    state.overlay = Some(Overlay::Compose {
        value: editor.as_str().into(),
        cursor: editor.cursor(),
        selection: None,
    });
    let (buffer, geometry) = draw(130, 36, &mut state, &data);
    assert!(area_text(&buffer, geometry.editor.unwrap()).contains("Summary å"));
    assert_eq!(state.editor_offset, 0);
    assert_terminal_background_except_badge(&buffer);
}

#[test]
fn inline_selection_has_solid_highlights_for_text_tabs_and_newlines() {
    let value = "ab\t界e\u{301}\n\nx";
    let mut state = ViewState {
        overlay: Some(Overlay::Compose {
            value: value.into(),
            cursor: value.len() - 1,
            selection: Some(1..value.len() - 1),
        }),
        ..ViewState::default()
    };
    let (buffer, geometry) = draw(130, 36, &mut state, &RenderData::default());
    let area = geometry.editor.unwrap();
    for (x, y, selected) in [
        (0, 0, false),
        (1, 0, true),
        (2, 0, true),
        (3, 0, true),
        (4, 0, true),
        (6, 0, true),
        (7, 0, true),
        (0, 1, true),
        (0, 2, false),
    ] {
        let cell = &buffer[(area.x + x, area.y + y)];
        assert_eq!(
            cell.modifier.contains(Modifier::REVERSED),
            selected,
            "{x},{y}"
        );
        assert!(!cell.modifier.contains(Modifier::UNDERLINED));
        assert!(!cell.modifier.contains(Modifier::BOLD));
    }
    assert_eq!(buffer[(area.x, area.y + 2)].symbol(), "x");
    assert!(!text(&buffer).contains("Ctrl+S"));
    assert!(text(&buffer).contains("Ctrl+C"));
}
