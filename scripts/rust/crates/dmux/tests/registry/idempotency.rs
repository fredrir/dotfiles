//! rpc_requests idempotency ledger (plan §12.1): replay returns the stored
//! final result, UID reuse with a different digest is rejected, and
//! unknown-state rows are resumable.

use dmux::error::ErrorCode;
use dmux::registry::{RegistryError, RpcDisposition, RpcResultState};
use uuid::Uuid;

use crate::util::{open, scratch};

const DIGEST: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const OTHER_DIGEST: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

#[test]
fn replay_with_same_uid_and_digest_returns_the_stored_final_result() {
    let s = scratch();
    let mut reg = open(&s.config);
    let request = Uuid::new_v4();

    assert_eq!(
        reg.record_rpc_request(request, "new", DIGEST).unwrap(),
        RpcDisposition::New
    );
    let result = serde_json::json!({ "ok": true, "space_no": 2 });
    reg.finish_rpc_request(request, &result, None).unwrap();

    // A cross-route retry with the identical UID/method/digest replays the
    // original result instead of allocating or creating twice.
    match reg.record_rpc_request(request, "new", DIGEST).unwrap() {
        RpcDisposition::Replay {
            result_state,
            result_json,
        } => {
            assert_eq!(result_state, RpcResultState::Final);
            assert_eq!(result_json, Some(result));
        }
        other => panic!("expected replay, got {other:?}"),
    }
}

#[test]
fn uid_reuse_with_different_digest_or_method_is_rejected() {
    let s = scratch();
    let mut reg = open(&s.config);
    let request = Uuid::new_v4();
    reg.record_rpc_request(request, "new", DIGEST).unwrap();

    let err = reg
        .record_rpc_request(request, "new", OTHER_DIGEST)
        .unwrap_err();
    assert!(
        matches!(&err, RegistryError::IdempotencyReuse { request_uid } if *request_uid == request)
    );
    assert_eq!(err.error_code(), ErrorCode::IdempotencyReuse);

    // A different method with the same digest is also different content.
    let err = reg.record_rpc_request(request, "rm", DIGEST).unwrap_err();
    assert!(matches!(err, RegistryError::IdempotencyReuse { .. }));

    // The ledger row is unchanged by the rejected attempts.
    let (method, digest): (String, String) = reg
        .raw_connection()
        .query_row(
            "SELECT method, payload_sha256 FROM rpc_requests WHERE request_uid=?1",
            [request.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(method, "new");
    assert_eq!(digest, DIGEST);
}

#[test]
fn unknown_state_rows_are_resumable() {
    let s = scratch();
    let mut reg = open(&s.config);
    let request = Uuid::new_v4();
    reg.record_rpc_request(request, "rename", DIGEST).unwrap();

    // Before any result is bound, the same request replays as unknown —
    // the holder resumes it rather than re-running blindly.
    match reg.record_rpc_request(request, "rename", DIGEST).unwrap() {
        RpcDisposition::Replay {
            result_state,
            result_json,
        } => {
            assert_eq!(result_state, RpcResultState::Unknown);
            assert_eq!(result_json, None);
        }
        other => panic!("expected unknown replay, got {other:?}"),
    }

    // Resumption finishes it; the final result then replays.
    let result = serde_json::json!({ "ok": true });
    reg.finish_rpc_request(request, &result, None).unwrap();
    match reg.record_rpc_request(request, "rename", DIGEST).unwrap() {
        RpcDisposition::Replay { result_state, .. } => {
            assert_eq!(result_state, RpcResultState::Final);
        }
        other => panic!("expected final replay, got {other:?}"),
    }
}

#[test]
fn finishing_an_unknown_request_uid_is_typed_not_found() {
    let s = scratch();
    let mut reg = open(&s.config);
    let err = reg
        .finish_rpc_request(Uuid::new_v4(), &serde_json::json!({}), None)
        .unwrap_err();
    assert!(matches!(err, RegistryError::NotFound { .. }));
}
