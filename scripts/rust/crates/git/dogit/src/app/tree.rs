use super::*;

#[derive(Default)]
struct Node {
    directory: bool,
    state: view::FileState,
    children: Vec<PathBuf>,
}

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
                expanded: false,
            };
            (!self.changes_filter.is_some_and(|side| match side {
                DiffSide::Index => !state.staged,
                DiffSide::Worktree => !state.unstaged,
            }))
            .then_some((path, state))
        });
        self.file_tree_entries(paths)
    }

    pub(super) fn history_entries(&self) -> (Vec<Entry>, view::NavSummary) {
        self.file_tree_entries(
            self.revision_paths
                .iter()
                .map(|path| (path.as_path(), view::FileState::default())),
        )
    }

    fn file_tree_entries<'a>(
        &self,
        paths: impl IntoIterator<Item = (&'a Path, view::FileState)>,
    ) -> (Vec<Entry>, view::NavSummary) {
        let query = self.query.to_lowercase();
        let mut tree = BTreeMap::<PathBuf, Node>::new();
        let mut summary = view::NavSummary::default();
        let mut root: Option<PathBuf> = None;
        for (path, state) in paths {
            if !query.is_empty() && !path.to_string_lossy().to_lowercase().contains(&query) {
                continue;
            }
            let parent = path.parent().unwrap_or(Path::new(""));
            if let Some(root) = &mut root {
                while !parent.starts_with(&root) {
                    if !root.pop() {
                        break;
                    }
                }
            } else {
                root = Some(parent.to_owned());
            }
            summary.items += 1;
            summary.staged += usize::from(state.staged);
            summary.unstaged += usize::from(state.unstaged);
            merge_state(&mut tree.entry(path.to_owned()).or_default().state, state);
            for parent in path.ancestors().skip(1) {
                let node = tree.entry(parent.to_owned()).or_default();
                node.directory = true;
                merge_state(&mut node.state, state);
            }
        }
        if self.view.workspace == Workspace::Changes
            && self.changes_filter.is_some()
            && tree.is_empty()
        {
            return (Vec::new(), summary);
        }
        let root = root.unwrap_or_default();
        tree.entry(root.clone()).or_default().directory = true;
        let paths: Vec<_> = tree.keys().cloned().collect();
        for path in paths {
            if let Some(parent) = path.parent()
                && let Some(node) = tree.get_mut(parent)
            {
                node.children.push(path);
            }
        }
        let mut entries = Vec::new();
        let mut pending = vec![(root.clone(), root.clone(), 0)];
        while let Some((path, parent, depth)) = pending.pop() {
            let node = &tree[&path];
            let mut state = node.state;
            state.expanded =
                node.directory && (!query.is_empty() || !self.tree_collapsed().contains(&path));
            let label = if depth == 0 {
                if path.as_os_str().is_empty() {
                    self.repo
                        .root
                        .file_name()
                        .unwrap_or(self.repo.root.as_os_str())
                } else {
                    path.as_os_str()
                }
            } else {
                path.strip_prefix(&parent).unwrap_or(&path).as_os_str()
            };
            entries.push(Entry {
                item: NavItem {
                    label: label.to_string_lossy().into_owned(),
                    depth,
                    kind: if node.directory {
                        NavKind::Folder
                    } else {
                        NavKind::File
                    },
                    staged: state.staged,
                    file_state: Some(state),
                    ..Default::default()
                },
                target: Target::Path(path.clone(), node.directory),
            });
            if state.expanded {
                let mut children: Vec<_> = node.children.iter().collect();
                children.sort_by_key(|path| (!tree[*path].directory, *path));
                pending.extend(
                    children
                        .into_iter()
                        .rev()
                        .map(|child| (child.clone(), path.clone(), depth + 1)),
                );
            }
        }
        (entries, summary)
    }

    pub(super) fn selected_file_state(&self) -> Option<view::FileState> {
        self.entries
            .get(self.view.selected)
            .and_then(|entry| entry.item.file_state)
    }

    pub(super) fn align_diff_side(&mut self) {
        if self.view.workspace != Workspace::Changes || self.composing() {
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

fn merge_state(target: &mut view::FileState, source: view::FileState) {
    target.staged |= source.staged;
    target.unstaged |= source.unstaged;
    target.conflicted |= source.conflicted;
}

#[cfg(test)]
#[path = "../../tests/unit/tree.rs"]
mod tests;
