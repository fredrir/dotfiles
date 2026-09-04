use std::fs::OpenOptions;
use std::time::{Duration, UNIX_EPOCH};

use serde_json::json;
use tempfile::TempDir;

use super::*;

fn home() -> (TempDir, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("home");
    fs::create_dir_all(&home).unwrap();
    (temporary, home)
}

fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn codex_path(home: &Path, day: &str, id: &str) -> PathBuf {
    home.join(format!(
        ".codex/sessions/2026/09/{day}/rollout-2026-09-{day}T00-00-00-{id}.jsonl"
    ))
}

fn codex_record(id: &str, cwd: &Path, thread_source: Value, source: Value) -> String {
    format!(
        "{}\n",
        json!({
            "type": "session_meta",
            "ordinal": 0,
            "payload": {
                "id": id,
                "cwd": cwd,
                "thread_source": thread_source,
                "source": source,
                "future_field": {"is": "ignored"}
            }
        })
    )
}

fn claude_path(home: &Path, project: &str, id: &str) -> PathBuf {
    home.join(format!(".claude/projects/{project}/{id}.jsonl"))
}

fn set_modified(path: &Path, seconds: u64) {
    OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_secs(seconds))
        .unwrap();
}

#[test]
fn unreadable_or_missing_sibling_directories_do_not_hide_healthy_sessions() {
    let (temporary, home) = home();
    let healthy = temporary.path().join("healthy");
    fs::create_dir(&healthy).unwrap();
    let transcript = healthy.join("session.jsonl");
    fs::write(&transcript, "{}\n").unwrap();
    let scan = scan_jsonl_files(
        vec![temporary.path().join("missing"), healthy],
        Scan::default(),
    );
    assert_eq!(scan.regular, [transcript]);
    assert_eq!(scan.errors.len(), 1);
    drop(home);
}

#[test]
fn session_ids_are_opaque_safe_path_components() {
    for value in [
        "01999999-1111-7222-8333-444444444444",
        "future.id",
        "id with spaces",
    ] {
        assert_eq!(SessionId::new(value).unwrap().as_str(), value);
    }
    for value in ["", ".", "..", "a/b", "a\\b", "a\0b", "a\nb"] {
        assert!(SessionId::new(value).is_err(), "{value:?}");
    }
}

#[test]
fn workspace_mapping_is_component_safe() {
    let home = Path::new("/home/fredrir");
    assert_eq!(workspace_relative(home, home).unwrap(), Path::new(""));
    assert_eq!(
        workspace_relative(home, Path::new("/home/fredrir/src/project")).unwrap(),
        Path::new("src/project")
    );
    assert!(workspace_relative(home, Path::new("/home/fredrir-other")).is_err());
    assert!(workspace_relative(home, Path::new("/home/fredrir/../other")).is_err());
    assert!(workspace_relative(home, Path::new("relative")).is_err());
}

#[test]
fn claude_project_keys_match_the_cli_encoding() {
    assert_eq!(
        claude_project_key(Path::new("/home/fredrir/src/my-project_2")).unwrap(),
        "-home-fredrir-src-my-project_2"
    );
    assert_eq!(
        claude_project_key(Path::new("/Users/fréd/project")).unwrap(),
        "-Users-fr-d-project"
    );
    assert!(claude_project_key(Path::new("relative/path")).is_err());
}

#[test]
fn codex_latest_uses_only_user_cli_sessions() {
    let (_temporary, home) = home();
    let workspace = home.join("src/project");
    fs::create_dir_all(&workspace).unwrap();
    let cli_id = "01999999-1111-7222-8333-444444444441";
    let cli = codex_path(&home, "01", cli_id);
    write(
        &cli,
        &codex_record(cli_id, &workspace, json!("user"), json!("cli")),
    );
    let vscode_id = "01999999-1111-7222-8333-444444444442";
    let vscode = codex_path(&home, "02", vscode_id);
    write(
        &vscode,
        &codex_record(vscode_id, &workspace, json!("user"), json!("vscode")),
    );
    let child_id = "01999999-1111-7222-8333-444444444443";
    let child = codex_path(&home, "03", child_id);
    write(
        &child,
        &codex_record(
            child_id,
            &workspace,
            json!("subagent"),
            json!({"subagent": {"future": true}}),
        ),
    );
    set_modified(&cli, 1);
    set_modified(&vscode, 3);
    set_modified(&child, 4);
    let session = discover(&home, &workspace, Agent::Codex, None).unwrap();
    assert_eq!(session.id.as_str(), cli_id);
    assert_eq!(session.transcript, cli);
    assert_eq!(session.agent, Agent::Codex);
    assert!(session.companion.is_none());
}

#[test]
fn latest_selection_uses_mtime_and_a_stable_path_tie_break() {
    let (_temporary, home) = home();
    let workspace = home.join("src");
    fs::create_dir_all(&workspace).unwrap();
    let first_id = "01999999-1111-7222-8333-444444444441";
    let second_id = "01999999-1111-7222-8333-444444444442";
    let first = codex_path(&home, "01", first_id);
    let second = codex_path(&home, "02", second_id);
    write(
        &first,
        &codex_record(first_id, &workspace, json!("user"), json!("cli")),
    );
    write(
        &second,
        &codex_record(second_id, &workspace, json!("user"), json!("cli")),
    );
    set_modified(&first, 7);
    set_modified(&second, 7);
    assert_eq!(
        discover(&home, &workspace, Agent::Codex, None)
            .unwrap()
            .id
            .as_str(),
        first_id
    );
    set_modified(&second, 8);
    assert_eq!(
        discover(&home, &workspace, Agent::Codex, None)
            .unwrap()
            .id
            .as_str(),
        second_id
    );
}

#[test]
fn codex_requires_metadata_on_the_first_physical_line() {
    let (_temporary, home) = home();
    let workspace = home.join("src");
    let id = "01999999-1111-7222-8333-444444444441";
    let path = codex_path(&home, "01", id);
    write(
        &path,
        &format!(
            "\n{}",
            codex_record(id, &workspace, json!("user"), json!("cli"))
        ),
    );
    assert!(discover(&home, &workspace, Agent::Codex, Some(id)).is_err());
}

#[test]
fn catalog_candidate_failures_distinguish_invalid_records_from_storage_failures() {
    let (_temporary, home) = home();
    let invalid = codex_path(&home, "01", "invalid");
    write(&invalid, "not json\n");
    assert_eq!(
        parse_candidate_for_catalog(Agent::Codex, invalid)
            .unwrap_err()
            .kind,
        CandidateFailureKind::Invalid
    );

    assert_eq!(
        parse_candidate_for_catalog(Agent::Codex, home.join("missing.jsonl"))
            .unwrap_err()
            .kind,
        CandidateFailureKind::Storage
    );
}

#[test]
fn codex_explicit_discovery_refuses_metadata_and_filename_mismatch() {
    let (_temporary, home) = home();
    let workspace = home.join("src");
    let requested = "01999999-1111-7222-8333-444444444441";
    let other = "01999999-1111-7222-8333-444444444442";
    write(
        &codex_path(&home, "01", requested),
        &codex_record(other, &workspace, json!("user"), json!("cli")),
    );
    let error = discover(&home, &workspace, Agent::Codex, Some(requested)).unwrap_err();
    assert!(error.contains("not resumable"));
}

#[test]
fn duplicate_explicit_codex_ids_are_refused() {
    let (_temporary, home) = home();
    let workspace = home.join("src");
    let id = "01999999-1111-7222-8333-444444444441";
    for day in ["01", "02"] {
        write(
            &codex_path(&home, day, id),
            &codex_record(id, &workspace, json!("user"), json!("cli")),
        );
    }
    let error = discover(&home, &workspace, Agent::Codex, Some(id)).unwrap_err();
    assert!(error.contains("ambiguous"));
}

#[test]
fn malformed_unrelated_codex_files_do_not_hide_a_latest_session() {
    let (_temporary, home) = home();
    let workspace = home.join("src");
    let id = "01999999-1111-7222-8333-444444444441";
    write(&codex_path(&home, "01", "broken"), "not json\n");
    write(
        &codex_path(&home, "02", id),
        &codex_record(id, &workspace, json!("user"), json!("cli")),
    );
    assert_eq!(
        discover(&home, &workspace, Agent::Codex, None)
            .unwrap()
            .id
            .as_str(),
        id
    );
}

#[test]
fn claude_uses_the_first_non_sidechain_workspace() {
    let (_temporary, home) = home();
    let first = home.join("first");
    let second = home.join("second");
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(&second).unwrap();
    let id = "11111111-2222-4333-8444-555555555555";
    let path = claude_path(&home, "-home-first", id);
    let content = [
        json!({"type": "mode", "sessionId": id}),
        json!({"sessionId": id, "cwd": home.join("side"), "isSidechain": true}),
        json!({"sessionId": id, "cwd": first, "isSidechain": false}),
        json!({"sessionId": id, "cwd": second}),
    ]
    .into_iter()
    .map(|record| format!("{record}\n"))
    .collect::<String>();
    write(&path, &content);
    let session = discover(&home, &first, Agent::Claude, None).unwrap();
    assert_eq!(session.workspace, first);
    assert!(discover(&home, &second, Agent::Claude, None).is_err());
}

#[test]
fn claude_accepts_missing_and_non_boolean_sidechain_markers() {
    let (_temporary, home) = home();
    for (project, marker) in [("missing", None), ("string", Some(json!("true")))] {
        let workspace = home.join(project);
        fs::create_dir_all(&workspace).unwrap();
        let id = format!("{project}-session");
        let mut record = json!({"sessionId": id, "cwd": workspace});
        if let Some(marker) = marker {
            record["isSidechain"] = marker;
        }
        write(&claude_path(&home, project, &id), &format!("{record}\n"));
        assert!(discover(&home, &workspace, Agent::Claude, Some(&id)).is_ok());
    }
}

#[test]
fn claude_metadata_only_sessions_have_no_workspace() {
    let (_temporary, home) = home();
    let workspace = home.join("src");
    let id = "11111111-2222-4333-8444-555555555555";
    write(
        &claude_path(&home, "project", id),
        &format!("{}\n", json!({"type": "mode", "sessionId": id})),
    );
    let error = discover(&home, &workspace, Agent::Claude, Some(id)).unwrap_err();
    assert!(error.contains("no workspace"));
}

#[test]
fn claude_internal_session_id_must_match_the_filename() {
    let (_temporary, home) = home();
    let workspace = home.join("src");
    let id = "11111111-2222-4333-8444-555555555555";
    let other = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    write(
        &claude_path(&home, "project", id),
        &format!("{}\n", json!({"sessionId": other, "cwd": workspace})),
    );
    let error = discover(&home, &workspace, Agent::Claude, Some(id)).unwrap_err();
    assert!(error.contains("not resumable"));
}

#[test]
fn claude_companion_is_returned_and_nested_jsonl_is_not_discovered() {
    let (_temporary, home) = home();
    let workspace = home.join("src");
    fs::create_dir_all(&workspace).unwrap();
    let id = "11111111-2222-4333-8444-555555555555";
    let path = claude_path(&home, "project", id);
    write(
        &path,
        &format!("{}\n", json!({"sessionId": id, "cwd": workspace})),
    );
    let companion = path.with_extension("");
    write(
        &companion.join("subagents/nested.jsonl"),
        &format!("{}\n", json!({"cwd": home.join("wrong")})),
    );
    let session = discover(&home, &workspace, Agent::Claude, None).unwrap();
    assert_eq!(session.companion, Some(companion));
}

#[test]
fn latest_duplicate_claude_ids_are_refused_across_projects() {
    let (_temporary, home) = home();
    let workspace = home.join("src");
    fs::create_dir_all(&workspace).unwrap();
    let id = "11111111-2222-4333-8444-555555555555";
    let content = format!("{}\n", json!({"sessionId": id, "cwd": workspace}));
    write(&claude_path(&home, "one", id), &content);
    write(&claude_path(&home, "two", id), &content);
    let error = discover(&home, &workspace, Agent::Claude, None).unwrap_err();
    assert!(error.contains("ambiguous"));
}

#[test]
fn selected_workspace_must_be_absolute_and_below_home() {
    let (_temporary, home) = home();
    let id = "11111111-2222-4333-8444-555555555555";
    let path = claude_path(&home, "project", id);
    write(
        &path,
        &format!("{}\n", json!({"sessionId": id, "cwd": "/outside"})),
    );
    assert!(discover(&home, Path::new("/outside"), Agent::Claude, Some(id)).is_err());
    write(
        &path,
        &format!("{}\n", json!({"sessionId": id, "cwd": "relative"})),
    );
    assert!(discover(&home, Path::new("relative"), Agent::Claude, Some(id)).is_err());
}

#[test]
fn missing_stores_are_reported() {
    let (_temporary, home) = home();
    let workspace = home.join("src");
    assert!(
        discover(&home, &workspace, Agent::Codex, None)
            .unwrap_err()
            .contains("unavailable")
    );
    assert!(
        discover(&home, &workspace, Agent::Claude, None)
            .unwrap_err()
            .contains("unavailable")
    );
}

#[cfg(unix)]
#[test]
fn explicit_symlink_transcripts_are_refused() {
    use std::os::unix::fs::symlink;

    let (_temporary, home) = home();
    let workspace = home.join("src");
    let id = "11111111-2222-4333-8444-555555555555";
    let target = home.join("target.jsonl");
    write(
        &target,
        &format!("{}\n", json!({"sessionId": id, "cwd": workspace})),
    );
    let path = claude_path(&home, "project", id);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    symlink(&target, &path).unwrap();
    let error = discover(&home, &workspace, Agent::Claude, Some(id)).unwrap_err();
    assert!(error.contains("unsafe non-regular"));
}

#[cfg(unix)]
#[test]
fn symlink_companions_are_refused() {
    use std::os::unix::fs::symlink;

    let (_temporary, home) = home();
    let workspace = home.join("src");
    fs::create_dir_all(&workspace).unwrap();
    let id = "11111111-2222-4333-8444-555555555555";
    let path = claude_path(&home, "project", id);
    write(
        &path,
        &format!("{}\n", json!({"sessionId": id, "cwd": workspace})),
    );
    let target = home.join("attachments");
    fs::create_dir_all(&target).unwrap();
    symlink(&target, path.with_extension("")).unwrap();
    let error = discover(&home, &workspace, Agent::Claude, Some(id)).unwrap_err();
    assert!(error.contains("not a safe directory"));
}
