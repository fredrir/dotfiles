use super::*;

impl App {
    pub(super) fn draw(&mut self, terminal: &mut crate::session::Session) -> Result<(), String> {
        self.view.overlay = self.dialog.as_ref().map(|dialog| match dialog {
            Dialog::Input {
                prompt: Prompt::Compose,
                value,
                ..
            } => {
                let (summary, body) = value.split_once('\n').unwrap_or((value, ""));
                Overlay::Compose {
                    summary: summary.into(),
                    body: body.into(),
                    body_started: value.contains('\n'),
                }
            }
            Dialog::Input {
                title,
                value,
                prompt,
            } => Overlay::Input {
                title: title.clone(),
                value: value.clone(),
                detail: if matches!(prompt, Prompt::Search) {
                    "Filter this workspace"
                } else {
                    "Enter to accept"
                }
                .into(),
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
        let title = if matches!(
            self.dialog,
            Some(Dialog::Input {
                prompt: Prompt::Compose,
                ..
            })
        ) {
            "Commit contents".into()
        } else {
            match self.selected() {
                Some(Target::Path(path, _)) | Some(Target::Comparison(path)) => {
                    path.display().to_string()
                }
                Some(Target::Worktree(_)) => "Details".into(),
                Some(Target::Commit(oid)) => self
                    .snapshot
                    .commits
                    .iter()
                    .find(|commit| commit.oid == *oid)
                    .map_or_else(|| short(oid).into(), |commit| commit.subject.clone()),
                Some(Target::Stash(..)) => "Patch".into(),
                Some(Target::Branch(reference, _)) => reference.clone(),
                Some(Target::Operation(index)) => self
                    .saved
                    .operations
                    .get(*index)
                    .map_or(String::new(), |operation| operation.action.clone()),
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
                None => match self.view.workspace {
                    Workspace::Changes
                    | Workspace::Stashes
                    | Workspace::History
                    | Workspace::Branches => "Patch",
                    Workspace::Worktrees => "Details",
                    Workspace::Operations => "Output",
                    Workspace::Compare => "Comparison",
                }
                .into(),
            }
        };
        let nav_title = if self.view.workspace == Workspace::Changes {
            if self.all_files {
                "Files"
            } else if self.side == DiffSide::Index {
                "Staged"
            } else {
                "Working"
            }
        } else {
            match self.view.workspace {
                Workspace::History if self.history_commit.is_some() => "Files",
                Workspace::History => "Commits",
                Workspace::Branches => "References",
                Workspace::Stashes => "Saved changes",
                Workspace::Worktrees => "Directories",
                Workspace::Compare => "Files",
                Workspace::Operations => "Actions",
                Workspace::Changes => unreachable!(),
            }
        };
        let subtitle = if self.copy_ack.is_some() {
            "Copied".into()
        } else if self.content_pending {
            "Loading".to_owned()
        } else if self.structure_enabled && self.structural.is_some() {
            "Structural comparison".into()
        } else if self.view.workspace == Workspace::Changes {
            format!(
                "{}  {} staged",
                if self.side == DiffSide::Index {
                    "Staged"
                } else {
                    "Working"
                },
                self.staged_count
            )
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
        let operation = if let Some(pending) = self
            .operation
            .as_ref()
            .filter(|pending| pending.started.elapsed() > Duration::from_millis(150))
        {
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
        let data = RenderData {
            repo: &repo,
            branch,
            ahead: self.snapshot.status.ahead,
            behind: self.snapshot.status.behind,
            nav_title,
            items: &self.nav_items,
            nav_summary: Some(self.nav_summary),
            diff_counts: Some(self.diff_counts),
            canvas_title: &title,
            canvas_subtitle: &subtitle,
            diff: self.diff.as_deref(),
            diff_rows: Some((&self.row_map, self.row_split)),
            canvas_lines: &self.text,
            structural: self.structural.as_deref(),
            operation: operation.as_ref(),
            selected_lines: &self.marked,
        };
        let mut geometry = None;
        terminal
            .terminal()
            .draw(|frame| {
                geometry = Some(view::render(frame, &mut self.view, &data));
            })
            .map_err(|error| error.to_string())?;
        self.geometry = geometry;
        if self.row_split
            != (self.view.split
                && self
                    .geometry
                    .as_ref()
                    .is_some_and(|geometry| geometry.canvas.width >= 72))
        {
            self.update_rows();
            self.dirty = true;
        }
        Ok(())
    }
}
