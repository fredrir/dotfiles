#![forbid(unsafe_code)]

use std::thread::{self, JoinHandle};

use dot_code_typescript::{EXTRACT_INTERFACE, EXTRACT_TYPE};
use lsp_server::{Connection, Message, Notification, Request, RequestId};
use serde_json::{Value, json};

const URI: &str = "file:///project/test.tsx";

struct Client {
    connection: Connection,
    server: JoinHandle<()>,
    next: i32,
}

impl Client {
    fn start() -> (Self, Value) {
        let (connection, server) = Connection::memory();
        let server = thread::spawn(move || {
            dot_code_server::run(&server, dot_code_server::features()).expect("server runs");
        });
        let mut client = Self {
            connection,
            server,
            next: 0,
        };
        let initialized = client.request(
            "initialize",
            json!({ "capabilities": { "workspace": { "workspaceEdit": { "documentChanges": true } } } }),
        );
        client.notify("initialized", json!({}));
        (client, initialized)
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = RequestId::from(self.next);
        self.connection
            .sender
            .send(Request::new(id.clone(), method.into(), params).into())
            .expect("server is listening");
        for message in &self.connection.receiver {
            if let Message::Response(response) = message
                && response.id == id
            {
                return response
                    .response_result
                    .unwrap_or_else(|error| panic!("{method} failed: {error:?}"));
            }
        }
        panic!("server stopped before answering {method}");
    }

    fn notify(&self, method: &str, params: Value) {
        self.connection
            .sender
            .send(Notification::new(method.into(), params).into())
            .expect("server is listening");
    }

    fn open(&self, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": URI, "languageId": "typescriptreact", "version": 1, "text": text } }),
        );
    }

    fn code_actions(&mut self, line: u32, character: u32, only: &[&str]) -> Vec<Value> {
        let position = json!({ "line": line, "character": character });
        let actions = self.request(
            "textDocument/codeAction",
            json!({
                "textDocument": { "uri": URI },
                "range": { "start": position, "end": position },
                "context": { "diagnostics": [], "only": only, "triggerKind": 1 },
            }),
        );
        serde_json::from_value(actions).expect("code action list")
    }

    fn stop(mut self) {
        self.request("shutdown", Value::Null);
        self.notify("exit", Value::Null);
        self.server.join().expect("server exits cleanly");
    }
}

fn edits(action: &Value) -> &Vec<Value> {
    action["edit"]["documentChanges"][0]["edits"]
        .as_array()
        .expect("versioned edits")
}

#[test]
fn initialize_advertises_incremental_sync_and_every_feature_kind() {
    let (client, initialized) = Client::start();
    let capabilities = &initialized["capabilities"];
    assert_eq!(capabilities["textDocumentSync"], 2);
    let kinds = capabilities["codeActionProvider"]["codeActionKinds"]
        .as_array()
        .expect("kinds");
    assert!(kinds.contains(&json!(EXTRACT_INTERFACE)));
    assert_eq!(initialized["serverInfo"]["name"], "dot-code-server");
    client.stop();
}

#[test]
fn edits_and_rename_positions_are_utf16_after_incremental_changes() {
    let (mut client, _) = Client::start();
    client.open("function f(v: { a: string }) {}\n");
    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": URI, "version": 2 },
            "contentChanges": [{
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } },
                "text": "/* 😀 */ ",
            }],
        }),
    );
    // "/* 😀 */ " is 9 UTF-16 units wide: the emoji takes two.
    let actions = client.code_actions(0, 9 + 20, &[EXTRACT_INTERFACE]);
    assert_eq!(actions.len(), 1);
    let action = &actions[0];
    assert_eq!(action["kind"], EXTRACT_INTERFACE);
    assert_eq!(
        action["edit"]["documentChanges"][0]["textDocument"]["version"],
        2
    );
    let edits = edits(action);
    assert_eq!(
        edits[0]["range"]["start"],
        json!({ "line": 0, "character": 0 })
    );
    assert_eq!(
        edits[1]["range"],
        json!({ "start": { "line": 0, "character": 23 }, "end": { "line": 0, "character": 36 } })
    );
    assert_eq!(edits[1]["newText"], "Props");
    assert_eq!(
        action["command"]["command"],
        dot_code_server::RENAME_COMMAND
    );
    assert_eq!(
        action["command"]["arguments"],
        json!([URI, { "line": 0, "character": 10 }])
    );
    client.stop();
}

#[test]
fn configuration_changes_reach_the_features() {
    let (mut client, _) = Client::start();
    client.notify(
        "workspace/didChangeConfiguration",
        json!({ "settings": { "dotCode": { "typescript": { "extract": { "name": "${name}Props", "rename": false } } } } }),
    );
    client.open("function Card(v: { a: string }) {}\n");
    let actions = client.code_actions(0, 20, &[EXTRACT_TYPE]);
    assert_eq!(actions.len(), 1);
    assert!(
        edits(&actions[0])[0]["newText"]
            .as_str()
            .is_some_and(|text| text.starts_with("type CardProps = "))
    );
    assert_eq!(actions[0].get("command"), None);
    client.stop();
}

#[test]
fn unrelated_kinds_and_languages_get_nothing() {
    let (mut client, _) = Client::start();
    client.open("function f(v: { a: string }) {}\n");
    assert!(client.code_actions(0, 20, &["quickfix"]).is_empty());
    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": "file:///project/notes.md", "languageId": "markdown", "version": 1, "text": "{ a: string }" } }),
    );
    let position = json!({ "line": 0, "character": 3 });
    let actions = client.request(
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": "file:///project/notes.md" },
            "range": { "start": position, "end": position },
            "context": { "diagnostics": [] },
        }),
    );
    assert_eq!(actions, json!([]));
    client.stop();
}
