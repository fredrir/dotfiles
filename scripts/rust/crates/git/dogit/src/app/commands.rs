use super::*;

impl App {
    pub(super) fn prepare(
        &mut self,
        label: &str,
        work: impl FnOnce(Repo) -> Result<Prepared, String> + Send + 'static,
    ) {
        if self.operation.is_some() {
            self.note("Finish the current operation first");
            return;
        }
        self.snapshot_worker.invalidate();
        self.snapshot_pending = false;
        self.content_worker.invalidate();
        self.ready_content = None;
        self.content_pending = false;
        self.notification = None;
        let repo = self.repo.clone();
        self.prepare_worker.submit(move |_| work(repo));
        self.operation = Some(PendingOperation {
            label: label.into(),
            started: Instant::now(),
        });
        self.dirty = true;
    }

    pub(super) fn command(
        &mut self,
        label: &str,
        work: impl FnOnce(Repo) -> Result<CommandSpec, crate::git::GitError> + Send + 'static,
    ) {
        self.prepare(label, move |repo| {
            work(repo)
                .map(|command| Prepared::Commands(vec![command]))
                .map_err(|error| error.to_string())
        });
    }

    pub(super) fn paths(&self, target: Option<&Target>, staged: bool) -> Vec<PathBuf> {
        let Some(Target::Path(path, directory)) = target else {
            return Vec::new();
        };
        self.snapshot
            .status
            .entries
            .iter()
            .filter(|entry| {
                (entry.path == *path || (*directory && entry.path.starts_with(path)))
                    && self.matches(&entry.path.to_string_lossy())
                    && if staged {
                        entry.staged()
                    } else {
                        entry.unstaged() || entry.unmerged
                    }
            })
            .map(|entry| entry.path.clone())
            .collect()
    }

    pub(super) fn action(
        &mut self,
        action: Action,
        terminal: &mut Option<crate::session::Session>,
        signals: &mut Option<ui_terminal::SignalGuard>,
    ) -> Result<(), String> {
        if matches!(action, Action::Commit | Action::Compose) && self.staged_count == 0 {
            return Ok(());
        }
        if self.view.focus == Focus::Canvas
            && (self.content_pending || self.preview_target() != self.selected())
            && matches!(
                action,
                Action::Stage
                    | Action::Unstage
                    | Action::StageFile
                    | Action::UnstageFile
                    | Action::ToggleStage
                    | Action::Discard
                    | Action::Edit
            )
        {
            return Ok(());
        }
        let target = self.selected().cloned();
        match action {
            Action::Inspect => self.inspect_commit(),
            Action::Changes => self.workspace(Workspace::Changes), Action::History => self.workspace(Workspace::History),
            Action::Branches => self.workspace(Workspace::Branches), Action::Stashes => self.workspace(Workspace::Stashes),
            Action::Worktrees => self.workspace(Workspace::Worktrees),
            Action::Compare => {
                let value = self.compare.as_ref().map_or_else(|| "HEAD~1".into(), |(from, _)| from.clone());
                self.prompt(Prompt::CompareFrom, "Compare from revision", &value);
            }
            Action::Edit if self.view.workspace == Workspace::Changes => {
                let path = match target { Some(Target::Path(path, _)) => self.repo.root.join(path), _ => return Ok(()) };
                self.prepare("Open editor", move |repo| {
                    let mut command = repo.command(&CommandSpec::new("Editor", ["var", "GIT_EDITOR"]));
                    let output = hostkit::process::output(&mut command, hostkit::process::CaptureLimits::default(), Duration::from_secs(5)).map_err(|error| error.to_string())?;
                    if !output.status.success() { return Err(String::from_utf8_lossy(&output.stderr).into_owned()); }
                    let editor = String::from_utf8(output.stdout).map_err(|error| error.to_string())?.trim().to_owned();
                    if editor.is_empty() { return Err("No editor configured".into()); }
                    Ok(Prepared::Editor(editor, path))
                });
            }
            Action::ToggleStageAll if self.view.workspace == Workspace::Changes => {
                self.prepare("Toggle all staging", |repo| {
                    let status = repo.status().map_err(|error| error.to_string())?;
                    let stage = status.entries.iter().any(|entry| entry.unstaged() || entry.unmerged);
                    let paths: Vec<_> = status.entries.iter().filter(|entry| {
                        if stage { entry.unstaged() || entry.unmerged } else { entry.staged() }
                    }).map(|entry| entry.path.clone()).collect();
                    let commands = if paths.is_empty() { Vec::new() } else {
                        vec![if stage { repo.stage(&paths) } else { repo.unstage(&paths) }.map_err(|error| error.to_string())?]
                    };
                    Ok(Prepared::Commands(commands))
                });
            }
            Action::ToggleStage if self.view.workspace == Workspace::Changes => {
                if let Some(state) = self.selected_file_state() {
                    let action = if state.unstaged { Action::StageFile } else { Action::UnstageFile };
                    return self.action(action, terminal, signals);
                }
            }
            Action::Stage | Action::Unstage | Action::StageFile | Action::UnstageFile if self.view.workspace == Workspace::Changes => {
                if self.operation.is_some() { self.note("Finish the current operation first"); return Ok(()); }
                let unstage = matches!(action, Action::Unstage | Action::UnstageFile);
                let whole = matches!(action, Action::StageFile | Action::UnstageFile) || self.view.focus == Focus::Navigator;
                if !whole {
                    if self.structural.is_some() { self.structure_enabled = false; self.load_content(false); self.note("Exact diff opened for staging"); return Ok(()); }
                    let side = if unstage { DiffSide::Index } else { DiffSide::Worktree };
                    if self.side != side {
                        self.select_diff_side(side); return Ok(());
                    }
                    if self.content_pending { return Ok(()); }
                    let Some(diff) = self.diff.clone() else { return Ok(()); };
                    if diff.side != side { self.load_content(true); return Ok(()); }
                    let selection = if self.marked.is_empty() {
                        let Some(row) = self.row_map.get(self.view.diff_cursor) else { return Ok(()); };
                        Selection::Hunk(row.hunk)
                    } else { Selection::Lines(self.marked.iter().copied().collect()) };
                    self.notification = None;
                    self.operation = Some(PendingOperation { label: if unstage { "Unstage selection" } else { "Stage selection" }.into(), started: Instant::now() });
                    self.mutation_worker.submit(move |_| { diff.apply(&selection)?; Ok(if unstage { "Selection unstaged" } else { "Selection staged" }.into()) });
                } else {
                    let paths = self.paths(target.as_ref(), unstage);
                    if paths.is_empty() { return Ok(()); }
                    self.command(if unstage { "Unstage files" } else { "Stage files" }, move |repo| if unstage { repo.unstage(&paths) } else { repo.stage(&paths) });
                }
            }
            Action::Compose => {
                self.workspace(Workspace::Changes);
                self.dialog = Some(Dialog::Compose { editor: ui_widgets::EditorBuffer::default(), amend: None });
                self.view.editor_offset = 0;
                self.side = DiffSide::Index;
                self.view.focus = Focus::Canvas;
                self.load_content(true);
            },
            Action::Commit => self.command("Commit", |repo| repo.commit(None, false)),
            Action::Amend => self.dialog = Some(Dialog::Confirm { action, target: self.snapshot.status.oid.clone().map(Target::Commit), title: "Amend last commit".into(), detail: "Replace the last commit with the staged changes and an edited message. Published history will change.".into(), discard: None, abort: None, scroll: 0 }),
            Action::Fetch => self.command("Fetch", |repo| Ok(repo.fetch())),
            Action::Pull => self.command("Pull", |repo| Ok(repo.pull())),
            Action::Push => self.command("Push", |repo| Ok(repo.push())),
            Action::Switch => match target {
                Some(Target::Branch(branch, remote)) => self.command("Switch branch", move |repo| if remote { repo.switch_remote(&branch) } else { repo.switch(&branch) }),
                Some(Target::Worktree(path)) => {
                    if self.operation.is_some() { self.note("Finish the current operation first"); return Ok(()); }
                    self.prepare("Open worktree", move |_| Repo::open(path).map(Prepared::Worktree).map_err(|error| error.to_string()));
                }
                _ => {},
            },
            Action::CreateBranch => self.prompt(Prompt::Branch, "Create branch", ""),
            Action::StashSave => self.prompt(Prompt::Stash, "Stash tracked and untracked changes", ""),
            Action::StashApply | Action::StashPop => self.confirmed(action, target, None),
            Action::StashDrop if matches!(target, Some(Target::Stash(..))) => {
                let detail = match &target {
                    Some(Target::Stash(reference, oid)) => format!("Permanently remove {reference} ({}) from this repository.", short(oid)),
                    _ => unreachable!(),
                };
                self.dialog = Some(Dialog::Confirm { action, target, title: "Drop selected stash".into(), detail, discard: None, abort: None, scroll: 0 });
            },
            Action::CherryPick | Action::Revert if matches!(target, Some(Target::Commit(_))) => {
                if let Some(Target::Commit(oid)) = &target
                    && self.snapshot.commits.iter().find(|commit| commit.oid == *oid).is_some_and(|commit| commit.parents.len() > 1) {
                    self.prompt(Prompt::Mainline(action, oid.clone()), "Merge commit: choose mainline parent number", "1");
                    return Ok(());
                }
                let label = if action == Action::CherryPick { "Cherry-pick selected commit" } else { "Revert selected commit" };
                let detail = match &target {
                    Some(Target::Commit(oid)) => format!("Apply commit {} to the current branch.", short(oid)),
                    _ => unreachable!(),
                };
                self.dialog = Some(Dialog::Confirm { action, target, title: label.into(), detail, discard: None, abort: None, scroll: 0 });
            }
            Action::Merge | Action::Rebase => {
                let value = match &target { Some(Target::Branch(name, true)) => format!("refs/remotes/{name}"), Some(Target::Branch(name, false)) => format!("refs/heads/{name}"), Some(Target::Commit(name)) => name.clone(), _ => String::new() };
                self.prompt(if action == Action::Merge { Prompt::Merge } else { Prompt::Rebase }, if action == Action::Merge { "Merge revision into current branch" } else { "Rebase current branch onto revision" }, &value);
            }
            Action::Continue if self.snapshot.operation.is_some() => self.command("Continue operation", |repo| repo.continue_operation()),
            Action::Abort if self.snapshot.operation.is_some() => self.prepare("Review operation", |repo| crate::git::OperationGuard::capture(&repo).map(|guard| Prepared::Abort(Box::new(guard))).map_err(|error| error.to_string())),
            Action::Discard if self.view.workspace == Workspace::Changes => {
                let paths = self.paths(target.as_ref(), false);
                if !paths.is_empty() { self.prepare("Review discard", move |repo| crate::git::DiscardGuard::capture(&repo, &paths).map(|guard| Prepared::Discard(Box::new(guard))).map_err(|error| error.to_string())); }
            }
            Action::ToggleFiles if self.view.workspace == Workspace::Changes => { self.all_files = !self.all_files; self.rebuild(); self.load_content(true); },
            Action::ToggleSide if self.view.workspace == Workspace::Changes => { self.select_diff_side(if self.side == DiffSide::Index { DiffSide::Worktree } else { DiffSide::Index }); },
            Action::ShowStaged => self.toggle_changes_filter(DiffSide::Index),
            Action::ShowUnstaged => self.toggle_changes_filter(DiffSide::Worktree),
            Action::ToggleStructural => { self.structure_enabled = !self.structure_enabled; self.load_content(true); },
            Action::ToggleSplit => { self.view.split = !self.view.split; self.split_override = Some(self.view.split); self.update_rows(); self.load_content(false); },
            Action::Copy => {
                let value = match target {
                    Some(Target::Commit(oid)) => Some(short(&oid).to_owned()),
                    Some(Target::Path(path, _)) | Some(Target::Worktree(path)) => Some(path.to_string_lossy().into_owned()),
                    Some(Target::Branch(name, _)) | Some(Target::Stash(name, _)) => Some(name),
                    _ => None,
                };
                if let Some(value) = value { self.copy_text(value); }
            }
            Action::Refresh => self.refresh(),
            Action::CompareSelected => if let Some(Target::Commit(oid)) = target { self.compare = Some(if let Some(base) = &self.compare_base { (base.clone(), oid) } else { (oid, "HEAD".into()) }); self.workspace(Workspace::Compare); },
            Action::PinCommit => if let Some(Target::Commit(oid)) = target { self.compare_base = if self.compare_base.as_ref() == Some(&oid) { None } else { Some(oid) }; self.rebuild(); },
            Action::FollowChild => if let Some(Target::Commit(oid)) = target
                && let Some(child) = self.snapshot.commits.iter().find(|commit| commit.parents.contains(&oid))
                && let Some(index) = self.entries.iter().position(|entry| entry.target == Target::Commit(child.oid.clone())) { self.view.selected = index; self.load_content(true); },
            Action::FollowParent => if let Some(Target::Commit(oid)) = target
                && let Some(parent) = self.snapshot.commits.iter().find(|commit| commit.oid == oid).and_then(|commit| commit.parents.first())
                && let Some(index) = self.entries.iter().position(|entry| entry.target == Target::Commit(parent.clone())) { self.view.selected = index; self.load_content(true); },
            Action::LoadMore => { self.history_limit = (self.history_limit + 500).min(20_000); self.refresh(); },
            _ => {},
        }
        let _ = (terminal, signals);
        Ok(())
    }

    pub(super) fn confirmed(
        &mut self,
        action: Action,
        target: Option<Target>,
        discard: Option<Box<crate::git::DiscardGuard>>,
    ) {
        match action {
            Action::Discard => {
                if let Some(guard) = discard {
                    self.prepare("Discard selected changes", move |repo| {
                        guard
                            .commands(&repo)
                            .map(Prepared::Commands)
                            .map_err(|error| error.to_string())
                    });
                }
            }
            Action::Amend => {
                let expected = match target {
                    Some(Target::Commit(oid)) => Some(oid),
                    _ => None,
                };
                self.prepare("Amend", move |repo| {
                    if repo.status().map_err(|error| error.to_string())?.oid != expected {
                        return Err("HEAD changed; review the commit before amending".into());
                    }
                    let oid = expected.ok_or("There is no commit to amend")?;
                    let message = repo
                        .commit_message(&oid)
                        .map_err(|error| error.to_string())?;
                    Ok(Prepared::Amend { oid, message })
                });
            }
            Action::Abort => self.command("Abort operation", |repo| repo.abort_operation()),
            Action::StashApply | Action::StashPop | Action::StashDrop => {
                if let Some(Target::Stash(reference, oid)) = target {
                    self.command(
                        if action == Action::StashDrop {
                            "Drop stash"
                        } else if action == Action::StashPop {
                            "Pop stash"
                        } else {
                            "Apply stash"
                        },
                        move |repo| {
                            repo.verify_stash(&reference, &oid)?;
                            if action == Action::StashDrop {
                                repo.stash_drop(&reference)
                            } else {
                                repo.stash_apply(&reference, action == Action::StashPop)
                            }
                        },
                    );
                }
            }
            Action::CherryPick | Action::Revert | Action::Rebase => {
                if let Some(Target::Commit(oid)) = target {
                    self.command(
                        if action == Action::CherryPick {
                            "Cherry-pick"
                        } else if action == Action::Revert {
                            "Revert"
                        } else {
                            "Rebase"
                        },
                        move |repo| match action {
                            Action::CherryPick => repo.cherry_pick(&oid),
                            Action::Revert => repo.revert(&oid),
                            _ => repo.rebase(&oid),
                        },
                    );
                }
            }
            _ => {}
        }
    }

    pub(super) fn prompt(&mut self, prompt: Prompt, title: &str, value: &str) {
        self.view.editor_offset = 0;
        self.dialog = Some(Dialog::Input {
            prompt,
            title: title.into(),
            editor: ui_widgets::EditorBuffer::new(value),
        });
    }

    pub(super) fn open_actions(&mut self) {
        self.view.editor_offset = 0;
        self.dialog = Some(Dialog::Actions {
            editor: ui_widgets::EditorBuffer::default(),
            selected: 0,
        });
    }

    pub(super) fn enter(
        &mut self,
        terminal: &mut Option<crate::session::Session>,
        signals: &mut Option<ui_terminal::SignalGuard>,
    ) -> Result<(), String> {
        if self.view.focus == Focus::CommitDetails {
            self.activate_commit_field();
            return Ok(());
        }
        match self.selected() {
            None if self.view.focus == Focus::Navigator => {}
            Some(Target::Commit(oid)) if self.view.focus == Focus::Navigator => {
                self.history_commit = Some(oid.clone());
                self.revision_paths.clear();
                self.history_collapsed.clear();
                self.view.commit_field = 0;
                self.view.commit_manual_scroll = false;
                self.view.selected = 0;
                self.view.offset = 0;
                self.query.clear();
                self.rebuild();
                self.load_content(true);
            }
            Some(Target::Path(_, true)) if self.view.focus == Focus::Navigator => {
                self.toggle_folder()
            }
            Some(Target::Branch(..) | Target::Worktree(_)) => {
                return self.action(Action::Switch, terminal, signals);
            }
            _ => self.view.focus = Focus::Canvas,
        }
        Ok(())
    }

    pub(super) fn dialog_key(
        &mut self,
        key: KeyEvent,
        terminal: &mut Option<crate::session::Session>,
        signals: &mut Option<ui_terminal::SignalGuard>,
    ) -> Result<(), String> {
        let Some(mut dialog) = self.dialog.take() else {
            return Ok(());
        };
        if key.code == KeyCode::Esc {
            if matches!(dialog, Dialog::Compose { .. }) {
                self.load_content(true);
            }
            return Ok(());
        }
        if let Some(editor) = dialog.editor()
            && editor::copy_key(key)
        {
            if key.kind != KeyEventKind::Repeat
                && let Some(selection) = editor.selection()
            {
                self.copy_text(editor.as_str()[selection].to_owned());
            }
            self.dialog = Some(dialog);
            return Ok(());
        }
        let submit = key.modifiers.is_empty() && key.code == KeyCode::Enter;
        if submit && key.kind == KeyEventKind::Repeat {
            self.dialog = Some(dialog);
            return Ok(());
        }
        let width = self
            .geometry
            .as_ref()
            .and_then(|geometry| geometry.editor)
            .map_or(72, |area| usize::from(area.width));
        match &mut dialog {
            Dialog::Error { editor, .. } => {
                if key.code == KeyCode::Enter {
                    return Ok(());
                }
                if key.code == KeyCode::Char('y') && key.modifiers.is_empty() {
                    self.copy_text(editor.as_str().to_owned());
                } else {
                    let height = self
                        .geometry
                        .as_ref()
                        .and_then(|geometry| geometry.editor)
                        .map_or(10, |area| usize::from(area.height));
                    editor::select_key(editor, key, width, height);
                }
            }
            Dialog::Inspect {
                message, scroll, ..
            } => {
                let outer = self
                    .geometry
                    .as_ref()
                    .map_or(ratatui::layout::Rect::new(0, 0, 100, 30), |geometry| {
                        geometry.overlay_bounds
                    });
                let maximum = view::inspection_scroll_max(message, outer);
                *scroll = match key.code {
                    KeyCode::Up | KeyCode::Char('k') => scroll.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => scroll.saturating_add(1).min(maximum),
                    KeyCode::PageUp => scroll.saturating_sub(10),
                    KeyCode::PageDown => scroll.saturating_add(10).min(maximum),
                    KeyCode::Home => 0,
                    KeyCode::End => maximum,
                    KeyCode::Char('y') => {
                        self.copy_text(message.clone());
                        *scroll
                    }
                    KeyCode::Char('i') | KeyCode::Char('q') | KeyCode::Enter => return Ok(()),
                    _ => *scroll,
                };
            }
            Dialog::Compose { editor, amend } => {
                if submit {
                    if editor.as_str().trim().is_empty()
                        || (amend.is_none() && self.staged_count == 0)
                    {
                        self.dialog = Some(dialog);
                        return Ok(());
                    }
                    if self.operation.is_some() {
                        self.note("Finish the current operation first");
                        self.dialog = Some(dialog);
                        return Ok(());
                    }
                    let value = editor.as_str().to_owned();
                    if let Some(expected) = amend.clone() {
                        self.prepare("Amend", move |repo| {
                            if repo
                                .status()
                                .map_err(|error| error.to_string())?
                                .oid
                                .as_ref()
                                != Some(&expected)
                            {
                                return Err(
                                    "HEAD changed; review the commit before amending".into()
                                );
                            }
                            repo.commit(Some(&value), true)
                                .map(|spec| Prepared::Commands(vec![spec]))
                                .map_err(|error| error.to_string())
                        });
                    } else {
                        self.command("Commit", move |repo| repo.commit(Some(&value), false));
                    }
                    return Ok(());
                }
                editor::edit_key(editor, key, width);
            }
            Dialog::Confirm { scroll, detail, .. }
                if matches!(
                    key.code,
                    KeyCode::Up
                        | KeyCode::Down
                        | KeyCode::PageUp
                        | KeyCode::PageDown
                        | KeyCode::Home
                        | KeyCode::End
                ) =>
            {
                let body = self
                    .geometry
                    .as_ref()
                    .map_or(ratatui::layout::Rect::new(1, 6, 78, 20), |geometry| {
                        geometry.overlay_bounds
                    });
                let maximum = view::confirmation_scroll_max(detail, body);
                *scroll = match key.code {
                    KeyCode::Up => scroll.saturating_sub(1),
                    KeyCode::Down => scroll.saturating_add(1).min(maximum),
                    KeyCode::PageUp => scroll.saturating_sub(8),
                    KeyCode::PageDown => scroll.saturating_add(8).min(maximum),
                    KeyCode::Home => 0,
                    KeyCode::End => maximum,
                    _ => *scroll,
                };
            }
            Dialog::Confirm {
                action,
                target,
                discard,
                abort,
                ..
            } if key.code == KeyCode::Enter => {
                if self.operation.is_some() {
                    self.note("Finish the current operation first");
                    self.dialog = Some(dialog);
                    return Ok(());
                }
                if let Some(guard) = abort.take() {
                    self.command("Abort operation", move |repo| guard.abort(&repo));
                } else {
                    self.confirmed(*action, target.take(), discard.take());
                }
                return Ok(());
            }
            Dialog::Actions { editor, selected } => {
                let actions = self.available_actions(editor.as_str());
                match key.code {
                    KeyCode::Enter if submit => {
                        if let Some(definition) = actions.get(*selected) {
                            self.action(definition.action, terminal, signals)?;
                        }
                        return Ok(());
                    }
                    KeyCode::Down if key.modifiers.is_empty() => {
                        *selected = if *selected >= actions.len().saturating_sub(1) {
                            0
                        } else {
                            *selected + 1
                        };
                    }
                    KeyCode::Up if key.modifiers.is_empty() => {
                        *selected = if *selected == 0 {
                            actions.len().saturating_sub(1)
                        } else {
                            *selected - 1
                        };
                    }
                    KeyCode::PageDown if key.modifiers.is_empty() => {
                        *selected = selected
                            .saturating_add(8)
                            .min(actions.len().saturating_sub(1))
                    }
                    KeyCode::PageUp if key.modifiers.is_empty() => {
                        *selected = selected.saturating_sub(8)
                    }
                    _ => {
                        let previous = editor.as_str().to_owned();
                        editor::edit_single_line_key(editor, key, width);
                        if editor.as_str() != previous {
                            *selected = 0;
                        }
                    }
                }
            }
            Dialog::Input { prompt, editor, .. } => {
                if submit {
                    if self.operation.is_some()
                        && !matches!(
                            prompt,
                            Prompt::Search | Prompt::CompareFrom | Prompt::CompareTo(_)
                        )
                    {
                        self.note("Finish the current operation first");
                        self.dialog = Some(dialog);
                        return Ok(());
                    }
                    let value = editor.as_str().to_owned();
                    match prompt.clone() {
                        Prompt::Search => {
                            self.query = value;
                            self.rebuild();
                            self.load_content(true);
                        }
                        Prompt::Branch => self.command("Create branch", move |repo| {
                            repo.create_branch(&value, None)
                        }),
                        Prompt::Stash => self.command("Save stash", move |repo| {
                            repo.stash_save((!value.is_empty()).then_some(value.as_str()), true)
                        }),
                        Prompt::Merge => self.command("Merge", move |repo| repo.merge(&value)),
                        Prompt::Rebase => {
                            self.dialog = Some(Dialog::Confirm {
                                action: Action::Rebase,
                                target: Some(Target::Commit(value.clone())),
                                title: "Rebase current branch".into(),
                                detail: format!(
                                    "Rewrite current branch commits onto {value}. Published commit identities may change."
                                ),
                                discard: None,
                                abort: None,
                                scroll: 0,
                            })
                        }
                        Prompt::CompareFrom => {
                            self.prompt(Prompt::CompareTo(value), "Compare to revision", "HEAD")
                        }
                        Prompt::CompareTo(from) => {
                            self.compare = Some((from, value));
                            self.workspace(Workspace::Compare);
                        }
                        Prompt::Mainline(action, oid) => {
                            if let Ok(parent) = value.parse::<usize>() {
                                self.command(
                                    if action == Action::CherryPick {
                                        "Cherry-pick merge"
                                    } else {
                                        "Revert merge"
                                    },
                                    move |repo| {
                                        if action == Action::CherryPick {
                                            repo.cherry_pick_mainline(&oid, parent)
                                        } else {
                                            repo.revert_mainline(&oid, parent)
                                        }
                                    },
                                );
                            } else {
                                self.dialog = Some(dialog);
                                return Ok(());
                            }
                        }
                    }
                    return Ok(());
                }
                editor::edit_single_line_key(editor, key, width);
                if matches!(prompt, Prompt::Search) && self.query != editor.as_str() {
                    self.query = editor.as_str().to_owned();
                    self.rebuild();
                    self.load_content(true);
                }
            }
            _ => {}
        }
        self.dialog = Some(dialog);
        Ok(())
    }
}
