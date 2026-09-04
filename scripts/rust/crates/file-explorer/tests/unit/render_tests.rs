use super::*;
use crate::{DirectoryStatus, Entry};

fn directory(names: &[(&str, EntryKind)]) -> Directory<usize> {
    Directory {
        location: 1,
        parent: Some(0),
        label: "remote:~/projects".into(),
        entries: names
            .iter()
            .enumerate()
            .map(|(location, (name, kind))| Entry {
                location,
                name: (*name).into(),
                kind: *kind,
            })
            .collect(),
        status: DirectoryStatus::Present,
    }
}

fn context<'a>(directory: &'a Directory<usize>, rows: &'a [usize]) -> RenderContext<'a, usize> {
    RenderContext {
        directory,
        rows,
        cursor: 0,
        offset: 0,
        prompt: None,
        error: None,
        selection: None,
        help: false,
    }
}

fn frame(directory: &Directory<usize>, rows: &[usize], size: Size) -> RenderedFrame {
    render(
        &context(directory, rows),
        &crate::DefaultView,
        &Style::plain(),
        size,
        RenderLimits::default(),
    )
}

#[test]
fn every_line_fits_the_terminal_in_cells() {
    let directory = directory(&[
        ("東京の長い名前", EntryKind::Directory),
        ("notes", EntryKind::File),
    ]);
    let rendered = frame(
        &directory,
        &[0, 1],
        Size {
            width: 18,
            height: 8,
        },
    );

    assert!(
        rendered
            .lines
            .iter()
            .all(|line| UnicodeWidthStr::width(line.as_str()) <= 18)
    );
    assert!(rendered.lines.iter().any(|line| line.contains('…')));
}

#[test]
fn narrow_terminals_do_not_force_a_minimum_box_width() {
    let directory = directory(&[("a-very-long-name", EntryKind::File)]);

    for width in 1..12 {
        let rendered = frame(&directory, &[0], Size { width, height: 5 });
        assert!(
            rendered
                .lines
                .iter()
                .all(|line| UnicodeWidthStr::width(line.as_str()) <= width)
        );
    }
}

#[test]
fn short_terminals_never_receive_too_many_lines() {
    let directory = directory(&[("one", EntryKind::File), ("two", EntryKind::File)]);

    for height in 0..8 {
        let rendered = frame(&directory, &[0, 1], Size { width: 40, height });
        assert!(rendered.lines.len() <= height);
    }
}

#[test]
fn terminal_controls_and_direction_overrides_are_visible_text() {
    let directory = directory(&[("safe\x1b[31m\n\u{202e}txt", EntryKind::File)]);
    let rendered = frame(
        &directory,
        &[0],
        Size {
            width: 70,
            height: 5,
        },
    );
    let joined = rendered.lines.join("\n");

    assert!(!joined.contains('\x1b'));
    assert!(!joined.contains('\u{202e}'));
    assert!(joined.contains("\\u{1b}"));
    assert!(joined.contains("\\n"));
    assert!(joined.contains("\\u{202e}"));
}

#[test]
fn directory_names_receive_a_suffix_only_for_display() {
    let directory = directory(&[("src", EntryKind::Directory)]);
    let rendered = frame(
        &directory,
        &[0],
        Size {
            width: 40,
            height: 5,
        },
    );

    assert!(rendered.lines.iter().any(|line| line.contains("src/")));
    assert_eq!(directory.entries[0].name, "src");
}

#[test]
fn prompt_and_inline_errors_are_rendered_safely() {
    let directory = directory(&[]);
    let mut context = context(&directory, &[]);
    context.prompt = Some(("bad\tname", InputKind::Search));
    context.error = Some("denied\x1b[2J");
    let rendered = render(
        &context,
        &crate::DefaultView,
        &Style::plain(),
        Size {
            width: 50,
            height: 7,
        },
        RenderLimits::default(),
    );
    let joined = rendered.lines.join("\n");

    assert!(joined.contains("bad\\tname"));
    assert!(joined.contains("denied\\u{1b}[2J"));
    assert!(!joined.contains('\x1b'));
}

#[test]
fn missing_empty_and_no_match_states_are_distinct() {
    let mut directory = directory(&[]);
    let empty = frame(
        &directory,
        &[],
        Size {
            width: 50,
            height: 5,
        },
    )
    .lines
    .join("\n");
    directory.status = DirectoryStatus::Missing;
    let missing = frame(
        &directory,
        &[],
        Size {
            width: 50,
            height: 5,
        },
    )
    .lines
    .join("\n");
    directory.status = DirectoryStatus::Present;
    let mut context = context(&directory, &[]);
    context.prompt = Some(("xyz", InputKind::Search));
    let no_match = render(
        &context,
        &crate::DefaultView,
        &Style::plain(),
        Size {
            width: 50,
            height: 5,
        },
        RenderLimits::default(),
    )
    .lines
    .join("\n");

    assert!(empty.contains("(empty)"));
    assert!(missing.contains("(not found)"));
    assert!(no_match.contains("(no matches)"));
}

#[test]
fn viewport_reserves_a_row_for_overflow_status() {
    let names: Vec<(String, EntryKind)> = (0..20)
        .map(|index| (format!("entry-{index}"), EntryKind::File))
        .collect();
    let directory = Directory {
        location: 1,
        parent: Some(0),
        label: "many".into(),
        entries: names
            .iter()
            .enumerate()
            .map(|(location, (name, kind))| Entry {
                location,
                name: name.clone(),
                kind: *kind,
            })
            .collect(),
        status: DirectoryStatus::Present,
    };
    let rows: Vec<usize> = (0..20).collect();
    let rendered = frame(
        &directory,
        &rows,
        Size {
            width: 50,
            height: 8,
        },
    );

    assert_eq!(rendered.viewport_rows, 4);
    assert!(rendered.lines.iter().any(|line| line.contains("below")));
}

#[test]
fn stale_offset_does_not_create_overflow_when_every_row_fits() {
    let entries: Vec<_> = (0..5)
        .map(|index| (format!("entry-{index}"), EntryKind::File))
        .collect();
    let borrowed: Vec<_> = entries
        .iter()
        .map(|(name, kind)| (name.as_str(), *kind))
        .collect();
    let directory = directory(&borrowed);
    let rows = [0, 1, 2, 3, 4];
    let mut context = context(&directory, &rows);
    context.offset = 1;
    let rendered = render(
        &context,
        &crate::DefaultView,
        &Style::plain(),
        Size {
            width: 40,
            height: 8,
        },
        RenderLimits::default(),
    );

    assert_eq!(rendered.viewport_rows, 5);
    assert!(!rendered.lines.iter().any(|line| line.contains("above")));
}

struct BadgedView;

impl ExplorerView<usize> for BadgedView {
    fn badge(&self, _context: &ViewContext<'_, usize>, _entry: &Entry<usize>) -> Option<Line> {
        Some(Line::styled("mirror", Role::Success))
    }

    fn accept_label(&self, _context: &ViewContext<'_, usize>) -> String {
        "push here".into()
    }
}

#[test]
fn badges_and_application_accept_labels_share_the_layout() {
    let directory = directory(&[("src", EntryKind::Directory)]);
    let rendered = render(
        &context(&directory, &[0]),
        &BadgedView,
        &Style::plain(),
        Size {
            width: 50,
            height: 5,
        },
        RenderLimits::default(),
    );
    let joined = rendered.lines.join("\n");

    assert!(joined.contains("mirror"));
    assert!(joined.contains("push here"));
}

#[test]
fn help_replaces_the_regular_footer() {
    let directory = directory(&[("src", EntryKind::Directory)]);
    let mut context = context(&directory, &[0]);
    context.help = true;
    let rendered = render(
        &context,
        &crate::DefaultView,
        &Style::plain(),
        Size {
            width: 100,
            height: 5,
        },
        RenderLimits {
            max_width: 100,
            ..RenderLimits::default()
        },
    );
    let joined = rendered.lines.join("\n");

    assert!(joined.contains("pgup/pgdn"));
    assert!(joined.contains("r refresh"));
    assert!(!joined.contains("? help"));
}

#[test]
fn colored_semantic_spans_do_not_change_cell_width() {
    let line = Line::from_spans([
        Span::new("green", Role::Success),
        Span::new(" danger", Role::Danger),
    ]);
    let painted = paint(&line, &Style::for_stdout_with_color(true), 20);

    assert_eq!(workstation::screen::width(&painted), 12);
    assert!(painted.contains("\x1b["));
}
