use super::*;

#[test]
fn gallery_uses_live_selection_and_diff_controls() {
    let mut gallery = Gallery::default();
    input(
        &mut gallery,
        KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        24,
    );
    input(
        &mut gallery,
        KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        24,
    );
    input(
        &mut gallery,
        KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        24,
    );
    assert_eq!(gallery.selection.selected(), [0, 1]);
    input(
        &mut gallery,
        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        24,
    );
    input(
        &mut gallery,
        KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE),
        24,
    );
    assert_eq!(gallery.diff_state.mode, ui_diff_view::ViewMode::SideBySide);
}

#[test]
fn preview_renders_shared_components_and_fits_narrow_terminals() {
    for width in [1, 20, 60, 100] {
        let lines = preview(&Palette::default(), width, false);
        assert!(
            lines
                .iter()
                .all(|line| ui_terminal::text::width(line) <= width)
        );
        assert!(lines.iter().all(|line| !line.contains('\x1b')));
    }
    let text = preview(&Palette::default(), 100, false).join("\n");
    for expected in [
        "Local files",
        "Comparison",
        "repo",
        "live",
        "7 / 12",
        "Warning",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
}

#[test]
fn diff_end_uses_the_pane_height_on_narrow_and_wide_layouts() {
    for width in [42, 60, 100] {
        let mut gallery = Gallery::default();
        let source = (0..80)
            .map(|index| format!("line {index}\n"))
            .collect::<String>();
        gallery.diff = DiffDocument::new(&source, &source.replace("line", "change"));
        gallery.diff_state.mode = ui_diff_view::ViewMode::SideBySide;
        gallery.diff_active = true;
        let area = Rect::new(0, 0, width, 24);
        let mut buffer = Buffer::empty(area);
        let palette = Palette::default();
        render(&mut gallery, area, &mut buffer, &palette, false, 0);
        input(
            &mut gallery,
            KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
            area.height,
        );
        buffer.reset();
        render(&mut gallery, area, &mut buffer, &palette, false, 0);
        let text = buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("change 79"), "{width}: {text}");
    }
}

#[test]
fn ansi16_preview_uses_basic_color_escapes() {
    let lines = preview(
        &Palette::default().with_depth(ui_theme::ColorDepth::Ansi16),
        100,
        true,
    )
    .join("\n");
    assert!(lines.contains("\x1b[35m") || lines.contains("\x1b[90m"));
    assert!(!lines.contains("38;5;") && !lines.contains("48;5;"));
}

#[test]
fn resizing_the_gallery_preserves_split_preference_after_narrow_navigation() {
    let mut gallery = Gallery::default();
    let source = (0..80)
        .map(|index| format!("line {index}\n"))
        .collect::<String>();
    gallery.diff = DiffDocument::new(&source, &source.replace("line", "change"));
    gallery.diff_state = ViewState::new(ui_diff_view::ViewMode::SideBySide);
    gallery.diff_active = true;
    let palette = Palette::default();
    for width in [100, 42, 100] {
        let area = Rect::new(0, 0, width, 24);
        let mut buffer = Buffer::empty(area);
        render(&mut gallery, area, &mut buffer, &palette, false, 0);
        let layout = if width < 44 {
            ui_diff_view::ViewMode::Unified
        } else {
            ui_diff_view::ViewMode::SideBySide
        };
        assert_eq!(gallery.diff_state.effective_mode(), layout);
        assert_eq!(gallery.diff_state.mode, ui_diff_view::ViewMode::SideBySide);
        if width < 44 {
            for code in [KeyCode::End, KeyCode::Char('v'), KeyCode::Char('v')] {
                input(
                    &mut gallery,
                    KeyEvent::new(code, KeyModifiers::NONE),
                    area.height,
                );
            }
            buffer.reset();
            render(&mut gallery, area, &mut buffer, &palette, false, 0);
        }
        if width == 42 || gallery.diff_state.offset > 0 {
            let text = buffer
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(text.contains("change 79"), "{width}: {text}");
        }
    }
}
