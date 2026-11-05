//! The dotfmt language server.

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread;

use dotfmt::app::{FixMode, Selection};
use dotfmt_core::edit::{Applicability, Fix, apply_fixes};
use dotfmt_core::language::Language;
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeConfiguration, DidChangeTextDocument, DidChangeWatchedFiles,
    DidChangeWorkspaceFolders, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument,
    Notification as _,
};
use lsp_types::request::{
    CodeActionRequest, DocumentDiagnosticRequest, ExecuteCommand, Formatting, OnTypeFormatting,
    RangeFormatting, Request as _, WillSaveWaitUntil,
};
use lsp_types::{
    CodeActionKind, CodeActionOptions, CodeActionOrCommand, CodeActionParams,
    CodeActionProviderCapability, DiagnosticOptions, DiagnosticServerCapabilities,
    DidChangeConfigurationParams, DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
    DidChangeWorkspaceFoldersParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DidSaveTextDocumentParams, DocumentDiagnosticParams, DocumentDiagnosticReport,
    DocumentFormattingParams, DocumentOnTypeFormattingOptions, DocumentOnTypeFormattingParams,
    DocumentRangeFormattingParams, ExecuteCommandOptions, ExecuteCommandParams,
    FullDocumentDiagnosticReport, InitializeParams, NumberOrString, OneOf, PositionEncodingKind,
    Range, RelatedFullDocumentDiagnosticReport, RelatedUnchangedDocumentDiagnosticReport,
    SaveOptions, ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions, TextDocumentSyncSaveOptions, TextEdit,
    UnchangedDocumentDiagnosticReport, Uri, WillSaveTextDocumentParams,
    WorkspaceFoldersServerCapabilities, WorkspaceServerCapabilities,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::actions::{self, Capabilities as EditCapabilities};
use crate::analysis;
use crate::client::Client;
use crate::diagnostics::Diagnostics;
use crate::documents::{Document, Documents};
use crate::edits;
use crate::settings::{FixesOnSave, Settings, SettingsStore};
use crate::tsc::Tsc;
use crate::uri;
use crate::workspace::Workspace;

pub const FIX_ALL: &str = "dotfmt.fixAll";
pub const ORGANIZE_IMPORTS: &str = "dotfmt.organizeImports";
pub const SORT_TAILWIND: &str = "dotfmt.sortTailwind";
pub const EXPLAIN: &str = "dotfmt.explain";
pub const RELOAD_CONFIG: &str = "dotfmt/reloadConfig";

const DOCTOR_REQUEST: &str = "dotfmt/doctor";
const EXPLAIN_REQUEST: &str = "dotfmt/explain";
const FORMAT_TEXT_REQUEST: &str = "dotfmt/formatText";
const REQUEST_WORKERS: usize = 4;

const COMMANDS: &[&str] = &[
    FIX_ALL,
    ORGANIZE_IMPORTS,
    SORT_TAILWIND,
    EXPLAIN,
    RELOAD_CONFIG,
];

pub type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

/// LSP 3.18 `textDocument/rangesFormatting`, absent from lsp-types 0.97.
pub enum RangesFormatting {}

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RangesFormattingParams {
    pub text_document: lsp_types::TextDocumentIdentifier,
    pub ranges: Vec<Range>,
    pub options: lsp_types::FormattingOptions,
}

impl lsp_types::request::Request for RangesFormatting {
    type Params = RangesFormattingParams;
    type Result = Option<Vec<TextEdit>>;
    const METHOD: &'static str = "textDocument/rangesFormatting";
}

pub fn serve() -> std::result::Result<std::process::ExitCode, String> {
    let (connection, threads) = Connection::stdio();
    let served = run(&connection);
    drop(connection);
    served.map_err(|error| error.to_string())?;
    threads.join().map_err(|error| error.to_string())?;
    Ok(std::process::ExitCode::SUCCESS)
}

pub fn run(connection: &Connection) -> Result<()> {
    let (id, raw) = connection.initialize_start()?;
    let params: InitializeParams = serde_json::from_value(raw)?;
    let server = Arc::new(Server::new(connection.sender.clone(), &params));
    server.start();
    connection.initialize_finish(id, server.initialization_result())?;
    // `initialize_finish` consumes the `initialized` notification, so dynamic
    // registration happens immediately afterwards; a duplicate `initialized`
    // later is guarded by `watchers_registered`.
    server.register_watchers();
    let trust = server.settings.trusted();
    server.log(format!(
        "dotfmt trust: {}",
        if trust { "trusted" } else { "untrusted" }
    ));
    server.pull_settings();

    let mut shutting_down = false;
    for message in &connection.receiver {
        match message {
            Message::Request(request) if shutting_down => {
                let _ = connection.sender.send(
                    Response::new_err(
                        request.id,
                        ErrorCode::InvalidRequest as i32,
                        "server is shutting down".into(),
                    )
                    .into(),
                );
            }
            Message::Request(request) if request.method == "shutdown" => {
                shutting_down = true;
                let _ = connection
                    .sender
                    .send(Response::new_ok(request.id, ()).into());
            }
            Message::Request(request) => server.dispatch(request),
            Message::Notification(notification) if notification.method == "exit" => break,
            Message::Notification(_) if shutting_down => {}
            Message::Notification(notification) => {
                if notification.method == "$/cancelRequest" {
                    server.cancel(&notification.params);
                    continue;
                }
                if catch_unwind(AssertUnwindSafe(|| server.notify(notification))).is_err() {
                    server.workspace.invalidate();
                    server.log("notification panicked; continuing");
                }
            }
            Message::Response(response) => server.client.response(response),
        }
    }
    Ok(())
}

pub struct Server {
    documents: Arc<Mutex<Documents>>,
    workspace: Arc<Workspace>,
    settings: Arc<SettingsStore>,
    client: Arc<Client>,
    diagnostics: Arc<Diagnostics>,
    tsc: Arc<Tsc>,
    /// The client advertises `textDocument/diagnostic`; pull owns publication.
    pull: bool,
    supports_configuration: bool,
    /// Folders with a `workspace/configuration` request in flight.
    folder_requests: Mutex<HashSet<PathBuf>>,
    /// Orders overlapping settings pulls; older responses are discarded.
    settings_epoch: AtomicU64,
    capabilities: EditCapabilities,
    in_flight: Mutex<HashMap<RequestId, Arc<RequestState>>>,
    queued: Mutex<HashMap<(Uri, &'static str), RequestId>>,
    requests: crossbeam_channel::Sender<RequestJob>,
    request_jobs: crossbeam_channel::Receiver<RequestJob>,
    watchers_registered: AtomicBool,
}

#[derive(Default)]
struct RequestState {
    cancelled: AtomicBool,
    superseded: AtomicBool,
}

impl RequestState {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    fn supersede(&self) {
        self.superseded.store(true, Ordering::Relaxed);
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    fn is_superseded(&self) -> bool {
        self.superseded.load(Ordering::Relaxed)
    }

    fn check(&self) -> std::result::Result<(), RpcError> {
        if self.is_cancelled() {
            Err(RpcError::request_cancelled())
        } else {
            Ok(())
        }
    }
}

struct RequestJob {
    request: Request,
    state: Arc<RequestState>,
    queued: Option<(Uri, &'static str)>,
}

impl Server {
    fn new(sender: crossbeam_channel::Sender<Message>, params: &InitializeParams) -> Self {
        let roots = workspace_roots(params);
        let documents = Arc::new(Mutex::new(Documents::default()));
        let workspace = Arc::new(Workspace::new(roots));
        let mut settings = Settings::default();
        if let Some(options) = &params.initialization_options {
            settings.apply(options);
        }
        let trusted = params
            .initialization_options
            .as_ref()
            .and_then(Settings::trust)
            .or_else(SettingsStore::trust_from_environment)
            .unwrap_or(false);
        dotfmt_tailwind::bridge::set_trusted(trusted);
        let settings = Arc::new(SettingsStore::new(false, settings));
        settings.set_trusted(trusted);
        let pull = params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|text_document| text_document.diagnostic.as_ref())
            .is_some();
        let supports_configuration = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.configuration)
            .unwrap_or(false);
        let client = Arc::new(Client::new(sender));
        let diagnostics = Arc::new(Diagnostics::new(
            Arc::clone(&workspace),
            Arc::clone(&documents),
            Arc::clone(&client),
            Arc::clone(&settings),
            pull,
        ));
        let tsc = Arc::new(Tsc::new(
            Arc::clone(&diagnostics),
            Arc::clone(&documents),
            Arc::clone(&workspace),
        ));
        let workspace_edit = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.workspace_edit.as_ref());
        let capabilities = EditCapabilities {
            versioned: workspace_edit
                .and_then(|edit| edit.document_changes)
                .unwrap_or(false),
            annotations: workspace_edit
                .and_then(|edit| edit.change_annotation_support.as_ref())
                .is_some(),
        };
        let (requests, request_jobs) = crossbeam_channel::unbounded();
        Self {
            documents,
            workspace,
            settings,
            client,
            diagnostics,
            tsc,
            pull,
            supports_configuration,
            folder_requests: Mutex::new(HashSet::new()),
            settings_epoch: AtomicU64::new(0),
            capabilities,
            in_flight: Mutex::new(HashMap::new()),
            queued: Mutex::new(HashMap::new()),
            requests,
            request_jobs,
            watchers_registered: AtomicBool::new(false),
        }
    }

    /// Starts the bounded request worker pool. Called once by [`run`].
    pub fn start(self: &Arc<Self>) {
        for index in 0..REQUEST_WORKERS {
            let server = Arc::downgrade(self);
            let jobs = self.request_jobs.clone();
            let _ = thread::Builder::new()
                .name(format!("dotfmt-lsp-request-{index}"))
                .spawn(move || {
                    while let Ok(job) = jobs.recv() {
                        let Some(server) = Weak::upgrade(&server) else {
                            break;
                        };
                        server.run_request(job);
                    }
                });
        }
    }

    fn initialization_result(&self) -> Value {
        let mut capabilities =
            serde_json::to_value(capabilities(self.pull)).expect("server capabilities serialize");
        capabilities["documentRangeFormattingProvider"] = json!({ "rangesSupport": true });
        json!({
            "capabilities": capabilities,
            "serverInfo": { "name": "dotfmt-lsp", "version": env!("CARGO_PKG_VERSION") },
        })
    }

    fn trusted(&self) -> bool {
        self.settings.trusted()
    }

    fn settings_for(&self, path: &Path) -> Settings {
        self.settings.settings_for(path)
    }

    fn settings_for_uri(&self, uri: &Uri) -> Settings {
        self.settings.settings_for_uri(uri)
    }

    fn settings_for_root(&self, root: &Path) -> Settings {
        self.settings.settings_for_root(root)
    }

    fn log(&self, message: impl Into<String>) {
        self.diagnostics.log(message);
    }

    /// Applies a push-only `didChangeConfiguration` payload to the default.
    ///
    /// Clients without `workspace/configuration` cannot scope values per
    /// folder, so the payload is treated as the workspace default.
    fn configure_push(&self, value: &Value) {
        if self.settings.apply_default(value) {
            self.reload_all();
        }
    }

    /// Pulls the default and one configuration per workspace folder.
    ///
    /// A pull is only applied when no newer pull has started; only folders
    /// whose settings changed are reloaded.
    fn pull_settings(self: &Arc<Self>) {
        if !self.supports_configuration {
            return;
        }
        let roots = self.workspace.roots();
        let epoch = self.settings_epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let mut items = vec![json!({ "section": "dotfmt" })];
        {
            let mut pending = self
                .folder_requests
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            for root in &roots {
                items.push(json!({
                    "section": "dotfmt",
                    "scopeUri": uri::from_path(root),
                }));
                pending.insert(root.clone());
            }
        }
        let client = Arc::clone(&self.client);
        let server = Arc::clone(self);
        thread::spawn(move || {
            let result = client.request("workspace/configuration", json!({ "items": items }));
            let current = server.settings_epoch.load(Ordering::SeqCst) == epoch;
            if current && let Ok(value) = result {
                let default_changed = value
                    .get(0)
                    .is_some_and(|settings| server.settings.configure_default(settings));
                let mut changed = Vec::new();
                for (index, root) in roots.iter().enumerate() {
                    let Some(settings) = value.get(index + 1) else {
                        continue;
                    };
                    if server.settings.configure_folder(root, settings) {
                        changed.push(root.clone());
                    }
                }
                if default_changed {
                    server.reload_all();
                }
                for root in changed {
                    server.reload_folder(&root);
                }
            }
            let mut pending = server
                .folder_requests
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            for root in &roots {
                pending.remove(root);
            }
        });
    }

    /// Requests one folder's configuration after a `didOpen` beat the initial pull.
    fn request_folder(self: &Arc<Self>, root: PathBuf) {
        if !self.supports_configuration {
            return;
        }
        if !self
            .folder_requests
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(root.clone())
        {
            return;
        }
        let client = Arc::clone(&self.client);
        let server = Arc::clone(self);
        thread::spawn(move || {
            let known = server.settings.has_folder(&root);
            let requested = root.clone();
            let result = client.request(
                "workspace/configuration",
                json!({
                    "items": [{
                        "section": "dotfmt",
                        "scopeUri": uri::from_path(&requested),
                    }]
                }),
            );
            if let Ok(value) = result
                && let Some(settings) = value.get(0)
                && (server.settings.configure_folder(&root, settings) || !known)
            {
                server.reload_folder(&root);
            }
            server
                .folder_requests
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&root);
        });
    }

    /// Rebuilds everything: trust, the default configuration or a global config file.
    fn reload_all(&self) {
        self.workspace.invalidate();
        dotfmt_typescript::project::clear_cache();
        self.diagnostics.clear_all();
        self.tsc.invalidate();
        // Snapshot under the lock, then reschedule without holding it so
        // request handlers and diagnostics are not blocked by the clone.
        let snapshot = self.document_snapshot(|_| true);
        for (uri, document) in snapshot.iter() {
            if !supported(uri, document.language()) {
                continue;
            }
            if let Some(path) = analysis::buffer_path(uri, document.language()) {
                self.diagnostics.schedule(document, path, uri.clone());
            }
            self.schedule_tsc(uri.clone());
        }
    }

    /// Reloads one workspace folder without touching another folder's state.
    fn reload_folder(&self, root: &Path) {
        let root = crate::workspace::normalize(root);
        dotfmt_typescript::project::invalidate_root(&root);
        self.workspace.invalidate_root(&root);
        self.diagnostics.clear_under(root.clone());
        self.tsc.invalidate_root(&root);
        let snapshot = self.document_snapshot(|path| path.starts_with(&root));
        for (uri, document) in snapshot.iter() {
            if !supported(uri, document.language()) {
                continue;
            }
            if let Some(path) = analysis::buffer_path(uri, document.language()) {
                self.diagnostics.schedule(document, path, uri.clone());
            }
            self.schedule_tsc(uri.clone());
        }
    }

    fn document_snapshot(&self, keep: impl Fn(&Path) -> bool) -> Documents {
        let documents = self
            .documents
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut snapshot = Documents::default();
        for (uri, document) in documents.iter() {
            let Some(path) = analysis::buffer_path(uri, document.language()) else {
                continue;
            };
            if keep(&path) {
                snapshot.open(uri.clone(), document.clone());
            }
        }
        snapshot
    }

    /// Invalidates compiler results and rechecks the open documents of one
    /// root, or every root when `root` is `None`.
    fn reschedule_tsc(&self, root: Option<&Path>) {
        match root {
            Some(root) => self.tsc.invalidate_root(root),
            None => self.tsc.invalidate(),
        }
        let snapshot =
            self.document_snapshot(|path| root.is_none_or(|root| path.starts_with(root)));
        for (uri, _) in snapshot.iter() {
            self.schedule_tsc(uri.clone());
        }
    }

    fn document(&self, uri: &Uri) -> std::result::Result<Document, RpcError> {
        let documents = self
            .documents
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let document = documents.get(uri).ok_or_else(|| {
            RpcError::invalid_params(format!("document not open: {}", uri.as_str()))
        })?;
        if document.is_desynchronised() {
            return Err(RpcError::content_modified(
                "document is desynchronised; reopen it before formatting",
            ));
        }
        Ok(document.clone())
    }

    fn ensure_current(&self, uri: &Uri, version: i32) -> std::result::Result<(), RpcError> {
        let documents = self
            .documents
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match documents.get(uri) {
            Some(document) if !document.is_desynchronised() && document.version == version => {
                Ok(())
            }
            _ => Err(RpcError::content_modified(
                "document changed while formatting",
            )),
        }
    }

    fn path(&self, uri: &Uri, document: &Document) -> std::result::Result<PathBuf, RpcError> {
        analysis::buffer_path(uri, document.language())
            .ok_or_else(|| RpcError::invalid_params("buffer has no dotfmt language"))
    }

    fn dispatch(self: &Arc<Self>, request: Request) {
        let id = request.id.clone();
        let state = Arc::new(RequestState::default());
        self.in_flight
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(id.clone(), Arc::clone(&state));
        let uri = coalesce_key(&request.method, &request.params);
        if let Some(uri) = &uri {
            let previous = self
                .queued
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(uri.clone(), id.clone());
            if let Some(previous) = previous.filter(|previous| previous != &id) {
                let previous = self
                    .in_flight
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .get(&previous)
                    .cloned();
                if let Some(previous) = previous {
                    previous.supersede();
                }
            }
        }
        let job = RequestJob {
            request,
            state,
            queued: uri.clone(),
        };
        if let Err(error) = self.requests.send(job) {
            let id = error.0.request.id;
            self.finish(&id, uri.as_ref());
            let _ = self
                .client_sender()
                .send(Message::Response(Response::new_err(
                    id,
                    ErrorCode::InternalError as i32,
                    "request worker pool is unavailable".into(),
                )));
        }
    }

    fn run_request(self: &Arc<Self>, job: RequestJob) {
        let RequestJob {
            request,
            state,
            queued,
        } = job;
        let method = request.method;
        let params = request.params;
        let id = request.id;
        let result = if state.is_superseded() {
            Ok(Value::Null)
        } else {
            match catch_unwind(AssertUnwindSafe(|| self.handle(&method, &params, &state))) {
                Ok(Ok(_)) if state.is_superseded() => Ok(Value::Null),
                Ok(Ok(_)) if state.is_cancelled() => Err(RpcError::request_cancelled()),
                Ok(result) => result,
                Err(_) => {
                    self.workspace.invalidate();
                    self.log(format!("request {method} panicked"));
                    Err(RpcError {
                        code: ErrorCode::InternalError as i32,
                        message: "dotfmt internal error".into(),
                    })
                }
            }
        };
        let response = match result {
            Ok(value) => Response::new_ok(id.clone(), value),
            Err(error) => Response::new_err(id.clone(), error.code, error.message),
        };
        self.finish(&id, queued.as_ref());
        let _ = self.client_sender().send(Message::Response(response));
    }

    fn client_sender(&self) -> crossbeam_channel::Sender<Message> {
        self.client.sender()
    }

    fn finish(&self, id: &RequestId, queued: Option<&(Uri, &'static str)>) {
        self.in_flight
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(id);
        if let Some(queued) = queued {
            let mut pending = self
                .queued
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if pending.get(queued) == Some(id) {
                pending.remove(queued);
            }
        }
    }

    fn cancel(&self, params: &Value) {
        let Ok(params) = serde_json::from_value::<CancelParams>(params.clone()) else {
            return;
        };
        let id = match params.id {
            NumberOrString::Number(id) => RequestId::from(id),
            NumberOrString::String(id) => RequestId::from(id),
        };
        let state = self
            .in_flight
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&id)
            .cloned();
        if let Some(state) = state {
            state.cancel();
        }
    }

    fn handle(
        self: &Arc<Self>,
        method: &str,
        params: &Value,
        state: &RequestState,
    ) -> std::result::Result<Value, RpcError> {
        state.check()?;
        match method {
            Formatting::METHOD => {
                let params: DocumentFormattingParams = parse(params)?;
                let document = self.document(&params.text_document.uri)?;
                let edits = self.format_document(
                    &params.text_document.uri,
                    &document,
                    &[],
                    true,
                    Some(state),
                )?;
                Ok(json!(Some(edits)))
            }
            RangeFormatting::METHOD => {
                let params: DocumentRangeFormattingParams = parse(params)?;
                let document = self.document(&params.text_document.uri)?;
                let edits = self.format_document(
                    &params.text_document.uri,
                    &document,
                    &[params.range],
                    false,
                    Some(state),
                )?;
                Ok(json!(Some(edits)))
            }
            WillSaveWaitUntil::METHOD => {
                let params: WillSaveTextDocumentParams = parse(params)?;
                let document = self.document(&params.text_document.uri)?;
                let edits = self.fix_on_save(&params.text_document.uri, &document)?;
                Ok(json!(edits))
            }
            RangesFormatting::METHOD => {
                let params: RangesFormattingParams = parse(params)?;
                let document = self.document(&params.text_document.uri)?;
                let edits = self.format_document(
                    &params.text_document.uri,
                    &document,
                    &params.ranges,
                    false,
                    Some(state),
                )?;
                Ok(json!(Some(edits)))
            }
            OnTypeFormatting::METHOD => {
                let params: DocumentOnTypeFormattingParams = parse(params)?;
                let uri = params.text_document_position.text_document.uri;
                let document = self.document(&uri)?;
                let path = self.path(&uri, &document)?;
                let range = on_type_range(&document, &path, params.text_document_position.position);
                let edits = self.format_document(&uri, &document, &[range], false, Some(state))?;
                Ok(json!(Some(edits)))
            }
            CodeActionRequest::METHOD => {
                let params: CodeActionParams = parse(params)?;
                Ok(json!(self.code_actions(params, state)?))
            }
            ExecuteCommand::METHOD => {
                let params: ExecuteCommandParams = parse(params)?;
                self.execute(params, state)
            }
            DocumentDiagnosticRequest::METHOD => self.diagnostic(params),
            DOCTOR_REQUEST => Ok(self.doctor()),
            EXPLAIN_REQUEST => self.explain(params.clone()),
            FORMAT_TEXT_REQUEST => self.format_text(params.clone()),
            _ => Err(RpcError {
                code: ErrorCode::MethodNotFound as i32,
                message: format!("unhandled method: {method}"),
            }),
        }
    }

    /// Pull diagnostics: the merged dotfmt + tsc error set for one URI.
    ///
    /// The report always contains both sources so a pull client never loses
    /// tsc entries to a dotfmt-only response. Unsupported documents report an
    /// empty full set rather than an error.
    fn diagnostic(&self, params: &Value) -> std::result::Result<Value, RpcError> {
        let params: DocumentDiagnosticParams = parse(params)?;
        let uri = params.text_document.uri;
        let version = self
            .documents
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&uri)
            .map(|document| document.version);
        if !self.supported_document(&uri) {
            return serde_json::to_value(DocumentDiagnosticReport::Full(
                RelatedFullDocumentDiagnosticReport {
                    related_documents: None,
                    full_document_diagnostic_report: FullDocumentDiagnosticReport {
                        result_id: Some("unsupported".into()),
                        items: Vec::new(),
                    },
                },
            ))
            .map_err(|error| RpcError::invalid_params(error.to_string()));
        }
        let merged = self.diagnostics.merged(&uri, version);
        let report = if params.previous_result_id.as_deref() == Some(merged.result_id.as_str()) {
            DocumentDiagnosticReport::Unchanged(RelatedUnchangedDocumentDiagnosticReport {
                related_documents: None,
                unchanged_document_diagnostic_report: UnchangedDocumentDiagnosticReport {
                    result_id: merged.result_id,
                },
            })
        } else {
            DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                related_documents: None,
                full_document_diagnostic_report: FullDocumentDiagnosticReport {
                    result_id: Some(merged.result_id),
                    items: merged.diagnostics,
                },
            })
        };
        serde_json::to_value(report).map_err(|error| RpcError::invalid_params(error.to_string()))
    }

    fn supported_document(&self, uri: &Uri) -> bool {
        let language = self
            .documents
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(uri)
            .map(|document| document.language().to_owned());
        language
            .as_deref()
            .map(|language| supported(uri, language))
            .unwrap_or_else(|| {
                uri::path(uri).is_some_and(|path| Language::for_path(&path).is_some())
            })
    }

    fn format_document(
        &self,
        uri: &Uri,
        document: &Document,
        ranges: &[Range],
        request_fixes: bool,
        state: Option<&RequestState>,
    ) -> std::result::Result<Vec<TextEdit>, RpcError> {
        // Configured dotfmt.dotfile fixes are only for full-document formatting
        // and require explicit trust; range/on-type formatting never applies
        // them.
        let fixes = request_fixes && ranges.is_empty() && self.trusted();
        let path = self.path(uri, document)?;
        if fixes
            && let Some(root) = dotfmt_typescript::project::discover_root(&path)
        {
            dotfmt_typescript::project::invalidate_root(&root);
        }
        let languages = analysis::forced(&path, uri, document.language());
        let session = self.workspace.session(&path);
        let mut session = session.lock().unwrap_or_else(|error| error.into_inner());
        let output = match analysis::format(&mut session, &path, document.text(), languages, fixes)
        {
            Ok(output) => output,
            Err(errors) => {
                self.publish_fatal(uri, document.text(), document.version, &errors);
                return Ok(Vec::new());
            }
        };
        if let Some(state) = state {
            state.check()?;
        }
        self.ensure_current(uri, document.version)?;
        let edits = edits::minimal(document.text(), &output);
        let edits = if ranges.is_empty() {
            edits
        } else {
            let ranges = expanded_ranges(document, &path, ranges);
            edits::filter(edits, &ranges)
        };
        Ok(edits::text_edits(document, &edits))
    }

    fn fix_on_save(
        &self,
        uri: &Uri,
        document: &Document,
    ) -> std::result::Result<Vec<TextEdit>, RpcError> {
        let path = self.path(uri, document)?;
        let settings = self.settings_for(&path);
        let mode = settings.fixes_on_save;
        let languages = analysis::forced(&path, uri, document.language());
        let session = self.workspace.session(&path);
        let mut session = session.lock().unwrap_or_else(|error| error.into_inner());
        let mut source = document.text().to_owned();
        let mut on_save_fixes = false;
        if mode != FixesOnSave::Off {
            let fix = match mode {
                FixesOnSave::Off => unreachable!(),
                FixesOnSave::Safe => FixMode::Safe,
                FixesOnSave::Unsafe if self.trusted() => FixMode::Unsafe,
                FixesOnSave::Unsafe => FixMode::Safe,
            };
            let inspection = match analysis::inspect(
                &mut session,
                &path,
                document.text(),
                languages.clone(),
                fix,
            ) {
                Ok(inspection) => inspection,
                Err(errors) => {
                    self.publish_fatal(uri, document.text(), document.version, &errors);
                    return Ok(Vec::new());
                }
            };
            source = inspection.after;
            on_save_fixes = true;
        }
        // `willSaveWaitUntil` is one of the two paths allowed to apply
        // configured fixes. Untrusted workspaces never run them.
        if self.trusted()
            && let Some(root) = dotfmt_typescript::project::discover_root(&path)
        {
            dotfmt_typescript::project::invalidate_root(&root);
        }
        let formatted = match analysis::format(
            &mut session,
            &path,
            &source,
            languages.clone(),
            self.trusted(),
        ) {
            Ok(text) => text,
            Err(errors) => {
                self.publish_fatal(uri, document.text(), document.version, &errors);
                return Ok(Vec::new());
            }
        };
        if !on_save_fixes {
            // `fixes.onSave = off` must not turn will-save into format-on-save.
            let plain =
                match analysis::format(&mut session, &path, document.text(), languages, false) {
                    Ok(text) => text,
                    Err(errors) => {
                        self.publish_fatal(uri, document.text(), document.version, &errors);
                        return Ok(Vec::new());
                    }
                };
            if formatted == plain {
                return Ok(Vec::new());
            }
        }
        self.ensure_current(uri, document.version)?;
        Ok(edits::whole(document, &formatted))
    }

    fn code_actions(
        &self,
        params: CodeActionParams,
        state: &RequestState,
    ) -> std::result::Result<Vec<CodeActionOrCommand>, RpcError> {
        let uri = params.text_document.uri;
        let document = self.document(&uri)?;
        let path = self.path(&uri, &document)?;
        let languages = analysis::forced(&path, &uri, document.language());
        let only: Vec<String> = params
            .context
            .only
            .unwrap_or_default()
            .into_iter()
            .map(|kind| kind.as_str().to_owned())
            .collect();
        let session = self.workspace.session(&path);
        let mut session = session.lock().unwrap_or_else(|error| error.into_inner());
        state.check()?;
        // One inspection supplies quickfixes and the safe fix set for both
        // fix-all and organize-imports; formatting then runs exactly once.
        let inspection = match analysis::inspect(
            &mut session,
            &path,
            document.text(),
            languages.clone(),
            FixMode::Unsafe,
        ) {
            Ok(inspection) => inspection,
            Err(errors) => {
                self.publish_fatal(&uri, document.text(), document.version, &errors);
                return Ok(Vec::new());
            }
        };
        let fixes = inspection.fixes;
        let wants_fix_all = actions::wants(&only, actions::FIX_ALL);
        let wants_organize = actions::wants(&only, actions::ORGANIZE_IMPORTS);
        let wants_format = actions::wants(&only, actions::FORMAT);
        let mut format = None;
        let mut fix_all = None;
        let mut organize = None;
        if wants_fix_all || wants_organize || wants_format {
            state.check()?;
            let safe_fixes: Vec<&Fix> = fixes
                .iter()
                .filter(|fix| fix.applicability == Applicability::Safe)
                .collect();
            let organize_fixes: Vec<&Fix> = safe_fixes
                .iter()
                .copied()
                .filter(|fix| fix.kind.contains("organizeImports"))
                .collect();
            let selected: Vec<&Fix> = if wants_fix_all {
                safe_fixes
            } else if wants_organize {
                organize_fixes
            } else {
                Vec::new()
            };
            let source = if selected.is_empty() {
                document.text().to_owned()
            } else {
                match apply_fixes(document.text(), &selected) {
                    Ok(text) => text,
                    Err(error) => {
                        self.log(format!("apply fixes: {error}"));
                        document.text().to_owned()
                    }
                }
            };
            match analysis::format(&mut session, &path, &source, languages, false) {
                Ok(text) => {
                    if wants_fix_all {
                        fix_all = Some(text.clone());
                    }
                    if wants_organize {
                        organize = Some(text.clone());
                    }
                    if wants_format {
                        format = Some(text);
                    }
                }
                Err(errors) => {
                    self.publish_fatal(&uri, document.text(), document.version, &errors);
                }
            }
        }
        state.check()?;
        self.ensure_current(&uri, document.version)?;
        Ok(actions::code_actions(&actions::Context {
            document: &document,
            uri: &uri,
            range: params.range,
            only,
            trusted: self.trusted(),
            fixes: &fixes,
            format: format.as_deref(),
            fix_all: fix_all.as_deref(),
            organize: organize.as_deref(),
            capabilities: self.capabilities,
        }))
    }

    fn execute(
        self: &Arc<Self>,
        params: ExecuteCommandParams,
        state: &RequestState,
    ) -> std::result::Result<Value, RpcError> {
        let command = params.command.as_str();
        if command == RELOAD_CONFIG {
            self.reload_all();
            self.pull_settings();
            return Ok(Value::Null);
        }
        let argument = params.arguments.first();
        if command == EXPLAIN {
            let value = match argument {
                Some(Value::Object(_)) => argument.cloned().unwrap_or(Value::Null),
                Some(Value::String(uri)) => json!({
                    "textDocument": { "uri": uri },
                    "languageId": params.arguments.get(1).and_then(Value::as_str),
                }),
                _ => Value::Null,
            };
            return self.explain(value);
        }
        let uri: Uri = argument
            .and_then(Value::as_str)
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| RpcError::invalid_params("command needs a document URI"))?;
        let document = self.document(&uri)?;
        let path = self.path(&uri, &document)?;
        let languages = analysis::forced(&path, &uri, document.language());
        let session = self.workspace.session(&path);
        let mut session = session.lock().unwrap_or_else(|error| error.into_inner());
        let text = match command {
            FIX_ALL => {
                let requested = params
                    .arguments
                    .get(1)
                    .and_then(Value::as_str)
                    .unwrap_or("safe");
                let mode = if !self.trusted() || requested != "unsafe" {
                    FixMode::Safe
                } else {
                    FixMode::Unsafe
                };
                analysis::inspect(&mut session, &path, document.text(), languages, mode)
                    .map(|inspection| inspection.after)
            }
            ORGANIZE_IMPORTS => analysis::organize(&mut session, &path, document.text(), languages),
            SORT_TAILWIND => {
                analysis::format(&mut session, &path, document.text(), languages, false)
            }
            _ => {
                return Err(RpcError {
                    code: ErrorCode::MethodNotFound as i32,
                    message: format!("unhandled command: {command}"),
                });
            }
        };
        let text = text.map_err(|errors| RpcError {
            code: ErrorCode::RequestFailed as i32,
            message: errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        })?;
        drop(session);
        state.check()?;
        self.ensure_current(&uri, document.version)?;
        let edit =
            actions::workspace_edit_text(&document, &uri, &text, command, false, self.capabilities);
        self.client
            .request("workspace/applyEdit", json!({ "edit": edit }))
            .map_err(|error| RpcError {
                code: ErrorCode::RequestFailed as i32,
                message: format!("apply edit: {error}"),
            })
    }

    fn format_text(&self, value: Value) -> std::result::Result<Value, RpcError> {
        let params: FormatTextParams = parse(&value)?;
        let language = params
            .language_id
            .as_deref()
            .and_then(analysis::language_for);
        let path = params
            .uri
            .as_deref()
            .and_then(|uri| uri.parse::<Uri>().ok())
            .and_then(|uri| uri::path(&uri))
            .or_else(|| language.map(|language| language.default_stdin().to_path_buf()))
            .ok_or_else(|| RpcError::invalid_params("formatText needs a uri or languageId"))?;
        let languages = language.into_iter().collect();
        let session = self.workspace.session(&path);
        let mut session = session.lock().unwrap_or_else(|error| error.into_inner());
        let text = analysis::format(&mut session, &path, &params.text, languages, false).map_err(
            |errors| RpcError {
                code: ErrorCode::RequestFailed as i32,
                message: errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n"),
            },
        )?;
        Ok(json!(text))
    }

    fn explain(&self, value: Value) -> std::result::Result<Value, RpcError> {
        let params: ExplainParams = parse(&value)?;
        let language_id = params.language_id.as_deref().unwrap_or("");
        let path = analysis::buffer_path(&params.text_document.uri, language_id)
            .ok_or_else(|| RpcError::invalid_params("buffer has no dotfmt language"))?;
        let languages = analysis::forced(&path, &params.text_document.uri, language_id);
        let session = self.workspace.session(&path);
        let mut session = session.lock().unwrap_or_else(|error| error.into_inner());
        match session.explain(&path, languages, None) {
            Ok(explanation) => {
                let mut value = json!({
                    "configs": explanation.sources.iter().map(|path| path.display().to_string()).collect::<Vec<_>>(),
                });
                match explanation.selection {
                    Selection::Language(language) => {
                        value["language"] = json!(language.name());
                    }
                    Selection::Disabled(language) => {
                        value["language"] = json!(language.name());
                        value["skipped"] = json!("disabled");
                    }
                    Selection::Excluded => {
                        value["skipped"] = json!("excluded");
                    }
                    Selection::Unmapped => {
                        value["skipped"] = json!("unmapped");
                    }
                }
                if let Some(dialect) = explanation.dialect {
                    value["dialect"] = json!(dialect);
                }
                Ok(value)
            }
            Err(errors) => Ok(json!({
                "configs": [],
                "error": errors.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n"),
            })),
        }
    }

    fn notify(self: &Arc<Self>, notification: Notification) {
        match notification.method.as_str() {
            "initialized" => self.register_watchers(),
            DidOpenTextDocument::METHOD => {
                if let Ok(params) =
                    serde_json::from_value::<DidOpenTextDocumentParams>(notification.params)
                {
                    let item = params.text_document;
                    let is_supported = supported(&item.uri, &item.language_id);
                    let language = item.language_id;
                    let document = Document::new(item.text, language, item.version);
                    self.documents
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .open(item.uri.clone(), document.clone());
                    if !is_supported {
                        return;
                    }
                    let path = self.document_path(&item.uri, &document);
                    self.diagnostics
                        .schedule(&document, path.clone(), item.uri.clone());
                    self.schedule_tsc(item.uri);
                    // The initial pull may have raced this open, and a folder
                    // may have been added after `initialized`.
                    if let Some(root) = self.workspace.containing_root(&path)
                        && (!self.settings.has_folder(&root)
                            || self
                                .folder_requests
                                .lock()
                                .unwrap_or_else(|error| error.into_inner())
                                .contains(&root))
                    {
                        self.request_folder(root);
                    }
                }
            }
            DidChangeTextDocument::METHOD => {
                if let Ok(params) =
                    serde_json::from_value::<DidChangeTextDocumentParams>(notification.params)
                {
                    let uri = params.text_document.uri;
                    let version = params.text_document.version;
                    let outcome = {
                        let mut documents = self
                            .documents
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        match documents.get_mut(&uri) {
                            None => None,
                            Some(document) if version <= document.version => {
                                Some(Err(document.version))
                            }
                            Some(document) => {
                                document.version = version;
                                let mut failed = false;
                                for change in params.content_changes {
                                    if !document.change(change) {
                                        failed = true;
                                        break;
                                    }
                                }
                                Some(Ok((document.clone(), failed)))
                            }
                        }
                    };
                    match outcome {
                        None => {}
                        Some(Err(current)) => {
                            self.log(format!(
                                "ignoring out-of-order didChange for {} version {version} (current version {current})",
                                uri.as_str()
                            ));
                            self.diagnostics.clear(uri.clone());
                            self.diagnostics.clear_tsc_uri(uri);
                        }
                        Some(Ok((_, true))) => {
                            self.log(format!(
                                "incremental didChange for {} could not be applied; document is desynchronised until reopen",
                                uri.as_str()
                            ));
                            self.diagnostics.clear_tsc_uri(uri);
                        }
                        Some(Ok((snapshot, false))) => {
                            // tsc entries describe the file on disk; an edit
                            // invalidates them until the next save/open.
                            self.diagnostics.clear_tsc_uri(uri.clone());
                            if supported(&uri, snapshot.language())
                                && let Some(path) = analysis::buffer_path(&uri, snapshot.language())
                            {
                                self.diagnostics.schedule(&snapshot, path, uri);
                            }
                        }
                    }
                }
            }
            DidCloseTextDocument::METHOD => {
                if let Ok(params) =
                    serde_json::from_value::<DidCloseTextDocumentParams>(notification.params)
                {
                    let settings = self.settings_for_uri(&params.text_document.uri);
                    self.documents
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .close(&params.text_document.uri);
                    self.diagnostics.clear(params.text_document.uri.clone());
                    if settings.tsc_scope == crate::settings::TscScope::Open {
                        self.diagnostics.clear_tsc_uri(params.text_document.uri);
                    }
                }
            }
            DidSaveTextDocument::METHOD => {
                if let Ok(params) =
                    serde_json::from_value::<DidSaveTextDocumentParams>(notification.params)
                {
                    let uri = params.text_document.uri;
                    let snapshot = {
                        let mut documents = self
                            .documents
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        let Some(document) = documents.get_mut(&uri) else {
                            return;
                        };
                        if let Some(text) = params.text {
                            document.change(lsp_types::TextDocumentContentChangeEvent {
                                range: None,
                                range_length: None,
                                text,
                            });
                        }
                        document.clone()
                    };
                    if supported(&uri, snapshot.language())
                        && let Some(path) = analysis::buffer_path(&uri, snapshot.language())
                    {
                        self.diagnostics.schedule(&snapshot, path.clone(), uri.clone());
                        if let Some(root) = dotfmt_typescript::project::discover_root(&path) {
                            dotfmt_typescript::project::invalidate_root(&root);
                        }
                    }
                    self.schedule_tsc(uri);
                }
            }
            DidChangeConfiguration::METHOD => {
                if self.supports_configuration {
                    // Pull is authoritative; `params.settings` can be stale and
                    // does not carry per-folder scopes.
                    self.pull_settings();
                } else if let Ok(params) =
                    serde_json::from_value::<DidChangeConfigurationParams>(notification.params)
                {
                    self.configure_push(&params.settings);
                }
            }
            DidChangeWatchedFiles::METHOD => {
                if let Ok(params) =
                    serde_json::from_value::<DidChangeWatchedFilesParams>(notification.params)
                {
                    let global_config = crate::workspace::global_config();
                    let mut config_roots = HashSet::new();
                    let mut tsc_roots = HashSet::new();
                    let mut all_config = false;
                    let mut all_tsc = false;
                    for change in &params.changes {
                        let Some(path) = uri::path(&change.uri) else {
                            continue;
                        };
                        let root = self.workspace.containing_root(&path);
                        if is_config_file(&path) {
                            match root {
                                Some(root) if global_config.as_deref() != Some(path.as_path()) => {
                                    config_roots.insert(root);
                                }
                                _ => all_config = true,
                            }
                        } else {
                            match root {
                                Some(root) => {
                                    tsc_roots.insert(root);
                                }
                                None => all_tsc = true,
                            }
                        }
                    }
                    if all_config {
                        self.reload_all();
                    } else {
                        for root in config_roots {
                            self.reload_folder(&root);
                        }
                    }
                    if !all_config {
                        if all_tsc {
                            self.reschedule_tsc(None);
                        } else {
                            for root in tsc_roots {
                                self.reschedule_tsc(Some(&root));
                            }
                        }
                    }
                }
            }
            DidChangeWorkspaceFolders::METHOD => {
                if let Ok(params) =
                    serde_json::from_value::<DidChangeWorkspaceFoldersParams>(notification.params)
                {
                    let mut roots = self.workspace.roots();
                    let mut affected = Vec::new();
                    for folder in params.event.removed {
                        let Some(path) = uri::path(&folder.uri) else {
                            continue;
                        };
                        let root = crate::workspace::normalize(&path);
                        roots.retain(|candidate| candidate != &root);
                        self.settings.remove_folder(&root);
                        affected.push(root);
                    }
                    let mut added = false;
                    for folder in params.event.added {
                        let Some(path) = uri::path(&folder.uri) else {
                            continue;
                        };
                        let root = crate::workspace::normalize(&path);
                        roots.push(root.clone());
                        if !self.settings.has_folder(&root) {
                            self.settings.set_folder(root.clone(), Settings::default());
                        }
                        affected.push(root);
                        added = true;
                    }
                    self.workspace.set_roots(roots);
                    for root in affected {
                        self.reload_folder(&root);
                    }
                    if added {
                        self.pull_settings();
                    }
                }
            }
            _ => {}
        }
    }

    fn register_watchers(&self) {
        if self.watchers_registered.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut watchers = vec![
            json!({ "globPattern": "**/dotfmt.dotfile" }),
            json!({ "globPattern": "**/.prettierignore" }),
            json!({ "globPattern": "**/tsconfig*.json" }),
            json!({ "globPattern": "**/jsconfig*.json" }),
            json!({ "globPattern": "**/package.json" }),
            json!({ "globPattern": "**/package-lock.json" }),
            json!({ "globPattern": "**/pnpm-lock.yaml" }),
            json!({ "globPattern": "**/yarn.lock" }),
            json!({ "globPattern": "**/bun.lock" }),
            json!({ "globPattern": "**/bun.lockb" }),
        ];
        if let Some(global) = crate::workspace::global_config() {
            watchers.push(json!({ "globPattern": global.display().to_string() }));
        }
        let params = json!({
            "registrations": [{
                "id": "dotfmt-watched-files",
                "method": "workspace/didChangeWatchedFiles",
                "registerOptions": { "watchers": watchers },
            }]
        });
        if let Err(error) = self
            .client
            .send_request("client/registerCapability", params)
        {
            self.log(format!("register file watchers: {error}"));
        }
    }

    fn document_path(&self, uri: &Uri, document: &Document) -> PathBuf {
        analysis::buffer_path(uri, document.language()).unwrap_or_else(|| PathBuf::from("stdin"))
    }

    fn schedule_tsc(&self, uri: Uri) {
        if !self.trusted() {
            return;
        }
        let Some(path) = uri::path(&uri) else {
            return;
        };
        if !analysis::is_typescript(&path) {
            return;
        }
        let settings = self.settings_for(&path);
        if !settings.type_aware {
            return;
        }
        self.tsc
            .schedule(path.clone(), self.workspace.root_for(&path), settings);
    }

    fn publish_fatal(
        &self,
        uri: &Uri,
        text: &str,
        version: i32,
        errors: &[dotfmt_core::diagnostic::Diagnostic],
    ) {
        if self.settings_for_uri(uri).syntax_diagnostics {
            self.diagnostics
                .publish_fatal(uri.clone(), text, version, errors);
        } else {
            self.diagnostics.clear(uri.clone());
        }
        for error in errors {
            self.log(error.to_string());
        }
    }

    fn doctor(&self) -> Value {
        let roots = self.workspace.roots();
        let default = self.settings.default_settings();
        let folders: Vec<(PathBuf, Settings)> = roots
            .iter()
            .map(|root| (root.clone(), self.settings_for_root(root)))
            .collect();
        let projects = self.tsc.status(&folders);
        json!({
            "server": {
                "name": "dotfmt-lsp",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "trusted": self.trusted(),
            "typeAware": default.type_aware,
            "syntax": default.syntax_diagnostics,
            "tscScope": default.tsc_scope.as_str(),
            "default": settings_json(&default),
            "folders": folders
                .iter()
                .map(|(root, settings)| {
                    let mut value = settings_json(settings);
                    value["root"] = json!(root.display().to_string());
                    value
                })
                .collect::<Vec<_>>(),
            "roots": roots
                .iter()
                .map(|root| root.display().to_string())
                .collect::<Vec<_>>(),
            "projects": projects
                .iter()
                .map(|project| json!({
                    "root": project.root.display().to_string(),
                    "project": project.project.display().to_string(),
                    "tsconfig": project
                        .tsconfig
                        .as_ref()
                        .map(|path| path.display().to_string()),
                    "compiler": project
                        .compiler
                        .as_ref()
                        .map(|path| path.display().to_string()),
                    "version": project.version,
                    "state": project.state,
                }))
                .collect::<Vec<_>>(),
        })
    }
}

fn settings_json(settings: &Settings) -> Value {
    json!({
        "typeAware": settings.type_aware,
        "syntax": settings.syntax_diagnostics,
        "tscScope": settings.tsc_scope.as_str(),
        "debounceMs": settings.debounce_ms,
        "fixesOnSave": settings.fixes_on_save.as_str(),
        "tsdk": settings.tsdk.as_ref().map(|path| path.display().to_string()),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FormatTextParams {
    text: String,
    language_id: Option<String>,
    uri: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExplainParams {
    text_document: lsp_types::TextDocumentIdentifier,
    language_id: Option<String>,
}

#[derive(Deserialize)]
struct CancelParams {
    id: NumberOrString,
}

#[derive(Debug)]
struct RpcError {
    code: i32,
    message: String,
}

impl RpcError {
    fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::InvalidParams as i32,
            message: message.into(),
        }
    }

    fn content_modified(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::ContentModified as i32,
            message: message.into(),
        }
    }

    fn request_cancelled() -> Self {
        Self {
            code: ErrorCode::RequestCanceled as i32,
            message: "request cancelled".into(),
        }
    }
}

fn parse<T: DeserializeOwned>(value: &Value) -> std::result::Result<T, RpcError> {
    serde_json::from_value(value.clone())
        .map_err(|error| RpcError::invalid_params(error.to_string()))
}

/// The queued format/on-type work a request supersedes. Only formatting
/// families coalesce; will-save and code-action requests never do.
fn coalesce_key(method: &str, params: &Value) -> Option<(Uri, &'static str)> {
    let family = match method {
        Formatting::METHOD | RangeFormatting::METHOD | RangesFormatting::METHOD => "format",
        OnTypeFormatting::METHOD => "onType",
        _ => return None,
    };
    let uri = params
        .get("textDocument")
        .and_then(|document| document.get("uri"))
        .and_then(Value::as_str)
        .and_then(|uri| uri.parse().ok())?;
    Some((uri, family))
}

fn supported(uri: &Uri, language_id: &str) -> bool {
    uri::path(uri)
        .and_then(|path| Language::for_path(&path))
        .is_some()
        || analysis::language_for(language_id).is_some()
}

fn is_config_file(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("dotfmt.dotfile" | ".prettierignore")
    )
}

fn capabilities(pull: bool) -> ServerCapabilities {
    ServerCapabilities {
        diagnostic_provider: pull.then(|| {
            DiagnosticServerCapabilities::Options(DiagnosticOptions {
                identifier: Some("dotfmt".into()),
                inter_file_dependencies: true,
                workspace_diagnostics: false,
                ..DiagnosticOptions::default()
            })
        }),
        position_encoding: Some(PositionEncodingKind::UTF16),
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::INCREMENTAL),
                will_save: Some(true),
                will_save_wait_until: Some(true),
                save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                    include_text: Some(false),
                })),
            },
        )),
        document_formatting_provider: Some(OneOf::Left(true)),
        document_range_formatting_provider: Some(OneOf::Left(true)),
        document_on_type_formatting_provider: Some(DocumentOnTypeFormattingOptions {
            first_trigger_character: ";".into(),
            more_trigger_character: Some(vec!["}".into()]),
        }),
        code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
            code_action_kinds: Some(vec![
                CodeActionKind::QUICKFIX,
                CodeActionKind::from(actions::FIX_ALL),
                CodeActionKind::from(actions::ORGANIZE_IMPORTS),
                CodeActionKind::from(actions::FORMAT),
            ]),
            resolve_provider: Some(false),
            ..CodeActionOptions::default()
        })),
        execute_command_provider: Some(ExecuteCommandOptions {
            commands: COMMANDS
                .iter()
                .map(|command| (*command).to_owned())
                .collect(),
            ..ExecuteCommandOptions::default()
        }),
        workspace: Some(WorkspaceServerCapabilities {
            workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                supported: Some(true),
                change_notifications: Some(OneOf::Left(true)),
            }),
            file_operations: None,
        }),
        ..ServerCapabilities::default()
    }
}

fn workspace_roots(params: &InitializeParams) -> Vec<PathBuf> {
    if let Some(folders) = &params.workspace_folders
        && !folders.is_empty()
    {
        return folders
            .iter()
            .filter_map(|folder| uri::path(&folder.uri))
            .collect();
    }
    #[allow(deprecated)]
    params
        .root_uri
        .as_ref()
        .and_then(uri::path)
        .into_iter()
        .collect()
}

fn block(document: &Document, position: lsp_types::Position) -> Range {
    let line = (position.line as usize).min(document.line_count().saturating_sub(1));
    let mut first = line;
    while first > 0 && line - first < 40 && !document.line_text(first - 1).trim().is_empty() {
        first -= 1;
    }
    let count = document.line_count();
    let mut last = line.min(count.saturating_sub(1));
    while last + 1 < count && last - line < 40 && !document.line_text(last + 1).trim().is_empty() {
        last += 1;
    }
    document.lsp_range(&document.line_range(first, last))
}

/// Expands each UTF-16 range to the enclosing TypeScript/JavaScript syntax
/// node. Other languages keep the requested byte range.
fn expanded_ranges(
    document: &Document,
    path: &Path,
    ranges: &[Range],
) -> Vec<std::ops::Range<usize>> {
    let dialect = dotfmt_typescript::Dialect::for_path(path);
    ranges
        .iter()
        .filter_map(|range| {
            let range = document.range(*range)?;
            let range = match dialect {
                Some(dialect) => dotfmt_typescript::expand_range(
                    path,
                    document.text(),
                    range.start,
                    range.end,
                    dialect,
                )
                .map_or(range, |(start, end)| start..end),
                None => range,
            };
            Some(full_lines(document, range))
        })
        .collect()
}

/// Range edits are line-oriented, so a syntax node or cursor selection expands
/// to the whole lines it touches. This keeps every emitted edit contained in
/// the range while never rewriting a statement outside it.
fn full_lines(document: &Document, range: std::ops::Range<usize>) -> std::ops::Range<usize> {
    let start = document.position(range.start);
    let end = document.position(range.end);
    let first = start.line as usize;
    let last = end.line as usize;
    let start = document.line_start(first);
    let end = if end.character == 0 && last > first {
        document.line_start(last)
    } else {
        document.line_start(last.saturating_add(1))
    };
    start..end.max(start)
}

/// The syntax node for a zero-width on-type cursor, or the line block for
/// languages without TypeScript syntax support.
fn on_type_range(document: &Document, path: &Path, position: lsp_types::Position) -> Range {
    let Some(dialect) = dotfmt_typescript::Dialect::for_path(path) else {
        return block(document, position);
    };
    let Some(offset) = document.offset(position) else {
        return block(document, position);
    };
    let Some((start, end)) =
        dotfmt_typescript::expand_range(path, document.text(), offset, offset, dialect)
    else {
        return block(document, position);
    };
    document.lsp_range(&(start..end))
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use lsp_server::{Connection, Message, Notification, Request, RequestId};
    use serde_json::json;

    use super::*;

    fn receive_response(
        client: &Connection,
        id: &RequestId,
    ) -> std::result::Result<Value, lsp_server::ResponseError> {
        loop {
            let message = client
                .receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("server response");
            match message {
                Message::Response(response) if response.id == *id => {
                    return response.response_result;
                }
                Message::Response(_) | Message::Notification(_) | Message::Request(_) => continue,
            }
        }
    }

    fn server_with_options(
        root: &Path,
        options: Value,
    ) -> (Arc<Server>, crossbeam_channel::Receiver<Message>) {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let mut value = json!({
            "processId": null,
            "rootUri": uri::from_path(root),
            "capabilities": {},
        });
        if !options.is_null() {
            value["initializationOptions"] = options;
        }
        let params: InitializeParams = serde_json::from_value(value).unwrap();
        (Arc::new(Server::new(sender, &params)), receiver)
    }

    fn test_server(root: &Path) -> Arc<Server> {
        server_with_options(root, Value::Null).0
    }

    fn server_with_roots(
        roots: &[&Path],
        options: Value,
        capabilities: Value,
    ) -> (Arc<Server>, crossbeam_channel::Receiver<Message>) {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let folders: Vec<Value> = roots
            .iter()
            .enumerate()
            .map(|(index, root)| {
                json!({ "uri": uri::from_path(root), "name": format!("root-{index}") })
            })
            .collect();
        let mut value = json!({
            "processId": null,
            "capabilities": capabilities,
            "workspaceFolders": folders,
        });
        if !options.is_null() {
            value["initializationOptions"] = options;
        }
        let params: InitializeParams = serde_json::from_value(value).unwrap();
        (Arc::new(Server::new(sender, &params)), receiver)
    }

    fn receive_request(client: &Connection, method: &str) -> Option<Request> {
        loop {
            match client.receiver.recv_timeout(Duration::from_secs(5)).ok()? {
                Message::Request(request) if request.method == method => return Some(request),
                _ => continue,
            }
        }
    }

    fn receive_notification(client: &Connection, method: &str) -> Option<lsp_server::Notification> {
        loop {
            let message = client.receiver.recv_timeout(Duration::from_secs(5)).ok()?;
            match message {
                Message::Notification(notification) if notification.method == method => {
                    return Some(notification);
                }
                Message::Response(_) | Message::Notification(_) | Message::Request(_) => continue,
            }
        }
    }

    fn apply_edits(text: &str, edits: &[TextEdit]) -> String {
        let document = Document::new(text.into(), "typescript".into(), 1);
        let mut ranges: Vec<(std::ops::Range<usize>, String)> = edits
            .iter()
            .map(|edit| {
                (
                    document.range(edit.range).expect("edit range"),
                    edit.new_text.clone(),
                )
            })
            .collect();
        ranges.sort_by_key(|(range, _)| range.start);
        let mut output = text.to_owned();
        for (range, new_text) in ranges.into_iter().rev() {
            output.replace_range(range, &new_text);
        }
        output
    }

    #[test]
    fn serves_initialize_formatting_and_shutdown_over_channels() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("dotfmt.dotfile"), "json {}\n").unwrap();
        let path = temporary.path().join("a.json");
        let uri = uri::from_path(&path);
        let root = uri::from_path(temporary.path());

        let (server, client) = Connection::memory();
        let serving = std::thread::spawn(move || run(&server));

        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(1),
                "initialize".into(),
                json!({
                    "processId": null,
                    "rootUri": root,
                    "capabilities": {
                        "workspace": {
                            "workspaceEdit": { "documentChanges": true }
                        }
                    }
                }),
            )))
            .unwrap();
        let initialize = receive_response(&client, &RequestId::from(1)).unwrap();
        assert_eq!(
            initialize["capabilities"]["documentRangeFormattingProvider"]["rangesSupport"],
            json!(true)
        );
        assert_eq!(
            initialize["capabilities"]["documentOnTypeFormattingProvider"]["firstTriggerCharacter"],
            json!(";")
        );
        let kinds = initialize["capabilities"]["codeActionProvider"]["codeActionKinds"]
            .as_array()
            .unwrap();
        assert!(kinds.iter().any(|kind| kind == "quickfix"));
        assert!(kinds.iter().any(|kind| kind == "source.fixAll.dotfmt"));
        assert!(!kinds.iter().any(|kind| kind == "source.fixAll"));
        assert!(!kinds.iter().any(|kind| kind == "source.organizeImports"));

        client
            .sender
            .send(Message::Notification(Notification::new(
                "initialized".into(),
                json!({}),
            )))
            .unwrap();
        client
            .sender
            .send(Message::Notification(Notification::new(
                "textDocument/didOpen".into(),
                json!({
                    "textDocument": {
                        "uri": uri,
                        "languageId": "json",
                        "version": 1,
                        "text": "{\"a\":1}\n"
                    }
                }),
            )))
            .unwrap();
        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(2),
                "dotfmt/explain".into(),
                json!({
                    "textDocument": { "uri": uri },
                    "languageId": "json"
                }),
            )))
            .unwrap();
        let explanation = receive_response(&client, &RequestId::from(2)).unwrap();
        assert_eq!(explanation["language"], json!("json"));
        let configs = explanation["configs"].as_array().unwrap();
        assert!(!configs.is_empty());
        assert!(configs.iter().any(|config| {
            config
                .as_str()
                .is_some_and(|config| config.ends_with("dotfmt.dotfile"))
        }));

        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(3),
                "textDocument/formatting".into(),
                json!({
                    "textDocument": { "uri": uri },
                    "options": { "tabSize": 2, "insertSpaces": true }
                }),
            )))
            .unwrap();
        let formatted = receive_response(&client, &RequestId::from(3)).unwrap();
        let edits = formatted.as_array().expect("formatting edits");
        assert!(!edits.is_empty());
        assert!(
            edits.iter().any(|edit| edit["newText"]
                .as_str()
                .is_some_and(|text| text.contains("\"a\": 1"))),
            "{edits:?}"
        );

        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(4),
                "textDocument/rangesFormatting".into(),
                json!({
                    "textDocument": { "uri": uri },
                    "ranges": [{
                        "start": { "line": 0, "character": 0 },
                        "end": { "line": 1, "character": 0 }
                    }],
                    "options": { "tabSize": 2, "insertSpaces": true }
                }),
            )))
            .unwrap();
        let ranged = receive_response(&client, &RequestId::from(4)).unwrap();
        assert!(!ranged.as_array().expect("range edits").is_empty());

        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(5),
                "shutdown".into(),
                json!(null),
            )))
            .unwrap();
        let _ = receive_response(&client, &RequestId::from(5));
        client
            .sender
            .send(Message::Notification(Notification::new(
                "exit".into(),
                json!(null),
            )))
            .unwrap();
        serving.join().unwrap().unwrap();
    }

    #[test]
    fn shutdown_tolerates_late_configuration_response() {
        let temporary = tempfile::tempdir().unwrap();
        let (server, client) = Connection::memory();
        let serving = std::thread::spawn(move || run(&server));

        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(1),
                "initialize".into(),
                json!({
                    "processId": null,
                    "rootUri": uri::from_path(temporary.path()),
                    "capabilities": { "workspace": { "configuration": true } }
                }),
            )))
            .unwrap();
        receive_response(&client, &RequestId::from(1)).unwrap();
        client
            .sender
            .send(Message::Notification(Notification::new(
                "initialized".into(),
                json!({}),
            )))
            .unwrap();
        let configuration = loop {
            match client
                .receiver
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
            {
                Message::Request(request) if request.method == "workspace/configuration" => {
                    break request;
                }
                _ => continue,
            }
        };

        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(2),
                "shutdown".into(),
                json!(null),
            )))
            .unwrap();
        receive_response(&client, &RequestId::from(2)).unwrap();
        client
            .sender
            .send(Message::Response(Response::new_ok(
                configuration.id,
                json!([{}]),
            )))
            .unwrap();
        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(3),
                "textDocument/formatting".into(),
                json!(null),
            )))
            .unwrap();
        let rejected = receive_response(&client, &RequestId::from(3)).unwrap_err();
        assert_eq!(rejected.code, ErrorCode::InvalidRequest as i32);
        client
            .sender
            .send(Message::Notification(Notification::new(
                "exit".into(),
                json!(null),
            )))
            .unwrap();
        serving.join().unwrap().unwrap();
    }

    #[test]
    fn range_formatting_expands_a_jsx_selection_to_the_element() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("dotfmt.dotfile"), "typescript {}\n").unwrap();
        let path = temporary.path().join("App.tsx");
        let text = "const App = () => {\n  return (\n    <section>\n      <span className=\"hero\"    data-kind=\"text\">world</span>\n    </section>\n  );\n};\n";
        std::fs::write(&path, text).unwrap();
        let server = test_server(temporary.path());
        let document = Document::new(text.into(), "typescriptreact".into(), 1);
        let uri = uri::from_path(&path);
        server
            .documents
            .lock()
            .unwrap()
            .open(uri.clone(), document.clone());
        let start = text.find("world").unwrap();
        let range = document.lsp_range(&(start..start + "world".len()));

        let edits = server
            .format_document(&uri, &document, &[range], false, None)
            .unwrap_or_else(|error| panic!("formatting failed: {}", error.message));
        assert!(!edits.is_empty());
        assert!(
            edits
                .iter()
                .all(|edit| edit.range.start.line >= 3 && edit.range.end.line <= 4),
            "{edits:?}"
        );
        assert_eq!(
            apply_edits(text, &edits),
            "const App = () => {\n  return (\n    <section>\n      <span className=\"hero\" data-kind=\"text\">\n        world\n      </span>\n    </section>\n  );\n};\n"
        );
    }

    #[test]
    fn range_formatting_expands_a_selection_inside_a_multiline_object() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("dotfmt.dotfile"), "typescript {}\n").unwrap();
        let path = temporary.path().join("config.ts");
        let text = "const config = {\n  nested: {\n    first:\"one\",\n    second:\"two\",\n  },\n  untouched: true,\n};\n";
        std::fs::write(&path, text).unwrap();
        let server = test_server(temporary.path());
        let document = Document::new(text.into(), "typescript".into(), 1);
        let uri = uri::from_path(&path);
        server
            .documents
            .lock()
            .unwrap()
            .open(uri.clone(), document.clone());
        let start = text.find("one").unwrap();
        let range = document.lsp_range(&(start..start + "one".len()));

        let edits = server
            .format_document(&uri, &document, &[range], false, None)
            .unwrap_or_else(|error| panic!("formatting failed: {}", error.message));
        assert!(!edits.is_empty());
        assert!(
            edits
                .iter()
                .all(|edit| edit.range.start.line >= 2 && edit.range.end.line <= 5),
            "{edits:?}"
        );
        let formatted = apply_edits(text, &edits);
        assert_eq!(
            formatted,
            "const config = {\n  nested: {\n    first: \"one\",\n    second: \"two\",\n  },\n  untouched: true,\n};\n"
        );
        let before: Vec<&str> = text.lines().collect();
        let after: Vec<&str> = formatted.lines().collect();
        for line in [0, 1, 5, 6] {
            assert_eq!(after[line], before[line]);
        }
    }

    #[test]
    fn on_type_formatting_expands_the_cursor_inside_a_function() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("dotfmt.dotfile"), "typescript {}\n").unwrap();
        let path = temporary.path().join("add.ts");
        let text = "function add(a: number, b: number) {\n  const sum=a+b;\n  return sum;\n}\nconst untouched = 1;\n";
        std::fs::write(&path, text).unwrap();
        let server = test_server(temporary.path());
        let document = Document::new(text.into(), "typescript".into(), 1);
        let uri = uri::from_path(&path);
        server
            .documents
            .lock()
            .unwrap()
            .open(uri.clone(), document.clone());
        let position =
            document.position(text.find("const sum=a+b;").unwrap() + "const sum=a+b;".len());

        let result = server
            .handle(
                OnTypeFormatting::METHOD,
                &json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": position.line, "character": position.character },
                    "ch": ";",
                    "options": { "tabSize": 2, "insertSpaces": true }
                }),
                &RequestState::default(),
            )
            .unwrap_or_else(|error| panic!("on-type formatting failed: {}", error.message));
        let edits: Vec<TextEdit> = serde_json::from_value(result).unwrap();
        assert!(!edits.is_empty());
        assert!(
            edits
                .iter()
                .all(|edit| edit.range.start.line >= 1 && edit.range.end.line <= 2),
            "{edits:?}"
        );
        let formatted = apply_edits(text, &edits);
        assert_eq!(
            formatted,
            "function add(a: number, b: number) {\n  const sum = a + b;\n  return sum;\n}\nconst untouched = 1;\n"
        );
        let before: Vec<&str> = text.lines().collect();
        let after: Vec<&str> = formatted.lines().collect();
        for line in [0, 2, 3, 4] {
            assert_eq!(after[line], before[line]);
        }
    }

    #[test]
    fn on_type_range_falls_back_to_the_line_block_outside_typescript() {
        let text = "{\n\"a\":1\n}\n";
        let document = Document::new(text.into(), "json".into(), 1);
        let path = Path::new("a.json");
        let position = lsp_types::Position::new(1, 5);
        assert_eq!(
            on_type_range(&document, path, position),
            block(&document, position)
        );
    }

    #[test]
    fn defaults_to_untrusted_and_accepts_explicit_initialization_trust() {
        let temporary = tempfile::tempdir().unwrap();
        let server = test_server(temporary.path());
        assert!(!server.trusted());
        let (server, _) = server_with_options(temporary.path(), json!({ "trusted": true }));
        assert!(server.trusted());
        let (server, _) =
            server_with_options(temporary.path(), json!({ "dotfmt": { "trusted": true } }));
        assert!(server.trusted());
    }

    #[test]
    fn folder_configuration_cannot_enable_trust() {
        let temporary = tempfile::tempdir().unwrap();
        let server = test_server(temporary.path());
        assert!(!server.trusted());
        server
            .settings
            .configure_folder(temporary.path(), &json!({ "dotfmt.trusted": true }));
        assert!(!server.trusted());
        server.configure_push(&json!({ "dotfmt.trusted": true }));
        assert!(!server.trusted());
    }

    #[test]
    fn initialize_logs_the_effective_trust_level_once() {
        let temporary = tempfile::tempdir().unwrap();
        let (server, client) = Connection::memory();
        let serving = std::thread::spawn(move || run(&server));
        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(1),
                "initialize".into(),
                json!({
                    "processId": null,
                    "rootUri": uri::from_path(temporary.path()),
                    "capabilities": {},
                }),
            )))
            .unwrap();
        receive_response(&client, &RequestId::from(1)).unwrap();
        client
            .sender
            .send(Message::Notification(Notification::new(
                "initialized".into(),
                json!({}),
            )))
            .unwrap();
        let log = receive_notification(&client, "window/logMessage").expect("trust log");
        let message = log.params["message"].as_str().unwrap();
        assert!(message.contains("dotfmt trust: untrusted"), "{message}");
        let deadline = Instant::now() + Duration::from_millis(250);
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            match client.receiver.recv_timeout(remaining) {
                Ok(Message::Notification(notification))
                    if notification.method == "window/logMessage"
                        && notification.params["message"]
                            .as_str()
                            .is_some_and(|message| message.contains("dotfmt trust")) =>
                {
                    panic!("trust was logged more than once");
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(2),
                "shutdown".into(),
                json!(null),
            )))
            .unwrap();
        let _ = receive_response(&client, &RequestId::from(2));
        client
            .sender
            .send(Message::Notification(Notification::new(
                "exit".into(),
                json!(null),
            )))
            .unwrap();
        serving.join().unwrap().unwrap();
    }

    #[test]
    fn out_of_range_change_desynchronises_and_did_open_recovers() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("dotfmt.dotfile"), "typescript {}\n").unwrap();
        let path = temporary.path().join("a.ts");
        let text = "const value=1;\n";
        std::fs::write(&path, text).unwrap();
        let server = test_server(temporary.path());
        let uri = uri::from_path(&path);
        let document = Document::new(text.into(), "typescript".into(), 1);
        server.documents.lock().unwrap().open(uri.clone(), document);
        server.notify(Notification::new(
            DidChangeTextDocument::METHOD.into(),
            json!({
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [{
                    "range": {
                        "start": { "line": 99, "character": 0 },
                        "end": { "line": 99, "character": 1 }
                    },
                    "text": "x"
                }]
            }),
        ));
        assert_eq!(
            server.document(&uri).unwrap_err().code,
            ErrorCode::ContentModified as i32
        );
        let error = server
            .handle(
                Formatting::METHOD,
                &json!({
                    "textDocument": { "uri": uri },
                    "options": { "tabSize": 2, "insertSpaces": true }
                }),
                &RequestState::default(),
            )
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::ContentModified as i32);

        server.notify(Notification::new(
            DidOpenTextDocument::METHOD.into(),
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "typescript",
                    "version": 3,
                    "text": text
                }
            }),
        ));
        assert!(!server.document(&uri).unwrap().is_desynchronised());
        let formatted = server
            .handle(
                Formatting::METHOD,
                &json!({
                    "textDocument": { "uri": uri },
                    "options": { "tabSize": 2, "insertSpaces": true }
                }),
                &RequestState::default(),
            )
            .unwrap();
        assert!(!formatted.as_array().unwrap().is_empty());
    }

    #[test]
    fn full_text_change_clears_desynchronisation() {
        let temporary = tempfile::tempdir().unwrap();
        let server = test_server(temporary.path());
        let path = temporary.path().join("a.json");
        let uri = uri::from_path(&path);
        server.documents.lock().unwrap().open(
            uri.clone(),
            Document::new("{\"a\":1}\n".into(), "json".into(), 1),
        );
        server.notify(Notification::new(
            DidChangeTextDocument::METHOD.into(),
            json!({
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [{
                    "range": {
                        "start": { "line": 99, "character": 0 },
                        "end": { "line": 99, "character": 1 }
                    },
                    "text": "x"
                }]
            }),
        ));
        assert!(server.document(&uri).is_err());
        server.notify(Notification::new(
            DidChangeTextDocument::METHOD.into(),
            json!({
                "textDocument": { "uri": uri, "version": 3 },
                "contentChanges": [{ "text": "{\"b\":2}\n" }]
            }),
        ));
        let document = server.document(&uri).unwrap();
        assert!(!document.is_desynchronised());
        assert_eq!(document.text(), "{\"b\":2}\n");
    }

    #[test]
    fn unknown_cancel_does_not_create_in_flight_state() {
        let temporary = tempfile::tempdir().unwrap();
        let server = test_server(temporary.path());
        server.cancel(&json!({ "id": 41 }));
        assert!(server.in_flight.lock().unwrap().is_empty());
        assert!(server.queued.lock().unwrap().is_empty());
    }

    #[test]
    fn cancelling_an_in_flight_request_returns_request_canceled() {
        let temporary = tempfile::tempdir().unwrap();
        let server = test_server(temporary.path());
        let state = RequestState::default();
        state.cancel();
        let error = server
            .handle(Formatting::METHOD, &json!(null), &state)
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::RequestCanceled as i32);
    }

    #[test]
    fn coalescing_is_per_uri_and_method_family() {
        let temporary = tempfile::tempdir().unwrap();
        let server = Arc::new(test_server(temporary.path()));
        let first = uri::from_path(&temporary.path().join("a.ts"));
        let second = uri::from_path(&temporary.path().join("b.ts"));
        let formatting = |id: i32, uri: &Uri| {
            Request::new(
                RequestId::from(id),
                Formatting::METHOD.into(),
                json!({
                    "textDocument": { "uri": uri },
                    "options": { "tabSize": 2, "insertSpaces": true }
                }),
            )
        };
        server.dispatch(formatting(1, &first));
        server.dispatch(formatting(2, &second));
        let superseded = |id: i32| {
            server
                .in_flight
                .lock()
                .unwrap()
                .get(&RequestId::from(id))
                .expect("in-flight request")
                .is_superseded()
        };
        assert!(!superseded(1));
        assert!(!superseded(2));
        server.dispatch(formatting(3, &first));
        assert!(superseded(1));
        assert!(!superseded(2));
        assert!(!superseded(3));

        server.dispatch(Request::new(
            RequestId::from(4),
            OnTypeFormatting::METHOD.into(),
            json!({
                "textDocument": { "uri": first },
                "position": { "line": 0, "character": 0 },
                "ch": ";",
                "options": { "tabSize": 2, "insertSpaces": true }
            }),
        ));
        assert!(!superseded(3));

        let queued = server.queued.lock().unwrap().len();
        server.dispatch(Request::new(
            RequestId::from(5),
            WillSaveWaitUntil::METHOD.into(),
            json!({ "textDocument": { "uri": first }, "reason": 1 }),
        ));
        assert_eq!(server.queued.lock().unwrap().len(), queued);
    }

    #[test]
    fn a_burst_of_on_type_requests_is_bounded_and_coalesced() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("dotfmt.dotfile"), "typescript {}\n").unwrap();
        let path = temporary.path().join("a.ts");
        let text = "const value=1;\n";
        std::fs::write(&path, text).unwrap();
        let uri = uri::from_path(&path);
        let (server, client) = Connection::memory();
        let serving = std::thread::spawn(move || run(&server));
        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(1),
                "initialize".into(),
                json!({
                    "processId": null,
                    "rootUri": uri::from_path(temporary.path()),
                    "capabilities": {},
                }),
            )))
            .unwrap();
        receive_response(&client, &RequestId::from(1)).unwrap();
        client
            .sender
            .send(Message::Notification(Notification::new(
                "initialized".into(),
                json!({}),
            )))
            .unwrap();
        client
            .sender
            .send(Message::Notification(Notification::new(
                DidOpenTextDocument::METHOD.into(),
                json!({
                    "textDocument": {
                        "uri": uri,
                        "languageId": "typescript",
                        "version": 1,
                        "text": text
                    }
                }),
            )))
            .unwrap();
        let started = Instant::now();
        for id in 100..150 {
            client
                .sender
                .send(Message::Request(Request::new(
                    RequestId::from(id),
                    OnTypeFormatting::METHOD.into(),
                    json!({
                        "textDocument": { "uri": uri },
                        "position": { "line": 0, "character": 15 },
                        "ch": ";",
                        "options": { "tabSize": 2, "insertSpaces": true }
                    }),
                )))
                .unwrap();
        }
        let mut coalesced = 0;
        let mut analyzed = 0;
        for _ in 0..50 {
            let message = client
                .receiver
                .recv_timeout(Duration::from_secs(20))
                .expect("burst response");
            let Message::Response(response) = message else {
                continue;
            };
            let result = response.response_result.expect("burst response result");
            if result.is_null() {
                coalesced += 1;
            } else {
                analyzed += 1;
            }
        }
        assert!(coalesced >= 40, "coalesced={coalesced} analyzed={analyzed}");
        assert!(analyzed <= 10, "coalesced={coalesced} analyzed={analyzed}");
        assert!(started.elapsed() < Duration::from_secs(10));

        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(2),
                "shutdown".into(),
                json!(null),
            )))
            .unwrap();
        let _ = receive_response(&client, &RequestId::from(2));
        client
            .sender
            .send(Message::Notification(Notification::new(
                "exit".into(),
                json!(null),
            )))
            .unwrap();
        serving.join().unwrap().unwrap();
    }

    #[test]
    fn initialized_registers_watched_files_once() {
        let temporary = tempfile::tempdir().unwrap();
        let (server, client) = Connection::memory();
        let serving = std::thread::spawn(move || run(&server));
        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(1),
                "initialize".into(),
                json!({
                    "processId": null,
                    "rootUri": uri::from_path(temporary.path()),
                    "capabilities": {},
                }),
            )))
            .unwrap();
        receive_response(&client, &RequestId::from(1)).unwrap();
        client
            .sender
            .send(Message::Notification(Notification::new(
                "initialized".into(),
                json!({}),
            )))
            .unwrap();
        let registration = loop {
            let message = client
                .receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("registration request");
            match message {
                Message::Request(request) if request.method == "client/registerCapability" => {
                    break request;
                }
                _ => continue,
            }
        };
        let watchers = registration.params["registrations"][0]["registerOptions"]["watchers"]
            .as_array()
            .expect("watchers");
        let globs: Vec<&str> = watchers
            .iter()
            .filter_map(|watcher| watcher["globPattern"].as_str())
            .collect();
        for expected in [
            "**/dotfmt.dotfile",
            "**/.prettierignore",
            "**/tsconfig*.json",
            "**/jsconfig*.json",
        ] {
            assert!(globs.contains(&expected), "{globs:?}");
        }
        assert!(
            globs
                .iter()
                .any(|glob| glob.ends_with("dotfmt/dotfmt.dotfile")),
            "{globs:?}"
        );

        client
            .sender
            .send(Message::Notification(Notification::new(
                "initialized".into(),
                json!({}),
            )))
            .unwrap();
        let deadline = Instant::now() + Duration::from_millis(250);
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            match client.receiver.recv_timeout(remaining) {
                Ok(Message::Request(request)) if request.method == "client/registerCapability" => {
                    panic!("watchers registered twice");
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }

        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(2),
                "shutdown".into(),
                json!(null),
            )))
            .unwrap();
        let _ = receive_response(&client, &RequestId::from(2));
        client
            .sender
            .send(Message::Notification(Notification::new(
                "exit".into(),
                json!(null),
            )))
            .unwrap();
        serving.join().unwrap().unwrap();
    }

    #[test]
    fn prettierignore_changes_invalidate_sessions_like_config_changes() {
        let temporary = tempfile::tempdir().unwrap();
        let server = test_server(temporary.path());
        let path = temporary.path().join("a.ts");
        let before = server.workspace.session(&path);
        server.notify(Notification::new(
            DidChangeWatchedFiles::METHOD.into(),
            json!({
                "changes": [{
                    "uri": uri::from_path(&temporary.path().join(".prettierignore")),
                    "type": 2
                }]
            }),
        ));
        let after = server.workspace.session(&path);
        assert!(!Arc::ptr_eq(&before, &after));
    }

    #[test]
    fn unsupported_documents_do_not_publish_diagnostics() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("notes.txt");
        let uri = uri::from_path(&path);
        let (server, client) = Connection::memory();
        let serving = std::thread::spawn(move || run(&server));
        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(1),
                "initialize".into(),
                json!({
                    "processId": null,
                    "rootUri": uri::from_path(temporary.path()),
                    "capabilities": {},
                }),
            )))
            .unwrap();
        receive_response(&client, &RequestId::from(1)).unwrap();
        client
            .sender
            .send(Message::Notification(Notification::new(
                "initialized".into(),
                json!({}),
            )))
            .unwrap();
        client
            .sender
            .send(Message::Notification(Notification::new(
                DidOpenTextDocument::METHOD.into(),
                json!({
                    "textDocument": {
                        "uri": uri,
                        "languageId": "plaintext",
                        "version": 1,
                        "text": "hello\n"
                    }
                }),
            )))
            .unwrap();
        let deadline = Instant::now() + Duration::from_millis(600);
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            match client.receiver.recv_timeout(remaining) {
                Ok(Message::Notification(notification))
                    if notification.method == "textDocument/publishDiagnostics" =>
                {
                    assert_ne!(
                        notification.params["uri"].as_str(),
                        Some(uri.as_str()),
                        "unsupported document received diagnostics"
                    );
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(2),
                "shutdown".into(),
                json!(null),
            )))
            .unwrap();
        let _ = receive_response(&client, &RequestId::from(2));
        client
            .sender
            .send(Message::Notification(Notification::new(
                "exit".into(),
                json!(null),
            )))
            .unwrap();
        serving.join().unwrap().unwrap();
    }

    #[test]
    fn out_of_order_change_is_logged() {
        let temporary = tempfile::tempdir().unwrap();
        let (server, receiver) = server_with_options(temporary.path(), Value::Null);
        let path = temporary.path().join("a.json");
        let uri = uri::from_path(&path);
        server.documents.lock().unwrap().open(
            uri.clone(),
            Document::new("{\"a\":1}\n".into(), "json".into(), 5),
        );
        server.notify(Notification::new(
            DidChangeTextDocument::METHOD.into(),
            json!({
                "textDocument": { "uri": uri, "version": 4 },
                "contentChanges": [{ "text": "{\"b\":2}\n" }]
            }),
        ));
        assert_eq!(server.document(&uri).unwrap().text(), "{\"a\":1}\n");
        loop {
            let message = receiver
                .recv_timeout(Duration::from_secs(2))
                .expect("out-of-order log");
            if let Message::Notification(notification) = message
                && notification.method == "window/logMessage"
                && notification.params["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("out-of-order didChange"))
            {
                break;
            }
        }
    }

    #[test]
    fn range_formatting_stays_inside_a_multiline_hunk() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("dotfmt.dotfile"), "json {}\n").unwrap();
        let path = temporary.path().join("a.json");
        let text = "{\n\"a\":1,\n\"b\":2,\n\"c\":3\n}\n";
        std::fs::write(&path, text).unwrap();
        let server = test_server(temporary.path());
        let uri = uri::from_path(&path);
        let document = Document::new(text.into(), "json".into(), 1);
        server
            .documents
            .lock()
            .unwrap()
            .open(uri.clone(), document.clone());
        let range = document.line_range(2, 2);
        let range = document.lsp_range(&range);
        let edits = server
            .format_document(&uri, &document, &[range], false, None)
            .unwrap_or_else(|error| panic!("formatting failed: {}", error.message));
        assert!(!edits.is_empty());
        assert!(
            edits
                .iter()
                .all(|edit| edit.range.start.line >= 2 && edit.range.end.line <= 3),
            "{edits:?}"
        );
        let formatted = apply_edits(text, &edits);
        let before: Vec<&str> = text.lines().collect();
        let after: Vec<&str> = formatted.lines().collect();
        assert_eq!(after[1], before[1]);
        assert_eq!(after[3], before[3]);
        assert_eq!(after[2], "  \"b\": 2,");
    }

    #[cfg(unix)]
    fn write_fake_tsc(root: &Path, output: &str) {
        let directory = root.join("node_modules/.bin");
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("tsc");
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'Version 9.9.9'; exit 0; fi\n\
                 echo run >> marker.log\n{output}\n"
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();
    }

    #[cfg(unix)]
    fn wait_for_tsc_diagnostics(
        receiver: &crossbeam_channel::Receiver<Message>,
        uri: &Uri,
    ) -> lsp_types::PublishDiagnosticsParams {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .expect("timed out waiting for tsc diagnostics");
            let message = receiver.recv_timeout(remaining).expect("server message");
            let Message::Notification(notification) = message else {
                continue;
            };
            if notification.method != "textDocument/publishDiagnostics" {
                continue;
            }
            let params: lsp_types::PublishDiagnosticsParams =
                serde_json::from_value(notification.params).unwrap();
            if &params.uri == uri
                && params
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.source.as_deref() == Some("dotfmt(tsc)"))
            {
                return params;
            }
        }
    }

    #[cfg(unix)]
    fn open_typescript(server: &Arc<Server>, root: &Path, text: &str) -> (PathBuf, Uri) {
        let path = root.join("src/a.ts");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        let uri = uri::from_path(&path);
        server.notify(Notification::new(
            DidOpenTextDocument::METHOD.into(),
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "typescript",
                    "version": 1,
                    "text": text
                }
            }),
        ));
        (path, uri)
    }

    #[cfg(unix)]
    #[test]
    fn a_tsconfig_change_invalidates_and_reruns_tsc() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::write(root.join("tsconfig.json"), "{}\n").unwrap();
        std::fs::write(root.join("dotfmt.dotfile"), "typescript {}\n").unwrap();
        write_fake_tsc(
            root,
            "echo \"src/a.ts(1,7): error TS2322: Type 'string' is not assignable to type 'number'.\"",
        );
        let (server, receiver) =
            server_with_options(root, json!({ "trusted": true, "typeAware": true }));
        let (_path, uri) = open_typescript(&server, root, "const value: number = 'x';\n");
        let first = wait_for_tsc_diagnostics(&receiver, &uri);
        assert_eq!(first.diagnostics.len(), 1);
        assert_eq!(
            first.diagnostics[0].code,
            Some(NumberOrString::String("TS2322".into()))
        );

        std::fs::write(
            root.join("tsconfig.json"),
            "{\"compilerOptions\":{\"strict\":true}}\n",
        )
        .unwrap();
        server.notify(Notification::new(
            DidChangeWatchedFiles::METHOD.into(),
            json!({
                "changes": [{
                    "uri": uri::from_path(&root.join("tsconfig.json")),
                    "type": 2
                }]
            }),
        ));
        wait_for_tsc_diagnostics(&receiver, &uri);
        let runs = std::fs::read_to_string(root.join("marker.log")).unwrap();
        assert_eq!(runs.lines().count(), 2, "{runs}");
    }

    #[cfg(unix)]
    #[test]
    fn did_change_clears_tsc_diagnostics_for_the_uri() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::write(root.join("tsconfig.json"), "{}\n").unwrap();
        std::fs::write(root.join("dotfmt.dotfile"), "typescript {}\n").unwrap();
        write_fake_tsc(
            root,
            "echo \"src/a.ts(1,7): error TS2322: Type 'string' is not assignable to type 'number'.\"",
        );
        let (server, receiver) = server_with_options(root, json!({ "trusted": true }));
        let (_path, uri) = open_typescript(&server, root, "const value: number = 'x';\n");
        wait_for_tsc_diagnostics(&receiver, &uri);

        server.notify(Notification::new(
            DidChangeTextDocument::METHOD.into(),
            json!({
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [{ "text": "const value: number = 1;\n" }]
            }),
        ));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .expect("timed out waiting for the cleared publish");
            let message = receiver.recv_timeout(remaining).expect("server message");
            let Message::Notification(notification) = message else {
                continue;
            };
            if notification.method != "textDocument/publishDiagnostics" {
                continue;
            }
            let params: lsp_types::PublishDiagnosticsParams =
                serde_json::from_value(notification.params).unwrap();
            if params.uri == uri && params.version == Some(2) {
                assert!(params.diagnostics.is_empty(), "{:?}", params.diagnostics);
                break;
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn doctor_reports_compiler_settings_and_roots() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::write(root.join("tsconfig.json"), "{}\n").unwrap();
        write_fake_tsc(root, "");
        let (server, _receiver) = server_with_options(root, json!({ "trusted": true }));
        let value = server
            .handle(DOCTOR_REQUEST, &Value::Null, &RequestState::default())
            .unwrap();
        assert_eq!(value["server"]["name"], json!("dotfmt-lsp"));
        assert_eq!(value["server"]["version"], json!(env!("CARGO_PKG_VERSION")));
        assert_eq!(value["trusted"], json!(true));
        assert_eq!(value["typeAware"], json!(true));
        assert_eq!(value["syntax"], json!(true));
        assert_eq!(value["tscScope"], json!("open"));
        let roots = value["roots"].as_array().unwrap();
        assert_eq!(roots.len(), 1);
        assert_eq!(
            std::fs::canonicalize(roots[0].as_str().unwrap()).unwrap(),
            std::fs::canonicalize(root).unwrap()
        );
        assert_eq!(value["default"]["typeAware"], json!(true));
        assert_eq!(value["default"]["syntax"], json!(true));
        let folders = value["folders"].as_array().unwrap();
        assert_eq!(folders.len(), 1);
        assert_eq!(
            std::fs::canonicalize(folders[0]["root"].as_str().unwrap()).unwrap(),
            std::fs::canonicalize(root).unwrap()
        );
        assert_eq!(folders[0]["typeAware"], json!(true));
        assert_eq!(folders[0]["fixesOnSave"], json!("off"));
        let projects = value["projects"].as_array().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0]["state"], json!("ok"));
        assert_eq!(projects[0]["version"], json!("9.9.9"));
        assert!(
            projects[0]["tsconfig"]
                .as_str()
                .unwrap()
                .ends_with("tsconfig.json")
        );
        assert!(
            projects[0]["compiler"]
                .as_str()
                .unwrap()
                .ends_with("node_modules/.bin/tsc")
        );
    }

    #[cfg(unix)]
    #[test]
    fn doctor_lists_referenced_project_configs() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::write(
            root.join("tsconfig.json"),
            r#"{"files":[],"references":[{"path":"./app"}]}"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join("app/src")).unwrap();
        std::fs::write(root.join("app/tsconfig.json"), "{}\n").unwrap();
        std::fs::write(root.join("app/src/a.ts"), "export const a = 1;\n").unwrap();
        write_fake_tsc(root, "");
        let (server, _receiver) = server_with_options(root, json!({ "trusted": true }));
        let value = server
            .handle(DOCTOR_REQUEST, &Value::Null, &RequestState::default())
            .unwrap();
        let projects = value["projects"].as_array().unwrap();
        assert_eq!(projects.len(), 2, "{projects:?}");
        assert!(
            projects[0]["tsconfig"]
                .as_str()
                .unwrap()
                .ends_with("tsconfig.json")
        );
        assert!(
            projects[1]["tsconfig"]
                .as_str()
                .unwrap()
                .ends_with("app/tsconfig.json")
        );
        assert_eq!(projects[1]["state"], json!("ok"));
        assert_eq!(projects[1]["version"], json!("9.9.9"));
        assert_eq!(
            std::fs::canonicalize(projects[0]["project"].as_str().unwrap()).unwrap(),
            std::fs::canonicalize(root).unwrap()
        );
        assert_eq!(
            std::fs::canonicalize(projects[1]["project"].as_str().unwrap()).unwrap(),
            std::fs::canonicalize(root.join("app")).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn configured_fixes_are_limited_to_trusted_full_document_formatting() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::write(
            root.join("dotfmt.dotfile"),
            "typescript { fixes = safe\n type_aware = false }\n",
        )
        .unwrap();
        let text = "import { unused } from \"./side\";\nconst value=1;\n";
        let path = root.join("src/a.ts");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        let uri = uri::from_path(&path);
        let document = Document::new(text.into(), "typescript".into(), 1);

        let untrusted = test_server(root);
        untrusted
            .documents
            .lock()
            .unwrap()
            .open(uri.clone(), document.clone());
        let edits = untrusted
            .format_document(&uri, &document, &[], true, None)
            .unwrap();
        assert!(
            apply_edits(text, &edits).contains("unused"),
            "untrusted formatting applied configured fixes"
        );

        let (trusted, _receiver) = server_with_options(root, json!({ "trusted": true }));
        trusted
            .documents
            .lock()
            .unwrap()
            .open(uri.clone(), document.clone());
        let edits = trusted
            .format_document(&uri, &document, &[], true, None)
            .unwrap();
        assert!(
            !apply_edits(text, &edits).contains("unused"),
            "trusted full-document formatting did not apply configured fixes"
        );

        let range = document.lsp_range(&document.line_range(1, 1));
        let edits = trusted
            .format_document(&uri, &document, &[range], false, None)
            .unwrap();
        assert!(
            apply_edits(text, &edits).contains("unused"),
            "range formatting applied configured fixes"
        );
    }

    #[cfg(unix)]
    #[test]
    fn will_save_applies_configured_fixes_only_when_trusted() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::write(
            root.join("dotfmt.dotfile"),
            "typescript { fixes = safe\n type_aware = false }\n",
        )
        .unwrap();
        let text = "import { unused } from \"./side\";\nconst value=1;\n";
        let path = root.join("src/a.ts");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        let uri = uri::from_path(&path);
        let document = Document::new(text.into(), "typescript".into(), 1);

        let untrusted = test_server(root);
        untrusted
            .documents
            .lock()
            .unwrap()
            .open(uri.clone(), document.clone());
        assert!(
            untrusted.fix_on_save(&uri, &document).unwrap().is_empty(),
            "untrusted will-save applied configured fixes"
        );

        let (trusted, _receiver) = server_with_options(root, json!({ "trusted": true }));
        trusted
            .documents
            .lock()
            .unwrap()
            .open(uri.clone(), document.clone());
        let edits = trusted.fix_on_save(&uri, &document).unwrap();
        assert!(
            !apply_edits(text, &edits).contains("unused"),
            "trusted will-save did not apply configured fixes"
        );
    }

    #[test]
    fn syntax_disabled_logs_formatting_failures_without_publishing() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::write(root.join("dotfmt.dotfile"), "typescript {}\n").unwrap();
        let text = "const = ;\n";
        let path = root.join("src/a.ts");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        let uri = uri::from_path(&path);
        let document = Document::new(text.into(), "typescript".into(), 1);
        let (server, receiver) =
            server_with_options(root, json!({ "diagnostics": { "syntax": false } }));
        server
            .documents
            .lock()
            .unwrap()
            .open(uri.clone(), document.clone());
        assert!(
            server
                .format_document(&uri, &document, &[], false, None)
                .unwrap()
                .is_empty()
        );
        let mut logged = false;
        let deadline = Instant::now() + Duration::from_secs(2);
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            let Ok(message) = receiver.recv_timeout(remaining) else {
                break;
            };
            if let Message::Notification(notification) = message {
                if notification.method == "window/logMessage" {
                    logged = true;
                }
                if notification.method == "textDocument/publishDiagnostics" {
                    let params: lsp_types::PublishDiagnosticsParams =
                        serde_json::from_value(notification.params).unwrap();
                    if params.uri == uri {
                        assert!(
                            params.diagnostics.is_empty(),
                            "syntax diagnostics were published while disabled"
                        );
                    }
                }
            }
        }
        assert!(logged, "formatting failure was not logged");
    }

    #[test]
    fn code_actions_keep_the_existing_action_shape() {
        let temporary = tempfile::tempdir().unwrap();
        std::fs::write(temporary.path().join("dotfmt.dotfile"), "typescript {}\n").unwrap();
        let path = temporary.path().join("a.ts");
        let text = "const value=1;\n";
        std::fs::write(&path, text).unwrap();
        let server = test_server(temporary.path());
        let uri = uri::from_path(&path);
        server.documents.lock().unwrap().open(
            uri.clone(),
            Document::new(text.into(), "typescript".into(), 1),
        );
        let result = server
            .handle(
                CodeActionRequest::METHOD,
                &json!({
                    "textDocument": { "uri": uri },
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end": { "line": 0, "character": 0 }
                    },
                    "context": { "diagnostics": [], "only": null }
                }),
                &RequestState::default(),
            )
            .unwrap();
        let actions: Vec<CodeActionOrCommand> = serde_json::from_value(result).unwrap();
        assert!(!actions.is_empty());
        let kinds: Vec<&str> = actions
            .iter()
            .filter_map(|action| match action {
                CodeActionOrCommand::CodeAction(action) => {
                    action.kind.as_ref().map(CodeActionKind::as_str)
                }
                CodeActionOrCommand::Command(_) => None,
            })
            .collect();
        assert!(kinds.contains(&actions::FORMAT), "{kinds:?}");
        let mut seen = std::collections::HashSet::new();
        for action in &actions {
            let CodeActionOrCommand::CodeAction(action) = action else {
                panic!("unexpected command action");
            };
            assert!(seen.insert(action.title.clone()), "duplicate action title");
            assert!(
                action.edit.is_some(),
                "action without edit: {}",
                action.title
            );
        }
    }

    #[test]
    fn diagnostic_provider_is_advertised_only_for_pull_clients() {
        let temporary = tempfile::tempdir().unwrap();
        let (server, client) = Connection::memory();
        let serving = std::thread::spawn(move || run(&server));
        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(1),
                "initialize".into(),
                json!({
                    "processId": null,
                    "rootUri": uri::from_path(temporary.path()),
                    "capabilities": {},
                }),
            )))
            .unwrap();
        let initialize = receive_response(&client, &RequestId::from(1)).unwrap();
        assert!(
            initialize["capabilities"]
                .get("diagnosticProvider")
                .is_none()
        );
        client
            .sender
            .send(Message::Notification(Notification::new(
                "initialized".into(),
                json!({}),
            )))
            .unwrap();
        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(2),
                "shutdown".into(),
                json!(null),
            )))
            .unwrap();
        let _ = receive_response(&client, &RequestId::from(2));
        client
            .sender
            .send(Message::Notification(Notification::new(
                "exit".into(),
                json!(null),
            )))
            .unwrap();
        serving.join().unwrap().unwrap();

        let temporary = tempfile::tempdir().unwrap();
        let (server, client) = Connection::memory();
        let serving = std::thread::spawn(move || run(&server));
        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(1),
                "initialize".into(),
                json!({
                    "processId": null,
                    "rootUri": uri::from_path(temporary.path()),
                    "capabilities": { "textDocument": { "diagnostic": {} } },
                }),
            )))
            .unwrap();
        let initialize = receive_response(&client, &RequestId::from(1)).unwrap();
        assert_eq!(
            initialize["capabilities"]["diagnosticProvider"]["identifier"],
            json!("dotfmt")
        );
        assert_eq!(
            initialize["capabilities"]["diagnosticProvider"]["interFileDependencies"],
            json!(true)
        );
        assert_eq!(
            initialize["capabilities"]["diagnosticProvider"]["workspaceDiagnostics"],
            json!(false)
        );
        client
            .sender
            .send(Message::Notification(Notification::new(
                "initialized".into(),
                json!({}),
            )))
            .unwrap();
        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(2),
                "shutdown".into(),
                json!(null),
            )))
            .unwrap();
        let _ = receive_response(&client, &RequestId::from(2));
        client
            .sender
            .send(Message::Notification(Notification::new(
                "exit".into(),
                json!(null),
            )))
            .unwrap();
        serving.join().unwrap().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn pull_diagnostics_return_the_merged_set_and_refresh_after_tsc() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::write(root.join("tsconfig.json"), "{}\n").unwrap();
        std::fs::write(root.join("dotfmt.dotfile"), "typescript {}\n").unwrap();
        write_fake_tsc(
            root,
            "echo \"src/a.ts(1,1): error TS2322: Type 'string' is not assignable to type 'number'.\"",
        );
        let text = "const = ;\n";
        let path = root.join("src/a.ts");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        let uri = uri::from_path(&path);
        let (server, client) = Connection::memory();
        let serving = std::thread::spawn(move || run(&server));
        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(1),
                "initialize".into(),
                json!({
                    "processId": null,
                    "rootUri": uri::from_path(root),
                    "capabilities": { "textDocument": { "diagnostic": {} } },
                    "initializationOptions": { "trusted": true },
                }),
            )))
            .unwrap();
        let initialize = receive_response(&client, &RequestId::from(1)).unwrap();
        assert_eq!(
            initialize["capabilities"]["diagnosticProvider"]["identifier"],
            json!("dotfmt")
        );
        client
            .sender
            .send(Message::Notification(Notification::new(
                "initialized".into(),
                json!({}),
            )))
            .unwrap();
        client
            .sender
            .send(Message::Notification(Notification::new(
                DidOpenTextDocument::METHOD.into(),
                json!({
                    "textDocument": {
                        "uri": uri,
                        "languageId": "typescript",
                        "version": 1,
                        "text": text,
                    }
                }),
            )))
            .unwrap();

        let mut saw_refresh = false;
        let mut saw_push = false;
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut id = 100;
        let (result_id, items) = loop {
            assert!(
                Instant::now() < deadline,
                "pull never saw both dotfmt and tsc diagnostics"
            );
            client
                .sender
                .send(Message::Request(Request::new(
                    RequestId::from(id),
                    DocumentDiagnosticRequest::METHOD.into(),
                    json!({ "textDocument": { "uri": uri } }),
                )))
                .unwrap();
            let response = loop {
                let message = client
                    .receiver
                    .recv_timeout(Duration::from_secs(5))
                    .expect("pull response");
                match message {
                    Message::Response(response) if response.id == RequestId::from(id) => {
                        break response.response_result.expect("pull result");
                    }
                    Message::Request(request)
                        if request.method == "workspace/diagnostic/refresh" =>
                    {
                        saw_refresh = true;
                    }
                    Message::Notification(notification)
                        if notification.method == "textDocument/publishDiagnostics" =>
                    {
                        saw_push = true;
                    }
                    _ => {}
                }
            };
            if response["kind"] == json!("full") {
                let items = response["items"].as_array().cloned().unwrap_or_default();
                let dotfmt = items.iter().any(|item| item["source"] == json!("dotfmt"));
                let tsc = items
                    .iter()
                    .any(|item| item["source"] == json!("dotfmt(tsc)"));
                if dotfmt && tsc {
                    break (response["resultId"].as_str().unwrap().to_owned(), items);
                }
            }
            id += 1;
            std::thread::sleep(Duration::from_millis(50));
        };
        assert!(saw_refresh, "tsc completion did not ask for a refresh");
        assert!(!saw_push, "the pull client received push diagnostics");
        assert!(
            items
                .iter()
                .all(|item| item["severity"].as_u64() == Some(1)),
            "non-error diagnostics were reported: {items:?}"
        );

        id += 1;
        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(id),
                DocumentDiagnosticRequest::METHOD.into(),
                json!({
                    "textDocument": { "uri": uri },
                    "previousResultId": result_id,
                }),
            )))
            .unwrap();
        let unchanged = receive_response(&client, &RequestId::from(id)).unwrap();
        assert_eq!(unchanged["kind"], json!("unchanged"));
        assert_eq!(unchanged["resultId"], json!(result_id));

        client
            .sender
            .send(Message::Notification(Notification::new(
                DidCloseTextDocument::METHOD.into(),
                json!({ "textDocument": { "uri": uri } }),
            )))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            assert!(
                Instant::now() < deadline,
                "closed document kept its diagnostics"
            );
            id += 1;
            client
                .sender
                .send(Message::Request(Request::new(
                    RequestId::from(id),
                    DocumentDiagnosticRequest::METHOD.into(),
                    json!({ "textDocument": { "uri": uri } }),
                )))
                .unwrap();
            let response = receive_response(&client, &RequestId::from(id)).unwrap();
            if response["kind"] == json!("full")
                && response["items"].as_array().is_some_and(Vec::is_empty)
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(2),
                "shutdown".into(),
                json!(null),
            )))
            .unwrap();
        let _ = receive_response(&client, &RequestId::from(2));
        client
            .sender
            .send(Message::Notification(Notification::new(
                "exit".into(),
                json!(null),
            )))
            .unwrap();
        serving.join().unwrap().unwrap();
    }

    #[test]
    fn pull_diagnostics_report_unsupported_documents_as_empty() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("notes.txt");
        let uri = uri::from_path(&path);
        let (server, client) = Connection::memory();
        let serving = std::thread::spawn(move || run(&server));
        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(1),
                "initialize".into(),
                json!({
                    "processId": null,
                    "rootUri": uri::from_path(temporary.path()),
                    "capabilities": { "textDocument": { "diagnostic": {} } },
                }),
            )))
            .unwrap();
        let _ = receive_response(&client, &RequestId::from(1)).unwrap();
        client
            .sender
            .send(Message::Notification(Notification::new(
                "initialized".into(),
                json!({}),
            )))
            .unwrap();
        client
            .sender
            .send(Message::Notification(Notification::new(
                DidOpenTextDocument::METHOD.into(),
                json!({
                    "textDocument": {
                        "uri": uri,
                        "languageId": "plaintext",
                        "version": 1,
                        "text": "hello\n",
                    }
                }),
            )))
            .unwrap();
        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(2),
                DocumentDiagnosticRequest::METHOD.into(),
                json!({ "textDocument": { "uri": uri } }),
            )))
            .unwrap();
        let report = receive_response(&client, &RequestId::from(2)).unwrap();
        assert_eq!(report["kind"], json!("full"));
        assert_eq!(report["items"], json!([]));
        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(3),
                "shutdown".into(),
                json!(null),
            )))
            .unwrap();
        let _ = receive_response(&client, &RequestId::from(3));
        client
            .sender
            .send(Message::Notification(Notification::new(
                "exit".into(),
                json!(null),
            )))
            .unwrap();
        serving.join().unwrap().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn push_mode_still_publishes_diagnostics_without_pull() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::write(root.join("tsconfig.json"), "{}\n").unwrap();
        std::fs::write(root.join("dotfmt.dotfile"), "typescript {}\n").unwrap();
        write_fake_tsc(
            root,
            "echo \"src/a.ts(1,7): error TS2322: Type 'string' is not assignable to type 'number'.\"",
        );
        let (server, receiver) = server_with_options(root, json!({ "trusted": true }));
        let (_path, uri) = open_typescript(&server, root, "const value: number = 'x';\n");
        let params = wait_for_tsc_diagnostics(&receiver, &uri);
        assert_eq!(
            params.diagnostics[0].code,
            Some(NumberOrString::String("TS2322".into()))
        );
    }

    #[cfg(unix)]
    #[test]
    fn per_folder_type_aware_controls_tsc_for_each_root() {
        let temporary = tempfile::tempdir().unwrap();
        let root_a = temporary.path().join("a");
        let root_b = temporary.path().join("b");
        for root in [&root_a, &root_b] {
            std::fs::create_dir_all(root).unwrap();
            std::fs::write(root.join("tsconfig.json"), "{}\n").unwrap();
            std::fs::write(root.join("dotfmt.dotfile"), "typescript {}\n").unwrap();
            write_fake_tsc(
                root,
                "echo \"src/a.ts(1,1): error TS2322: Type 'string' is not assignable.\"",
            );
        }
        let (server, receiver) =
            server_with_roots(&[&root_a, &root_b], json!({ "trusted": true }), json!({}));
        server
            .settings
            .configure_folder(&root_a, &json!({ "dotfmt.typeAware": false }));
        let (_path_a, _uri_a) = open_typescript(&server, &root_a, "const value: number = 'x';\n");
        let (_path_b, uri_b) = open_typescript(&server, &root_b, "const value: number = 'x';\n");
        wait_for_tsc_diagnostics(&receiver, &uri_b);
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            !root_a.join("marker.log").exists(),
            "a folder with typeAware=false ran tsc"
        );
        assert!(
            root_b.join("marker.log").exists(),
            "the typeAware folder never ran tsc"
        );
    }

    #[cfg(unix)]
    #[test]
    fn changing_one_folder_does_not_clear_another_folders_diagnostics() {
        let temporary = tempfile::tempdir().unwrap();
        let root_a = temporary.path().join("a");
        let root_b = temporary.path().join("b");
        for root in [&root_a, &root_b] {
            std::fs::create_dir_all(root).unwrap();
            std::fs::write(root.join("tsconfig.json"), "{}\n").unwrap();
            std::fs::write(root.join("dotfmt.dotfile"), "typescript {}\n").unwrap();
            write_fake_tsc(
                root,
                "echo \"src/a.ts(1,1): error TS2322: Type 'string' is not assignable.\"",
            );
        }
        let (server, receiver) =
            server_with_roots(&[&root_a, &root_b], json!({ "trusted": true }), json!({}));
        let (_path_a, uri_a) = open_typescript(&server, &root_a, "const value: number = 'x';\n");
        let (_path_b, uri_b) = open_typescript(&server, &root_b, "const value: number = 'x';\n");
        wait_for_tsc_diagnostics(&receiver, &uri_a);
        wait_for_tsc_diagnostics(&receiver, &uri_b);
        assert!(
            !server
                .diagnostics
                .merged(&uri_b, None)
                .diagnostics
                .is_empty()
        );

        assert!(
            server
                .settings
                .configure_folder(&root_a, &json!({ "dotfmt.diagnostics.debounceMs": 1000 }),)
        );
        server.reload_folder(&root_a);
        let deadline = Instant::now() + Duration::from_millis(500);
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            if let Ok(Message::Notification(notification)) = receiver.recv_timeout(remaining)
                && notification.method == "textDocument/publishDiagnostics"
            {
                let params: lsp_types::PublishDiagnosticsParams =
                    serde_json::from_value(notification.params).unwrap();
                assert_ne!(
                    params.uri, uri_b,
                    "reloading one folder republished another folder's URI"
                );
            }
        }
        assert!(
            !server
                .diagnostics
                .merged(&uri_b, None)
                .diagnostics
                .is_empty(),
            "reloading one folder cleared another folder's tsc entries"
        );
    }

    #[test]
    fn configuration_pulls_are_per_folder_and_ignore_did_change_payloads() {
        let temporary = tempfile::tempdir().unwrap();
        let root_a = temporary.path().join("a");
        let root_b = temporary.path().join("b");
        std::fs::create_dir_all(&root_a).unwrap();
        std::fs::create_dir_all(&root_b).unwrap();
        let folders: Vec<Value> = [&root_a, &root_b]
            .into_iter()
            .enumerate()
            .map(|(index, root)| {
                json!({ "uri": uri::from_path(root), "name": format!("root-{index}") })
            })
            .collect();
        let (server, client) = Connection::memory();
        let serving = std::thread::spawn(move || run(&server));
        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(1),
                "initialize".into(),
                json!({
                    "processId": null,
                    "capabilities": { "workspace": { "configuration": true } },
                    "workspaceFolders": folders,
                }),
            )))
            .unwrap();
        let _ = receive_response(&client, &RequestId::from(1)).unwrap();
        client
            .sender
            .send(Message::Notification(Notification::new(
                "initialized".into(),
                json!({}),
            )))
            .unwrap();

        let request = receive_request(&client, "workspace/configuration").expect("pull");
        let items = request.params["items"].as_array().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0]["section"], json!("dotfmt"));
        assert!(items[0].get("scopeUri").is_none());
        assert_eq!(items[1]["scopeUri"], json!(uri::from_path(&root_a)));
        assert_eq!(items[2]["scopeUri"], json!(uri::from_path(&root_b)));
        client
            .sender
            .send(Message::Response(Response::new_ok(
                request.id,
                json!([
                    {},
                    { "dotfmt.typeAware": false },
                    {}
                ]),
            )))
            .unwrap();

        let folders = |client: &Connection| {
            client
                .sender
                .send(Message::Request(Request::new(
                    RequestId::from(50),
                    "dotfmt/doctor".into(),
                    json!(null),
                )))
                .unwrap();
            let doctor = receive_response(client, &RequestId::from(50)).unwrap();
            doctor["folders"].as_array().cloned().unwrap_or_default()
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            assert!(
                Instant::now() < deadline,
                "folder settings were never pulled"
            );
            let folders = folders(&client);
            if folders[0]["typeAware"] == json!(false) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        client
            .sender
            .send(Message::Notification(Notification::new(
                DidChangeConfiguration::METHOD.into(),
                json!({ "settings": { "dotfmt.typeAware": false } }),
            )))
            .unwrap();
        let request = receive_request(&client, "workspace/configuration").expect("repull");
        assert_eq!(request.params["items"].as_array().unwrap().len(), 3);
        client
            .sender
            .send(Message::Response(Response::new_ok(
                request.id,
                json!([
                    {},
                    { "dotfmt.typeAware": false },
                    { "dotfmt.diagnostics.syntax": false }
                ]),
            )))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            assert!(
                Instant::now() < deadline,
                "re-pulled settings were never applied"
            );
            let folders = folders(&client);
            if folders[1]["syntax"] == json!(false) {
                assert_eq!(folders[0]["typeAware"], json!(false));
                assert_eq!(folders[1]["typeAware"], json!(true));
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        client
            .sender
            .send(Message::Request(Request::new(
                RequestId::from(2),
                "shutdown".into(),
                json!(null),
            )))
            .unwrap();
        let _ = receive_response(&client, &RequestId::from(2));
        client
            .sender
            .send(Message::Notification(Notification::new(
                "exit".into(),
                json!(null),
            )))
            .unwrap();
        serving.join().unwrap().unwrap();
    }

    #[test]
    fn did_open_requests_a_missing_folder_configuration() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::write(root.join("dotfmt.dotfile"), "typescript {}\n").unwrap();
        let text = "const value=1;\n";
        let path = root.join("src/a.ts");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        let uri = uri::from_path(&path);
        let (sender, receiver) = crossbeam_channel::unbounded();
        let params: InitializeParams = serde_json::from_value(json!({
            "processId": null,
            "rootUri": uri::from_path(root),
            "capabilities": { "workspace": { "configuration": true } },
        }))
        .unwrap();
        let server = Arc::new(Server::new(sender, &params));
        let responder = {
            let client = Arc::clone(&server.client);
            let receiver = receiver.clone();
            let expected = uri::from_path(root);
            std::thread::spawn(move || {
                let request = loop {
                    match receiver
                        .recv_timeout(Duration::from_secs(5))
                        .expect("configuration request")
                    {
                        Message::Request(request)
                            if request.method == "workspace/configuration" =>
                        {
                            break request;
                        }
                        _ => continue,
                    }
                };
                assert_eq!(
                    request.params["items"][0]["scopeUri"],
                    json!(expected),
                    "the folder request did not carry its scopeUri"
                );
                client.response(Response::new_ok(
                    request.id,
                    json!([{ "dotfmt.diagnostics.syntax": false }]),
                ));
            })
        };
        server.notify(Notification::new(
            DidOpenTextDocument::METHOD.into(),
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "typescript",
                    "version": 1,
                    "text": text,
                }
            }),
        ));
        responder.join().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline && server.settings_for_root(root).syntax_diagnostics {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!server.settings_for_root(root).syntax_diagnostics);
    }
}
