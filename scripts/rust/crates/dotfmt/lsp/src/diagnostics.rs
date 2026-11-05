
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use dotfmt_core::diagnostic::Diagnostic as Fatal;
use dotfmt_core::edit::{Finding, Severity, Tag};
use dotfmt_core::position::LineIndex;
use lsp_server::{Message, Notification};
use lsp_types::notification::{LogMessage, Notification as _, PublishDiagnostics};
use lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, DiagnosticTag, Location,
    LogMessageParams, MessageType, NumberOrString, PublishDiagnosticsParams, Uri,
};

use crate::analysis;
use crate::client::Client;
use crate::documents::{Document, Documents};
use crate::settings::{Settings, SettingsStore};
use crate::uri::{self, Key};
use crate::workspace::Workspace;

#[derive(Debug, PartialEq)]
pub struct Merged {
    pub diagnostics: Vec<Diagnostic>,
    pub related_documents: Vec<(Uri, Vec<Diagnostic>)>,
    pub result_id: String,
}

pub struct Diagnostics {
    pending: crossbeam_channel::Sender<Job>,
    published: Arc<Mutex<Published>>,
    client: Arc<Client>,
    settings: Arc<SettingsStore>,
    workspace: Arc<Workspace>,
    removal_epochs: Arc<Mutex<HashMap<PathBuf, u64>>>,
    worker: Option<JoinHandle<()>>,
}

enum Job {
    Schedule(Box<Scheduled>),
    Clear(Uri),
    ClearAll,
    ClearRoot(PathBuf),
    RemoveRoot(PathBuf),
    ClearTscRoot(PathBuf),
    ClearTscUri(Uri),
    Shutdown,
}

struct Scheduled {
    uri: Uri,
    version: i32,
    path: PathBuf,
    language_id: String,
    text: String,
    delay: Duration,
    settings: Settings,
    epoch_root: PathBuf,
    epoch: u64,
}

impl Diagnostics {
    pub fn new(
        workspace: Arc<Workspace>,
        documents: Arc<Mutex<Documents>>,
        client: Arc<Client>,
        settings: Arc<SettingsStore>,
        pull: bool,
    ) -> Self {
        let (pending, jobs) = crossbeam_channel::unbounded();
        let published = Arc::new(Mutex::new(Published::new(
            pull,
            Arc::clone(&workspace),
            Arc::clone(&documents),
        )));
        let removal_epochs = Arc::new(Mutex::new(HashMap::new()));
        let worker = {
            let published = Arc::clone(&published);
            let settings = Arc::clone(&settings);
            let client = Arc::clone(&client);
            let epochs = Arc::clone(&removal_epochs);
            let workspace = Arc::clone(&workspace);
            thread::Builder::new()
                .name("dotfmt-lsp-diagnostics".into())
                .spawn(move || {
                    run(
                        jobs, workspace, documents, settings, published, client, epochs,
                    )
                })
                .ok()
        };
        Self {
            pending,
            published,
            client,
            settings,
            workspace,
            removal_epochs,
            worker,
        }
    }

    pub fn schedule(&self, document: &Document, path: PathBuf, uri: Uri) {
        let path = crate::workspace::normalize(&path);
        let settings = self.settings.settings_for(&path);
        let epoch_root = self.workspace.root_for(&path);
        let epoch = epoch_for(&self.removal_epochs, &epoch_root);
        let _ = self.pending.send(Job::Schedule(Box::new(Scheduled {
            uri,
            version: document.version,
            path,
            language_id: document.language().to_owned(),
            text: document.text().to_owned(),
            delay: Duration::from_millis(settings.debounce_ms),
            settings,
            epoch_root,
            epoch,
        })));
    }

    pub fn clear(&self, uri: Uri) {
        let _ = self.pending.send(Job::Clear(uri));
    }

    pub fn clear_all(&self) {
        let _ = self.pending.send(Job::ClearAll);
    }

    pub fn clear_under(&self, root: PathBuf) {
        let _ = self.pending.send(Job::ClearRoot(root));
    }

    pub fn remove_root(&self, root: PathBuf) {
        let root = crate::workspace::normalize(&root);
        {
            let mut epochs = self
                .removal_epochs
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            *epochs.entry(root.clone()).or_insert(0) += 1;
        }
        let _ = self.pending.send(Job::RemoveRoot(root));
    }

    pub fn clear_tsc_uri(&self, uri: Uri) {
        let _ = self.pending.send(Job::ClearTscUri(uri));
    }

    pub fn clear_tsc_under(&self, root: PathBuf) {
        let _ = self.pending.send(Job::ClearTscRoot(root));
    }

    pub fn publish_fatal(&self, uri: Uri, text: &str, version: i32, errors: &[Fatal]) {
        let entries = fatal_entries(&uri, text, errors);
        let mut published = self
            .published
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        published.set_dotfmt(&uri, version, entries, &self.client);
    }

    pub fn set_tsc(&self, project: &Path, generation: u64, entries: Vec<(Uri, Vec<Diagnostic>)>) {
        let mut published = self
            .published
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        published.set_tsc(project, generation, entries, &self.client);
    }

    pub fn clear_tsc(&self) {
        let mut published = self
            .published
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        published.clear_tsc(&self.client);
    }

    pub fn merged(&self, uri: &Uri, version: Option<i32>) -> Merged {
        self.published
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .merged(uri, version)
    }

    pub fn log(&self, message: impl Into<String>) {
        let _ = self
            .client
            .sender()
            .send(Message::Notification(Notification::new(
                LogMessage::METHOD.into(),
                LogMessageParams {
                    typ: MessageType::LOG,
                    message: message.into(),
                },
            )));
    }
}

impl Drop for Diagnostics {
    fn drop(&mut self) {
        let _ = self.pending.send(Job::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run(
    jobs: crossbeam_channel::Receiver<Job>,
    workspace: Arc<Workspace>,
    documents: Arc<Mutex<Documents>>,
    settings: Arc<SettingsStore>,
    published: Arc<Mutex<Published>>,
    client: Arc<Client>,
    removal_epochs: Arc<Mutex<HashMap<PathBuf, u64>>>,
) {
    let mut pending: HashMap<Key, (Instant, Scheduled)> = HashMap::new();
    loop {
        let now = Instant::now();
        let due: Vec<Key> = pending
            .iter()
            .filter(|(_, (at, _))| *at <= now)
            .map(|(uri, _)| uri.clone())
            .collect();
        for uri in due {
            let Some((_, job)) = pending.remove(&uri) else {
                continue;
            };
            if epoch_for(&removal_epochs, &job.epoch_root) != job.epoch {
                continue;
            }
            if let Some(entries) = inspect(&workspace, &documents, &settings, &job) {
                let mut published = published.lock().unwrap_or_else(|error| error.into_inner());
                published.set_dotfmt(&job.uri, job.version, entries, &client);
            }
        }

        let timeout = pending
            .values()
            .map(|(at, _)| at.saturating_duration_since(Instant::now()))
            .min()
            .unwrap_or(Duration::from_secs(1))
            .min(Duration::from_secs(1));
        match jobs.recv_timeout(timeout) {
            Ok(Job::Schedule(job)) => {
                let job = *job;
                let at = Instant::now() + job.delay;
                let key = uri::key(&job.uri);
                pending.insert(key, (at, job));
            }
            Ok(Job::Clear(uri)) => {
                pending.remove(&uri::key(&uri));
                let mut published = published.lock().unwrap_or_else(|error| error.into_inner());
                published.clear_dotfmt(&uri, &client);
            }
            Ok(Job::ClearAll) => {
                pending.clear();
                let mut published = published.lock().unwrap_or_else(|error| error.into_inner());
                published.clear_all(&client);
            }
            Ok(Job::ClearRoot(root)) => {
                pending.retain(|_, (_, job)| !job.path.starts_with(&root));
                let mut published = published.lock().unwrap_or_else(|error| error.into_inner());
                published.clear_under(&root, &client);
            }
            Ok(Job::RemoveRoot(root)) => {
                pending.retain(|_, (_, job)| job.epoch_root != root);
                let mut published = published.lock().unwrap_or_else(|error| error.into_inner());
                published.clear_under(&root, &client);
            }
            Ok(Job::ClearTscRoot(root)) => {
                let mut published = published.lock().unwrap_or_else(|error| error.into_inner());
                published.clear_tsc_under(&root, &client);
            }
            Ok(Job::ClearTscUri(uri)) => {
                let mut published = published.lock().unwrap_or_else(|error| error.into_inner());
                published.clear_tsc_uri(&uri, &client);
            }
            Ok(Job::Shutdown) => break,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn epoch_for(epochs: &Mutex<HashMap<PathBuf, u64>>, root: &Path) -> u64 {
    epochs
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(root)
        .copied()
        .unwrap_or(0)
}

fn inspect(
    workspace: &Workspace,
    documents: &Mutex<Documents>,
    settings: &SettingsStore,
    job: &Scheduled,
) -> Option<Vec<(Uri, Vec<Diagnostic>)>> {
    let current = documents
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&job.uri)
        .map(|document| document.version);
    if current != Some(job.version) {
        return None;
    }
    if settings.settings_for(&job.path) != job.settings {
        return None;
    }
    let session = workspace.session(&job.path);
    let mut session = session.lock().unwrap_or_else(|error| error.into_inner());
    let languages = analysis::forced(&job.path, &job.uri, &job.language_id);
    match analysis::inspect_errors(&mut session, &job.path, &job.text, languages) {
        Ok(found) => {
            let diagnostics = if job.settings.syntax_diagnostics {
                findings(&job.uri, &job.text, &found)
            } else {
                Vec::new()
            };
            Some(vec![(job.uri.clone(), diagnostics)])
        }
        Err(errors) => Some(if job.settings.syntax_diagnostics {
            fatal_entries(&job.uri, &job.text, &errors)
        } else {
            vec![(job.uri.clone(), Vec::new())]
        }),
    }
}

pub fn findings(uri: &Uri, text: &str, findings: &[Finding]) -> Vec<Diagnostic> {
    let index = LineIndex::new(text);
    findings
        .iter()
        .filter_map(|finding| {
            let severity = match finding.severity {
                Severity::Error => DiagnosticSeverity::ERROR,
                Severity::Warning => DiagnosticSeverity::WARNING,
                Severity::Information => DiagnosticSeverity::INFORMATION,
                Severity::Hint => return None,
            };
            let start = clamp(text, finding.start);
            let end = clamp(text, finding.end).max(start);
            Some(Diagnostic {
                range: range(&index, text, start, end),
                severity: Some(severity),
                code: Some(NumberOrString::String(finding.code.clone())),
                source: Some("dotfmt".into()),
                message: finding.message.clone(),
                related_information: (!finding.related.is_empty()).then(|| {
                    finding
                        .related
                        .iter()
                        .map(|related| DiagnosticRelatedInformation {
                            location: Location {
                                uri: uri.clone(),
                                range: range(
                                    &index,
                                    text,
                                    clamp(text, related.start),
                                    clamp(text, related.end).max(clamp(text, related.start)),
                                ),
                            },
                            message: related.message.clone(),
                        })
                        .collect()
                }),
                tags: (!finding.tags.is_empty()).then(|| {
                    finding
                        .tags
                        .iter()
                        .map(|tag| match tag {
                            Tag::Unnecessary => DiagnosticTag::UNNECESSARY,
                            Tag::Deprecated => DiagnosticTag::DEPRECATED,
                        })
                        .collect()
                }),
                ..Diagnostic::default()
            })
        })
        .collect()
}

pub fn fatal(_uri: &Uri, text: &str, error: &Fatal) -> Diagnostic {
    let start = clamp(
        text,
        error
            .line
            .map(|line| column_offset(text, line, error.column))
            .unwrap_or(0),
    );
    let end = next_position(text, start);
    let index = LineIndex::new(text);
    Diagnostic {
        range: range(&index, text, start, end),
        severity: Some(DiagnosticSeverity::ERROR),
        code: Some(NumberOrString::String(
            match error.kind {
                dotfmt_core::diagnostic::DiagnosticKind::Configuration => "configuration",
                dotfmt_core::diagnostic::DiagnosticKind::Syntax => "syntax",
                dotfmt_core::diagnostic::DiagnosticKind::Io => "io",
                dotfmt_core::diagnostic::DiagnosticKind::Internal => "internal",
            }
            .into(),
        )),
        source: Some("dotfmt".into()),
        message: error.message.clone(),
        ..Diagnostic::default()
    }
}

fn fatal_entries(uri: &Uri, text: &str, errors: &[Fatal]) -> Vec<(Uri, Vec<Diagnostic>)> {
    let mut grouped: HashMap<Key, (Uri, Vec<Diagnostic>)> = HashMap::new();
    let owner = uri::key(uri);
    for error in errors {
        let (target, target_text) = error
            .path
            .as_deref()
            .map(|path| {
                let target = uri::from_path(path);
                let contents = std::fs::read_to_string(path).unwrap_or_default();
                (target, contents)
            })
            .filter(|(target, _)| uri::key(target) != owner)
            .unwrap_or_else(|| (uri.clone(), text.to_owned()));
        let key = uri::key(&target);
        grouped
            .entry(key)
            .or_insert_with(|| (target.clone(), Vec::new()))
            .1
            .push(fatal(&target, &target_text, error));
    }
    grouped.into_values().collect()
}

fn range(index: &LineIndex, text: &str, start: usize, end: usize) -> lsp_types::Range {
    let range = index.range(text, start, end);
    lsp_types::Range::new(
        lsp_types::Position::new(range.start.line, range.start.character),
        lsp_types::Position::new(range.end.line, range.end.character),
    )
}

fn clamp(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn column_offset(text: &str, line: usize, column: Option<usize>) -> usize {
    let start = text
        .split_inclusive('\n')
        .take(line.saturating_sub(1))
        .map(str::len)
        .sum::<usize>()
        .min(text.len());
    let end = text[start..]
        .find('\n')
        .map_or(text.len(), |offset| start + offset);
    let column = column.unwrap_or(1).saturating_sub(1);
    (start + column).min(end)
}

fn next_position(text: &str, start: usize) -> usize {
    let end = text[start..]
        .find('\n')
        .map_or(text.len(), |offset| start + offset);
    text[start..]
        .char_indices()
        .nth(1)
        .map(|(offset, _)| start + offset)
        .filter(|offset| *offset <= end)
        .unwrap_or(end.max(start))
}

struct Entry {
    version: Option<i32>,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, PartialEq)]
struct Sent {
    uri: Uri,
    version: Option<i32>,
    diagnostics: Vec<Diagnostic>,
}

struct Published {
    pull: bool,
    dotfmt: HashMap<Key, Entry>,
    owners: HashMap<Key, Vec<Key>>,
    projects: BTreeMap<PathBuf, TscProject>,
    last: HashMap<Key, Sent>,
    workspace: Arc<Workspace>,
    documents: Arc<Mutex<Documents>>,
}

struct TscProject {
    generation: u64,
    config: PathBuf,
    diagnostics: HashMap<Key, Vec<Diagnostic>>,
}

impl Default for Published {
    fn default() -> Self {
        Self {
            pull: false,
            dotfmt: HashMap::new(),
            owners: HashMap::new(),
            projects: BTreeMap::new(),
            last: HashMap::new(),
            workspace: Arc::new(Workspace::new(Vec::new())),
            documents: Arc::new(Mutex::new(Documents::default())),
        }
    }
}

impl Published {
    fn new(pull: bool, workspace: Arc<Workspace>, documents: Arc<Mutex<Documents>>) -> Self {
        Self {
            pull,
            workspace,
            documents,
            ..Self::default()
        }
    }

    fn set_dotfmt(
        &mut self,
        owner: &Uri,
        version: i32,
        entries: Vec<(Uri, Vec<Diagnostic>)>,
        client: &Client,
    ) {
        let owner = uri::key(owner);
        if self
            .dotfmt
            .get(&owner)
            .and_then(|entry| entry.version)
            .is_some_and(|stored| stored > version)
        {
            return;
        }
        let old_aliases = self.owners.remove(&owner).unwrap_or_default();
        let referenced = self.referenced_by_others(&owner);
        let mut affected = Vec::new();
        for key in old_aliases {
            if !referenced.contains(&key) {
                self.dotfmt.remove(&key);
                affected.push(key);
            }
        }
        self.dotfmt.remove(&owner);
        affected.push(owner.clone());
        let mut aliases = Vec::new();
        let mut includes_owner = false;
        for (uri, diagnostics) in entries {
            let key = uri::key(&uri);
            aliases.push(key.clone());
            affected.push(key.clone());
            let is_owner = key == owner;
            includes_owner |= is_owner;
            if is_owner || !diagnostics.is_empty() {
                self.dotfmt.insert(
                    key,
                    Entry {
                        version: is_owner.then_some(version),
                        diagnostics,
                    },
                );
            }
        }
        if !includes_owner {
            self.dotfmt.insert(
                owner.clone(),
                Entry {
                    version: Some(version),
                    diagnostics: Vec::new(),
                },
            );
        }
        self.owners.insert(owner, aliases);
        self.publish(&affected, client);
    }

    fn clear_dotfmt(&mut self, owner: &Uri, client: &Client) {
        let owner = uri::key(owner);
        let aliases = self.owners.remove(&owner).unwrap_or_default();
        let referenced = self.referenced_by_others(&owner);
        let mut affected = Vec::new();
        for key in aliases {
            if !referenced.contains(&key) {
                self.dotfmt.remove(&key);
                affected.push(key);
            }
        }
        self.dotfmt.remove(&owner);
        affected.push(owner);
        self.publish(&affected, client);
    }

    fn referenced_by_others(&self, owner: &Key) -> HashSet<Key> {
        self.owners
            .iter()
            .filter(|(other, _)| *other != owner)
            .flat_map(|(other, aliases)| {
                std::iter::once(other.clone()).chain(aliases.iter().cloned())
            })
            .collect()
    }

    fn clear_all(&mut self, client: &Client) {
        let mut affected: Vec<Key> = self.dotfmt.keys().cloned().collect();
        affected.extend(self.owners.keys().cloned());
        affected.extend(self.last.keys().cloned());
        affected.extend(
            self.projects
                .values()
                .flat_map(|project| project.diagnostics.keys().cloned()),
        );
        self.dotfmt.clear();
        self.owners.clear();
        self.projects.clear();
        self.publish(&affected, client);
    }

    fn clear_under(&mut self, root: &Path, client: &Client) {
        let mut affected = Vec::new();
        let owners: Vec<(Key, Vec<Key>)> = self
            .owners
            .iter()
            .filter(|(owner, _)| key_in_root(&self.workspace, owner, root))
            .map(|(owner, aliases)| (owner.clone(), aliases.clone()))
            .collect();
        let cleared: HashSet<Key> = owners.iter().map(|(owner, _)| owner.clone()).collect();
        let kept: HashSet<Key> = self
            .owners
            .iter()
            .filter(|(owner, _)| !cleared.contains(*owner))
            .flat_map(|(owner, aliases)| {
                std::iter::once(owner.clone()).chain(aliases.iter().cloned())
            })
            .collect();
        for (owner, aliases) in owners {
            self.owners.remove(&owner);
            self.dotfmt.remove(&owner);
            affected.push(owner);
            for alias in aliases {
                if !kept.contains(&alias) {
                    self.dotfmt.remove(&alias);
                    affected.push(alias);
                }
            }
        }
        let projects: Vec<PathBuf> = self
            .projects
            .keys()
            .filter(|project| self.workspace.in_root(project, root))
            .cloned()
            .collect();
        for project in projects {
            if let Some(entry) = self.projects.remove(&project) {
                affected.extend(entry.diagnostics.into_keys());
            }
        }
        self.publish(&affected, client);
    }

    fn set_tsc(
        &mut self,
        project: &Path,
        generation: u64,
        entries: Vec<(Uri, Vec<Diagnostic>)>,
        client: &Client,
    ) {
        if self
            .projects
            .get(project)
            .is_some_and(|stored| stored.generation > generation)
        {
            return;
        }
        let mut affected: Vec<Key> = self
            .projects
            .remove(project)
            .map(|project| project.diagnostics.into_keys().collect())
            .unwrap_or_default();
        let mut diagnostics = HashMap::new();
        for (uri, entries) in entries {
            let key = uri::key(&uri);
            affected.push(key.clone());
            if !entries.is_empty() {
                diagnostics.insert(key, entries);
            }
        }
        self.projects.insert(
            project.to_path_buf(),
            TscProject {
                generation,
                config: project.to_path_buf(),
                diagnostics,
            },
        );
        self.publish(&affected, client);
    }

    fn clear_tsc_uri(&mut self, uri: &Uri, client: &Client) {
        let key = uri::key(uri);
        for project in self.projects.values_mut() {
            project.diagnostics.remove(&key);
        }
        self.publish(std::slice::from_ref(&key), client);
    }

    fn clear_tsc_under(&mut self, root: &Path, client: &Client) {
        let projects: Vec<PathBuf> = self
            .projects
            .keys()
            .filter(|project| self.workspace.in_root(project, root))
            .cloned()
            .collect();
        let mut affected = Vec::new();
        for project in projects {
            if let Some(entry) = self.projects.remove(&project) {
                affected.extend(entry.diagnostics.into_keys());
            }
        }
        self.publish(&affected, client);
    }

    fn clear_tsc(&mut self, client: &Client) {
        let affected: Vec<Key> = self
            .projects
            .values()
            .flat_map(|project| project.diagnostics.keys().cloned())
            .collect();
        self.projects.clear();
        self.publish(&affected, client);
    }

    fn merged(&self, uri: &Uri, version: Option<i32>) -> Merged {
        let key = uri::key(uri);
        let dotfmt_entry = self.dotfmt.get(&key).filter(|entry| {
            !matches!(
                (entry.version, version),
                (Some(stored), Some(current)) if stored != current
            )
        });
        let mut diagnostics = dotfmt_entry
            .map(|entry| entry.diagnostics.clone())
            .unwrap_or_default();
        diagnostics.extend(self.tsc_for(&key));
        let related = self.related_for(&key);
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        serde_json::to_string(&diagnostics)
            .unwrap_or_default()
            .hash(&mut hasher);
        for (related_key, diagnostics) in &related {
            related_key.hash(&mut hasher);
            serde_json::to_string(diagnostics)
                .unwrap_or_default()
                .hash(&mut hasher);
        }
        Merged {
            result_id: format!("{}:{:x}", version.unwrap_or(-1), hasher.finish()),
            diagnostics,
            related_documents: related
                .into_iter()
                .map(|(key, diagnostics)| (self.display_uri(&key), diagnostics))
                .collect(),
        }
    }

    fn related_for(&self, key: &Key) -> BTreeMap<Key, Vec<Diagnostic>> {
        let mut related = BTreeMap::new();
        if let Some(aliases) = self.owners.get(key) {
            for alias in aliases {
                if alias == key {
                    continue;
                }
                if let Some(entry) = self.dotfmt.get(alias)
                    && !entry.diagnostics.is_empty()
                {
                    related
                        .entry(alias.clone())
                        .or_insert_with(Vec::new)
                        .extend(entry.diagnostics.iter().cloned());
                }
            }
        }
        let path = key.path();
        let mut projects: Vec<&TscProject> = self
            .projects
            .values()
            .filter(|project| project.diagnostics.contains_key(key))
            .collect();
        if projects.is_empty()
            && let Some(path) = path
        {
            let depth = self
                .projects
                .values()
                .filter(|project| {
                    project
                        .config
                        .parent()
                        .is_some_and(|parent| path.starts_with(parent))
                })
                .map(|project| {
                    project
                        .config
                        .parent()
                        .map_or(0, |parent| parent.components().count())
                })
                .max();
            if let Some(depth) = depth {
                projects = self
                    .projects
                    .values()
                    .filter(|project| {
                        project.config.parent().is_some_and(|parent| {
                            path.starts_with(parent) && parent.components().count() == depth
                        })
                    })
                    .collect();
            }
        }
        for project in projects {
            let config = uri::key(&uri::from_path(&project.config));
            for (diagnostic_key, diagnostics) in &project.diagnostics {
                if diagnostic_key == key || diagnostics.is_empty() {
                    continue;
                }
                let is_config =
                    diagnostic_key == &config || diagnostic_key.path().is_some_and(is_tsconfig);
                if is_config {
                    related
                        .entry(diagnostic_key.clone())
                        .or_insert_with(Vec::new)
                        .extend(diagnostics.iter().cloned());
                }
            }
        }
        related
    }

    fn display_uri(&self, key: &Key) -> Uri {
        self.documents
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .uri_for_identity(key)
            .unwrap_or_else(|| key.uri())
    }

    fn publish(&mut self, affected: &[Key], client: &Client) {
        let mut seen = HashSet::new();
        let mut changed = false;
        let mut notifications = Vec::new();
        for key in affected {
            if !seen.insert(key.clone()) {
                continue;
            }
            let uri = self.display_uri(key);
            let dotfmt_entry = self.dotfmt.get(key);
            let tsc = self.tsc_for(key);
            let mut diagnostics = dotfmt_entry
                .map(|entry| entry.diagnostics.clone())
                .unwrap_or_default();
            let has_tsc = !tsc.is_empty();
            diagnostics.extend(tsc);
            let version = match (dotfmt_entry, has_tsc) {
                (Some(entry), false) => entry.version,
                _ => None,
            };
            let sent = Sent {
                uri: uri.clone(),
                version,
                diagnostics,
            };
            if self.last.get(key) == Some(&sent) {
                continue;
            }
            changed = true;
            if let Some(previous) = self.last.get(key)
                && previous.uri != uri
            {
                notifications.push((
                    previous.uri.clone(),
                    Sent {
                        uri: previous.uri.clone(),
                        version: None,
                        diagnostics: Vec::new(),
                    },
                ));
            }
            notifications.push((uri, sent.clone()));
            self.last.insert(key.clone(), sent);
        }
        if !changed {
            return;
        }
        if self.pull {
            let _ = client.send_request("workspace/diagnostic/refresh", ());
            return;
        }
        for (uri, sent) in notifications {
            let notification = Notification::new(
                PublishDiagnostics::METHOD.into(),
                PublishDiagnosticsParams::new(uri, sent.diagnostics, sent.version),
            );
            let _ = client.sender().send(Message::Notification(notification));
        }
    }
}

impl Published {
    fn tsc_for(&self, key: &Key) -> Vec<Diagnostic> {
        let mut diagnostics: Vec<Diagnostic> = self
            .projects
            .values()
            .filter_map(|project| project.diagnostics.get(key))
            .flatten()
            .cloned()
            .collect();
        diagnostics.sort_by(|left, right| {
            (
                left.range.start.line,
                left.range.start.character,
                format!("{:?}", left.code),
            )
                .cmp(&(
                    right.range.start.line,
                    right.range.start.character,
                    format!("{:?}", right.code),
                ))
        });
        diagnostics.dedup();
        diagnostics
    }
}

fn is_tsconfig(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            (name.starts_with("tsconfig") || name.starts_with("jsconfig"))
                && name.ends_with(".json")
        })
}

fn key_in_root(workspace: &Workspace, key: &Key, root: &Path) -> bool {
    key.path().is_some_and(|path| workspace.in_root(path, root))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::str::FromStr;
    use std::sync::Mutex;

    use dotfmt_core::edit::Related;
    use lsp_types::Position;

    use super::*;

    fn write(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, text).unwrap();
    }

    fn scheduled(uri: Uri, path: PathBuf, text: &str, version: i32) -> Scheduled {
        Scheduled {
            uri,
            version,
            epoch_root: path.clone(),
            path,
            language_id: "typescript".into(),
            text: text.into(),
            delay: Duration::from_millis(0),
            settings: Settings::default(),
            epoch: 0,
        }
    }

    fn inspect_ts(root: &Path, text: &str) -> Vec<Diagnostic> {
        write(root, "dotfmt.dotfile", "typescript {}\n");
        let path = root.join("src/a.ts");
        write(root, "src/a.ts", text);
        let uri = uri::from_path(&path);
        let document = Document::new(text.into(), "typescript".into(), 1);
        let workspace = Workspace::new(vec![root.to_path_buf()]);
        let documents = Mutex::new(Documents::default());
        documents.lock().unwrap().open(uri.clone(), document);
        let settings = SettingsStore::new(false, Settings::default());
        let job = scheduled(uri.clone(), path, text, 1);
        let entries = inspect(&workspace, &documents, &settings, &job).expect("inspection");
        entries
            .into_iter()
            .find_map(|(target, diagnostics)| (target == uri).then_some(diagnostics))
            .expect("diagnostics for the open file")
    }

    #[test]
    fn findings_keep_tags_related_information_and_source() {
        let uri = Uri::from_str("file:///tmp/a.json").unwrap();
        let finding = Finding::new(
            "json/trailing-comma",
            "trailing comma",
            Severity::Warning,
            2,
            3,
        )
        .with_tag(Tag::Unnecessary)
        .with_related(Related {
            start: 1,
            end: 2,
            message: "previous value".into(),
        });
        let diagnostics = findings(&uri, "[1,2]\n", &[finding]);
        let diagnostic = &diagnostics[0];
        assert_eq!(diagnostic.source.as_deref(), Some("dotfmt"));
        assert_eq!(diagnostic.severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(diagnostic.tags, Some(vec![DiagnosticTag::UNNECESSARY]));
        assert!(diagnostic.related_information.is_some());
        assert_eq!(
            diagnostic.code,
            Some(NumberOrString::String("json/trailing-comma".into()))
        );
    }

    #[test]
    fn hints_are_never_mapped_to_lsp_diagnostics() {
        let uri = Uri::from_str("file:///tmp/a.json").unwrap();
        let finding = Finding::new("hint", "hint", Severity::Hint, 0, 1);
        assert!(findings(&uri, "a\n", &[finding]).is_empty());
    }

    #[test]
    fn fatal_errors_use_one_based_lines_and_byte_columns() {
        let uri = Uri::from_str("file:///tmp/a.json").unwrap();
        let error = Fatal::new(dotfmt_core::diagnostic::DiagnosticKind::Syntax, "bad")
            .with_location(2, Some(2));
        let diagnostic = fatal(&uri, "ab\ncd\n", &error);
        assert_eq!(diagnostic.range.start, Position::new(1, 1));
        assert_eq!(diagnostic.range.end, Position::new(1, 2));
        assert_eq!(diagnostic.source.as_deref(), Some("dotfmt"));
    }

    fn harness(pull: bool) -> (Published, Arc<Client>, crossbeam_channel::Receiver<Message>) {
        harness_with_roots(pull, &[])
    }

    fn harness_with_roots(
        pull: bool,
        roots: &[&Path],
    ) -> (Published, Arc<Client>, crossbeam_channel::Receiver<Message>) {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let client = Arc::new(Client::new(sender));
        let workspace = Arc::new(Workspace::new(
            roots.iter().map(|root| root.to_path_buf()).collect(),
        ));
        (
            Published {
                pull,
                workspace,
                ..Published::default()
            },
            client,
            receiver,
        )
    }

    fn tsc(code: &str) -> Diagnostic {
        Diagnostic {
            code: Some(NumberOrString::String(code.into())),
            ..Diagnostic::default()
        }
    }

    fn legacy_uri(path: &Path) -> Uri {
        let text = path.to_string_lossy();
        let mut encoded = String::from("file://");
        for byte in text.bytes() {
            if byte.is_ascii_alphanumeric()
                || matches!(byte, b'/' | b':' | b'-' | b'_' | b'.' | b'~')
            {
                encoded.push(byte as char);
            } else {
                encoded.push_str(&format!("%{byte:02X}"));
            }
        }
        Uri::from_str(&encoded).unwrap()
    }

    #[test]
    fn special_paths_publish_under_the_open_client_uri() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let names = [
            "app/(dashboard)/page.tsx",
            "app/(.)photo/page.tsx",
            "app/@modal/page.tsx",
            "[slug]/page.tsx",
            "~ ! $ & ' * + , ; = # %/file.ts",
            "space folder/page.tsx",
            "üñïçødé/page.tsx",
        ];
        let mut entries = Vec::new();
        let mut opened = Vec::new();
        for name in names {
            let path = root.join(name);
            write(root, name, "");
            let canonical = uri::from_path(&path);
            let client = legacy_uri(&path);
            opened.push(client);
            entries.push((canonical, vec![tsc("TS2322")]));
        }
        let (mut published, client, receiver) = harness_with_roots(false, &[root]);
        {
            let mut documents = published
                .documents
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            for uri in &opened {
                documents.open(
                    uri.clone(),
                    Document::new("".into(), "typescript".into(), 1),
                );
            }
        }
        published.set_tsc(root, 1, entries, &client);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen = HashSet::new();
        while seen.len() < opened.len() {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .expect("timed out waiting for special-path diagnostics");
            let message = receiver
                .recv_timeout(remaining)
                .expect("diagnostics message");
            let Message::Notification(notification) = message else {
                continue;
            };
            if notification.method != "textDocument/publishDiagnostics" {
                continue;
            }
            let params: PublishDiagnosticsParams =
                serde_json::from_value(notification.params).unwrap();
            assert!(
                opened.contains(&params.uri),
                "published under a non-client URI: {}",
                params.uri.as_str()
            );
            assert_eq!(params.diagnostics.len(), 1);
            seen.insert(params.uri);
        }
        for uri in opened {
            assert_eq!(published.merged(&uri, None).diagnostics.len(), 1);
        }
    }

    #[test]
    fn a_config_error_under_a_route_group_stays_on_the_open_owner() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let path = root.join("app/(dashboard)/page.tsx");
        write(root, "app/(dashboard)/page.tsx", "");
        let owner = legacy_uri(&path);
        let error = Fatal::config(&path, 1, "bad configuration");
        let entries = fatal_entries(&owner, "", &[error]);
        assert_eq!(entries.len(), 1);
        assert_eq!(uri::key(&entries[0].0), uri::key(&owner));

        let (mut published, client, _receiver) = harness_with_roots(true, &[root]);
        published.set_dotfmt(&owner, 1, entries, &client);
        let merged = published.merged(&owner, None);
        assert_eq!(merged.diagnostics.len(), 1);
        assert_eq!(
            merged.diagnostics[0].code,
            Some(NumberOrString::String("configuration".into()))
        );
        assert_eq!(
            merged,
            published.merged(&uri::from_path(&path), None),
            "one path produced different pull reports"
        );
    }

    #[test]
    fn repeated_publishes_replace_the_previous_view() {
        let (mut published, client, receiver) = harness(false);
        let uri = Uri::from_str("file:///tmp/a.json").unwrap();
        published.set_dotfmt(
            &uri,
            1,
            vec![(uri.clone(), vec![Diagnostic::default()])],
            &client,
        );
        published.set_dotfmt(&uri, 2, vec![(uri.clone(), Vec::new())], &client);
        let _first = receiver.recv().unwrap();
        let second = receiver.recv().unwrap();
        let Message::Notification(notification) = second else {
            panic!("expected diagnostics notification");
        };
        let params: PublishDiagnosticsParams = serde_json::from_value(notification.params).unwrap();
        assert!(params.diagnostics.is_empty());
        assert_eq!(params.version, Some(2));
    }

    #[test]
    fn an_older_document_version_cannot_replace_a_newer_one() {
        let (mut published, client, receiver) = harness(false);
        let uri = Uri::from_str("file:///tmp/a.json").unwrap();
        published.set_dotfmt(&uri, 5, vec![(uri.clone(), Vec::new())], &client);
        let _ = receiver.recv().unwrap();
        published.set_dotfmt(
            &uri,
            4,
            vec![(uri.clone(), vec![Diagnostic::default()])],
            &client,
        );
        assert!(
            receiver.recv_timeout(Duration::from_millis(50)).is_err(),
            "stale result was published"
        );
    }

    #[test]
    fn a_shared_alias_survives_until_its_last_owner_drops_it() {
        let (mut published, client, _receiver) = harness(true);
        let a = Uri::from_str("file:///work/a.ts").unwrap();
        let b = Uri::from_str("file:///work/b.ts").unwrap();
        let dotfile = Uri::from_str("file:///work/dotfmt.dotfile").unwrap();
        let config_error = || vec![tsc("configuration")];
        published.set_dotfmt(
            &a,
            1,
            vec![(a.clone(), Vec::new()), (dotfile.clone(), config_error())],
            &client,
        );
        published.set_dotfmt(
            &b,
            1,
            vec![(b.clone(), Vec::new()), (dotfile.clone(), config_error())],
            &client,
        );
        published.set_dotfmt(&a, 2, vec![(a.clone(), Vec::new())], &client);
        assert_eq!(
            published.merged(&dotfile, None).diagnostics,
            config_error(),
            "one owner's republish removed another owner's alias"
        );
        published.set_dotfmt(&b, 2, vec![(b.clone(), Vec::new())], &client);
        assert!(published.merged(&dotfile, None).diagnostics.is_empty());
    }

    #[test]
    fn closing_one_owner_does_not_clear_another_owners_shared_alias() {
        let (mut published, client, _receiver) = harness(true);
        let a = Uri::from_str("file:///work/a.ts").unwrap();
        let b = Uri::from_str("file:///work/b.ts").unwrap();
        let dotfile = Uri::from_str("file:///work/dotfmt.dotfile").unwrap();
        let config_error = || vec![tsc("configuration")];
        published.set_dotfmt(
            &a,
            1,
            vec![(a.clone(), Vec::new()), (dotfile.clone(), config_error())],
            &client,
        );
        published.set_dotfmt(
            &b,
            1,
            vec![(b.clone(), Vec::new()), (dotfile.clone(), config_error())],
            &client,
        );
        published.clear_dotfmt(&a, &client);
        assert_eq!(
            published.merged(&dotfile, None).diagnostics,
            config_error(),
            "closing one owner removed another owner's shared alias"
        );
        published.clear_dotfmt(&b, &client);
        assert!(published.merged(&dotfile, None).diagnostics.is_empty());
    }

    #[test]
    fn tsc_entries_make_the_published_version_versionless() {
        let (mut published, client, receiver) = harness(false);
        let uri = Uri::from_str("file:///tmp/a.ts").unwrap();
        published.set_dotfmt(&uri, 3, vec![(uri.clone(), Vec::new())], &client);
        let _ = receiver.recv().unwrap();
        published.set_tsc(
            Path::new("/tmp"),
            1,
            vec![(uri.clone(), vec![Diagnostic::default()])],
            &client,
        );
        let message = receiver.recv().unwrap();
        let Message::Notification(notification) = message else {
            panic!("expected diagnostics notification");
        };
        let params: PublishDiagnosticsParams = serde_json::from_value(notification.params).unwrap();
        assert_eq!(params.version, None);
        assert_eq!(params.diagnostics.len(), 1);
    }

    #[test]
    fn overlapping_projects_do_not_duplicate_identical_tsc_diagnostics() {
        let (mut published, client, _receiver) = harness(false);
        let uri = Uri::from_str("file:///tmp/a.ts").unwrap();
        let entries = vec![(uri.clone(), vec![Diagnostic::default()])];
        published.set_tsc(
            Path::new("/tmp/tsconfig.app.json"),
            1,
            entries.clone(),
            &client,
        );
        published.set_tsc(Path::new("/tmp/tsconfig.node.json"), 1, entries, &client);
        assert_eq!(published.merged(&uri, None).diagnostics.len(), 1);
    }

    #[test]
    fn identical_tsc_sets_are_published_once() {
        let (mut published, client, receiver) = harness(false);
        let uri = Uri::from_str("file:///tmp/a.ts").unwrap();
        let entries = vec![(uri.clone(), vec![Diagnostic::default()])];
        published.set_tsc(Path::new("/tmp"), 1, entries.clone(), &client);
        let _ = receiver.recv().unwrap();
        published.set_tsc(Path::new("/tmp"), 2, entries, &client);
        assert!(
            receiver.recv_timeout(Duration::from_millis(100)).is_err(),
            "an identical tsc set was published twice"
        );
    }

    #[test]
    fn identical_pull_sets_do_not_ask_for_refresh() {
        let (mut published, client, receiver) = harness(true);
        let uri = Uri::from_str("file:///tmp/a.ts").unwrap();
        let entries = vec![(uri.clone(), vec![Diagnostic::default()])];
        published.set_tsc(Path::new("/tmp"), 1, entries.clone(), &client);
        let refresh = receiver.recv().unwrap();
        let Message::Request(request) = refresh else {
            panic!("expected refresh request");
        };
        assert_eq!(request.method, "workspace/diagnostic/refresh");
        published.set_tsc(Path::new("/tmp"), 2, entries, &client);
        assert!(
            receiver.recv_timeout(Duration::from_millis(100)).is_err(),
            "an identical pull set asked for a refresh"
        );
        assert!(
            !receiver.try_iter().any(|message| matches!(
                message,
                Message::Notification(notification)
                    if notification.method == "textDocument/publishDiagnostics"
            )),
            "pull mode published diagnostics"
        );
    }

    #[test]
    fn clearing_one_root_keeps_another_roots_entries() {
        let (mut published, client, receiver) = harness(false);
        let a = Uri::from_str("file:///work/a/a.ts").unwrap();
        let b = Uri::from_str("file:///work/b/b.ts").unwrap();
        published.set_tsc(
            Path::new("/work/a"),
            1,
            vec![(a.clone(), vec![Diagnostic::default()])],
            &client,
        );
        published.set_tsc(
            Path::new("/work/b"),
            1,
            vec![(b.clone(), vec![Diagnostic::default()])],
            &client,
        );
        let _ = receiver.recv().unwrap();
        let _ = receiver.recv().unwrap();
        published.clear_under(Path::new("/work/a"), &client);
        let message = receiver.recv().unwrap();
        let Message::Notification(notification) = message else {
            panic!("expected diagnostics notification");
        };
        let params: PublishDiagnosticsParams = serde_json::from_value(notification.params).unwrap();
        assert_eq!(params.uri, a);
        assert!(params.diagnostics.is_empty());
        assert!(
            receiver.recv_timeout(Duration::from_millis(100)).is_err(),
            "clearing one root published for another"
        );
        assert_eq!(
            published.merged(&b, None).diagnostics,
            vec![Diagnostic::default()]
        );
    }

    #[test]
    fn overlapping_projects_merge_and_replace_independently() {
        let (mut published, client, _receiver) = harness(true);
        let uri = Uri::from_str("file:///work/shared/a.ts").unwrap();
        published.set_tsc(
            Path::new("/work/pkg-a"),
            1,
            vec![(uri.clone(), vec![tsc("TS1001")])],
            &client,
        );
        published.set_tsc(
            Path::new("/work/pkg-b"),
            1,
            vec![(uri.clone(), vec![tsc("TS1002")])],
            &client,
        );
        let merged = published.merged(&uri, None);
        assert_eq!(
            merged
                .diagnostics
                .iter()
                .filter_map(|diagnostic| match &diagnostic.code {
                    Some(NumberOrString::String(code)) => Some(code.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            vec!["TS1001", "TS1002"]
        );

        published.set_tsc(Path::new("/work/pkg-a"), 2, Vec::new(), &client);
        assert_eq!(
            published.merged(&uri, None).diagnostics,
            vec![tsc("TS1002")]
        );
        published.clear_tsc_uri(&uri, &client);
        assert!(published.merged(&uri, None).diagnostics.is_empty());
    }

    #[test]
    fn clearing_a_parent_root_keeps_nested_root_projects() {
        let (mut published, client, _receiver) =
            harness_with_roots(true, &[Path::new("/work"), Path::new("/work/app")]);
        let outer = Uri::from_str("file:///work/pkg/a.ts").unwrap();
        let nested = Uri::from_str("file:///work/app/pkg/a.ts").unwrap();
        published.set_dotfmt(
            &outer,
            1,
            vec![(outer.clone(), vec![tsc("syntax-outer")])],
            &client,
        );
        published.set_dotfmt(
            &nested,
            1,
            vec![(nested.clone(), vec![tsc("syntax-nested")])],
            &client,
        );
        published.set_tsc(
            Path::new("/work/pkg"),
            1,
            vec![(outer.clone(), vec![tsc("TS2001")])],
            &client,
        );
        published.set_tsc(
            Path::new("/work/app/pkg"),
            1,
            vec![(nested.clone(), vec![tsc("TS2002")])],
            &client,
        );
        published.clear_under(Path::new("/work"), &client);
        assert!(published.merged(&outer, None).diagnostics.is_empty());
        assert_eq!(
            published.merged(&nested, None).diagnostics,
            vec![tsc("syntax-nested"), tsc("TS2002")]
        );
        published.clear_tsc_under(Path::new("/work/app"), &client);
        assert_eq!(
            published.merged(&nested, None).diagnostics,
            vec![tsc("syntax-nested")]
        );
    }

    #[test]
    fn merged_reports_include_both_sources_and_a_stable_result_id() {
        let (mut published, client, _receiver) = harness(true);
        let uri = Uri::from_str("file:///work/a.ts").unwrap();
        published.set_dotfmt(
            &uri,
            4,
            vec![(uri.clone(), vec![Diagnostic::default()])],
            &client,
        );
        published.set_tsc(
            Path::new("/work"),
            1,
            vec![(uri.clone(), vec![Diagnostic::default()])],
            &client,
        );
        let merged = published.merged(&uri, Some(4));
        assert_eq!(merged.diagnostics.len(), 2);
        assert_eq!(merged, published.merged(&uri, Some(4)));
        assert_ne!(merged.result_id, published.merged(&uri, Some(5)).result_id);

        let other = Uri::from_str("file:///work/b.ts").unwrap();
        published.set_dotfmt(
            &other,
            4,
            vec![(other.clone(), vec![Diagnostic::default()])],
            &client,
        );
        published.set_tsc(
            Path::new("/work/other"),
            1,
            vec![(other.clone(), vec![Diagnostic::default()])],
            &client,
        );
        let other_merged = published.merged(&other, Some(4));
        assert_eq!(other_merged.diagnostics, merged.diagnostics);
        assert_ne!(
            other_merged.result_id, merged.result_id,
            "the result id ignored the path identity"
        );
    }

    #[test]
    fn pull_reports_dotfmt_configuration_errors_as_related_documents() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        write(root, "src/a.ts", "export const a = 1;\n");
        write(root, "dotfmt.dotfile", "typescript {}\n");
        let source = uri::from_path(&root.join("src/a.ts"));
        let config = uri::from_path(&root.join("dotfmt.dotfile"));
        let (mut published, client, _receiver) = harness_with_roots(true, &[root]);

        let first = Fatal::config(root.join("dotfmt.dotfile"), 1, "bad configuration");
        let entries = fatal_entries(&source, "export const a = 1;\n", &[first]);
        published.set_dotfmt(&source, 1, entries, &client);
        let merged = published.merged(&source, None);
        assert!(merged.diagnostics.is_empty());
        assert_eq!(merged.related_documents.len(), 1);
        assert_eq!(merged.related_documents[0].0, config);
        assert_eq!(
            merged.related_documents[0].1[0].code,
            Some(NumberOrString::String("configuration".into()))
        );
        let first_id = merged.result_id;

        let second = Fatal::config(root.join("dotfmt.dotfile"), 1, "different error");
        let entries = fatal_entries(&source, "export const a = 1;\n", &[second]);
        published.set_dotfmt(&source, 2, entries, &client);
        let changed = published.merged(&source, None);
        assert_eq!(changed.related_documents.len(), 1);
        assert_ne!(
            changed.result_id, first_id,
            "a changed configuration error did not change the result id"
        );
    }

    #[test]
    fn pull_reports_tsconfig_errors_as_related_documents() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        write(root, "src/a.ts", "export const a = 1;\n");
        write(root, "tsconfig.json", "{}\n");
        let source = uri::from_path(&root.join("src/a.ts"));
        let config = uri::from_path(&root.join("tsconfig.json"));
        let (mut published, client, _receiver) = harness_with_roots(true, &[root]);
        let diagnostic = tsc("TS18003");

        published.set_tsc(
            &root.join("tsconfig.json"),
            1,
            vec![(config.clone(), vec![diagnostic])],
            &client,
        );
        let merged = published.merged(&source, None);
        assert!(merged.diagnostics.is_empty());
        assert_eq!(merged.related_documents.len(), 1);
        assert_eq!(merged.related_documents[0].0, config);
        assert_eq!(
            merged.related_documents[0].1[0].code,
            Some(NumberOrString::String("TS18003".into()))
        );
        let first_id = merged.result_id;

        published.set_tsc(
            &root.join("tsconfig.json"),
            2,
            vec![(config, vec![tsc("TS18003"), tsc("TS5053")])],
            &client,
        );
        let changed = published.merged(&source, None);
        assert_eq!(changed.related_documents[0].1.len(), 2);
        assert_ne!(
            changed.result_id, first_id,
            "a changed tsconfig error did not change the result id"
        );
    }

    #[test]
    fn a_stale_buffer_version_is_not_reported_by_pull() {
        let (mut published, client, _receiver) = harness(true);
        let uri = Uri::from_str("file:///work/a.ts").unwrap();
        published.set_dotfmt(
            &uri,
            4,
            vec![(uri.clone(), vec![Diagnostic::default()])],
            &client,
        );
        assert!(published.merged(&uri, Some(5)).diagnostics.is_empty());
        assert_eq!(published.merged(&uri, Some(4)).diagnostics.len(), 1);
    }

    #[test]
    fn no_lint_findings_are_published_from_a_typescript_buffer() {
        let directory = tempfile::tempdir().unwrap();
        let text = "import { unused } from \"./mod\";\n\
                    import type { Only } from \"./mod\";\n\
                    function f(unusedParam: number) {}\n\
                    export const value = 1;\n";
        let diagnostics = inspect_ts(directory.path(), text);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn project_unused_exports_are_not_lsp_diagnostics() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(root, "package.json", "{\"private\":true}\n");
        let text = "export const consumed = 1;\nexport const orphan = 2;\n";
        write(
            root,
            "src/use.ts",
            "import { consumed } from \"./a\";\nconsole.log(consumed);\n",
        );
        let diagnostics = inspect_ts(root, text);
        assert!(
            diagnostics.iter().all(|diagnostic| {
                diagnostic.code != Some(NumberOrString::String("unused-export".into()))
            }),
            "{diagnostics:?}"
        );
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn syntax_errors_remain_error_diagnostics() {
        let directory = tempfile::tempdir().unwrap();
        let diagnostics = inspect_ts(directory.path(), "const = ;\n");
        assert!(!diagnostics.is_empty());
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.severity == Some(DiagnosticSeverity::ERROR))
        );
    }

    #[test]
    fn disabled_syntax_diagnostics_publish_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(root, "dotfmt.dotfile", "typescript {}\n");
        let text = "const = ;\n";
        let path = root.join("src/a.ts");
        write(root, "src/a.ts", text);
        let uri = uri::from_path(&path);
        let document = Document::new(text.into(), "typescript".into(), 1);
        let workspace = Workspace::new(vec![root.to_path_buf()]);
        let documents = Mutex::new(Documents::default());
        documents.lock().unwrap().open(uri.clone(), document);
        let settings = SettingsStore::new(
            false,
            Settings {
                syntax_diagnostics: false,
                ..Settings::default()
            },
        );
        let mut job = scheduled(uri.clone(), path, text, 1);
        job.settings.syntax_diagnostics = false;
        let entries = inspect(&workspace, &documents, &settings, &job).expect("inspection");
        assert_eq!(entries, vec![(uri, Vec::new())]);
    }

    #[test]
    fn a_stale_settings_snapshot_is_dropped() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(root, "dotfmt.dotfile", "typescript {}\n");
        let text = "const = ;\n";
        let path = root.join("src/a.ts");
        write(root, "src/a.ts", text);
        let uri = uri::from_path(&path);
        let document = Document::new(text.into(), "typescript".into(), 1);
        let workspace = Workspace::new(vec![root.to_path_buf()]);
        let documents = Mutex::new(Documents::default());
        documents.lock().unwrap().open(uri.clone(), document);
        let settings = SettingsStore::new(
            false,
            Settings {
                syntax_diagnostics: false,
                ..Settings::default()
            },
        );
        let job = scheduled(uri, path, text, 1);
        assert!(inspect(&workspace, &documents, &settings, &job).is_none());
    }

    #[test]
    fn per_folder_settings_are_used_by_the_worker() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        write(root, "dotfmt.dotfile", "typescript {}\n");
        let text = "const = ;\n";
        let path = root.join("src/a.ts");
        write(root, "src/a.ts", text);
        let uri = uri::from_path(&path);
        let document = Document::new(text.into(), "typescript".into(), 1);
        let workspace = Workspace::new(vec![root.to_path_buf()]);
        let documents = Mutex::new(Documents::default());
        documents.lock().unwrap().open(uri.clone(), document);
        let store = SettingsStore::new(false, Settings::default());
        store.configure_folder(
            root,
            &serde_json::json!({ "dotfmt.diagnostics.syntax": false }),
        );
        let mut job = scheduled(uri, path, text, 1);
        job.settings.syntax_diagnostics = false;
        assert!(inspect(&workspace, &documents, &store, &job).is_some());
    }
}
