#![forbid(unsafe_code)]

mod documents;

use std::collections::HashMap;
use std::error::Error;
use std::io;

use documents::{Document, Documents, position_in};
use dot_code_core::{Action, Feature, Request as Query, Trigger, apply};
use dot_code_workspace::{packages, siblings};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeConfiguration, DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument,
    Notification as _, ShowMessage,
};
use lsp_types::request::{CodeActionRequest, Request as _};
use lsp_types::{
    CodeAction, CodeActionDisabled, CodeActionKind, CodeActionOptions, CodeActionOrCommand,
    CodeActionParams, CodeActionProviderCapability, CodeActionTriggerKind, Command,
    DidChangeConfigurationParams, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DocumentChanges, InitializeParams, MessageType, OneOf,
    OptionalVersionedTextDocumentIdentifier, PositionEncodingKind, ServerCapabilities,
    ShowMessageParams, TextDocumentEdit, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextEdit, Uri, WorkspaceEdit,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

pub type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

/// Client-side command that starts an inline rename at a position.
pub const RENAME_COMMAND: &str = "dotCode.rename";

/// Resolves the next or previous file in Explorer order.
pub const SIBLING_FILE: &str = "dotCode/siblingFile";

/// Detects the package manager and directory to install from.
pub const PACKAGE_MANAGER: &str = "dotCode/packageManager";

pub fn features() -> Vec<Box<dyn Feature>> {
    vec![Box::new(dot_code_typescript::TypeScript::default())]
}

pub fn run(connection: &Connection, features: Vec<Box<dyn Feature>>) -> Result<()> {
    let mut server = Server {
        documents: Documents::default(),
        features,
        versioned: false,
    };
    let (id, params) = connection.initialize_start()?;
    connection.initialize_finish(
        id,
        json!({
            "capabilities": server.capabilities(),
            "serverInfo": { "name": "dot-code-server", "version": env!("CARGO_PKG_VERSION") },
        }),
    )?;
    let params: InitializeParams = serde_json::from_value(params)?;
    server.versioned = params
        .capabilities
        .workspace
        .and_then(|workspace| workspace.workspace_edit)
        .and_then(|edit| edit.document_changes)
        .unwrap_or(false);
    if let Some(options) = params.initialization_options {
        for warning in server.configure(&options) {
            connection.sender.send(warning.into())?;
        }
    }
    for message in &connection.receiver {
        match message {
            Message::Request(request) => {
                if connection.handle_shutdown(&request)? {
                    return Ok(());
                }
                connection.sender.send(server.respond(request).into())?;
            }
            Message::Notification(notification) => {
                for warning in server.notify(notification) {
                    connection.sender.send(warning.into())?;
                }
            }
            Message::Response(_) => {}
        }
    }
    Ok(())
}

struct Server {
    documents: Documents,
    features: Vec<Box<dyn Feature>>,
    versioned: bool,
}

impl Server {
    fn capabilities(&self) -> ServerCapabilities {
        let kinds = self
            .features
            .iter()
            .flat_map(|feature| feature.kinds())
            .map(|kind| CodeActionKind::from(kind.to_string()))
            .collect();
        ServerCapabilities {
            position_encoding: Some(PositionEncodingKind::UTF16),
            text_document_sync: Some(TextDocumentSyncCapability::Kind(
                TextDocumentSyncKind::INCREMENTAL,
            )),
            code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
                code_action_kinds: Some(kinds),
                resolve_provider: Some(false),
                work_done_progress_options: Default::default(),
            })),
            ..ServerCapabilities::default()
        }
    }

    /// Hands each feature its section of the `dotCode` settings.
    fn configure(&mut self, settings: &Value) -> Vec<Notification> {
        let root = settings.get("dotCode").unwrap_or(settings);
        self.features
            .iter_mut()
            .filter_map(|feature| {
                let section = root
                    .get(feature.section())
                    .filter(|section| section.is_object())
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let error = feature.configure(section).err()?;
                Some(Notification::new(
                    ShowMessage::METHOD.into(),
                    ShowMessageParams {
                        typ: MessageType::WARNING,
                        message: format!("dotCode.{}: {error}", feature.section()),
                    },
                ))
            })
            .collect()
    }

    fn respond(&mut self, request: Request) -> Response {
        match request.method.as_str() {
            CodeActionRequest::METHOD => reply(request, |params: CodeActionParams| {
                Ok(self.code_actions(params))
            }),
            SIBLING_FILE => reply(request, |params| siblings::sibling(&params)),
            PACKAGE_MANAGER => reply(request, |params| Ok(packages::plan(&params))),
            method => Response::new_err(
                request.id,
                ErrorCode::MethodNotFound as i32,
                format!("unhandled method: {method}"),
            ),
        }
    }

    fn notify(&mut self, notification: Notification) -> Vec<Notification> {
        let params = notification.params;
        match notification.method.as_str() {
            DidOpenTextDocument::METHOD => {
                if let Ok(params) = serde_json::from_value::<DidOpenTextDocumentParams>(params) {
                    let document = params.text_document;
                    self.documents.open(
                        document.uri,
                        Document::new(document.text, document.language_id, document.version),
                    );
                }
            }
            DidChangeTextDocument::METHOD => {
                if let Ok(params) = serde_json::from_value::<DidChangeTextDocumentParams>(params)
                    && let Some(document) = self.documents.get_mut(&params.text_document.uri)
                {
                    document.version = params.text_document.version;
                    for change in params.content_changes {
                        document.change(change);
                    }
                }
            }
            DidCloseTextDocument::METHOD => {
                if let Ok(params) = serde_json::from_value::<DidCloseTextDocumentParams>(params) {
                    self.documents.close(&params.text_document.uri);
                }
            }
            DidChangeConfiguration::METHOD => {
                if let Ok(params) = serde_json::from_value::<DidChangeConfigurationParams>(params) {
                    return self.configure(&params.settings);
                }
            }
            _ => {}
        }
        Vec::new()
    }

    fn code_actions(&mut self, params: CodeActionParams) -> Vec<CodeActionOrCommand> {
        let uri = params.text_document.uri;
        let Some(document) = self.documents.get(&uri) else {
            return Vec::new();
        };
        let Some(selection) = document.range(params.range) else {
            return Vec::new();
        };
        let only: Vec<String> = params
            .context
            .only
            .unwrap_or_default()
            .iter()
            .map(|kind| kind.as_str().to_string())
            .collect();
        let query = Query {
            source: document.text(),
            language: document.language(),
            selection,
            trigger: if params.context.trigger_kind == Some(CodeActionTriggerKind::AUTOMATIC) {
                Trigger::Automatic
            } else {
                Trigger::Invoked
            },
            only: &only,
        };
        self.features
            .iter_mut()
            .filter(|feature| feature.languages().contains(&query.language))
            .flat_map(|feature| feature.code_actions(&query))
            .map(|action| code_action(action, &uri, document, self.versioned))
            .collect()
    }
}

fn reply<P: DeserializeOwned, R: Serialize>(
    request: Request,
    handle: impl FnOnce(P) -> io::Result<R>,
) -> Response {
    let params = match serde_json::from_value(request.params) {
        Ok(params) => params,
        Err(error) => {
            return Response::new_err(
                request.id,
                ErrorCode::InvalidParams as i32,
                error.to_string(),
            );
        }
    };
    match handle(params) {
        Ok(result) => Response::new_ok(request.id, result),
        Err(error) => Response::new_err(
            request.id,
            ErrorCode::RequestFailed as i32,
            error.to_string(),
        ),
    }
}

fn code_action(
    action: Action,
    uri: &Uri,
    document: &Document,
    versioned: bool,
) -> CodeActionOrCommand {
    let kind = Some(CodeActionKind::from(action.kind.to_string()));
    if let Some(reason) = action.disabled {
        return CodeActionOrCommand::CodeAction(CodeAction {
            title: action.title.into(),
            kind,
            disabled: Some(CodeActionDisabled {
                reason: reason.into(),
            }),
            ..CodeAction::default()
        });
    }
    let command = action.rename.map(|offset| Command {
        title: "Rename".into(),
        command: RENAME_COMMAND.into(),
        arguments: Some(vec![
            json!(uri),
            json!(position_in(&apply(document.text(), &action.edits), offset)),
        ]),
    });
    let edits: Vec<TextEdit> = action
        .edits
        .into_iter()
        .map(|edit| TextEdit {
            range: document.lsp_range(&edit.range),
            new_text: edit.text,
        })
        .collect();
    let edit = if versioned {
        WorkspaceEdit {
            document_changes: Some(DocumentChanges::Edits(vec![TextDocumentEdit {
                text_document: OptionalVersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version: Some(document.version),
                },
                edits: edits.into_iter().map(OneOf::Left).collect(),
            }])),
            ..WorkspaceEdit::default()
        }
    } else {
        WorkspaceEdit {
            changes: Some(HashMap::from([(uri.clone(), edits)])),
            ..WorkspaceEdit::default()
        }
    };
    CodeActionOrCommand::CodeAction(CodeAction {
        title: action.title.into(),
        kind,
        edit: Some(edit),
        command,
        ..CodeAction::default()
    })
}
