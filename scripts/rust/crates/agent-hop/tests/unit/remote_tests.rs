use std::ffi::OsString;

use super::*;

#[test]
fn every_remote_path_is_quoted_as_one_shell_word() {
    let path = Path::new("/home/fred rir/a'b; touch nope");
    assert_eq!(
        exists_script(path).unwrap(),
        "if [ -e '/home/fred rir/a'\\''b; touch nope' ] || [ -L '/home/fred rir/a'\\''b; touch nope' ]; then printf 'yes\\n'; else printf 'no\\n'; fi"
    );
    assert_eq!(
        compare_script(path).unwrap(),
        "cmp -s - '/home/fred rir/a'\\''b; touch nope'"
    );
    assert_eq!(
        mkdir_script(path).unwrap(),
        "mkdir -p -- '/home/fred rir/a'\\''b; touch nope'"
    );
}

#[test]
fn preflight_checks_the_workspace_agent_and_login_shell() {
    let script = preflight_script(Path::new("/home/fred rir/project"), Agent::Codex).unwrap();
    assert!(script.starts_with("test -d '/home/fred rir/project'"));
    assert!(script.contains("command -v 'codex'"));
    assert!(script.contains("command -v 'zsh'"));
    assert!(script.contains("workspace does not exist"));
}

#[test]
fn codex_forks_in_the_mapped_workspace() {
    assert_eq!(
        fork_command(Path::new("/home/fred rir/project"), Agent::Codex, "id'$(x)").unwrap(),
        "codex fork 'id'\\''$(x)' -C '/home/fred rir/project'"
    );
}

#[test]
fn claude_forks_without_interpreting_the_session_id() {
    assert_eq!(
        fork_command(
            Path::new("/home/fred rir/project"),
            Agent::Claude,
            "id; reboot"
        )
        .unwrap(),
        "claude --resume 'id; reboot' --fork-session"
    );
}

#[test]
fn launch_enters_the_workspace_through_an_interactive_login_zsh() {
    let script =
        launch_script(Path::new("/home/fred rir/project"), Agent::Codex, "0199-id").unwrap();
    assert_eq!(
        script,
        r#"cd -- '/home/fred rir/project' && exec zsh -lic 'codex fork '\''0199-id'\'' -C '\''/home/fred rir/project'\'''"#
    );
}

#[test]
fn noninteractive_ssh_has_no_tty_and_keeps_the_script_one_argument() {
    assert_eq!(
        ssh_session(Host::Archie, "test -d '/a b'", false).args(),
        [
            "-T",
            "-o",
            "ConnectTimeout=8",
            "-o",
            "LogLevel=ERROR",
            "--",
            "archie",
            "test -d '/a b'",
        ]
        .map(OsString::from)
    );
}

#[test]
fn interactive_ssh_allocates_a_tty_for_the_agent() {
    let arguments = ssh_session(Host::Macie, "exec zsh -lic 'codex'", true).args();
    assert_eq!(arguments[0], "-tt");
    assert_eq!(arguments[5], "--");
    assert_eq!(arguments[6], "macie");
    assert_eq!(arguments[7], "exec zsh -lic 'codex'");
}

#[test]
fn machine_requests_have_no_tty_and_cannot_prompt_or_inject_shell_words() {
    let script = machine_script(&[
        "__machine".to_owned(),
        "preview".to_owned(),
        "--session".to_owned(),
        "id'; touch /tmp/nope; '".to_owned(),
    ]);
    assert_eq!(
        script,
        "export PATH=\"$HOME/.local/bin:$PATH\"; exec agent-hop '__machine' 'preview' '--session' 'id'\\''; touch /tmp/nope; '\\'''"
    );
    let arguments = machine_ssh_session(Host::Archie, &script).args();
    assert_eq!(arguments[0], "-T");
    assert!(arguments.iter().any(|value| value == "BatchMode=yes"));
    assert!(
        arguments
            .iter()
            .any(|value| value == "ConnectionAttempts=1")
    );
    assert!(
        arguments
            .iter()
            .any(|value| value == "ServerAliveInterval=5")
    );
    assert!(
        arguments
            .iter()
            .any(|value| value == "ServerAliveCountMax=3")
    );
    assert_eq!(arguments[13], "--");
    assert_eq!(arguments[14], "archie");
    assert_eq!(arguments[15], script.as_str());
}

#[test]
fn catalog_protocol_round_trips_validated_transfer_sources() {
    let catalog = RemoteCatalog {
        sessions: vec![RemoteSession {
            agent: Agent::Codex,
            id: "safe-id".to_owned(),
            title: "A useful session".to_owned(),
            project: "app".to_owned(),
            workspace: PathBuf::from("/Users/fred/work/app"),
            transcript: PathBuf::from(
                "/Users/fred/.codex/sessions/2026/09/02/rollout-now-safe-id.jsonl",
            ),
            companion: None,
            modified_ms: 42,
        }],
        warnings: vec!["Claude store unavailable".to_owned()],
    };
    let encoded = encode_catalog_response(&catalog).unwrap();
    let parsed = parse_catalog_response(encoded.as_bytes(), Path::new("/Users/fred"), 10).unwrap();
    assert_eq!(parsed, catalog);
}

#[test]
fn lineage_protocol_round_trips_archived_ancestors_and_offsets() {
    let parent_id = "01999999-1111-7222-8333-444444444444";
    let child_id = "01999999-1111-7222-8333-555555555555";
    let lineage = RemoteLineage {
        agent: Agent::Codex,
        selected_id: child_id.to_string(),
        artifacts: vec![
            ArtifactDescriptor {
                session_id: parent_id.to_string(),
                workspace: "/Users/fred/work/app".into(),
                transcript: format!(
                    "/Users/fred/.codex/archived_sessions/rollout-{parent_id}.jsonl"
                )
                .into(),
                history_base: None,
                bytes: 200,
                sha256: "a".repeat(64),
            },
            ArtifactDescriptor {
                session_id: child_id.to_string(),
                workspace: "/Users/fred/work/app".into(),
                transcript: format!(
                    "/Users/fred/.codex/sessions/2026/09/02/rollout-{child_id}.jsonl"
                )
                .into(),
                history_base: Some(HistoryBase {
                    thread_id: parent_id.to_string(),
                    end_ordinal_exclusive: 2,
                    end_byte_offset: 200,
                }),
                bytes: 100,
                sha256: "b".repeat(64),
            },
        ],
    };
    let encoded = encode_lineage_response(&lineage).unwrap();
    let parsed = parse_lineage_response(
        encoded.as_bytes(),
        Path::new("/Users/fred"),
        Agent::Codex,
        child_id,
    )
    .unwrap();
    assert_eq!(parsed, lineage);
}

#[test]
fn older_catalog_without_project_uses_workspace_basename() {
    let response = json!({
        "protocol": MACHINE_PROTOCOL,
        "version": MACHINE_PROTOCOL_VERSION,
        "kind": "catalog",
        "ok": true,
        "data": {"sessions": [{
            "agent": "codex",
            "id": "safe-id",
            "title": "title",
            "workspace": "/Users/fred/work/app",
            "transcript": "/Users/fred/.codex/sessions/2026/09/02/rollout-now-safe-id.jsonl",
            "companion": null,
            "modified_ms": 1,
        }], "warnings": []},
    })
    .to_string();

    let parsed = parse_catalog_response(response.as_bytes(), Path::new("/Users/fred"), 10).unwrap();
    assert_eq!(parsed.sessions[0].project, "app");
}

#[test]
fn catalog_protocol_rejects_paths_outside_the_peer_home_and_mismatched_ids() {
    let outside = json!({
        "protocol": MACHINE_PROTOCOL,
        "version": MACHINE_PROTOCOL_VERSION,
        "kind": "catalog",
        "ok": true,
        "data": {"sessions": [{
            "agent": "claude",
            "id": "safe-id",
            "title": "title",
            "workspace": "/Users/fred/work",
            "transcript": "/tmp/safe-id.jsonl",
            "companion": null,
            "modified_ms": 1,
        }], "warnings": []},
    })
    .to_string();
    assert!(
        parse_catalog_response(outside.as_bytes(), Path::new("/Users/fred"), 10)
            .unwrap_err()
            .contains("outside")
    );

    let mismatch = outside.replace(
        "/tmp/safe-id.jsonl",
        "/Users/fred/.claude/projects/-Users-fred-work/different.jsonl",
    );
    assert!(
        parse_catalog_response(mismatch.as_bytes(), Path::new("/Users/fred"), 10)
            .unwrap_err()
            .contains("does not match")
    );
}

#[test]
fn preview_protocol_allows_only_sanitized_user_and_assistant_text() {
    let wire = json!({
        "protocol": MACHINE_PROTOCOL,
        "version": MACHINE_PROTOCOL_VERSION,
        "kind": "preview",
        "ok": true,
        "data": {
            "title": "hello\u{1b}[31m red",
            "messages": [
                {"role": "user", "text": "one\n two"},
                {"role": "assistant", "text": "abcdefghij"}
            ],
            "truncated": false,
        },
    })
    .to_string();
    let preview = parse_preview_response(wire.as_bytes(), 8).unwrap();
    assert_eq!(preview.title, "hello red");
    assert_eq!(preview.messages[0].text, "one two");
    assert_eq!(preview.messages[1].text, "a");
    assert!(preview.truncated);

    let tool = wire.replace("\"user\"", "\"tool\"");
    assert!(
        parse_preview_response(tool.as_bytes(), 100)
            .unwrap_err()
            .contains("unsupported message role")
    );
}

#[test]
fn incompatible_protocol_versions_fail_closed() {
    let response = json!({
        "protocol": MACHINE_PROTOCOL,
        "version": 999,
        "kind": "preview",
        "ok": true,
        "data": {},
    })
    .to_string();
    assert!(
        parse_preview_response(response.as_bytes(), 100)
            .unwrap_err()
            .contains("incompatible")
    );
}

#[test]
fn remote_warnings_are_bounded_and_terminal_safe() {
    let response = json!({
        "protocol": MACHINE_PROTOCOL,
        "version": MACHINE_PROTOCOL_VERSION,
        "kind": "catalog",
        "ok": true,
        "data": {
            "sessions": [],
            "warnings": ["bad\u{1b}[31m red\nnext"],
        },
    })
    .to_string();
    let parsed = parse_catalog_response(response.as_bytes(), Path::new("/Users/fred"), 10).unwrap();
    assert_eq!(parsed.warnings, ["bad red next"]);

    let too_many = json!({
        "protocol": MACHINE_PROTOCOL,
        "version": MACHINE_PROTOCOL_VERSION,
        "kind": "catalog",
        "ok": true,
        "data": {
            "sessions": [],
            "warnings": vec!["warning"; MAX_REMOTE_WARNINGS + 1],
        },
    })
    .to_string();
    assert!(
        parse_catalog_response(too_many.as_bytes(), Path::new("/Users/fred"), 10)
            .unwrap_err()
            .contains("too many warnings")
    );
}

#[test]
fn companion_export_round_trips_only_relative_regular_entries() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    let destination = directory.path().join("destination");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&destination).unwrap();
    fs::create_dir(source.join("nested")).unwrap();
    fs::write(source.join("root.txt"), b"root\nbytes").unwrap();
    fs::write(source.join("nested/child.jsonl"), b"{\"safe\":true}\n").unwrap();

    let mut stream = Vec::new();
    write_companion_export(&source, &mut stream).unwrap();
    read_companion_export(io::Cursor::new(stream), &destination).unwrap();

    assert_eq!(
        fs::read(destination.join("root.txt")).unwrap(),
        b"root\nbytes"
    );
    assert_eq!(
        fs::read(destination.join("nested/child.jsonl")).unwrap(),
        b"{\"safe\":true}\n"
    );
}

#[test]
fn companion_export_rejects_links_and_unsafe_wire_paths() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    let destination = directory.path().join("destination");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&destination).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/etc/passwd", source.join("link")).unwrap();
        assert!(write_companion_export(&source, Vec::new()).is_err());
    }

    let stream = format!(
        "{}{}\n",
        String::from_utf8_lossy(COMPANION_STREAM_MAGIC),
        json!({"kind":"file", "path":"../escape", "bytes":0})
    );
    let error = read_companion_export(io::Cursor::new(stream), &destination).unwrap_err();
    assert!(error.contains("unsafe relative path"));
    assert!(!directory.path().join("escape").exists());
}

#[test]
fn capped_reader_drains_input_without_retaining_the_excess() {
    let captured = read_capped(io::Cursor::new(vec![b'x'; 100]), 12).unwrap();
    assert_eq!(captured.bytes, vec![b'x'; 12]);
    assert!(captured.truncated);
}

#[test]
fn subprocess_timeout_is_enforced() {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg("sleep 1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let error = output_with_timeout(command, 100, Duration::from_millis(20)).unwrap_err();
    assert!(error.contains("timed out"));
}

#[test]
fn redirected_transfer_timeout_and_diagnostics_are_bounded() {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg("sleep 1")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let error = redirected_output_with_timeout(command, Duration::from_millis(20)).unwrap_err();
    assert!(error.contains("timed out"));

    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg("head -c 100000 /dev/zero >&2")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let output = redirected_output_with_timeout(command, Duration::from_secs(1)).unwrap();
    assert!(output.stderr.truncated);
    assert_eq!(output.stderr.bytes.len(), MAX_ERROR_OUTPUT);
}
