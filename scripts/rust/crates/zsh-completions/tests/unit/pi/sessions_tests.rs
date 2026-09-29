use super::*;
use testkit::tree;

fn session(id: &str, cwd: &str, lines: &[&str]) -> String {
    let mut text = format!(
        "{{\"type\":\"session\",\"version\":3,\"id\":\"{id}\",\"timestamp\":\"2026-09-23T14:46:23.322Z\",\"cwd\":\"{cwd}\"}}\n"
    );
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
    text
}

const SYSTEM: &str = r#"{"type":"message","message":{"role":"system","content":"You are"}}"#;
const ASK: &str = r#"{"type":"message","message":{"role":"user","content":[{"type":"text","text":"Fix the\nflaky test"}]}}"#;
const NAMED: &str = r#"{"type":"session_info","name":"first name"}"#;
const RENAMED: &str = r#"{"type":"session_info","name":" final name "}"#;

#[test]
fn the_default_directory_encodes_the_working_directory() {
    let dir = default_directory(Path::new("/h/.pi/agent"), Path::new("/home/me/dotfiles"));
    assert_eq!(
        dir,
        PathBuf::from("/h/.pi/agent/sessions/--home-me-dotfiles--")
    );
}

#[test]
fn a_summary_takes_the_latest_name_and_the_first_request() {
    let root = tree(&[&format!(
        "s.jsonl={}",
        session("abc", "/w", &[SYSTEM, NAMED, ASK, RENAMED])
    )]);
    let summary = summarize(&root.path().join("s.jsonl")).expect("a session");
    assert_eq!(summary.id, "abc");
    assert_eq!(summary.cwd, "/w");
    assert_eq!(summary.name.as_deref(), Some("final name"));
    assert_eq!(summary.first_message.as_deref(), Some("Fix the flaky test"));
}

#[test]
fn files_without_a_session_header_are_skipped() {
    let root = tree(&["other.jsonl={\"type\":\"message\"}\n", "empty.jsonl="]);
    assert!(summarize(&root.path().join("other.jsonl")).is_none());
    assert!(summarize(&root.path().join("empty.jsonl")).is_none());
}

#[test]
fn a_shared_directory_only_lists_sessions_from_this_directory() {
    let root = tree(&[
        &format!("dir/a.jsonl={}", session("mine", "/work", &[ASK])),
        &format!("dir/b.jsonl={}", session("theirs", "/elsewhere", &[])),
        "dir/notes.txt=ignored",
    ]);
    let ctx = Context::testing(root.path(), Path::new("/work"), &[]);
    let dir = root.path().join("dir");
    let ids = |filter| {
        list(&ctx, &dir, filter)
            .into_iter()
            .map(|s| s.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(true), ["mine"]);
    let mut all = ids(false);
    all.sort();
    assert_eq!(all, ["mine", "theirs"]);
}

#[test]
fn summaries_are_reused_until_a_file_changes() {
    let root = tree(&[&format!("dir/a.jsonl={}", session("one", "/w", &[ASK]))]);
    let ctx = Context::testing(root.path(), Path::new("/w"), &[]);
    let dir = root.path().join("dir");
    assert_eq!(list(&ctx, &dir, false)[0].name, None);
    fs::write(dir.join("a.jsonl"), session("one", "/w", &[ASK, NAMED])).unwrap();
    assert_eq!(
        list(&ctx, &dir, false)[0].name.as_deref(),
        Some("first name")
    );
}

#[test]
fn the_session_directory_follows_pi_s_precedence() {
    let root = tree(&["agent/settings.json={\"sessionDir\":\"~/from-settings\"}"]);
    let pi = Pi {
        binary: None,
        package: None,
        agent_dir: root.path().join("agent"),
    };
    let cwd = Path::new("/work");
    let ctx = Context::testing(root.path(), cwd, &[]);
    assert_eq!(
        directory(&ctx, &pi, Some("/flag")),
        (PathBuf::from("/flag"), true)
    );
    assert_eq!(
        directory(&ctx, &pi, None),
        (root.path().join("from-settings"), true)
    );
    let env = Context::testing(root.path(), cwd, &[("PI_CODING_AGENT_SESSION_DIR", "/env")]);
    assert_eq!(directory(&env, &pi, None).0, PathBuf::from("/env"));
    let plain = Pi {
        agent_dir: root.path().join("none"),
        ..pi
    };
    assert_eq!(
        directory(&ctx, &plain, None),
        (default_directory(&plain.agent_dir, cwd), false)
    );
}
