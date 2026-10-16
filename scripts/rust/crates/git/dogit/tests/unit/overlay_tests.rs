use super::*;
use ratatui::backend::TestBackend;

fn contents(buffer: &Buffer) -> String {
    buffer
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>()
}

fn detail() -> crate::commit::CommitDetails {
    crate::commit::CommitDetails {
        oid: "402b900123456789012345678901234567890123456".into(),
        author: "Example Author".into(),
        authored_iso: "2026-10-15T23:30:00-07:00".into(),
        subject: "A prominent commit subject".into(),
        signature: Some("N".into()),
        github: Some(crate::commit::GitHubCommit {
            url: "https://github.com/owner/repo/commit/402b900123456789012345678901234567890123456"
                .into(),
        }),
        ..Default::default()
    }
}

#[test]
fn history_layout_keeps_metadata_and_subject_outside_scrolling_diff() {
    for inspecting_commit in [false, true] {
        let details = detail();
        let data = RenderData {
            commit: Some(&details),
            inspecting_commit,
            canvas_lines: &["diff body".into()],
            ..Default::default()
        };
        let mut state = ViewState {
            workspace: Workspace::History,
            nav_width: 50,
            ..Default::default()
        };
        let mut terminal = ratatui::Terminal::new(TestBackend::new(130, 42)).unwrap();
        let mut geometry = Geometry::default();
        terminal
            .draw(|frame| geometry = render(frame, &mut state, &data))
            .unwrap();
        let screen = contents(terminal.backend().buffer());
        assert!(!screen.contains("Commit details"));
        for text in [
            "A prominent commit subject",
            "Author:",
            "Example Author",
            "Commit:",
            "402b900",
            "Unsigned",
            "October 15, 2026",
            "GitHub:",
        ] {
            assert!(screen.contains(text), "missing {text}");
        }
        assert_eq!(geometry.commit_fields.len(), 4);
        assert!(geometry.commit_subject.x >= 50);
        assert!(geometry.canvas.y > geometry.commit_subject.y);
        assert!(geometry.canvas.height >= 8);
        if inspecting_commit {
            assert_eq!(geometry.commit_details.x, geometry.nav.x);
            assert!(geometry.nav.bottom() <= geometry.commit_details.y);
        } else {
            assert_eq!(geometry.commit_subject.y, geometry.nav.y);
            assert_eq!(geometry.commit_details.x, geometry.canvas.x);
            assert!(geometry.commit_details.bottom() <= geometry.canvas.y);
        }
    }
}

#[test]
fn verified_badge_matches_diff_badges_and_aligns_with_title() {
    let mut details = detail();
    details.signature = Some("G".into());
    let area = Rect::new(0, 0, 90, 16);
    let mut buffer = Buffer::empty(area);
    let (_, title) = commit::details(&mut buffer, area, &ViewState::default(), &details, None);
    assert_eq!(title.y, 0);
    let first_row: String = (0..area.width).map(|x| buffer[(x, 0)].symbol()).collect();
    assert!(first_row.contains(&details.subject));
    assert!(first_row.ends_with("  Verified  "));
    let mut reference = Buffer::empty(area);
    let diff_badge = chrome::render_badges(
        &mut reference,
        area,
        vec![Badge::new("+123".into(), style(Role::DiffAdded))],
        false,
    );
    let verified_cell = &buffer[(area.width - 3, 0)];
    let diff_cell = &reference[(diff_badge.x + 1, 0)];
    assert_eq!(verified_cell.style(), diff_cell.style());
}

#[test]
fn narrow_and_short_views_keep_selected_metadata_action_visible() {
    for (width, height) in [(40, 15), (79, 24), (80, 20), (130, 42), (10, 5)] {
        for inspecting_commit in [false, true] {
            let details = detail();
            let data = RenderData {
                commit: Some(&details),
                inspecting_commit,
                ..Default::default()
            };
            let mut state = ViewState {
                workspace: Workspace::History,
                focus: Focus::CommitDetails,
                commit_field: 3,
                ..Default::default()
            };
            let mut terminal = ratatui::Terminal::new(TestBackend::new(width, height)).unwrap();
            let mut geometry = Geometry::default();
            terminal
                .draw(|frame| {
                    geometry = render(frame, &mut state, &data);
                })
                .unwrap();
            assert_eq!(
                canvas_width(Rect::new(0, 0, width, height), &state, inspecting_commit),
                geometry.canvas.width
            );
            if width >= 20 && height >= 15 {
                assert!(contents(terminal.backend().buffer()).contains("GitHub:"));
            }
        }
    }
}

#[test]
fn inspector_preserves_paragraphs_and_scrolls_to_last_line() {
    let message = format!(
        "Subject\n\n{}\nFinal trailer",
        "Body paragraph\n\n".repeat(60)
    );
    let outer = Rect::new(0, 0, 100, 30);
    let scroll = inspection_scroll_max(&message, outer);
    assert!(scroll > 0);
    let mut terminal = ratatui::Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal
        .draw(|frame| {
            render_overlay(
                frame,
                outer,
                &Overlay::Inspect {
                    message: message.clone(),
                    scroll,
                },
                &mut 0,
            );
        })
        .unwrap();
    let screen = contents(terminal.backend().buffer());
    assert!(screen.contains("Final trailer"));
    assert!(screen.contains("Inspect commit message"));
    assert!(screen.contains("Esc close"));
}
