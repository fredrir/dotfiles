use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::actions::{ACTIONS, Action};
use crate::diff::{DiffSide, ExactDiff, LineKind, Selection};
use crate::git::{CommandSpec, Repo, Snapshot};
use crate::jobs::Worker;
use crate::presentation::{DiffPresentation, DiffPresentationConfig};
use crate::state::SavedState;
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
    Branch,
    Stash,
    Merge,
    Rebase,
    CompareFrom,
    CompareTo(String),
    Mainline(Action, String),
}

enum Dialog {
    Compose {
        editor: ui_widgets::EditorBuffer,
    },
    Input {
        prompt: Prompt,
        title: String,
        editor: ui_widgets::EditorBuffer,
    },
    Actions {
        editor: ui_widgets::EditorBuffer,
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

impl Dialog {
    fn editor(&self) -> Option<&ui_widgets::EditorBuffer> {
        match self {
            Self::Compose { editor }
            | Self::Input { editor, .. }
            | Self::Actions { editor, .. } => Some(editor),
            Self::Confirm { .. } => None,
        }
    }
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
    presentation_config: Option<DiffPresentationConfig>,
}
struct Content {
    diff: Option<ExactDiff>,
    presentation: Option<DiffPresentation>,
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
    presentation: DiffPresentationConfig,
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
                    for file in &key.files {
                        key.stamps.push(path_stamp(&repo.root.join(file), false)?);
                    }
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
    cell_offset: usize,
}

#[derive(Clone, Copy)]
struct RowAnchor {
    hunk: usize,
    source: Option<(usize, usize)>,
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
    changes_filter: Option<DiffSide>,
    all_files: bool,
    collapsed: BTreeSet<PathBuf>,
    query: String,
    dialog: Option<Dialog>,
    marked: BTreeSet<usize>,
    row_map: Vec<view::DiffRow>,
    row_paint: Option<view::DiffPaint>,
    row_width: u16,
    row_config: Option<DiffPresentationConfig>,
    changed_lines: BTreeSet<usize>,
    diff_counts: (usize, usize),
    row_split: bool,
    diff: Option<Arc<ExactDiff>>,
    presentation: Option<DiffPresentation>,
    presentation_config: Option<DiffPresentationConfig>,
    split_override: Option<bool>,
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
    clipboard_worker: Option<Worker<Result<Option<String>, String>>>,
    waker: Option<ui_terminal::InputWake>,
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
    notification: Option<notification::Notification>,
    last_refresh: Instant,
    dirty: bool,
    quit: bool,
    dragging: bool,
    drag_offset: i16,
    geometry: Option<Geometry>,
    mouse_capture: ui_terminal::MouseCapture,
}

mod clipboard;
mod commands;
mod editor;
mod execution;
mod input;
mod notification;
mod render;
mod tree;

#[cfg(test)]
#[path = "../../tests/unit/changes_filter_tests.rs"]
mod changes_filter_tests;

#[cfg(test)]
#[path = "../../tests/unit/dialog_tests.rs"]
mod dialog_tests;

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
            changes_filter: None,
            all_files: false,
            collapsed: BTreeSet::new(),
            query: String::new(),
            dialog: None,
            marked: BTreeSet::new(),
            diff: None,
            presentation: None,
            presentation_config: None,
            split_override: None,
            text: Vec::new(),
            structural: None,
            content_line_count: 0,
            loaded_preview: None,
            row_map: Vec::new(),
            row_paint: None,
            row_width: 0,
            row_config: None,
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
            waker: None,
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
            notification: None,
            last_refresh: Instant::now(),
            dirty: true,
            quit: false,
            dragging: false,
            drag_offset: 0,
            geometry: None,
            mouse_capture: ui_terminal::MouseCapture::Enabled,
        }
    }

    fn set_waker(&mut self, wake: ui_terminal::InputWake) {
        self.snapshot_worker.set_waker(wake.clone());
        self.content_worker.set_waker(wake.clone());
        self.prepare_worker.set_waker(wake.clone());
        self.mutation_worker.set_waker(wake.clone());
        self.waker = Some(wake);
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

    fn composing(&self) -> bool {
        matches!(self.dialog, Some(Dialog::Compose { .. }))
    }

    fn preview_request(&self) -> PreviewRequest {
        let mut presentation = self.presentation_config.clone().unwrap_or_default();
        presentation.split = self.view.split;
        PreviewRequest {
            target: self.selected().cloned(),
            workspace: self.view.workspace,
            history_commit: self.history_commit.clone(),
            comparison: self.compare.clone(),
            side: self.side,
            all_files: self.all_files,
            composing: self.composing(),
            structural: self.structure_enabled,
            presentation,
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
        let read_presentation_config = force || self.presentation_config.is_none();
        self.snapshot_worker.submit(move |cancel| {
            let repo = repo.with_cancel(Arc::clone(&cancel));
            let presentation_config = read_presentation_config.then(|| {
                crate::presentation::load_config(&repo.root, Some(&cancel)).unwrap_or_default()
            });
            let snapshot = repo.snapshot(limit).map_err(|error| error.to_string())?;
            let files = repo.files().map_err(|error| error.to_string())?;
            let preview = PreviewKey::capture(&repo, request, &snapshot, &files).ok();
            Ok(Data {
                snapshot,
                files,
                preview,
                force,
                presentation_config,
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
                    let waker = self.waker.clone();
                    let mouse_capture = self.mouse_capture;
                    *self = Self::new(repo, Start::Changes);
                    self.mouse_capture = mouse_capture;
                    if let Some(waker) = waker {
                        self.set_waker(waker);
                    }
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
                    self.report_result(&label, Err(error));
                }
            }
        }
        if let Some(result) = self.snapshot_worker.take() {
            self.snapshot_pending = false;
            let force = std::mem::take(&mut self.refresh_forced);
            match result {
                Ok(data) => {
                    let reread_config = force && data.presentation_config.is_none();
                    if let Some(config) = data.presentation_config {
                        let previous_split = self.view.split;
                        self.view.split = self.split_override.unwrap_or(config.split);
                        self.presentation_config = Some(config);
                        if previous_split != self.view.split {
                            self.update_rows();
                        }
                    }
                    let previous = self.selected().cloned();
                    let previous_side = self.side;
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
                            .filter(|entry| entry.staged() && !entry.unmerged)
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
                    let selected_changed =
                        previous != self.selected().cloned() || previous_side != self.side;
                    let request = self.preview_request();
                    let stale = data.preview.as_ref().is_none_or(|key| {
                        if key.request == request {
                            self.loaded_preview.as_ref() != Some(key)
                        } else {
                            self.loaded_preview.is_none()
                        }
                    });
                    if selected_changed || data.force || force || (!self.content_pending && stale) {
                        self.load_content(selected_changed);
                    }
                    if reread_config {
                        self.request_refresh(true);
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
                        self.row_paint = None;
                    }
                    self.diff = content.diff.map(Arc::new);
                    self.presentation = content.presentation;
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
                    self.presentation = None;
                    self.row_map.clear();
                    self.row_paint = None;
                    self.text = vec![error];
                    self.structural = None;
                    self.content_line_count = self.text.len();
                    self.loaded_preview = None;
                }
            }
            self.dirty = true;
        }
        if let Some(result) = self.clipboard_worker.as_ref().and_then(Worker::take) {
            let result = result.and_then(|fallback| {
                if let Some(value) = fallback {
                    crossterm::execute!(
                        io::stdout(),
                        crossterm::clipboard::CopyToClipboard::to_clipboard_from(value)
                    )
                    .map_err(|error| error.to_string())?;
                }
                Ok(())
            });
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
            self.report_result(&label, result);
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
            .notification
            .as_ref()
            .is_some_and(|notification| notification.view(false).expired())
        {
            self.notification = None;
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

    fn report_result(&mut self, label: &str, result: Result<String, String>) {
        let (detail, failed) = match result {
            Ok(text) => (text, false),
            Err(text) => (text, true),
        };
        if failed || (!label.starts_with("Stage") && !label.starts_with("Unstage")) {
            self.feedback = None;
            self.notification = Some(notification::Notification {
                title: if failed {
                    format!("{} failed", notification::capitalize(label))
                } else {
                    notification::success_message(label)
                },
                detail: if failed { detail } else { String::new() },
                failed,
                started: Instant::now(),
            });
        }
        self.dirty = true;
    }

    fn note(&mut self, message: impl Into<String>) {
        self.feedback = Some((message.into(), false, Instant::now()));
        self.dirty = true;
    }

    fn rebuild(&mut self) {
        let selected = self.selected().cloned();
        let mut entries = Vec::new();
        let mut changes_summary = None;
        match self.view.workspace {
            Workspace::Changes => {
                let (tree, summary) = self.changes_entries();
                entries = tree;
                changes_summary = Some(summary);
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
                                file_state: None,
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
                    if item.kind != NavKind::Folder {
                        summary.staged += usize::from(item.staged);
                        summary.unstaged +=
                            usize::from(item.file_state.is_some_and(|state| state.unstaged));
                    }
                    summary.has_commits |= item.kind == NavKind::Commit;
                    summary
                });
        if let Some(summary) = changes_summary {
            self.nav_summary = summary;
        }
        if let Some(index) = selected.as_ref().and_then(|target| {
            self.entries
                .iter()
                .position(|entry| entry.target == *target)
        }) {
            self.view.selected = index;
        } else if self.view.workspace == Workspace::Changes && selected.is_some() {
            self.view.selected = selected
                .as_ref()
                .and_then(|target| {
                    let Target::Path(path, _) = target else {
                        return None;
                    };
                    path.ancestors().skip(1).find_map(|parent| {
                        self.entries
                            .iter()
                            .position(|entry| entry.target == Target::Path(parent.to_owned(), true))
                    })
                })
                .unwrap_or(self.view.selected);
        } else if self.view.workspace == Workspace::Changes {
            self.view.selected = 0;
        }
        self.view.selected = self.view.selected.min(self.entries.len().saturating_sub(1));
        self.align_diff_side();
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
        let previous_side = self.side;
        self.align_diff_side();
        let reset = reset || previous_side != self.side;
        if reset {
            self.view.canvas_manual_scroll = false;
            self.view.diff_offset = 0;
            self.view.diff_cursor = 0;
            self.view.horizontal = 0;
            self.marked.clear();
            self.diff = None;
            self.presentation = None;
            self.row_map.clear();
            self.row_paint = None;
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
        let composing = self.composing();
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
        if let Some(Target::Worktree(_)) = &target {
            self.content_worker.invalidate();
            self.content_pending = false;
            self.diff = None;
            self.presentation = None;
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
            let presentation_config = request.presentation.clone();
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
                Some(Target::Path(path, true)) => Some(ExactDiff::directory_cancellable(
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
            if let Some((path, directory)) = preview_path
                && !directory
                && !composing
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
            let presentation = diff
                .as_ref()
                .map(|document| {
                    crate::presentation::render(
                        &root,
                        document,
                        width,
                        &presentation_config,
                        Some(&cancel),
                    )
                })
                .transpose()?;
            Ok(Content {
                diff,
                presentation,
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
            cell_offset: if row.line.is_some() {
                row.left_offset
            } else {
                row.right_offset
            },
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
            && let Some(index) = self.row_for_anchor(RowAnchor {
                hunk,
                source: (!anchor.header).then_some((line.id, anchor.cell_offset)),
            })
        {
            self.view.diff_cursor = index;
        }
    }

    fn update_rows(&mut self) {
        let width = self
            .geometry
            .as_ref()
            .map_or(0, |geometry| geometry.canvas.width);
        self.update_rows_for_width(width);
    }

    fn diff_config(&self) -> &DiffPresentationConfig {
        static DEFAULT: OnceLock<DiffPresentationConfig> = OnceLock::new();
        self.presentation
            .as_ref()
            .map(|value| &value.config)
            .or(self.presentation_config.as_ref())
            .unwrap_or_else(|| DEFAULT.get_or_init(DiffPresentationConfig::default))
    }

    fn row_anchor(&self, index: usize) -> Option<RowAnchor> {
        let row = self.row_map.get(index)?;
        Some(RowAnchor {
            hunk: row.hunk,
            source: row
                .line
                .map(|id| (id, row.left_offset))
                .or_else(|| row.right_line.map(|id| (id, row.right_offset))),
        })
    }

    fn row_for_anchor(&self, anchor: RowAnchor) -> Option<usize> {
        if let Some((id, offset)) = anchor.source {
            self.row_map
                .iter()
                .enumerate()
                .filter_map(|(index, row)| {
                    let start = if row.line == Some(id) {
                        row.left_offset
                    } else if row.right_line == Some(id) {
                        row.right_offset
                    } else {
                        return None;
                    };
                    Some((index, start))
                })
                .min_by_key(|(_, start)| (*start > offset, start.abs_diff(offset)))
                .map(|(index, _)| index)
        } else {
            self.row_map.iter().position(|row| {
                row.hunk == anchor.hunk && row.line.is_none() && row.right_line.is_none()
            })
        }
    }

    fn update_rows_for_width(&mut self, width: u16) {
        let cursor = self.row_anchor(self.view.diff_cursor);
        let viewport = self.row_anchor(self.view.diff_offset);
        let config = self.diff_config().clone();
        self.row_width = width;
        self.row_split = self.view.split && width >= 72;
        if let Some(diff) = &self.diff {
            let layout = view::configured_diff_layout(
                diff,
                width,
                self.row_split,
                self.presentation.as_ref(),
                &config,
            );
            self.row_map = layout.rows;
            self.row_paint = Some(layout.paint);
            self.view.horizontal =
                self.view
                    .horizontal
                    .min(view::configured_diff_horizontal_limit(
                        diff,
                        width,
                        self.row_split,
                        &config,
                    ));
        } else {
            self.row_map.clear();
            self.row_paint = None;
        }
        self.row_config = Some(config);
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
        if let Some(index) = cursor.and_then(|anchor| self.row_for_anchor(anchor)) {
            self.view.diff_cursor = index;
        }
        if let Some(index) = viewport.and_then(|anchor| self.row_for_anchor(anchor)) {
            self.view.diff_offset = index;
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
        file_state: None,
        graph: String::new(),
    }
}

pub fn run(directory: &Path, start: Start, mouse: bool) -> Result<(), String> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("dogit needs an interactive terminal".into());
    }
    let repo = Repo::open(directory).map_err(|error| error.to_string())?;
    let mut app = App::new(repo, start);
    if !mouse {
        app.mouse_capture = ui_terminal::MouseCapture::Disabled;
    }
    let mut signals = Some(ui_terminal::SignalGuard::new().map_err(|error| error.to_string())?);
    let mut terminal =
        Some(crate::session::Session::new(app.mouse_capture).map_err(|error| error.to_string())?);
    let input = ui_terminal::Input::new().map_err(|error| error.to_string())?;
    app.set_waker(input.waker());
    app.refresh();
    let mut last_frame = Instant::now();
    let mut toast_was_animated = false;
    let mut queued = None;
    while !app.quit && !ui_terminal::termination_requested() {
        app.collect();
        if let Some(prepared) = app.prepared.take() {
            app.handoff(prepared, &mut terminal, &mut signals)?;
        }
        if app.last_refresh.elapsed() > Duration::from_secs(3) {
            app.refresh_background();
        }
        if let Some(session) = terminal.as_mut() {
            session
                .set_editor_selection(
                    app.dialog
                        .as_ref()
                        .and_then(Dialog::editor)
                        .is_some_and(|editor| editor.selection().is_some()),
                )
                .map_err(|error| error.to_string())?;
            session
                .set_mouse_capture(if app.dialog.as_ref().and_then(Dialog::editor).is_some() {
                    ui_terminal::MouseCapture::Disabled
                } else {
                    app.mouse_capture
                })
                .map_err(|error| error.to_string())?;
            session
                .set_tree_shortcuts(
                    app.view.workspace == Workspace::Changes
                        && app.view.focus == Focus::Navigator
                        && app.dialog.is_none(),
                )
                .map_err(|error| error.to_string())?;
        }
        let reduced_motion = ui_terminal::reduced_motion_requested("DOGIT_REDUCED_MOTION");
        let toast_animated = app
            .notification
            .as_ref()
            .is_some_and(|notification| notification.view(reduced_motion).animating());
        let frame_interval = if toast_animated {
            Some(Duration::from_millis(16))
        } else if app.operation.is_some() && !reduced_motion {
            Some(Duration::from_millis(80))
        } else {
            None
        };
        if app.dirty
            || toast_was_animated != toast_animated
            || frame_interval.is_some_and(|interval| last_frame.elapsed() >= interval)
        {
            app.dirty = false;
            app.draw(terminal.as_mut().ok_or("terminal unavailable")?)?;
            last_frame = Instant::now();
        }
        toast_was_animated = toast_animated;
        let mut wait = frame_interval.map_or(Duration::from_millis(150), |interval| {
            interval.saturating_sub(last_frame.elapsed())
        });
        if let Some(notification) = &app.notification {
            wait = wait.min(notification.view(reduced_motion).next_transition());
        }
        let mut waited = match queued.take() {
            Some(event) => event,
            None => input.wait(wait).map_err(|error| error.to_string())?,
        };
        let burst = Instant::now();
        for index in 0..128 {
            let batchable = app.batchable_input(&waited);
            match waited {
                ui_terminal::Waited::Event(Event::Key(key))
                    if key.kind != KeyEventKind::Release =>
                {
                    app.key(key, &mut terminal, &mut signals)?
                }
                ui_terminal::Waited::Event(Event::Mouse(mouse))
                    if app.mouse_capture == ui_terminal::MouseCapture::Enabled =>
                {
                    app.mouse(mouse)
                }
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
                ui_terminal::Waited::HangUp => app.quit = true,
                _ => {}
            }
            if !batchable || app.quit || index == 127 || burst.elapsed() >= Duration::from_millis(2)
            {
                break;
            }
            waited = input
                .wait(Duration::ZERO)
                .map_err(|error| error.to_string())?;
            if !app.batchable_input(&waited) {
                queued = Some(waited);
                break;
            }
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
