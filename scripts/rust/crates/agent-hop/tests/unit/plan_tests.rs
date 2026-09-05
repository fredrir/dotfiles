use super::*;

#[test]
fn codex_keeps_the_transcript_path_below_its_state_directory() {
    let found = destination(
        Agent::Codex,
        "id",
        Path::new("/Users/f/.codex/sessions/2026/09/02/rollout-id.jsonl"),
        Path::new("/Users/f/projects/app"),
        Path::new("/Users/f"),
        Path::new("/home/f"),
        false,
    )
    .unwrap();
    assert_eq!(found.workspace, Path::new("/home/f/projects/app"));
    assert_eq!(
        found.transcript,
        Path::new("/home/f/.codex/sessions/2026/09/02/rollout-id.jsonl")
    );
    assert!(found.companion.is_none());
}

#[test]
fn claude_uses_the_destination_workspace_for_its_project_key() {
    let found = destination(
        Agent::Claude,
        "session-id",
        Path::new("/Users/f/.claude/projects/-Users-f-app/session-id.jsonl"),
        Path::new("/Users/f/my app/.work"),
        Path::new("/Users/f"),
        Path::new("/home/f"),
        true,
    )
    .unwrap();
    assert_eq!(found.workspace, Path::new("/home/f/my app/.work"));
    assert_eq!(
        found.transcript,
        Path::new("/home/f/.claude/projects/-home-f-my-app--work/session-id.jsonl")
    );
    assert_eq!(
        found.companion.as_deref(),
        Some(Path::new(
            "/home/f/.claude/projects/-home-f-my-app--work/session-id"
        ))
    );
}

#[test]
fn a_home_workspace_maps_to_the_other_home() {
    let found = destination(
        Agent::Claude,
        "id",
        Path::new("/Users/f/.claude/projects/-Users-f/id.jsonl"),
        Path::new("/Users/f"),
        Path::new("/Users/f"),
        Path::new("/home/f"),
        false,
    )
    .unwrap();
    assert_eq!(found.workspace, Path::new("/home/f"));
}

#[test]
fn a_codex_file_outside_its_state_root_is_rejected() {
    assert!(
        destination(
            Agent::Codex,
            "id",
            Path::new("/Users/f/session.jsonl"),
            Path::new("/Users/f/project"),
            Path::new("/Users/f"),
            Path::new("/home/f"),
            false,
        )
        .is_err()
    );
}
