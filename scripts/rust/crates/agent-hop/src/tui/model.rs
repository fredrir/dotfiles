use std::collections::HashMap;

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::layout::{Margin, Rect};

use super::{
    AgentFilter, CatalogSnapshot, Origin, OriginFilter, PickerAction, PickerView, Preview,
    PreviewDensity, ScopeFilter, SessionEntry, clean,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Pane {
    List,
    Preview,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PreviewState {
    Loading,
    Ready(Preview),
    Error(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Mode {
    Browse,
    Search,
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
    CopySessionId(String),
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
    pub(crate) mode: Mode,
    pub(crate) review_action: usize,
    initial_review_action: usize,
    pub(crate) previews: HashMap<String, PreviewState>,
    favorite_overrides: HashMap<String, bool>,
    pub(crate) area: Rect,
    pub(crate) animation_frame: u64,
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
            mode: Mode::Browse,
            review_action: 0,
            initial_review_action: 0,
            previews: HashMap::new(),
            favorite_overrides: HashMap::new(),
            area: Rect::new(0, 0, 80, 24),
            animation_frame: 0,
        }
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

    pub(crate) fn advance_animation(&mut self) {
        self.animation_frame = self.animation_frame.wrapping_add(1);
    }

    pub(crate) fn load(&mut self, mut snapshot: CatalogSnapshot, complete: bool) -> Effect {
        let selected_key = self.selected_entry().map(|entry| entry.key.clone());
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
        let selected = self.selected_entry().is_some_and(|entry| entry.key == key);
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
        }
    }

    pub(crate) fn preview_skipped(&mut self, key: &str) -> Effect {
        if matches!(self.previews.get(key), Some(PreviewState::Loading)) {
            self.previews.remove(key);
        }
        if self.selected_entry().is_some_and(|entry| entry.key == key) {
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
        self.status = Some("Session ID copied with dclip".into());
    }

    pub(crate) fn copy_failed(&mut self, error: String) {
        self.status = Some(format!("Could not copy session ID: {}", clean(&error)));
    }

    pub(crate) fn selected_entry(&self) -> Option<&SessionEntry> {
        self.filtered
            .get(self.selected)
            .and_then(|index| self.entries.get(*index))
    }

    pub(crate) fn selected_preview(&self) -> Option<&PreviewState> {
        self.selected_entry()
            .and_then(|entry| self.previews.get(&entry.key))
    }

    pub(crate) fn page_size(&self) -> usize {
        let layout = super::view::layout(self.area, self.pane);
        usize::from(layout.list.height.saturating_sub(2).max(1))
    }

    pub(crate) fn apply(&mut self, event: UiEvent) -> Effect {
        match event {
            UiEvent::Resize(width, height) => {
                self.area = Rect::new(0, 0, width, height);
                self.keep_selection_visible();
                Effect::None
            }
            UiEvent::Key(key) if key.kind != KeyEventKind::Press => Effect::None,
            UiEvent::Key(key) => self.key(key),
            UiEvent::Mouse { kind, column, row } => self.mouse(kind, column, row),
        }
    }

    fn key(&mut self, key: KeyEvent) -> Effect {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return match key.code {
                KeyCode::Char('c') => Effect::Cancel,
                KeyCode::Char('n') => self.move_selection(1),
                KeyCode::Char('p') => self.move_selection(-1),
                KeyCode::Char('u') if self.mode == Mode::Search => {
                    self.query.clear();
                    self.rebuild_filter(None);
                    self.request_preview()
                }
                _ => Effect::None,
            };
        }

        match self.mode {
            Mode::Search => self.search_key(key.code),
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

    fn search_key(&mut self, code: KeyCode) -> Effect {
        match code {
            KeyCode::Esc | KeyCode::Enter => {
                self.mode = Mode::Browse;
                Effect::None
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.rebuild_filter(None);
                self.request_preview()
            }
            KeyCode::Char(character) if !character.is_control() => {
                self.query.push(character);
                self.rebuild_filter(None);
                self.request_preview()
            }
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Down => self.move_selection(1),
            _ => Effect::None,
        }
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
            KeyCode::Char('/') => {
                self.mode = Mode::Search;
                Effect::None
            }
            KeyCode::Char('x') => {
                self.query.clear();
                self.origin_filter = OriginFilter::All;
                self.agent_filter = AgentFilter::All;
                self.scope_filter = ScopeFilter::All;
                self.rebuild_filter(None);
                self.request_preview()
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.pane = match self.pane {
                    Pane::List => Pane::Preview,
                    Pane::Preview => Pane::List,
                };
                Effect::None
            }
            KeyCode::Up | KeyCode::Char('k') if self.pane == Pane::List => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') if self.pane == Pane::List => self.move_selection(1),
            KeyCode::PageUp if self.pane == Pane::List => {
                self.move_selection(-(self.page_size() as isize))
            }
            KeyCode::PageDown if self.pane == Pane::List => {
                self.move_selection(self.page_size() as isize)
            }
            KeyCode::Home if self.pane == Pane::List => self.select_absolute(0),
            KeyCode::End if self.pane == Pane::List => {
                self.select_absolute(self.filtered.len().saturating_sub(1))
            }
            KeyCode::Up | KeyCode::Char('k') if self.pane == Pane::Preview => {
                self.preview_scroll = self.preview_scroll.saturating_sub(1);
                Effect::None
            }
            KeyCode::Down | KeyCode::Char('j') if self.pane == Pane::Preview => {
                self.preview_scroll = self.preview_scroll.saturating_add(1);
                Effect::None
            }
            KeyCode::PageUp if self.pane == Pane::Preview => {
                self.preview_scroll = self
                    .preview_scroll
                    .saturating_sub(self.area.height.saturating_sub(8).max(1));
                Effect::None
            }
            KeyCode::PageDown if self.pane == Pane::Preview => {
                self.preview_scroll = self
                    .preview_scroll
                    .saturating_add(self.area.height.saturating_sub(8).max(1));
                Effect::None
            }
            KeyCode::Home if self.pane == Pane::Preview => {
                self.preview_scroll = 0;
                Effect::None
            }
            KeyCode::End if self.pane == Pane::Preview => {
                self.preview_scroll = u16::MAX;
                Effect::None
            }
            KeyCode::Enter => self.open_review(),
            KeyCode::Char('r') => Effect::Refresh,
            KeyCode::Char('1') | KeyCode::Char('o') => {
                self.origin_filter = self.origin_filter.next();
                self.rebuild_filter(None);
                self.request_preview()
            }
            KeyCode::Char('2') | KeyCode::Char('a') => {
                self.agent_filter = self.agent_filter.next();
                self.rebuild_filter(None);
                self.request_preview()
            }
            KeyCode::Char('3') | KeyCode::Char('s') => {
                self.scope_filter = self.scope_filter.next();
                self.rebuild_filter(None);
                self.request_preview()
            }
            KeyCode::Char('v') => {
                self.preview_density = self.preview_density.next();
                self.preview_scroll = 0;
                self.request_preview()
            }
            KeyCode::Char('f') => self.toggle_favorite(),
            KeyCode::Char('y') => self
                .selected_entry()
                .filter(|entry| entry.disabled_reason.is_none())
                .map(|entry| Effect::CopySessionId(entry.id.clone()))
                .unwrap_or(Effect::None),
            _ => Effect::None,
        }
    }

    fn mouse(&mut self, kind: MouseEventKind, column: u16, row: u16) -> Effect {
        if matches!(self.mode, Mode::Help | Mode::Diagnostics) {
            match kind {
                MouseEventKind::ScrollUp => {
                    self.overlay_scroll = self.overlay_scroll.saturating_sub(3)
                }
                MouseEventKind::ScrollDown => {
                    self.overlay_scroll = self.overlay_scroll.saturating_add(3)
                }
                _ => {}
            }
            return Effect::None;
        }
        if self.mode == Mode::Review {
            return match kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(action) = super::view::review_hit(self.area, column, row) {
                        self.review_action = action;
                    }
                    Effect::None
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    super::view::review_hit(self.area, column, row)
                        .map(|index| Effect::Pick(PickerAction::ALL[index]))
                        .unwrap_or(Effect::None)
                }
                _ => Effect::None,
            };
        }
        let regions = super::view::layout(self.area, self.pane);
        let list_inner = regions.list.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });
        match kind {
            MouseEventKind::ScrollUp if regions.preview.contains((column, row).into()) => {
                self.preview_scroll = self.preview_scroll.saturating_sub(3);
                Effect::None
            }
            MouseEventKind::ScrollDown if regions.preview.contains((column, row).into()) => {
                self.preview_scroll = self.preview_scroll.saturating_add(3);
                Effect::None
            }
            MouseEventKind::ScrollUp => self.move_selection(-3),
            MouseEventKind::ScrollDown => self.move_selection(3),
            MouseEventKind::Down(MouseButton::Left)
                if list_inner.contains((column, row).into()) =>
            {
                self.pane = Pane::List;
                if row >= list_inner.y {
                    let position = self.list_offset + usize::from(row - list_inner.y);
                    if position < self.filtered.len() {
                        self.selected = position;
                        self.preview_scroll = 0;
                        return self.request_preview();
                    }
                }
                Effect::None
            }
            MouseEventKind::Down(MouseButton::Left)
                if regions.preview.contains((column, row).into()) =>
            {
                self.pane = Pane::Preview;
                Effect::None
            }
            _ => Effect::None,
        }
    }

    fn open_review(&mut self) -> Effect {
        let Some(entry) = self.selected_entry() else {
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
        let Some(index) = self.filtered.get(self.selected).copied() else {
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
        self.rebuild_filter(Some(&key));
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
        self.preview_scroll = 0;
        self.keep_selection_visible();
        self.request_preview()
    }

    fn select_absolute(&mut self, position: usize) -> Effect {
        if self.filtered.is_empty() {
            return Effect::None;
        }
        self.selected = position.min(self.filtered.len() - 1);
        self.preview_scroll = 0;
        self.keep_selection_visible();
        self.request_preview()
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
        let previous = preserve_key
            .map(ToOwned::to_owned)
            .or_else(|| self.selected_entry().map(|entry| entry.key.clone()));
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
                        .current_project
                        .cmp(&self.entries[*left_index].current_project)
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
        let Some(key) = self.selected_entry().map(|entry| entry.key.clone()) else {
            return Effect::None;
        };
        if self
            .selected_entry()
            .is_some_and(|entry| entry.disabled_reason.is_some())
        {
            return Effect::None;
        }
        if self.previews.contains_key(&key) {
            return Effect::None;
        }
        self.previews.insert(key.clone(), PreviewState::Loading);
        Effect::LoadPreview(key)
    }
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
            sort_timestamp: 0,
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
        model
    }

    fn key(code: KeyCode) -> UiEvent {
        UiEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn current_project_is_first_and_navigation_is_bounded() {
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
    fn preview_density_cycles_and_metadata_avoids_transcript_reads() {
        let mut model = loaded();
        model.previews.clear();
        assert_eq!(
            model.apply(key(KeyCode::Char('v'))),
            Effect::LoadPreview("current".into())
        );
        assert_eq!(model.preview_density, PreviewDensity::Compact);
        model.previews.clear();
        assert_eq!(model.apply(key(KeyCode::Char('v'))), Effect::None);
        assert_eq!(model.preview_density, PreviewDensity::Metadata);
        model.apply(key(KeyCode::Char('v')));
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
    fn enter_requires_review_and_disabled_rows_cannot_apply() {
        let mut model = loaded();
        assert_eq!(model.apply(key(KeyCode::Enter)), Effect::None);
        assert_eq!(model.mode, Mode::Review);
        assert_eq!(
            model.apply(key(KeyCode::Char('d'))),
            Effect::Pick(PickerAction::DryRun)
        );

        model.mode = Mode::Browse;
        let index = model.filtered[model.selected];
        model.entries[index].disabled_reason = Some("missing transcript".into());
        assert_eq!(model.apply(key(KeyCode::Enter)), Effect::None);
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
        assert_eq!(model.apply(key(KeyCode::Home)), Effect::None);
        assert!(matches!(
            model.previews.get(&session_key),
            Some(PreviewState::Ready(_))
        ));
    }

    #[test]
    fn refresh_keeps_remote_selection_and_invalidates_cached_previews() {
        let mut model = loaded();
        model.rebuild_filter(Some("favorite"));
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
    }

    #[test]
    fn complete_remote_merge_preserves_a_live_favorite_change() {
        let mut model = loaded();
        model.rebuild_filter(Some("favorite"));
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
        assert_eq!(model.mode, Mode::Review);
        assert_eq!(model.apply(key(KeyCode::Enter)), Effect::None);
        assert!(model.status.as_deref().unwrap().contains("Resize"));
    }
}
