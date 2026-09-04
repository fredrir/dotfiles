use super::*;

#[test]
fn manifest_updates_are_separate_immutable_files() {
    let home = tempfile::tempdir().unwrap();
    let installed = TransferManifest {
        schema_version: SCHEMA_VERSION,
        transfer_id: "transfer".to_string(),
        created_at_ms: 1,
        state: "installed".to_string(),
        agent: "codex".to_string(),
        parent_id: "parent".to_string(),
        child_id: None,
        mapping: HostMapping {
            source_host: "macie".to_string(),
            destination_host: "archie".to_string(),
            source_home: "/Users/f".into(),
            destination_home: "/home/f".into(),
            source_workspace: "/Users/f/work".into(),
            destination_workspace: "/home/f/work".into(),
        },
        artifacts: vec![ManifestArtifact {
            session_id: "parent".to_string(),
            parent_id: None,
            source_path: "/Users/f/source.jsonl".into(),
            destination_path: "/home/f/destination.jsonl".into(),
            source_sha256: "a".repeat(64),
            destination_sha256: "b".repeat(64),
            source_bytes: 10,
            destination_bytes: 11,
            source_history_offset: None,
            destination_history_offset: None,
            source_history_ordinal: None,
            destination_history_ordinal: None,
        }],
    };
    let first = record(home.path(), &installed).unwrap();
    let launched = installed.launched("child".to_string()).unwrap();
    let second = record(home.path(), &launched).unwrap();
    assert_ne!(first, second);
    assert!(first.exists());
    assert!(second.exists());
    assert_eq!(
        latest_child(
            home.path(),
            "macie",
            "archie",
            Agent::Codex,
            "parent",
            &"a".repeat(64)
        )
        .unwrap()
        .as_deref(),
        Some("child")
    );
}
