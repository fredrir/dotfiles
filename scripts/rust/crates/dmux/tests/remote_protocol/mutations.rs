//! Bounded remote mutations against the real `_agent` on a scratch tmux
//! namespace: create with end-to-end request-UID idempotency (same
//! envelope twice → ONE session, identical replayed result), the
//! backend-instance/epoch verification matrix (stale claims refused,
//! nothing created), rename/rm with cross-invocation replay.

use std::time::{Duration, Instant};

use dmux::locks::{LockMode, LockScope, OrderedLocks};
use dmux::model::{Backend, BackendInstanceUid, ServerEpoch};
use dmux::operations::CreatedSpace;
use dmux::remote::protocol::{self, RenameResult, RmResult};
use serde_json::json;
use uuid::Uuid;

use crate::util::{Scratch, envelope, error_code};

fn new_payload(name: &str) -> serde_json::Value {
    json!({
        "name": name,
        "backend": "tmux",
        "program": ["sleep", "300"],
    })
}

#[test]
fn create_is_idempotent_by_envelope_request_uid() {
    let scratch = Scratch::with_tmux("create");
    let request = envelope(protocol::methods::NEW, Uuid::new_v4(), new_payload("proj"));
    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 0, "{response:?}");
    // Mutation responses carry the backend instance and epoch qualifiers.
    assert!(response.backend_instance_uid.is_some());
    assert!(response.server_epoch.is_some());
    let created: CreatedSpace = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert!(!created.replayed);
    assert!(created.native_token.starts_with('$'));
    assert!(scratch.session_names().contains(&"proj".to_string()));

    // Identical envelope again: replayed result, still exactly one session.
    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 0, "{response:?}");
    let replayed: CreatedSpace = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert!(replayed.replayed);
    assert_eq!(replayed.space_uid, created.space_uid);
    assert_eq!(replayed.space_no, created.space_no);
    assert_eq!(replayed.native_token, created.native_token);
    assert_eq!(
        scratch
            .session_names()
            .iter()
            .filter(|n| *n == "proj")
            .count(),
        1,
        "replay must not create a second session"
    );
}

#[test]
fn concurrent_same_name_serializes_and_concurrent_same_request_replays() {
    let scratch = Scratch::with_tmux("create-races");

    // Different requests for one exact name serialize on the owner decision
    // lock. Exactly one allocates; the loser observes the winner through the
    // locked cross-backend preflight and never invokes a second create.
    let left = envelope(
        protocol::methods::NEW,
        Uuid::new_v4(),
        new_payload("decision-race"),
    );
    let right = envelope(
        protocol::methods::NEW,
        Uuid::new_v4(),
        new_payload("decision-race"),
    );
    let (left_result, right_result) = std::thread::scope(|threads| {
        let left = threads.spawn(|| scratch.agent(&left));
        let right = threads.spawn(|| scratch.agent(&right));
        (left.join().unwrap(), right.join().unwrap())
    });
    let results = [left_result, right_result];
    assert_eq!(results.iter().filter(|(code, _)| *code == 0).count(), 1);
    let loser = results.iter().find(|(code, _)| *code != 0).unwrap();
    assert_eq!(loser.0, 4, "{:?}", loser.1);
    assert_eq!(error_code(&loser.1), "name_conflict");
    assert_eq!(
        scratch
            .session_names()
            .iter()
            .filter(|name| *name == "decision-race")
            .count(),
        1
    );
    assert_eq!(
        scratch
            .registry()
            .spaces()
            .unwrap()
            .iter()
            .filter(|space| space.logical_name == "decision-race")
            .count(),
        1,
        "the losing race must not allocate a reserved/aborted identity"
    );

    // Two in-flight deliveries of the same byte-identical request use the
    // same decision fence. The waiter re-reads the ledger after acquiring
    // it and returns the first invocation's result rather than classifying
    // that result's native row as a collision.
    let replay = envelope(
        protocol::methods::NEW,
        Uuid::new_v4(),
        new_payload("concurrent-replay"),
    );
    let (first, second) = std::thread::scope(|threads| {
        let first = threads.spawn(|| scratch.agent(&replay));
        let second = threads.spawn(|| scratch.agent(&replay));
        (first.join().unwrap(), second.join().unwrap())
    });
    assert_eq!((first.0, second.0), (0, 0), "{first:?}\n{second:?}");
    let first: CreatedSpace = serde_json::from_value(first.1.payload.unwrap()).unwrap();
    let second: CreatedSpace = serde_json::from_value(second.1.payload.unwrap()).unwrap();
    assert_eq!(first.space_uid, second.space_uid);
    assert_eq!(first.native_token, second.native_token);
    assert_ne!(first.replayed, second.replayed);
    assert_eq!(
        scratch
            .session_names()
            .iter()
            .filter(|name| *name == "concurrent-replay")
            .count(),
        1
    );
}

#[test]
fn stale_epoch_or_wrong_instance_claims_refuse_before_creation() {
    let scratch = Scratch::with_tmux("epoch");

    // Claimed server epoch that is not the live verified one.
    let mut request = envelope(
        protocol::methods::NEW,
        Uuid::new_v4(),
        new_payload("epochy"),
    );
    request.server_epoch = Some(ServerEpoch(Uuid::from_u128(42)));
    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 1, "{response:?}");
    assert_eq!(error_code(&response), "backend_epoch_changed");
    assert!(
        !scratch.session_names().contains(&"epochy".to_string()),
        "a stale-epoch refusal must create nothing"
    );

    // Claimed backend instance that is not this owner's tmux instance.
    let mut request = envelope(
        protocol::methods::NEW,
        Uuid::new_v4(),
        new_payload("foreign"),
    );
    request.backend_instance_uid = Some(BackendInstanceUid(Uuid::from_u128(43)));
    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 1, "{response:?}");
    assert_eq!(error_code(&response), "wrong_backend_instance");
    assert!(!scratch.session_names().contains(&"foreign".to_string()));

    // The correct claims pass the same matrix.
    let probe = envelope(protocol::methods::HELLO, Uuid::new_v4(), json!({}));
    let (_, hello) = scratch.agent(&probe);
    let info: protocol::HelloInfo = serde_json::from_value(hello.payload.unwrap()).unwrap();
    let tmux = info
        .backends
        .iter()
        .find(|b| b.backend == dmux::model::Backend::Tmux)
        .expect("bootstrapped tmux instance");
    let mut request = envelope(
        protocol::methods::NEW,
        Uuid::new_v4(),
        new_payload("qualified"),
    );
    request.backend_instance_uid = Some(tmux.backend_instance_uid);
    request.server_epoch = tmux.server_epoch;
    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 0, "{response:?}");
    assert!(scratch.session_names().contains(&"qualified".to_string()));
}

#[test]
fn unbootstrapped_owner_refuses_mutations_without_creating_state() {
    let scratch = Scratch::new("no-tmux");
    let request = envelope(protocol::methods::NEW, Uuid::new_v4(), new_payload("nope"));
    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 6, "{response:?}");
    assert_eq!(error_code(&response), "provider_unavailable");
    // Nothing was allocated: the registry has no spaces and no instances.
    assert!(scratch.registry().spaces().unwrap().is_empty());
}

#[test]
fn wez_creation_is_a_typed_refusal_never_a_tmux_fallback() {
    let scratch = Scratch::with_tmux("wez-refusal");
    let request = envelope(
        protocol::methods::NEW,
        Uuid::new_v4(),
        json!({ "name": "wezzy", "backend": "wez" }),
    );
    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 6, "{response:?}");
    assert_eq!(error_code(&response), "provider_unavailable");
    assert!(
        !scratch.session_names().contains(&"wezzy".to_string()),
        "a refused wez create must NOT fall back to tmux"
    );
}

#[test]
fn rename_and_rm_replay_cross_invocation() {
    let scratch = Scratch::with_tmux("cycle");
    let create = envelope(
        protocol::methods::NEW,
        Uuid::new_v4(),
        new_payload("before"),
    );
    let (code, response) = scratch.agent(&create);
    assert_eq!(code, 0, "{response:?}");
    let created: CreatedSpace = serde_json::from_value(response.payload.unwrap()).unwrap();

    // Rename, then replay the identical envelope.
    let rename = envelope(
        protocol::methods::RENAME,
        Uuid::new_v4(),
        json!({ "space_uid": created.space_uid, "new_name": "after" }),
    );
    let (code, response) = scratch.agent(&rename);
    assert_eq!(code, 0, "{response:?}");
    let result: RenameResult = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert!(!result.replayed);
    assert_eq!(result.name, "after");
    let names = scratch.session_names();
    assert!(names.contains(&"after".to_string()) && !names.contains(&"before".to_string()));

    let (code, response) = scratch.agent(&rename);
    assert_eq!(code, 0, "{response:?}");
    let replayed: RenameResult = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert!(replayed.replayed);
    assert_eq!(replayed.name, "after");

    // Remove, replay, then a FRESH uid against the tombstone.
    let rm = envelope(
        protocol::methods::RM,
        Uuid::new_v4(),
        json!({ "space_uid": created.space_uid }),
    );
    let (code, response) = scratch.agent(&rm);
    assert_eq!(code, 0, "{response:?}");
    let result: RmResult = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert!(result.removed && !result.replayed);
    assert!(!scratch.session_names().contains(&"after".to_string()));

    let (code, response) = scratch.agent(&rm);
    assert_eq!(code, 0, "{response:?}");
    let replayed: RmResult = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert!(replayed.removed && replayed.replayed);

    let fresh_rm = envelope(
        protocol::methods::RM,
        Uuid::new_v4(),
        json!({ "space_uid": created.space_uid }),
    );
    let (code, response) = scratch.agent(&fresh_rm);
    assert_eq!(code, 3, "{response:?}");
    assert_eq!(error_code(&response), "space_deleted");
}

#[test]
fn spaces_lists_registry_rows_with_a_complete_scan() {
    let scratch = Scratch::with_tmux("spaces");
    let create = envelope(
        protocol::methods::NEW,
        Uuid::new_v4(),
        new_payload("listed"),
    );
    let (code, _) = scratch.agent(&create);
    assert_eq!(code, 0);

    let request = envelope(protocol::methods::SPACES, Uuid::new_v4(), json!({}));
    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 0, "{response:?}");
    let info: protocol::SpacesInfo = serde_json::from_value(response.payload.unwrap()).unwrap();
    let row = info
        .spaces
        .iter()
        .find(|s| s.name == "listed")
        .expect("created space listed");
    assert_eq!(row.backend, dmux::model::Backend::Tmux);
    assert_eq!(row.lifecycle, dmux::model::Lifecycle::Active);
    assert!(row.native_token.as_deref().unwrap_or("").starts_with('$'));
    let scan = info
        .scans
        .iter()
        .find(|s| s.backend == dmux::model::Backend::Tmux)
        .expect("tmux scan summary");
    assert_eq!(scan.outcome, "complete");
    assert!(scan.server_epoch.is_some());
}

#[test]
fn spaces_reports_recovering_without_native_ids_or_waiting_on_backend_exclusive() {
    let scratch = Scratch::with_tmux("spaces-recovering");
    let create = envelope(
        protocol::methods::NEW,
        Uuid::new_v4(),
        new_payload("fenced"),
    );
    let (code, response) = scratch.agent(&create);
    assert_eq!(code, 0, "{response:?}");

    let instance = scratch
        .registry()
        .register_backend_instance(Backend::Tmux, scratch.ns.as_deref(), None)
        .unwrap();
    let mut recovery_locks = OrderedLocks::new(scratch.locks.path());
    recovery_locks
        .acquire(LockScope::AuthorityGate, LockMode::Shared)
        .unwrap();
    recovery_locks
        .acquire(LockScope::BackendInstance(instance), LockMode::Exclusive)
        .unwrap();

    let started = Instant::now();
    let request = envelope(protocol::methods::SPACES, Uuid::new_v4(), json!({}));
    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 0, "{response:?}");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "spaces must fail fast on the non-blocking backend read fence"
    );
    let info: protocol::SpacesInfo = serde_json::from_value(response.payload.unwrap()).unwrap();
    let row = info
        .spaces
        .iter()
        .find(|space| space.name == "fenced")
        .expect("logical registry row remains visible");
    assert_eq!(row.backend, Backend::Tmux);
    assert_eq!(row.native_token, None, "partial native IDs must be hidden");
    let scan = info
        .scans
        .iter()
        .find(|scan| scan.backend == Backend::Tmux)
        .expect("tmux scan summary");
    assert_eq!(scan.outcome, "recovering");
    assert_eq!(scan.rows, None);
    assert_eq!(scan.server_epoch, None);
    assert!(scan.detail.as_deref().unwrap_or("").contains("recovering"));
}

#[test]
fn remote_new_canonicalizes_only_a_valid_owner_directory_before_mutation() {
    let scratch = Scratch::with_tmux("owner-cwd");
    let owner_dir = scratch.data.path().join("owner-dir");
    std::fs::create_dir(&owner_dir).unwrap();
    let owner_alias = scratch.data.path().join("owner-alias");
    std::os::unix::fs::symlink(&owner_dir, &owner_alias).unwrap();
    let marker = scratch.data.path().join("owner-pwd");
    let request = envelope(
        protocol::methods::NEW,
        Uuid::new_v4(),
        json!({
            "name": "valid-owner-cwd",
            "backend": "tmux",
            "cwd": owner_alias,
            "program": ["sh", "-c", format!("pwd > {}; exec sleep 300", marker.display())],
        }),
    );
    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 0, "{response:?}");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !marker.exists() {
        assert!(Instant::now() < deadline, "owner cwd marker never appeared");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap().trim(),
        std::fs::canonicalize(&owner_dir)
            .unwrap()
            .display()
            .to_string()
    );

    let owner_file = scratch.data.path().join("not-a-directory");
    std::fs::write(&owner_file, b"file").unwrap();
    for (name, cwd, expected) in [
        ("relative", "controller/relative".to_string(), "usage"),
        (
            "missing",
            scratch.data.path().join("missing").display().to_string(),
            "not_found",
        ),
        ("file", owner_file.display().to_string(), "usage"),
        ("control", "/tmp/owner\nforged".to_string(), "usage"),
    ] {
        let (code, response) = scratch.agent(&envelope(
            protocol::methods::NEW,
            Uuid::new_v4(),
            json!({
                "name": name,
                "backend": "tmux",
                "cwd": cwd,
                "program": ["sleep", "300"],
            }),
        ));
        assert_ne!(code, 0, "{name}: {response:?}");
        assert_eq!(error_code(&response), expected, "{name}: {response:?}");
    }
    let names = scratch.session_names();
    assert!(names.contains(&"valid-owner-cwd".to_string()));
    for rejected in ["relative", "missing", "file", "control"] {
        assert!(
            !names.contains(&rejected.to_string()),
            "{rejected} allocated native identity"
        );
    }
    let registry_names: Vec<_> = scratch
        .registry()
        .spaces()
        .unwrap()
        .into_iter()
        .map(|space| space.logical_name)
        .collect();
    assert_eq!(registry_names, vec!["valid-owner-cwd"]);
}
