use super::*;
use ratatui::{Terminal, backend::TestBackend};
use std::fs;

fn repository() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .arg(directory.path())
            .status()
            .unwrap()
            .success()
    );
    directory
}

fn app(directory: &Path) -> App {
    let mut app = App::new(Repo::open(directory).unwrap(), Start::History);
    app.view.focus = Focus::Canvas;
    app.view.canvas_manual_scroll = true;
    app.view.commit_manual_scroll = true;
    app
}

fn commit(body_rows: usize) -> crate::commit::CommitDetails {
    let body = (0..body_rows)
        .map(|n| format!("Message line {n:03}"))
        .collect::<Vec<_>>()
        .join("\n");
    crate::commit::CommitDetails {
        subject: "Preview subject".into(),
        message: Some(format!("Preview subject\n\n{body}")),
        author: "Example Author".into(),
        oid: "0123456789abcdef".into(),
        signature: Some("G".into()),
        ..crate::commit::CommitDetails::default()
    }
}

fn draw(app: &mut App, terminal: &mut Terminal<TestBackend>, data: &RenderData<'_>) -> Vec<String> {
    terminal
        .draw(|frame| {
            app.geometry = Some(view::render(frame, &mut app.view, data));
        })
        .unwrap();
    let area = app.geometry.as_ref().unwrap().canvas_viewport;
    let buffer = terminal.backend().buffer();
    (area.y..area.bottom())
        .map(|y| {
            (area.x..area.right().saturating_sub(1))
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect()
}

#[test]
fn preview_and_content_scroll_as_one_document() {
    let directory = repository();
    let lines = (0..100)
        .map(|n| format!("Diff line {n:03}"))
        .collect::<Vec<_>>();
    for width in [60, 100, 160] {
        for body_rows in [0, 3, 60] {
            let commit = commit(body_rows);
            let data = RenderData {
                commit: Some(&commit),
                canvas_lines: &lines,
                ..RenderData::default()
            };
            let mut reference = app(directory.path());
            let mut full = Terminal::new(TestBackend::new(width, 240)).unwrap();
            let expected = draw(&mut reference, &mut full, &data);
            let mut app = app(directory.path());
            let mut terminal = Terminal::new(TestBackend::new(width, 28)).unwrap();
            draw(&mut app, &mut terminal, &data);
            let prefix = app.geometry.as_ref().unwrap().commit_height;
            let bar = app
                .geometry
                .as_ref()
                .unwrap()
                .scrollbar(view::ScrollRegion::Canvas)
                .unwrap();
            for offset in 0..=bar.maximum {
                app.view.commit_offset = offset;
                app.view.diff_offset = offset.saturating_sub(prefix);
                let actual = draw(&mut app, &mut terminal, &data);
                assert_eq!(
                    actual,
                    expected[offset..offset + actual.len()],
                    "width {width}, body {body_rows}, offset {offset}"
                );
                let geometry = app.geometry.as_ref().unwrap();
                assert_eq!(app.view.commit_offset, offset);
                assert!(
                    geometry
                        .scrollbar(view::ScrollRegion::CommitDetails)
                        .is_none()
                );
                assert_eq!(
                    geometry.commit_details.height as usize,
                    prefix.saturating_sub(offset).min(actual.len())
                );
                if offset >= prefix {
                    assert_eq!(geometry.canvas, geometry.canvas_viewport);
                    assert!(geometry.commit_fields.is_empty());
                }
            }
        }
    }
}

#[test]
fn mouse_paging_and_edges_share_the_scroll_position() {
    let directory = repository();
    let commit = commit(3);
    let lines = (0..100)
        .map(|n| format!("Diff line {n:03}"))
        .collect::<Vec<_>>();
    let data = RenderData {
        commit: Some(&commit),
        canvas_lines: &lines,
        ..RenderData::default()
    };
    for over_preview in [true, false] {
        let mut app = app(directory.path());
        let mut terminal = Terminal::new(TestBackend::new(120, 32)).unwrap();
        draw(&mut app, &mut terminal, &data);
        let geometry = app.geometry.as_ref().unwrap();
        let area = if over_preview {
            geometry.commit_details
        } else {
            geometry.canvas
        };
        app.wheel(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: area.x + 4,
            row: area.y + 1,
            modifiers: KeyModifiers::NONE,
        });
        app.animate_scroll(true);
        draw(&mut app, &mut terminal, &data);
        assert_eq!(app.view.commit_offset, 1);
        for focus in [Focus::CommitDetails, Focus::Canvas] {
            app.view.focus = focus;
            let before = app.view.commit_offset;
            app.key(
                KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
                &mut None,
                &mut None,
            )
            .unwrap();
            draw(&mut app, &mut terminal, &data);
            assert!(app.view.commit_offset > before);
            assert!(app.geometry.as_ref().unwrap().commit_details.is_empty());
        }
        app.key(
            KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
            &mut None,
            &mut None,
        )
        .unwrap();
        let bottom = draw(&mut app, &mut terminal, &data).join("\n");
        assert!(bottom.contains("Diff line 099"));
        app.key(
            KeyEvent::new(KeyCode::Home, KeyModifiers::NONE),
            &mut None,
            &mut None,
        )
        .unwrap();
        let top = draw(&mut app, &mut terminal, &data).join("\n");
        assert_eq!(app.view.commit_offset, 0);
        assert!(top.contains("Preview subject"));
        assert!(top.contains("Example Author"));
        app.key(
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
            &mut None,
            &mut None,
        )
        .unwrap();
        draw(&mut app, &mut terminal, &data);
        app.key(
            KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
            &mut None,
            &mut None,
        )
        .unwrap();
        draw(&mut app, &mut terminal, &data);
        assert_eq!(app.view.commit_offset, 0);
    }
}

#[test]
fn real_diff_scrolls_without_losing_rows_or_hit_targets() {
    let directory = repository();
    let content = (0..80)
        .map(|n| format!("Added line {n:03}\n"))
        .collect::<String>();
    fs::write(directory.path().join("example.txt"), &content).unwrap();
    assert!(
        Command::new("git")
            .current_dir(directory.path())
            .args(["add", "example.txt"])
            .status()
            .unwrap()
            .success()
    );
    let diff =
        ExactDiff::load(directory.path(), Path::new("example.txt"), DiffSide::Index).unwrap();
    let commit = commit(40);
    for split in [false, true] {
        let data = RenderData {
            commit: Some(&commit),
            diff: Some(&diff),
            ..RenderData::default()
        };
        let mut reference = app(directory.path());
        reference.view.split = split;
        let mut full = Terminal::new(TestBackend::new(180, 240)).unwrap();
        let expected = draw(&mut reference, &mut full, &data);
        let mut app = app(directory.path());
        app.view.split = split;
        let mut terminal = Terminal::new(TestBackend::new(180, 28)).unwrap();
        draw(&mut app, &mut terminal, &data);
        let prefix = app.geometry.as_ref().unwrap().commit_height;
        let maximum = app
            .geometry
            .as_ref()
            .unwrap()
            .scrollbar(view::ScrollRegion::Canvas)
            .unwrap()
            .maximum;
        for offset in 0..=maximum {
            app.view.commit_offset = offset;
            app.view.diff_offset = offset.saturating_sub(prefix);
            let actual = draw(&mut app, &mut terminal, &data);
            assert_eq!(
                actual,
                expected[offset..offset + actual.len()],
                "split {split}, offset {offset}"
            );
            let geometry = app.geometry.as_ref().unwrap();
            assert!(geometry.line_ids.len() <= geometry.canvas.height as usize);
        }
    }
}

#[test]
fn small_viewports_empty_diffs_and_field_navigation_remain_usable() {
    let directory = repository();
    let commit = commit(60);
    let data = RenderData {
        commit: Some(&commit),
        ..RenderData::default()
    };
    for width in [20, 60, 100] {
        for height in [10, 14, 28] {
            let mut app = app(directory.path());
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            draw(&mut app, &mut terminal, &data);
            app.canvas_edge(true);
            draw(&mut app, &mut terminal, &data);
            app.view.focus = Focus::CommitDetails;
            app.view.commit_manual_scroll = false;
            for field in [0, 1, 2] {
                app.view.commit_field = field;
                draw(&mut app, &mut terminal, &data);
                assert!(
                    app.geometry
                        .as_ref()
                        .unwrap()
                        .commit_fields
                        .iter()
                        .any(|(index, _)| *index == field)
                );
            }
            app.view.commit_offset = usize::MAX;
            app.view.diff_offset = 0;
            app.view.commit_manual_scroll = true;
            draw(&mut app, &mut terminal, &data);
            assert!(app.view.commit_offset < usize::MAX);
        }
    }
}
