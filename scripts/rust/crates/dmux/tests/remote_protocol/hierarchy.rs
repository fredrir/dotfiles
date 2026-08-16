//! P8b remote hierarchy conformance, local direct-argv leg on a scratch
//! tmux server: hierarchy read, group/split creation through the REAL
//! pane-bootstrap helper (env proven in the exec'd program), envelope
//! replay (no second window/pane), stale-epoch child refs, cascade and
//! unstamped-adopted refusals, and payload validation.

use std::path::Path;
use std::time::{Duration, Instant};

use dmux::backend::InventoryScope;
use dmux::backend::tmux::TmuxProvider;
use dmux::model::{Backend, ChildKind, ServerEpoch};
use dmux::operations::{CreatedChild, CreatedSpace, RemovedChild, SpaceHierarchy};
use dmux::remote::protocol::{self};
use serde_json::json;
use uuid::Uuid;

use crate::util::{Scratch, envelope, error_code};

/// The exec'd program writes its marker env to `marker` then parks.
fn marker_program(marker: &Path) -> Vec<String> {
    vec![
        "sh".into(),
        "-c".into(),
        format!(
            "printf %s \"$DMUX_GROUP_REF|$DMUX_SPLIT_REF|$DMUX_SPACE_UID\" > {} \
             && exec sleep 300",
            marker.display()
        ),
    ]
}

fn wait_marker(marker: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(text) = std::fs::read_to_string(marker)
            && !text.is_empty()
        {
            return text;
        }
        assert!(Instant::now() < deadline, "helper never exec'd the program");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn create(scratch: &Scratch, name: &str) -> CreatedSpace {
    let request = envelope(
        protocol::methods::NEW,
        Uuid::new_v4(),
        json!({ "name": name, "backend": "tmux", "program": ["sleep", "300"] }),
    );
    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 0, "{response:?}");
    serde_json::from_value(response.payload.unwrap()).unwrap()
}

fn tree(scratch: &Scratch, space_uid: dmux::model::SpaceUid) -> SpaceHierarchy {
    let request = envelope(
        protocol::methods::HIERARCHY,
        Uuid::new_v4(),
        json!({ "space_uid": space_uid }),
    );
    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 0, "{response:?}");
    serde_json::from_value(response.payload.unwrap()).unwrap()
}

fn window_count(scratch: &Scratch, session: &str) -> usize {
    String::from_utf8_lossy(
        &scratch
            .tmux(&["list-windows", "-t", session, "-F", "#{window_id}"])
            .stdout,
    )
    .lines()
    .count()
}

fn pane_count(scratch: &Scratch, session: &str) -> usize {
    String::from_utf8_lossy(
        &scratch
            .tmux(&["list-panes", "-s", "-t", session, "-F", "#{pane_id}"])
            .stdout,
    )
    .lines()
    .count()
}

#[test]
fn remote_hierarchy_full_cycle_with_replay_and_markers() {
    let scratch = Scratch::with_tmux("rhier");
    let created = create(&scratch, "proj");

    // --- hierarchy read: one group, one split, live epoch.
    let t = tree(&scratch, created.space_uid);
    assert_eq!(t.groups.len(), 1);
    assert_eq!(t.groups[0].splits.len(), 1);
    assert_eq!(t.groups[0].group_ref, created.group_ref);

    // --- group_new through the real helper; marker env proven.
    let gmark = scratch.data.path().join("gmark");
    let group_request = envelope(
        protocol::methods::GROUP_NEW,
        Uuid::new_v4(),
        json!({
            "space_uid": created.space_uid,
            "cwd": "/tmp",
            "program": marker_program(&gmark),
        }),
    );
    let (code, response) = scratch.agent(&group_request);
    assert_eq!(code, 0, "{response:?}");
    assert!(response.backend_instance_uid.is_some() && response.server_epoch.is_some());
    let group: CreatedChild = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert_eq!(group.kind, ChildKind::Group);
    assert_ne!(group.group_ref, created.group_ref);
    let stamped = wait_marker(&gmark);
    assert_eq!(
        stamped,
        format!(
            "{}|{}|{}",
            group.group_ref, group.split_ref, created.space_uid.0
        ),
        "marker propagation through the remote protocol"
    );
    assert_eq!(window_count(&scratch, "proj"), 2);

    // --- replay the identical envelope: no second window.
    let (code, response) = scratch.agent(&group_request);
    assert_eq!(code, 0, "{response:?}");
    let replayed: CreatedChild = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert!(replayed.replayed);
    assert_eq!(replayed.group_ref, group.group_ref);
    assert_eq!(window_count(&scratch, "proj"), 2, "replay must not spawn");

    // --- split_new with placement into the new group, then replay.
    let smark = scratch.data.path().join("smark");
    let split_request = envelope(
        protocol::methods::SPLIT_NEW,
        Uuid::new_v4(),
        json!({
            "space_uid": created.space_uid,
            "group_ref": group.group_ref,
            "direction": "right",
            "percent": 30,
            "program": marker_program(&smark),
        }),
    );
    let (code, response) = scratch.agent(&split_request);
    assert_eq!(code, 0, "{response:?}");
    let split: CreatedChild = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert_eq!(split.kind, ChildKind::Split);
    assert_eq!(split.group_ref, group.group_ref);
    wait_marker(&smark);
    assert_eq!(pane_count(&scratch, "proj"), 3);
    let (code, response) = scratch.agent(&split_request);
    assert_eq!(code, 0, "{response:?}");
    let replayed: CreatedChild = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert!(replayed.replayed);
    assert_eq!(pane_count(&scratch, "proj"), 3, "replay must not split");

    // --- group_rename is presentation-only and visible in the tree.
    let rename_request = envelope(
        protocol::methods::GROUP_RENAME,
        Uuid::new_v4(),
        json!({
            "space_uid": created.space_uid,
            "group_ref": group.group_ref,
            "title": "editor",
        }),
    );
    let (code, response) = scratch.agent(&rename_request);
    assert_eq!(code, 0, "{response:?}");
    let t = tree(&scratch, created.space_uid);
    assert_eq!(
        t.groups
            .iter()
            .find(|g| g.group_ref == group.group_ref)
            .unwrap()
            .title
            .as_deref(),
        Some("editor")
    );

    // --- removes: split, then group; the LAST group refuses (cascade).
    let (code, response) = scratch.agent(&envelope(
        protocol::methods::SPLIT_RM,
        Uuid::new_v4(),
        json!({ "space_uid": created.space_uid, "split_ref": split.split_ref }),
    ));
    assert_eq!(code, 0, "{response:?}");
    let removed: RemovedChild = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert_eq!(removed.kind, ChildKind::Split);
    let (code, _) = scratch.agent(&envelope(
        protocol::methods::GROUP_RM,
        Uuid::new_v4(),
        json!({ "space_uid": created.space_uid, "group_ref": group.group_ref }),
    ));
    assert_eq!(code, 0);
    assert_eq!(window_count(&scratch, "proj"), 1);

    let t = tree(&scratch, created.space_uid);
    let (code, response) = scratch.agent(&envelope(
        protocol::methods::GROUP_RM,
        Uuid::new_v4(),
        json!({ "space_uid": created.space_uid, "group_ref": t.groups[0].group_ref }),
    ));
    assert_eq!(code, 4, "last group cascade: {response:?}");
    assert_eq!(error_code(&response), "repair_required");
}

#[test]
fn stale_epoch_and_bad_payloads_are_typed_refusals() {
    let scratch = Scratch::with_tmux("rstale");
    let created = create(&scratch, "proj");

    // A child ref minted under a DIFFERENT epoch: typed refusal.
    let stale_epoch = ServerEpoch(Uuid::from_u128(7));
    let handle = created.group_ref.split_once('.').unwrap().1;
    let stale_ref = format!("g{}.{handle}", stale_epoch.0);
    let (code, response) = scratch.agent(&envelope(
        protocol::methods::SPLIT_NEW,
        Uuid::new_v4(),
        json!({ "space_uid": created.space_uid, "group_ref": stale_ref }),
    ));
    assert_eq!(code, 1, "{response:?}");
    assert_eq!(error_code(&response), "backend_epoch_changed");
    assert_eq!(
        String::from_utf8_lossy(
            &scratch
                .tmux(&["list-panes", "-s", "-t", "proj", "-F", "#{pane_id}"])
                .stdout
        )
        .lines()
        .count(),
        1,
        "stale-epoch refusal creates nothing"
    );

    // Malformed child ref / wrong kind / bad direction / bad percent.
    for (payload, expected) in [
        (
            json!({ "space_uid": created.space_uid, "group_ref": "not-a-ref" }),
            "invalid_ref",
        ),
        (
            json!({ "space_uid": created.space_uid, "group_ref": created.split_ref }),
            "invalid_ref",
        ),
        (
            json!({ "space_uid": created.space_uid, "group_ref": created.group_ref,
                    "direction": "sideways" }),
            "usage",
        ),
        (
            json!({ "space_uid": created.space_uid, "group_ref": created.group_ref,
                    "percent": 0 }),
            "usage",
        ),
    ] {
        let (_, response) = scratch.agent(&envelope(
            protocol::methods::SPLIT_NEW,
            Uuid::new_v4(),
            payload,
        ));
        assert_eq!(error_code(&response), expected, "{response:?}");
    }
}

#[test]
fn unstamped_adopted_space_refuses_child_mutations() {
    let scratch = Scratch::with_tmux("radopt");
    let _created = create(&scratch, "seed-managed");

    // Adopt an external session (owner-side setup, straight through the
    // operations layer): it lands active + unstamped.
    let out = scratch.tmux(&["new-session", "-d", "-s", "legacy"]);
    assert!(out.status.success());
    let sessions = String::from_utf8_lossy(
        &scratch
            .tmux(&["list-sessions", "-F", "#{session_id} #{session_name}"])
            .stdout,
    )
    .lines()
    .map(str::to_string)
    .collect::<Vec<_>>();
    let legacy_id = sessions
        .iter()
        .find(|l| l.ends_with("legacy"))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    let ns = scratch.ns.clone().unwrap();
    let provider = TmuxProvider::new(ns.clone());
    let mut registry = scratch.registry();
    // register_backend_instance is idempotent: it FINDS the bootstrapped
    // instance for (owner, tmux).
    let instance = registry
        .register_backend_instance(Backend::Tmux, None, None)
        .unwrap();
    let epoch = registry
        .backend_server(instance)
        .unwrap()
        .server_epoch
        .unwrap();
    drop(registry);
    let scope = InventoryScope {
        backend: Backend::Tmux,
        endpoint: ns,
        expected_epoch: Some(epoch),
    };
    let adopted = dmux::operations::adopt_tmux(
        &scratch.env(),
        &provider,
        &scope,
        &legacy_id,
        None,
        Uuid::new_v4(),
    )
    .unwrap();

    let (code, response) = scratch.agent(&envelope(
        protocol::methods::GROUP_NEW,
        Uuid::new_v4(),
        json!({ "space_uid": adopted.space_uid }),
    ));
    assert_eq!(code, 4, "{response:?}");
    assert_eq!(error_code(&response), "repair_required");
    assert!(
        response
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("acknowledges its marker"),
        "{response:?}"
    );
}
