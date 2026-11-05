//! Debounced diagnostics and the per-URI view the editor receives.
//!
//! dotfmt publishes errors only. The worker drops every non-`Error` finding
//! from [`crate::analysis::inspect_errors`]; lint findings stay available to
//! code actions and `dotfmt --diagnose`. Syntax/config errors are buffer
//! derived, `tsc` entries describe disk files and are therefore versionless.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
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
use crate::documents::{Document, Documents};
use crate::settings::Settings;
use crate::uri;
use crate::workspace::Workspace;

pub struct Diagnostics {
    pending: crossbeam_channel::Sender<Job>,
    published: Arc<Mutex<Published>>,
    sender: crossbeam_channel::Sender<Message>,
    worker: Option<JoinHandle<()>>,
}

enum Job {
    Schedule(Scheduled),
    Clear(Uri),
    ClearAll,
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
        sender: crossbeam_channel::Sender<Message>,
        settings: Arc<RwLock<Settings>>,
    ) -> Self {
        let (pending, jobs) = crossbeam_channel::unbounded();
        let published = Arc::new(Mutex::new(Published::default()));
        let worker = {
            let published = Arc::clone(&published);
            let settings = Arc::clone(&settings);
            let sender = sender.clone();
            thread::Builder::new()
                .name("dotfmt-lsp-diagnostics".into())
                .spawn(move || run(jobs, workspace, documents, settings, published, sender))
                .ok()
        };
        Self {
            pending,
            published,
            sender,
            worker,
        }
    }

    pub fn schedule(&self, document: &Document, path: PathBuf, uri: Uri, settings: &Settings) {
        let _ = self.pending.send(Job::Schedule(Scheduled {
            uri,
            version: document.version,
            path,
            language_id: document.language().to_owned(),
            text: document.text().to_owned(),
            delay: Duration::from_millis(settings.debounce_ms),
            settings: settings.clone(),
        }));
    }

    pub fn schedule_all(&self, documents: &Documents, settings: &Settings) {
        for (uri, document) in documents.iter() {
            if let Some(path) = analysis::buffer_path(uri, document.language()) {
                self.schedule(document, path, uri.clone(), settings);
            }
        }
    }

    pub fn clear(&self, uri: Uri) {
        let _ = self.pending.send(Job::Clear(uri));
    }

    pub fn clear_all(&self) {
        let _ = self.pending.send(Job::ClearAll);
    }

    /// Drops disk-derived `tsc` entries for one URI, for example after an edit.
    pub fn clear_tsc_uri(&self, uri: Uri) {
        let _ = self.pending.send(Job::ClearTscUri(uri));
    }

    pub fn publish_fatal(&self, uri: Uri, text: &str, version: i32, errors: &[Fatal]) {
        let entries = fatal_entries(&uri, text, errors);
        let mut published = self
            .published
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        published.set_dotfmt(&uri, version, entries, &self.sender);
    }

    pub fn set_tsc(&self, project: &Path, generation: u64, entries: Vec<(Uri, Vec<Diagnostic>)>) {
        let mut published = self
            .published
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        published.set_tsc(project, generation, entries, &self.sender);
    }

    pub fn clear_tsc(&self) {
        let mut published = self
            .published
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        published.clear_tsc(&self.sender);
    }

    /// Logs to the editor's output channel; syntax failures never use popups.
    pub fn log(&self, message: impl Into<String>) {
        let _ = self.sender.send(Message::Notification(Notification::new(
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
    settings: Arc<RwLock<Settings>>,
    published: Arc<Mutex<Published>>,
    sender: crossbeam_channel::Sender<Message>,
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
                published.set_dotfmt(&job.uri, job.version, entries, &sender);
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
                published.clear_dotfmt(&uri, &sender);
            }
            Ok(Job::ClearAll) => {
                pending.clear();
                let mut published = published.lock().unwrap_or_else(|error| error.into_inner());
                published.clear_all_dotfmt(&sender);
            }
            Ok(Job::ClearTscUri(uri)) => {
                let mut published = published.lock().unwrap_or_else(|error| error.into_inner());
                published.clear_tsc_uri(&uri, &sender);
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
    settings: &RwLock<Settings>,
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
    // A configuration change reschedules every document. Drop a result that
    // was produced from a snapshot the user has already replaced.
    let latest = settings
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    if latest != job.settings {
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

#[derive(Default)]
struct Published {
    dotfmt: HashMap<Uri, Entry>,
    owners: HashMap<Uri, Vec<Uri>>,
    tsc: HashMap<Uri, Vec<Diagnostic>>,
    projects: HashMap<PathBuf, TscProject>,
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
        sender: &crossbeam_channel::Sender<Message>,
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
        publish(&affected, &self.dotfmt, &self.tsc, sender, true);
    }

    fn clear_dotfmt(&mut self, owner: &Uri, sender: &crossbeam_channel::Sender<Message>) {
        let mut affected = self.owners.remove(owner).unwrap_or_default();
        affected.push(owner.clone());
        for uri in &affected {
            self.dotfmt.remove(uri);
        }
        publish(&affected, &self.dotfmt, &self.tsc, sender, true);
    }

    fn clear_all_dotfmt(&mut self, sender: &crossbeam_channel::Sender<Message>) {
        let mut affected: Vec<Uri> = self.dotfmt.keys().cloned().collect();
        affected.extend(self.owners.keys().cloned());
        self.dotfmt.clear();
        self.owners.clear();
        publish(&affected, &self.dotfmt, &self.tsc, sender, true);
    }

    fn set_tsc(
        &mut self,
        project: &Path,
        generation: u64,
        entries: Vec<(Uri, Vec<Diagnostic>)>,
        sender: &crossbeam_channel::Sender<Message>,
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
        publish(&affected, &self.dotfmt, &self.tsc, sender, true);
    }

    fn clear_tsc_uri(&mut self, uri: &Uri, sender: &crossbeam_channel::Sender<Message>) {
        self.tsc.remove(uri);
        for project in self.projects.values_mut() {
            project.uris.retain(|owned| owned != uri);
        }
        // A compiler clear is not tied to a document version; a versioned
        // notification could be discarded as stale and leave tsc errors behind.
        publish(
            std::slice::from_ref(uri),
            &self.dotfmt,
            &self.tsc,
            sender,
            false,
        );
    }

    fn clear_tsc(&mut self, sender: &crossbeam_channel::Sender<Message>) {
        let mut affected: Vec<Uri> = self.tsc.keys().cloned().collect();
        affected.extend(self.projects.values().flat_map(|project| project.uris.iter().cloned()));
        self.tsc.clear();
        self.projects.clear();
        publish(&affected, &self.dotfmt, &self.tsc, sender, true);
    }
}

fn publish(
    affected: &[Uri],
    dotfmt: &HashMap<Uri, Entry>,
    tsc: &HashMap<Uri, Vec<Diagnostic>>,
    sender: &crossbeam_channel::Sender<Message>,
    stamp_version: bool,
) {
    let mut seen = HashSet::new();
    for uri in affected {
        if !seen.insert(uri) {
            continue;
        }
        let dotfmt_entry = dotfmt.get(uri);
        let tsc_entries = tsc.get(uri);
        let mut diagnostics = dotfmt_entry
            .map(|entry| entry.diagnostics.clone())
            .unwrap_or_default();
        if let Some(tsc) = tsc_entries {
            diagnostics.extend(tsc.iter().cloned());
        }
        // `tsc` results describe the file on disk and carry no document
        // version. Never stamp the merged notification with a buffer version.
        let version = match (dotfmt_entry, tsc_entries.filter(|entries| !entries.is_empty())) {
            (Some(entry), None) if stamp_version => entry.version,
            _ => None,
        };
        let notification = Notification::new(
            PublishDiagnostics::METHOD.into(),
            PublishDiagnosticsParams::new(uri.clone(), diagnostics, version),
        );
        let _ = sender.send(Message::Notification(notification));
    }
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
        let settings = RwLock::new(Settings::default());
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

    #[test]
    fn repeated_publishes_replace_the_previous_view() {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let mut published = Published::default();
        let uri = Uri::from_str("file:///tmp/a.json").unwrap();
        published.set_dotfmt(
            &uri,
            1,
            vec![(uri.clone(), vec![Diagnostic::default()])],
            &sender,
        );
        published.set_dotfmt(&uri, 2, vec![(uri.clone(), Vec::new())], &sender);
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
        let (sender, receiver) = crossbeam_channel::unbounded();
        let mut published = Published::default();
        let uri = Uri::from_str("file:///tmp/a.json").unwrap();
        published.set_dotfmt(&uri, 5, vec![(uri.clone(), Vec::new())], &sender);
        let _ = receiver.recv().unwrap();
        published.set_dotfmt(
            &uri,
            4,
            vec![(uri.clone(), vec![Diagnostic::default()])],
            &sender,
        );
        assert!(
            receiver.recv_timeout(Duration::from_millis(50)).is_err(),
            "stale result was published"
        );
    }

    #[test]
    fn tsc_entries_make_the_published_version_versionless() {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let mut published = Published::default();
        let uri = Uri::from_str("file:///tmp/a.ts").unwrap();
        published.set_dotfmt(&uri, 3, vec![(uri.clone(), Vec::new())], &sender);
        let _ = receiver.recv().unwrap();
        published.set_tsc(
            Path::new("/tmp"),
            1,
            vec![(uri.clone(), vec![Diagnostic::default()])],
            &sender,
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
                diagnostic.code
                    != Some(NumberOrString::String("unused-export".into()))
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
        let settings = RwLock::new(Settings {
            syntax_diagnostics: false,
            ..Settings::default()
        });
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
        let settings = RwLock::new(Settings {
            syntax_diagnostics: false,
            ..Settings::default()
        });
        let job = scheduled(uri, path, text, 1);
        assert!(inspect(&workspace, &documents, &settings, &job).is_none());
    }
}
