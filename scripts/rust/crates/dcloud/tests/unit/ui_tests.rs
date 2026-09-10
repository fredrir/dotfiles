use super::*;
use serde_json::json;

#[test]
fn human_catalog_is_a_table_without_embedded_raw_records() {
    let value = json!({"items":[{"kind":"archive","id":"archive-id","host":"macie","job":"Documents","destination":"drive","manifest":{"secret_metadata":"not displayed"}}],"errors":[]});
    let rendered = human(&value);
    assert!(rendered.contains("DESTINATION"));
    assert!(rendered.contains("archive-id"));
    assert!(!rendered.contains("secret_metadata"));
    assert!(!rendered.contains('{'));
}

#[test]
fn terminal_controls_and_bidirectional_overrides_are_not_rendered() {
    let value = json!({"error":"bad\u{1b}[31m\nname\u{202e}exe"});
    let rendered = human(&value);
    assert!(!rendered.contains('\u{1b}'));
    assert!(!rendered.contains('\u{202e}'));
    assert!(rendered.contains("bad�[31m�name�exe"));
}

#[test]
fn an_empty_catalog_never_opens_or_selects_the_synthetic_root() {
    assert_eq!(select(&[]).unwrap(), None);
}

#[test]
fn virtual_file_tree_does_not_follow_symlinks() {
    let tree = Tree {
        entries: vec![
            ("directory/file".into(), EntryKind::File),
            ("link".into(), EntryKind::Symlink),
        ],
    };
    let root = tree.read_directory(&PathBuf::new()).unwrap();
    assert_eq!(root.entries[0].kind, EntryKind::Directory);
    assert_eq!(root.entries[1].kind, EntryKind::Symlink);
    assert!(root.parent.is_none());
}

#[test]
fn status_summarizes_recorded_maintenance_without_dumping_provider_details() {
    let warning = "archive expiration deferred: googleapi: Error 403: Drive API disabled\nDetails:\n{\"provider_payload\":\"verbose server response\"}";
    let record = json!({"at":"2026-09-10T18:56:52Z","attempted":true,"result":{"warnings":[{"warning":warning}]}});
    let value = json!({
        "host":"macie","local_only":false,"observed_at":"2026-09-10T19:03:26Z",
        "items":[{"host":"archie","job":"Documents","destination":"drive","status":"verified","last_verified":"2026-09-10T19:03:26Z","last_full_restore":"2026-09-10T19:03:26Z"}],
        "hosts":[{"host":"macie","status":"local","maintenance":record}],
        "maintenance":record,"pending":[],"pending_cleanup":[],"sync":[],"errors":[]
    });
    let rendered = human(&value);
    assert!(rendered.contains("source journals"));
    assert!(rendered.contains("archie"));
    assert!(rendered.contains("verified"));
    assert!(rendered.contains("Maintenance (last recorded run)"));
    assert!(rendered.contains("macie: deferred"));
    assert!(rendered.contains("Error 403: Drive API disabled"));
    assert!(rendered.contains("dcloud status --json"));
    assert!(!rendered.contains("provider_payload"));
    assert!(!rendered.contains("2026-09-10T19:03:26Z"));
    assert_eq!(
        value["maintenance"]["result"]["warnings"][0]["warning"],
        warning
    );
}

#[test]
fn status_keeps_offline_source_diagnostics_and_marks_local_only_scope() {
    let value = json!({
        "host":"macie","local_only":true,"observed_at":"2026-09-10T19:03:26Z",
        "items":[],"hosts":[{"host":"archie","status":"unreachable","message":"SSH connection timed out"}],
        "pending":[],"pending_cleanup":[],"sync":[],"errors":["archie: SSH connection timed out"]
    });
    let rendered = human(&value);
    assert!(rendered.contains("local source journal"));
    assert_eq!(
        rendered.matches("archie: SSH connection timed out").count(),
        1
    );
    assert!(!rendered.contains("verified"));
}

#[test]
fn successful_maintenance_is_concise_and_has_no_diagnostic_warning() {
    let value = json!({
        "host":"macie","local_only":false,"observed_at":"2026-09-10T19:03:26Z",
        "items":[],"hosts":[{"host":"macie","status":"local","maintenance":{"at":"2026-09-10T19:03:26Z","result":{"source_cleanup":{"applied":true,"pending":[]},"warnings":[],"quarantine":[]}}}],
        "pending":[],"pending_cleanup":[],"sync":[],"errors":[]
    });
    let rendered = human(&value);
    assert!(rendered.contains("macie: ok"));
    assert!(!rendered.contains("--json"));
    assert!(!rendered.contains("applied"));
    assert!(!rendered.contains("warnings"));
}
