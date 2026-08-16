//! Provider contract: the trait every backend adapter implements and the
//! normalized result types it returns (plan §9.3, §8.1).
//!
//! Providers accept exact native locators and return normalized typed
//! results. They do NOT resolve names, select backends, write the registry,
//! choose routes, render user output, or call the other provider. Backend
//! selection, resolution, and policy live above this interface. Wez
//! presentation/disconnect is GUI orchestration, not a provider operation.
//!
//! Root-owned (plan §19 W1): this module and the conformance harness stay
//! with the root integrator; `tmux.rs`/`wez.rs` are specialist-owned in P3.

use serde::{Deserialize, Serialize};

use crate::model::{Backend, ProviderHandle, ServerEpoch};
pub mod tmux;
pub mod wez;

/// Typed inventory outcomes (plan §8.1, exhaustive). Only `Complete` or an
/// owner-local, identity-checked `ServerStopped` establishes zero live native
/// rows — and neither erases a durable registry match. A remote connection
/// failure is `Unreachable`, never proof a server is stopped/empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum InventoryOutcome {
    Complete(NativeInventory),
    /// Owner-local proof the selected server process is stopped: the
    /// service-recorded PID/start token no longer runs and the endpoint
    /// probe classified accordingly (ADR 001). Never inferred remotely.
    ServerStopped {
        detail: String,
    },
    Unreachable {
        detail: String,
    },
    AuthFailed {
        detail: String,
    },
    HostKeyIdentityFailed {
        detail: String,
    },
    CommandMissing {
        detail: String,
    },
    VersionMismatch {
        detail: String,
    },
    ProtocolMismatch {
        detail: String,
    },
    Malformed {
        detail: String,
    },
    /// Always dmux-imposed: the stock CLI can hang forever (ADR 001).
    Timeout {
        detail: String,
    },
    PermissionFailure {
        detail: String,
    },
}

impl InventoryOutcome {
    /// Determinate outcomes establish a definite zero-or-more row count.
    pub fn is_determinate(&self) -> bool {
        matches!(
            self,
            InventoryOutcome::Complete(_) | InventoryOutcome::ServerStopped { .. }
        )
    }
}

/// A complete owner-side scan of one backend instance under one epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeInventory {
    /// Epoch the scan was verified against (sentinel handshake for Wez,
    /// `@dmux_server_epoch` for tmux); None only for an unepoched/unmanaged
    /// server, whose children are unaddressable (plan §11.2).
    pub server_epoch: Option<ServerEpoch>,
    pub rows: Vec<NativeSpaceRow>,
}

/// One native Space-level resource: a Wez workspace (grouped by opaque key)
/// or a tmux session. The reserved sentinel is excluded before this layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeSpaceRow {
    /// Exact native token: Wez workspace key or tmux session id (`$N`).
    pub native_token: String,
    /// Native display name: Wez workspace name (same as token today) or the
    /// mutable tmux session name.
    pub native_name: String,
    pub groups: Vec<NativeGroupRow>,
    /// Wez one-window invariant check result (plan §2.3): true when the
    /// workspace spans more than one native mux window. Always false on tmux.
    pub multi_window: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeGroupRow {
    pub handle: ProviderHandle,
    pub title: Option<String>,
    pub splits: Vec<NativeSplitRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeSplitRow {
    pub handle: ProviderHandle,
    pub title: Option<String>,
    pub cwd: Option<String>,
}

/// Scope for an inventory scan. v1 has one managed instance per backend per
/// owner; the scope carries the exact endpoint identity to verify against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryScope {
    pub backend: Backend,
    /// Exact socket path (Wez service socket / tmux `-L` namespace socket).
    pub endpoint: String,
    /// Expected epoch when the caller already holds one; a mismatch is
    /// `backend_epoch_changed`, and returned native IDs are discarded.
    pub expected_epoch: Option<ServerEpoch>,
}

/// Exact creation order for one native Space (plan §8.2 step 9, ADR 004).
/// The provider spawns the bootstrap helper argv, never the user command
/// directly; the payload reaches the helper through the broker handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateSpec {
    /// Wez: opaque workspace key; tmux: exact session name.
    pub native_token: String,
    /// Owner-validated working directory (plan §11.3).
    pub cwd: Option<String>,
    /// Bootstrap helper argv, including the request UID (ADR 004).
    pub bootstrap_argv: Vec<String>,
}

/// Placement of a new Split on its split axis (plan §7.2 `--direction`).
/// `Down` is the CLI default and matches both backends' native default
/// orientation (tmux `split-window`, wez `split-pane --bottom`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    Left,
    Right,
    Up,
    Down,
}

/// Split creation order (plan §7.2): the shared bootstrap spec plus
/// placement. Adapters always emit the direction flag explicitly so the
/// native argv is deterministic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitSpec {
    pub spec: CreateSpec,
    pub direction: SplitDirection,
    /// New-pane size as a percentage of the split axis (1..=99); native
    /// default when absent.
    pub percent: Option<u8>,
}

impl From<CreateSpec> for SplitSpec {
    fn from(spec: CreateSpec) -> Self {
        SplitSpec {
            spec,
            direction: SplitDirection::Down,
            percent: None,
        }
    }
}

/// Deterministic multi-window merge plan (plan §10.3): every pane of every
/// extra window moves into the lowest-numbered window, ascending pane id.
/// The plan is shown for confirmation before `normalize_apply` runs it
/// under the caller's exclusive fence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizePlan {
    /// Opaque workspace key (wez) the plan was computed for.
    pub native_token: String,
    /// Epoch the plan is valid in; apply re-verifies it.
    pub server_epoch: ServerEpoch,
    pub target_window: u64,
    pub moves: Vec<NormalizeMove>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizeMove {
    pub pane_id: u64,
    pub from_window: u64,
}

/// A verified native binding returned by create/adopt (plan §9.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeBinding {
    pub native_token: String,
    pub server_epoch: ServerEpoch,
    pub root_group: ProviderHandle,
    pub root_split: ProviderHandle,
}

/// What "connect" needs, per backend. A tmux target becomes a locally
/// validated attach/switch or the dedicated `_attach` streaming channel
/// (plan §12.1); it is never sent over the bounded JSON mutation RPC. A Wez
/// target is consumed by GUI orchestration (bridge/`--launch-gui`), never
/// executed by the owner provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresentationTarget {
    Wez {
        /// Stable native domain name from the route registry (plan §12.3).
        domain: String,
        opaque_key: String,
        child: Option<(crate::model::ChildKind, ProviderHandle)>,
    },
    Tmux {
        /// Exact owner-generated argv; the client validates and execs it —
        /// it never builds or interpolates native target strings itself.
        exact_argv: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    pub backend: Backend,
    /// ADR 006 fork primitive availability (Wez adoption requires it).
    pub cas_rename: bool,
    /// tmux: exact IDs/options, client detach, passthrough all probed
    /// (plan §17), not inferred from a version string.
    pub probed: Vec<String>,
}

/// Typed provider-level failures. Mapped to `crate::error::ErrorCode` above
/// this layer; providers never render user output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// Epoch differed before/after a native-ID action; IDs discarded.
    EpochChanged {
        expected: ServerEpoch,
        observed: Option<ServerEpoch>,
    },
    WrongInstance {
        detail: String,
    },
    NotFound {
        native_ref: String,
    },
    /// One-window invariant violated (Wez): only listing/inspect,
    /// normalization, or confirmed whole-Space removal remain legal.
    MultiWindow {
        native_ref: String,
        window_count: u32,
    },
    NativeFailure {
        detail: String,
    },
    /// Post-mutation verification failed; the operation journal decides
    /// between retry-by-same-request-uid and `conflict`.
    PostconditionFailed {
        detail: String,
    },
    Timeout {
        detail: String,
    },
}

pub type ProviderResult<T> = Result<T, ProviderError>;

/// The provider contract (plan §9.3). Object-safe: orchestration holds
/// `Box<dyn Provider>` per backend instance.
pub trait Provider {
    fn capabilities(&self) -> Capabilities;

    fn inventory(&self, scope: &InventoryScope) -> InventoryOutcome;

    /// Create exactly one native Space resource and verify it (one Group,
    /// one Split, bootstrap helper running). Never retried blindly: replay
    /// is journal-driven with a complete keyed lookup first (plan §10.2).
    fn create(&self, scope: &InventoryScope, spec: &CreateSpec) -> ProviderResult<NativeBinding>;

    /// Validate the binding (and optional child) and return what the
    /// presentation layer needs. Read-only; never creates.
    fn prepare_presentation(
        &self,
        scope: &InventoryScope,
        binding: &NativeBinding,
        child: Option<&ProviderHandle>,
    ) -> ProviderResult<PresentationTarget>;

    /// Native rename where the backend has one (tmux session name). A Wez
    /// logical rename is registry-only (plan §2.5) and never reaches here;
    /// the Wez CAS rename exists solely for adoption/repair (ADR 006).
    fn rename(
        &self,
        scope: &InventoryScope,
        binding: &NativeBinding,
        new_native_name: &str,
    ) -> ProviderResult<()>;

    /// Remove exact native contents with bounded re-list/kill convergence
    /// and verified absence (plan §14, ADR 005). Non-convergence is an
    /// error, never a silent partial success.
    fn remove(&self, scope: &InventoryScope, binding: &NativeBinding) -> ProviderResult<()>;

    fn group_list(
        &self,
        scope: &InventoryScope,
        binding: &NativeBinding,
    ) -> ProviderResult<Vec<NativeGroupRow>>;
    fn group_new(
        &self,
        scope: &InventoryScope,
        binding: &NativeBinding,
        spec: &CreateSpec,
    ) -> ProviderResult<ProviderHandle>;
    fn group_activate(&self, scope: &InventoryScope, handle: &ProviderHandle)
    -> ProviderResult<()>;
    fn group_rename(
        &self,
        scope: &InventoryScope,
        handle: &ProviderHandle,
        title: &str,
    ) -> ProviderResult<()>;
    fn group_remove(&self, scope: &InventoryScope, handle: &ProviderHandle) -> ProviderResult<()>;

    fn split_list(
        &self,
        scope: &InventoryScope,
        group: &ProviderHandle,
    ) -> ProviderResult<Vec<NativeSplitRow>>;
    fn split_new(
        &self,
        scope: &InventoryScope,
        group: &ProviderHandle,
        spec: &SplitSpec,
    ) -> ProviderResult<ProviderHandle>;
    fn split_activate(&self, scope: &InventoryScope, handle: &ProviderHandle)
    -> ProviderResult<()>;
    fn split_remove(&self, scope: &InventoryScope, handle: &ProviderHandle) -> ProviderResult<()>;

    /// Wez-only (plan §10.3): compute the deterministic tab-to-window merge
    /// plan for a multi-window resource. Read-only. Backends without the
    /// concept (tmux never violates one-window) refuse with a typed error.
    fn normalize_plan(
        &self,
        _scope: &InventoryScope,
        native_token: &str,
    ) -> ProviderResult<NormalizePlan> {
        Err(ProviderError::NativeFailure {
            detail: format!("normalize_unsupported:{native_token}"),
        })
    }

    /// Apply a previously shown merge plan under the caller's exclusive
    /// fence and prove exactly one resulting window. A drifted epoch, a
    /// vanished pane, or a non-converging merge is an error, never a
    /// silent partial success (plan §10.3: quarantined, not half-managed).
    fn normalize_apply(&self, _scope: &InventoryScope, plan: &NormalizePlan) -> ProviderResult<()> {
        Err(ProviderError::NativeFailure {
            detail: format!("normalize_unsupported:{}", plan.native_token),
        })
    }

    fn inspect(
        &self,
        scope: &InventoryScope,
        binding: &NativeBinding,
    ) -> ProviderResult<NativeSpaceRow>;
}
