//! The dotfmt language server.

use std::collections::HashSet;
use std::error::Error;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;

use dotfmt::app::{FixMode, Selection};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeConfiguration, DidChangeTextDocument, DidChangeWatchedFiles,
    DidChangeWorkspaceFolders, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument,
    Notification as _,
};
use lsp_types::request::{
    CodeActionRequest, ExecuteCommand, Formatting, OnTypeFormatting, RangeFormatting, Request as _,
    WillSaveWaitUntil,
};
use lsp_types::{
    CodeActionKind, CodeActionOptions, CodeActionOrCommand, CodeActionParams,
    CodeActionProviderCapability, DidChangeConfigurationParams, DidChangeTextDocumentParams,
    DidChangeWatchedFilesParams, DidChangeWorkspaceFoldersParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, DocumentFormattingParams,
    DocumentOnTypeFormattingOptions, DocumentOnTypeFormattingParams, DocumentRangeFormattingParams,
    ExecuteCommandOptions, ExecuteCommandParams, InitializeParams, NumberOrString, OneOf,
    PositionEncodingKind, Range, SaveOptions, ServerCapabilities, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextDocumentSyncOptions, TextDocumentSyncSaveOptions, TextEdit, Uri,
    WillSaveTextDocumentParams, WorkspaceFoldersServerCapabilities, WorkspaceServerCapabilities,
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
use crate::settings::Settings;
use crate::tsc::Tsc;
use crate::uri;
use crate::workspace::Workspace;

pub const FIX_ALL: &str = "dotfmt.fixAll";
pub const ORGANIZE_IMPORTS: &str = "dotfmt.organizeImports";
pub const SORT_TAILWIND: &str = "dotfmt.sortTailwind";
pub const EXPLAIN: &str = "dotfmt.explain";
pub const RELOAD_CONFIG: &str = "dotfmt/reloadConfig";

const EXPLAIN_REQUEST: &str = "dotfmt/explain";
const FORMAT_TEXT_REQUEST: &str = "dotfmt/formatText";

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
    let supports_configuration = params
        .capabilities
        .workspace
        .as_ref()
        .and_then(|workspace| workspace.configuration)
        .unwrap_or(false);
    let server = Arc::new(Server::new(connection.sender.clone(), &params));
    connection.initialize_finish(id, server.initialization_result())?;

    if supports_configuration {
        let client = Arc::clone(&server.client);
        let configured = Arc::clone(&server);
        thread::spawn(move || {
            if let Ok(value) = client.request(
                "workspace/configuration",
                json!({ "items": [{ "section": "dotfmt" }] }),
            ) && let Some(settings) = value.get(0)
            {
                configured.configure(settings);
            }
        });
    }

    for message in &connection.receiver {
        match message {
            Message::Request(request) => {
                if connection.handle_shutdown(&request)? {
                    break;
                }
                server.dispatch(request);
            }
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
    settings: RwLock<Settings>,
    client: Arc<Client>,
    diagnostics: Arc<Diagnostics>,
    tsc: Arc<Tsc>,
    capabilities: EditCapabilities,
    cancelled: Mutex<HashSet<RequestId>>,
}

impl Server {
    fn new(sender: crossbeam_channel::Sender<Message>, params: &InitializeParams) -> Self {
        let roots = workspace_roots(params);
        let documents = Arc::new(Mutex::new(Documents::default()));
        let workspace = Arc::new(Workspace::new(roots));
        let diagnostics = Arc::new(Diagnostics::new(
            Arc::clone(&workspace),
            Arc::clone(&documents),
            sender.clone(),
        ));
        let tsc = Arc::new(Tsc::new(Arc::clone(&diagnostics)));
        let mut settings = Settings::default();
        if let Some(options) = &params.initialization_options {
            settings.apply(options);
        }
        dotfmt_tailwind::bridge::set_trusted(settings.trusted);
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
        Self {
            documents,
            workspace,
            settings: RwLock::new(settings),
            client: Arc::new(Client::new(sender)),
            diagnostics,
            tsc,
            capabilities,
            cancelled: Mutex::new(HashSet::new()),
        }
    }

    fn initialization_result(&self) -> Value {
        let mut capabilities =
            serde_json::to_value(capabilities()).expect("server capabilities serialize");
        capabilities["documentRangeFormattingProvider"] = json!({ "rangesSupport": true });
        json!({
            "capabilities": capabilities,
            "serverInfo": { "name": "dotfmt-lsp", "version": env!("CARGO_PKG_VERSION") },
        })
    }

    fn settings(&self) -> Settings {
        self.settings
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    fn log(&self, message: impl Into<String>) {
        self.diagnostics.log(message);
    }

    fn configure(&self, value: &Value) {
        self.settings
            .write()
            .unwrap_or_else(|error| error.into_inner())
            .apply(value);
        dotfmt_tailwind::bridge::set_trusted(self.settings().trusted);
        self.reload();
    }

    fn reload(&self) {
        self.workspace.invalidate();
        self.diagnostics.clear_all();
        self.tsc.invalidate();
        let documents = self
            .documents
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.diagnostics.schedule_all(&documents);
        for (uri, _) in documents.iter() {
            self.schedule_tsc(uri.clone());
        }
    }

    fn document(&self, uri: &Uri) -> std::result::Result<Document, RpcError> {
        self.documents
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(uri)
            .cloned()
            .ok_or_else(|| RpcError::invalid_params(format!("document not open: {}", uri.as_str())))
    }

    fn current_version(&self, uri: &Uri) -> Option<i32> {
        self.documents
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(uri)
            .map(|document| document.version)
    }

    fn ensure_current(&self, uri: &Uri, version: i32) -> std::result::Result<(), RpcError> {
        if self.current_version(uri) == Some(version) {
            Ok(())
        } else {
            Err(RpcError {
                code: ErrorCode::ContentModified as i32,
                message: "document changed while formatting".into(),
            })
        }
    }

    fn path(&self, uri: &Uri, document: &Document) -> std::result::Result<PathBuf, RpcError> {
        analysis::buffer_path(uri, document.language())
            .ok_or_else(|| RpcError::invalid_params("buffer has no dotfmt language"))
    }

    fn dispatch(self: &Arc<Self>, request: Request) {
        let id = request.id.clone();
        let server = Arc::clone(self);
        let method = request.method.clone();
        let params = request.params.clone();
        let sender = self.client_sender();
        let id_for_thread = id.clone();
        let spawned = thread::Builder::new()
            .name("dotfmt-lsp-request".into())
            .spawn(move || {
                let result = catch_unwind(AssertUnwindSafe(|| {
                    server.handle(&method, &params, &id_for_thread)
                }));
                let result = match result {
                    Ok(Ok(_)) if server.cancelled(&id_for_thread) => Ok(Err(RpcError {
                        code: ErrorCode::RequestCanceled as i32,
                        message: "request cancelled".into(),
                    })),
                    result => result,
                };
                let response = match result {
                    Ok(Ok(value)) => Response::new_ok(id_for_thread.clone(), value),
                    Ok(Err(error)) => {
                        Response::new_err(id_for_thread.clone(), error.code, error.message)
                    }
                    Err(_) => {
                        server.workspace.invalidate();
                        server.log(format!("request {method} panicked"));
                        Response::new_err(
                            id_for_thread.clone(),
                            ErrorCode::InternalError as i32,
                            "dotfmt internal error".into(),
                        )
                    }
                };
                server.finish(&id_for_thread);
                let _ = sender.send(Message::Response(response));
            });
        if spawned.is_err() {
            let _ = self
                .client_sender()
                .send(Message::Response(Response::new_err(
                    id,
                    ErrorCode::InternalError as i32,
                    "could not start request worker".into(),
                )));
        }
    }

    fn client_sender(&self) -> crossbeam_channel::Sender<Message> {
        self.client.sender()
    }

    fn cancelled(&self, id: &RequestId) -> bool {
        self.cancelled
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains(id)
    }

    fn finish(&self, id: &RequestId) {
        self.cancelled
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(id);
    }

    fn cancel(&self, params: &Value) {
        let Ok(params) = serde_json::from_value::<CancelParams>(params.clone()) else {
            return;
        };
        let id = match params.id {
            NumberOrString::Number(id) => RequestId::from(id),
            NumberOrString::String(id) => RequestId::from(id),
        };
        self.cancelled
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(id);
    }

    fn handle(
        &self,
        method: &str,
        params: &Value,
        id: &RequestId,
    ) -> std::result::Result<Value, RpcError> {
        if self.cancelled(id) {
            return Err(RpcError {
                code: ErrorCode::RequestCanceled as i32,
                message: "request cancelled".into(),
            });
        }
        match method {
            Formatting::METHOD => {
                let params: DocumentFormattingParams = parse(params)?;
                let document = self.document(&params.text_document.uri)?;
                let edits = self.format_document(&params.text_document.uri, &document, &[])?;
                Ok(json!(Some(edits)))
            }
            RangeFormatting::METHOD => {
                let params: DocumentRangeFormattingParams = parse(params)?;
                let document = self.document(&params.text_document.uri)?;
                let edits =
                    self.format_document(&params.text_document.uri, &document, &[params.range])?;
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
                let edits =
                    self.format_document(&params.text_document.uri, &document, &params.ranges)?;
                Ok(json!(Some(edits)))
            }
            OnTypeFormatting::METHOD => {
                let params: DocumentOnTypeFormattingParams = parse(params)?;
                let document = self.document(&params.text_document_position.text_document.uri)?;
                let range = block(&document, params.text_document_position.position);
                let edits = self.format_document(
                    &params.text_document_position.text_document.uri,
                    &document,
                    &[range],
                )?;
                Ok(json!(Some(edits)))
            }
            CodeActionRequest::METHOD => {
                let params: CodeActionParams = parse(params)?;
                Ok(json!(self.code_actions(params)?))
            }
            ExecuteCommand::METHOD => {
                let params: ExecuteCommandParams = parse(params)?;
                self.execute(params)
            }
            EXPLAIN_REQUEST => self.explain(params.clone()),
            FORMAT_TEXT_REQUEST => self.format_text(params.clone()),
            _ => Err(RpcError {
                code: ErrorCode::MethodNotFound as i32,
                message: format!("unhandled method: {method}"),
            }),
        }
    }

    fn format_document(
        &self,
        uri: &Uri,
        document: &Document,
        ranges: &[Range],
    ) -> std::result::Result<Vec<TextEdit>, RpcError> {
        let path = self.path(uri, document)?;
        let languages = analysis::forced(&path, uri, document.language());
        let session = self.workspace.session(&path);
        let mut session = session.lock().unwrap_or_else(|error| error.into_inner());
        let output = match analysis::format(&mut session, &path, document.text(), languages) {
            Ok(output) => output,
            Err(errors) => {
                self.diagnostics
                    .publish_fatal(uri.clone(), document.text(), &errors);
                for error in errors {
                    self.log(error.to_string());
                }
                return Ok(Vec::new());
            }
        };
        self.ensure_current(uri, document.version)?;
        let edits = edits::minimal(document.text(), &output);
        let edits = if ranges.is_empty() {
            edits
        } else {
            edits::filter(document, edits, ranges)
        };
        Ok(edits::text_edits(document, &edits))
    }

    fn fix_on_save(
        &self,
        uri: &Uri,
        document: &Document,
    ) -> std::result::Result<Vec<TextEdit>, RpcError> {
        let mode = self.settings().fixes_on_save;
        let fix = match mode {
            crate::settings::FixesOnSave::Off => return Ok(Vec::new()),
            crate::settings::FixesOnSave::Safe => FixMode::Safe,
            crate::settings::FixesOnSave::Unsafe if self.settings().trusted => FixMode::Unsafe,
            crate::settings::FixesOnSave::Unsafe => FixMode::Safe,
        };
        let path = self.path(uri, document)?;
        let languages = analysis::forced(&path, uri, document.language());
        let session = self.workspace.session(&path);
        let mut session = session.lock().unwrap_or_else(|error| error.into_inner());
        let inspection =
            match analysis::inspect(&mut session, &path, document.text(), languages, fix) {
                Ok(inspection) => inspection,
                Err(errors) => {
                    self.diagnostics
                        .publish_fatal(uri.clone(), document.text(), &errors);
                    for error in errors {
                        self.log(error.to_string());
                    }
                    return Ok(Vec::new());
                }
            };
        self.ensure_current(uri, document.version)?;
        Ok(edits::whole(document, &inspection.after))
    }

    fn code_actions(
        &self,
        params: CodeActionParams,
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
        let mut fixes = Vec::new();
        let mut format = None;
        let mut fix_all = None;
        let mut organize = None;
        let mut failure = None;
        if actions::wants(&only, CodeActionKind::QUICKFIX.as_str()) {
            match analysis::inspect(
                &mut session,
                &path,
                document.text(),
                languages.clone(),
                FixMode::Unsafe,
            ) {
                Ok(inspection) => fixes = inspection.fixes,
                Err(errors) => failure = Some(errors),
            }
        }
        if actions::wants(&only, actions::FORMAT) {
            match analysis::format(&mut session, &path, document.text(), languages.clone()) {
                Ok(text) => format = Some(text),
                Err(errors) => failure = Some(errors),
            }
        }
        if actions::wants(&only, actions::FIX_ALL) {
            match analysis::inspect(
                &mut session,
                &path,
                document.text(),
                languages.clone(),
                FixMode::Safe,
            ) {
                Ok(inspection) => fix_all = Some(inspection.after),
                Err(errors) => failure = Some(errors),
            }
        }
        if actions::wants(&only, actions::ORGANIZE_IMPORTS) {
            match analysis::organize(&mut session, &path, document.text(), languages) {
                Ok(text) => organize = Some(text),
                Err(errors) => failure = Some(errors),
            }
        }
        if let Some(errors) = failure {
            self.diagnostics
                .publish_fatal(uri.clone(), document.text(), &errors);
            for error in errors {
                self.log(error.to_string());
            }
        }
        Ok(actions::code_actions(&actions::Context {
            document: &document,
            uri: &uri,
            range: params.range,
            only,
            trusted: self.settings().trusted,
            fixes: &fixes,
            format: format.as_deref(),
            fix_all: fix_all.as_deref(),
            organize: organize.as_deref(),
            capabilities: self.capabilities,
        }))
    }

    fn execute(&self, params: ExecuteCommandParams) -> std::result::Result<Value, RpcError> {
        let command = params.command.as_str();
        if command == RELOAD_CONFIG {
            self.reload();
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
                let mode = if !self.settings().trusted || requested != "unsafe" {
                    FixMode::Safe
                } else {
                    FixMode::Unsafe
                };
                analysis::inspect(&mut session, &path, document.text(), languages, mode)
                    .map(|inspection| inspection.after)
            }
            ORGANIZE_IMPORTS => analysis::organize(&mut session, &path, document.text(), languages),
            SORT_TAILWIND => analysis::format(&mut session, &path, document.text(), languages),
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
        let text =
            analysis::format(&mut session, &path, &params.text, languages).map_err(|errors| {
                RpcError {
                    code: ErrorCode::RequestFailed as i32,
                    message: errors
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("\n"),
                }
            })?;
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

    fn notify(&self, notification: Notification) {
        match notification.method.as_str() {
            DidOpenTextDocument::METHOD => {
                if let Ok(params) =
                    serde_json::from_value::<DidOpenTextDocumentParams>(notification.params)
                {
                    let item = params.text_document;
                    let document = Document::new(item.text, item.language_id, item.version);
                    self.documents
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .open(item.uri.clone(), document.clone());
                    self.diagnostics.schedule(
                        &document,
                        self.document_path(&item.uri, &document),
                        item.uri.clone(),
                    );
                    self.schedule_tsc(item.uri);
                }
            }
            DidChangeTextDocument::METHOD => {
                if let Ok(params) =
                    serde_json::from_value::<DidChangeTextDocumentParams>(notification.params)
                {
                    let snapshot = {
                        let mut documents = self
                            .documents
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        let Some(document) = documents.get_mut(&params.text_document.uri) else {
                            return;
                        };
                        if params.text_document.version <= document.version {
                            return;
                        }
                        document.version = params.text_document.version;
                        for change in params.content_changes {
                            document.change(change);
                        }
                        document.clone()
                    };
                    if let Some(path) =
                        analysis::buffer_path(&params.text_document.uri, snapshot.language())
                    {
                        self.diagnostics.schedule(
                            &snapshot,
                            path,
                            params.text_document.uri.clone(),
                        );
                    }
                }
            }
            DidCloseTextDocument::METHOD => {
                if let Ok(params) =
                    serde_json::from_value::<DidCloseTextDocumentParams>(notification.params)
                {
                    self.documents
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .close(&params.text_document.uri);
                    self.diagnostics.clear(params.text_document.uri);
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
                        if let Some(text) = params.text
                            && text != document.text()
                        {
                            document.change(lsp_types::TextDocumentContentChangeEvent {
                                range: None,
                                range_length: None,
                                text,
                            });
                        }
                        document.clone()
                    };
                    if let Some(path) = analysis::buffer_path(&uri, snapshot.language()) {
                        self.diagnostics.schedule(&snapshot, path, uri.clone());
                    }
                    self.schedule_tsc(uri);
                }
            }
            DidChangeConfiguration::METHOD => {
                if let Ok(params) =
                    serde_json::from_value::<DidChangeConfigurationParams>(notification.params)
                {
                    self.configure(&params.settings);
                }
            }
            DidChangeWatchedFiles::METHOD => {
                if let Ok(params) =
                    serde_json::from_value::<DidChangeWatchedFilesParams>(notification.params)
                {
                    let config = params.changes.iter().any(|change| {
                        uri::path(&change.uri)
                            .is_some_and(|path| path.file_name() == Some("dotfmt.dotfile".as_ref()))
                    });
                    if config {
                        self.reload();
                    } else {
                        self.tsc.invalidate();
                    }
                }
            }
            DidChangeWorkspaceFolders::METHOD => {
                if let Ok(params) =
                    serde_json::from_value::<DidChangeWorkspaceFoldersParams>(notification.params)
                {
                    let mut roots = self.workspace.roots();
                    for folder in params.event.removed {
                        if let Some(path) = uri::path(&folder.uri) {
                            roots.retain(|root| root != &path);
                        }
                    }
                    for folder in params.event.added {
                        if let Some(path) = uri::path(&folder.uri) {
                            roots.push(path);
                        }
                    }
                    self.workspace.set_roots(roots);
                    self.reload();
                }
            }
            _ => {}
        }
    }

    fn document_path(&self, uri: &Uri, document: &Document) -> PathBuf {
        analysis::buffer_path(uri, document.language()).unwrap_or_else(|| PathBuf::from("stdin"))
    }

    fn schedule_tsc(&self, uri: Uri) {
        let settings = self.settings();
        if !settings.trusted || !settings.type_aware {
            return;
        }
        let Some(path) = uri::path(&uri) else {
            return;
        };
        if !is_typescript(&path) {
            return;
        }
        self.tsc
            .schedule(path.clone(), self.workspace.root_for(&path));
    }
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
}

fn parse<T: DeserializeOwned>(value: &Value) -> std::result::Result<T, RpcError> {
    serde_json::from_value(value.clone())
        .map_err(|error| RpcError::invalid_params(error.to_string()))
}

fn capabilities() -> ServerCapabilities {
    ServerCapabilities {
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

fn is_typescript(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs")
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

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
                Message::Response(_) | Message::Notification(_) => continue,
                Message::Request(request) => panic!("unexpected request: {request:?}"),
            }
        }
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
        assert!(edits[0]["newText"].as_str().unwrap().contains("\"a\": 1"));

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
}
