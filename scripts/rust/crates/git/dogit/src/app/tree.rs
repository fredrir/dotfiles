use super::*;

#[derive(Default)]
struct Node {
    directory: bool,
    state: view::FileState,
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
            summary.items += 1;
            summary.staged += usize::from(state.staged);
            summary.unstaged += usize::from(state.unstaged);
            merge_state(&mut tree.entry(path.to_owned()).or_default().state, state);
            for parent in path
                .ancestors()
                .skip(1)
                .filter(|path| !path.as_os_str().is_empty())
            {
                let node = tree.entry(parent.to_owned()).or_default();
                node.directory = true;
                merge_state(&mut node.state, state);
            }
        }
        let mut entries: Vec<Entry> = tree
            .into_iter()
            .filter(|(path, _)| {
                !query.is_empty()
                    || !path
                        .ancestors()
                        .skip(1)
                        .any(|parent| self.collapsed.contains(parent))
            })
            .map(|(path, mut node)| {
                node.state.expanded =
                    node.directory && (!query.is_empty() || !self.collapsed.contains(&path));
                let status = statuses.get(path.as_path());
                let code = if node.state.conflicted {
                    "U".into()
                } else if node.directory {
                    String::new()
                } else {
                    status.map_or_else(String::new, |entry| {
                        if entry.untracked {
                            "new".into()
                        } else if entry.unstaged() {
                            entry.worktree.to_string()
                        } else {
                            entry.index.to_string()
                        }
                    })
                };
                Entry {
                    item: NavItem {
                        label: path
                            .file_name()
                            .unwrap_or(path.as_os_str())
                            .to_string_lossy()
                            .into_owned(),
                        status: code,
                        depth: path.components().count().saturating_sub(1),
                        kind: if node.directory {
                            NavKind::Folder
                        } else {
                            NavKind::File
                        },
                        staged: node.state.staged,
                        file_state: Some(node.state),
                        ..Default::default()
                    },
                    target: Target::Path(path, node.directory),
                }
            })
            .collect();
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
        if self.view.workspace != Workspace::Changes
            || matches!(
                self.dialog,
                Some(Dialog::Input {
                    prompt: Prompt::Compose,
                    ..
                })
            )
        {
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
        let available = self.selected_file_state().is_some_and(|state| match side {
            DiffSide::Worktree => state.unstaged || state.conflicted,
            DiffSide::Index => state.staged,
        });
        if self.view.workspace == Workspace::Changes && available && self.side != side {
            self.side = side;
            self.load_content(true);
        }
    }
}

fn merge_state(target: &mut view::FileState, source: view::FileState) {
    target.staged |= source.staged;
    target.unstaged |= source.unstaged;
    target.conflicted |= source.conflicted;
}
