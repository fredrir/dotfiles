use super::*;
use crate::cli::Agent;

fn entry(key: &str, title: &str, origin: Origin) -> SessionEntry {
    SessionEntry {
        key: key.into(),
        id: format!("id-{key}"),
        agent: Agent::Codex,
        origin,
        host: None,
        project: "dotfiles".into(),
        workspace: "/work/dotfiles".into(),
        title: title.into(),
        updated: "now".into(),
        current_project: key == "current",
        favorite: key == "favorite",
        disabled_reason: None,
        warning: None,
        sort_timestamp: match key {
            "current" => 3,
            "favorite" => 2,
            _ => 1,
        },
    }
}

fn loaded() -> Model {
    let mut model = Model::new();
    model.load(
        CatalogSnapshot {
            sessions: vec![
                entry("old", "write parser", Origin::Local),
                entry("current", "repair transfer", Origin::Local),
                entry("favorite", "remote picker", Origin::Remote),
            ],
            warnings: vec![],
        },
        true,
    );
    model.toolbar_focus = None;
    model
}

fn key(code: KeyCode) -> UiEvent {
    UiEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn mouse(kind: MouseEventKind, point: Position) -> UiEvent {
    UiEvent::Mouse {
        kind,
        column: point.x,
        row: point.y,
    }
}

#[test]
fn newest_session_is_first_and_navigation_is_bounded() {
    let mut model = loaded();
    assert_eq!(model.selected_entry().unwrap().key, "current");
    model.apply(key(KeyCode::End));
    assert_eq!(model.selected, 2);
    model.apply(key(KeyCode::Down));
    assert_eq!(model.selected, 2);
    model.apply(key(KeyCode::Home));
    assert_eq!(model.selected, 0);
}

#[test]
fn recency_beats_current_project_for_the_default_row() {
    let mut model = loaded();
    model
        .entries
        .iter_mut()
        .find(|entry| entry.key == "old")
        .unwrap()
        .sort_timestamp = 10;
    model.selection_touched = false;
    model.rebuild_filter(None);
    assert_eq!(model.selected_entry().unwrap().key, "old");
}

#[test]
fn newer_remote_result_becomes_default_until_the_user_enters_the_list() {
    let mut model = Model::new();
    let mut local = entry("current", "local latest", Origin::Local);
    local.sort_timestamp = 3;
    model.load(
        CatalogSnapshot {
            sessions: vec![local.clone()],
            warnings: vec![],
        },
        false,
    );
    assert_eq!(model.selected_entry().unwrap().key, "current");

    let mut remote = entry("favorite", "remote latest", Origin::Remote);
    remote.sort_timestamp = 10;
    model.load(
        CatalogSnapshot {
            sessions: vec![remote.clone(), local.clone()],
            warnings: vec![],
        },
        true,
    );
    assert_eq!(model.selected_entry().unwrap().key, "favorite");

    let mut navigated = Model::new();
    navigated.load(
        CatalogSnapshot {
            sessions: vec![local.clone()],
            warnings: vec![],
        },
        false,
    );
    navigated.apply(key(KeyCode::Down));
    navigated.load(
        CatalogSnapshot {
            sessions: vec![remote, local],
            warnings: vec![],
        },
        true,
    );
    assert_eq!(navigated.selected_entry().unwrap().key, "current");
}

#[test]
fn launch_focuses_the_first_session_and_toolbar_supports_search_and_filters() {
    assert_eq!(
        Model::new().toolbar_focus,
        None,
        "launch focus belongs to the first session"
    );
    let mut model = loaded();
    assert_eq!(model.selected_entry().unwrap().key, "current");
    assert_eq!(model.preview_entry().unwrap().key, "current");
    model.apply(key(KeyCode::Char('/')));

    for character in "rmte pckr".chars() {
        model.apply(key(KeyCode::Char(character)));
    }
    assert_eq!(model.query, "rmte pckr");
    assert_eq!(model.selected_entry().unwrap().key, "favorite");
    assert_eq!(model.toolbar_focus, Some(ToolbarItem::Search));

    model.query.clear();
    model.rebuild_filter(None);
    model.apply(key(KeyCode::Right));
    assert_eq!(model.toolbar_focus, Some(ToolbarItem::Origin));
    model.apply(key(KeyCode::Enter));
    assert_eq!(model.origin_filter, OriginFilter::Local);
    model.apply(key(KeyCode::Right));
    assert_eq!(model.toolbar_focus, Some(ToolbarItem::Agent));
    model.apply(key(KeyCode::Left));
    assert_eq!(model.toolbar_focus, Some(ToolbarItem::Origin));

    model.apply(key(KeyCode::Down));
    assert_eq!(model.toolbar_focus, None);
    assert_eq!(model.selected, 0);
    assert_eq!(model.pane, Pane::List);
}

#[test]
fn moving_up_from_the_first_session_focuses_search() {
    let mut model = loaded();

    assert_eq!(model.apply(key(KeyCode::Up)), Effect::None);
    assert_eq!(model.toolbar_focus, Some(ToolbarItem::Search));

    model.apply(key(KeyCode::Down));
    model.apply(key(KeyCode::Down));
    assert_eq!(model.selected, 1);
    model.apply(key(KeyCode::Up));
    assert_eq!(model.selected, 0);
    assert_eq!(model.toolbar_focus, None);
    model.apply(key(KeyCode::Char('k')));
    assert_eq!(model.toolbar_focus, Some(ToolbarItem::Search));
}

#[test]
fn enter_from_search_returns_to_the_already_previewed_first_session() {
    let mut model = loaded();
    model.apply(key(KeyCode::Char('/')));
    assert_eq!(model.apply(key(KeyCode::Enter)), Effect::None);
    assert_eq!(model.toolbar_focus, None);
    assert_eq!(model.preview_entry().unwrap().key, "current");

    assert_eq!(model.apply(key(KeyCode::Enter)), Effect::None);
    assert_eq!(model.pane, Pane::Preview);
}

#[test]
fn horizontal_arrows_only_move_focus_and_enter_selects_the_highlighted_session() {
    let mut model = loaded();
    model.apply(key(KeyCode::Down));
    assert_eq!(model.selected_entry().unwrap().key, "favorite");
    assert_eq!(model.preview_entry().unwrap().key, "current");

    assert_eq!(model.apply(key(KeyCode::Right)), Effect::None);
    assert_eq!(model.pane, Pane::Preview);
    assert_eq!(model.preview_entry().unwrap().key, "current");

    assert_eq!(model.apply(key(KeyCode::Right)), Effect::None);
    assert_eq!(model.pane, Pane::List);
    assert_eq!(model.preview_entry().unwrap().key, "current");

    assert_eq!(
        model.apply(key(KeyCode::Enter)),
        Effect::LoadPreview("favorite".into())
    );
    assert_eq!(model.pane, Pane::Preview);
    assert_eq!(model.preview_entry().unwrap().key, "favorite");

    assert_eq!(model.apply(key(KeyCode::Left)), Effect::None);
    assert_eq!(model.pane, Pane::List);
    assert_eq!(model.preview_entry().unwrap().key, "favorite");
}

#[test]
fn clicking_a_card_opens_it_and_preview_drag_stays_inside_the_pane() {
    let mut model = loaded();
    model.area = Rect::new(0, 0, 120, 28);
    model.set_reduced_motion(true);
    let list = super::super::view::layout(model.area).list;
    let card = Position::new(list.x.saturating_add(2), list.y.saturating_add(5));
    assert_eq!(
        model.apply(mouse(MouseEventKind::Down(MouseButton::Left), card)),
        Effect::LoadPreview("favorite".into())
    );
    assert_eq!(model.pane, Pane::Preview);
    assert_eq!(model.preview_entry().unwrap().key, "favorite");

    let preview = super::super::view::preview_text_area(&model);
    let start = Position::new(preview.x.saturating_add(1), preview.y);
    let outside = Position::new(list.x, preview.y.saturating_add(1));
    model.apply(mouse(MouseEventKind::Down(MouseButton::Left), start));
    model.apply(mouse(MouseEventKind::Drag(MouseButton::Left), outside));
    model.apply(mouse(MouseEventKind::Up(MouseButton::Left), outside));

    let selection = model.text_selection.expect("drag remains selected");
    assert!(selection.dragged());
    assert!(preview.contains(selection.anchor));
    assert!(preview.contains(selection.head));
    assert!(!selection.contains(Position::new(list.x, list.y), preview));
}

#[test]
fn fuzzy_search_is_token_aware_and_deterministic() {
    let mut model = loaded();
    model.apply(key(KeyCode::Char('/')));
    for character in "rmte pckr".chars() {
        model.apply(key(KeyCode::Char(character)));
    }
    assert_eq!(model.filtered.len(), 1);
    assert_eq!(model.selected_entry().unwrap().key, "favorite");
}

#[test]
fn filters_cover_remote_and_favorites() {
    let mut model = loaded();
    model.apply(key(KeyCode::Char('1')));
    assert!(
        model
            .filtered
            .iter()
            .all(|index| model.entries[*index].origin == Origin::Local)
    );
    model.apply(key(KeyCode::Char('1')));
    assert_eq!(model.filtered.len(), 1);
    assert_eq!(model.selected_entry().unwrap().origin, Origin::Remote);
    model.apply(key(KeyCode::Char('3')));
    model.apply(key(KeyCode::Char('3')));
    assert_eq!(model.selected_entry().unwrap().key, "favorite");
}

#[test]
fn initial_preview_is_present_immediately() {
    let model = loaded();
    assert_eq!(model.preview_entry().unwrap().key, "current");
    assert_eq!(model.preview_progress(), 1.0);
    assert_eq!(model.preview_transition, PREVIEW_TRANSITION_MAX);
    assert!(matches!(
        model.selected_preview(),
        Some(PreviewState::Loading)
    ));
    assert!(!model.is_animating());
}

#[test]
fn slash_focuses_search_from_every_mode_without_entering_a_slash() {
    let mut model = loaded();
    model.apply(key(KeyCode::Enter));
    for mode in [Mode::Browse, Mode::Help, Mode::Diagnostics, Mode::Review] {
        model.mode = mode;
        model.pane = Pane::Preview;
        assert_eq!(model.apply(key(KeyCode::Char('/'))), Effect::None);
        assert_eq!(model.toolbar_focus, Some(ToolbarItem::Search));
        assert_eq!(model.pane, Pane::List);
        assert!(model.query.is_empty());
    }
    assert_eq!(model.mode, Mode::Browse);
}

#[test]
fn escape_cancels_from_preview_without_clearing_it() {
    let mut model = loaded();
    model.apply(key(KeyCode::Right));
    assert_eq!(model.pane, Pane::Preview);
    assert_eq!(model.apply(key(KeyCode::Esc)), Effect::Cancel);
    assert_eq!(model.pane, Pane::Preview);
    assert_eq!(model.preview_entry().unwrap().key, "current");
}

#[test]
fn removed_session_immediately_falls_back_to_an_available_preview() {
    let mut model = loaded();
    model.preview_loaded("current", Ok(Preview::default()));
    model.selection_touched = true;

    assert_eq!(
        model.load(
            CatalogSnapshot {
                sessions: vec![entry("old", "write parser", Origin::Local)],
                warnings: Vec::new(),
            },
            true,
        ),
        Effect::LoadPreview("old".into())
    );
    assert_eq!(model.pane, Pane::List);
    assert_eq!(model.preview_entry().unwrap().key, "old");
    assert!(matches!(
        model.selected_preview(),
        Some(PreviewState::Loading)
    ));
    assert!(!model.previews.contains_key("current"));
}

#[test]
fn preview_density_cycles_and_metadata_avoids_transcript_reads() {
    let mut model = loaded();
    model.apply(key(KeyCode::Right));
    model.previews.clear();
    assert_eq!(
        model.apply(key(KeyCode::Char('v'))),
        Effect::LoadPreview("current".into())
    );
    assert_eq!(model.preview_density, PreviewDensity::Compact);
    model.previews.clear();
    assert_eq!(model.apply(key(KeyCode::Char('v'))), Effect::None);
    assert_eq!(model.preview_density, PreviewDensity::Metadata);
    assert_eq!(
        model.apply(key(KeyCode::Char('v'))),
        Effect::LoadPreview("current".into())
    );
    assert_eq!(model.preview_density, PreviewDensity::Conversation);
}

#[test]
fn view_settings_round_trip_through_the_model() {
    let mut model = loaded();
    let view = PickerView {
        origin: OriginFilter::Remote,
        agent: AgentFilter::Claude,
        scope: ScopeFilter::Favorites,
        preview: PreviewDensity::Compact,
    };
    model.set_view(view);
    assert_eq!(model.view(), view);
}

#[test]
fn enter_locks_preview_and_space_opens_review() {
    let mut model = loaded();
    model.apply(key(KeyCode::Down));
    assert_eq!(
        model.apply(key(KeyCode::Enter)),
        Effect::LoadPreview("favorite".into())
    );
    assert_eq!(model.pane, Pane::Preview);
    assert_eq!(model.preview_entry().unwrap().key, "favorite");
    assert_eq!(model.mode, Mode::Browse);
    assert_eq!(model.apply(key(KeyCode::Enter)), Effect::None);
    assert_eq!(model.mode, Mode::Browse);
    assert_eq!(model.apply(key(KeyCode::Char(' '))), Effect::None);
    assert_eq!(model.mode, Mode::Review);
    assert_eq!(
        model.apply(key(KeyCode::Char('d'))),
        Effect::Pick(PickerAction::DryRun)
    );
}

#[test]
fn disabled_rows_can_be_previewed_but_cannot_open_review() {
    let mut model = loaded();
    let index = model.filtered[model.selected];
    model.entries[index].disabled_reason = Some("missing transcript".into());
    assert_eq!(model.apply(key(KeyCode::Enter)), Effect::None);
    assert_eq!(model.pane, Pane::Preview);
    assert_eq!(model.apply(key(KeyCode::Char(' '))), Effect::None);
    assert_eq!(model.mode, Mode::Browse);
    assert!(
        model
            .status
            .as_deref()
            .unwrap()
            .contains("missing transcript")
    );
}

#[test]
fn review_honors_the_cli_seeded_action() {
    let mut model = loaded();
    model.set_initial_action(PickerAction::CopyOnly);
    model.apply(key(KeyCode::Enter));
    model.apply(key(KeyCode::Char(' ')));
    assert_eq!(model.review_action, 1);
    assert_eq!(
        model.apply(key(KeyCode::Enter)),
        Effect::Pick(PickerAction::CopyOnly)
    );
}

#[test]
fn previews_are_requested_once_and_cached() {
    let mut model = loaded();
    let session_key = model.selected_entry().unwrap().key.clone();
    assert!(matches!(
        model.previews.get(&session_key),
        Some(PreviewState::Loading)
    ));
    model.preview_loaded(&session_key, Ok(Preview::default()));
    assert_eq!(model.apply(key(KeyCode::Right)), Effect::None);
    assert_eq!(model.apply(key(KeyCode::Left)), Effect::None);
    assert_eq!(model.pane, Pane::List);
    assert_eq!(model.apply(key(KeyCode::Enter)), Effect::None);
    assert!(matches!(
        model.previews.get(&session_key),
        Some(PreviewState::Ready(_))
    ));
}

#[test]
fn refresh_keeps_remote_selection_and_invalidates_cached_previews() {
    let mut model = loaded();
    model.rebuild_filter(Some("favorite"));
    model.selection_touched = true;
    assert_eq!(
        model.apply(key(KeyCode::Enter)),
        Effect::LoadPreview("favorite".into())
    );
    model.preview_loaded("favorite", Ok(Preview::default()));
    model.focus_session_list();
    assert_eq!(model.selected_entry().unwrap().key, "favorite");
    model
        .previews
        .insert("old".into(), PreviewState::Ready(Preview::default()));

    model.begin_refresh();
    assert!(model.previews.is_empty());
    let effect = model.load(
        CatalogSnapshot {
            sessions: vec![entry("old", "refreshed local", Origin::Local)],
            warnings: vec![],
        },
        false,
    );

    assert_eq!(model.selected_entry().unwrap().key, "favorite");
    assert_eq!(effect, Effect::LoadPreview("favorite".into()));
    assert!(matches!(
        model.previews.get("favorite"),
        Some(PreviewState::Loading)
    ));
}

#[test]
fn complete_remote_merge_preserves_a_live_favorite_change() {
    let mut model = loaded();
    model.rebuild_filter(Some("favorite"));
    model.apply(key(KeyCode::Enter));
    assert_eq!(
        model.apply(key(KeyCode::Char('f'))),
        Effect::SetFavorite {
            key: "favorite".into(),
            favorite: false,
        }
    );

    let mut stale = entry("favorite", "remote picker", Origin::Remote);
    stale.favorite = true;
    model.load(
        CatalogSnapshot {
            sessions: vec![stale],
            warnings: vec![],
        },
        true,
    );

    assert!(!model.entries[0].favorite);
}

#[test]
fn skipped_background_previews_can_be_requested_again() {
    let mut model = loaded();
    let session_key = model.selected_entry().unwrap().key.clone();
    assert_eq!(
        model.preview_skipped(&session_key),
        Effect::LoadPreview(session_key.clone())
    );
    assert!(matches!(
        model.previews.get(&session_key),
        Some(PreviewState::Loading)
    ));
}

#[test]
fn control_navigation_and_cancellation_are_supported() {
    let mut model = loaded();
    let down = UiEvent::Key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
    model.apply(down);
    assert_eq!(model.selected, 1);
    let cancel = UiEvent::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert_eq!(model.apply(cancel), Effect::Cancel);
}

#[test]
fn review_cannot_apply_when_the_action_cannot_be_rendered() {
    let mut model = loaded();
    model.area = Rect::new(0, 0, 12, 4);
    model.apply(key(KeyCode::Enter));
    model.apply(key(KeyCode::Char(' ')));
    assert_eq!(model.mode, Mode::Review);
    assert_eq!(model.apply(key(KeyCode::Enter)), Effect::None);
    assert!(model.status.as_deref().unwrap().contains("Resize"));
}

#[test]
fn preview_only_keys_require_an_explicit_enter() {
    let mut model = loaded();
    let initial_density = model.preview_density;
    let initial_favorite = model.selected_entry().unwrap().favorite;

    for code in [
        KeyCode::Char('f'),
        KeyCode::Char('y'),
        KeyCode::Char('v'),
        KeyCode::Char(' '),
    ] {
        assert_eq!(model.apply(key(code)), Effect::None);
    }
    assert_eq!(model.pane, Pane::List);
    assert_eq!(model.mode, Mode::Browse);
    assert_eq!(model.preview_density, initial_density);
    assert_eq!(model.selected_entry().unwrap().favorite, initial_favorite);

    model.apply(key(KeyCode::Enter));
    assert_eq!(model.apply(key(KeyCode::Char('r'))), Effect::None);
    assert!(matches!(
        model.apply(key(KeyCode::Char('y'))),
        Effect::CopySessionDescription(_)
    ));
}

#[test]
fn copy_description_is_complete_and_terminal_safe() {
    let mut model = loaded();
    let index = model.filtered[model.selected];
    model.entries[index].title = "repair\ntransfer\u{1b}".into();
    model.entries[index].host = Some("macie\tdev".into());
    model.entries[index].updated = "8m\rago".into();
    model.apply(key(KeyCode::Enter));

    let Effect::CopySessionDescription(description) = model.apply(key(KeyCode::Char('y'))) else {
        panic!("expected complete session description");
    };
    for expected in [
        "Summary: repair transfer�",
        "Agent: codex",
        "Updated: 8m ago",
        "Project: dotfiles",
        "Origin: local",
        "Host: macie dev",
        "Workspace: /work/dotfiles",
        "Favorite: no",
        "Session ID: id-current",
    ] {
        assert!(description.contains(expected), "missing {expected:?}");
    }
    assert!(!description.contains('\u{1b}'));
}

#[test]
fn search_changes_the_highlight_without_changing_the_displayed_preview() {
    let mut model = loaded();
    let favorite = model
        .entries
        .iter_mut()
        .find(|entry| entry.favorite)
        .unwrap();
    favorite.updated = "8m ago".into();

    model.apply(key(KeyCode::Char('/')));
    for character in "8m ago".chars() {
        assert_eq!(model.apply(key(KeyCode::Char(character))), Effect::None);
    }
    assert_eq!(model.selected_entry().unwrap().key, "favorite");
    assert_eq!(model.preview_entry().unwrap().key, "current");
    assert_eq!(model.previews.len(), 1);
    assert!(model.previews.contains_key("current"));

    model.apply(UiEvent::Key(KeyEvent::new(
        KeyCode::Char('u'),
        KeyModifiers::CONTROL,
    )));
    for character in "starred".chars() {
        model.apply(key(KeyCode::Char(character)));
    }
    assert_eq!(model.selected_entry().unwrap().key, "favorite");
    assert_eq!(model.preview_entry().unwrap().key, "current");
    assert_eq!(model.previews.len(), 1);
}

#[test]
fn unfavoriting_a_locked_favorites_row_returns_to_the_list() {
    let mut model = loaded();
    model.scope_filter = ScopeFilter::Favorites;
    model.rebuild_filter(Some("favorite"));
    model.apply(key(KeyCode::Enter));
    assert_eq!(model.pane, Pane::Preview);

    assert_eq!(
        model.apply(key(KeyCode::Char('f'))),
        Effect::SetFavorite {
            key: "favorite".into(),
            favorite: false,
        }
    );
    assert_eq!(model.pane, Pane::List);
    assert!(model.filtered.is_empty());
    assert!(model.status.as_deref().unwrap().contains("active filters"));
}

#[test]
fn escape_returns_from_review_then_cancels_without_closing_preview() {
    let mut model = loaded();
    model.apply(key(KeyCode::Enter));
    model.apply(key(KeyCode::Char(' ')));

    model.apply(key(KeyCode::Esc));
    assert_eq!(model.mode, Mode::Browse);
    assert_eq!(model.pane, Pane::Preview);
    assert_eq!(model.apply(key(KeyCode::Esc)), Effect::Cancel);
    assert_eq!(model.pane, Pane::Preview);
    assert!(model.preview_entry().is_some());
}
