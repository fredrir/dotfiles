use super::*;

fn item(label: &str, depth: usize, kind: ItemKind, file: FileState) -> NavItem {
    NavItem {
        label: label.into(),
        depth,
        kind,
        file_state: Some(file),
        ..Default::default()
    }
}

fn row(buffer: &Buffer, y: u16) -> String {
    (buffer.area.x..buffer.area.right())
        .map(|x| buffer[(x, y)].symbol())
        .collect()
}

#[test]
fn explorer_has_disclosures_icons_and_neutral_names_with_separate_statuses() {
    let items = [
        item(
            "src",
            0,
            ItemKind::Folder,
            FileState {
                expanded: true,
                staged: true,
                unstaged: true,
                ..Default::default()
            },
        ),
        item("components", 1, ItemKind::Folder, FileState::default()),
        item(
            "main.ts",
            1,
            ItemKind::File,
            FileState {
                staged: true,
                ..Default::default()
            },
        ),
        item(
            "app.tsx",
            1,
            ItemKind::File,
            FileState {
                unstaged: true,
                ..Default::default()
            },
        ),
        item(
            "init.lua",
            1,
            ItemKind::File,
            FileState {
                staged: true,
                unstaged: true,
                ..Default::default()
            },
        ),
        item(
            "conflict.rs",
            1,
            ItemKind::File,
            FileState {
                conflicted: true,
                unstaged: true,
                ..Default::default()
            },
        ),
    ];
    let area = Rect::new(0, 0, 40, 6);
    let mut buffer = Buffer::empty(area);
    let mut state = ViewState {
        selected: 3,
        nav_hovered: Some(4),
        ..Default::default()
    };
    let disclosures = render_navigator(&mut buffer, area, &mut state, &items);
    assert_eq!(
        disclosures,
        vec![(0, Rect::new(1, 0, 2, 1)), (1, Rect::new(3, 1, 2, 1))]
    );
    assert!(row(&buffer, 0).contains("\u{f07c} src"));
    assert!(row(&buffer, 1).contains("▸ components"));
    assert!(!row(&buffer, 0).contains(['●', '○', '▾', '▸']));
    assert!(!row(&buffer, 1).contains(['\u{f07c}', '\u{f07b}']));
    for y in 1..6 {
        assert_eq!(buffer[(4, y)].symbol(), " ");
        assert_ne!(buffer[(5, y)].symbol(), " ");
    }
    for (y, icon) in [(2, "\u{e628}"), (3, "\u{e625}"), (4, "\u{e620}")] {
        assert!(row(&buffer, y).contains(icon));
        assert_eq!(buffer[(5, y)].fg, foreground(Role::Plain));
    }
    assert!(row(&buffer, 2).ends_with("● "));
    assert!(row(&buffer, 3).ends_with("○ "));
    assert!(row(&buffer, 4).ends_with("● ○ "));
    assert!(row(&buffer, 5).ends_with("! "));
    assert_eq!(
        buffer[(36, 4)].fg,
        file_side_style(DiffSide::Index).fg.unwrap()
    );
    assert_eq!(
        buffer[(38, 4)].fg,
        file_side_style(DiffSide::Worktree).fg.unwrap()
    );
    assert!(buffer[(3, 0)].modifier.contains(Modifier::BOLD));
    assert!(!buffer[(5, 3)].modifier.contains(Modifier::BOLD));
    assert_ne!(buffer[(0, 3)].bg, buffer[(0, 2)].bg);
    assert_eq!(buffer[(0, 3)].bg, buffer[(39, 3)].bg);
    assert_ne!(buffer[(0, 4)].bg, buffer[(0, 2)].bg);
    for y in 0..6 {
        assert!(!row(&buffer, y).contains(['┃', '├', '└']));
    }
}

#[test]
fn explorer_clips_deep_unicode_rows_and_keeps_controls_fixed_when_scrolling() {
    let items = [item(
        "長いディレクトリ/very-long-component-name.tsx",
        30,
        ItemKind::Folder,
        FileState {
            staged: true,
            unstaged: true,
            ..Default::default()
        },
    )];
    for width in 0..50 {
        let mut buffer = Buffer::filled(Rect::new(0, 0, 60, 3), ratatui::buffer::Cell::new("·"));
        let area = Rect::new(2, 1, width, 1);
        let mut state = ViewState {
            nav_horizontal: usize::MAX,
            ..Default::default()
        };
        let disclosures = render_navigator(&mut buffer, area, &mut state, &items);
        for x in 0..60 {
            if x < area.x || x >= area.right() {
                assert_eq!(buffer[(x, 1)].symbol(), "·", "width {width}, x {x}");
            }
        }
        for (_, hit) in disclosures {
            assert!(area.contains(hit.as_position()));
            assert!(hit.right() <= area.right());
            assert_eq!(buffer[(hit.x, hit.y)].symbol(), "▸");
        }
        if width >= 22 {
            assert!(row(&buffer, 1).contains("tsx"));
            assert_eq!(buffer[(area.right() - 4, 1)].symbol(), "●");
            assert_eq!(buffer[(area.right() - 2, 1)].symbol(), "○");
        }
    }
}

#[test]
fn file_icons_handle_names_case_and_unknown_extensions() {
    assert_eq!(file_icons::for_path("src/APP.TSX"), "\u{e625}");
    assert_eq!(file_icons::for_path("types/index.d.ts"), "\u{e628}");
    assert_eq!(file_icons::for_path("config/init.lua"), "\u{e620}");
    assert_eq!(file_icons::for_path("build/Dockerfile"), "\u{e7b0}");
    assert_eq!(file_icons::for_path(".gitignore"), "\u{e702}");
    assert_eq!(file_icons::for_path("unknown.xyz"), "\u{e64e}");
    assert_eq!(file_icons::for_path("LICENSE"), "\u{e64e}");
}

#[test]
fn homogeneous_subdirectories_replace_file_markers_with_colored_directory_names() {
    let mixed = FileState {
        staged: true,
        unstaged: true,
        expanded: true,
        ..Default::default()
    };
    let staged = FileState {
        staged: true,
        expanded: true,
        ..Default::default()
    };
    let unstaged = FileState {
        unstaged: true,
        expanded: true,
        ..Default::default()
    };
    let items = [
        item("project", 0, ItemKind::Folder, mixed),
        item("staged", 1, ItemKind::Folder, staged),
        item("one.ts", 2, ItemKind::File, staged),
        item("two.ts", 2, ItemKind::File, staged),
        item("unstaged", 1, ItemKind::Folder, unstaged),
        item("three.lua", 2, ItemKind::File, unstaged),
        item(
            "mixed",
            1,
            ItemKind::Folder,
            FileState {
                expanded: false,
                ..mixed
            },
        ),
        item("root.ts", 1, ItemKind::File, staged),
        item("clean.txt", 1, ItemKind::File, FileState::default()),
    ];
    let area = Rect::new(0, 0, 34, 9);
    let mut buffer = Buffer::empty(area);
    render_navigator(&mut buffer, area, &mut ViewState::default(), &items);
    for y in [0, 1, 2, 3, 4, 5, 8] {
        assert!(!row(&buffer, y).contains(['●', '○']), "row {y}");
    }
    assert_eq!(
        buffer[(5, 1)].fg,
        file_side_style(DiffSide::Index).fg.unwrap()
    );
    assert_eq!(
        buffer[(5, 4)].fg,
        file_side_style(DiffSide::Worktree).fg.unwrap()
    );
    for y in [0, 1, 4, 6] {
        let icon = if y == 0 { 1 } else { 3 };
        assert_eq!(buffer[(icon, y)].fg, buffer[(icon + 2, y)].fg);
    }
    for y in [2, 3, 5] {
        assert_eq!(buffer[(7, y)].fg, foreground(Role::Plain));
    }
    assert!(row(&buffer, 6).ends_with("● ○ "));
    assert!(row(&buffer, 7).ends_with("● "));

    // Parent context must survive when it is above the viewport.
    let viewport = Rect::new(0, 0, 34, 2);
    let mut buffer = Buffer::empty(viewport);
    let mut state = ViewState {
        selected: 2,
        offset: 2,
        nav_manual_scroll: true,
        ..Default::default()
    };
    render_navigator(&mut buffer, viewport, &mut state, &items);
    assert!(row(&buffer, 0).contains("one.ts"));
    assert!(!row(&buffer, 0).contains(['●', '○']));
    assert!(!row(&buffer, 1).contains(['●', '○']));
}

#[test]
fn uniform_tree_with_many_files_is_quiet_and_preserves_conflict_warnings() {
    for side in [DiffSide::Index, DiffSide::Worktree] {
        let file = FileState {
            staged: side == DiffSide::Index,
            unstaged: side == DiffSide::Worktree,
            expanded: true,
            ..Default::default()
        };
        let mut items = vec![item("src", 0, ItemKind::Folder, file)];
        items.extend((0..20).map(|i| item(&format!("file-{i}.ts"), 1, ItemKind::File, file)));
        let area = Rect::new(0, 0, 34, 22);
        let mut buffer = Buffer::empty(area);
        render_navigator(&mut buffer, area, &mut ViewState::default(), &items);
        assert_eq!(buffer[(3, 0)].fg, file_side_style(side).fg.unwrap());
        assert_eq!(buffer[(1, 0)].fg, buffer[(3, 0)].fg);
        for y in 0..21 {
            assert!(!row(&buffer, y).contains(['●', '○', '│']));
        }
        items[0].file_state.as_mut().unwrap().expanded = false;
        let mut buffer = Buffer::empty(area);
        render_navigator(&mut buffer, area, &mut ViewState::default(), &items[..1]);
        assert!(row(&buffer, 0).contains("▸ src"));
        assert_eq!(buffer[(1, 0)].fg, buffer[(3, 0)].fg);
        assert!(!row(&buffer, 0).contains(['●', '○']));
        assert_eq!(buffer[(3, 0)].fg, file_side_style(side).fg.unwrap());

        items[0].file_state.as_mut().unwrap().expanded = true;
        items[1].file_state.as_mut().unwrap().conflicted = true;
        let mut buffer = Buffer::empty(area);
        render_navigator(&mut buffer, area, &mut ViewState::default(), &items);
        assert_eq!(buffer[(32, 1)].symbol(), "!");
    }
}

#[test]
fn folder_transition_keeps_its_hit_target_and_label_fixed() {
    use std::time::Duration;

    let area = Rect::new(0, 0, 34, 1);
    for expanded in [false, true] {
        let items = [item(
            "src",
            0,
            ItemKind::Folder,
            FileState {
                expanded,
                ..Default::default()
            },
        )];
        let before = if expanded { "▸" } else { "\u{f07c}" };
        let after = if expanded { "\u{f07c}" } else { "▸" };
        for (ms, icon) in [
            (0, before),
            (40, before),
            (80, " "),
            (120, after),
            (160, after),
        ] {
            let mut buffer = Buffer::empty(area);
            let mut state = ViewState {
                folder_animation: Some(FolderAnimation {
                    row: 0,
                    elapsed: Duration::from_millis(ms),
                }),
                ..Default::default()
            };
            let disclosures = render_navigator(&mut buffer, area, &mut state, &items);
            assert_eq!(disclosures, vec![(0, Rect::new(1, 0, 2, 1))]);
            assert_eq!(buffer[(1, 0)].symbol(), icon);
            assert_eq!(buffer[(3, 0)].symbol(), "s");
            assert_eq!(buffer[(1, 0)].bg, buffer[(3, 0)].bg);
            if ms == 0 || ms == 160 {
                assert_eq!(buffer[(1, 0)].fg, buffer[(3, 0)].fg);
            }
            if ms == 40 || ms == 120 {
                let (_, steady) =
                    folder::appearance(icon == "\u{f07c}", None, tree_row_style(true, true, false));
                assert_ne!(buffer[(1, 0)].style(), steady);
            }
        }
        // Reversing a toggle at the complementary progress preserves its visible frame.
        let row_style = tree_row_style(true, true, false);
        for ms in [0, 20, 40, 60, 80, 100, 120, 140, 160] {
            let forward = folder::appearance(
                expanded,
                Some(FolderAnimation {
                    row: 0,
                    elapsed: Duration::from_millis(ms),
                }),
                row_style,
            );
            let reverse = folder::appearance(
                !expanded,
                Some(FolderAnimation {
                    row: 0,
                    elapsed: Duration::from_millis(160 - ms),
                }),
                row_style,
            );
            assert_eq!(forward.0, reverse.0);
            if ms != 80 {
                assert_eq!(forward.1, reverse.1);
            }
        }
    }
}

#[test]
fn directory_guides_connect_siblings_and_stop_before_trailing_file_lists() {
    let directory = FileState {
        expanded: true,
        ..Default::default()
    };
    let items = [
        item("root", 0, ItemKind::Folder, directory),
        item("src", 1, ItemKind::Folder, directory),
        item("components", 2, ItemKind::Folder, directory),
        item("button.tsx", 3, ItemKind::File, FileState::default()),
        item("hooks", 2, ItemKind::Folder, FileState::default()),
        item("index.ts", 2, ItemKind::File, FileState::default()),
        item("tests", 1, ItemKind::Folder, directory),
        item("tree.rs", 2, ItemKind::File, FileState::default()),
    ];
    let area = Rect::new(0, 0, 34, 8);
    let mut buffer = Buffer::empty(area);
    let mut state = ViewState {
        selected: 4,
        ..Default::default()
    };
    let disclosures = render_navigator(&mut buffer, area, &mut state, &items);
    assert_eq!(buffer[(3, 2)].symbol(), "╷");
    assert_eq!(buffer[(3, 5)].symbol(), "╵");
    for y in 3..5 {
        assert_eq!(buffer[(3, y)].symbol(), "│");
    }
    // Components and hooks are leaf directories, so they do not get their own rail.
    assert_eq!(buffer[(5, 3)].symbol(), " ");
    for y in 0..area.height {
        assert_eq!(buffer[(2, y)].symbol(), " ");
    }
    assert!(!row(&buffer, 7).contains(['│', '╷', '╵']));
    assert_ne!(buffer[(3, 4)].fg, buffer[(3, 4)].bg);
    assert!(!buffer[(3, 4)].modifier.contains(Modifier::DIM));
    assert_eq!(buffer[(3, 4)].bg, buffer[(6, 4)].bg);
    for (row, hit) in disclosures {
        assert_eq!(hit.width, 2);
        assert_eq!(buffer[(hit.x, hit.y)].fg, buffer[(hit.right(), hit.y)].fg);
        assert_eq!(
            buffer[(hit.right(), hit.y)].symbol(),
            &items[row].label[..1]
        );
    }

    // Scrolling away the parents retains exactly the same guide segments.
    let viewport = Rect::new(0, 0, 34, 4);
    let mut scrolled = Buffer::empty(viewport);
    state.offset = 3;
    state.nav_manual_scroll = true;
    render_navigator(&mut scrolled, viewport, &mut state, &items);
    for y in 0..4 {
        for x in [3, 5] {
            assert_eq!(scrolled[(x, y)].symbol(), buffer[(x, y + 3)].symbol());
        }
    }

    // Guides may never overwrite icons when a narrow pane compresses the indentation.
    for width in 1..=34 {
        let area = Rect::new(2, 1, width, 8);
        let mut buffer = Buffer::filled(Rect::new(0, 0, 40, 10), ratatui::buffer::Cell::new("·"));
        let disclosures = render_navigator(&mut buffer, area, &mut ViewState::default(), &items);
        for (row, hit) in disclosures {
            let expected = if items[row].file_state.unwrap().expanded {
                "\u{f07c}"
            } else {
                "▸"
            };
            assert_eq!(
                buffer[(hit.x, hit.y)].symbol(),
                expected,
                "width {width}, row {row}"
            );
        }
        for y in area.y..area.bottom() {
            assert_eq!(buffer[(area.x - 1, y)].symbol(), "·");
            assert_eq!(buffer[(area.right(), y)].symbol(), "·");
        }
    }
}

#[test]
fn directory_guides_skip_single_chains_and_leaf_siblings() {
    let directory = FileState {
        expanded: true,
        ..Default::default()
    };
    let items: Vec<_> = [
        ("dotfiles", 0, true),
        ("scripts", 1, true),
        ("rust", 2, true),
        ("crates", 3, true),
        ("git", 4, true),
        ("dogit", 5, true),
        ("src", 6, true),
        ("app", 7, true),
        ("tree.rs", 8, false),
        ("view", 7, true),
        ("navigator.rs", 8, false),
        ("tests", 6, true),
        ("unit", 7, true),
        ("navigator.rs", 8, false),
        ("shared", 1, true),
        ("wezterm", 2, true),
        ("keymap", 3, true),
        ("init.lua", 4, false),
    ]
    .into_iter()
    .map(|(label, depth, folder)| {
        item(
            label,
            depth,
            if folder {
                ItemKind::Folder
            } else {
                ItemKind::File
            },
            if folder {
                directory
            } else {
                FileState::default()
            },
        )
    })
    .collect();
    let area = Rect::new(0, 0, 52, items.len() as u16);
    let mut buffer = Buffer::empty(area);
    render_navigator(&mut buffer, area, &mut ViewState::default(), &items);
    for y in 0..area.height {
        for x in 0..area.width {
            let expected = (x == 3 && (2..14).contains(&y)) || (x == 13 && (7..11).contains(&y));
            assert_eq!(
                matches!(buffer[(x, y)].symbol(), "│" | "╷" | "╵"),
                expected,
                "x {x}, row {y}"
            );
        }
    }

    // Collapsing both branches leaves a pair of visible leaf directories.
    let mut collapsed = vec![items[0].clone(), items[1].clone(), items[14].clone()];
    for item in &mut collapsed[1..] {
        item.file_state.as_mut().unwrap().expanded = false;
    }
    let mut buffer = Buffer::empty(area);
    render_navigator(&mut buffer, area, &mut ViewState::default(), &collapsed);
    for y in 0..area.height {
        assert!(!row(&buffer, y).contains(['│', '╷', '╵']));
    }
}
