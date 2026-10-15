use super::*;

impl App {
    pub(super) fn draw(&mut self, terminal: &mut crate::session::Session) -> Result<(), String> {
        self.view.overlay = self.dialog.as_ref().map(|dialog| match dialog {
            Dialog::Compose { editor } => Overlay::Compose {
                value: editor.as_str().into(),
                cursor: editor.cursor(),
                selection: editor.selection(),
            },
            Dialog::Input { title, value, .. } => Overlay::Input {
                title: title.clone(),
                value: value.clone(),
            },
            Dialog::Actions { query, selected } => Overlay::Commands {
                query: query.clone(),
                selected: *selected,
                items: self
                    .available_actions(query)
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
        let branch = self.snapshot.status.branch.as_deref().unwrap_or_else(|| {
            if self.snapshot.status.oid.is_some() {
                "Detached HEAD"
            } else if self.snapshot_pending {
                "Loading"
            } else {
                "New repository"
            }
        });
        let composing = self.composing();
        let canvas_path = match self.selected() {
            _ if composing => String::new(),
            Some(Target::Path(path, _)) | Some(Target::Comparison(path)) => {
                path.display().to_string()
            }
            Some(Target::Worktree(_)) => String::new(),
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
            .then(|| self.entries.get(self.view.selected)?.item.file_state)
            .flatten()
            .map(|state| view::DiffScope {
                side: self.side,
                staged: state.staged,
                unstaged: state.unstaged,
            });
        let subtitle = if self.copy_ack.is_some() {
            "Copied".into()
        } else if self.content_pending {
            "Loading".to_owned()
        } else if self.structure_enabled && self.structural.is_some() {
            "Structural comparison".into()
        } else if self.view.workspace == Workspace::Changes {
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
        let operation = if let Some(pending) = self.operation.as_ref() {
            Some(view::OperationView {
                label: pending.label.clone(),
                detail: "Running".into(),
                tone: view::Tone::Running,
                phase: if ui_terminal::reduced_motion_requested("DOGIT_REDUCED_MOTION") {
                    0
                } else {
                    pending.started.elapsed().as_millis() as usize / 80
                },
            })
        } else if let Some((message, failed, _)) = &self.feedback {
            Some(view::OperationView {
                label: message.clone(),
                detail: String::new(),
                tone: if *failed {
                    view::Tone::Attention
                } else {
                    view::Tone::Normal
                },
                phase: 0,
            })
        } else {
            self.snapshot
                .operation
                .map(|operation| view::OperationView {
                    label: format!("{} in progress", operation),
                    detail: format!("{} unresolved files", self.unresolved_count),
                    tone: view::Tone::Attention,
                    phase: 0,
                })
        };
        let toast = self.notification.as_ref().map(|notification| {
            notification.view(ui_terminal::reduced_motion_requested(
                "DOGIT_REDUCED_MOTION",
            ))
        });
        let data = RenderData {
            repo: &repo,
            branch,
            ahead: self.snapshot.status.ahead,
            behind: self.snapshot.status.behind,
            items: &self.nav_items,
            nav_summary: Some(self.nav_summary),
            diff_counts: Some(self.diff_counts),
            canvas_path: &canvas_path,
            directory_scope: match self
                .entries
                .get(self.view.selected)
                .map(|entry| &entry.target)
            {
                Some(Target::Path(path, true)) if !composing => Some(path.as_path()),
                _ => None,
            },
            diff_scope,
            canvas_subtitle: &subtitle,
            diff: self.diff.as_deref(),
            presentation: self.presentation.as_ref(),
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
        terminal
            .terminal()
            .draw(|frame| {
                geometry = Some(view::render(frame, &mut self.view, &data));
            })
            .map_err(|error| error.to_string())?;
        self.geometry = geometry;
        let actual_width = self
            .geometry
            .as_ref()
            .map_or(0, |geometry| geometry.canvas.width);
        if self.row_width != actual_width {
            // The terminal may resize between its size query and this frame.
            self.view.diff_cursor = previous_cursor;
            self.view.diff_offset = previous_offset;
            self.update_rows_for_width(actual_width);
            self.dirty = true;
        }
        Ok(())
    }
}
