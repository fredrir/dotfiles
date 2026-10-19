use super::*;

impl App {
    pub(super) fn batchable_input(&self, waited: &ui_terminal::Waited) -> bool {
        match waited {
            ui_terminal::Waited::Event(Event::Mouse(MouseEvent {
                kind:
                    MouseEventKind::Moved
                    | MouseEventKind::ScrollUp
                    | MouseEventKind::ScrollDown
                    | MouseEventKind::ScrollLeft
                    | MouseEventKind::ScrollRight,
                ..
            })) => true,
            ui_terminal::Waited::Event(Event::Key(key))
                if self.dialog.as_ref().and_then(Dialog::editor).is_some() =>
            {
                key.code != KeyCode::Esc
                    && !(key.modifiers.is_empty() && key.code == KeyCode::Enter)
            }
            _ => false,
        }
    }

    pub(super) fn available_actions(
        &self,
        query: &str,
    ) -> Vec<&'static crate::actions::Definition> {
        ACTIONS
            .iter()
            .filter(|definition| {
                let available = match definition.action {
                    Action::Inspect => self.history.current().is_some(),
                    Action::Commit | Action::Compose => self.staged_count > 0,
                    Action::Stage
                    | Action::Unstage
                    | Action::StageFile
                    | Action::UnstageFile
                    | Action::ToggleStage
                    | Action::ToggleStageAll
                    | Action::Discard
                    | Action::ToggleFiles
                    | Action::ShowStaged
                    | Action::ShowUnstaged
                    | Action::ToggleSide => self.view.workspace == Workspace::Changes,
                    Action::StashApply | Action::StashPop | Action::StashDrop => {
                        matches!(self.selected(), Some(Target::Stash(..)))
                    }
                    Action::CherryPick
                    | Action::Revert
                    | Action::CompareSelected
                    | Action::FollowParent
                    | Action::FollowChild
                    | Action::PinCommit => matches!(self.selected(), Some(Target::Commit(_))),
                    Action::Switch => matches!(
                        self.selected(),
                        Some(Target::Branch(..) | Target::Worktree(_))
                    ),
                    Action::Continue | Action::Abort => self.snapshot.operation.is_some(),
                    Action::LoadMore => self.view.workspace == Workspace::History,
                    Action::Edit => self.view.workspace == Workspace::Changes,
                    _ => true,
                };
                available
                    && definition
                        .label
                        .to_lowercase()
                        .contains(&query.to_lowercase())
            })
            .collect()
    }

    pub(super) fn key(
        &mut self,
        mut key: KeyEvent,
        terminal: &mut Option<crate::session::Session>,
        signals: &mut Option<ui_terminal::SignalGuard>,
    ) -> Result<(), String> {
        if key.kind == KeyEventKind::Release {
            return Ok(());
        }
        self.view.nav_hovered = None;
        if !matches!(key.code, KeyCode::Modifier(_)) {
            self.scroll.stop();
        }
        self.dirty = true;
        if key.code == KeyCode::Char('h') && key.modifiers == KeyModifiers::CONTROL {
            key.code = KeyCode::Backspace;
        }
        if self.dialog.is_some() {
            return self.dialog_key(key, terminal, signals);
        }
        if key.code == KeyCode::Esc && self.notification.take().is_some() {
            return Ok(());
        }
        if self.view.workspace == Workspace::Changes && self.view.focus == Focus::Navigator {
            if key.modifiers == KeyModifiers::SUPER
                && matches!(key.code, KeyCode::Up | KeyCode::Down)
            {
                self.move_to_edge(key.code == KeyCode::Down);
                return Ok(());
            }
            if key.code == KeyCode::Char(' ') && key.modifiers == KeyModifiers::SHIFT {
                if key.kind != KeyEventKind::Repeat {
                    return self.action(Action::ToggleStageAll, terminal, signals);
                }
                return Ok(());
            }
        }
        if let KeyCode::Char(character) = key.code {
            if character.is_ascii_digit() && !(key.modifiers - KeyModifiers::SHIFT).is_empty() {
                return Ok(());
            }
            if (key.modifiers - KeyModifiers::SHIFT).is_empty() && ('1'..='6').contains(&character)
            {
                self.workspace(Workspace::ALL[(character as u8 - b'1') as usize]);
                return Ok(());
            }
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('r') => {
                    self.refresh();
                    return Ok(());
                }
                KeyCode::Char('c') => {
                    if self.operation.is_none() {
                        self.quit = true;
                    } else {
                        self.note("An operation is still running");
                    }
                    return Ok(());
                }
                KeyCode::Char('p') => {
                    self.open_actions();
                    return Ok(());
                }
                KeyCode::Enter | KeyCode::Char('g') => {
                    return self.action(Action::Continue, terminal, signals);
                }
                KeyCode::Left => {
                    self.view.nav_width = self.view.nav_width.saturating_sub(2).max(18);
                    return Ok(());
                }
                KeyCode::Right => {
                    self.view.nav_width = self.view.nav_width.saturating_add(2);
                    return Ok(());
                }
                _ => {}
            }
        }
        match key.code {
            KeyCode::Char('q') => {
                if self.operation.is_none() {
                    self.quit = true;
                } else {
                    self.note("An operation is still running");
                }
            }
            KeyCode::Esc => {
                if !self.marked.is_empty() {
                    self.marked.clear();
                } else if !self.query.is_empty() {
                    self.query.clear();
                    self.rebuild();
                    self.load_content(true);
                } else if self.history_commit.is_some() {
                    self.leave_commit();
                } else if self.operation.is_none() {
                    self.quit = true;
                } else {
                    self.note("An operation is still running");
                }
            }
            KeyCode::Backspace => self.leave_commit(),
            KeyCode::Char(':') | KeyCode::Char('?') => {
                self.open_actions();
            }
            KeyCode::Char('/') => {
                self.prompt(Prompt::Search, "Search", &self.query.clone());
            }
            KeyCode::Tab | KeyCode::BackTab if (key.modifiers - KeyModifiers::SHIFT).is_empty() => {
                self.cycle_focus(
                    key.code == KeyCode::BackTab || key.modifiers.contains(KeyModifiers::SHIFT),
                );
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_cursor(1, key.modifiers.contains(KeyModifiers::SHIFT))
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_cursor(-1, key.modifiers.contains(KeyModifiers::SHIFT))
            }
            KeyCode::PageDown => self.move_cursor(15, false),
            KeyCode::PageUp => self.move_cursor(-15, false),
            KeyCode::Home => self.move_to_edge(false),
            KeyCode::End => self.move_to_edge(true),
            KeyCode::Left | KeyCode::Char('h') => {
                self.horizontal_navigation(false, terminal, signals)?
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.horizontal_navigation(true, terminal, signals)?
            }
            KeyCode::Char('i') => self.inspect_commit(),
            KeyCode::Enter if key.kind == KeyEventKind::Repeat => {}
            KeyCode::Enter => return self.enter(terminal, signals),
            KeyCode::Char(' ') if (key.modifiers - KeyModifiers::SHIFT).is_empty() => {
                if self.view.focus == Focus::Canvas {
                    self.mark_current(false);
                } else if matches!(self.selected(), Some(Target::Commit(_))) {
                    return self.action(Action::PinCommit, terminal, signals);
                } else if self.view.workspace == Workspace::Changes
                    && key.kind != KeyEventKind::Repeat
                {
                    return self.action(Action::ToggleStage, terminal, signals);
                }
            }
            KeyCode::Char('[')
                if self.view.focus == Focus::Navigator
                    && self.view.workspace == Workspace::History =>
            {
                return self.action(Action::FollowParent, terminal, signals);
            }
            KeyCode::Char('[') => self.jump_hunk(false),
            KeyCode::Char(']')
                if self.view.focus == Focus::Navigator
                    && self.view.workspace == Workspace::History =>
            {
                return self.action(Action::FollowChild, terminal, signals);
            }
            KeyCode::Char(']') => self.jump_hunk(true),
            KeyCode::Char(character @ ('s' | 'u'))
                if self.view.workspace == Workspace::Changes && key.modifiers.is_empty() =>
            {
                if key.kind == KeyEventKind::Repeat {
                    return Ok(());
                }
                return self.action(
                    if character == 's' {
                        Action::ShowStaged
                    } else {
                        Action::ShowUnstaged
                    },
                    terminal,
                    signals,
                );
            }
            KeyCode::Char(character) => {
                let action = match character {
                    'e' => Some(Action::Edit),
                    's' if self.view.workspace == Workspace::Stashes => Some(Action::StashApply),
                    'S' => Some(Action::StageFile),
                    'U' => Some(Action::UnstageFile),
                    'c' => Some(Action::Commit),
                    'C' => Some(Action::Compose),
                    'a' => Some(Action::Amend),
                    'f' => Some(Action::Fetch),
                    'p' => Some(if self.view.workspace == Workspace::Stashes {
                        Action::StashPop
                    } else {
                        Action::Pull
                    }),
                    'P' => Some(Action::Push),
                    'n' => Some(Action::CreateBranch),
                    'z' => Some(Action::StashSave),
                    'd' => Some(if self.view.workspace == Workspace::Stashes {
                        Action::StashDrop
                    } else {
                        Action::Discard
                    }),
                    'x' => Some(Action::CherryPick),
                    'r' => Some(Action::Revert),
                    'm' => Some(Action::Merge),
                    'R' => Some(Action::Rebase),
                    't' => Some(Action::ToggleFiles),
                    'b' => Some(Action::ToggleSide),
                    'D' => Some(Action::ToggleStructural),
                    'v' => Some(Action::ToggleSplit),
                    'y' => Some(Action::Copy),
                    '=' => Some(Action::CompareSelected),
                    '+' => Some(Action::LoadMore),
                    _ => None,
                };
                if let Some(action) = action {
                    return self.action(action, terminal, signals);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn cycle_focus(&mut self, backwards: bool) {
        let panes: &[Focus] = if self.history.current().is_some() {
            &[Focus::Navigator, Focus::CommitDetails, Focus::Canvas]
        } else {
            &[Focus::Navigator, Focus::Canvas]
        };
        let index = panes
            .iter()
            .position(|focus| *focus == self.view.focus)
            .unwrap_or(0);
        self.view.focus =
            panes[(index + if backwards { panes.len() - 1 } else { 1 }) % panes.len()];
        self.view.commit_field = self.view.commit_field.min(self.commit_field_count() - 1);
    }

    fn horizontal_navigation(
        &mut self,
        right: bool,
        terminal: &mut Option<crate::session::Session>,
        signals: &mut Option<ui_terminal::SignalGuard>,
    ) -> Result<(), String> {
        match self.view.focus {
            Focus::Navigator => {
                if right {
                    if matches!(self.selected(), Some(Target::Commit(_))) {
                        return self.enter(terminal, signals);
                    }
                    self.view.focus = Focus::Canvas;
                    return Ok(());
                }
                let root = self.entries.get(self.view.selected).is_some_and(|entry| {
                    entry.item.depth == 0 && entry.item.kind == NavKind::Folder
                });
                if !root && self.collapse_selected() {
                    return Ok(());
                }
            }
            Focus::Canvas => {
                if self.scroll_canvas_horizontal(if right { 8 } else { -8 }) {
                    return Ok(());
                }
                if !right {
                    self.view.focus = Focus::Navigator;
                    return Ok(());
                }
            }
            Focus::CommitDetails => {
                if self.history_commit.is_some() && right {
                    self.view.focus = Focus::Canvas;
                    return Ok(());
                }
                if self.history_commit.is_none() && !right {
                    self.view.focus = Focus::Navigator;
                    return Ok(());
                }
            }
        }
        let index = Workspace::ALL
            .iter()
            .position(|workspace| *workspace == self.view.workspace)
            .unwrap_or(0);
        let next =
            (index + if right { 1 } else { Workspace::ALL.len() - 1 }) % Workspace::ALL.len();
        self.workspace(Workspace::ALL[next]);
        self.view.focus = if right {
            Focus::Navigator
        } else {
            Focus::Canvas
        };
        Ok(())
    }

    pub(super) fn canvas_horizontal_limit(&self) -> usize {
        self.geometry
            .as_ref()
            .and_then(|geometry| {
                let width = geometry.canvas.width;
                if let Some(text) = &self.structural {
                    Some(view::text_horizontal_limit(text.lines(), width))
                } else if let Some(diff) = &self.diff {
                    Some(view::configured_diff_horizontal_limit(
                        diff,
                        width,
                        self.view.split,
                        self.diff_config(),
                    ))
                } else if !self.text.is_empty() {
                    Some(view::text_horizontal_limit(
                        self.text.iter().map(String::as_str),
                        width,
                    ))
                } else {
                    None
                }
            })
            .unwrap_or(0)
    }

    fn scroll_canvas_horizontal(&mut self, amount: isize) -> bool {
        let previous = self.view.horizontal;
        self.view.horizontal = previous
            .saturating_add_signed(amount)
            .min(self.canvas_horizontal_limit());
        previous != self.view.horizontal
    }

    pub(super) fn move_cursor(&mut self, amount: isize, select: bool) {
        if self.view.focus == Focus::CommitDetails {
            self.view.commit_manual_scroll = false;
            self.view.commit_field = (self.view.commit_field as isize + amount)
                .rem_euclid(self.commit_field_count() as isize)
                as usize;
            return;
        }
        if self.view.focus == Focus::Navigator {
            self.view.nav_manual_scroll = false;
            let old = self.view.selected;
            let last = self.entries.len().saturating_sub(1);
            self.view.selected = match (amount, old) {
                (-1, 0) => last,
                (1, index) if index >= last => 0,
                _ => old.saturating_add_signed(amount).min(last),
            };
            if old != self.view.selected {
                self.load_content(true);
            }
        } else {
            if !select && matches!(amount, -1 | 1) && self.has_diff_hunks() {
                self.navigate_hunk(amount > 0);
                return;
            }
            if select {
                self.mark_current(true);
            }
            self.view.diff_cursor = self.view.diff_cursor.saturating_add_signed(amount);
            self.clamp_cursor();
            if select {
                self.mark_current(true);
            }
            self.follow_canvas_cursor();
        }
    }

    fn move_to_edge(&mut self, end: bool) {
        if self.view.focus == Focus::CommitDetails {
            self.view.commit_manual_scroll = false;
            self.view.commit_field = if end {
                self.commit_field_count() - 1
            } else {
                0
            };
            return;
        }
        if self.view.focus == Focus::Navigator {
            self.view.nav_manual_scroll = false;
            let old = self.view.selected;
            self.view.selected = if end {
                self.entries.len().saturating_sub(1)
            } else {
                0
            };
            if old != self.view.selected {
                self.load_content(true);
            }
        } else {
            self.view.diff_cursor = if end && self.has_diff_hunks() {
                self.row_map
                    .iter()
                    .rposition(view::DiffRow::is_hunk_header)
                    .unwrap_or(0)
            } else if end {
                self.row_map
                    .len()
                    .max(self.content_line_count)
                    .saturating_sub(1)
            } else {
                0
            };
            if self.has_diff_hunks() {
                self.view.diff_offset = self.view.diff_cursor;
            }
            self.follow_canvas_cursor();
        }
    }

    fn follow_canvas_cursor(&mut self) {
        self.view.canvas_manual_scroll = false;
        let height = self
            .geometry
            .as_ref()
            .map_or(1, |geometry| usize::from(geometry.canvas.height).max(1));
        if self.view.diff_cursor < self.view.diff_offset {
            self.view.diff_offset = self.view.diff_cursor;
        } else if self.view.diff_cursor >= self.view.diff_offset.saturating_add(height) {
            self.view.diff_offset = self.view.diff_cursor + 1 - height;
        }
        self.view.diff_offset = self.view.diff_offset.min(
            self.row_map
                .len()
                .max(self.content_line_count)
                .saturating_sub(height),
        );
    }

    pub(super) fn mark_current(&mut self, add: bool) {
        if self.structural.is_some() {
            return;
        }
        let Some(row) = self.row_map.get(self.view.diff_cursor).copied() else {
            return;
        };
        let ids: Vec<_> = [row.line, row.right_line]
            .into_iter()
            .flatten()
            .filter(|id| self.changed_lines.contains(id))
            .collect();
        let remove = !add && ids.iter().all(|id| self.marked.contains(id));
        for id in ids {
            if remove {
                self.marked.remove(&id);
            } else {
                self.marked.insert(id);
            }
        }
    }

    fn has_diff_hunks(&self) -> bool {
        self.structural.is_none() && !self.row_map.is_empty()
    }

    fn navigate_hunk(&mut self, forward: bool) {
        let Some(current) = self.row_map.get(self.view.diff_cursor) else {
            return;
        };
        let start = self.row_map.partition_point(|row| row.hunk < current.hunk);
        let end = self.row_map.partition_point(|row| row.hunk <= current.hunk);
        let height = self
            .geometry
            .as_ref()
            .map_or(self.row_map.len(), |geometry| {
                usize::from(geometry.canvas.height).max(1)
            });
        let step = height.saturating_sub(2).max(1);
        let offset = if forward {
            if end > self.view.diff_offset.saturating_add(height) {
                self.view
                    .diff_offset
                    .saturating_add(step)
                    .min(end.saturating_sub(height))
                    .max(start)
            } else if end < self.row_map.len() {
                end
            } else {
                return;
            }
        } else if self.view.diff_offset > start {
            self.view.diff_offset.saturating_sub(step).max(start)
        } else if let Some(previous) = start.checked_sub(1) {
            let previous_hunk = self.row_map[previous].hunk;
            let previous_start = self.row_map.partition_point(|row| row.hunk < previous_hunk);
            start.saturating_sub(height).max(previous_start)
        } else {
            return;
        };
        self.view.diff_cursor = offset;
        self.view.diff_offset = offset;
        self.follow_canvas_cursor();
    }

    pub(super) fn jump_hunk(&mut self, forward: bool) {
        if !self.has_diff_hunks() {
            return;
        }
        let rows = &self.row_map;
        let cursor = self.view.diff_cursor;
        let index = if forward {
            rows.iter()
                .enumerate()
                .find(|(index, row)| *index > cursor && row.is_hunk_header())
                .map(|(index, _)| index)
        } else {
            rows.iter()
                .enumerate()
                .rev()
                .find(|(index, row)| *index < cursor && row.is_hunk_header())
                .map(|(index, _)| index)
        };
        if let Some(index) = index {
            self.view.diff_cursor = index;
            self.view.diff_offset = index;
            self.view.focus = Focus::Canvas;
            self.follow_canvas_cursor();
        }
    }

    pub(super) fn toggle_folder(&mut self) {
        if let Some(Target::Path(path, true)) = self.selected().cloned() {
            let expanded = self.tree_collapsed().contains(&path);
            self.set_folder_expanded(path, expanded);
        }
    }

    pub(super) fn leave_commit(&mut self) {
        if let Some(oid) = self.history_commit.take() {
            self.view.nav_manual_scroll = false;
            self.rebuild();
            if let Some(index) = self
                .entries
                .iter()
                .position(|entry| entry.target == Target::Commit(oid.clone()))
            {
                self.view.selected = index;
            }
            self.view.focus = Focus::Navigator;
            self.load_content(true);
        }
    }

    pub(super) fn collapse_selected(&mut self) -> bool {
        match self.selected().cloned() {
            Some(Target::Path(path, true))
                if self.query.is_empty() && !self.tree_collapsed().contains(&path) =>
            {
                self.set_folder_expanded(path, false);
                return true;
            }
            Some(Target::Path(path, _)) => {
                if let Some(index) = path.ancestors().skip(1).find_map(|parent| {
                    self.entries
                        .iter()
                        .position(|entry| entry.target == Target::Path(parent.to_owned(), true))
                }) {
                    self.view.selected = index;
                    self.load_content(true);
                    return true;
                }
            }
            _ => {}
        }
        false
    }

    pub(super) fn mouse(
        &mut self,
        event: MouseEvent,
        terminal: &mut Option<crate::session::Session>,
        signals: &mut Option<ui_terminal::SignalGuard>,
    ) -> Result<(), String> {
        if matches!(
            event.kind,
            MouseEventKind::ScrollDown
                | MouseEventKind::ScrollUp
                | MouseEventKind::ScrollLeft
                | MouseEventKind::ScrollRight
        ) {
            self.wheel(event);
            return Ok(());
        }
        if event.kind != MouseEventKind::Moved {
            self.scroll.stop();
        }
        let point = ratatui::layout::Position::new(event.column, event.row);
        if self.dialog.is_some() {
            if self.dragging || self.view.divider_hovered || self.view.nav_hovered.is_some() {
                self.dragging = false;
                self.view.nav_hovered = None;
                self.view.divider_hovered = false;
                self.view.divider_hover_row = None;
                self.dirty = true;
            }
            if event.kind == MouseEventKind::Down(MouseButton::Left)
                && let Some(geometry) = &self.geometry
            {
                let key = if geometry
                    .overlay_close
                    .is_some_and(|area| area.contains(point))
                {
                    Some(KeyCode::Esc)
                } else if matches!(self.dialog, Some(Dialog::Confirm { .. }))
                    && geometry
                        .overlay_confirm
                        .is_some_and(|area| area.contains(point))
                {
                    Some(KeyCode::Enter)
                } else {
                    None
                };
                if let Some(key) = key {
                    return self.key(KeyEvent::new(key, KeyModifiers::NONE), terminal, signals);
                }
            }
            return Ok(());
        }
        if event.kind == MouseEventKind::Moved {
            let hovered = self.dragging
                || self
                    .geometry
                    .as_ref()
                    .and_then(|geometry| geometry.divider_hit)
                    .is_some_and(|area| area.contains(point));
            let hover_row = hovered.then_some(event.row);
            let nav_hovered = self.geometry.as_ref().and_then(|geometry| {
                if hovered || !geometry.nav.contains(point) {
                    return None;
                }
                let index = geometry.nav_start + usize::from(event.row - geometry.nav.y);
                self.entries
                    .get(index)
                    .filter(|entry| !entry.item.disabled)
                    .map(|_| index)
            });
            if self.view.divider_hovered != hovered
                || self.view.divider_hover_row != hover_row
                || self.view.nav_hovered != nav_hovered
            {
                self.view.divider_hovered = hovered;
                self.view.divider_hover_row = hover_row;
                self.view.nav_hovered = nav_hovered;
                self.dirty = true;
            }
            return Ok(());
        }
        let Some(geometry) = self.geometry.clone() else {
            return Ok(());
        };
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.view.nav_hovered = None;
                if let Some((workspace, _)) =
                    geometry.tabs.iter().find(|(_, area)| area.contains(point))
                {
                    self.workspace(*workspace);
                } else if let Some((side, _)) = geometry
                    .diff_sides
                    .iter()
                    .find(|(_, area)| area.contains(point))
                {
                    self.select_diff_side(*side);
                } else if geometry
                    .divider_hit
                    .is_some_and(|area| area.contains(point))
                {
                    self.dragging = true;
                    self.view.divider_hovered = true;
                    self.view.divider_hover_row = Some(event.row);
                    self.drag_offset = i16::try_from(
                        i32::from(event.column)
                            - i32::from(geometry.divider_x.unwrap_or(event.column)),
                    )
                    .unwrap_or(0);
                } else if let Some((field, _)) = geometry
                    .commit_fields
                    .iter()
                    .find(|(_, area)| area.contains(point))
                {
                    self.view.focus = Focus::CommitDetails;
                    self.view.commit_manual_scroll = false;
                    self.view.commit_field = *field;
                } else if geometry.commit_details.contains(point) {
                    self.view.focus = Focus::CommitDetails;
                } else if geometry.commit_subject.contains(point) {
                    self.inspect_commit();
                } else if geometry.nav.contains(point) {
                    let index = geometry.nav_start + usize::from(event.row - geometry.nav.y);
                    if self
                        .entries
                        .get(index)
                        .is_some_and(|entry| entry.item.disabled)
                    {
                        return Ok(());
                    }
                    self.view.focus = Focus::Navigator;
                    self.view.nav_manual_scroll = false;
                    if index < self.entries.len() {
                        let disclosure = geometry
                            .nav_disclosures
                            .iter()
                            .any(|(row, area)| *row == index && area.contains(point));
                        let was_selected = self.view.selected == index;
                        self.view.selected = index;
                        if (disclosure || was_selected)
                            && matches!(self.selected(), Some(Target::Path(_, true)))
                        {
                            self.toggle_folder();
                            if !was_selected {
                                self.load_content(true);
                            }
                        } else {
                            self.load_content(true);
                        }
                    }
                } else if geometry.canvas.contains(point) {
                    self.view.focus = Focus::Canvas;
                    self.view.canvas_manual_scroll = false;
                    self.view.diff_cursor =
                        self.view.diff_offset + usize::from(event.row - geometry.canvas.y);
                    self.clamp_cursor();
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                self.view.nav_width = (i32::from(event.column)
                    - i32::from(geometry.body.x)
                    - i32::from(self.drag_offset))
                .clamp(
                    22,
                    i32::from(geometry.body.width.saturating_sub(32).max(22)),
                ) as u16;
                self.view.divider_hovered = true;
                self.view.divider_hover_row = Some(event.row);
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if self.dragging && self.structure_enabled {
                    self.load_content(false);
                }
                self.dragging = false;
                self.view.divider_hovered = geometry
                    .divider_hit
                    .is_some_and(|area| area.contains(point));
                self.view.divider_hover_row = self.view.divider_hovered.then_some(event.row);
            }
            _ => {}
        }
        self.dirty = true;
        Ok(())
    }

    pub(super) fn paste(&mut self, text: &str) {
        if let Some(Dialog::Compose { editor, .. }) = &mut self.dialog {
            editor.paste(text);
        } else if let Some(Dialog::Input { editor, .. }) = &mut self.dialog {
            editor::paste_single_line(editor, text);
        } else if let Some(Dialog::Actions { editor, selected }) = &mut self.dialog {
            editor::paste_single_line(editor, text);
            *selected = 0;
        }
        if let Some(Dialog::Input {
            prompt: Prompt::Search,
            editor,
            ..
        }) = &self.dialog
            && self.query != editor.as_str()
        {
            self.query = editor.as_str().to_owned();
            self.rebuild();
            self.load_content(true);
        }
        self.dirty = true;
    }
}
