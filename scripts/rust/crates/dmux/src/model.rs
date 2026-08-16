//! Core provider-neutral model: identities, backends, hierarchy, and states.
//! Plan §5 (domain model), §6.1 (identity types), §5.2 (Space state).
//!
//! Serialized spellings are contract: every enum serializes to the exact
//! snake_case token the plan and `docs/adr/dmux/registry-v1.sql` use.

use std::fmt;
use std::num::NonZeroU64;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One dmux installation authority (UUIDv4). Permanent unless explicit rekey.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HostUid(pub Uuid);

/// One Space lifecycle (UUIDv7). Permanent and globally unique; never reused
/// even after deletion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SpaceUid(pub Uuid);

/// Registry lineage identity for clone/rollback detection (plan §6.1, §12.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RegistryUid(pub Uuid);

/// One managed backend server namespace (plan §2.15: in v1, exactly one
/// unix-Wez instance and one default tmux namespace per owner).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BackendInstanceUid(pub Uuid);

/// One backend-server incarnation. A fresh epoch invalidates every live
/// Group/Split handle minted under the previous one (plan §6.3, ADR 002).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ServerEpoch(pub Uuid);

/// Per-owner monotonic display number. Permanent, never reused; canonical
/// form is nonzero decimal with no leading zeros (plan §6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SpaceNo(pub NonZeroU64);

impl SpaceNo {
    pub fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Display for SpaceNo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    Wez,
    Tmux,
}

impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Wez => "wez",
            Backend::Tmux => "tmux",
        }
    }
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Registry lifecycle — the durable half of Space state (plan §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    Reserved,
    Active,
    Deleting,
    Deleted,
    Conflict,
    Aborted,
}

impl Lifecycle {
    /// Rows in a live lifecycle occupy their logical name
    /// (`spaces_live_name_uq` in registry-v1.sql).
    pub fn occupies_name(self) -> bool {
        matches!(
            self,
            Lifecycle::Reserved | Lifecycle::Active | Lifecycle::Deleting | Lifecycle::Conflict
        )
    }

    /// Terminal history: never matches a lookup, but keeps its identifiers
    /// unavailable forever (plan §8.2 step 5).
    pub fn is_terminal(self) -> bool {
        matches!(self, Lifecycle::Deleted | Lifecycle::Aborted)
    }
}

/// Observation state — what the last determinate look at the native resource
/// showed. Never erases a durable registry match (plan §5.2, §8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Observation {
    Live,
    Absent,
    Stopped,
    Unreachable,
    Incompatible,
    Unmanaged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    Healthy,
    MultiWindow,
    NativeKeyCollision,
    Unstamped,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientState {
    Attached,
    Detached,
    Unknown,
}

/// A backend-native child handle as it appears inside a ref (plan §6.3):
/// `wz-<decimal>` for Wez tab/pane IDs, `tx-<decimal>` for a tmux `@N`
/// window or `%N` pane (the g/p position of the ref carries the kind), and
/// `x-<base64url-no-padding>` for a future opaque provider handle. Shell
/// metacharacters never appear in a ref.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderHandle {
    Wz(u64),
    Tx(u64),
    Opaque(String),
}

impl fmt::Display for ProviderHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderHandle::Wz(n) => write!(f, "wz-{n}"),
            ProviderHandle::Tx(n) => write!(f, "tx-{n}"),
            ProviderHandle::Opaque(s) => write!(f, "x-{s}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildKind {
    Group,
    Split,
}

/// A live, Space-scoped Group handle, valid only for one server epoch
/// (plan §2.8: durable child IDs are a later extension).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupRef {
    pub epoch: ServerEpoch,
    pub handle: ProviderHandle,
}

/// A live, Space-scoped Split handle; implies its Group (plan §6.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitRef {
    pub epoch: ServerEpoch,
    pub handle: ProviderHandle,
}

/// The durable identity core of a managed Space (plan §2.1): one immutable
/// owner, one immutable backend, one immutable identity, one mutable name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Space {
    pub space_uid: SpaceUid,
    pub space_no: SpaceNo,
    pub owner: HostUid,
    pub backend: Backend,
    pub backend_instance: BackendInstanceUid,
    pub name: String,
    pub lifecycle: Lifecycle,
    pub health: Health,
}

impl Space {
    /// The opaque, stable Wez workspace key (plan §2.4). The friendly name
    /// lives in the registry; this key never changes on logical rename.
    pub fn wez_workspace_key(&self) -> String {
        format!("dmux:{}:{}", self.owner.0, self.space_uid.0)
    }
}

/// Prefix of every reserved Wez workspace key (sentinel and Space keys).
/// The sentinel is `dmux:system:<epoch>` and is never a Space (ADR 002).
pub const WEZ_RESERVED_PREFIX: &str = "dmux:";
pub const WEZ_SENTINEL_PREFIX: &str = "dmux:system:";

/// Mutation-journal kinds — `operations.kind` in
/// `docs/adr/dmux/registry-v1.sql` (P2 additive extension).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Create,
    Rename,
    Remove,
    Adopt,
    Rebind,
    Normalize,
    Stamp,
}

impl OperationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            OperationKind::Create => "create",
            OperationKind::Rename => "rename",
            OperationKind::Remove => "remove",
            OperationKind::Adopt => "adopt",
            OperationKind::Rebind => "rebind",
            OperationKind::Normalize => "normalize",
            OperationKind::Stamp => "stamp",
        }
    }

    pub fn parse(token: &str) -> Option<Self> {
        Some(match token {
            "create" => OperationKind::Create,
            "rename" => OperationKind::Rename,
            "remove" => OperationKind::Remove,
            "adopt" => OperationKind::Adopt,
            "rebind" => OperationKind::Rebind,
            "normalize" => OperationKind::Normalize,
            "stamp" => OperationKind::Stamp,
            _ => return None,
        })
    }
}

impl fmt::Display for OperationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Mutation-journal states (plan §10.2) — `operations.operation_state` in
/// `docs/adr/dmux/registry-v1.sql` (P2 additive extension).
/// `prepared`/`running`/`unknown` are the unfinished states the partial
/// index `operations_one_unfinished_uq` counts; the rest are terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Prepared,
    Running,
    Unknown,
    Completed,
    Failed,
    Aborted,
    Conflict,
}

impl OperationState {
    pub fn as_str(self) -> &'static str {
        match self {
            OperationState::Prepared => "prepared",
            OperationState::Running => "running",
            OperationState::Unknown => "unknown",
            OperationState::Completed => "completed",
            OperationState::Failed => "failed",
            OperationState::Aborted => "aborted",
            OperationState::Conflict => "conflict",
        }
    }

    pub fn parse(token: &str) -> Option<Self> {
        Some(match token {
            "prepared" => OperationState::Prepared,
            "running" => OperationState::Running,
            "unknown" => OperationState::Unknown,
            "completed" => OperationState::Completed,
            "failed" => OperationState::Failed,
            "aborted" => OperationState::Aborted,
            "conflict" => OperationState::Conflict,
            _ => return None,
        })
    }

    /// Counted by `operations_one_unfinished_uq`: at most one such row per
    /// Space may exist at a time.
    pub fn is_unfinished(self) -> bool {
        matches!(
            self,
            OperationState::Prepared | OperationState::Running | OperationState::Unknown
        )
    }

    pub fn is_terminal(self) -> bool {
        !self.is_unfinished()
    }

    /// Legal journal transitions: an unfinished state may move to `running`,
    /// `unknown`, or any terminal state; nothing leaves a terminal state;
    /// nothing returns to `prepared`; a self-loop is not a transition.
    pub fn can_transition_to(self, to: OperationState) -> bool {
        self.is_unfinished() && to != OperationState::Prepared && to != self
    }
}

impl fmt::Display for OperationState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_tokens_serialize_to_plan_spellings() {
        for (value, token) in [
            (
                serde_json::to_value(Health::MultiWindow).unwrap(),
                "multi_window",
            ),
            (
                serde_json::to_value(Health::NativeKeyCollision).unwrap(),
                "native_key_collision",
            ),
            (
                serde_json::to_value(Lifecycle::Reserved).unwrap(),
                "reserved",
            ),
            (
                serde_json::to_value(Observation::Unreachable).unwrap(),
                "unreachable",
            ),
            (
                serde_json::to_value(ClientState::Detached).unwrap(),
                "detached",
            ),
            (serde_json::to_value(Backend::Wez).unwrap(), "wez"),
        ] {
            assert_eq!(value, serde_json::Value::String(token.into()));
        }
    }

    #[test]
    fn lifecycle_name_occupancy_matches_registry_partial_index() {
        let occupying = [
            Lifecycle::Reserved,
            Lifecycle::Active,
            Lifecycle::Deleting,
            Lifecycle::Conflict,
        ];
        let terminal = [Lifecycle::Deleted, Lifecycle::Aborted];
        for l in occupying {
            assert!(l.occupies_name() && !l.is_terminal(), "{l:?}");
        }
        for l in terminal {
            assert!(!l.occupies_name() && l.is_terminal(), "{l:?}");
        }
    }

    #[test]
    fn provider_handles_format_per_ref_grammar() {
        assert_eq!(ProviderHandle::Wz(3).to_string(), "wz-3");
        assert_eq!(ProviderHandle::Tx(7).to_string(), "tx-7");
        assert_eq!(ProviderHandle::Opaque("aGk".into()).to_string(), "x-aGk");
    }

    #[test]
    fn operation_tokens_match_registry_contract() {
        for kind in [
            OperationKind::Create,
            OperationKind::Rename,
            OperationKind::Remove,
            OperationKind::Adopt,
            OperationKind::Rebind,
            OperationKind::Normalize,
            OperationKind::Stamp,
        ] {
            assert_eq!(OperationKind::parse(kind.as_str()), Some(kind));
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                serde_json::Value::String(kind.as_str().into())
            );
        }
        for state in [
            OperationState::Prepared,
            OperationState::Running,
            OperationState::Unknown,
            OperationState::Completed,
            OperationState::Failed,
            OperationState::Aborted,
            OperationState::Conflict,
        ] {
            assert_eq!(OperationState::parse(state.as_str()), Some(state));
        }
        assert_eq!(OperationKind::parse("mkdir"), None);
        assert_eq!(OperationState::parse("done"), None);
    }

    #[test]
    fn operation_transition_matrix() {
        use OperationState::*;
        let unfinished = [Prepared, Running, Unknown];
        let terminal = [Completed, Failed, Aborted, Conflict];
        for from in unfinished {
            assert!(from.is_unfinished() && !from.is_terminal());
            // Never back to prepared, never a self-loop.
            assert!(!from.can_transition_to(Prepared));
            assert!(!from.can_transition_to(from));
            for to in terminal {
                assert!(from.can_transition_to(to), "{from:?} -> {to:?}");
            }
        }
        assert!(Prepared.can_transition_to(Running));
        assert!(Prepared.can_transition_to(Unknown));
        assert!(Running.can_transition_to(Unknown));
        assert!(Unknown.can_transition_to(Running));
        for from in terminal {
            assert!(from.is_terminal());
            for to in [
                Prepared, Running, Unknown, Completed, Failed, Aborted, Conflict,
            ] {
                assert!(!from.can_transition_to(to), "{from:?} -> {to:?}");
            }
        }
    }

    #[test]
    fn wez_workspace_key_is_opaque_and_rename_stable() {
        let host = HostUid(Uuid::nil());
        let space = SpaceUid(Uuid::max());
        let mut s = Space {
            space_uid: space,
            space_no: SpaceNo(NonZeroU64::new(2).unwrap()),
            owner: host,
            backend: Backend::Wez,
            backend_instance: BackendInstanceUid(Uuid::nil()),
            name: "dotfiles".into(),
            lifecycle: Lifecycle::Active,
            health: Health::Healthy,
        };
        let key = s.wez_workspace_key();
        assert!(key.starts_with(WEZ_RESERVED_PREFIX));
        assert!(!key.starts_with(WEZ_SENTINEL_PREFIX));
        s.name = "renamed".into();
        assert_eq!(s.wez_workspace_key(), key);
    }
}
