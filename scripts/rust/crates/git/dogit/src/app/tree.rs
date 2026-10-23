use super::*;

pub(super) struct FolderAnimation {
    path: PathBuf,
    started: Instant,
}

impl App {
    pub(super) fn set_folder_expanded(&mut self, path: PathBuf, expanded: bool) {
        if !self.query.is_empty() {
            return;
        }
        let changed = if expanded {
            self.tree_collapsed_mut().remove(&path)
        } else {
            self.tree_collapsed_mut().insert(path.clone())
        };
        if !changed {
            return;
        }
        let now = Instant::now();
        let reverse = self
            .folder_animation
            .as_ref()
            .filter(|animation| animation.path == path)
            .map(|animation| now.saturating_duration_since(animation.started))
            .filter(|elapsed| *elapsed < view::FolderAnimation::DURATION);
        let started = reverse
            .and_then(|elapsed| now.checked_sub(view::FolderAnimation::DURATION - elapsed))
            .unwrap_or(now);
        self.folder_animation = Some(FolderAnimation { path, started });
        self.rebuild();
    }

    pub(super) fn animate_tree(&mut self, now: Instant, reduced_motion: bool) -> bool {
        let frame = self.folder_animation.as_ref().and_then(|animation| {
            let elapsed = now.saturating_duration_since(animation.started);
            if reduced_motion || elapsed >= view::FolderAnimation::DURATION {
                return None;
            }
            self.entries
                .iter()
                .position(|entry| matches!(&entry.target, Target::Path(path, true) if path == &animation.path))
                .map(|row| view::FolderAnimation { row, elapsed })
        });
        if frame.is_none() && self.folder_animation.take().is_some() {
            self.dirty = true;
        }
        self.view.folder_animation = frame;
        frame.is_some()
    }

    pub(super) fn changes_entries(&self) -> (Vec<Entry>, view::NavSummary) {
        let statuses: BTreeMap<_, _> = self
            .snapshot
            .status
            .entries
            .iter()
            .map(|entry| (entry.path.as_path(), entry))
            .collect();
        let mut paths: BTreeSet<_> = statuses.keys().copied().collect();
        if self.all_files {
            paths.extend(self.files.iter().map(PathBuf::as_path));
        }
        let paths = paths.into_iter().filter_map(|path| {
            let status = statuses.get(path);
            let state = view::FileState {
                staged: status.is_some_and(|entry| entry.staged() && !entry.unmerged),
                unstaged: status.is_some_and(|entry| entry.unstaged() || entry.unmerged),
                conflicted: status.is_some_and(|entry| entry.unmerged),
                deleted: status.is_some_and(|entry| {
                    !entry.unmerged && (entry.index == 'D' || entry.worktree == 'D')
                }),
                staged_deletion: status.is_some_and(|entry| !entry.unmerged && entry.index == 'D'),
                ..view::FileState::default()
            };
            (!self.changes_filter.is_some_and(|side| match side {
                DiffSide::Index => !state.staged,
                DiffSide::Worktree => !state.unstaged,
            }))
            .then_some((path, state))
        });
        let (mut entries, summary) = self.file_tree_entries(paths);
        let deleted_directories = deleted_directories(&statuses, &self.files);
        for entry in &mut entries {
            if let Target::Path(path, true) = &entry.target
                && let Some(staged) = deleted_directories.get(path)
                && let Some(state) = &mut entry.item.file_state
            {
                state.deleted = true;
                state.staged_deletion = *staged;
            }
        }
        (entries, summary)
    }

    pub(super) fn history_entries(&self) -> (Vec<Entry>, view::NavSummary) {
        self.file_tree_entries(
            self.revision_paths
                .iter()
                .map(|path| (path.as_path(), view::FileState::default())),
        )
    }

    pub(super) fn file_tree_entries<'a>(
        &self,
        paths: impl IntoIterator<Item = (&'a Path, view::FileState)>,
    ) -> (Vec<Entry>, view::NavSummary) {
        let query = self.query.to_lowercase();
        let paths = paths.into_iter().filter(|(path, _)| {
            query.is_empty() || path.to_string_lossy().to_lowercase().contains(&query)
        });
        let (tree, summary) = view::file_tree(&self.repo.root, paths, |path| {
            !query.is_empty() || !self.tree_collapsed().contains(path)
        });
        if self.view.workspace == Workspace::Changes
            && self.changes_filter.is_some()
            && summary.items == 0
        {
            return (Vec::new(), summary);
        }
        let disabled = self.view.workspace == Workspace::Changes && summary.items == 0;
        let entries = tree
            .into_iter()
            .map(|(path, mut item)| {
                item.disabled = disabled;
                if disabled && let Some(state) = &mut item.file_state {
                    state.expanded = true;
                }
                Entry {
                    target: Target::Path(path, item.kind == NavKind::Folder),
                    item,
                }
            })
            .collect();
        (entries, summary)
    }

    pub(super) fn selected_file_state(&self) -> Option<view::FileState> {
        self.entries
            .get(self.view.selected)
            .filter(|entry| !entry.item.disabled)
            .and_then(|entry| entry.item.file_state)
    }

    pub(super) fn align_diff_side(&mut self) {
        if self.view.workspace != Workspace::Changes {
            return;
        }
        if let Some(side) = self.changes_filter {
            self.side = side;
            return;
        }
        if let Some(state) = self.selected_file_state() {
            if state.unstaged && !state.staged {
                self.side = DiffSide::Worktree;
            } else if state.staged && !state.unstaged {
                self.side = DiffSide::Index;
            }
        }
    }

    pub(super) fn select_diff_side(&mut self, side: DiffSide) {
        if self.view.workspace != Workspace::Changes {
            return;
        }
        if self.changes_filter.is_some() && self.changes_filter != Some(side) {
            self.set_changes_filter(Some(side));
            return;
        }
        let available = self.selected_file_state().is_some_and(|state| match side {
            DiffSide::Worktree => state.unstaged || state.conflicted,
            DiffSide::Index => state.staged,
        });
        if available && self.side != side {
            self.side = side;
            self.load_content(true);
        }
    }

    pub(super) fn toggle_changes_filter(&mut self, side: DiffSide) {
        if self.view.workspace == Workspace::Changes {
            self.set_changes_filter((self.changes_filter != Some(side)).then_some(side));
        }
    }

    fn set_changes_filter(&mut self, filter: Option<DiffSide>) {
        self.changes_filter = filter;
        self.view.nav_manual_scroll = false;
        self.view.nav_horizontal = 0;
        self.rebuild();
        self.load_content(true);
    }
}

fn deleted_directories(
    statuses: &BTreeMap<&Path, &crate::git::StatusEntry>,
    files: &[PathBuf],
) -> BTreeMap<PathBuf, bool> {
    let deleted: BTreeMap<_, _> = statuses
        .iter()
        .filter(|(_, entry)| !entry.unmerged && (entry.index == 'D' || entry.worktree == 'D'))
        .map(|(path, entry)| (*path, entry.index == 'D'))
        .collect();
    let mut directories = BTreeMap::new();
    for (path, staged) in &deleted {
        for parent in path
            .ancestors()
            .skip(1)
            .filter(|path| !path.as_os_str().is_empty())
        {
            *directories.entry(parent.to_owned()).or_insert(true) &= staged;
        }
    }
    if directories.is_empty() {
        return directories;
    }
    for path in files
        .iter()
        .map(PathBuf::as_path)
        .chain(statuses.keys().copied())
    {
        if !deleted.contains_key(path) {
            for parent in path.ancestors().skip(1) {
                directories.remove(parent);
            }
        }
    }
    directories
}
