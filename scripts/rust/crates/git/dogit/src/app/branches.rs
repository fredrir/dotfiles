use super::*;
use std::sync::atomic::AtomicBool;

use crate::git::{Branch, Commit, GitError};

#[derive(Clone, Debug, PartialEq, Eq)]
struct OverviewKey {
    reference: String,
    tip: String,
    against: Option<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FilesKey {
    reference: String,
    tip: String,
    against: Option<String>,
}

pub(super) struct Files {
    reference: String,
    tip: String,
    base: Option<String>,
    paths: Vec<PathBuf>,
    changes: BTreeMap<PathBuf, Option<(usize, usize)>>,
    deleted: BTreeSet<PathBuf>,
}

pub(super) struct Origin {
    pub commit: Commit,
    target: Option<Target>,
    open: Option<String>,
    tree: Option<Target>,
    focus: Focus,
    cursor: usize,
}

#[derive(Default)]
pub(super) struct Branches {
    pub open: Option<String>,
    pub changed_only: bool,
    pub collapsed: BTreeSet<PathBuf>,
    pub origin: Option<Origin>,
    fresh: bool,
    files: Option<Arc<Files>>,
    files_key: Option<FilesKey>,
    files_worker: Worker<(FilesKey, Result<Files, String>)>,
    overview: Option<(OverviewKey, view::BranchOverview)>,
    commits: Option<crate::git::CommitStream>,
    wanted: Option<OverviewKey>,
    pending: Option<OverviewKey>,
    overview_worker: Worker<(OverviewKey, Result<view::BranchOverview, String>)>,
}

impl Files {
    pub(super) fn base(&self) -> Option<&str> {
        self.base.as_deref()
    }
}

impl Branches {
    pub(super) fn set_waker(&mut self, wake: ui_terminal::InputWake) {
        self.files_worker.set_waker(wake.clone());
        self.overview_worker.set_waker(wake);
    }

    pub(super) fn overview(&self) -> Option<&view::BranchOverview> {
        let wanted = self.wanted.as_ref()?;
        self.overview
            .as_ref()
            .filter(|(key, _)| key == wanted)
            .map(|(_, overview)| overview)
    }

    pub(super) fn files(&self) -> Option<&Arc<Files>> {
        let open = self.open.as_ref()?;
        self.files.as_ref().filter(|files| files.reference == *open)
    }

    fn apply_default_collapse(&mut self) {
        if !self.fresh {
            return;
        }
        let Some(files) = self.files().cloned() else {
            return;
        };
        self.fresh = false;
        let changed: BTreeSet<_> = files
            .changes
            .keys()
            .flat_map(|path| path.ancestors().skip(1))
            .collect();
        self.collapsed = files
            .paths
            .iter()
            .flat_map(|path| path.ancestors().skip(1))
            .filter(|directory| !directory.as_os_str().is_empty() && !changed.contains(directory))
            .map(Path::to_path_buf)
            .collect();
    }
}

pub(super) fn reference(name: &str, remote: bool) -> String {
    format!("refs/{}/{name}", if remote { "remotes" } else { "heads" })
}

fn find(snapshot: &Snapshot, target: Option<Target>) -> Option<&Branch> {
    let Some(Target::Branch(name, remote)) = target else {
        return None;
    };
    let reference = reference(&name, remote);
    snapshot
        .branches
        .iter()
        .find(|branch| branch.full_name == reference)
}

pub(super) fn panel<'a>(
    snapshot: &'a Snapshot,
    branches: &'a Branches,
    target: Option<Target>,
) -> Option<view::BranchPanel<'a>> {
    Some(view::BranchPanel {
        branch: find(snapshot, target)?,
        overview: branches.overview(),
        docked: branches.open.is_some(),
    })
}

fn target(reference: &str) -> Option<Target> {
    reference
        .strip_prefix("refs/heads/")
        .map(|name| Target::Branch(name.into(), false))
        .or_else(|| {
            reference
                .strip_prefix("refs/remotes/")
                .map(|name| Target::Branch(name.into(), true))
        })
}

impl App {
    pub(super) fn branch_target(&self) -> Option<Target> {
        if self.view.workspace != Workspace::Branches {
            return None;
        }
        match &self.branches.open {
            Some(reference) => target(reference),
            None => self
                .selected()
                .filter(|target| matches!(target, Target::Branch(..)))
                .cloned(),
        }
    }

    pub(super) fn focused_branch(&self) -> Option<&Branch> {
        find(&self.snapshot, self.branch_target())
    }

    pub(super) fn branch_panel_focused(&self) -> bool {
        self.view.workspace == Workspace::Branches
            && if self.branches.open.is_some() {
                self.view.focus == Focus::CommitDetails
            } else {
                self.view.focus == Focus::Canvas
                    && matches!(self.selected(), Some(Target::Branch(..)))
            }
    }

    pub(super) fn default_remote(&self) -> String {
        let remotes: BTreeSet<&str> = self
            .snapshot
            .branches
            .iter()
            .filter_map(|branch| {
                branch
                    .remote_parts()
                    .map(|(remote, _)| remote)
                    .or(branch.upstream_remote.as_deref())
            })
            .collect();
        if remotes.is_empty() || remotes.contains("origin") {
            "origin".into()
        } else {
            remotes.first().copied().unwrap_or("origin").into()
        }
    }

    pub(super) fn branch_entries(&self) -> Vec<Entry> {
        let tracked: BTreeSet<&str> = self
            .snapshot
            .branches
            .iter()
            .filter(|branch| !branch.remote)
            .filter_map(|branch| branch.upstream.as_deref())
            .collect();
        let mut branches: Vec<_> = self
            .snapshot
            .branches
            .iter()
            .filter(|branch| !(branch.remote && tracked.contains(branch.name.as_str())))
            .filter(|branch| self.matches(&branch.name))
            .collect();
        branches.sort_by_key(|branch| {
            let remote = branch.remote_parts().map(|(remote, _)| remote);
            (
                branch.remote,
                !branch.current,
                remote != Some("origin"),
                remote,
            )
        });
        branches
            .into_iter()
            .map(|branch| Entry {
                target: Target::Branch(branch.name.clone(), branch.remote),
                item: NavItem {
                    label: branch.name.clone(),
                    kind: NavKind::Branch,
                    branch: Some(branch.clone()),
                    ..NavItem::default()
                },
            })
            .collect()
    }

    pub(super) fn branch_tree_entries(&self) -> (Vec<Entry>, view::NavSummary) {
        let Some(files) = self.branches.files().cloned() else {
            return (Vec::new(), view::NavSummary::default());
        };
        let changed_only = self.branches.changed_only;
        self.file_tree_entries(
            files
                .paths
                .iter()
                .filter(|path| !changed_only || files.changes.contains_key(*path))
                .map(|path| {
                    let change = files.changes.get(path);
                    (
                        path.as_path(),
                        view::FileState {
                            changed: change.is_some(),
                            lines: change.copied().flatten(),
                            deleted: files.deleted.contains(path),
                            ..view::FileState::default()
                        },
                    )
                }),
        )
    }

    pub(super) fn sync_branch(&mut self) {
        let Some(branch) = self.focused_branch().cloned() else {
            self.branches.wanted = None;
            self.branches.overview = None;
            self.branches.commits = None;
            self.branches.overview_worker.invalidate();
            self.branches.pending = None;
            return;
        };
        let against = if branch.current {
            branch.upstream.as_ref().and_then(|upstream| {
                self.snapshot
                    .branches
                    .iter()
                    .filter(|candidate| candidate.name == *upstream)
                    .max_by_key(|candidate| candidate.remote)
                    .map(|candidate| (upstream.clone(), candidate.oid.clone()))
            })
        } else {
            self.snapshot.status.oid.clone().map(|oid| {
                (
                    self.snapshot
                        .status
                        .branch
                        .clone()
                        .unwrap_or_else(|| "HEAD".into()),
                    oid,
                )
            })
        };
        let files_against = against.as_ref().map(|(_, oid)| oid.clone());
        let key = OverviewKey {
            reference: branch.full_name.clone(),
            tip: branch.oid.clone(),
            against,
        };
        if self
            .branches
            .wanted
            .as_ref()
            .is_none_or(|wanted| wanted.reference != key.reference)
        {
            self.view.branch_cursor = 0;
        }
        if self.branches.wanted.as_ref() != Some(&key) {
            self.branches.commits = None;
            self.branches.wanted = Some(key.clone());
            self.dirty = true;
        }
        if self.branches.overview().is_none() && self.branches.pending.as_ref() != Some(&key) {
            self.branches.pending = Some(key.clone());
            let repo = self.repo.clone();
            self.branches.overview_worker.submit(move |cancel| {
                let result = load_overview(&repo.with_cancel(cancel), &key);
                (key, result.map_err(|error| error.to_string()))
            });
        }
        if let Some(reference) = self.branches.open.clone() {
            let key = FilesKey {
                reference,
                tip: branch.oid,
                against: files_against,
            };
            if self.branches.files_key.as_ref() != Some(&key) {
                self.branches.files_key = Some(key.clone());
                let repo = self.repo.clone();
                self.branches.files_worker.submit(move |cancel| {
                    let result = load_files(&repo.with_cancel(cancel), &key);
                    (key, result.map_err(|error| error.to_string()))
                });
            }
        }
    }

    pub(super) fn collect_branches(&mut self) {
        if let Some((key, result)) = self.branches.overview_worker.take() {
            if self.branches.pending.as_ref() == Some(&key) {
                self.branches.pending = None;
            }
            match result {
                Ok(overview) if self.branches.wanted.as_ref() == Some(&key) => {
                    let revision = if overview.total > 0 {
                        key.against.as_ref().map_or_else(
                            || key.tip.clone(),
                            |(_, base)| format!("{base}..{}", key.tip),
                        )
                    } else {
                        key.tip.clone()
                    };
                    self.branches.commits = Some(self.repo.commit_stream(
                        vec![revision],
                        false,
                        self.history_page_size(),
                        self.waker.clone(),
                    ));
                    self.branches.overview = Some((key, overview));
                }
                Ok(_) => {}
                Err(error) => self.note(error),
            }
            self.dirty = true;
        }
        let page = self.branches.commits.as_ref().and_then(|stream| {
            let overview = self.branches.overview()?;
            let near_cursor =
                self.view.branch_cursor.saturating_add(stream.page_size) >= overview.commits.len();
            let near_scroll = self.geometry.as_ref().is_some_and(|geometry| {
                let (region, offset) = if self.branches.open.is_some() {
                    (view::ScrollRegion::CommitDetails, self.view.commit_offset)
                } else {
                    (view::ScrollRegion::Canvas, self.view.diff_offset)
                };
                geometry.scrollbar(region).is_some_and(|bar| {
                    offset.saturating_add(self.history_page_size()) >= bar.maximum
                })
            });
            (overview.commits.is_empty() || near_cursor || near_scroll)
                .then(|| stream.try_next())
                .flatten()
        });
        if let Some(page) = page {
            match page {
                Ok(commits) => {
                    if let Some((_, overview)) = &mut self.branches.overview {
                        overview.commits.extend(commits);
                    }
                }
                Err(error) => self.note(error.to_string()),
            }
            self.dirty = true;
        }
        if let Some((key, result)) = self.branches.files_worker.take() {
            match result {
                Ok(files) if self.branches.files_key.as_ref() == Some(&key) => {
                    let previous = self.selected().cloned();
                    self.branches.files = Some(Arc::new(files));
                    self.branches.apply_default_collapse();
                    if self.branches.open.is_some() {
                        self.rebuild();
                        self.load_content(previous != self.selected().cloned());
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    self.branches.files_key = None;
                    self.note(error);
                }
            }
            self.dirty = true;
        }
    }

    pub(super) fn open_branch(&mut self) {
        let Some(Target::Branch(name, remote)) = self.selected().cloned() else {
            return;
        };
        let reference = reference(&name, remote);
        if self.branches.files.as_ref().map(|files| &files.reference) != Some(&reference) {
            self.branches.files = None;
            self.branches.files_key = None;
        }
        self.branches.open = Some(reference);
        self.branches.changed_only = false;
        self.branches.collapsed.clear();
        self.branches.fresh = true;
        self.branches.apply_default_collapse();
        self.query.clear();
        self.view.selected = 0;
        self.view.offset = 0;
        self.view.nav_manual_scroll = false;
        self.view.focus = Focus::Navigator;
        self.rebuild();
        self.load_content(true);
    }

    pub(super) fn leave_branch(&mut self) {
        let Some(target) = self
            .branches
            .open
            .take()
            .and_then(|reference| target(&reference))
        else {
            return;
        };
        self.view.nav_manual_scroll = false;
        self.query.clear();
        self.rebuild();
        if let Some(index) = self.entries.iter().position(|entry| entry.target == target) {
            self.view.selected = index;
        }
        self.view.focus = Focus::Navigator;
        self.load_content(true);
    }

    pub(super) fn open_branch_commit(&mut self) {
        let Some(commit) = self
            .branches
            .overview()
            .and_then(|overview| overview.commits.get(self.view.branch_cursor))
            .cloned()
        else {
            return;
        };
        let origin = Origin {
            commit: commit.clone(),
            target: self.branch_target(),
            open: self.branches.open.clone(),
            tree: self
                .branches
                .open
                .as_ref()
                .and_then(|_| self.selected().cloned()),
            focus: self.view.focus,
            cursor: self.view.branch_cursor,
        };
        self.workspace(Workspace::History);
        self.branches.origin = Some(origin);
        self.history_commit = Some(commit.oid);
        self.revision_paths.clear();
        self.history_collapsed.clear();
        self.view.commit_field = 0;
        self.view.commit_manual_scroll = false;
        self.rebuild();
        self.load_content(true);
    }

    pub(super) fn return_to_branch(&mut self, origin: Origin) {
        self.workspace(Workspace::Branches);
        if let Some(index) = origin.target.as_ref().and_then(|target| {
            self.entries
                .iter()
                .position(|entry| entry.target == *target)
        }) {
            self.view.selected = index;
        }
        if origin.open.is_some() {
            self.branches.open = origin.open;
            self.rebuild();
            if let Some(index) = origin.tree.as_ref().and_then(|target| {
                self.entries
                    .iter()
                    .position(|entry| entry.target == *target)
            }) {
                self.view.selected = index;
            }
        }
        self.load_content(true);
        self.view.focus = origin.focus;
        self.view.branch_cursor = origin.cursor;
    }

    pub(super) fn move_branch_cursor(&mut self, amount: isize) {
        let count = self
            .branches
            .overview()
            .map_or(0, |overview| overview.commits.len());
        self.view.branch_cursor = self
            .view
            .branch_cursor
            .saturating_add_signed(amount)
            .min(count.saturating_sub(1));
        if self.branches.open.is_some() {
            self.view.commit_manual_scroll = false;
        } else {
            self.view.canvas_manual_scroll = false;
        }
    }
}

fn load_overview(repo: &Repo, key: &OverviewKey) -> Result<view::BranchOverview, GitError> {
    let Some((label, against)) = &key.against else {
        return Ok(view::BranchOverview {
            ..view::BranchOverview::default()
        });
    };
    let base = repo.merge_base(against, &key.tip)?;
    let total = repo.count_range(against, &key.tip)?;
    let stat = match &base {
        Some(base) if *base != key.tip => Some(repo.line_stats(base, &key.tip)?.into_iter().fold(
            view::DiffStat::default(),
            |stat, (_, lines)| {
                let (added, removed) = lines.unwrap_or_default();
                view::DiffStat {
                    files: stat.files + 1,
                    added: stat.added + added,
                    removed: stat.removed + removed,
                }
            },
        )),
        _ => None,
    };
    Ok(view::BranchOverview {
        against: label.clone(),
        base: match base {
            Some(oid) => repo.find_commit(&oid)?,
            None => None,
        },
        commits: Vec::new(),
        total,
        stat,
    })
}

fn load_files(repo: &Repo, key: &FilesKey) -> Result<Files, GitError> {
    let base = match &key.against {
        Some(against) => repo.merge_base(against, &key.tip)?,
        None => None,
    };
    let mut paths: BTreeSet<_> = repo.tree_paths(&key.tip)?.into_iter().collect();
    let changes: BTreeMap<_, _> = match &base {
        Some(base) if *base != key.tip => repo.line_stats(base, &key.tip)?.into_iter().collect(),
        _ => BTreeMap::new(),
    };
    let deleted: BTreeSet<_> = changes
        .keys()
        .filter(|path| !paths.contains(*path))
        .cloned()
        .collect();
    paths.extend(deleted.iter().cloned());
    Ok(Files {
        reference: key.reference.clone(),
        tip: key.tip.clone(),
        base,
        paths: paths.into_iter().collect(),
        changes,
        deleted,
    })
}

#[derive(Default)]
pub(super) struct Preview {
    pub diff: Option<ExactDiff>,
    pub text: Vec<String>,
    pub image: Option<Arc<crate::image_preview::Preview>>,
}

pub(super) fn preview(
    repo: &Repo,
    files: &Files,
    target: Option<&Target>,
    cancel: &AtomicBool,
) -> Result<Preview, String> {
    let mut preview = Preview::default();
    let Some(Target::Path(path, directory)) = target else {
        return Ok(preview);
    };
    let changed = if *directory {
        files
            .changes
            .keys()
            .any(|changed| changed.starts_with(path))
    } else {
        files.changes.contains_key(path)
    };
    if changed && let Some(base) = &files.base {
        preview.diff = Some(ExactDiff::between_cancellable(
            &repo.root,
            base,
            &files.tip,
            (!path.as_os_str().is_empty()).then_some(path.as_path()),
            Some(cancel),
        )?);
        return Ok(preview);
    }
    if *directory {
        return Ok(preview);
    }
    let error = |error: GitError| error.to_string();
    let Some(entry) = repo.tree_entry(&files.tip, path).map_err(error)? else {
        return Ok(preview);
    };
    preview.text = if entry.kind == "commit" {
        vec!["Submodule".into()]
    } else if entry.mode == "120000" {
        let destination = repo.blob(&files.tip, path).map_err(error)?;
        vec![format!(
            "Symbolic link → {}",
            String::from_utf8_lossy(&destination)
        )]
    } else {
        let bytes = repo.blob(&files.tip, path).map_err(error)?;
        if let Some(image) = crate::image_preview::Preview::from_bytes(path, &bytes) {
            preview.image = Some(Arc::new(image));
            return Ok(preview);
        }
        text_preview_reader(std::io::Cursor::new(bytes), Some(cancel))?
    };
    Ok(preview)
}
