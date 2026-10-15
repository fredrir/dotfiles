use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::actions::{ACTIONS, Action};
use crate::diff::{DiffSide, ExactDiff, LineKind, Selection};
use crate::git::{CommandSpec, Repo, Snapshot};
use crate::jobs::Worker;
use crate::state::{OperationRecord, SavedState};
use crate::view::{
    self, Focus, Geometry, ItemKind as NavKind, NavItem, Overlay, RenderData, ViewState, Workspace,
};

pub enum Start {
    Changes,
    History,
    Compare(String, String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Target {
    Path(PathBuf, bool),
    Commit(String),
    Branch(String, bool),
    Stash(String, String),
    Worktree(PathBuf),
    Operation(usize),
    Comparison(PathBuf),
    RevisionAll,
}

struct Entry {
    target: Target,
    item: NavItem,
}

#[derive(Clone)]
enum Prompt {
    Search,
    Compose,
    Branch,
    Stash,
    Merge,
    Rebase,
    CompareFrom,
    CompareTo(String),
    Mainline(Action, String),
}

enum Dialog {
    Input {
        prompt: Prompt,
        title: String,
        value: String,
    },
    Actions {
        query: String,
        selected: usize,
    },
    Confirm {
        action: Action,
        target: Option<Target>,
        title: String,
        detail: String,
        discard: Option<Box<crate::git::DiscardGuard>>,
        abort: Option<Box<crate::git::OperationGuard>>,
        scroll: u16,
    },
}

enum Prepared {
    Commands(Vec<CommandSpec>),
    Discard(Box<crate::git::DiscardGuard>),
    Abort(Box<crate::git::OperationGuard>),
    Editor(String, PathBuf),
    Worktree(Repo),
}

struct Data {
    snapshot: Snapshot,
    files: Vec<PathBuf>,
    preview: Option<PreviewKey>,
    force: bool,
}
struct Content {
    diff: Option<ExactDiff>,
    text: Vec<String>,
    structural: Option<String>,
    paths: Option<Vec<PathBuf>>,
    key: PreviewKey,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PreviewRequest {
    target: Option<Target>,
    workspace: Workspace,
    history_commit: Option<String>,
    comparison: Option<(String, String)>,
    side: DiffSide,
    all_files: bool,
    composing: bool,
    structural: bool,
    width: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PreviewKey {
    request: PreviewRequest,
    revisions: Vec<String>,
    stamps: Vec<Option<PathStamp>>,
    files: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PathStamp {
    length: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    file_type: std::fs::FileType,
    permissions: std::fs::Permissions,
    link: Option<PathBuf>,
    #[cfg(unix)]
    inode: (u64, u64, i64, i64),
}

impl PreviewKey {
    fn capture(
        repo: &Repo,
        request: PreviewRequest,
        snapshot: &Snapshot,
        files: &[PathBuf],
    ) -> Result<Self, String> {
        let mut key = Self {
            request,
            revisions: Vec::new(),
            stamps: Vec::new(),
            files: Vec::new(),
        };
        let working = key.request.composing || matches!(key.request.target, Some(Target::Path(..)));
        if working {
            let index = std::env::var_os("GIT_INDEX_FILE")
                .map(PathBuf::from)
                .map(|path| {
                    if path.is_absolute() {
                        path
                    } else {
                        repo.root.join(path)
                    }
                })
                .unwrap_or_else(|| repo.git_dir.join("index"));
            key.stamps.push(path_stamp(&index, false)?);
            key.stamps.push(path_stamp(&index, true)?);
            key.revisions
                .push(snapshot.status.oid.clone().unwrap_or_default());
            if let Some(Target::Path(path, directory)) = &key.request.target {
                key.stamps.push(path_stamp(&repo.root.join(path), false)?);
                if *directory {
                    key.files = files
                        .iter()
                        .filter(|file| file.starts_with(path))
                        .cloned()
                        .collect();
                }
            }
        } else if let Some(Target::Commit(oid) | Target::Stash(_, oid)) = &key.request.target {
            key.revisions.push(oid.clone());
        } else if let Some(oid) = &key.request.history_commit {
            key.revisions.push(oid.clone());
        } else if let Some(Target::Branch(branch, remote)) = &key.request.target {
            key.revisions
                .push(snapshot.status.oid.clone().unwrap_or_default());
            key.revisions.push(resolve_preview_revision(
                repo,
                snapshot,
                &format!(
                    "refs/{}/{branch}",
                    if *remote { "remotes" } else { "heads" }
                ),
            )?);
        } else if key.request.workspace == Workspace::Compare
            && let Some((from, to)) = &key.request.comparison
        {
            key.revisions
                .push(resolve_preview_revision(repo, snapshot, from)?);
            key.revisions
                .push(resolve_preview_revision(repo, snapshot, to)?);
        }
        Ok(key)
    }
}

fn resolve_preview_revision(
    repo: &Repo,
    snapshot: &Snapshot,
    revision: &str,
) -> Result<String, String> {
    if revision == "HEAD"
        && let Some(oid) = &snapshot.status.oid
    {
        return Ok(oid.clone());
    }
    if let Some(branch) = snapshot
        .branches
        .iter()
        .find(|branch| branch.full_name == revision)
    {
        return Ok(branch.oid.clone());
    }
    if (revision.len() == 40 || revision.len() == 64)
        && revision.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Ok(revision.to_owned());
    }
    repo.verify_revision(revision)
        .map_err(|error| error.to_string())
}

fn path_stamp(path: &Path, follow: bool) -> Result<Option<PathStamp>, String> {
    let metadata = match if follow {
        std::fs::metadata(path)
    } else {
        std::fs::symlink_metadata(path)
    } {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    #[cfg(unix)]
    let inode = {
        use std::os::unix::fs::MetadataExt;
        (
            metadata.dev(),
            metadata.ino(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        )
    };
    Ok(Some(PathStamp {
        length: metadata.len(),
        modified: metadata.modified().ok(),
        created: metadata.created().ok(),
        file_type: metadata.file_type(),
        permissions: metadata.permissions(),
        link: if metadata.is_symlink() {
            Some(std::fs::read_link(path).map_err(|error| error.to_string())?)
        } else {
            None
        },
        #[cfg(unix)]
        inode,
    }))
}

fn preview_file(path: &Path) -> Result<Option<Vec<String>>, String> {
    use std::io::Read;
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.is_symlink() {
        let destination = std::fs::read_link(path).map_err(|error| error.to_string())?;
        return Ok(Some(vec![format!(
            "Symbolic link → {}",
            destination.display()
        )]));
    }
    if !metadata.is_file() {
        return Ok(Some(vec![
            if metadata.is_dir() {
                "Directory"
            } else {
                "Special file"
            }
            .into(),
        ]));
    }
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Ok(Some(vec!["Special file".into()]));
    }
    let mut bytes = Vec::new();
    file.take(256 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.contains(&0) {
        return Ok(Some(vec!["Binary file".into()]));
    }
    if bytes.is_empty() {
        return Ok(Some(vec!["Empty file".into()]));
    }
    Ok(Some(
        String::from_utf8_lossy(&bytes)
            .lines()
            .enumerate()
            .map(|(index, line)| format!("{:>4}  {line}", index + 1))
            .collect(),
    ))
}

struct LineAnchor {
    path: String,
    line: crate::diff::DiffLine,
    header: bool,
}
struct PendingOperation {
    label: String,
    started: Instant,
}

struct App {
    repo: Repo,
    snapshot: Snapshot,
    files: Vec<PathBuf>,
    view: ViewState,
    entries: Vec<Entry>,
    nav_items: Vec<NavItem>,
    nav_summary: view::NavSummary,
    staged_count: usize,
    unresolved_count: usize,
    saved: SavedState,
    side: DiffSide,
    all_files: bool,
    collapsed: BTreeSet<PathBuf>,
    query: String,
    dialog: Option<Dialog>,
    marked: BTreeSet<usize>,
    row_map: Vec<view::DiffRow>,
    changed_lines: BTreeSet<usize>,
    diff_counts: (usize, usize),
    row_split: bool,
    diff: Option<Arc<ExactDiff>>,
    text: Vec<String>,
    structural: Option<String>,
    content_line_count: usize,
    loaded_preview: Option<PreviewKey>,
    structure_enabled: bool,
    compare: Option<(String, String)>,
    compare_base: Option<String>,
    history_limit: usize,
    history_commit: Option<String>,
    revision_paths: Vec<PathBuf>,
    copy_ack: Option<Instant>,
    clipboard_worker: Option<Worker<Result<String, String>>>,
    snapshot_worker: Worker<Result<Data, String>>,
    content_worker: Worker<Result<Content, String>>,
    mutation_worker: Worker<Result<String, String>>,
    prepare_worker: Worker<Result<Prepared, String>>,
    prepared: Option<Prepared>,
    snapshot_pending: bool,
    refresh_forced: bool,
    content_pending: bool,
    operation: Option<PendingOperation>,
    feedback: Option<(String, bool, Instant)>,
    last_refresh: Instant,
    dirty: bool,
    quit: bool,
    dragging: bool,
    drag_offset: i16,
    geometry: Option<Geometry>,
}

mod clipboard;
mod commands;
#[cfg(test)]
mod core_tests;
mod execution;
mod input;
mod render;

use execution::execute_captured;

impl App {
    fn new(repo: Repo, start: Start) -> Self {
        let saved = SavedState::load(&repo.root);
        let mut view = ViewState::default();
        let compare = match start {
            Start::Changes => None,
            Start::History => {
                view.workspace = Workspace::History;
                view.nav_width = 50;
                None
            }
            Start::Compare(from, to) => {
                view.workspace = Workspace::Compare;
                Some((from, to))
            }
        };
        if let Some(width) = saved.widths.get(&format!("{:?}", view.workspace)) {
            view.nav_width = *width;
        }
        Self {
            repo,
            snapshot: Snapshot::default(),
            files: Vec::new(),
            view,
            entries: Vec::new(),
            nav_items: Vec::new(),
            nav_summary: view::NavSummary::default(),
            staged_count: 0,
            unresolved_count: 0,
            saved,
            side: DiffSide::Worktree,
            all_files: false,
            collapsed: BTreeSet::new(),
            query: String::new(),
            dialog: None,
            marked: BTreeSet::new(),
            diff: None,
            text: Vec::new(),
            structural: None,
            content_line_count: 0,
            loaded_preview: None,
            row_map: Vec::new(),
            changed_lines: BTreeSet::new(),
            diff_counts: (0, 0),
            row_split: false,
            structure_enabled: false,
            compare,
            compare_base: None,
            history_limit: 300,
            history_commit: None,
            revision_paths: Vec::new(),
            copy_ack: None,
            clipboard_worker: None,
            snapshot_worker: Worker::default(),
            content_worker: Worker::default(),
            mutation_worker: Worker::default(),
            prepare_worker: Worker::default(),
            prepared: None,
            snapshot_pending: false,
            refresh_forced: false,
            content_pending: false,
            operation: None,
            feedback: None,
            last_refresh: Instant::now(),
            dirty: true,
            quit: false,
            dragging: false,
            drag_offset: 0,
            geometry: None,
        }
    }

    fn selected(&self) -> Option<&Target> {
        self.entries
            .get(self.view.selected)
            .map(|entry| &entry.target)
    }

    fn refresh(&mut self) {
        self.request_refresh(true);
    }

    fn refresh_background(&mut self) {
        self.request_refresh(false);
    }

    fn preview_request(&self) -> PreviewRequest {
        PreviewRequest {
            target: self.selected().cloned(),
            workspace: self.view.workspace,
            history_commit: self.history_commit.clone(),
            comparison: self.compare.clone(),
            side: self.side,
            all_files: self.all_files,
            composing: matches!(
                self.dialog,
                Some(Dialog::Input {
                    prompt: Prompt::Compose,
                    ..
                })
            ),
            structural: self.structure_enabled,
            width: if self.structure_enabled {
                self.geometry.as_ref().map_or(100, |geometry| {
                    geometry.canvas.width.saturating_sub(4) as usize
                })
            } else {
                0
            },
        }
    }

    fn request_refresh(&mut self, force: bool) {
        if self.operation.is_some() {
            return;
        }
        if self.snapshot_pending {
            self.refresh_forced |= force;
            return;
        }
        let repo = self.repo.clone();
        let limit = self.history_limit;
        let request = self.preview_request();
        self.snapshot_worker.submit(move |cancel| {
            let repo = repo.with_cancel(cancel);
            let snapshot = repo.snapshot(limit).map_err(|error| error.to_string())?;
            let files = repo.files().map_err(|error| error.to_string())?;
            let preview = PreviewKey::capture(&repo, request, &snapshot, &files).ok();
            Ok(Data {
                snapshot,
                files,
                preview,
                force,
            })
        });
        self.snapshot_pending = true;
        self.last_refresh = Instant::now();
    }

    fn collect(&mut self) {
        if let Some(result) = self.prepare_worker.take() {
            match result {
                Ok(Prepared::Worktree(repo)) => {
                    self.saved
                        .widths
                        .insert(format!("{:?}", self.view.workspace), self.view.nav_width);
                    let _ = self.saved.save(&self.repo.root);
                    *self = Self::new(repo, Start::Changes);
                    self.refresh();
                    return;
                }
                Ok(Prepared::Abort(guard)) => {
                    self.operation = None;
                    self.dialog = Some(Dialog::Confirm { action: Action::Abort, target: None, title: "Abort current operation".into(), detail: "Return to the state before this Git operation. Conflict-resolution edits may be discarded.".into(), discard: None, abort: Some(guard), scroll: 0 });
                    self.dirty = true;
                }
                Ok(Prepared::Discard(guard)) => {
                    self.operation = None;
                    let detail = format!(
                        "Discard unstaged content in {} file(s):\n{}\n\nStaged content is preserved. Untracked files are deleted.",
                        guard.paths.len(),
                        guard
                            .paths
                            .iter()
                            .map(|path| path
                                .to_string_lossy()
                                .chars()
                                .map(|character| {
                                    if character.is_control() {
                                        character.escape_default().to_string()
                                    } else {
                                        character.to_string()
                                    }
                                })
                                .collect::<String>())
                            .collect::<Vec<_>>()
                            .join("\n")
                    );
                    self.dialog = Some(Dialog::Confirm {
                        action: Action::Discard,
                        target: None,
                        title: "Discard selected changes".into(),
                        detail,
                        discard: Some(guard),
                        abort: None,
                        scroll: 0,
                    });
                    self.dirty = true;
                }
                Ok(Prepared::Commands(commands))
                    if !commands.iter().any(|command| command.interactive) =>
                {
                    let repo = self.repo.clone();
                    self.mutation_worker
                        .submit(move |_| execute_captured(&repo, &commands));
                }
                Ok(prepared) => self.prepared = Some(prepared),
                Err(error) => {
                    let label = self
                        .operation
                        .take()
                        .map_or_else(|| "Git operation".into(), |operation| operation.label);
                    self.record(&label, Err(error));
                }
            }
        }
        if let Some(result) = self.snapshot_worker.take() {
            self.snapshot_pending = false;
            let force = std::mem::take(&mut self.refresh_forced);
            match result {
                Ok(data) => {
                    let previous = self.selected().cloned();
                    let changed = self.snapshot.status != data.snapshot.status
                        || self.snapshot.branches != data.snapshot.branches
                        || self.snapshot.commits != data.snapshot.commits
                        || self.snapshot.stashes != data.snapshot.stashes
                        || self.snapshot.worktrees != data.snapshot.worktrees
                        || self.snapshot.operation != data.snapshot.operation
                        || self.files != data.files;
                    if changed {
                        self.snapshot = data.snapshot;
                        self.files = data.files;
                        self.staged_count = self
                            .snapshot
                            .status
                            .entries
                            .iter()
                            .filter(|entry| entry.staged())
                            .count();
                        self.unresolved_count = self
                            .snapshot
                            .status
                            .entries
                            .iter()
                            .filter(|entry| entry.unmerged)
                            .count();
                        self.rebuild();
                    }
                    let selected_changed = previous != self.selected().cloned();
                    let request = self.preview_request();
                    let stale = data.preview.as_ref().is_none_or(|key| {
                        if key.request == request {
                            self.loaded_preview.as_ref() != Some(key)
                        } else {
                            self.loaded_preview.is_none()
                        }
                    });
                    // Timer refreshes must not repeatedly cancel a slow preview.
                    // Explicit refresh and selection changes still supersede it.
                    if selected_changed || data.force || force || (!self.content_pending && stale) {
                        self.load_content(selected_changed);
                    }
                }
                Err(error) => {
                    if self.snapshot.status.branch.is_none() {
                        self.text = vec![error];
                        self.content_line_count = self.text.len();
                    } else {
                        self.feedback = Some((
                            "Repository view is stale; refresh to retry".into(),
                            true,
                            Instant::now(),
                        ));
                    }
                }
            }
            self.dirty = true;
        }
        if let Some(result) = self.content_worker.take() {
            self.content_pending = false;
            match result {
                Ok(content) => {
                    let changed = self.diff.as_ref().map(|diff| &diff.raw)
                        != content.diff.as_ref().map(|diff| &diff.raw);
                    let anchor = changed.then(|| self.line_anchor()).flatten();
                    if changed {
                        self.marked.clear();
                        self.row_map.clear();
                    }
                    self.diff = content.diff.map(Arc::new);
                    self.update_rows();
                    if let Some(anchor) = anchor {
                        self.restore_line_anchor(&anchor);
                    }
                    self.text = content.text;
                    self.structural = content.structural;
                    self.content_line_count = self.text.len().max(
                        self.structural
                            .as_ref()
                            .map_or(0, |text| text.lines().count()),
                    );
                    self.loaded_preview = Some(content.key);
                    if let Some(paths) = content.paths {
                        let previous = self.selected().cloned();
                        self.revision_paths = paths;
                        self.rebuild();
                        if previous != self.selected().cloned() {
                            self.load_content(true);
                        }
                    }
                    self.clamp_cursor();
                }
                Err(error) => {
                    self.diff = None;
                    self.row_map.clear();
                    self.text = vec![error];
                    self.structural = None;
                    self.content_line_count = self.text.len();
                    self.loaded_preview = None;
                }
            }
            self.dirty = true;
        }
        if let Some(result) = self.clipboard_worker.as_ref().and_then(Worker::take) {
            match result {
                Ok(_) => self.copy_ack = Some(Instant::now()),
                Err(error) => self.note(&error),
            }
            self.dirty = true;
        }
        if let Some(result) = self.mutation_worker.take() {
            let label = self
                .operation
                .take()
                .map_or_else(|| "Git operation".into(), |operation| operation.label);
            self.record(&label, result);
            self.snapshot_pending = false;
            self.snapshot_worker.invalidate();
            self.refresh();
        }
        if self
            .feedback
            .as_ref()
            .is_some_and(|(_, failed, since)| !failed && since.elapsed() > Duration::from_secs(5))
        {
            self.feedback = None;
            self.dirty = true;
        }
        if self
            .copy_ack
            .is_some_and(|since| since.elapsed() > Duration::from_millis(1000))
        {
            self.copy_ack = None;
            self.dirty = true;
        }
    }

    fn record(&mut self, label: &str, result: Result<String, String>) {
        let selected_operation = if self.view.workspace == Workspace::Operations {
            match self.selected() {
                Some(Target::Operation(index)) => Some(index + 1),
                _ => None,
            }
        } else {
            None
        };
        let (detail, failed) = match result {
            Ok(text) => (text, false),
            Err(text) => (text, true),
        };
        let outcome = if detail.trim().is_empty() {
            format!("{label} completed")
        } else {
            detail
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or(label)
                .to_owned()
        };
        self.saved.push(OperationRecord {
            action: label.to_owned(),
            outcome: outcome.clone(),
            detail,
            failed,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        });
        let _ = self.saved.save(&self.repo.root);
        if failed || (!label.starts_with("Stage") && !label.starts_with("Unstage")) {
            self.feedback = Some((outcome, failed, Instant::now()));
        }
        self.dirty = true;
        if self.view.workspace == Workspace::Operations {
            self.rebuild();
            if let Some(selected) = selected_operation.and_then(|operation| {
                self.entries
                    .iter()
                    .position(|entry| entry.target == Target::Operation(operation))
            }) {
                self.view.selected = selected;
            }
            self.load_content(false);
        }
    }

    fn note(&mut self, message: impl Into<String>) {
        self.feedback = Some((message.into(), false, Instant::now()));
        self.dirty = true;
    }

    fn rebuild(&mut self) {
        let selected = self.selected().cloned();
        let mut entries = Vec::new();
        match self.view.workspace {
            Workspace::Changes => {
                let statuses: BTreeMap<_, _> = self
                    .snapshot
                    .status
                    .entries
                    .iter()
                    .map(|entry| (entry.path.clone(), entry))
                    .collect();
                let paths: Vec<_> = if self.all_files {
                    self.files.clone()
                } else {
                    self.snapshot
                        .status
                        .entries
                        .iter()
                        .filter(|entry| {
                            entry.unmerged
                                || match self.side {
                                    DiffSide::Worktree => entry.unstaged(),
                                    DiffSide::Index => entry.staged(),
                                }
                        })
                        .map(|entry| entry.path.clone())
                        .collect()
                };
                let mut tree = BTreeMap::<PathBuf, bool>::new();
                for path in paths {
                    if !self.query.is_empty()
                        && !path
                            .to_string_lossy()
                            .to_lowercase()
                            .contains(&self.query.to_lowercase())
                    {
                        continue;
                    }
                    let mut parent = path.parent();
                    while let Some(dir) = parent.filter(|dir| !dir.as_os_str().is_empty()) {
                        tree.insert(dir.to_owned(), true);
                        parent = dir.parent();
                    }
                    tree.entry(path).or_insert(false);
                }
                for (path, directory) in tree {
                    if path
                        .ancestors()
                        .skip(1)
                        .any(|parent| self.collapsed.contains(parent))
                    {
                        continue;
                    }
                    let status = statuses.get(&path);
                    let label = path
                        .file_name()
                        .unwrap_or(path.as_os_str())
                        .to_string_lossy()
                        .into_owned();
                    let detail = if directory {
                        if self.collapsed.contains(&path) {
                            "closed"
                        } else {
                            ""
                        }
                        .into()
                    } else if status.is_some_and(|entry| entry.unmerged) {
                        "conflict".into()
                    } else if status.is_some_and(|entry| entry.staged() && entry.unstaged()) {
                        "mixed".into()
                    } else {
                        String::new()
                    };
                    let code = status.map_or(String::new(), |entry| {
                        if entry.untracked {
                            "new".into()
                        } else {
                            match self.side {
                                DiffSide::Worktree => entry.worktree,
                                DiffSide::Index => entry.index,
                            }
                            .to_string()
                        }
                    });
                    entries.push(Entry {
                        target: Target::Path(path.clone(), directory),
                        item: NavItem {
                            label,
                            detail,
                            status: code,
                            depth: path.components().count().saturating_sub(1),
                            kind: if directory {
                                NavKind::Folder
                            } else {
                                NavKind::File
                            },
                            staged: status.is_some_and(|entry| entry.staged()),
                            graph: String::new(),
                        },
                    });
                }
            }
            Workspace::History => {
                if self.history_commit.is_some() {
                    self.revision_entries(&mut entries);
                } else {
                    let graphs = view::graph_rows(&self.snapshot.commits);
                    for (index, commit) in self.snapshot.commits.iter().enumerate() {
                        if !self.matches(&format!(
                            "{} {} {} {}",
                            commit.oid, commit.subject, commit.author, commit.decorations
                        )) {
                            continue;
                        }
                        entries.push(Entry {
                            target: Target::Commit(commit.oid.clone()),
                            item: NavItem {
                                label: commit.subject.clone(),
                                detail: format!("{}  {}", short(&commit.oid), commit.decorations),
                                status: if self.compare_base.as_deref() == Some(commit.oid.as_str())
                                {
                                    "base".into()
                                } else {
                                    String::new()
                                },
                                depth: 0,
                                kind: NavKind::Commit,
                                staged: false,
                                graph: graphs.get(index).cloned().unwrap_or_default(),
                            },
                        });
                    }
                }
            }
            Workspace::Branches => {
                for branch in &self.snapshot.branches {
                    if self.matches(&branch.name) {
                        entries.push(Entry {
                            target: Target::Branch(branch.name.clone(), branch.remote),
                            item: nav(
                                &branch.name,
                                branch.upstream.as_deref().unwrap_or(""),
                                if branch.current {
                                    "HEAD"
                                } else if branch.remote {
                                    "remote"
                                } else {
                                    ""
                                },
                                NavKind::Branch,
                            ),
                        });
                    }
                }
            }
            Workspace::Stashes => {
                for stash in &self.snapshot.stashes {
                    if self.matches(&stash.subject) {
                        entries.push(Entry {
                            target: Target::Stash(stash.reference.clone(), stash.oid.clone()),
                            item: nav(&stash.subject, &stash.reference, "", NavKind::Stash),
                        });
                    }
                }
            }
            Workspace::Worktrees => {
                for tree in &self.snapshot.worktrees {
                    if self.matches(&tree.path.to_string_lossy()) {
                        entries.push(Entry {
                            target: Target::Worktree(tree.path.clone()),
                            item: nav(
                                &tree
                                    .path
                                    .file_name()
                                    .unwrap_or(tree.path.as_os_str())
                                    .to_string_lossy(),
                                tree.branch.as_deref().unwrap_or("detached"),
                                if tree.path == self.repo.root {
                                    "current"
                                } else if tree.locked.is_some() {
                                    "locked"
                                } else {
                                    ""
                                },
                                NavKind::Worktree,
                            ),
                        });
                    }
                }
            }
            Workspace::Operations => {
                for (index, operation) in self.saved.operations.iter().enumerate() {
                    if self.matches(&format!("{} {}", operation.action, operation.outcome)) {
                        entries.push(Entry {
                            target: Target::Operation(index),
                            item: nav(
                                &operation.action,
                                &operation.outcome,
                                if operation.failed { "failed" } else { "done" },
                                NavKind::File,
                            ),
                        });
                    }
                }
            }
            Workspace::Compare => self.revision_entries(&mut entries),
        }
        self.entries = entries;
        self.nav_items = self
            .entries
            .iter()
            .map(|entry| entry.item.clone())
            .collect();
        self.nav_summary =
            self.nav_items
                .iter()
                .fold(view::NavSummary::default(), |mut summary, item| {
                    summary.items += usize::from(item.kind != NavKind::Folder);
                    summary.staged += usize::from(item.staged);
                    summary.has_commits |= item.kind == NavKind::Commit;
                    summary
                });
        if let Some(index) = selected.as_ref().and_then(|target| {
            self.entries
                .iter()
                .position(|entry| entry.target == *target)
        }) {
            self.view.selected = index;
        } else if self.view.workspace == Workspace::Changes {
            self.view.selected = self
                .entries
                .iter()
                .position(|entry| matches!(entry.target, Target::Path(_, false)))
                .unwrap_or(0);
        }
        self.view.selected = self.view.selected.min(self.entries.len().saturating_sub(1));
        self.dirty = true;
    }

    fn matches(&self, value: &str) -> bool {
        self.query.is_empty() || value.to_lowercase().contains(&self.query.to_lowercase())
    }

    fn revision_entries(&self, entries: &mut Vec<Entry>) {
        entries.push(Entry {
            target: Target::RevisionAll,
            item: nav("All changes", "", "", NavKind::Folder),
        });
        for path in &self.revision_paths {
            if self.matches(&path.to_string_lossy()) {
                entries.push(Entry {
                    target: Target::Comparison(path.clone()),
                    item: nav(&path.to_string_lossy(), "", "", NavKind::File),
                });
            }
        }
    }

    fn load_content(&mut self, reset: bool) {
        if reset {
            self.view.canvas_manual_scroll = false;
            self.view.diff_offset = 0;
            self.view.diff_cursor = 0;
            self.view.horizontal = 0;
            self.marked.clear();
            self.diff = None;
            self.row_map.clear();
            self.structural = None;
            self.text.clear();
            self.content_line_count = 0;
            self.loaded_preview = None;
        }
        let request = self.preview_request();
        let target = self.selected().cloned();
        let root = self.repo.root.clone();
        let side = self.side;
        let all_files = self.all_files;
        let composing = matches!(
            self.dialog,
            Some(Dialog::Input {
                prompt: Prompt::Compose,
                ..
            })
        );
        let workspace = self.view.workspace;
        let comparison = self.compare.clone();
        let history_commit = self.history_commit.clone();
        let repo = self.repo.clone();
        let source_snapshot = Snapshot {
            status: crate::git::Status {
                oid: self.snapshot.status.oid.clone(),
                ..Default::default()
            },
            branches: self.snapshot.branches.clone(),
            ..Default::default()
        };
        let source_files = if matches!(target, Some(Target::Path(_, true))) {
            self.files.clone()
        } else {
            Vec::new()
        };
        let structural = self.structure_enabled;
        let width = self.geometry.as_ref().map_or(100, |geometry| {
            geometry.canvas.width.saturating_sub(4) as usize
        });
        if let Some(Target::Operation(index)) = target {
            self.content_worker.invalidate();
            self.content_pending = false;
            self.diff = None;
            self.structural = None;
            self.text = self
                .saved
                .operations
                .get(index)
                .map(|operation| operation.detail.lines().map(str::to_owned).collect())
                .unwrap_or_default();
            self.content_line_count = self.text.len();
            self.loaded_preview = Some(PreviewKey {
                request,
                revisions: Vec::new(),
                stamps: Vec::new(),
                files: Vec::new(),
            });
            return;
        }
        if let Some(Target::Worktree(_)) = &target {
            self.content_worker.invalidate();
            self.content_pending = false;
            self.diff = None;
            self.structural = None;
            self.text.clear();
            self.content_line_count = self.text.len();
            self.loaded_preview = Some(PreviewKey {
                request,
                revisions: Vec::new(),
                stamps: Vec::new(),
                files: Vec::new(),
            });
            return;
        }
        self.content_worker.submit(move |cancel| {
            let repo = repo.with_cancel(Arc::clone(&cancel));
            let key = PreviewKey::capture(&repo, request, &source_snapshot, &source_files)?;
            let paths = match &target {
                Some(Target::Commit(oid)) => {
                    Some(repo.commit_paths(oid).map_err(|error| error.to_string())?)
                }
                Some(Target::RevisionAll) if history_commit.is_some() => Some(
                    repo.commit_paths(history_commit.as_deref().unwrap_or("HEAD"))
                        .map_err(|error| error.to_string())?,
                ),
                _ if workspace == Workspace::Compare => comparison
                    .as_ref()
                    .map(|(from, to)| {
                        repo.changed_paths(from, to)
                            .map_err(|error| error.to_string())
                    })
                    .transpose()?,
                _ => None,
            };
            let preview_path = match &target {
                Some(Target::Path(path, directory)) => Some((path.clone(), *directory)),
                _ => None,
            };
            let mut diff = match target {
                _ if composing => Some(ExactDiff::staged_cancellable(&root, Some(&cancel))?),
                Some(Target::Path(path, false)) => Some(ExactDiff::load_cancellable(
                    &root,
                    &path,
                    side,
                    Some(&cancel),
                )?),
                Some(Target::Comparison(path)) => {
                    if let Some(oid) = &history_commit {
                        Some(ExactDiff::commit_cancellable(
                            &root,
                            oid,
                            Some(&path),
                            Some(&cancel),
                        )?)
                    } else {
                        comparison
                            .as_ref()
                            .map(|(from, to)| {
                                ExactDiff::between_cancellable(
                                    &root,
                                    from,
                                    to,
                                    Some(&path),
                                    Some(&cancel),
                                )
                            })
                            .transpose()?
                    }
                }
                Some(Target::Commit(oid)) | Some(Target::Stash(_, oid)) => Some(
                    ExactDiff::commit_cancellable(&root, &oid, None, Some(&cancel))?,
                ),
                Some(Target::Branch(branch, remote)) => Some(ExactDiff::between_cancellable(
                    &root,
                    "HEAD",
                    &format!("refs/{}/{branch}", if remote { "remotes" } else { "heads" }),
                    None,
                    Some(&cancel),
                )?),
                Some(Target::RevisionAll) if history_commit.is_some() => {
                    Some(ExactDiff::commit_cancellable(
                        &root,
                        history_commit.as_deref().unwrap_or("HEAD"),
                        None,
                        Some(&cancel),
                    )?)
                }
                _ if workspace == Workspace::Compare => match comparison {
                    Some((from, to)) => Some(ExactDiff::between_cancellable(
                        &root,
                        &from,
                        &to,
                        None,
                        Some(&cancel),
                    )?),
                    None => None,
                },
                _ => None,
            };
            let mut text = Vec::new();
            if let Some((path, directory)) = preview_path {
                if directory {
                    text = key
                        .files
                        .iter()
                        .filter(|file| file.starts_with(&path))
                        .map(|file| {
                            file.strip_prefix(&path)
                                .unwrap_or(file)
                                .display()
                                .to_string()
                        })
                        .collect();
                } else if !composing
                    && (all_files && diff.as_ref().is_some_and(|diff| diff.raw.is_empty())
                        || diff.as_ref().is_some_and(|diff| {
                            diff.partial_reason.as_deref()
                                == Some("Resolve the conflict before staging individual lines")
                        }))
                    && let Some(lines) = preview_file(&root.join(&path))?
                {
                    text = lines;
                    diff = None;
                }
            }
            let structure = if structural {
                if let Some(document) = &diff {
                    match document.structural_cancellable(width, Some(&cancel)) {
                        Ok(Some(output)) => Some(output),
                        Ok(None) => None,
                        Err(error) => {
                            text.push(error);
                            None
                        }
                    }
                } else {
                    None
                }
            } else {
                None
            };
            Ok(Content {
                diff,
                text,
                structural: structure,
                paths,
                key,
            })
        });
        self.content_pending = true;
        self.dirty = true;
    }

    fn workspace(&mut self, workspace: Workspace) {
        self.view.nav_manual_scroll = false;
        self.view.canvas_manual_scroll = false;
        self.view.nav_horizontal = 0;
        self.view.divider_hover_row = None;
        self.dragging = false;
        self.view.divider_hovered = false;
        self.saved
            .widths
            .insert(format!("{:?}", self.view.workspace), self.view.nav_width);
        self.view.workspace = workspace;
        self.history_commit = None;
        self.revision_paths.clear();
        self.view.nav_width = self
            .saved
            .widths
            .get(&format!("{workspace:?}"))
            .copied()
            .unwrap_or(if workspace == Workspace::History {
                50
            } else {
                34
            });
        self.view.selected = 0;
        self.view.offset = 0;
        self.view.focus = Focus::Navigator;
        self.query.clear();
        self.rebuild();
        self.load_content(true);
    }

    fn line_anchor(&self) -> Option<LineAnchor> {
        let row = self.row_map.get(self.view.diff_cursor)?;
        let hunk = self.diff.as_ref()?.hunks.get(row.hunk)?;
        let id = row.line.or(row.right_line);
        let line = if let Some(id) = id {
            hunk.lines.iter().find(|line| line.id == id)?
        } else {
            hunk.lines
                .iter()
                .find(|line| line.kind != LineKind::Context)
                .or_else(|| hunk.lines.first())?
        };
        Some(LineAnchor {
            path: hunk.path.clone(),
            line: line.clone(),
            header: id.is_none(),
        })
    }

    fn restore_line_anchor(&mut self, anchor: &LineAnchor) {
        let Some(diff) = &self.diff else {
            return;
        };
        let candidate = diff
            .hunks
            .iter()
            .enumerate()
            .filter(|(_, hunk)| hunk.path == anchor.path)
            .flat_map(|(index, hunk)| hunk.lines.iter().map(move |line| (index, line)))
            .filter(|(_, line)| {
                line.kind == anchor.line.kind && line.content == anchor.line.content
            })
            .min_by_key(|(_, line)| {
                line.old_line
                    .unwrap_or(0)
                    .abs_diff(anchor.line.old_line.unwrap_or(0))
                    + line
                        .new_line
                        .unwrap_or(0)
                        .abs_diff(anchor.line.new_line.unwrap_or(0))
            });
        if let Some((hunk, line)) = candidate
            && let Some(index) = self.row_map.iter().position(|row| {
                if anchor.header {
                    row.hunk == hunk && row.line.is_none() && row.right_line.is_none()
                } else {
                    row.line == Some(line.id) || row.right_line == Some(line.id)
                }
            })
        {
            self.view.diff_cursor = index;
        }
    }

    fn update_rows(&mut self) {
        let anchor = self
            .row_map
            .get(self.view.diff_cursor)
            .and_then(|row| row.line.or(row.right_line));
        self.row_split = self.view.split
            && self
                .geometry
                .as_ref()
                .is_some_and(|geometry| geometry.canvas.width >= 72);
        self.row_map = self
            .diff
            .as_ref()
            .map_or_else(Vec::new, |diff| view::diff_rows(diff, self.row_split));
        self.changed_lines.clear();
        self.diff_counts = (0, 0);
        if let Some(diff) = &self.diff {
            for line in diff.hunks.iter().flat_map(|hunk| &hunk.lines) {
                match line.kind {
                    LineKind::Addition => self.diff_counts.0 += 1,
                    LineKind::Deletion => self.diff_counts.1 += 1,
                    LineKind::Context => continue,
                }
                self.changed_lines.insert(line.id);
            }
        }
        if let Some(index) = anchor.and_then(|id| {
            self.row_map
                .iter()
                .position(|row| row.line == Some(id) || row.right_line == Some(id))
        }) {
            self.view.diff_cursor = index;
        }
    }

    fn clamp_cursor(&mut self) {
        let len = self.row_map.len().max(self.content_line_count);
        self.view.diff_cursor = self.view.diff_cursor.min(len.saturating_sub(1));
        self.view.diff_offset = self.view.diff_offset.min(len.saturating_sub(1));
    }
}

fn short(oid: &str) -> &str {
    &oid[..oid.len().min(8)]
}

fn nav(label: &str, detail: &str, status: &str, kind: NavKind) -> NavItem {
    NavItem {
        label: label.into(),
        detail: detail.into(),
        status: status.into(),
        depth: 0,
        kind,
        staged: false,
        graph: String::new(),
    }
}

pub fn run(directory: &Path, start: Start) -> Result<(), String> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("dogit needs an interactive terminal".into());
    }
    let repo = Repo::open(directory).map_err(|error| error.to_string())?;
    let mut app = App::new(repo, start);
    let mut signals = Some(ui_terminal::SignalGuard::new().map_err(|error| error.to_string())?);
    let mut terminal = Some(crate::session::Session::new().map_err(|error| error.to_string())?);
    let input = ui_terminal::Input::new().map_err(|error| error.to_string())?;
    app.refresh();
    while !app.quit && !ui_terminal::termination_requested() {
        app.collect();
        if let Some(prepared) = app.prepared.take() {
            app.handoff(prepared, &mut terminal, &mut signals)?;
        }
        if app.last_refresh.elapsed() > Duration::from_secs(3) {
            app.refresh_background();
        }
        if app.dirty || app.operation.is_some() || app.view.animating() {
            app.dirty = false;
            app.draw(terminal.as_mut().ok_or("terminal unavailable")?)?;
        }
        let wait = if app.view.animating() {
            Duration::from_millis(16)
        } else if app.snapshot_pending || app.content_pending || app.operation.is_some() {
            Duration::from_millis(25)
        } else {
            Duration::from_millis(150)
        };
        match input.wait(wait).map_err(|error| error.to_string())? {
            ui_terminal::Waited::Event(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                app.key(key, &mut terminal, &mut signals)?
            }
            ui_terminal::Waited::Event(Event::Mouse(mouse)) => app.mouse(mouse),
            ui_terminal::Waited::Event(Event::Resize(_, _)) => {
                app.view.divider_hover_row = None;
                app.dragging = false;
                app.view.divider_hovered = false;
                app.dirty = true;
                if app.structure_enabled {
                    app.load_content(false);
                }
            }
            ui_terminal::Waited::Event(Event::FocusGained) => app.refresh(),
            ui_terminal::Waited::Event(Event::FocusLost) => {
                app.view.divider_hover_row = None;
                app.dragging = false;
                app.view.divider_hovered = false;
                app.dirty = true;
            }
            ui_terminal::Waited::Event(Event::Paste(text)) => app.paste(&text),
            ui_terminal::Waited::HangUp => break,
            _ => {}
        }
    }
    app.saved
        .widths
        .insert(format!("{:?}", app.view.workspace), app.view.nav_width);
    let _ = app.saved.save(&app.repo.root);
    drop(terminal);
    drop(signals);
    Ok(())
}
