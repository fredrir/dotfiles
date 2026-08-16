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
use crate::model::{
    BackendInstanceUid, ChildKind, Health, HostUid, ProviderHandle, RegistryUid, ServerEpoch,
    SpaceUid,
};

pub const PROTOCOL_VERSION: u32 = 1;
pub const CAP_NEW_LOOKUP: &str = "new_lookup_v1";
pub const CAP_NEW_FENCED_COLLISION: &str = "new_fenced_collision_v1";

/// Method names carried in the envelope. The set grows per phase; the
/// envelope shape is what P1 freezes. `hello` (enrollment/lineage handshake,
/// plan §12.2) and `attach_plan` (§12.1) are named by the plan itself.
/// P7 (ADR 009 §4) adds the read/mutation methods additively.
pub mod methods {
    pub const HELLO: &str = "hello";
    pub const SPACES: &str = "spaces";
    pub const NEW_LOOKUP: &str = "new_lookup";
    pub const NEW: &str = "new";
    pub const RENAME: &str = "rename";
    pub const RM: &str = "rm";
    pub const ATTACH_PLAN: &str = "attach_plan";
    // P8b remote hierarchy (additive).
    pub const HIERARCHY: &str = "hierarchy";
    pub const GROUP_NEW: &str = "group_new";
    pub const GROUP_ACTIVATE: &str = "group_activate";
    pub const GROUP_RENAME: &str = "group_rename";
    pub const GROUP_RM: &str = "group_rm";
    pub const SPLIT_NEW: &str = "split_new";
    pub const SPLIT_DIRECTION: &str = "split_direction";
    pub const SPLIT_RESIZE: &str = "split_resize";
    pub const SPLIT_ZOOM: &str = "split_zoom";
    pub const SPLIT_RM: &str = "split_rm";
    // P10 owner recovery control (additive, registry/runtime scoped).
    pub const RECOVERY_STATUS: &str = "recovery_status";
    pub const RECOVERY_RESUME: &str = "recovery_resume";
    pub const RECOVERY_ABORT: &str = "recovery_abort";
}

/// Canonical payload bytes for `payload_sha256`: the exact `payload` value
/// serialized with `serde_json::to_string`. Both sides run stock serde_json
/// (object keys sorted, no preserve_order feature), so parsing and
/// re-serializing a payload yields identical bytes on client and owner.
pub fn canonical_payload_sha256(payload: &Value) -> String {
    crate::registry::sha256::sha256_hex(
        serde_json::to_string(payload)
            .expect("serde_json::Value always serializes")
            .as_bytes(),
    )
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
    /// Single-use opaque token; replay is rejected owner-side. The raw
    /// token is returned exactly once: a replayed `attach_plan` request
    /// returns the stored plan with an EMPTY token (only the sha256 is ever
    /// persisted), so the caller must mint a fresh plan under a fresh
    /// request UID.
    pub token: String,
    /// Exact owner-correlated child focus stored in the token's native argv.
    /// Absent for a Space-only attach and in pre-P9 response documents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child: Option<AttachPlanChild>,
    /// True when this response replayed the idempotency ledger (P7
    /// additive; absent means false).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replayed: bool,
}

/// Epoch-qualified child requested by the client.  A Split's Group parent is
/// intentionally not supplied: only the owner may discover it from a live
/// same-epoch hierarchy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachChildRequest {
    pub kind: ChildKind,
    pub epoch: ServerEpoch,
    pub handle: ProviderHandle,
}

/// Child focus the owner proved and embedded in the single-use token's exact
/// native argv.  Split responses include the owner-discovered Group parent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AttachPlanChild {
    Group {
        epoch: ServerEpoch,
        handle: ProviderHandle,
    },
    Split {
        epoch: ServerEpoch,
        group: ProviderHandle,
        split: ProviderHandle,
    },
}

// ---------------------------------------------------------------------------
// P7 method payloads (additive; ADR 009 §4). The envelope shape above is
// the frozen contract — these are the per-method `payload` schemas.

/// `hello` request payload. The nonce binds a "fresh" handshake for the
/// §12.1 rollback-quarantine rule; the response echoes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct HelloPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<Uuid>,
}

/// One managed backend instance as `hello` reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackendStatus {
    pub backend: crate::model::Backend,
    pub backend_instance_uid: BackendInstanceUid,
    /// Published server incarnation epoch; None while stopped/unbootstrapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_epoch: Option<ServerEpoch>,
    /// wez: exact service socket; tmux: `-L` namespace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket_path: Option<String>,
}

/// One authority-revision link, as recorded in `authority_revisions`. The
/// client recomputes `chain_head_hash(parent_head_hash, revision, txn_uid)`
/// to verify descent from its cached checkpoint (or from the genesis hash).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChainLink {
    pub revision: u64,
    pub parent_head_hash: String,
    pub head_hash: String,
    pub txn_uid: Uuid,
}

/// `hello` response payload: stable identity, lineage proof, capabilities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloInfo {
    pub host_uid: HostUid,
    pub registry_uid: RegistryUid,
    pub authority_revision: u64,
    pub authority_head_hash: String,
    pub protocol_version: u32,
    pub agent_version: String,
    pub capabilities: Vec<String>,
    pub backends: Vec<BackendStatus>,
    /// Full recorded revision chain — the §12.1 ancestry proof. Verifiable
    /// from `genesis_head_hash(registry_uid)` upward.
    pub revision_chain: Vec<ChainLink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<Uuid>,
}

/// One registry Space row as `spaces` reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpaceInfo {
    pub space_uid: SpaceUid,
    pub space_no: u64,
    pub name: String,
    pub backend: crate::model::Backend,
    pub backend_instance_uid: BackendInstanceUid,
    pub lifecycle: crate::model::Lifecycle,
    pub health: crate::model::Health,
    /// Current native binding token, when one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_token: Option<String>,
}

/// Typed summary of one owner-side provider scan (`spaces` response).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScanSummary {
    pub backend: crate::model::Backend,
    /// Snake-case token of the `InventoryOutcome` variant (plan §8.1), or
    /// `unavailable` when the agent has no scan path for this backend.
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_epoch: Option<ServerEpoch>,
}

/// `spaces` response payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpacesInfo {
    pub spaces: Vec<SpaceInfo>,
    pub scans: Vec<ScanSummary>,
}

/// Owner-scoped literal exact-name lookup request. The owner computes both
/// backend partitions under its decision/instance fences; SpacesInfo cannot
/// substitute because it intentionally omits unmanaged native names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewLookupPayload {
    pub name: String,
}

/// Provider-neutral wire form of one §8.2 exact-name partition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "class", rename_all = "snake_case")]
pub enum NewLookupClass {
    NoMatch,
    Selectable {
        space_uid: SpaceUid,
        space_no: u64,
    },
    Blocking {
        reason: NewLookupBlockReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        space_uid: Option<SpaceUid>,
    },
    Indeterminate,
}

/// Exact typed blocking reason, intentionally excluding every provider
/// native token/handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", content = "health", rename_all = "snake_case")]
pub enum NewLookupBlockReason {
    LifecycleReserved,
    LifecycleDeleting,
    LifecycleConflict,
    OperationInProgress,
    Unhealthy(Health),
    NoBinding,
    ActiveAbsent,
    ServerStopped,
    IndeterminateObservation,
    UnmanagedSameName,
    MultiWindow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewLookupResult {
    pub wez: NewLookupClass,
    pub tmux: NewLookupClass,
}

/// `new` request payload. `backend` is the client's product-level choice;
/// native details (namespace, socket, helper, epochs) NEVER come from the
/// client — the owner resolves them (ADR 009 §4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewPayload {
    pub name: String,
    pub backend: crate::model::Backend,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// User program argv; empty means a login shell.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub program: Vec<String>,
    /// Valid only with an explicitly selected backend. The owner may retain
    /// one opposite managed selectable row, never an unmanaged/blocking one.
    #[serde(default)]
    pub allow_name_collision: bool,
}

/// `rename` request payload. The Space's backend/instance come from the
/// owner registry, never from the client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenamePayload {
    pub space_uid: SpaceUid,
    pub new_name: String,
}

/// `rename` response payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenameResult {
    pub space_uid: SpaceUid,
    pub name: String,
    #[serde(default)]
    pub replayed: bool,
}

/// `rm` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RmPayload {
    pub space_uid: SpaceUid,
}

/// `rm` response payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RmResult {
    pub space_uid: SpaceUid,
    pub removed: bool,
    #[serde(default)]
    pub replayed: bool,
}

/// `attach_plan` request payload (tmux Spaces only). `route` is an audit
/// label naming the route the client selected; it is recorded with the
/// token and echoed in the plan — it is never a native target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttachPlanPayload {
    pub space_uid: SpaceUid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child: Option<AttachChildRequest>,
}

// ---------------------------------------------------------------------------
// P8b remote hierarchy payloads (additive). Child refs travel as the
// canonical §6.3 child-suffix STRING (`g<epoch-uuid>.<handle>` /
// `p<epoch-uuid>.<handle>`) — the exact spelling markers and the CLI use —
// and are parsed owner-side via `refs::parse_ref`; a stale epoch is a typed
// refusal, never a retarget. Responses reuse the operations-layer result
// types verbatim: `hierarchy` → `operations::SpaceHierarchy`,
// `group_new`/`split_new` → `operations::CreatedChild`,
// `group_rm`/`split_rm` → `operations::RemovedChild`.

/// `hierarchy` request payload (read-only tree of one Space).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HierarchyPayload {
    pub space_uid: SpaceUid,
}

/// `group_new` request payload. Owner paths (helper binary, sockets,
/// namespaces) never come from the client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupNewPayload {
    pub space_uid: SpaceUid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// User program argv; empty means a login shell.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub program: Vec<String>,
}

/// `group_activate` targets one exact epoch-qualified Group. Presentation
/// focus is owner-side; the client never supplies a native window ID.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupActivatePayload {
    pub space_uid: SpaceUid,
    pub group_ref: String,
}

/// Idempotently stored result of exact Group activation. The response
/// contains only canonical dmux refs, never raw backend IDs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupActivateResult {
    pub space_uid: SpaceUid,
    pub server_epoch: ServerEpoch,
    pub group_ref: String,
    #[serde(default)]
    pub replayed: bool,
}

/// `split_new` request payload. `direction` is one of
/// `left|right|up|down` (absent means `down`, the CLI default);
/// `percent` is 1..=99.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SplitNewPayload {
    pub space_uid: SpaceUid,
    /// Canonical child suffix of the target Group.
    pub group_ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub program: Vec<String>,
}

/// `split_direction` selects the exact Split adjacent to `split_ref` in
/// `direction`. At an edge the result has no target; no ordinal or wrap
/// guess is permitted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SplitDirectionPayload {
    pub space_uid: SpaceUid,
    pub split_ref: String,
    pub direction: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SplitDirectionResult {
    pub space_uid: SpaceUid,
    pub server_epoch: ServerEpoch,
    /// Canonical parent Group of the exact origin/target.
    pub group_ref: String,
    /// Canonical adjacent target, or `None` for the verified edge no-op.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split_ref: Option<String>,
    #[serde(default)]
    pub replayed: bool,
}

/// `split_resize` resizes one exact epoch-qualified Split by a positive
/// number of cells. Native layout boundaries are reported as
/// `changed=false`, not guessed as success-with-change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SplitResizePayload {
    pub space_uid: SpaceUid,
    pub split_ref: String,
    pub direction: String,
    pub amount: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SplitResizeResult {
    pub space_uid: SpaceUid,
    pub server_epoch: ServerEpoch,
    pub split_ref: String,
    pub changed: bool,
    #[serde(default)]
    pub replayed: bool,
}

/// `split_zoom` toggles zoom for one exact epoch-qualified Split and
/// returns the verified final state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SplitZoomPayload {
    pub space_uid: SpaceUid,
    pub split_ref: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SplitZoomResult {
    pub space_uid: SpaceUid,
    pub server_epoch: ServerEpoch,
    pub split_ref: String,
    pub zoomed: bool,
    #[serde(default)]
    pub replayed: bool,
}

/// `group_rename` request payload (presentation title, not an authority
/// name).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupRenamePayload {
    pub space_uid: SpaceUid,
    pub group_ref: String,
    pub title: String,
}

/// `group_rename` response payload. Carries no replay marker: the
/// operations-layer ledger treats a replayed title write as the same
/// idempotent success.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupRenameResult {
    pub space_uid: SpaceUid,
    pub group_ref: String,
    pub title: String,
}

/// `group_rm` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupRmPayload {
    pub space_uid: SpaceUid,
    pub group_ref: String,
}

/// `split_rm` request payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SplitRmPayload {
    pub space_uid: SpaceUid,
    pub split_ref: String,
}

// ---------------------------------------------------------------------------
// P10 owner recovery control. Targets are deliberately empty: the owner
// resolves its one managed Wez backend instance and production/scratch
// runtime. A controller can qualify the common envelope with the expected
// backend-instance/epoch, but can never submit owner filesystem paths.

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RecoveryStatusPayload {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RecoveryResumePayload {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RecoveryAbortPayload {}

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
    fn canonical_digest_is_stable_across_parse_reserialize() {
        // Key order in the source text must not matter: stock serde_json
        // sorts object keys, so both sides derive identical canonical bytes.
        let a: Value = serde_json::from_str(r#"{"name":"x","backend":"tmux"}"#).unwrap();
        let b: Value = serde_json::from_str(r#"{"backend":"tmux","name":"x"}"#).unwrap();
        assert_eq!(canonical_payload_sha256(&a), canonical_payload_sha256(&b));
        // Empty payload digest matches sha256("{}").
        let empty: Value = serde_json::json!({});
        assert_eq!(
            canonical_payload_sha256(&empty),
            crate::registry::sha256::sha256_hex(b"{}")
        );
    }

    #[test]
    fn attach_plan_replayed_flag_is_additive() {
        // Old-shape JSON (no `replayed`) still parses; false is omitted on
        // serialize so pre-P7 readers see the frozen shape.
        let json = r#"{
          "request_uid": "0192aaaa-bbbb-4ccc-8ddd-eeeeffff0001",
          "host_uid": "0192aaaa-bbbb-4ccc-8ddd-eeeeffff0002",
          "space_uid": "0192aaaa-bbbb-7ccc-8ddd-eeeeffff0003",
          "server_epoch": "0192aaaa-bbbb-4ccc-8ddd-eeeeffff0004",
          "route": "archie",
          "expires_at": "2026-08-16T12:34:56Z",
          "token": "t"
        }"#;
        let plan: AttachPlan = serde_json::from_str(json).unwrap();
        assert!(!plan.replayed);
        let value = serde_json::to_value(&plan).unwrap();
        assert!(value.get("replayed").is_none());
    }

    #[test]
    fn method_payloads_round_trip() {
        let new: NewPayload = serde_json::from_value(serde_json::json!({
            "name": "proj", "backend": "tmux"
        }))
        .unwrap();
        assert!(new.program.is_empty() && new.cwd.is_none() && !new.allow_name_collision);
        let lookup = NewLookupResult {
            wez: NewLookupClass::Blocking {
                reason: NewLookupBlockReason::UnmanagedSameName,
                space_uid: None,
            },
            tmux: NewLookupClass::Selectable {
                space_uid: SpaceUid(Uuid::nil()),
                space_no: 7,
            },
        };
        let lookup_json = serde_json::to_value(&lookup).unwrap();
        assert!(!lookup_json.to_string().contains("native"));
        assert_eq!(
            serde_json::from_value::<NewLookupResult>(lookup_json).unwrap(),
            lookup
        );
        let rename = RenamePayload {
            space_uid: SpaceUid(Uuid::nil()),
            new_name: "after".into(),
        };
        let back: RenamePayload =
            serde_json::from_value(serde_json::to_value(&rename).unwrap()).unwrap();
        assert_eq!(back, rename);
        let hello: HelloPayload = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(hello.nonce, None);
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
