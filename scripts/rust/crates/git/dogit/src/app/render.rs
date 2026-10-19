use super::*;
use crossterm::SynchronizedUpdate;

impl App {
    pub(super) fn draw(&mut self, terminal: &mut crate::session::Session) -> Result<(), String> {
        self.view.overlay = self.dialog.as_ref().map(|dialog| match dialog {
            Dialog::Error { title, editor } => Overlay::Error {
                title: title.clone(),
                editor: editor.into(),
            },
            Dialog::Inspect {
                message, scroll, ..
            } => Overlay::Inspect {
                message: message.clone(),
                scroll: *scroll,
            },
            Dialog::Compose { editor, .. } => Overlay::Compose {
                editor: editor.into(),
            },
            Dialog::Input { title, editor, .. } => Overlay::Input {
                title: title.clone(),
                editor: editor.into(),
            },
            Dialog::Actions { editor, selected } => Overlay::Commands {
                editor: editor.into(),
                selected: *selected,
                items: self
                    .available_actions(editor.as_str())
                    .iter()
                    .map(|definition| nav(definition.label, definition.key, "", NavKind::File))
                    .collect(),
            },
            Dialog::Confirm {
                title,
                detail,
                scroll,
                ..
            } => Overlay::Confirm {
                scroll: *scroll,
                title: title.clone(),
                detail: detail.clone(),
                destructive: true,
            },
        });
        let size = terminal
            .terminal()
            .size()
            .map_err(|error| error.to_string())?;
        let width = view::canvas_width(
            ratatui::layout::Rect::new(0, 0, size.width, size.height),
            &self.view,
            self.history_commit.is_some(),
        );
        if self.row_width != width
            || self.row_split != (self.view.split && width >= 72)
            || self.row_config.as_ref() != Some(self.diff_config())
        {
            self.update_rows_for_width(width);
        }
        let repo = self
            .repo
            .root
            .file_name()
            .unwrap_or(self.repo.root.as_os_str())
            .to_string_lossy();
        let branch = if self.snapshot_loaded {
            self.snapshot.status.branch.as_deref().unwrap_or_else(|| {
                if self.snapshot.status.oid.is_some() {
                    "Detached HEAD"
                } else {
                    "New Repository"
                }
            })
        } else {
            ""
        };
        let composing = self.composing();
        let preview_target = self.preview_target().cloned();
        let canvas_path = match preview_target.as_ref() {
            _ if composing => String::new(),
            Some(Target::Path(path, _)) | Some(Target::Comparison(path)) => {
                path.display().to_string()
            }
            Some(Target::Worktree(_)) => String::new(),
            Some(Target::Commit(_)) if self.view.workspace == Workspace::History => String::new(),
            Some(Target::Commit(oid)) => self
                .snapshot
                .commits
                .iter()
                .find(|commit| commit.oid == *oid)
                .map_or_else(|| short(oid).into(), |commit| commit.subject.clone()),
            Some(Target::Stash(name, _)) => name.clone(),
            Some(Target::Branch(reference, _)) => reference.clone(),
            Some(Target::RevisionAll) | None if self.view.workspace == Workspace::Compare => {
                self.compare.as_ref().map_or_else(
                    || "Compare revisions".into(),
                    |(from, to)| format!("{from}  →  {to}"),
                )
            }
            Some(Target::RevisionAll) => self.history_commit.as_ref().map_or_else(
                || "All changes".into(),
                |oid| format!("{}  All changes", short(oid)),
            ),
            None => String::new(),
        };
        let diff_scope = (self.view.workspace == Workspace::Changes && !composing)
            .then(|| {
                self.entries
                    .get(self.view.selected)
                    .filter(|entry| Some(&entry.target) == preview_target.as_ref())
                    .or_else(|| {
                        self.entries
                            .iter()
                            .find(|entry| Some(&entry.target) == preview_target.as_ref())
                    })?
                    .item
                    .file_state
            })
            .flatten()
            .map(|state| view::DiffScope {
                side: self
                    .loaded_preview
                    .as_ref()
                    .map_or(self.side, |key| key.request.side),
                staged: state.staged,
                unstaged: state.unstaged,
            });
        let subtitle = if self.copy_ack.is_some() {
            "Copied".into()
        } else if self.structure_enabled && self.structural.is_some() {
            "Structural comparison".into()
        } else if self.view.workspace == Workspace::Changes {
            match self.changes_filter {
                Some(DiffSide::Index) => "Staged files · s show all".into(),
                Some(DiffSide::Worktree) => "Unstaged files · u show all".into(),
                None => String::new(),
            }
        } else if self.view.workspace == Workspace::History {
            String::new()
        } else if let Some(Target::Commit(oid)) = self.selected() {
            self.snapshot
                .commits
                .iter()
                .find(|commit| commit.oid == *oid)
                .map_or(String::new(), |commit| {
                    format!("{}  {}  {}", short(oid), commit.author, commit.decorations)
                })
        } else if self.view.workspace == Workspace::Worktrees {
            String::new()
        } else {
            self.entries
                .get(self.view.selected)
                .map_or(String::new(), |entry| entry.item.detail.clone())
        };
        let operation = if self.operation.is_some() {
            None
        } else if let Some((message, failed, _)) = &self.feedback {
            Some(view::OperationView {
                label: message.clone(),
                detail: String::new(),
                tone: if *failed {
                    view::Tone::Attention
                } else {
                    view::Tone::Normal
                },
            })
        } else {
            self.snapshot
                .operation
                .map(|operation| view::OperationView {
                    label: format!("{} in progress", operation),
                    detail: format!("{} unresolved files", self.unresolved_count),
                    tone: view::Tone::Attention,
                })
        };
        let reduced_motion = ui_terminal::reduced_motion_requested("DOGIT_REDUCED_MOTION");
        let toast = self
            .operation
            .as_ref()
            .map(|operation| operation.view(reduced_motion))
            .or_else(|| {
                self.notification
                    .as_ref()
                    .map(|notification| notification.view(reduced_motion))
            });
        let commit_oid = self.preview_commit().map(|details| details.oid.clone());
        let commit = commit_oid.as_deref().and_then(|oid| self.history.get(oid));
        let data = RenderData {
            commit,
            avatar: commit.and_then(|details| self.history.avatar_for(details)),
            inspecting_commit: self.history_commit.is_some(),
            repo: &repo,
            branch,
            ahead: self.snapshot.status.ahead,
            behind: self.snapshot.status.behind,
            items: &self.nav_items,
            nav_summary: Some(self.nav_summary),
            diff_counts: self.diff.as_ref().map(|_| self.diff_counts),
            canvas_path: &canvas_path,
            directory_scope: match preview_target.as_ref() {
                Some(Target::Path(path, true)) if !composing => Some(path.as_path()),
                _ => None,
            },
            diff_scope,
            canvas_subtitle: &subtitle,
            diff: self.diff.as_deref(),
            presentation: self.presentation.as_deref(),
            diff_rows: Some((&self.row_map, self.row_split, self.row_width)),
            diff_paint: self.row_paint.as_ref(),
            canvas_lines: &self.text,
            structural: self.structural.as_deref(),
            operation: operation.as_ref(),
            toast: toast.as_ref(),
            selected_lines: &self.marked,
        };
        let mut geometry = None;
        let previous_cursor = self.view.diff_cursor;
        let previous_offset = self.view.diff_offset;
        io::stdout()
            .sync_update(|_| {
                terminal
                    .terminal()
                    .draw(|frame| {
                        geometry = Some(view::render(frame, &mut self.view, &data));
                    })
                    .map(|_| ())
            })
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        self.geometry = geometry;
        let actual_width = self
            .geometry
            .as_ref()
            .map_or(0, |geometry| geometry.canvas.width);
        if self.row_width != actual_width {
            self.view.diff_cursor = previous_cursor;
            self.view.diff_offset = previous_offset;
            self.update_rows_for_width(actual_width);
            self.dirty = true;
        }
        Ok(())
    }
}
