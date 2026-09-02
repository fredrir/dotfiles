use std::collections::HashMap;

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::layout::{Position, Rect};

use super::{
    AgentFilter, CatalogSnapshot, Origin, OriginFilter, PickerAction, PickerView, Preview,
    PreviewDensity, ScopeFilter, SessionEntry, clean,
};

pub(crate) const PREVIEW_TRANSITION_MAX: u16 = 1_000;
const PREVIEW_TRANSITION_STEP: u16 = 100;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Pane {
    List,
    Preview,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ToolbarItem {
    #[default]
    Search,
    Origin,
    Agent,
    Scope,
}

impl ToolbarItem {
    fn previous(self) -> Self {
        match self {
            Self::Search | Self::Origin => Self::Search,
            Self::Agent => Self::Origin,
            Self::Scope => Self::Agent,
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Search => Self::Origin,
            Self::Origin => Self::Agent,
            Self::Agent | Self::Scope => Self::Scope,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PreviewState {
    Loading,
    Ready(Preview),
    Error(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TextSelection {
    pub(crate) anchor: Position,
    pub(crate) head: Position,
    dragged: bool,
}

impl TextSelection {
    fn new(anchor: Position) -> Self {
        Self {
            anchor,
            head: anchor,
            dragged: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn between(anchor: Position, head: Position) -> Self {
        Self {
            anchor,
            head,
            dragged: anchor != head,
        }
    }

    pub(crate) fn dragged(self) -> bool {
        self.dragged
    }

    pub(crate) fn ordered(self) -> (Position, Position) {
        if (self.anchor.y, self.anchor.x) <= (self.head.y, self.head.x) {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    pub(crate) fn contains(self, point: Position, area: Rect) -> bool {
        if !area.contains(point) {
            return false;
        }
        let (start, end) = self.ordered();
        if point.y < start.y || point.y > end.y {
            return false;
        }
        if start.y == end.y {
            point.x >= start.x && point.x <= end.x
        } else if point.y == start.y {
            point.x >= start.x
        } else if point.y == end.y {
            point.x <= end.x
        } else {
            true
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Mode {
    Browse,
    Help,
    Diagnostics,
    Review,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum UiEvent {
    Key(KeyEvent),
    Mouse {
        kind: MouseEventKind,
        column: u16,
        row: u16,
    },
    Resize(u16, u16),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Effect {
    None,
    Cancel,
    Pick(PickerAction),
    Refresh,
    LoadPreview(String),
    SetFavorite { key: String, favorite: bool },
    CopySessionDescription(String),
}

#[derive(Clone, Debug)]
pub(crate) struct Model {
    pub(crate) entries: Vec<SessionEntry>,
    pub(crate) warnings: Vec<String>,
    pub(crate) loading: bool,
    pub(crate) fatal_error: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) query: String,
    pub(crate) origin_filter: OriginFilter,
    pub(crate) agent_filter: AgentFilter,
    pub(crate) scope_filter: ScopeFilter,
    pub(crate) preview_density: PreviewDensity,
    pub(crate) filtered: Vec<usize>,
    pub(crate) selected: usize,
    pub(crate) list_offset: usize,
    pub(crate) preview_scroll: u16,
    pub(crate) overlay_scroll: u16,
    pub(crate) pane: Pane,
    pub(crate) toolbar_focus: Option<ToolbarItem>,
    /// Linear, normalized drawer progress. The renderer owns visual easing.
    pub(crate) preview_transition: u16,
    preview_target_open: bool,
    pub(crate) mode: Mode,
    pub(crate) review_action: usize,
    initial_review_action: usize,
    pub(crate) previews: HashMap<String, PreviewState>,
    pub(crate) text_selection: Option<TextSelection>,
    favorite_overrides: HashMap<String, bool>,
    preview_session: Option<SessionEntry>,
    selection_touched: bool,
    reduced_motion: bool,
    pub(crate) area: Rect,
}

impl Default for Model {
    fn default() -> Self {
        Self::new()
    }
}

impl Model {
    pub(crate) fn new() -> Self {
        Self {
            entries: Vec::new(),
            warnings: Vec::new(),
            loading: true,
            fatal_error: None,
            status: None,
            query: String::new(),
            origin_filter: OriginFilter::All,
            agent_filter: AgentFilter::All,
            scope_filter: ScopeFilter::All,
            preview_density: PreviewDensity::Conversation,
            filtered: Vec::new(),
            selected: 0,
            list_offset: 0,
            preview_scroll: 0,
            overlay_scroll: 0,
            pane: Pane::List,
            toolbar_focus: None,
            preview_transition: 0,
            preview_target_open: false,
            mode: Mode::Browse,
            review_action: 0,
            initial_review_action: 0,
            previews: HashMap::new(),
            text_selection: None,
            favorite_overrides: HashMap::new(),
            preview_session: None,
            selection_touched: false,
            reduced_motion: false,
            area: Rect::new(0, 0, 80, 24),
        }
    }

    pub(crate) fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.reduced_motion = reduced_motion;
        if reduced_motion {
            self.preview_transition = if self.preview_target_open {
                PREVIEW_TRANSITION_MAX
            } else {
                0
            };
            if !self.preview_target_open {
                self.forget_preview_session();
            }
            self.keep_selection_visible();
        }
    }

    /// Normalized preview reveal in the inclusive range `0.0..=1.0`.
    pub(crate) fn preview_progress(&self) -> f32 {
        f32::from(self.preview_transition.min(PREVIEW_TRANSITION_MAX))
            / f32::from(PREVIEW_TRANSITION_MAX)
    }

    /// Preview actions are available while the displayed preview has focus.
    pub(crate) fn preview_actions_enabled(&self) -> bool {
        self.pane == Pane::Preview && self.preview_session.is_some()
    }

    pub(crate) fn preview_is_opening(&self) -> bool {
        self.preview_target_open && self.preview_transition < PREVIEW_TRANSITION_MAX
    }

    pub(crate) fn preview_is_closing(&self) -> bool {
        !self.preview_target_open && self.preview_transition > 0
    }

    pub(crate) fn is_animating(&self) -> bool {
        !self.reduced_motion && (self.preview_is_opening() || self.preview_is_closing())
    }

    /// Advance one deterministic animation frame. Returns whether geometry changed.
    pub(crate) fn tick_animation(&mut self) -> bool {
        if !self.is_animating() {
            return false;
        }
        let previous = self.preview_transition;
        if self.preview_target_open {
            self.preview_transition = self
                .preview_transition
                .saturating_add(PREVIEW_TRANSITION_STEP)
                .min(PREVIEW_TRANSITION_MAX);
        } else {
            self.preview_transition = self
                .preview_transition
                .saturating_sub(PREVIEW_TRANSITION_STEP);
            if self.preview_transition == 0 {
                self.forget_preview_session();
            }
        }
        self.keep_selection_visible();
        self.preview_transition != previous
    }

    pub(crate) fn set_initial_action(&mut self, action: PickerAction) {
        let selected = PickerAction::ALL
            .iter()
            .position(|candidate| *candidate == action)
            .unwrap_or(0);
        self.initial_review_action = selected;
        self.review_action = selected;
    }

    pub(crate) fn set_view(&mut self, view: PickerView) {
        self.origin_filter = view.origin;
        self.agent_filter = view.agent;
        self.scope_filter = view.scope;
        self.preview_density = view.preview;
        self.rebuild_filter(None);
    }

    pub(crate) fn view(&self) -> PickerView {
        PickerView {
            origin: self.origin_filter,
            agent: self.agent_filter,
            scope: self.scope_filter,
            preview: self.preview_density,
        }
    }

    pub(crate) fn begin_refresh(&mut self) {
        self.loading = true;
        self.fatal_error = None;
        self.status = Some("Refreshing local and remote sessions…".into());
        // A refresh is an explicit request to see current transcript state as
        // well as current catalog metadata.  Keeping cached previews here made
        // `r` appear to work while the selected transcript stayed stale.
        self.previews.clear();
        self.favorite_overrides.clear();
    }

    pub(crate) fn load(&mut self, mut snapshot: CatalogSnapshot, complete: bool) -> Effect {
        let preview_key = self.preview_session.as_ref().map(|entry| entry.key.clone());
        let selected_key = self
            .selection_touched
            .then(|| self.selected_entry().map(|entry| entry.key.clone()))
            .flatten();
        // Keep the previous remote half visible while its replacement is in
        // flight.  Besides avoiding a distracting list collapse, this keeps a
        // selected remote session stable across the local-first refresh.
        if !complete {
            snapshot = snapshot.merge(CatalogSnapshot {
                sessions: self
                    .entries
                    .iter()
                    .filter(|entry| entry.origin == Origin::Remote)
                    .cloned()
                    .collect(),
                warnings: Vec::new(),
            });
        }
        // Discovery runs on a snapshot of the favorite store.  A favorite can
        // be changed while remote discovery is pending, so the live model is
        // authoritative for rows it already knows about.
        for entry in &mut snapshot.sessions {
            if let Some(favorite) = self.favorite_overrides.get(&entry.key) {
                entry.favorite = *favorite;
            }
        }
        if complete {
            self.favorite_overrides.clear();
        }
        self.entries = snapshot.sessions;
        self.warnings = snapshot
            .warnings
            .into_iter()
            .map(|warning| clean(&warning))
            .collect();
        self.loading = !complete;
        self.fatal_error = None;
        self.status = (!complete).then(|| "Local sessions ready; fetching remote sessions…".into());
        self.previews
            .retain(|key, _| self.entries.iter().any(|entry| entry.key == *key));
        self.rebuild_filter(selected_key.as_deref());
        let preview_was_removed = preview_key
            .as_ref()
            .is_some_and(|key| self.entries.iter().all(|entry| entry.key != *key));
        let displayed = if !self.selection_touched || preview_was_removed {
            self.selected_entry()
                .cloned()
                .or_else(|| self.entries.first().cloned())
        } else {
            preview_key
                .as_deref()
                .and_then(|key| self.entries.iter().find(|entry| entry.key == key))
                .cloned()
                .or_else(|| self.selected_entry().cloned())
                .or_else(|| self.entries.first().cloned())
        };
        self.display_preview(displayed);
        if preview_was_removed {
            self.status = Some("The selected session is no longer available".into());
        }
        self.request_preview()
    }

    pub(crate) fn load_failed(&mut self, error: String, complete: bool) {
        self.loading = false;
        let error = clean(&error);
        if complete && !self.entries.is_empty() {
            self.warnings.push(error);
            self.warnings.sort();
            self.warnings.dedup();
            self.status = None;
        } else {
            self.fatal_error = Some(error);
            self.status = None;
        }
    }

    pub(crate) fn preview_loaded(&mut self, key: &str, result: Result<Preview, String>) {
        let selected = self.preview_entry().is_some_and(|entry| entry.key == key);
        let state = match result {
            Ok(mut preview) => {
                for line in &mut preview.lines {
                    line.text = clean(&line.text);
                }
                PreviewState::Ready(preview)
            }
            Err(error) => PreviewState::Error(clean(&error)),
        };
        self.previews.insert(key.to_string(), state);
        if selected {
            self.preview_scroll = 0;
            self.text_selection = None;
        }
    }

    pub(crate) fn preview_skipped(&mut self, key: &str) -> Effect {
        if matches!(self.previews.get(key), Some(PreviewState::Loading)) {
            self.previews.remove(key);
        }
        if self.preview_entry().is_some_and(|entry| entry.key == key) {
            self.request_preview()
        } else {
            Effect::None
        }
    }

    pub(crate) fn favorite_failed(&mut self, key: &str, previous: bool, error: String) {
        self.favorite_overrides.remove(key);
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.key == key) {
            entry.favorite = previous;
        }
        self.status = Some(format!("Could not save favorite: {}", clean(&error)));
        self.rebuild_filter(Some(key));
    }

    pub(crate) fn copied(&mut self) {
        self.status = Some("Session description copied with dclip".into());
    }

    pub(crate) fn copy_failed(&mut self, error: String) {
        self.status = Some(format!(
            "Could not copy session description: {}",
            clean(&error)
        ));
    }

    pub(crate) fn selected_text_copied(&mut self) {
        self.status = Some("Selected preview text copied".into());
    }

    pub(crate) fn selected_text_copy_failed(&mut self, error: String) {
        self.status = Some(format!("Could not copy selected text: {}", clean(&error)));
    }

    pub(crate) fn selected_entry(&self) -> Option<&SessionEntry> {
        self.filtered
            .get(self.selected)
            .and_then(|index| self.entries.get(*index))
    }

    pub(crate) fn selected_preview(&self) -> Option<&PreviewState> {
        self.preview_entry()
            .and_then(|entry| self.previews.get(&entry.key))
    }

    /// The session currently displayed in the persistent preview pane.
    pub(crate) fn preview_entry(&self) -> Option<&SessionEntry> {
        self.preview_session.as_ref()
    }

    pub(crate) fn page_size(&self) -> usize {
        super::view::list_page_size(self.area)
    }

    pub(crate) fn apply(&mut self, event: UiEvent) -> Effect {
        match event {
            UiEvent::Resize(width, height) => {
                self.area = Rect::new(0, 0, width, height);
                self.text_selection = None;
                self.keep_selection_visible();
                Effect::None
            }
            UiEvent::Key(key) if key.kind != KeyEventKind::Press => Effect::None,
            UiEvent::Key(key) => {
                self.text_selection = None;
                self.key(key)
            }
            UiEvent::Mouse { kind, column, row } => self.mouse(kind, Position::new(column, row)),
        }
    }

    fn key(&mut self, key: KeyEvent) -> Effect {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return match key.code {
                KeyCode::Char('c') => Effect::Cancel,
                KeyCode::Char('n') if self.pane == Pane::List && self.mode == Mode::Browse => {
                    if self.toolbar_focus.is_some() {
                        self.focus_list()
                    } else {
                        self.move_selection(1)
                    }
                }
                KeyCode::Char('p') if self.pane == Pane::List && self.mode == Mode::Browse => {
                    if self.toolbar_focus.is_some() || self.selected == 0 {
                        self.toolbar_focus = Some(ToolbarItem::Search);
                        Effect::None
                    } else {
                        self.move_selection(-1)
                    }
                }
                KeyCode::Char('u')
                    if self.pane == Pane::List
                        && self.mode == Mode::Browse
                        && self.toolbar_focus.is_some() =>
                {
                    self.query.clear();
                    self.selection_touched = true;
                    self.rebuild_filter(None);
                    Effect::None
                }
                _ => Effect::None,
            };
        }

        if key.code == KeyCode::Char('/') {
            return self.focus_search();
        }

        match self.mode {
            Mode::Help | Mode::Diagnostics => self.overlay_key(key.code),
            Mode::Review => self.review_key(key.code),
            Mode::Browse => self.browse_key(key.code),
        }
    }

    fn overlay_key(&mut self, code: KeyCode) -> Effect {
        match code {
            KeyCode::Char('?')
            | KeyCode::Char('!')
            | KeyCode::Char('w')
            | KeyCode::Esc
            | KeyCode::Char('q')
            | KeyCode::Enter => {
                self.mode = Mode::Browse;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.overlay_scroll = self.overlay_scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.overlay_scroll = self.overlay_scroll.saturating_add(1);
            }
            KeyCode::PageUp => {
                self.overlay_scroll = self.overlay_scroll.saturating_sub(8);
            }
            KeyCode::PageDown => {
                self.overlay_scroll = self.overlay_scroll.saturating_add(8);
            }
            KeyCode::Home => self.overlay_scroll = 0,
            KeyCode::End => self.overlay_scroll = u16::MAX,
            _ => {}
        }
        Effect::None
    }

    fn review_key(&mut self, code: KeyCode) -> Effect {
        if matches!(code, KeyCode::Enter | KeyCode::Char('o' | 'c' | 'd'))
            && !super::view::review_can_apply(self.area)
        {
            self.status = Some("Resize the terminal to apply this action".into());
            return Effect::None;
        }
        match code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.mode = Mode::Browse;
                self.pane = Pane::Preview;
                self.preview_target_open = true;
                if self.reduced_motion {
                    self.preview_transition = PREVIEW_TRANSITION_MAX;
                }
                Effect::None
            }
            KeyCode::Left | KeyCode::Up | KeyCode::BackTab | KeyCode::Char('h') => {
                self.review_action =
                    (self.review_action + PickerAction::ALL.len() - 1) % PickerAction::ALL.len();
                Effect::None
            }
            KeyCode::Right | KeyCode::Down | KeyCode::Tab | KeyCode::Char('l') => {
                self.review_action = (self.review_action + 1) % PickerAction::ALL.len();
                Effect::None
            }
            KeyCode::Char('o') => Effect::Pick(PickerAction::HopAndOpen),
            KeyCode::Char('c') => Effect::Pick(PickerAction::CopyOnly),
            KeyCode::Char('d') => Effect::Pick(PickerAction::DryRun),
            KeyCode::Enter => Effect::Pick(PickerAction::ALL[self.review_action]),
            _ => Effect::None,
        }
    }

    fn browse_key(&mut self, code: KeyCode) -> Effect {
        match self.pane {
            Pane::List => match self.toolbar_focus {
                Some(item) => self.toolbar_key(item, code),
                None => self.list_key(code),
            },
            Pane::Preview => self.preview_key(code),
        }
    }

    fn mouse(&mut self, kind: MouseEventKind, point: Position) -> Effect {
        if self.mode != Mode::Browse {
            if matches!(self.mode, Mode::Help | Mode::Diagnostics) {
                match kind {
                    MouseEventKind::ScrollUp => {
                        self.overlay_scroll = self.overlay_scroll.saturating_sub(3);
                    }
                    MouseEventKind::ScrollDown => {
                        self.overlay_scroll = self.overlay_scroll.saturating_add(3);
                    }
                    _ => {}
                }
            }
            return Effect::None;
        }
        match kind {
            MouseEventKind::Down(MouseButton::Left) => self.mouse_down(point),
            MouseEventKind::Drag(MouseButton::Left) => {
                self.update_text_selection(point);
                Effect::None
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.update_text_selection(point);
                if self
                    .text_selection
                    .is_some_and(|selection| !selection.dragged())
                {
                    self.text_selection = None;
                }
                Effect::None
            }
            MouseEventKind::ScrollUp => self.mouse_scroll(point, -1),
            MouseEventKind::ScrollDown => self.mouse_scroll(point, 1),
            _ => Effect::None,
        }
    }

    fn mouse_down(&mut self, point: Position) -> Effect {
        self.text_selection = None;
        match super::view::hit_test(self, point) {
            super::view::HitTarget::Toolbar(item) => {
                if self.pane == Pane::Preview {
                    self.focus_session_list();
                }
                self.toolbar_focus = Some(item);
                if item == ToolbarItem::Search {
                    Effect::None
                } else {
                    self.cycle_toolbar_filter(item)
                }
            }
            super::view::HitTarget::Session(position) => {
                self.pane = Pane::List;
                self.toolbar_focus = None;
                self.select_absolute(position);
                self.open_preview()
            }
            super::view::HitTarget::PreviewText => {
                self.pane = Pane::Preview;
                self.toolbar_focus = None;
                if let Some(point) = clamp_point(super::view::preview_text_area(self), point) {
                    self.text_selection = Some(TextSelection::new(point));
                }
                Effect::None
            }
            super::view::HitTarget::Issues if !self.warnings.is_empty() => {
                self.mode = Mode::Diagnostics;
                self.overlay_scroll = 0;
                Effect::None
            }
            super::view::HitTarget::List => {
                if self.pane == Pane::Preview {
                    self.focus_session_list();
                }
                self.toolbar_focus = None;
                Effect::None
            }
            super::view::HitTarget::Preview => {
                if self.preview_session.is_some() {
                    self.pane = Pane::Preview;
                    self.toolbar_focus = None;
                }
                Effect::None
            }
            super::view::HitTarget::Issues | super::view::HitTarget::None => Effect::None,
        }
    }

    fn mouse_scroll(&mut self, point: Position, amount: isize) -> Effect {
        self.text_selection = None;
        match super::view::hit_test(self, point) {
            super::view::HitTarget::Preview | super::view::HitTarget::PreviewText
                if self.preview_session.is_some() =>
            {
                self.pane = Pane::Preview;
                self.toolbar_focus = None;
                self.preview_scroll = if amount < 0 {
                    self.preview_scroll.saturating_sub(3)
                } else {
                    self.preview_scroll.saturating_add(3)
                };
                Effect::None
            }
            super::view::HitTarget::Session(_) | super::view::HitTarget::List => {
                self.text_selection = None;
                if self.pane == Pane::Preview {
                    self.focus_session_list();
                }
                self.toolbar_focus = None;
                self.move_selection(amount.saturating_mul(3))
            }
            _ => Effect::None,
        }
    }

    fn update_text_selection(&mut self, point: Position) {
        let area = super::view::preview_text_area(self);
        let Some(point) = clamp_point(area, point) else {
            return;
        };
        if let Some(selection) = &mut self.text_selection {
            selection.head = point;
            selection.dragged |= selection.anchor != point;
        }
    }

    fn toolbar_key(&mut self, item: ToolbarItem, code: KeyCode) -> Effect {
        match code {
            KeyCode::Left | KeyCode::BackTab => {
                self.toolbar_focus = Some(item.previous());
                Effect::None
            }
            KeyCode::Right | KeyCode::Tab => {
                self.toolbar_focus = Some(item.next());
                Effect::None
            }
            KeyCode::Down => self.focus_list(),
            KeyCode::Enter if item == ToolbarItem::Search => self.focus_list(),
            KeyCode::Enter | KeyCode::Char(' ') if item != ToolbarItem::Search => {
                self.cycle_toolbar_filter(item)
            }
            KeyCode::Backspace => {
                self.toolbar_focus = Some(ToolbarItem::Search);
                self.query.pop();
                self.selection_touched = true;
                self.rebuild_filter(None);
                Effect::None
            }
            KeyCode::Esc if !self.query.is_empty() => {
                self.query.clear();
                self.selection_touched = true;
                self.rebuild_filter(None);
                Effect::None
            }
            KeyCode::Esc => Effect::Cancel,
            KeyCode::Char(character) if !character.is_control() => {
                self.toolbar_focus = Some(ToolbarItem::Search);
                self.query.push(character);
                self.selection_touched = true;
                self.rebuild_filter(None);
                Effect::None
            }
            _ => Effect::None,
        }
    }

    fn focus_list(&mut self) -> Effect {
        self.pane = Pane::List;
        self.toolbar_focus = None;
        self.selection_touched = true;
        self.selected = 0;
        self.list_offset = 0;
        Effect::None
    }

    fn cycle_toolbar_filter(&mut self, item: ToolbarItem) -> Effect {
        match item {
            ToolbarItem::Search => return Effect::None,
            ToolbarItem::Origin => self.origin_filter = self.origin_filter.next(),
            ToolbarItem::Agent => self.agent_filter = self.agent_filter.next(),
            ToolbarItem::Scope => self.scope_filter = self.scope_filter.next(),
        }
        self.selection_touched = true;
        self.rebuild_filter(None);
        Effect::None
    }

    fn list_key(&mut self, code: KeyCode) -> Effect {
        match code {
            KeyCode::Esc | KeyCode::Char('q') => Effect::Cancel,
            KeyCode::Char('?') => {
                self.mode = Mode::Help;
                self.overlay_scroll = 0;
                Effect::None
            }
            KeyCode::Char('!') | KeyCode::Char('w') if !self.warnings.is_empty() => {
                self.mode = Mode::Diagnostics;
                self.overlay_scroll = 0;
                Effect::None
            }
            KeyCode::Char('x') => {
                self.query.clear();
                self.origin_filter = OriginFilter::All;
                self.agent_filter = AgentFilter::All;
                self.scope_filter = ScopeFilter::All;
                self.selection_touched = true;
                self.rebuild_filter(None);
                Effect::None
            }
            KeyCode::Up | KeyCode::Char('k') if self.selected == 0 => self.focus_search(),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-(self.page_size() as isize)),
            KeyCode::PageDown => self.move_selection(self.page_size() as isize),
            KeyCode::Home => self.select_absolute(0),
            KeyCode::End => self.select_absolute(self.filtered.len().saturating_sub(1)),
            KeyCode::Left | KeyCode::Right => self.focus_preview(),
            KeyCode::Enter => self.open_preview(),
            KeyCode::Char('r') => Effect::Refresh,
            KeyCode::Char('1') | KeyCode::Char('o') => {
                self.origin_filter = self.origin_filter.next();
                self.selection_touched = true;
                self.rebuild_filter(None);
                Effect::None
            }
            KeyCode::Char('2') | KeyCode::Char('a') => {
                self.agent_filter = self.agent_filter.next();
                self.selection_touched = true;
                self.rebuild_filter(None);
                Effect::None
            }
            KeyCode::Char('3') | KeyCode::Char('s') => {
                self.scope_filter = self.scope_filter.next();
                self.selection_touched = true;
                self.rebuild_filter(None);
                Effect::None
            }
            _ => Effect::None,
        }
    }

    fn preview_key(&mut self, code: KeyCode) -> Effect {
        match code {
            KeyCode::Left | KeyCode::Right => {
                self.focus_session_list();
                Effect::None
            }
            KeyCode::Esc => Effect::Cancel,
            KeyCode::Char('q') => Effect::Cancel,
            KeyCode::Char('?') => {
                self.mode = Mode::Help;
                self.overlay_scroll = 0;
                Effect::None
            }
            KeyCode::Char('!') | KeyCode::Char('w') if !self.warnings.is_empty() => {
                self.mode = Mode::Diagnostics;
                self.overlay_scroll = 0;
                Effect::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.preview_scroll = self.preview_scroll.saturating_sub(1);
                Effect::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.preview_scroll = self.preview_scroll.saturating_add(1);
                Effect::None
            }
            KeyCode::PageUp => {
                self.preview_scroll = self
                    .preview_scroll
                    .saturating_sub(self.area.height.saturating_sub(8).max(1));
                Effect::None
            }
            KeyCode::PageDown => {
                self.preview_scroll = self
                    .preview_scroll
                    .saturating_add(self.area.height.saturating_sub(8).max(1));
                Effect::None
            }
            KeyCode::Home => {
                self.preview_scroll = 0;
                Effect::None
            }
            KeyCode::End => {
                self.preview_scroll = u16::MAX;
                Effect::None
            }
            KeyCode::Char('v') => {
                self.preview_density = self.preview_density.next();
                self.preview_scroll = 0;
                self.request_preview()
            }
            KeyCode::Char('f') => self.toggle_favorite(),
            KeyCode::Char('y') => self
                .preview_entry()
                .filter(|entry| entry.disabled_reason.is_none())
                .map(|entry| Effect::CopySessionDescription(entry.complete_description()))
                .unwrap_or(Effect::None),
            KeyCode::Char(' ') => self.open_review(),
            _ => Effect::None,
        }
    }

    fn open_preview(&mut self) -> Effect {
        let Some(entry) = self.selected_entry().cloned() else {
            return Effect::None;
        };
        self.display_preview(Some(entry));
        self.pane = Pane::Preview;
        self.toolbar_focus = None;
        self.selection_touched = true;
        self.preview_scroll = 0;
        self.keep_selection_visible();
        self.request_preview()
    }

    fn display_preview(&mut self, entry: Option<SessionEntry>) {
        let changed = self
            .preview_session
            .as_ref()
            .map(|entry| entry.key.as_str())
            != entry.as_ref().map(|entry| entry.key.as_str());
        self.preview_session = entry;
        self.preview_target_open = self.preview_session.is_some();
        self.preview_transition = if self.preview_target_open {
            PREVIEW_TRANSITION_MAX
        } else {
            self.pane = Pane::List;
            0
        };
        if changed {
            self.preview_scroll = 0;
        }
        self.text_selection = None;
    }

    fn focus_search(&mut self) -> Effect {
        self.mode = Mode::Browse;
        self.pane = Pane::List;
        self.toolbar_focus = Some(ToolbarItem::Search);
        self.text_selection = None;
        Effect::None
    }

    fn focus_preview(&mut self) -> Effect {
        if self.preview_session.is_some() {
            self.pane = Pane::Preview;
            self.toolbar_focus = None;
            self.text_selection = None;
        }
        Effect::None
    }

    fn focus_session_list(&mut self) {
        self.pane = Pane::List;
        self.toolbar_focus = None;
        self.text_selection = None;
        self.keep_selection_visible();
    }

    fn forget_preview_session(&mut self) {
        if let Some(session) = self.preview_session.take()
            && self.entries.iter().all(|entry| entry.key != session.key)
        {
            self.previews.remove(&session.key);
        }
    }

    fn open_review(&mut self) -> Effect {
        let Some(entry) = self.preview_entry() else {
            return Effect::None;
        };
        if let Some(reason) = &entry.disabled_reason {
            self.status = Some(format!("Unavailable: {}", clean(reason)));
            return Effect::None;
        }
        self.mode = Mode::Review;
        self.review_action = self.initial_review_action;
        Effect::None
    }

    fn toggle_favorite(&mut self) -> Effect {
        let Some(index) = self
            .preview_session
            .as_ref()
            .map(|entry| entry.key.as_str())
            .and_then(|key| self.entries.iter().position(|entry| entry.key == key))
        else {
            return Effect::None;
        };
        let entry = &mut self.entries[index];
        if entry.disabled_reason.is_some() {
            self.status = Some("Diagnostics cannot be favorited".into());
            return Effect::None;
        }
        entry.favorite = !entry.favorite;
        self.favorite_overrides
            .insert(entry.key.clone(), entry.favorite);
        let effect = Effect::SetFavorite {
            key: entry.key.clone(),
            favorite: entry.favorite,
        };
        let key = entry.key.clone();
        self.preview_session = Some(entry.clone());
        self.rebuild_filter(Some(&key));
        let still_visible = self
            .filtered
            .iter()
            .any(|index| self.entries[*index].key == key);
        if self.preview_actions_enabled() && !still_visible {
            self.pane = Pane::List;
            self.status = Some("Session no longer matches the active filters".into());
        }
        effect
    }

    fn move_selection(&mut self, amount: isize) -> Effect {
        if self.filtered.is_empty() {
            return Effect::None;
        }
        self.selected = self
            .selected
            .saturating_add_signed(amount)
            .min(self.filtered.len() - 1);
        self.selection_touched = true;
        self.preview_scroll = 0;
        self.keep_selection_visible();
        Effect::None
    }

    fn select_absolute(&mut self, position: usize) -> Effect {
        if self.filtered.is_empty() {
            return Effect::None;
        }
        self.selected = position.min(self.filtered.len() - 1);
        self.selection_touched = true;
        self.preview_scroll = 0;
        self.keep_selection_visible();
        Effect::None
    }

    fn keep_selection_visible(&mut self) {
        let page = self.page_size();
        if self.selected < self.list_offset {
            self.list_offset = self.selected;
        } else if self.selected >= self.list_offset.saturating_add(page) {
            self.list_offset = self.selected.saturating_add(1).saturating_sub(page);
        }
        self.list_offset = self
            .list_offset
            .min(self.filtered.len().saturating_sub(page));
    }

    fn rebuild_filter(&mut self, preserve_key: Option<&str>) {
        let previous = preserve_key.map(ToOwned::to_owned);
        let tokens = self
            .query
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>();
        let mut matches = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| self.matches_filters(entry))
            .filter_map(|(index, entry)| {
                fuzzy_score(&entry.searchable_text(), &tokens).map(|score| (index, score))
            })
            .collect::<Vec<_>>();
        matches.sort_by(|(left_index, left_score), (right_index, right_score)| {
            right_score
                .cmp(left_score)
                .then_with(|| {
                    self.entries[*right_index]
                        .sort_timestamp
                        .cmp(&self.entries[*left_index].sort_timestamp)
                })
                .then_with(|| left_index.cmp(right_index))
        });
        self.filtered = matches.into_iter().map(|(index, _)| index).collect();
        self.selected = previous
            .as_deref()
            .and_then(|key| {
                self.filtered
                    .iter()
                    .position(|index| self.entries[*index].key == key)
            })
            .unwrap_or(0)
            .min(self.filtered.len().saturating_sub(1));
        self.preview_scroll = 0;
        self.keep_selection_visible();
    }

    fn matches_filters(&self, entry: &SessionEntry) -> bool {
        let origin = match self.origin_filter {
            OriginFilter::All => true,
            OriginFilter::Local => entry.origin == Origin::Local,
            OriginFilter::Remote => entry.origin == Origin::Remote,
        };
        let agent = match self.agent_filter {
            AgentFilter::All => true,
            AgentFilter::Codex => entry.agent.name() == "codex",
            AgentFilter::Claude => entry.agent.name() == "claude",
        };
        let scope = match self.scope_filter {
            ScopeFilter::All => true,
            ScopeFilter::CurrentProject => entry.current_project,
            ScopeFilter::Favorites => entry.favorite,
        };
        origin && agent && scope
    }

    fn request_preview(&mut self) -> Effect {
        if self.preview_density == PreviewDensity::Metadata {
            return Effect::None;
        }
        let Some(entry) = self.preview_entry() else {
            return Effect::None;
        };
        if entry.disabled_reason.is_some() {
            return Effect::None;
        }
        let key = entry.key.clone();
        if self.previews.contains_key(&key) {
            return Effect::None;
        }
        self.previews.insert(key.clone(), PreviewState::Loading);
        Effect::LoadPreview(key)
    }
}

fn clamp_point(area: Rect, point: Position) -> Option<Position> {
    if area.is_empty() {
        return None;
    }
    Some(Position::new(
        point.x.clamp(area.x, area.right().saturating_sub(1)),
        point.y.clamp(area.y, area.bottom().saturating_sub(1)),
    ))
}

/// Token-aware fuzzy match. Every token must be a case-insensitive
/// subsequence; contiguous and early matches rank higher. Catalog order is the
/// final deterministic tie breaker in `rebuild_filter`.
fn fuzzy_score(haystack: &str, tokens: &[String]) -> Option<i64> {
    if tokens.is_empty() {
        return Some(0);
    }
    let haystack = haystack.to_ascii_lowercase();
    let characters = haystack.char_indices().collect::<Vec<_>>();
    let mut total = 0i64;
    for token in tokens {
        let mut after = 0usize;
        let mut previous = None;
        let mut first = None;
        let mut contiguous = 0i64;
        for needle in token.chars() {
            let (position, (byte, _)) = characters
                .iter()
                .enumerate()
                .skip(after)
                .find(|(_, (_, candidate))| *candidate == needle)?;
            first.get_or_insert(*byte);
            if previous.is_some_and(|last| last + 1 == position) {
                contiguous += 8;
            }
            previous = Some(position);
            after = position + 1;
        }
        total += 1000 + contiguous - i64::try_from(first.unwrap_or(0)).unwrap_or(i64::MAX) / 4;
    }
    Some(total)
}

#[cfg(test)]
mod tests {
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

        let Effect::CopySessionDescription(description) = model.apply(key(KeyCode::Char('y')))
        else {
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
}
