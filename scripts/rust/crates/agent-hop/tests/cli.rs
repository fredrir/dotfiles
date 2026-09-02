use std::process::{Command, Output};

use sha2::{Digest, Sha256};

const MACHINE_PROTOCOL: &str = "2";

fn agent_hop(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agent-hop"))
        .args(args)
        .output()
        .expect("agent-hop runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn help_describes_the_command_and_its_examples() {
    let output = agent_hop(&["--help"]);
    assert!(output.status.success(), "{output:?}");
    let text = stdout(&output);
    assert!(text.starts_with("Move a Codex or Claude Code CLI session between workstations"));
    assert!(text.contains("Usage: agent-hop"));
    assert!(text.contains("Browse, preview, and move your sessions"));
    assert!(text.contains("agent-hop codex SESSION_ID"));
    assert!(text.contains("agent-hop --list"));
    assert!(text.contains("[possible values: codex, claude]"));
    assert!(!text.contains("__machine"));
}

#[test]
fn version_names_the_binary() {
    let output = agent_hop(&["--version"]);
    assert!(output.status.success(), "{output:?}");
    assert!(stdout(&output).starts_with("agent-hop "));
}

#[test]
fn zsh_completions_cover_the_public_surface() {
    let output = agent_hop(&["--completions", "zsh"]);
    assert!(output.status.success(), "{output:?}");
    let script = stdout(&output);
    assert!(script.starts_with("#compdef agent-hop\n"));
    for value in [
        "codex",
        "claude",
        "--dry-run",
        "--no-connect",
        "--color",
        "--list",
        "--completions",
    ] {
        assert!(script.contains(value), "completion script has no {value}");
    }
    assert!(script.contains("(auto always never)"));
}

#[test]
fn the_command_dump_describes_the_parser() {
    let output = agent_hop(&["--command-dump"]);
    assert!(output.status.success(), "{output:?}");
    let dump = stdout(&output);
    assert!(dump.starts_with(
        "C\tagent-hop\t0\tMove a Codex or Claude Code CLI session between workstations"
    ));
    assert!(dump.contains("\toption\tdry_run\t-n,--dry-run\t"));
    assert!(dump.contains("\toption\tno_connect\t--no-connect\t"));
    assert!(dump.contains("\toption\tcolor\t--color\tWHEN\t"));
    assert!(dump.contains("\toption\tlist\t--list\t"));
    assert!(dump.contains("\targument\tagent\t\tAGENT\t"));
    assert!(dump.contains("\targument\tsession_id\t\tSESSION_ID\t"));
}

#[test]
fn a_bare_noninteractive_invocation_prints_help_and_succeeds() {
    let output = agent_hop(&[]);
    assert!(output.status.success(), "{output:?}");
    assert!(stderr(&output).is_empty(), "{output:?}");
    let text = stdout(&output);
    assert!(text.starts_with("Move a Codex or Claude Code CLI session between workstations"));
    assert!(text.contains("Usage: agent-hop"));
}

#[test]
fn the_hidden_catalog_treats_absent_optional_stores_as_a_clean_empty_catalog() {
    let directory = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_agent-hop"))
        .args([
            "__machine",
            "catalog",
            "--protocol",
            MACHINE_PROTOCOL,
            "--limit",
            "10",
        ])
        .env("HOME", directory.path())
        .output()
        .expect("agent-hop machine catalog runs");
    assert!(output.status.success(), "{output:?}");
    assert!(stderr(&output).is_empty(), "{output:?}");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["protocol"], "agent-hop-machine");
    assert_eq!(value["version"], 2);
    assert_eq!(value["kind"], "catalog");
    assert_eq!(value["ok"], true);
    assert_eq!(value["data"]["sessions"].as_array().unwrap().len(), 0);
    assert!(value["data"]["warnings"].as_array().unwrap().is_empty());
}

#[test]
fn the_hidden_export_protocol_streams_a_validated_stable_session() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    std::fs::create_dir_all(home.join("work")).unwrap();
    let home = std::fs::canonicalize(home).unwrap();
    let workspace = home.join("work");
    let id = "session-export";
    let transcript = home.join(format!(
        ".codex/sessions/2026/09/02/rollout-2026-09-02T00-00-00-{id}.jsonl"
    ));
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    let content = format!(
        "{}\n{}\n",
        serde_json::json!({"type":"session_meta","payload":{"id":id,"cwd":workspace,"thread_source":"user","source":"cli"}}),
        serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":"hello"}}),
    );
    std::fs::write(&transcript, &content).unwrap();
    let hash = format!("{:x}", Sha256::digest(content.as_bytes()));
    let bytes = content.len().to_string();
    let output = Command::new(env!("CARGO_BIN_EXE_agent-hop"))
        .args([
            "__machine",
            "export",
            "--protocol",
            MACHINE_PROTOCOL,
            "--agent",
            "codex",
            "--session",
            id,
            "--sha256",
            &hash,
            "--bytes",
            &bytes,
        ])
        .env("HOME", &home)
        .output()
        .expect("agent-hop machine export runs");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, content.as_bytes());
    assert!(output.stderr.is_empty(), "{output:?}");
}

#[test]
fn the_hidden_companion_export_is_bound_to_session_workspace_identity() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    std::fs::create_dir_all(home.join("work")).unwrap();
    let home = std::fs::canonicalize(home).unwrap();
    let workspace = home.join("work");
    let id = "claude-export";
    let transcript = home.join(format!(".claude/projects/project/{id}.jsonl"));
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    std::fs::write(
        &transcript,
        format!(
            "{}\n",
            serde_json::json!({"type":"user","sessionId":id,"cwd":workspace,"message":{"role":"user","content":"hello"}})
        ),
    )
    .unwrap();
    let companion = transcript.with_extension("");
    std::fs::create_dir(&companion).unwrap();
    std::fs::write(companion.join("attachment.txt"), "bound-content").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_agent-hop"))
        .args([
            "__machine",
            "export-companion",
            "--protocol",
            MACHINE_PROTOCOL,
            "--agent",
            "claude",
            "--session",
            id,
            "--workspace",
            workspace.to_str().unwrap(),
        ])
        .env("HOME", &home)
        .output()
        .expect("agent-hop machine companion export runs");
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.starts_with(b"agent-hop-companion/1\n"));
    assert!(
        output
            .stdout
            .windows(b"bound-content".len())
            .any(|window| window == b"bound-content")
    );
    assert!(output.stderr.is_empty(), "{output:?}");

    let wrong_workspace = home.join("other");
    std::fs::create_dir(&wrong_workspace).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_agent-hop"))
        .args([
            "__machine",
            "export-companion",
            "--protocol",
            MACHINE_PROTOCOL,
            "--agent",
            "claude",
            "--session",
            id,
            "--workspace",
            wrong_workspace.to_str().unwrap(),
        ])
        .env("HOME", &home)
        .output()
        .expect("agent-hop rejects mismatched companion identity");
    assert!(!output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(stderr(&output).contains("workspace changed"), "{output:?}");
}

#[test]
fn an_invalid_agent_is_a_usage_error() {
    let output = agent_hop(&["other"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let complaint = stderr(&output);
    assert!(complaint.contains("invalid value 'other'"));
    assert!(complaint.contains("codex, claude"));
}

#[test]
fn extra_arguments_are_a_usage_error() {
    let output = agent_hop(&["codex", "one", "two"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(stderr(&output).contains("unexpected argument 'two'"));
}

#[test]
fn every_color_mode_is_accepted() {
    for mode in ["auto", "always", "never"] {
        let output = agent_hop(&["--color", mode, "--help"]);
        assert!(output.status.success(), "{mode}: {output:?}");
    }
}

#[test]
fn an_invalid_color_mode_is_a_usage_error() {
    let output = agent_hop(&["--color", "sometimes", "--help"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let complaint = stderr(&output);
    assert!(complaint.contains("invalid value 'sometimes'"));
    assert!(complaint.contains("auto, always, never"));
}
