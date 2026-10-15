use super::*;

impl App {
    pub(super) fn available_actions(
        &self,
        query: &str,
    ) -> Vec<&'static crate::actions::Definition> {
        ACTIONS
            .iter()
            .filter(|definition| {
                let available = match definition.action {
                    Action::Stage
                    | Action::Unstage
                    | Action::StageFile
                    | Action::UnstageFile
                    | Action::Discard
                    | Action::ToggleFiles
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
        self.dirty = true;
        if key.code == KeyCode::Char('h') && key.modifiers.contains(KeyModifiers::CONTROL) {
            key.code = KeyCode::Backspace;
        }
        if self.dialog.is_some() {
            return self.dialog_key(key, terminal, signals);
        }
        if let KeyCode::Char(character) = key.code {
            if character.is_ascii_digit() && !(key.modifiers - KeyModifiers::SHIFT).is_empty() {
                return Ok(());
            }
            if (key.modifiers - KeyModifiers::SHIFT).is_empty() && ('1'..='7').contains(&character)
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
                    self.dialog = Some(Dialog::Actions {
                        query: String::new(),
                        selected: 0,
                    });
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
                self.dialog = Some(Dialog::Actions {
                    query: String::new(),
                    selected: 0,
                })
            }
            KeyCode::Char('/') => {
                self.dialog = Some(Dialog::Input {
                    prompt: Prompt::Search,
                    title: "Search".into(),
                    value: self.query.clone(),
                })
            }
            KeyCode::Tab | KeyCode::BackTab if (key.modifiers - KeyModifiers::SHIFT).is_empty() => {
                self.view.focus = match self.view.focus {
                    Focus::Navigator => Focus::Canvas,
                    Focus::Canvas => Focus::Navigator,
                };
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
                if self.view.focus == Focus::Canvas {
                    self.scroll_canvas_horizontal(-8);
                } else {
                    self.collapse_selected();
                }
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if self.view.focus == Focus::Canvas {
                    self.scroll_canvas_horizontal(8);
                } else if matches!(self.selected(), Some(Target::Path(_, true))) {
                    self.expand_selected();
                } else {
                    self.view.focus = Focus::Canvas;
                }
            }
            KeyCode::Enter => return self.enter(terminal, signals),
            KeyCode::Char(' ') => {
                if self.view.focus == Focus::Canvas {
                    self.mark_current(false);
                } else if matches!(self.selected(), Some(Target::Commit(_))) {
                    return self.action(Action::PinCommit, terminal, signals);
                } else if matches!(self.selected(), Some(Target::Path(_, true))) {
                    self.toggle_folder();
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
            KeyCode::Char(character) => {
                let action = match character {
                    'e' => Some(Action::Edit),
                    's' => Some(if self.view.workspace == Workspace::Stashes {
                        Action::StashApply
                    } else {
                        Action::Stage
                    }),
                    'u' => Some(Action::Unstage),
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

    fn scroll_canvas_horizontal(&mut self, amount: isize) {
        let maximum = self.geometry.as_ref().and_then(|geometry| {
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
        });
        let offset = self.view.horizontal.saturating_add_signed(amount);
        self.view.horizontal = maximum.map_or(offset, |maximum| offset.min(maximum));
    }

    pub(super) fn move_cursor(&mut self, amount: isize, select: bool) {
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
            self.view.diff_cursor = if end {
                self.row_map
                    .len()
                    .max(self.content_line_count)
                    .saturating_sub(1)
            } else {
                0
            };
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

    pub(super) fn jump_hunk(&mut self, forward: bool) {
        let rows = &self.row_map;
        let cursor = self.view.diff_cursor;
        let index = if forward {
            rows.iter()
                .enumerate()
                .find(|(index, row)| {
                    *index > cursor && row.line.is_none() && row.right_line.is_none()
                })
                .map(|(index, _)| index)
        } else {
            rows.iter()
                .enumerate()
                .rev()
                .find(|(index, row)| {
                    *index < cursor && row.line.is_none() && row.right_line.is_none()
                })
                .map(|(index, _)| index)
        };
        if let Some(index) = index {
            self.view.diff_cursor = index;
            self.view.focus = Focus::Canvas;
            self.follow_canvas_cursor();
        }
    }

    pub(super) fn toggle_folder(&mut self) {
        if let Some(Target::Path(path, true)) = self.selected().cloned() {
            if !self.collapsed.remove(&path) {
                self.collapsed.insert(path);
            }
            self.rebuild();
        }
    }

    pub(super) fn expand_selected(&mut self) {
        let Some(Target::Path(path, true)) = self.selected().cloned() else {
            return;
        };
        self.view.nav_manual_scroll = false;
        if self.query.is_empty() && self.collapsed.remove(&path) {
            self.rebuild();
        } else if self.entries.get(self.view.selected + 1).is_some_and(
            |entry| matches!(&entry.target, Target::Path(child, _) if child.starts_with(&path)),
        ) {
            self.view.selected += 1;
            self.load_content(true);
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

    pub(super) fn collapse_selected(&mut self) {
        match self.selected().cloned() {
            Some(Target::Path(path, true))
                if self.query.is_empty() && !self.collapsed.contains(&path) =>
            {
                self.collapsed.insert(path);
                self.rebuild();
            }
            Some(Target::Path(path, _)) => {
                if let Some(parent) = path.parent()
                    && let Some(index) = self
                        .entries
                        .iter()
                        .position(|entry| entry.target == Target::Path(parent.to_owned(), true))
                {
                    self.view.selected = index;
                    self.load_content(true);
                }
            }
            _ => {}
        }
    }

    pub(super) fn mouse(&mut self, event: MouseEvent) {
        if self.dialog.is_some() {
            if self.dragging || self.view.divider_hovered {
                self.dragging = false;
                self.view.divider_hovered = false;
                self.view.divider_hover_row = None;
                self.dirty = true;
            }
            return;
        }
        let point = ratatui::layout::Position::new(event.column, event.row);
        if event.kind == MouseEventKind::Moved {
            let hovered = self.dragging
                || self
                    .geometry
                    .as_ref()
                    .and_then(|geometry| geometry.divider_hit)
                    .is_some_and(|area| area.contains(point));
            let hover_row = hovered.then_some(event.row);
            if self.view.divider_hovered != hovered || self.view.divider_hover_row != hover_row {
                self.view.divider_hovered = hovered;
                self.view.divider_hover_row = hover_row;
                self.dirty = true;
            }
            return;
        }
        if matches!(
            event.kind,
            MouseEventKind::ScrollDown
                | MouseEventKind::ScrollUp
                | MouseEventKind::ScrollLeft
                | MouseEventKind::ScrollRight
        ) {
            let Some(geometry) = &self.geometry else {
                return;
            };
            let amount = if matches!(
                event.kind,
                MouseEventKind::ScrollDown | MouseEventKind::ScrollRight
            ) {
                1
            } else {
                -1
            };
            let horizontal = matches!(
                event.kind,
                MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight
            );
            if geometry.nav.contains(point) {
                if horizontal {
                    let maximum = view::nav_horizontal_limit(
                        &self.nav_items,
                        geometry.nav.width,
                        self.view.workspace,
                    );
                    self.view.nav_horizontal = self
                        .view
                        .nav_horizontal
                        .saturating_add_signed(amount)
                        .min(maximum);
                } else {
                    let maximum = self
                        .entries
                        .len()
                        .saturating_sub(usize::from(geometry.nav.height));
                    self.view.offset = self.view.offset.saturating_add_signed(amount).min(maximum);
                    self.view.nav_manual_scroll = true;
                }
            } else if geometry.canvas.contains(point) {
                if horizontal {
                    self.scroll_canvas_horizontal(amount);
                } else {
                    let maximum = self
                        .row_map
                        .len()
                        .max(self.content_line_count)
                        .saturating_sub(usize::from(geometry.canvas.height));
                    self.view.diff_offset = self
                        .view
                        .diff_offset
                        .saturating_add_signed(amount)
                        .min(maximum);
                    self.view.canvas_manual_scroll = true;
                }
            } else {
                return;
            }
            self.dirty = true;
            return;
        }
        let Some(geometry) = self.geometry.clone() else {
            return;
        };
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
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
                } else if geometry.nav.contains(point) {
                    self.view.focus = Focus::Navigator;
                    self.view.nav_manual_scroll = false;
                    let index = geometry.nav_start + usize::from(event.row - geometry.nav.y);
                    if index < self.entries.len() {
                        if self.view.selected == index
                            && matches!(self.selected(), Some(Target::Path(_, true)))
                        {
                            self.toggle_folder();
                        } else {
                            self.view.selected = index;
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
    }

    pub(super) fn paste(&mut self, text: &str) {
        if let Some(Dialog::Input { value, prompt, .. }) = &mut self.dialog {
            value.extend(text.chars().filter(|character| {
                !character.is_control() || (matches!(prompt, Prompt::Compose) && *character == '\n')
            }));
        } else if let Some(Dialog::Actions { query, selected }) = &mut self.dialog {
            query.extend(text.chars().filter(|character| !character.is_control()));
            *selected = 0;
        }
        if let Some(Dialog::Input {
            prompt: Prompt::Search,
            value,
            ..
        }) = &self.dialog
        {
            self.query = value.clone();
            self.rebuild();
            self.load_content(true);
        }
        self.dirty = true;
    }
}
