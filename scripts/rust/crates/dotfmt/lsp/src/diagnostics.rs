//! Debounced diagnostics and the per-URI view the editor receives.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use dotfmt::app::FixMode;
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
use crate::uri;
use crate::workspace::Workspace;

const DEBOUNCE: Duration = Duration::from_millis(200);

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
    Shutdown,
}

struct Scheduled {
    uri: Uri,
    version: Option<i32>,
    path: PathBuf,
    language_id: String,
    text: String,
    delay: Duration,
}

impl Diagnostics {
    pub fn new(
        workspace: Arc<Workspace>,
        documents: Arc<Mutex<Documents>>,
        sender: crossbeam_channel::Sender<Message>,
    ) -> Self {
        let (pending, jobs) = crossbeam_channel::unbounded();
        let published = Arc::new(Mutex::new(Published::default()));
        let worker = {
            let published = Arc::clone(&published);
            let sender = sender.clone();
            thread::Builder::new()
                .name("dotfmt-lsp-diagnostics".into())
                .spawn(move || run(jobs, workspace, documents, published, sender))
                .ok()
        };
        Self {
            pending,
            published,
            sender,
            worker,
        }
    }

    pub fn schedule(&self, document: &Document, path: PathBuf, uri: Uri) {
        let _ = self.pending.send(Job::Schedule(Scheduled {
            uri,
            version: Some(document.version),
            path,
            language_id: document.language().to_owned(),
            text: document.text().to_owned(),
            delay: DEBOUNCE,
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

    pub fn publish_fatal(&self, uri: Uri, text: &str, errors: &[Fatal]) {
        let entries = fatal_entries(&uri, text, errors);
        let mut published = self
            .published
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        published.set_dotfmt(&uri, entries, &self.sender);
    }

    pub fn set_tsc(&self, project: &Path, entries: Vec<(Uri, Vec<Diagnostic>)>) {
        let mut published = self
            .published
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        published.set_tsc(project, entries, &self.sender);
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
            if let Some(entries) = inspect(&workspace, &documents, &job) {
                let mut published = published.lock().unwrap_or_else(|error| error.into_inner());
                published.set_dotfmt(&job.uri, entries, &sender);
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
            Ok(Job::Shutdown) => break,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn inspect(
    workspace: &Workspace,
    documents: &Mutex<Documents>,
    job: &Scheduled,
) -> Option<Vec<(Uri, Vec<Diagnostic>)>> {
    let current = documents
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&job.uri)
        .map(|document| document.version);
    if current != job.version {
        return None;
    }
    let session = workspace.session(&job.path);
    let mut session = session.lock().unwrap_or_else(|error| error.into_inner());
    let languages = analysis::forced(&job.path, &job.uri, &job.language_id);
    match analysis::inspect(&mut session, &job.path, &job.text, languages, FixMode::None) {
        Ok(inspection) => {
            let findings = findings(&job.uri, &job.text, &inspection.findings);
            Some(if findings.is_empty() {
                vec![(job.uri.clone(), Vec::new())]
            } else {
                vec![(job.uri.clone(), findings)]
            })
        }
        Err(errors) => Some(fatal_entries(&job.uri, &job.text, &errors)),
    }
}

/// Converts findings in one buffer to LSP diagnostics.
pub fn findings(uri: &Uri, text: &str, findings: &[Finding]) -> Vec<Diagnostic> {
    let index = LineIndex::new(text);
    findings
        .iter()
        .map(|finding| {
            let start = clamp(text, finding.start);
            let end = clamp(text, finding.end).max(start);
            Diagnostic {
                range: range(&index, text, start, end),
                severity: Some(match finding.severity {
                    Severity::Error => DiagnosticSeverity::ERROR,
                    Severity::Warning => DiagnosticSeverity::WARNING,
                    Severity::Information => DiagnosticSeverity::INFORMATION,
                    Severity::Hint => DiagnosticSeverity::HINT,
                }),
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
            }
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

#[derive(Default)]
struct Published {
    dotfmt: HashMap<Uri, Vec<Diagnostic>>,
    owners: HashMap<Uri, Vec<Uri>>,
    tsc: HashMap<Uri, Vec<Diagnostic>>,
    projects: HashMap<PathBuf, Vec<Uri>>,
}

impl Published {
    fn set_dotfmt(
        &mut self,
        owner: &Uri,
        entries: Vec<(Uri, Vec<Diagnostic>)>,
        sender: &crossbeam_channel::Sender<Message>,
    ) {
        let mut affected = self.owners.remove(owner).unwrap_or_default();
        affected.push(owner.clone());
        for uri in &affected {
            self.dotfmt.remove(uri);
        }
        let mut aliases = Vec::new();
        for (uri, diagnostics) in entries {
            aliases.push(uri.clone());
            affected.push(uri.clone());
            if diagnostics.is_empty() {
                self.dotfmt.remove(&uri);
            } else {
                self.dotfmt.insert(uri, diagnostics);
            }
        }
        self.owners.insert(owner.clone(), aliases);
        publish(&affected, &self.dotfmt, &self.tsc, sender);
    }

    fn clear_dotfmt(&mut self, owner: &Uri, sender: &crossbeam_channel::Sender<Message>) {
        let mut affected = self.owners.remove(owner).unwrap_or_default();
        affected.push(owner.clone());
        for uri in &affected {
            self.dotfmt.remove(uri);
        }
        publish(&affected, &self.dotfmt, &self.tsc, sender);
    }

    fn clear_all_dotfmt(&mut self, sender: &crossbeam_channel::Sender<Message>) {
        let mut affected: Vec<Uri> = self.dotfmt.keys().cloned().collect();
        affected.extend(self.owners.keys().cloned());
        self.dotfmt.clear();
        self.owners.clear();
        publish(&affected, &self.dotfmt, &self.tsc, sender);
    }

    fn set_tsc(
        &mut self,
        project: &Path,
        entries: Vec<(Uri, Vec<Diagnostic>)>,
        sender: &crossbeam_channel::Sender<Message>,
    ) {
        let mut affected = self.projects.remove(project).unwrap_or_default();
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
        self.projects.insert(project.to_path_buf(), owned);
        publish(&affected, &self.dotfmt, &self.tsc, sender);
    }

    fn clear_tsc(&mut self, sender: &crossbeam_channel::Sender<Message>) {
        let mut affected: Vec<Uri> = self.tsc.keys().cloned().collect();
        affected.extend(self.projects.values().flatten().cloned());
        self.tsc.clear();
        self.projects.clear();
        publish(&affected, &self.dotfmt, &self.tsc, sender);
    }
}

fn publish(
    affected: &[Uri],
    dotfmt: &HashMap<Uri, Vec<Diagnostic>>,
    tsc: &HashMap<Uri, Vec<Diagnostic>>,
    sender: &crossbeam_channel::Sender<Message>,
) {
    let mut seen = HashSet::new();
    for uri in affected {
        if !seen.insert(uri) {
            continue;
        }
        let mut diagnostics = dotfmt.get(uri).cloned().unwrap_or_default();
        diagnostics.extend(tsc.get(uri).cloned().unwrap_or_default());
        let notification = Notification::new(
            PublishDiagnostics::METHOD.into(),
            PublishDiagnosticsParams::new(uri.clone(), diagnostics, None),
        );
        let _ = sender.send(Message::Notification(notification));
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use dotfmt_core::edit::Related;
    use lsp_types::Position;

    use super::*;

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
            vec![(uri.clone(), vec![Diagnostic::default()])],
            &sender,
        );
        published.set_dotfmt(&uri, vec![(uri.clone(), Vec::new())], &sender);
        let _first = receiver.recv().unwrap();
        let second = receiver.recv().unwrap();
        let Message::Notification(notification) = second else {
            panic!("expected diagnostics notification");
        };
        let params: PublishDiagnosticsParams = serde_json::from_value(notification.params).unwrap();
        assert!(params.diagnostics.is_empty());
    }
}
