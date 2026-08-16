//! Bounded remote mutations against the real `_agent` on a scratch tmux
//! namespace: create with end-to-end request-UID idempotency (same
//! envelope twice → ONE session, identical replayed result), the
//! backend-instance/epoch verification matrix (stale claims refused,
//! nothing created), rename/rm with cross-invocation replay.

use dmux::model::{BackendInstanceUid, ServerEpoch};
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
