use super::*;
use std::fs;

fn snapshot(body: &str) -> Snapshot {
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(body.as_bytes()).unwrap();
    Snapshot::from_temporary(file).unwrap()
}

#[test]
fn transformer_maps_only_structural_codex_paths() {
    let source = snapshot(
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"child\",\"cwd\":\"/Users/f/work\",\"output\":\"see /Users/f/work\",\"workspace_roots\":[\"/Users/f/work\"]}}\n",
    );
    let (mapped, _, _) = transform_snapshot(
        source,
        Agent::Codex,
        Path::new("/Users/f"),
        Path::new("/home/f"),
        None,
    )
    .unwrap();
    let value: Value = serde_json::from_slice(&fs::read(mapped.path()).unwrap()).unwrap();
    assert_eq!(value.pointer("/payload/cwd").unwrap(), "/home/f/work");
    assert_eq!(
        value.pointer("/payload/workspace_roots/0").unwrap(),
        "/home/f/work"
    );
    assert_eq!(
        value.pointer("/payload/output").unwrap(),
        "see /Users/f/work"
    );
}

#[test]
fn structural_path_transform_is_byte_exact_when_reversed() {
    let body = "{\"type\":\"session_meta\",\"payload\":{\"id\":\"child\",\"cwd\":\"/Users/f/work\",\"thread_source\":\"user\",\"source\":\"cli\",\"workspace_roots\":[\"/Users/f/work\"]}}\n{\"type\":\"event_msg\",\"payload\":{\"message\":\"leave /Users/f/work unchanged here\"}}\n";
    let (mapped, _, _) = transform_snapshot(
        snapshot(body),
        Agent::Codex,
        Path::new("/Users/f"),
        Path::new("/home/f"),
        None,
    )
    .unwrap();
    let (returned, _, _) = transform_snapshot(
        mapped,
        Agent::Codex,
        Path::new("/home/f"),
        Path::new("/Users/f"),
        None,
    )
    .unwrap();
    assert_eq!(fs::read(returned.path()).unwrap(), body.as_bytes());
}

#[test]
fn child_history_offset_tracks_transformed_parent_boundary() {
    let parent_body = "{\"type\":\"session_meta\",\"ordinal\":0,\"payload\":{\"id\":\"parent\",\"cwd\":\"/Users/f/work\"}}\n{\"type\":\"event\",\"ordinal\":1}\n";
    let original_offset = parent_body.len() as u64;
    let (_, boundaries, _) = transform_snapshot(
        snapshot(parent_body),
        Agent::Codex,
        Path::new("/Users/f"),
        Path::new("/home/f"),
        None,
    )
    .unwrap();
    let mut child = serde_json::json!({
        "type": "session_meta",
        "ordinal": 2,
        "payload": {
            "id": "child",
            "cwd": "/Users/f/work",
            "history_base": {
                "thread_id": "parent",
                "end_ordinal_exclusive": 2,
                "end_byte_offset": original_offset,
            }
        }
    })
    .to_string();
    child.push('\n');
    let (mapped, _, base) = transform_snapshot(
        snapshot(&child),
        Agent::Codex,
        Path::new("/Users/f"),
        Path::new("/home/f"),
        Some(&boundaries),
    )
    .unwrap();
    let expected = boundaries[&original_offset].destination_offset;
    assert_eq!(base.unwrap().end_byte_offset, expected);
    let value: Value = serde_json::from_slice(&fs::read(mapped.path()).unwrap()).unwrap();
    assert_eq!(
        value
            .pointer("/payload/history_base/end_byte_offset")
            .and_then(Value::as_u64),
        Some(expected)
    );
}

#[test]
fn non_boundary_history_offset_is_refused() {
    let (_, boundaries, _) = transform_snapshot(
        snapshot("{\"type\":\"session_meta\",\"payload\":{\"id\":\"parent\",\"cwd\":\"/a\"}}\n"),
        Agent::Codex,
        Path::new("/a"),
        Path::new("/b"),
        None,
    )
    .unwrap();
    let child = snapshot(
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"child\",\"cwd\":\"/a\",\"history_base\":{\"thread_id\":\"parent\",\"end_ordinal_exclusive\":1,\"end_byte_offset\":3}}}\n",
    );
    assert!(
        transform_snapshot(
            child,
            Agent::Codex,
            Path::new("/a"),
            Path::new("/b"),
            Some(&boundaries)
        )
        .err()
        .unwrap()
        .contains("not a JSONL record boundary")
    );
}

#[test]
fn discovery_resolves_complete_codex_ancestry_root_first() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path();
    let store = home.join(".codex/sessions/2026/09/02");
    fs::create_dir_all(&store).unwrap();
    let archive = home.join(".codex/archived_sessions");
    fs::create_dir_all(&archive).unwrap();
    let parent_id = "01999999-1111-7222-8333-444444444444";
    let child_id = "01999999-1111-7222-8333-555555555555";
    let workspace = home.join("project");
    let parent_body = format!(
        "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{parent_id}\",\"cwd\":\"{}\",\"thread_source\":\"user\",\"source\":\"cli\"}}}}\n{{\"type\":\"event_msg\",\"payload\":{{}}}}\n",
        workspace.display()
    );
    let parent_path = archive.join(format!("rollout-{parent_id}.jsonl"));
    fs::write(&parent_path, &parent_body).unwrap();
    let child_body = serde_json::json!({
        "type": "session_meta",
        "payload": {
            "id": child_id,
            "cwd": workspace,
            "thread_source": "user",
            "source": "cli",
            "history_base": {
                "thread_id": parent_id,
                "end_ordinal_exclusive": 2,
                "end_byte_offset": parent_body.len(),
            }
        }
    });
    let child_path = store.join(format!("rollout-{child_id}.jsonl"));
    fs::write(&child_path, format!("{child_body}\n")).unwrap();
    let session = Session {
        agent: Agent::Codex,
        id: SessionId::new(child_id).unwrap(),
        transcript: child_path,
        companion: None,
        workspace,
    };
    let lineage = Lineage::discover(home, &session).unwrap();
    assert_eq!(
        lineage
            .artifacts
            .iter()
            .map(|artifact| artifact.descriptor.session_id.as_str())
            .collect::<Vec<_>>(),
        vec![parent_id, child_id]
    );
}

#[test]
fn projected_offset_beyond_snapshot_is_refused_before_launch() {
    let artifact = Artifact {
        descriptor: ArtifactDescriptor {
            session_id: "thread".to_string(),
            workspace: "/home/f/work".into(),
            transcript: "/home/f/.codex/sessions/thread.jsonl".into(),
            history_base: None,
            bytes: 99,
            sha256: "0".repeat(64),
        },
        snapshot: snapshot("{\"ordinal\":0}\n"),
    };
    let projections = HashMap::from([(
        "thread".to_string(),
        Projection {
            byte_offset: 100,
            next_ordinal: 1,
        },
    )]);
    let error = validate_projections(&[artifact], &projections).unwrap_err();
    assert!(error.contains("expects byte offset 100"));
    assert!(error.contains("only 99 bytes"));
}

#[test]
fn paginated_ordinals_are_normalized_without_touching_legacy_rollouts() {
    let duplicate = "{\"type\":\"session_meta\",\"ordinal\":0,\"payload\":{\"id\":\"root\",\"cwd\":\"/home/f/work\"}}\n{\"type\":\"event_msg\",\"ordinal\":0,\"payload\":{}}\n{\"type\":\"event_msg\",\"ordinal\":1,\"payload\":{}}\n";
    let (normalized, boundaries, _) = transform_snapshot(
        snapshot(duplicate),
        Agent::Codex,
        Path::new("/home/f"),
        Path::new("/home/f"),
        None,
    )
    .unwrap();
    let ordinals = fs::read_to_string(normalized.path())
        .unwrap()
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).unwrap()["ordinal"]
                .as_u64()
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(ordinals, [0, 1, 2]);
    assert_eq!(boundaries[&(duplicate.len() as u64)].next_ordinal, 3);

    let legacy = "{\"type\":\"session_meta\",\"payload\":{\"id\":\"legacy\",\"cwd\":\"/home/f/work\"}}\n{\"type\":\"event_msg\",\"payload\":{}}\n";
    let (unchanged, _, _) = transform_snapshot(
        snapshot(legacy),
        Agent::Codex,
        Path::new("/home/f"),
        Path::new("/home/f"),
        None,
    )
    .unwrap();
    assert_eq!(fs::read(unchanged.path()).unwrap(), legacy.as_bytes());
}

#[test]
fn child_ordinals_continue_from_the_transformed_history_boundary() {
    let parent = "{\"type\":\"session_meta\",\"ordinal\":0,\"payload\":{\"id\":\"parent\",\"cwd\":\"/Users/f/work\"}}\n{\"type\":\"event_msg\",\"ordinal\":0,\"payload\":{}}\n";
    let (_, parent_boundaries, _) = transform_snapshot(
        snapshot(parent),
        Agent::Codex,
        Path::new("/Users/f"),
        Path::new("/home/f"),
        None,
    )
    .unwrap();
    let child = format!(
        "{}\n{{\"type\":\"event_msg\",\"ordinal\":1,\"payload\":{{}}}}\n",
        serde_json::json!({
            "type": "session_meta",
            "ordinal": 1,
            "payload": {
                "id": "child",
                "cwd": "/Users/f/work",
                "history_base": {
                    "thread_id": "parent",
                    "end_ordinal_exclusive": 1,
                    "end_byte_offset": parent.len(),
                }
            }
        })
    );
    let (normalized, _, base) = transform_snapshot(
        snapshot(&child),
        Agent::Codex,
        Path::new("/Users/f"),
        Path::new("/home/f"),
        Some(&parent_boundaries),
    )
    .unwrap();
    let base = base.unwrap();
    assert_eq!(base.end_ordinal_exclusive, 2);
    let ordinals = fs::read_to_string(normalized.path())
        .unwrap()
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).unwrap()["ordinal"]
                .as_u64()
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(ordinals, [2, 3]);
}

#[test]
fn ancestry_ordinals_are_global_across_more_than_one_fork() {
    let root_body = "{\"ordinal\":0}\n{\"ordinal\":1}\n";
    let child_body = "{\"ordinal\":2}\n{\"ordinal\":3}\n";
    let grandchild_body = "{\"ordinal\":4}\n";
    let root = snapshot(root_body);
    let child = snapshot(child_body);
    let grandchild = snapshot(grandchild_body);
    let artifacts = vec![
        Artifact {
            descriptor: ArtifactDescriptor {
                session_id: "root".to_string(),
                workspace: "/home/f/work".into(),
                transcript: "/home/f/root.jsonl".into(),
                history_base: None,
                bytes: root_body.len() as u64,
                sha256: "a".repeat(64),
            },
            snapshot: root,
        },
        Artifact {
            descriptor: ArtifactDescriptor {
                session_id: "child".to_string(),
                workspace: "/home/f/work".into(),
                transcript: "/home/f/child.jsonl".into(),
                history_base: Some(HistoryBase {
                    thread_id: "root".to_string(),
                    end_ordinal_exclusive: 2,
                    end_byte_offset: root_body.len() as u64,
                }),
                bytes: child_body.len() as u64,
                sha256: "b".repeat(64),
            },
            snapshot: child,
        },
        Artifact {
            descriptor: ArtifactDescriptor {
                session_id: "grandchild".to_string(),
                workspace: "/home/f/work".into(),
                transcript: "/home/f/grandchild.jsonl".into(),
                history_base: Some(HistoryBase {
                    thread_id: "child".to_string(),
                    end_ordinal_exclusive: 4,
                    end_byte_offset: child_body.len() as u64,
                }),
                bytes: grandchild_body.len() as u64,
                sha256: "c".repeat(64),
            },
            snapshot: grandchild,
        },
    ];
    validate_ancestry(&artifacts).unwrap();
}

#[test]
fn projection_offset_and_ordinal_must_resolve_to_the_same_boundary() {
    let body = "{\"ordinal\":0}\n{\"ordinal\":1}\n";
    let artifact = Artifact {
        descriptor: ArtifactDescriptor {
            session_id: "thread".to_string(),
            workspace: "/home/f/work".into(),
            transcript: "/home/f/thread.jsonl".into(),
            history_base: None,
            bytes: body.len() as u64,
            sha256: "a".repeat(64),
        },
        snapshot: snapshot(body),
    };
    let projections = HashMap::from([(
        "thread".to_string(),
        Projection {
            byte_offset: "{\"ordinal\":0}\n".len() as u64,
            next_ordinal: 2,
        },
    )]);
    let error = validate_projections(&[artifact], &projections).unwrap_err();
    assert!(error.contains("expects ordinal 2"));
    assert!(error.contains("resolves to ordinal 1"));
}
