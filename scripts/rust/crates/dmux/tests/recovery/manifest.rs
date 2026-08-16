use std::fs;

use dmux::model::{BackendInstanceUid, ServerEpoch, SpaceUid};
use dmux::recovery::{
    MANIFEST_SCHEMA_VERSION, ManifestGroup, ManifestSpace, ManifestSplit, ManifestWindow,
    NativePane, NativeSnapshot, NativeTab, NativeWindow, RecoveryManifest, RestoreOperation,
    newest_eligible_manifest,
};
use serde_json::json;
use uuid::Uuid;

fn instance() -> BackendInstanceUid {
    BackendInstanceUid(Uuid::from_u128(1))
}

fn space_uid(n: u128) -> SpaceUid {
    SpaceUid(Uuid::from_u128(n))
}

fn split(cwd: &str) -> ManifestSplit {
    ManifestSplit {
        cwd: cwd.into(),
        domain: Some("local".into()),
        text: None,
        process: None,
        is_active: false,
        is_zoomed: false,
        left: Some(0),
        top: Some(0),
        width: Some(80),
        height: Some(24),
        right: None,
        bottom: None,
    }
}

fn manifest(revision: u64) -> RecoveryManifest {
    let mut root = split("/one");
    root.right = Some(Box::new(split("/right")));
    root.bottom = Some(Box::new(split("/bottom")));
    RecoveryManifest {
        schema_version: MANIFEST_SCHEMA_VERSION,
        state: "complete".into(),
        manifest_id: format!("manifest-{revision}"),
        backend_instance_uid: instance(),
        registry_revision: revision,
        generated_at: format!("2026-08-16T12:00:{revision:02}Z"),
        spaces: vec![ManifestSpace {
            space_uid: space_uid(11),
            space_no: 2,
            opaque_key: "dmux-space-opaque".into(),
            logical_name: "dotfiles".into(),
            window_state: ManifestWindow {
                title: "window".into(),
                size: None,
                tabs: vec![
                    ManifestGroup {
                        title: "editor".into(),
                        is_active: true,
                        is_zoomed: false,
                        pane_tree: root,
                    },
                    ManifestGroup {
                        title: "shell".into(),
                        is_active: false,
                        is_zoomed: false,
                        pane_tree: split("/two"),
                    },
                ],
            },
        }],
    }
}

#[test]
fn manifest_flattens_to_stable_space_group_split_paths() {
    let manifest = manifest(7);
    manifest.validate(instance()).unwrap();
    let nodes = manifest.restore_nodes();
    assert_eq!(nodes.len(), 4);
    assert_eq!(nodes[0].operation, RestoreOperation::SpaceRoot);
    assert_eq!(nodes[1].operation, RestoreOperation::Split);
    assert_eq!(nodes[2].operation, RestoreOperation::Split);
    assert_eq!(nodes[3].operation, RestoreOperation::GroupRoot);
    assert_eq!(
        nodes
            .iter()
            .map(|node| node.manifest_node_path.as_str())
            .collect::<Vec<_>>(),
        vec![
            format!("/spaces/{}/groups/1/splits/L", space_uid(11).0),
            format!("/spaces/{}/groups/1/splits/LB", space_uid(11).0),
            format!("/spaces/{}/groups/1/splits/LR", space_uid(11).0),
            format!("/spaces/{}/groups/2/splits/L", space_uid(11).0),
        ]
    );
    assert_eq!(
        nodes[1].parent_path.as_deref(),
        Some(nodes[0].manifest_node_path.as_str())
    );
    assert_eq!(
        nodes[2].parent_path.as_deref(),
        Some(nodes[0].manifest_node_path.as_str())
    );
}

#[test]
fn restore_node_wire_shape_matches_the_lua_prepare_execute_contract() {
    let node = manifest(7).restore_nodes()[1].clone();
    assert_eq!(
        serde_json::to_value(node).unwrap(),
        json!({
            "manifest_node_path": "/spaces/00000000-0000-0000-0000-00000000000b/groups/1/splits/LB",
            "space_uid": "00000000-0000-0000-0000-00000000000b",
            "space_no": 2,
            "opaque_key": "dmux-space-opaque",
            "logical_name": "dotfiles",
            "group_index": 1,
            "operation": "split",
            "parent_path": "/spaces/00000000-0000-0000-0000-00000000000b/groups/1/splits/L",
            "direction": "Bottom",
            "cwd": "/bottom",
            "window_title": "window",
            "group_title": "editor",
            "group_is_active": true,
            "text": null,
            "process": null,
            "is_active": false,
            "is_zoomed": false,
            "width": 80,
            "height": 24
        })
    );
}

#[test]
fn deep_manifest_uses_guillotine_cuts_then_structural_right_before_bottom() {
    let mut manifest = manifest(7);
    let root = &mut manifest.spaces[0].window_state.tabs[0].pane_tree;
    root.right.as_mut().unwrap().right = Some(Box::new(split("/right/right")));
    root.bottom.as_mut().unwrap().right = Some(Box::new(split("/bottom/right")));
    let paths = manifest
        .restore_nodes()
        .into_iter()
        .take(5)
        .map(|node| node.manifest_node_path)
        .collect::<Vec<_>>();
    let base = format!("/spaces/{}/groups/1/splits/L", space_uid(11).0);
    assert_eq!(
        paths,
        vec![
            base.clone(),
            format!("{base}B"),
            format!("{base}R"),
            format!("{base}RR"),
            format!("{base}BR"),
        ]
    );
}

#[test]
fn imported_remote_panes_are_never_eligible_local_shells() {
    let mut manifest = manifest(7);
    manifest.spaces[0].window_state.tabs[0].pane_tree.domain = Some("SSHMUX:archie".into());
    let err = manifest.validate(instance()).unwrap_err();
    assert!(err.to_string().contains("imported domain"), "{err}");
}

#[test]
fn newest_complete_manifest_falls_back_around_corruption_and_honors_empty_floor() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("newest.json"), b"{not json").unwrap();
    fs::write(
        dir.path().join("rev-8.json"),
        serde_json::to_vec(&manifest(8)).unwrap(),
    )
    .unwrap();
    fs::write(
        dir.path().join("rev-9.json.bak"),
        serde_json::to_vec(&manifest(9)).unwrap(),
    )
    .unwrap();

    let (selected, diagnostics) =
        newest_eligible_manifest(dir.path(), instance(), Some(8)).unwrap();
    assert_eq!(selected.unwrap().registry_revision, 9);
    assert!(diagnostics.iter().any(|line| line.contains("newest.json")));
    assert!(
        diagnostics
            .iter()
            .any(|line| line.contains("at/below intentional-empty"))
    );

    let (selected, _) = newest_eligible_manifest(dir.path(), instance(), Some(9)).unwrap();
    assert!(selected.is_none());
}

#[test]
fn newest_manifest_with_bad_dimensions_is_skipped_for_older_complete_one() {
    let dir = tempfile::tempdir().unwrap();
    let good = manifest(7);
    let mut bad = manifest(8);
    bad.spaces[0].window_state.tabs[0].pane_tree.width = Some(0);
    fs::write(
        dir.path().join("good.json"),
        serde_json::to_vec(&good).unwrap(),
    )
    .unwrap();
    fs::write(
        dir.path().join("bad.json"),
        serde_json::to_vec(&bad).unwrap(),
    )
    .unwrap();

    let (selected, diagnostics) = newest_eligible_manifest(dir.path(), instance(), None).unwrap();
    assert_eq!(selected.unwrap().registry_revision, 7);
    assert!(diagnostics.iter().any(|line| line.contains("dimensions")));
}

fn sentinel_snapshot(epoch: ServerEpoch) -> NativeSnapshot {
    NativeSnapshot {
        complete: true,
        server_epoch: epoch,
        windows: vec![NativeWindow {
            window_id: "1".into(),
            workspace: format!("dmux:system:{}", epoch.0),
            tabs: vec![NativeTab {
                tab_id: "2".into(),
                panes: vec![NativePane {
                    pane_id: "3".into(),
                    title: "sentinel".into(),
                    domain: Some("local".into()),
                }],
            }],
        }],
    }
}

#[test]
fn exactly_one_sentinel_and_no_default_shell_is_recovery_empty() {
    let epoch = ServerEpoch(Uuid::from_u128(99));
    let snapshot = sentinel_snapshot(epoch);
    let witness = snapshot.require_sentinel_only(epoch).unwrap();
    assert_eq!(witness.pane_id, "3");

    let mut nonempty = snapshot.clone();
    nonempty.windows.push(NativeWindow {
        window_id: "4".into(),
        workspace: "default".into(),
        tabs: vec![NativeTab {
            tab_id: "5".into(),
            panes: vec![NativePane {
                pane_id: "6".into(),
                title: "shell".into(),
                domain: Some("local".into()),
            }],
        }],
    });
    let err = nonempty.require_sentinel_only(epoch).unwrap_err();
    assert_eq!(err.stable_code(), "recovery_ineligible");
}
