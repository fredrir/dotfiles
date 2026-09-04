use std::fs;
use std::time::{Duration, UNIX_EPOCH};

use serde_json::json;

use super::*;

fn write(path: &Path, content: impl AsRef<[u8]>) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn codex(home: &Path, day: &str, id: &str, workspace: &Path) -> PathBuf {
    let path = home.join(format!(
        ".codex/sessions/2026/09/{day}/rollout-2026-09-{day}T00-00-00-{id}.jsonl"
    ));
    write(
        &path,
        format!(
            "{}\n",
            json!({"type":"session_meta","payload":{"id":id,"cwd":workspace,"thread_source":"user","source":"cli"}})
        ),
    );
    path
}

fn claude(home: &Path, project: &str, id: &str, workspace: &Path) -> PathBuf {
    let path = home.join(format!(".claude/projects/{project}/{id}.jsonl"));
    write(
        &path,
        format!("{}\n", json!({"sessionId":id,"cwd":workspace})),
    );
    path
}

fn stores(home: &Path) {
    fs::create_dir_all(home.join(".codex/sessions/2026/09/01")).unwrap();
    fs::create_dir_all(home.join(".claude/projects/project")).unwrap();
}

#[test]
fn scans_both_stores_and_orders_newest_first_deterministically() {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("home");
    let current = home.join("current");
    let other = home.join("other");
    fs::create_dir_all(&current).unwrap();
    fs::create_dir_all(&other).unwrap();
    stores(&home);
    let old_current = codex(&home, "01", "codex-current", &current);
    let new_other = claude(&home, "project", "claude-other", &other);
    fs::File::options()
        .write(true)
        .open(&old_current)
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_secs(1))
        .unwrap();
    fs::File::options()
        .write(true)
        .open(&new_other)
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_secs(9))
        .unwrap();

    let catalog = scan(&home, &current);
    assert_eq!(catalog.sessions.len(), 2);
    assert_eq!(catalog.sessions[0].session.id.as_str(), "claude-other");
    assert!(!catalog.sessions[0].current_workspace);
    assert_eq!(catalog.sessions[1].session.id.as_str(), "codex-current");
    assert!(catalog.sessions[1].current_workspace);
    assert_eq!(catalog.sources.len(), 2);
}

#[test]
fn absent_optional_source_does_not_hide_the_other_source() {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("home");
    let workspace = home.join("work");
    fs::create_dir_all(&workspace).unwrap();
    claude(&home, "project", "only-claude", &workspace);

    let catalog = scan(&home, &workspace);
    assert_eq!(catalog.sessions.len(), 1);
    assert_eq!(catalog.sources[0].state, SourceState::Absent);
    assert_eq!(
        catalog.sources[1].state,
        SourceState::Available { sessions: 1 }
    );
}

#[test]
fn malformed_files_are_silently_ignored_and_valid_files_remain() {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("home");
    let workspace = home.join("work");
    fs::create_dir_all(&workspace).unwrap();
    stores(&home);
    write(
        &home.join(".codex/sessions/2026/09/01/rollout-broken.jsonl"),
        "{broken\n",
    );
    codex(&home, "01", "valid", &workspace);

    let catalog = scan(&home, &workspace);
    assert_eq!(catalog.sessions.len(), 1);
    assert!(catalog.diagnostics.is_empty());
}

#[test]
fn malformed_duplicates_disable_a_valid_id_without_becoming_issues() {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("home");
    let workspace = home.join("work");
    fs::create_dir_all(&workspace).unwrap();
    stores(&home);
    codex(&home, "01", "same-id", &workspace);
    let duplicate =
        home.join(".codex/sessions/2026/09/02/rollout-2026-09-02T00-00-00-same-id.jsonl");
    write(&duplicate, "not json\n");

    let catalog = scan(&home, &workspace);
    assert!(catalog.sessions.is_empty());
    assert!(catalog.diagnostics.is_empty());
    assert_eq!(
        catalog.sources[0].state,
        SourceState::Available { sessions: 0 }
    );
}

#[cfg(unix)]
#[test]
fn unsafe_duplicates_disable_a_valid_id_without_becoming_issues() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("home");
    let workspace = home.join("work");
    fs::create_dir_all(&workspace).unwrap();
    stores(&home);
    codex(&home, "01", "same-id", &workspace);
    let target = home.join("target.jsonl");
    write(&target, "not a transcript\n");
    let duplicate =
        home.join(".codex/sessions/2026/09/02/rollout-2026-09-02T00-00-00-same-id.jsonl");
    fs::create_dir_all(duplicate.parent().unwrap()).unwrap();
    symlink(&target, duplicate).unwrap();

    let catalog = scan(&home, &workspace);
    assert!(catalog.sessions.is_empty());
    assert!(catalog.diagnostics.is_empty());
    assert_eq!(
        catalog.sources[0].state,
        SourceState::Available { sessions: 0 }
    );
}

#[cfg(unix)]
#[test]
fn unsafe_entries_are_diagnostic_not_followed() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("home");
    stores(&home);
    let target = home.join("secret");
    write(&target, "secret");
    let link = home.join(".claude/projects/project/unsafe.jsonl");
    symlink(&target, &link).unwrap();

    let catalog = scan(&home, &home);
    assert!(catalog.sessions.is_empty());
    assert!(
        catalog
            .diagnostics
            .iter()
            .any(|item| item.kind == DiagnosticKind::Unsafe)
    );
}

#[test]
fn broken_store_is_distinct_from_an_absent_optional_store() {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("home");
    fs::create_dir_all(home.join(".codex")).unwrap();
    write(&home.join(".codex/sessions"), "not a directory");

    let catalog = scan(&home, &home);
    assert!(matches!(catalog.sources[0].state, SourceState::Disabled(_)));
    assert_eq!(catalog.sources[1].state, SourceState::Absent);
}

#[cfg(unix)]
#[test]
fn dangling_store_parent_is_broken_not_absent() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("home");
    fs::create_dir_all(&home).unwrap();
    symlink(home.join("missing-codex-root"), home.join(".codex")).unwrap();

    let catalog = scan(&home, &home);
    assert!(matches!(catalog.sources[0].state, SourceState::Disabled(_)));
    assert_eq!(catalog.sources[1].state, SourceState::Absent);
}

#[cfg(unix)]
#[test]
fn valid_symlinked_store_parent_can_have_an_absent_optional_child() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("home");
    let state = temporary.path().join("codex-state");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir(&state).unwrap();
    symlink(&state, home.join(".codex")).unwrap();

    let catalog = scan(&home, &home);
    assert_eq!(catalog.sources[0].state, SourceState::Absent);
    assert_eq!(catalog.sources[1].state, SourceState::Absent);
}

#[test]
fn diagnostic_summaries_keep_bounded_actionable_details() {
    let diagnostics = vec![
        CatalogDiagnostic {
            agent: Agent::Codex,
            kind: DiagnosticKind::ScanFailure,
            message: "could not read /work/session.jsonl: permission denied".to_owned(),
        },
        CatalogDiagnostic {
            agent: Agent::Codex,
            kind: DiagnosticKind::Unsafe,
            message: "ignored unsafe /work/link.jsonl".to_owned(),
        },
    ];
    let summary = diagnostic_summary(&diagnostics, Agent::Codex).unwrap();
    assert!(summary.contains("1 unsafe entry ignored, 1 read failure"));
    assert!(summary.contains("permission denied"));
    assert!(summary.contains("/work/link.jsonl"));
    assert!(diagnostic_summary(&diagnostics, Agent::Claude).is_none());
}

#[test]
fn project_labels_use_the_nearest_git_root_and_fallback_to_workspace() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("repo");
    let nested_repo = root.join("packages/app");
    let workspace = nested_repo.join("src/deep");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir(root.join(".git")).unwrap();
    fs::write(nested_repo.join(".git"), "gitdir: elsewhere").unwrap();

    assert_eq!(project_label(&workspace), "app");

    fs::remove_file(nested_repo.join(".git")).unwrap();
    assert_eq!(project_label(&workspace), "repo");

    fs::remove_dir(root.join(".git")).unwrap();
    assert_eq!(project_label(&workspace), "deep");
}
