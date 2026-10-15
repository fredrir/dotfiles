use super::*;

#[derive(Default)]
struct Node {
    directory: bool,
    state: view::FileState,
    children: Vec<PathBuf>,
}

impl App {
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
        let query = self.query.to_lowercase();
        let mut tree = BTreeMap::<PathBuf, Node>::new();
        let mut summary = view::NavSummary::default();
        let mut root: Option<PathBuf> = None;
        for path in paths {
            if !query.is_empty() && !path.to_string_lossy().to_lowercase().contains(&query) {
                continue;
            }
            let status = statuses.get(path);
            let state = view::FileState {
                staged: status.is_some_and(|entry| entry.staged() && !entry.unmerged),
                unstaged: status.is_some_and(|entry| entry.unstaged() || entry.unmerged),
                conflicted: status.is_some_and(|entry| entry.unmerged),
                expanded: false,
            };
            if self.changes_filter.is_some_and(|side| match side {
                DiffSide::Index => !state.staged,
                DiffSide::Worktree => !state.unstaged,
            }) {
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
        if self.changes_filter.is_some() && tree.is_empty() {
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
        while let Some((mut path, parent, depth)) = pending.pop() {
            while let Some(node) = tree.get(&path) {
                if node.children.len() != 1 || !tree[&node.children[0]].directory {
                    break;
                }
                path = node.children[0].clone();
            }
            let node = &tree[&path];
            let mut state = node.state;
            state.expanded =
                node.directory && (!query.is_empty() || !self.collapsed.contains(&path));
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
                pending.extend(
                    node.children
                        .iter()
                        .rev()
                        .map(|child| (child.clone(), path.clone(), depth + 1)),
                );
            }
        }
        let items: Vec<_> = entries.iter().map(|entry| entry.item.clone()).collect();
        for (entry, prefix) in entries.iter_mut().zip(view::tree_prefixes(&items)) {
            entry.item.graph = prefix;
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
