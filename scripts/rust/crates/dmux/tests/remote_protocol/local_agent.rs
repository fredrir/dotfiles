//! Local protocol conformance against the real `_agent` binary over the
//! direct-argv transport: hello identity, digest mismatch, UID reuse with
//! different content, unknown method, protocol mismatch, malformed
//! requests. Exactly ONE response envelope on stdout in every case.

use dmux::model::SpaceUid;
use dmux::remote::lineage;
use dmux::remote::protocol::{self, HelloInfo, PROTOCOL_VERSION, RenamePayload};
use serde_json::json;
use uuid::Uuid;

use crate::util::{Scratch, envelope, error_code};

#[test]
fn hello_returns_identity_head_and_verifiable_chain() {
    let scratch = Scratch::new("hello");
    let owner = scratch.registry();
    let identity = owner.identity().unwrap();
    let head = owner.authority_head().unwrap();
    drop(owner);

    let nonce = Uuid::new_v4();
    let request = envelope(
        protocol::methods::HELLO,
        Uuid::new_v4(),
        json!({ "nonce": nonce.to_string() }),
    );
    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 0, "{response:?}");
    assert_eq!(response.protocol_version, PROTOCOL_VERSION);
    assert_eq!(response.request_uid, request.request_uid);
    assert_eq!(response.host_uid, identity.host_uid);
    assert_eq!(response.registry_uid, identity.registry_uid);
    assert_eq!(response.authority_revision, head.revision);
    assert_eq!(response.authority_head_hash, head.head_hash);
    assert!(response.capabilities.iter().any(|c| c == "proto:1"));
    // Response digest covers the canonical payload bytes.
    let payload = response.payload.clone().unwrap();
    assert_eq!(
        response.payload_sha256,
        protocol::canonical_payload_sha256(&payload)
    );

    let hello: HelloInfo = serde_json::from_value(payload).unwrap();
    assert_eq!(hello.host_uid, identity.host_uid);
    assert_eq!(hello.nonce, Some(nonce));
    assert_eq!(hello.protocol_version, PROTOCOL_VERSION);
    // The returned chain must verify from genesis and contain the head.
    assert!(lineage::chain_contains(
        hello.registry_uid,
        &hello.revision_chain,
        None,
        (hello.authority_revision, &hello.authority_head_hash),
    ));
}

#[test]
fn payload_digest_mismatch_is_a_typed_refusal() {
    let scratch = Scratch::new("digest");
    let mut request = envelope(protocol::methods::HELLO, Uuid::new_v4(), json!({}));
    request.payload_sha256 = "deadbeef".repeat(8);
    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 2);
    assert_eq!(error_code(&response), "usage");
    assert!(
        response
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("payload_sha256"),
        "{response:?}"
    );
}

#[test]
fn uid_reuse_with_different_payload_is_rejected() {
    let scratch = Scratch::new("reuse");
    let uid = Uuid::new_v4();
    let space = SpaceUid(Uuid::from_u128(99));
    let first = envelope(
        protocol::methods::RENAME,
        uid,
        serde_json::to_value(RenamePayload {
            space_uid: space,
            new_name: "one".into(),
        })
        .unwrap(),
    );
    // First attempt records the ledger row, then fails on the missing
    // space — the UID is now durably bound to this method+digest.
    let (code, response) = scratch.agent(&first);
    assert_eq!(code, 3, "{response:?}");
    assert_eq!(error_code(&response), "not_found");

    let second = envelope(
        protocol::methods::RENAME,
        uid,
        serde_json::to_value(RenamePayload {
            space_uid: space,
            new_name: "two".into(),
        })
        .unwrap(),
    );
    let (code, response) = scratch.agent(&second);
    assert_eq!(code, 4, "{response:?}");
    assert_eq!(error_code(&response), "idempotency_reuse");

    // Reusing the UID under a DIFFERENT method is equally rejected.
    let cross = envelope(protocol::methods::RM, uid, json!({ "space_uid": space }));
    let (code, response) = scratch.agent(&cross);
    assert_eq!(code, 4, "{response:?}");
    assert_eq!(error_code(&response), "idempotency_reuse");
}

#[test]
fn unknown_method_is_a_typed_usage_error() {
    let scratch = Scratch::new("unknown");
    let request = envelope("frobnicate", Uuid::new_v4(), json!({}));
    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 2);
    assert_eq!(error_code(&response), "usage");
    assert!(
        response
            .error
            .as_ref()
            .unwrap()
            .message
            .contains("frobnicate"),
        "{response:?}"
    );
}

#[test]
fn protocol_mismatch_is_exact_and_typed() {
    let scratch = Scratch::new("proto");
    // CLI flag speaks v2: refused regardless of envelope content.
    let request = envelope(protocol::methods::HELLO, Uuid::new_v4(), json!({}));
    let mut raw: serde_json::Value = serde_json::to_value(&request).unwrap();
    raw["protocol_version"] = json!(2);
    let out = scratch.agent_raw(2, protocol::methods::HELLO, &raw.to_string());
    assert_eq!(out.status.code(), Some(6));
    let response: dmux::remote::protocol::Envelope =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    assert_eq!(error_code(&response), "protocol_mismatch");

    // Envelope v2 behind a v1 flag is refused too (exact match, both ways).
    let out = scratch.agent_raw(1, protocol::methods::HELLO, &raw.to_string());
    assert_eq!(out.status.code(), Some(6));
}

#[test]
fn malformed_and_ill_formed_requests_still_answer_one_envelope() {
    let scratch = Scratch::new("malformed");
    // Not JSON at all.
    let out = scratch.agent_raw(1, protocol::methods::HELLO, "this is not json");
    assert_eq!(out.status.code(), Some(2));
    let response: dmux::remote::protocol::Envelope =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    assert_eq!(error_code(&response), "usage");

    // An error-bearing "request" is refused as ill-formed.
    let mut request = envelope(protocol::methods::HELLO, Uuid::new_v4(), json!({}));
    request.payload = None;
    request.error = Some(dmux::error::TypedError::new(
        dmux::error::ErrorCode::Usage,
        "requests cannot carry errors",
    ));
    let out = scratch.agent_raw(
        1,
        protocol::methods::HELLO,
        &serde_json::to_string(&request).unwrap(),
    );
    assert_eq!(out.status.code(), Some(2));

    // Method disagreement between flag and envelope.
    let request = envelope(protocol::methods::SPACES, Uuid::new_v4(), json!({}));
    let out = scratch.agent_raw(
        1,
        protocol::methods::HELLO,
        &serde_json::to_string(&request).unwrap(),
    );
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn spaces_on_a_fresh_owner_is_empty_with_no_scans() {
    let scratch = Scratch::new("spaces-empty");
    let request = envelope(protocol::methods::SPACES, Uuid::new_v4(), json!({}));
    let (code, response) = scratch.agent(&request);
    assert_eq!(code, 0, "{response:?}");
    let info: dmux::remote::protocol::SpacesInfo =
        serde_json::from_value(response.payload.unwrap()).unwrap();
    assert!(info.spaces.is_empty());
    assert!(info.scans.is_empty(), "no managed instances yet: {info:?}");
}

// ---------------------------------------------------------------------------
// The replies the agent sends BEFORE it can echo a request id.

/// Feed one real `_agent` invocation's stdout back through the client's
/// reply interpreter as the answer to `sent`, and return the failure the
/// caller would see.
fn as_caller_sees(out: &std::process::Output, sent: Uuid) -> dmux::remote::client::AttemptFailure {
    let reply = dmux::remote::client::RawReply {
        status: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        stdout_truncated: false,
        transport_diagnostics: None,
    };
    dmux::remote::client::interpret_reply(&reply, sent)
        .map(|e| panic!("a degraded reply must not be accepted: {e:?}"))
        .unwrap_err()
}

/// The agent answers a request that is not one JSON envelope with
/// `Uuid::nil()`, because there is no id to echo. The caller must see the
/// `usage` refusal, not a complaint about the nil echo.
#[test]
fn an_unparseable_request_is_answered_with_a_nil_echo_and_still_surfaces_usage() {
    let scratch = Scratch::new("nilparse");
    let sent = Uuid::new_v4();
    let out = scratch.agent_raw(protocol::PROTOCOL_VERSION, protocol::methods::HELLO, "{");
    let response: dmux::remote::protocol::Envelope =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    assert_eq!(response.request_uid, Uuid::nil(), "no id to echo");
    assert_eq!(error_code(&response), "usage");

    match as_caller_sees(&out, sent) {
        dmux::remote::client::AttemptFailure::Agent(error) => {
            assert_eq!(error.code, dmux::error::ErrorCode::Usage);
            assert_eq!(
                error,
                response.error.unwrap(),
                "the agent's error, verbatim"
            );
        }
        other => panic!("expected the agent's typed error, got {other:?}"),
    }
}

/// The registry-open path answers with `Uuid::nil()` too: the request has
/// not been parsed yet. Its typed code must reach the caller.
#[test]
fn an_unopenable_registry_is_answered_with_a_nil_echo_and_still_surfaces_its_code() {
    let scratch = Scratch::new("nilregistry");
    // A DIRECTORY where the database file belongs: `Registry::open` fails,
    // `resolve_env` does not.
    std::fs::create_dir(scratch.data.path().join("registry.sqlite3")).unwrap();
    let sent = Uuid::new_v4();
    let request = envelope(protocol::methods::HELLO, Uuid::new_v4(), json!({}));
    let out = scratch.agent_raw(
        protocol::PROTOCOL_VERSION,
        protocol::methods::HELLO,
        &serde_json::to_string(&request).unwrap(),
    );
    let response: dmux::remote::protocol::Envelope =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    assert_eq!(response.request_uid, Uuid::nil(), "no id read yet");
    let reported = response.error.clone().unwrap();
    assert!(
        reported.message.starts_with("registry:"),
        "{reported:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    match as_caller_sees(&out, sent) {
        dmux::remote::client::AttemptFailure::Agent(error) => assert_eq!(error, reported),
        other => panic!("expected the agent's typed error, got {other:?}"),
    }
}
