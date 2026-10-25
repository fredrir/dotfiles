use super::*;
use crate::commit::{
    CommitDetails, GitHubCommit,
    avatar::{self, Avatar},
    github::{Batch, CommitMetadata},
};

impl App {
    pub(super) fn history_page_size(&self) -> usize {
        crossterm::terminal::size()
            .map_or(24, |(_, rows)| usize::from(rows))
            .max(1)
    }

    pub(super) fn collect_history(&mut self) {
        self.finish_history_end();
        let Some(stream) = &self.history_stream else {
            return;
        };
        let wanted = self.history_incoming.is_some()
            || self.snapshot.commits.is_empty()
            || (self.view.workspace == Workspace::History
                && self.history_commit.is_none()
                && (self.history_more
                    || self.history_end
                    || !self.query.is_empty()
                    || self
                        .view
                        .selected
                        .max(self.view.offset)
                        .saturating_add(stream.page_size)
                        >= self.entries.len()));
        if !wanted {
            return;
        }
        let result = match stream.try_next() {
            Some(result) => result,
            None if stream.is_finished() && self.history_incoming.is_some() => Ok(Vec::new()),
            None => return,
        };
        self.history_more = false;
        match result {
            Ok(commits) => {
                let previous = self.selected().cloned();
                if let Some(incoming) = &mut self.history_incoming {
                    incoming.extend(commits);
                    if incoming.len() < self.snapshot.commits.len() && !stream.is_finished() {
                        return;
                    }
                    self.snapshot.commits = self.history_incoming.take().unwrap_or_default();
                } else {
                    self.snapshot.commits.extend(commits);
                }
                if let Some(commit) = self.snapshot.commits.first() {
                    self.history.prefetch(&self.repo, commit);
                }
                if self.view.workspace == Workspace::History
                    && self.history_commit.is_none()
                    && !self.history_end
                {
                    self.rebuild();
                    if previous != self.selected().cloned() {
                        self.load_content(true);
                    }
                }
                self.dirty = true;
            }
            Err(error) => {
                self.history_incoming = None;
                self.history_end = false;
                self.note(error.to_string());
            }
        }
    }

    /// Whether older commits are still streaming in, so the commit list does
    /// not yet reach the end of the history.
    pub(super) fn history_incomplete(&self) -> bool {
        self.view.workspace == Workspace::History
            && self.history_commit.is_none()
            && self
                .history_stream
                .as_ref()
                .is_some_and(|stream| !stream.is_finished())
    }

    /// Selects the oldest commit once the remaining history has streamed in.
    fn finish_history_end(&mut self) {
        if !self.history_end {
            return;
        }
        let Some(finished) = self
            .history_stream
            .as_ref()
            .map(crate::git::CommitStream::is_finished)
        else {
            self.history_end = false;
            return;
        };
        let visible = self.view.workspace == Workspace::History && self.history_commit.is_none();
        if !finished || !visible || self.history_incoming.is_some() {
            return;
        }
        self.history_end = false;
        self.rebuild();
        self.view.nav_manual_scroll = false;
        self.view.selected = self.entries.len().saturating_sub(1);
        self.load_content(true);
        self.dirty = true;
    }
}

pub(super) fn visible_parents<'a>(
    commits: &'a [crate::git::Commit],
    visible: &BTreeSet<&str>,
) -> BTreeMap<&'a str, Vec<String>> {
    let mut ancestors: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for commit in commits.iter().rev() {
        let mut parents = Vec::new();
        let mut seen = BTreeSet::new();
        for parent in &commit.parents {
            let nearest = if visible.contains(parent.as_str()) {
                std::slice::from_ref(parent)
            } else {
                ancestors
                    .get(parent.as_str())
                    .map_or(std::slice::from_ref(parent), Vec::as_slice)
            };
            for oid in nearest {
                if seen.insert(oid.as_str()) {
                    parents.push(oid.clone());
                }
            }
        }
        ancestors.insert(commit.oid.as_str(), parents);
    }
    ancestors
}

#[derive(Default)]
pub(super) struct History {
    selected: Option<String>,
    displayed: Option<String>,
    prefetched: Option<crate::git::Commit>,
    cache_pending: bool,
    cached: Worker<Batch>,
    cache: BTreeMap<String, CommitDetails>,
    remote: BTreeMap<String, CommitMetadata>,
    repository: String,
    author_urls: BTreeMap<String, String>,
    avatars: BTreeMap<String, Arc<Avatar>>,
    avatar_attempted: BTreeSet<String>,
    avatar_pending: Option<String>,
    avatar_worker: Worker<(String, Option<Avatar>)>,
    picker: Option<ratatui_image::picker::Picker>,
    github_attempted: BTreeSet<String>,
    github_pending: bool,
    error: Option<String>,
    details: Worker<Result<CommitDetails, String>>,
    github: Worker<Batch>,
    browser: Worker<Result<(), String>>,
}

impl History {
    pub(super) fn set_waker(&mut self, wake: ui_terminal::InputWake) {
        self.details.set_waker(wake.clone());
        self.github.set_waker(wake.clone());
        self.cached.set_waker(wake.clone());
        self.avatar_worker.set_waker(wake.clone());
        self.browser.set_waker(wake);
    }

    pub(super) fn set_image_picker(&mut self, picker: ratatui_image::picker::Picker) {
        self.picker = Some(picker);
    }

    pub(super) fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }
    pub(super) fn browser_result(&self) -> Option<Result<(), String>> {
        self.browser.take()
    }
    pub(super) fn current(&self) -> Option<&CommitDetails> {
        self.cache.get(self.selected.as_ref()?)
    }

    pub(super) fn get(&self, oid: &str) -> Option<&CommitDetails> {
        self.cache.get(oid)
    }

    pub(super) fn prefetch(&mut self, repo: &Repo, commit: &crate::git::Commit) {
        if self.prefetched.is_some() {
            return;
        }
        self.prefetched = Some(commit.clone());
        self.cache_pending = true;
        let repo = repo.clone();
        self.cached
            .submit(move |cancel| crate::commit::github::cached(&repo.with_cancel(cancel)));
    }

    fn avatar_url(&self) -> Option<&str> {
        let (oid, email) = self
            .current()
            .map(|commit| (&commit.oid, &commit.author_email))
            .or_else(|| {
                self.prefetched
                    .as_ref()
                    .map(|commit| (&commit.oid, &commit.author_email))
            })?;
        self.author_avatar_url(oid, email)
    }

    fn author_avatar_url(&self, oid: &str, email: &str) -> Option<&str> {
        self.remote
            .get(oid)
            .and_then(|data| data.avatar_url.as_deref())
            .or_else(|| self.author_urls.get(email).map(String::as_str))
    }

    pub(super) fn avatar_for(&self, details: &CommitDetails) -> Option<&Avatar> {
        self.avatars
            .get(self.author_avatar_url(&details.oid, &details.author_email)?)
            .map(Arc::as_ref)
    }

    fn apply_remote(&self, details: &mut CommitDetails) {
        if let Some(metadata) = self.remote.get(&details.oid) {
            details.github_signature_verified = Some(metadata.signature_verified);
            details.github = Some(GitHubCommit {
                url: format!(
                    "https://github.com/{}/commit/{}",
                    self.repository, details.oid
                ),
            });
        }
    }

    fn select(&mut self, repo: &Repo, commit: Option<&crate::git::Commit>) {
        let oid = commit.map(|commit| commit.oid.clone());
        if self.selected == oid {
            return;
        }
        self.details.invalidate();
        let previous = std::mem::replace(&mut self.selected, oid);
        let Some(commit) = commit else {
            return;
        };
        if self.cache.len() >= 256
            && !self.cache.contains_key(&commit.oid)
            && let Some(evicted) = self
                .cache
                .keys()
                .find(|oid| {
                    Some(*oid) != previous.as_ref() && Some(*oid) != self.displayed.as_ref()
                })
                .cloned()
        {
            self.cache.remove(&evicted);
        }
        let complete = self
            .cache
            .get(&commit.oid)
            .is_some_and(|detail| detail.message.is_some());
        if !complete {
            let mut pending = CommitDetails::pending(commit);
            self.apply_remote(&mut pending);
            self.cache.insert(commit.oid.clone(), pending);
            let repo = repo.clone();
            let oid = commit.oid.clone();
            self.details.submit(move |cancel| {
                repo.with_cancel(cancel)
                    .commit_details(&oid)
                    .map_err(|error| error.to_string())
            });
        }
        self.load_github(repo);
        self.load_avatar();
    }

    fn load_github(&mut self, repo: &Repo) {
        let Some(oid) = self
            .selected
            .clone()
            .or_else(|| self.prefetched.as_ref().map(|commit| commit.oid.clone()))
        else {
            return;
        };
        if self.cache_pending
            || self.github_pending
            || self.remote.contains_key(&oid)
            || !self.github_attempted.insert(oid.clone())
        {
            return;
        }
        let repo = repo.clone();
        self.github_pending = true;
        self.github.submit(move |cancel| {
            crate::commit::github::load(&repo.with_cancel(Arc::clone(&cancel)), &oid, &cancel)
        });
    }

    fn load_avatar(&mut self) {
        let Some(url) = self.avatar_url().map(str::to_owned) else {
            return;
        };
        if self.avatars.contains_key(&url)
            || self.avatar_pending.is_some()
            || !self.avatar_attempted.insert(url.clone())
        {
            return;
        }
        self.avatar_pending = Some(url.clone());
        let picker = self
            .picker
            .clone()
            .unwrap_or_else(ratatui_image::picker::Picker::halfblocks);
        self.avatar_worker.submit(move |cancel| {
            let image = avatar::load(&url, &picker, &cancel);
            (url, image)
        });
    }

    fn apply_batch(&mut self, batch: Batch) {
        if !batch.repository.is_empty() {
            self.repository = batch.repository;
        }
        for (oid, metadata) in batch.commits {
            if !metadata.author_email.is_empty()
                && let Some(url) = &metadata.avatar_url
            {
                self.author_urls
                    .insert(metadata.author_email.clone(), url.clone());
            }
            if let Some(details) = self.cache.get_mut(&oid) {
                details.github_signature_verified = Some(metadata.signature_verified);
                details.github = Some(GitHubCommit {
                    url: format!("https://github.com/{}/commit/{oid}", self.repository),
                });
            }
            self.remote.insert(oid, metadata);
        }
    }

    pub(super) fn collect(&mut self, repo: &Repo) -> bool {
        let mut changed = false;
        if let Some(batch) = self.cached.take() {
            self.cache_pending = false;
            self.apply_batch(batch);
            self.load_github(repo);
            changed = true;
        }
        if let Some(result) = self.details.take() {
            match result {
                Ok(mut details) => {
                    self.apply_remote(&mut details);
                    self.cache.insert(details.oid.clone(), details);
                }
                Err(error) => {
                    if let Some(details) = self
                        .selected
                        .as_ref()
                        .and_then(|oid| self.cache.get_mut(oid))
                    {
                        details.signature = Some("E".into());
                    }
                    self.error = Some(format!("Could not load commit details: {error}"));
                }
            }
            changed = true;
        }
        if let Some(batch) = self.github.take() {
            self.github_pending = false;
            self.apply_batch(batch);
            self.load_github(repo);
            changed = true;
        }
        if let Some((url, avatar)) = self.avatar_worker.take() {
            self.avatar_pending = None;
            if let Some(avatar) = avatar {
                if self.avatars.len() >= 128
                    && let Some((evicted, _)) = self.avatars.pop_first()
                {
                    self.avatar_attempted.remove(&evicted);
                }
                self.avatars.insert(url, Arc::new(avatar));
            }
            changed = true;
        }
        self.load_avatar();
        changed
    }
}

impl App {
    pub(super) fn sync_commit_details(&mut self) {
        self.history.displayed = self.preview_commit().map(|details| details.oid.clone());
        let oid = if self.view.workspace == Workspace::History {
            self.history_commit
                .as_deref()
                .or_else(|| match self.selected() {
                    Some(Target::Commit(oid)) => Some(oid.as_str()),
                    _ => None,
                })
        } else {
            None
        };
        let commit = oid
            .and_then(|oid| {
                self.snapshot
                    .commits
                    .iter()
                    .find(|commit| commit.oid == oid)
                    .or_else(|| {
                        self.branches
                            .origin
                            .as_ref()
                            .map(|origin| &origin.commit)
                            .filter(|commit| commit.oid == oid)
                    })
            })
            .cloned();
        self.history.select(&self.repo, commit.as_ref());
        if self.history.current().is_none() && self.view.focus == Focus::CommitDetails {
            self.view.focus = Focus::Navigator;
        }
    }

    pub(super) fn inspect_commit(&mut self) {
        if let Some(details) = self.history.current() {
            self.dialog = Some(Dialog::Inspect {
                message: details
                    .message
                    .clone()
                    .unwrap_or_else(|| "Loading commit message…".into()),
                scroll: 0,
                loading: details.message.is_none(),
            });
        }
    }

    pub(super) fn activate_commit_field(&mut self) {
        let Some(details) = self.preview_commit() else {
            return;
        };
        match self.view.commit_field {
            0 => self.copy_text(details.author.clone()),
            1 => self.copy_text(details.short_oid()),
            2 if !details.authored_iso.is_empty() => self.copy_text(details.authored_iso.clone()),
            2 => self.note("Commit date is still loading"),
            3 => {
                if let Some(github) = &details.github {
                    let url = github.url.clone();
                    self.history.browser.submit(move |_| {
                        let mut command = Command::new(if cfg!(target_os = "macos") {
                            "open"
                        } else {
                            "xdg-open"
                        });
                        let mut child = command
                            .arg(&url)
                            .stdin(Stdio::null())
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .spawn()
                            .map_err(|error| format!("Could not open browser: {error}"))?;
                        std::thread::spawn(move || {
                            let _ = child.wait();
                        });
                        Ok(())
                    });
                }
            }
            _ => {}
        }
    }

    pub(super) fn commit_field_count(&self) -> usize {
        if self
            .preview_commit()
            .is_some_and(|details| details.github.is_some())
        {
            4
        } else {
            3
        }
    }

    pub(super) fn tree_collapsed(&self) -> &BTreeSet<PathBuf> {
        if self.history_commit.is_some() {
            &self.history_collapsed
        } else if self.branches.open.is_some() {
            &self.branches.collapsed
        } else {
            &self.collapsed
        }
    }

    pub(super) fn tree_collapsed_mut(&mut self) -> &mut BTreeSet<PathBuf> {
        if self.history_commit.is_some() {
            &mut self.history_collapsed
        } else if self.branches.open.is_some() {
            &mut self.branches.collapsed
        } else {
            &mut self.collapsed
        }
    }
}
