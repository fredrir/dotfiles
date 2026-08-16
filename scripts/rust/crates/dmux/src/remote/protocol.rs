//! Versioned owner-agent message contract (plan §12.1). Frozen at P1.
//!
//! Requests and responses are ONE JSON document each, exchanged over
//! `ssh <verified-route> dmux _agent --protocol 1 <method>`. Mutations are
//! idempotent by `request_uid`: the owner durably binds the UID to method,
//! canonical payload digest, and final/unknown result; reuse with different
//! content is rejected; a cross-route retry sends the identical
//! UID/method/payload and reconciles the original result.
//!
//! Protocol v1 requires an exact version match (plan §17). The bounded JSON
//! RPC is never an interactive tmux transport — that is the `_attach`
//! single-use-token PTY channel below.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::error::TypedError;
use crate::model::{BackendInstanceUid, HostUid, RegistryUid, ServerEpoch, SpaceUid};

pub const PROTOCOL_VERSION: u32 = 1;

/// Method names carried in the envelope. The set grows per phase; the
/// envelope shape is what P1 freezes. `hello` (enrollment/lineage handshake,
/// plan §12.2) and `attach_plan` (§12.1) are named by the plan itself.
pub mod methods {
    pub const HELLO: &str = "hello";
    pub const ATTACH_PLAN: &str = "attach_plan";
}

/// The common frame both requests and responses carry (plan §12.1 field
/// list). Identity fields let every route present proof it reaches the
/// enrolled authority: HostUid, RegistryUid + lineage, backend instance and
/// server epoch where applicable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub protocol_version: u32,
    pub request_uid: Uuid,
    pub method: String,
    /// SHA-256 (lowercase hex) of the canonical `payload` bytes; the
    /// idempotency ledger stores it and rejects UID reuse with a
    /// different digest.
    pub payload_sha256: String,
    pub host_uid: HostUid,
    pub registry_uid: RegistryUid,
    pub authority_revision: u64,
    pub authority_head_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_instance_uid: Option<BackendInstanceUid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_epoch: Option<ServerEpoch>,
    pub capabilities: Vec<String>,
    /// Exactly one of `payload` / `error` is present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<TypedError>,
}

impl Envelope {
    /// Contract invariant: a well-formed envelope carries exactly one of
    /// payload/error (requests always use `payload`).
    pub fn is_well_formed(&self) -> bool {
        self.protocol_version == PROTOCOL_VERSION && (self.payload.is_some() ^ self.error.is_some())
    }
}

/// The validated short-lived attach plan for remote tmux presentation
/// (plan §12.1). The client then runs
/// `ssh -t <verified-route> dmux _attach --token <token>`; `_attach`
/// verifies every field plus replay state and `exec`s the exact
/// owner-generated attach command. It accepts no native target and no
/// command text from the client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttachPlan {
    pub request_uid: Uuid,
    pub host_uid: HostUid,
    pub space_uid: SpaceUid,
    pub server_epoch: ServerEpoch,
    /// Route the token is bound to; presenting it over another route fails.
    pub route: String,
    pub expires_at: String,
    /// Single-use opaque token; replay is rejected owner-side.
    pub token: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Golden v1 envelope — field names and shapes are frozen contract
    /// (mirrored in docs/adr/dmux/008-frozen-contracts-v1.md).
    const GOLDEN: &str = r#"{
      "protocol_version": 1,
      "request_uid": "0192aaaa-bbbb-4ccc-8ddd-eeeeffff0001",
      "method": "hello",
      "payload_sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
      "host_uid": "0192aaaa-bbbb-4ccc-8ddd-eeeeffff0002",
      "registry_uid": "0192aaaa-bbbb-4ccc-8ddd-eeeeffff0003",
      "authority_revision": 42,
      "authority_head_hash": "sha256:abc",
      "backend_instance_uid": "0192aaaa-bbbb-4ccc-8ddd-eeeeffff0004",
      "server_epoch": "0192aaaa-bbbb-4ccc-8ddd-eeeeffff0005",
      "capabilities": ["tmux", "wez", "cas_rename"],
      "payload": {}
    }"#;

    #[test]
    fn golden_envelope_round_trips() {
        let env: Envelope = serde_json::from_str(GOLDEN).unwrap();
        assert!(env.is_well_formed());
        assert_eq!(env.method, methods::HELLO);
        assert_eq!(env.authority_revision, 42);
        let back = serde_json::to_value(&env).unwrap();
        let orig: serde_json::Value = serde_json::from_str(GOLDEN).unwrap();
        assert_eq!(back, orig);
    }

    #[test]
    fn envelope_rejects_payload_and_error_together() {
        let mut env: Envelope = serde_json::from_str(GOLDEN).unwrap();
        env.error = Some(crate::error::TypedError::new(
            crate::error::ErrorCode::ProtocolMismatch,
            "both set",
        ));
        assert!(!env.is_well_formed());
        env.payload = None;
        assert!(env.is_well_formed());
    }

    #[test]
    fn optional_identity_fields_are_omitted_not_null() {
        let mut env: Envelope = serde_json::from_str(GOLDEN).unwrap();
        env.backend_instance_uid = None;
        env.server_epoch = None;
        let v = serde_json::to_value(&env).unwrap();
        let obj = v.as_object().unwrap();
        assert!(!obj.contains_key("backend_instance_uid"));
        assert!(!obj.contains_key("server_epoch"));
    }
}
