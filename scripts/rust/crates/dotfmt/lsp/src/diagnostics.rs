//! Debounced diagnostics and the per-URI view the editor receives.
//!
//! dotfmt reports errors only. The worker drops every non-`Error` finding
//! from [`crate::analysis::inspect_errors`]; lint findings stay available to
//! code actions and `dotfmt --diagnose`. Syntax/config errors are buffer
//! derived, `tsc` entries describe disk files and are therefore versionless.
//!
//! Clients with pull diagnostics never receive `publishDiagnostics`; the
//! store is the single source of truth, `workspace/diagnostic/refresh` asks
//! the client to pull again, and identical sets are suppressed.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
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
use crate::uri;
use crate::workspace::Workspace;

/// A merged dotfmt + tsc view for one URI, with a stable result id.
#[derive(Debug, PartialEq)]
pub struct Merged {
    pub diagnostics: Vec<Diagnostic>,
    pub result_id: String,
}

pub struct Diagnostics {
    pending: crossbeam_channel::Sender<Job>,
    published: Arc<Mutex<Published>>,
    client: Arc<Client>,
    settings: Arc<SettingsStore>,
    worker: Option<JoinHandle<()>>,
}

enum Job {
    Schedule(Scheduled),
    Clear(Uri),
    ClearAll,
    ClearRoot(PathBuf),
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
        let published = Arc::new(Mutex::new(Published {
            pull,
            ..Published::default()
        }));
        let worker = {
            let published = Arc::clone(&published);
            let settings = Arc::clone(&settings);
            let client = Arc::clone(&client);
            thread::Builder::new()
                .name("dotfmt-lsp-diagnostics".into())
                .spawn(move || run(jobs, workspace, documents, settings, published, client))
                .ok()
        };
        Self {
            pending,
            published,
            client,
            settings,
            worker,
        }
    }

    /// Schedules one buffer with the settings of the folder that contains it.
    pub fn schedule(&self, document: &Document, path: PathBuf, uri: Uri) {
        let settings = self.settings.settings_for(&path);
        let _ = self.pending.send(Job::Schedule(Scheduled {
            uri,
            version: document.version,
            path,
            language_id: document.language().to_owned(),
            text: document.text().to_owned(),
            delay: Duration::from_millis(settings.debounce_ms),
            settings,
        }));
    }

    pub fn schedule_all(&self, documents: &Documents) {
        for (uri, document) in documents.iter() {
            if let Some(path) = analysis::buffer_path(uri, document.language()) {
                self.schedule(document, path, uri.clone());
            }
        }
    }

    pub fn clear(&self, uri: Uri) {
        let _ = self.pending.send(Job::Clear(uri));
    }

    pub fn clear_all(&self) {
        let _ = self.pending.send(Job::ClearAll);
    }

    /// Drops every dotfmt and tsc entry for one workspace folder.
    pub fn clear_under(&self, root: PathBuf) {
        let _ = self.pending.send(Job::ClearRoot(root));
    }

    /// Drops disk-derived `tsc` entries for one URI, for example after an edit.
    pub fn clear_tsc_uri(&self, uri: Uri) {
        let _ = self.pending.send(Job::ClearTscUri(uri));
    }

    /// Drops disk-derived `tsc` entries for one workspace folder.
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

    /// The merged dotfmt + tsc view for a pull request.
    ///
    /// `version` is the current buffer version; buffer diagnostics produced
    /// for an older revision are dropped rather than reported as current.
    pub fn merged(&self, uri: &Uri, version: Option<i32>) -> Merged {
        self.published
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .merged(uri, version)
    }

    /// Logs to the editor's output channel; syntax failures never use popups.
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
) {
    let mut pending: HashMap<Uri, (Instant, Scheduled)> = HashMap::new();
    loop {
        let now = Instant::now();
        let due: Vec<Uri> = pending
            .iter()
            .filter(|(_, (at, _))| *at <= now)
            .map(|(uri, _)| uri.clone())
            .collect();
        for uri in due {
            let Some((_, job)) = pending.remove(&uri) else {
                continue;
            };
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
                let at = Instant::now() + job.delay;
                pending.insert(job.uri.clone(), (at, job));
            }
            Ok(Job::Clear(uri)) => {
                pending.remove(&uri);
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
    // A configuration change reschedules every affected document. Drop a
    // result that was produced from a snapshot the user has already replaced.
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

/// Converts findings in one buffer to LSP diagnostics for code actions.
///
/// The diagnostics publish path filters findings to [`Severity::Error`] before
/// calling this. Hints are never mapped to `DiagnosticSeverity::HINT`.
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

/// Converts fatal parse/configuration errors, keeping their line and byte column.
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
    let mut grouped: HashMap<Uri, Vec<Diagnostic>> = HashMap::new();
    for error in errors {
        let (target, target_text) = error
            .path
            .as_deref()
            .map(|path| {
                let target = uri::from_path(path);
                let contents = std::fs::read_to_string(path).unwrap_or_default();
                (target, contents)
            })
            .filter(|(target, _)| target != uri)
            .unwrap_or_else(|| (uri.clone(), text.to_owned()));
        grouped
            .entry(target.clone())
            .or_default()
            .push(fatal(&target, &target_text, error));
    }
    grouped.into_iter().collect()
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
    /// `Some` only when every diagnostic in the entry is buffer derived.
    version: Option<i32>,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, PartialEq)]
struct Sent {
    version: Option<i32>,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Default)]
struct Published {
    /// Pull clients re-request diagnostics; pushing would duplicate the set.
    pull: bool,
    dotfmt: HashMap<Uri, Entry>,
    owners: HashMap<Uri, Vec<Uri>>,
    tsc: HashMap<Uri, Vec<Diagnostic>>,
    projects: HashMap<PathBuf, TscProject>,
    /// The last set sent to (or pulled by) the client, for diff suppression.
    last: HashMap<Uri, Sent>,
}

struct TscProject {
    generation: u64,
    uris: Vec<Uri>,
}

impl Published {
    fn set_dotfmt(
        &mut self,
        owner: &Uri,
        version: i32,
        entries: Vec<(Uri, Vec<Diagnostic>)>,
        client: &Client,
    ) {
        if self
            .dotfmt
            .get(owner)
            .and_then(|entry| entry.version)
            .is_some_and(|stored| stored > version)
        {
            return;
        }
        let mut affected = self.owners.remove(owner).unwrap_or_default();
        affected.push(owner.clone());
        for uri in &affected {
            self.dotfmt.remove(uri);
        }
        let mut aliases = Vec::new();
        let mut includes_owner = false;
        for (uri, diagnostics) in entries {
            aliases.push(uri.clone());
            affected.push(uri.clone());
            let is_owner = uri == *owner;
            includes_owner |= is_owner;
            if is_owner || !diagnostics.is_empty() {
                self.dotfmt.insert(
                    uri,
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
        self.owners.insert(owner.clone(), aliases);
        self.publish(&affected, client);
    }

    fn clear_dotfmt(&mut self, owner: &Uri, client: &Client) {
        let mut affected = self.owners.remove(owner).unwrap_or_default();
        affected.push(owner.clone());
        for uri in &affected {
            self.dotfmt.remove(uri);
        }
        self.publish(&affected, client);
    }

    fn clear_all(&mut self, client: &Client) {
        let mut affected: Vec<Uri> = self.dotfmt.keys().cloned().collect();
        affected.extend(self.owners.keys().cloned());
        affected.extend(self.last.keys().cloned());
        self.dotfmt.clear();
        self.owners.clear();
        self.tsc.clear();
        self.projects.clear();
        self.publish(&affected, client);
    }

    fn clear_under(&mut self, root: &Path, client: &Client) {
        let mut affected = Vec::new();
        self.dotfmt.retain(|uri, _| {
            let under = is_under(uri, root);
            if under {
                affected.push(uri.clone());
            }
            !under
        });
        self.owners.retain(|owner, aliases| {
            let under = is_under(owner, root);
            if under {
                affected.push(owner.clone());
                affected.extend(aliases.iter().cloned());
            }
            !under
        });
        self.tsc.retain(|uri, _| {
            let under = is_under(uri, root);
            if under {
                affected.push(uri.clone());
            }
            !under
        });
        self.projects.retain(|project, project_entries| {
            let under = project.starts_with(root);
            if under {
                affected.extend(project_entries.uris.iter().cloned());
            }
            !under
        });
        self.last.retain(|uri, _| {
            let under = is_under(uri, root);
            if under {
                affected.push(uri.clone());
            }
            !under
        });
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
        let mut affected = self
            .projects
            .remove(project)
            .map(|project| project.uris)
            .unwrap_or_default();
        for uri in &affected {
            self.tsc.remove(uri);
        }
        let mut owned = Vec::new();
        for (uri, diagnostics) in entries {
            owned.push(uri.clone());
            affected.push(uri.clone());
            if !diagnostics.is_empty() {
                self.tsc.insert(uri, diagnostics);
            }
        }
        self.projects.insert(
            project.to_path_buf(),
            TscProject {
                generation,
                uris: owned,
            },
        );
        self.publish(&affected, client);
    }

    fn clear_tsc_uri(&mut self, uri: &Uri, client: &Client) {
        self.tsc.remove(uri);
        for project in self.projects.values_mut() {
            project.uris.retain(|owned| owned != uri);
        }
        self.publish(std::slice::from_ref(uri), client);
    }

    fn clear_tsc_under(&mut self, root: &Path, client: &Client) {
        let mut affected = Vec::new();
        self.tsc.retain(|uri, _| {
            let under = is_under(uri, root);
            if under {
                affected.push(uri.clone());
            }
            !under
        });
        self.projects.retain(|project, project_entries| {
            let under = project.starts_with(root);
            if under {
                affected.extend(project_entries.uris.iter().cloned());
            }
            !under
        });
        self.publish(&affected, client);
    }

    fn clear_tsc(&mut self, client: &Client) {
        let mut affected: Vec<Uri> = self.tsc.keys().cloned().collect();
        affected.extend(
            self.projects
                .values()
                .flat_map(|project| project.uris.iter().cloned()),
        );
        self.tsc.clear();
        self.projects.clear();
        self.publish(&affected, client);
    }

    fn merged(&self, uri: &Uri, version: Option<i32>) -> Merged {
        let dotfmt_entry = self.dotfmt.get(uri).filter(|entry| {
            !matches!(
                (entry.version, version),
                (Some(stored), Some(current)) if stored != current
            )
        });
        let mut diagnostics = dotfmt_entry
            .map(|entry| entry.diagnostics.clone())
            .unwrap_or_default();
        if let Some(tsc) = self.tsc.get(uri) {
            diagnostics.extend(tsc.iter().cloned());
        }
        let mut hasher = DefaultHasher::new();
        serde_json::to_string(&diagnostics)
            .unwrap_or_default()
            .hash(&mut hasher);
        Merged {
            result_id: format!("{}:{:x}", version.unwrap_or(-1), hasher.finish()),
            diagnostics,
        }
    }

    /// Sends one notification per URI whose set changed, or asks a pull client
    /// to refresh when any merged set changed.
    fn publish(&mut self, affected: &[Uri], client: &Client) {
        let mut seen = HashSet::new();
        let mut changed = false;
        let mut notifications = Vec::new();
        for uri in affected {
            if !seen.insert(uri.clone()) {
                continue;
            }
            let dotfmt_entry = self.dotfmt.get(uri);
            let tsc_entries = self.tsc.get(uri).filter(|entries| !entries.is_empty());
            let mut diagnostics = dotfmt_entry
                .map(|entry| entry.diagnostics.clone())
                .unwrap_or_default();
            if let Some(tsc) = tsc_entries {
                diagnostics.extend(tsc.iter().cloned());
            }
            // `tsc` results describe the file on disk and carry no document
            // version. Never stamp the merged notification with a buffer version.
            let version = match (dotfmt_entry, tsc_entries) {
                (Some(entry), None) => entry.version,
                _ => None,
            };
            let sent = Sent {
                version,
                diagnostics,
            };
            if self.last.get(uri) == Some(&sent) {
                continue;
            }
            changed = true;
            notifications.push((uri.clone(), sent.clone()));
            self.last.insert(uri.clone(), sent);
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

fn is_under(uri: &Uri, root: &Path) -> bool {
    uri::path(uri).is_some_and(|path| path.starts_with(root))
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
            path,
            language_id: "typescript".into(),
            text: text.into(),
            delay: Duration::from_millis(0),
            settings: Settings::default(),
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
        let (sender, receiver) = crossbeam_channel::unbounded();
        let client = Arc::new(Client::new(sender));
        (
            Published {
                pull,
                ..Published::default()
            },
            client,
            receiver,
        )
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
        assert_eq!(published.tsc.get(&b).map(Vec::len), Some(1));
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
