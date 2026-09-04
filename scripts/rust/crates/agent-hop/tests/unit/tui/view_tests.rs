use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use super::*;
use crate::cli::Agent;
use crate::tui::{CatalogSnapshot, Origin, UiEvent};

fn model() -> Model {
    let mut model = Model::new();
    model.load(
        CatalogSnapshot {
            sessions: vec![SessionEntry {
                key: "remote:one".into(),
                id: "01999999-1111-7222-8333-444444444444".into(),
                agent: Agent::Claude,
                origin: Origin::Remote,
                host: Some("archie".into()),
                project: "dotfiles".into(),
                workspace: "/Users/me/dotfiles".into(),
                title: "Make the session picker beautiful".into(),
                updated: "8m".into(),
                current_project: true,
                favorite: true,
                disabled_reason: None,
                warning: Some("preview is partial".into()),
                sort_timestamp: 0,
            }],
            warnings: vec!["remote host took longer than expected".into()],
        },
        true,
    );
    model
}

fn rendered(width: u16, height: u16, mut model: Model) -> String {
    model.area = Rect::new(0, 0, width, height);
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            render(
                frame,
                &model,
                PickerOptions {
                    color: false,
                    reduced_motion: true,
                    initial_action: PickerAction::HopAndOpen,
                    initial_view: super::super::PickerView::default(),
                },
            )
        })
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .chunks(usize::from(width))
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn with_open_preview(mut model: Model, transition: u16) -> Model {
    model.set_reduced_motion(true);
    let _ = model.apply(UiEvent::Key(KeyEvent::new(
        KeyCode::Down,
        KeyModifiers::NONE,
    )));
    let _ = model.apply(UiEvent::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )));
    model.preview_transition = transition;
    model
}

#[test]
fn default_workspace_focuses_first_card_and_displays_its_preview() {
    let text = rendered(120, 28, model());
    assert!(text.contains("Sessions  1 / 1"), "{text:?}");
    assert!(
        text.contains("Make the session picker beautiful"),
        "{text:?}"
    );
    assert!(text.contains("╭"), "{text:?}");
    assert!(text.contains("╰"), "{text:?}");
    assert!(text.contains("[Claude]"), "{text:?}");
    assert!(text.contains("Find sessions"), "{text:?}");
    assert!(text.contains("Type to search"), "{text:?}");
    assert!(text.contains("[Origin All]"), "{text:?}");
    assert!(text.contains("[Agent All]"), "{text:?}");
    assert!(text.contains("[Scope All]"), "{text:?}");
    assert!(text.contains("dotfiles"), "{text:?}");
    assert!(text.contains("archie"), "{text:?}");
    assert!(text.contains("! 1"), "{text:?}");
    assert!(text.contains("Preview Conversation"), "{text:?}");
    assert!(text.contains("Loading transcript preview"), "{text:?}");
    assert!(!text.contains("agent-hop"), "{text:?}");
    assert!(!text.contains("WARNING"), "{text:?}");
    assert!(!text.contains('\u{b7}'), "{text:?}");
}

#[test]
fn open_preview_keeps_list_visible_and_compacts_it_on_the_left() {
    let model = with_open_preview(model(), 1_000);
    let regions = layout(Rect::new(0, 0, 120, 28));
    assert!(regions.list.width > 0);
    assert!(regions.preview.width > regions.list.width);
    assert_eq!(regions.list.y, regions.preview.y);

    let text = rendered(120, 28, model);
    assert!(text.contains("Sessions  1 / 1"), "{text:?}");
    assert!(text.contains("Preview Conversation"), "{text:?}");
    assert!(
        text.contains("Make the session picker beautiful"),
        "{text:?}"
    );
    assert!(text.contains("dotfiles  archie"), "{text:?}");
    assert!(text.contains("[←→] sessions"), "{text:?}");
    assert!(!text.contains('\u{b7}'), "{text:?}");
}

#[test]
fn narrow_preview_stacks_below_a_visible_two_line_card() {
    let model = with_open_preview(model(), 1_000);
    let regions = layout(Rect::new(0, 0, 48, 14));
    assert!(regions.list.height >= COMPACT_CARD_HEIGHT);
    assert!(regions.preview.height > 0);
    assert!(regions.preview.y > regions.list.y);
    assert_eq!(regions.list.width, regions.preview.width);

    let text = rendered(48, 14, model);
    assert!(text.contains("Sessions"), "{text:?}");
    assert!(text.contains("[Claude]"), "{text:?}");
    assert!(text.contains("Preview Conversation"), "{text:?}");
    assert!(!text.contains('\u{b7}'), "{text:?}");
}

#[test]
fn stacked_preview_follows_the_last_complete_card_row() {
    for area in [Rect::new(0, 0, 80, 24), Rect::new(0, 0, 80, 40)] {
        let regions = layout(area);
        let height = card_height(regions.list) as u16;

        assert_eq!(regions.list.height % height, 0);
        assert_eq!(regions.preview.y, regions.list.bottom().saturating_add(1));
    }
}

#[test]
fn split_workspace_is_stable_before_and_after_explicit_preview() {
    let area = Rect::new(0, 0, 120, 28);
    let workspace = layout(area);
    assert!(workspace.list.width > 0);
    assert!(workspace.preview.width > workspace.list.width);
    assert!(workspace.list.right() < workspace.preview.x);

    let inner = Rect::new(0, 0, 60, 20);
    let hidden = preview_reveal(inner, 0.0);
    let midpoint = preview_reveal(inner, 0.5);
    let shown = preview_reveal(inner, 1.0);
    assert!(hidden.is_empty());
    assert!(midpoint.width > 0 && midpoint.width < shown.width);
    assert_eq!(shown, inner);
}

#[test]
fn page_size_tracks_responsive_card_and_preview_geometry() {
    assert_eq!(list_page_size(Rect::new(0, 0, 120, 24)), 4);
    assert_eq!(list_page_size(Rect::new(0, 0, 100, 24)), 4);
    assert_eq!(list_page_size(Rect::new(0, 0, 80, 24)), 1);
    assert_eq!(list_page_size(Rect::new(0, 0, 48, 14)), 1);
}

#[test]
fn toolbar_shows_search_and_current_filter_values() {
    let all = rendered(100, 24, model());
    assert!(all.contains("[Origin All]"), "{all:?}");
    assert!(all.contains("[Agent All]"), "{all:?}");
    assert!(all.contains("[Scope All]"), "{all:?}");

    let mut filtered = model();
    filtered.origin_filter = OriginFilter::Remote;
    filtered.agent_filter = AgentFilter::Claude;
    filtered.scope_filter = ScopeFilter::Favorites;
    let text = rendered(100, 24, filtered);
    assert!(text.contains("[Origin Remote]"), "{text:?}");
    assert!(text.contains("[Agent Claude]"), "{text:?}");
    assert!(text.contains("[Scope Starred]"), "{text:?}");
}

#[test]
fn slash_focuses_an_empty_search_without_leaving_placeholder_text() {
    let mut focused = model();
    focused.apply(UiEvent::Key(KeyEvent::new(
        KeyCode::Char('/'),
        KeyModifiers::NONE,
    )));
    let text = rendered(100, 24, focused);
    assert!(!text.contains("Type to search"), "{text:?}");
    assert!(text.contains('▏'), "{text:?}");
}

#[test]
fn horizontal_focus_moves_the_accent_border_between_card_and_preview() {
    fn border_colors(mut model: Model) -> (Color, Color) {
        model.area = Rect::new(0, 0, 120, 28);
        let regions = layout(model.area);
        let backend = TestBackend::new(model.area.width, model.area.height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &model,
                    PickerOptions {
                        color: true,
                        ..PickerOptions::default()
                    },
                )
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        (
            buffer
                .cell(ratatui::layout::Position::new(
                    regions.list.x,
                    regions.list.y,
                ))
                .unwrap()
                .fg,
            buffer
                .cell(ratatui::layout::Position::new(
                    regions.preview.x,
                    regions.preview.y,
                ))
                .unwrap()
                .fg,
        )
    }

    let mut model = model();
    assert_eq!(
        border_colors(model.clone()),
        (Color::Rgb(167, 139, 250), Color::Rgb(71, 85, 105))
    );
    model.apply(UiEvent::Key(KeyEvent::new(
        KeyCode::Right,
        KeyModifiers::NONE,
    )));
    assert_eq!(
        border_colors(model.clone()),
        (Color::Rgb(71, 85, 105), Color::Rgb(167, 139, 250))
    );
    model.apply(UiEvent::Key(KeyEvent::new(
        KeyCode::Left,
        KeyModifiers::NONE,
    )));
    assert_eq!(
        border_colors(model),
        (Color::Rgb(167, 139, 250), Color::Rgb(71, 85, 105))
    );
}

#[test]
fn mouse_hit_testing_covers_toolbar_cards_preview_and_issues() {
    let mut model = model();
    model.area = Rect::new(0, 0, 120, 28);
    let regions = layout(model.area);
    let columns = toolbar_columns(toolbar_area(regions.header)).unwrap();
    for (area, item) in columns.into_iter().zip([
        ToolbarItem::Search,
        ToolbarItem::Origin,
        ToolbarItem::Agent,
        ToolbarItem::Scope,
    ]) {
        assert_eq!(
            hit_test(&model, ratatui::layout::Position::new(area.x, area.y)),
            HitTarget::Toolbar(item)
        );
    }
    assert_eq!(
        hit_test(
            &model,
            ratatui::layout::Position::new(regions.list.x, regions.list.y)
        ),
        HitTarget::Session(0)
    );
    assert_eq!(
        hit_test(
            &model,
            ratatui::layout::Position::new(
                regions.footer.right().saturating_sub(1),
                regions.footer.y,
            )
        ),
        HitTarget::Issues
    );

    let mut open = with_open_preview(model, 1_000);
    open.area = Rect::new(0, 0, 120, 28);
    let text = preview_text_area(&open);
    assert_eq!(
        hit_test(&open, ratatui::layout::Position::new(text.x, text.y)),
        HitTarget::PreviewText
    );
}

#[test]
fn review_and_help_are_rendered_as_modal_overlays() {
    let mut review = with_open_preview(model(), 1_000);
    review.mode = Mode::Review;
    let review_text = rendered(100, 26, review);
    assert!(review_text.contains("Transfer actions"));
    assert!(review_text.contains("Hop & open"));
    assert!(review_text.contains("Copy only"));
    assert!(review_text.contains("Dry run"));
    assert!(review_text.contains("[Space] opened actions"));
    assert!(!review_text.contains('\u{b7}'), "{review_text:?}");

    let mut help = model();
    help.mode = Mode::Help;
    let help_text = rendered(100, 26, help);
    assert!(help_text.contains("focus search from anywhere"));
    assert!(help_text.contains("select highlighted session"));
    assert!(help_text.contains("copy complete session details"));
    assert!(help_text.contains("copy only that text"));
    assert!(!help_text.contains('\u{b7}'), "{help_text:?}");
}

#[test]
fn wrapped_preview_can_scroll_all_the_way_to_its_tail() {
    let mut model = with_open_preview(model(), 1_000);
    model.preview_scroll = u16::MAX;
    model.previews.insert(
        "remote:one".into(),
        PreviewState::Ready(super::super::Preview {
            lines: vec![super::super::PreviewLine {
                role: super::super::PreviewRole::Assistant,
                text: format!("{} TAIL_MARKER", "wrapped words ".repeat(80)),
            }],
            truncated: false,
            warning: None,
        }),
    );
    let text = rendered(48, 14, model);
    assert!(text.contains("TAIL_MARKER"), "{text:?}");
}

#[test]
fn diagnostics_and_short_review_keep_controls_visible() {
    let mut diagnostics = model();
    diagnostics.mode = Mode::Diagnostics;
    let diagnostic_text = rendered(64, 14, diagnostics);
    assert!(diagnostic_text.contains("Issues"));
    assert!(diagnostic_text.contains("remote host"));

    let mut review = with_open_preview(model(), 1_000);
    review.mode = Mode::Review;
    let review_text = rendered(64, 9, review);
    assert!(review_text.contains("Hop & open"));
    assert!(review_text.contains("[Enter]"));

    let mut very_short = with_open_preview(model(), 1_000);
    very_short.mode = Mode::Review;
    let very_short_text = rendered(30, 6, very_short);
    assert!(
        very_short_text.contains("Hop & open"),
        "{very_short_text:?}"
    );
    assert!(very_short_text.contains("[Enter]"), "{very_short_text:?}");
}

#[test]
fn wrapped_overlays_scroll_to_their_last_content() {
    let mut diagnostics = model();
    diagnostics.mode = Mode::Diagnostics;
    diagnostics.overlay_scroll = u16::MAX;
    diagnostics.warnings = vec![format!("{} DIAGNOSTIC_TAIL", "wrapped warning ".repeat(40))];
    let diagnostic_text = rendered(36, 9, diagnostics);
    assert!(
        diagnostic_text.contains("DIAGNOSTIC_TAIL"),
        "{diagnostic_text:?}"
    );

    let mut help = model();
    help.mode = Mode::Help;
    help.overlay_scroll = u16::MAX;
    let help_text = rendered(36, 9, help);
    assert!(help_text.contains("copy only that text"), "{help_text:?}");
}
