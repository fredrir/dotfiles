#![forbid(unsafe_code)]

use serde_json::{Value, json};
use testkit::{Bin, stdout};

fn server(args: &[&str]) -> Bin {
    Bin::new(env!("CARGO_BIN_EXE_dot-code-server"))
        .args(args)
        .plain()
}

#[test]
fn the_long_version_flag_reports_the_crate_version() {
    let output = server(&["--version"]).output();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        stdout(&output).trim(),
        format!("dot-code-server {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn the_short_version_flag_matches_the_long_one() {
    assert_eq!(
        stdout(&server(&["-v"]).output()),
        stdout(&server(&["--version"]).output())
    );
}

#[test]
fn the_completions_flag_answers_for_this_tool() {
    let output = server(&["--completions", "zsh"]).output();
    assert!(output.status.success(), "{output:?}");
    assert!(
        stdout(&output).contains("#compdef dot-code-server"),
        "{output:?}"
    );
}

#[test]
fn the_command_dump_describes_the_parser() {
    let dump: Value = serde_json::from_str(&stdout(&server(&["--command-dump"]).output())).unwrap();
    assert_eq!(dump["version"], 1);
    assert_eq!(dump["command"]["path"], json!(["dot-code-server"]));
    let params = dump["command"]["params"].as_array().unwrap();
    assert!(
        params
            .iter()
            .any(|param| param["opts"] == json!(["-v", "--version"]))
    );
}

fn frame(message: Value) -> String {
    let body = message.to_string();
    format!("Content-Length: {}\r\n\r\n{body}", body.len())
}

#[test]
fn the_stdio_flag_lsp_clients_pass_serves_a_session() {
    let session: String = [
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "capabilities": {} } }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "shutdown" }),
        json!({ "jsonrpc": "2.0", "method": "exit" }),
    ]
    .into_iter()
    .map(frame)
    .collect();
    let output = server(&["--stdio"]).stdin(&session).output();
    assert!(output.status.success(), "{output:?}");
    assert!(stdout(&output).contains("\"serverInfo\""), "{output:?}");
}

#[test]
fn an_unknown_flag_is_a_usage_error() {
    assert_eq!(server(&["--nowhere"]).output().status.code(), Some(2));
}
