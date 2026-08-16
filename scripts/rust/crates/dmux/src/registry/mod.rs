//! Registry: transactions and the public registry API (plan §10).
//!
//! The storage contract is `docs/adr/dmux/registry-v1.sql`, implemented as
//! versioned migrations in [`schema`]. Crash-reconciliation decision
//! functions live in [`reconcile`]; [`sha256`] provides the dependency-free
//! digest every contract point uses.
//!
//! Connection discipline (contract header): every connection enables
//! foreign_keys=ON, journal_mode=WAL, synchronous=FULL, trusted_schema=OFF,
//! and a 5-second busy timeout in production. `SQLITE_BUSY` gets bounded
//! jittered retries for reads and short `BEGIN IMMEDIATE` transitions, then
//! becomes typed `registry_busy` with no native action started; a short
//! transition is never held across a backend call.
//!
//! Authority hash chain (plan §12.1) — the exact, documented formula:
//!
//! ```text
//! head_hash(revision N) =
//!   "sha256:" + lowercase_hex(SHA-256(
//!       parent_head_hash_utf8 ++ 0x0A
//!    ++ decimal_revision_utf8 ++ 0x0A
//!    ++ txn_uid_lowercase_hyphenated_utf8 ++ 0x0A))
//!
//! genesis (revision 0, no chain row) =
//!   "sha256:" + lowercase_hex(SHA-256(
//!       "dmux-authority-genesis" ++ 0x0A
//!    ++ registry_uid_lowercase_hyphenated_utf8 ++ 0x0A))
//! ```
//!
//! Revision-advance policy: identity/lifecycle/name/binding mutations,
//! backend-instance registration, and server-epoch publication
//! ([`Registry::publish_backend_server`] — which server incarnation is
//! authoritative is identity, exactly like registering the instance was)
//! advance the chain (one row per committed mutation transaction). Journal
//! state bookkeeping (operations and bootstrap_requests), lease
//! grants/renewals, and the RPC ledger do not advance it. Space health
//! updates ([`Registry::set_space_health`]) do not advance it either:
//! health is observation-derived (pane-stamp acknowledgements and scans,
//! plan §10.3), not identity — the identity-bearing adoption transition is
//! [`Registry::finalize_adopt`] itself, a lifecycle+binding mutation that
//! does advance the chain.

pub mod bootstrap_journal;
pub mod hosts;
pub mod reconcile;
pub mod recovery;
pub mod remote;
pub mod schema;
pub mod sha256;

pub use bootstrap_journal::{
    BootstrapRequestRow, bootstrap_can_transition, bootstrap_is_terminal, parse_bootstrap_state,
};
pub use hosts::{EnrolledHost, HostLifecycle, HostRow};
pub use remote::{
    AttachRedemption, AttachTokenSpec, NetworkClass, PeerCache, RedeemedAttach, RouteRow,
    RouteSpec, Transport,
};

use std::fmt;
use std::io;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use uuid::Uuid;

use crate::bootstrap::BootstrapState;
use crate::error::{ErrorCode, TypedError};
use crate::locks::{self, HeldLock, LockMode, LockScope};
use crate::model::{
    Backend, BackendInstanceUid, Health, HostUid, Lifecycle, Observation, OperationKind,
    OperationState, RegistryUid, ServerEpoch, SpaceNo, SpaceUid,
};

use self::sha256::sha256_hex;

// ---------------------------------------------------------------------------
// Errors

#[derive(Debug)]
pub enum RegistryError {
    /// Bounded retries exhausted on SQLITE_BUSY; no side effects occurred.
    Busy,
    /// `spaces_live_name_uq`: the exact name is occupied in a live lifecycle.
    NameConflict {
        name: String,
    },
    /// `operations_one_unfinished_uq`: one unfinished mutation per Space.
    OperationInProgress {
        space_uid: SpaceUid,
    },
    /// `bindings_current_native_uq`: the native token already has a current
    /// binding on this backend instance.
    NativeTokenConflict {
        native_token: String,
    },
    /// rpc request UID reused with a different method/payload digest.
    IdempotencyReuse {
        request_uid: Uuid,
    },
    /// A live lease holder owns the scope and no valid takeover proof was
    /// presented. Clock expiry alone never authorizes takeover.
    LeaseHeld {
        scope: String,
        holder_pid: Option<i32>,
    },
    /// The presented kernel lock does not pair with the lease scope
    /// (wrong scope, or not exclusive).
    KernelLockMismatch {
        scope: String,
    },
    /// Illegal journal transition.
    InvalidTransition {
        from: OperationState,
        to: OperationState,
    },
    /// The journal kind is not legal for the called API: a fresh
    /// reservation may journal only create/adopt/rebind
    /// ([`Registry::reserve_space_kind`]), and adoption finalization
    /// accepts only adopt/rebind rows ([`Registry::finalize_adopt`]).
    KindNotAllowed {
        kind: OperationKind,
        allowed: &'static str,
    },
    /// Illegal bootstrap-journal transition (plan §11.1, ADR 004; see
    /// [`bootstrap_journal`] for the matrix).
    InvalidBootstrapTransition {
        from: BootstrapState,
        to: BootstrapState,
    },
    /// `bootstrap_requests.request_uid` primary key: the request UID was
    /// already issued — two brokers claiming one request identity.
    BootstrapRequestExists {
        request_uid: Uuid,
    },
    /// `forget_host` targeting the local authority: `a` can never be
    /// forgotten (plan §12.2).
    LocalHostImmutable {
        host_uid: HostUid,
    },
    /// A host-ref spelling (alias or label), once used, is permanently
    /// bound to its first HostUid and never rebound (registry-v1.sql
    /// host_refs contract).
    SpellingBound {
        spelling: String,
        bound_to: HostUid,
    },
    /// Host labels are `[a-z][a-z0-9-]{0,31}` (plan §6.2).
    InvalidLabel {
        label: String,
    },
    /// `attach_tokens` token_hash/request_uid uniqueness: the token (or its
    /// request identity) was already issued. Request replay is the RPC
    /// ledger's job, never a re-issue.
    AttachTokenExists {
        request_uid: Uuid,
    },
    NotFound {
        what: String,
    },
    Corrupt(String),
    Io(io::Error),
    Sqlite(rusqlite::Error),
    Lock(locks::LockError),
}

impl RegistryError {
    /// The stable JSON error code (and via it the exit status) this maps to.
    pub fn error_code(&self) -> ErrorCode {
        match self {
            RegistryError::Busy => ErrorCode::RegistryBusy,
            RegistryError::NameConflict { .. } => ErrorCode::NameConflict,
            RegistryError::OperationInProgress { .. } | RegistryError::LeaseHeld { .. } => {
                ErrorCode::OperationInProgress
            }
            RegistryError::NativeTokenConflict { .. }
            | RegistryError::BootstrapRequestExists { .. }
            | RegistryError::SpellingBound { .. }
            | RegistryError::AttachTokenExists { .. } => ErrorCode::IdentityConflict,
            RegistryError::IdempotencyReuse { .. } => ErrorCode::IdempotencyReuse,
            RegistryError::NotFound { .. } => ErrorCode::NotFound,
            RegistryError::KindNotAllowed { .. } | RegistryError::LocalHostImmutable { .. } => {
                ErrorCode::Usage
            }
            RegistryError::InvalidLabel { .. } => ErrorCode::InvalidName,
            RegistryError::KernelLockMismatch { .. }
            | RegistryError::InvalidTransition { .. }
            | RegistryError::InvalidBootstrapTransition { .. }
            | RegistryError::Corrupt(_)
            | RegistryError::Io(_)
            | RegistryError::Sqlite(_)
            | RegistryError::Lock(_) => ErrorCode::OperationFailed,
        }
    }
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegistryError::Busy => write!(f, "registry busy after bounded retries"),
            RegistryError::NameConflict { name } => {
                write!(f, "logical name {name:?} is occupied by a live Space")
            }
            RegistryError::OperationInProgress { space_uid } => {
                write!(
                    f,
                    "an unfinished operation already exists for {}",
                    space_uid.0
                )
            }
            RegistryError::NativeTokenConflict { native_token } => {
                write!(
                    f,
                    "native token {native_token:?} already has a current binding"
                )
            }
            RegistryError::IdempotencyReuse { request_uid } => {
                write!(f, "request {request_uid} reused with different content")
            }
            RegistryError::LeaseHeld { scope, holder_pid } => {
                write!(
                    f,
                    "lease {scope:?} held by pid {holder_pid:?}; no takeover proof"
                )
            }
            RegistryError::KernelLockMismatch { scope } => {
                write!(
                    f,
                    "lease {scope:?} requires its paired exclusive kernel lock"
                )
            }
            RegistryError::InvalidTransition { from, to } => {
                write!(f, "illegal journal transition {from} -> {to}")
            }
            RegistryError::KindNotAllowed { kind, allowed } => {
                write!(
                    f,
                    "operation kind {kind} not allowed here (expected {allowed})"
                )
            }
            RegistryError::InvalidBootstrapTransition { from, to } => {
                write!(
                    f,
                    "illegal bootstrap transition {} -> {}",
                    from.as_str(),
                    to.as_str()
                )
            }
            RegistryError::BootstrapRequestExists { request_uid } => {
                write!(f, "bootstrap request {request_uid} was already issued")
            }
            RegistryError::LocalHostImmutable { host_uid } => {
                write!(
                    f,
                    "host {} is the local authority ('a') and cannot be forgotten",
                    host_uid.0
                )
            }
            RegistryError::SpellingBound { spelling, bound_to } => {
                write!(
                    f,
                    "spelling {spelling:?} is permanently bound to host {}",
                    bound_to.0
                )
            }
            RegistryError::InvalidLabel { label } => {
                write!(
                    f,
                    "invalid host label {label:?} (want [a-z][a-z0-9-]{{0,31}})"
                )
            }
            RegistryError::AttachTokenExists { request_uid } => {
                write!(
                    f,
                    "attach token for request {request_uid} was already issued"
                )
            }
            RegistryError::NotFound { what } => write!(f, "{what} not found"),
            RegistryError::Corrupt(msg) => write!(f, "registry corrupt: {msg}"),
            RegistryError::Io(e) => write!(f, "registry i/o: {e}"),
            RegistryError::Sqlite(e) => write!(f, "sqlite: {e}"),
            RegistryError::Lock(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RegistryError {}

/// The trait-boundary mapping (e.g. [`crate::bootstrap::BootstrapJournal`]):
/// the stable code from [`RegistryError::error_code`] plus the display text.
impl From<RegistryError> for TypedError {
    fn from(e: RegistryError) -> TypedError {
        TypedError::new(e.error_code(), e.to_string())
    }
}

impl From<io::Error> for RegistryError {
    fn from(e: io::Error) -> Self {
        RegistryError::Io(e)
    }
}

impl From<locks::LockError> for RegistryError {
    fn from(e: locks::LockError) -> Self {
        RegistryError::Lock(e)
    }
}

impl From<rusqlite::Error> for RegistryError {
    fn from(e: rusqlite::Error) -> Self {
        if is_busy(&e) {
            return RegistryError::Busy;
        }
        if let rusqlite::Error::SqliteFailure(failure, Some(message)) = &e {
            if failure.code == rusqlite::ErrorCode::ConstraintViolation {
                // SQLite names either the index or the indexed columns in
                // its UNIQUE-violation message depending on index shape;
                // match both spellings.
                if message.contains("spaces_live_name_uq")
                    || message.contains("spaces.logical_name")
                {
                    return RegistryError::NameConflict {
                        name: String::new(),
                    };
                }
                if message.contains("operations_one_unfinished_uq")
                    || message.contains("operations.space_uid")
                {
                    return RegistryError::OperationInProgress {
                        space_uid: SpaceUid(Uuid::nil()),
                    };
                }
                if message.contains("bindings_current_native_uq")
                    || message.contains("native_bindings.native_token")
                    || message.contains("native_bindings.space_uid")
                {
                    return RegistryError::NativeTokenConflict {
                        native_token: String::new(),
                    };
                }
            }
        }
        RegistryError::Sqlite(e)
    }
}

fn is_busy(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(f, _)
            if f.code == rusqlite::ErrorCode::DatabaseBusy
                || f.code == rusqlite::ErrorCode::DatabaseLocked
    )
}

pub type Result<T> = std::result::Result<T, RegistryError>;

// ---------------------------------------------------------------------------
// Configuration

/// SQLITE_BUSY policy: the per-statement busy timeout plus bounded jittered
/// retries around short `BEGIN IMMEDIATE` transitions. Production default is
/// the contract's 5000 ms timeout; tests shrink it.
#[derive(Debug, Clone)]
pub struct BusyPolicy {
    pub busy_timeout: Duration,
    pub attempts: u32,
    pub retry_base: Duration,
}

impl Default for BusyPolicy {
    fn default() -> Self {
        BusyPolicy {
            busy_timeout: Duration::from_millis(5000),
            attempts: 4,
            retry_base: Duration::from_millis(25),
        }
    }
}

/// Every API takes explicit paths: the SQLite file and the directory the
/// kernel-lock files live in. Production uses [`RegistryConfig::production`];
/// tests inject scratch directories and never touch real state.
#[derive(Debug, Clone)]
pub struct RegistryConfig {
    pub db_path: PathBuf,
    pub lock_dir: PathBuf,
    pub busy: BusyPolicy,
}

impl RegistryConfig {
    pub fn new(db_path: impl Into<PathBuf>, lock_dir: impl Into<PathBuf>) -> Self {
        RegistryConfig {
            db_path: db_path.into(),
            lock_dir: lock_dir.into(),
            busy: BusyPolicy::default(),
        }
    }

    /// Production locations: `$XDG_DATA_HOME/dmux/registry.sqlite3` (dir
    /// 0700, db 0600) and kernel locks under the secure runtime dir.
    pub fn production() -> io::Result<Self> {
        Ok(RegistryConfig::new(
            production_db_path().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "neither XDG_DATA_HOME nor HOME is set",
                )
            })?,
            crate::runtime::dmux_runtime_dir()?,
        ))
    }
}

/// `$XDG_DATA_HOME/dmux/registry.sqlite3`, falling back to
/// `~/.local/share/dmux/registry.sqlite3`. Never inside synced dotfiles.
pub fn production_db_path() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME")?).join(".local/share"),
    };
    Some(base.join("dmux/registry.sqlite3"))
}

// ---------------------------------------------------------------------------
// Row types

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryIdentity {
    pub host_uid: HostUid,
    pub registry_uid: RegistryUid,
    pub schema_version: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpaceReservation {
    pub space_uid: SpaceUid,
    pub space_no: SpaceNo,
    pub operation_uid: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceRow {
    pub space_uid: SpaceUid,
    pub owner: HostUid,
    pub space_no: SpaceNo,
    pub backend_instance: BackendInstanceUid,
    pub logical_name: String,
    pub lifecycle: Lifecycle,
    pub health: Health,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeKind {
    WezWorkspaceKey,
    TmuxSessionId,
}

impl NativeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            NativeKind::WezWorkspaceKey => "wez_workspace_key",
            NativeKind::TmuxSessionId => "tmux_session_id",
        }
    }

    pub fn parse(token: &str) -> Option<Self> {
        Some(match token {
            "wez_workspace_key" => NativeKind::WezWorkspaceKey,
            "tmux_session_id" => NativeKind::TmuxSessionId,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingState {
    Current,
    Superseded,
    Severed,
}

impl BindingState {
    pub fn as_str(self) -> &'static str {
        match self {
            BindingState::Current => "current",
            BindingState::Superseded => "superseded",
            BindingState::Severed => "severed",
        }
    }

    pub fn parse(token: &str) -> Option<Self> {
        Some(match token {
            "current" => BindingState::Current,
            "superseded" => BindingState::Superseded,
            "severed" => BindingState::Severed,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeBindingSpec {
    pub native_token: String,
    pub native_kind: NativeKind,
    pub server_epoch: Option<ServerEpoch>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingRow {
    pub binding_id: i64,
    pub space_uid: SpaceUid,
    pub native_token: String,
    pub native_kind: NativeKind,
    pub binding_state: BindingState,
    pub observation: Observation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationRow {
    pub operation_uid: Uuid,
    pub space_uid: SpaceUid,
    pub kind: OperationKind,
    pub state: OperationState,
    pub request_uid: Uuid,
    pub payload_json: String,
    pub fencing_token: Option<i64>,
    pub started_at: String,
    pub updated_at: String,
    pub finished_at: Option<String>,
}

/// The published server incarnation of a backend instance
/// (`backend_instances.server_*`/`socket_*`; ADR 001/002).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendServerRecord {
    pub server_epoch: Option<ServerEpoch>,
    pub server_pid: Option<i64>,
    pub server_start_token: Option<String>,
    pub socket_dev: Option<i64>,
    pub socket_ino: Option<i64>,
}

/// The static registration half of a backend instance — what
/// [`Registry::register_backend_instance`] recorded (the incarnation half
/// lives in [`BackendServerRecord`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendInstanceInfo {
    pub backend: Backend,
    pub owner: HostUid,
    /// wez: exact service socket; tmux: `-L` namespace.
    pub socket_path: Option<String>,
    /// systemd unit / launchd label.
    pub service_label: Option<String>,
    pub created_at: String,
}

/// One epoch-scoped pane stamp acknowledgement (plan §10.3). The health
/// recompute over these rows stays caller-side (operations layer); the
/// registry only records and lists them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneStampRow {
    pub space_uid: SpaceUid,
    pub server_epoch: ServerEpoch,
    /// Canonical provider handle string, e.g. `tx-13` / `wz-42`.
    pub pane_handle: String,
    pub stamped_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorityHead {
    pub revision: u64,
    pub head_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionRecord {
    pub revision: u64,
    pub parent_head_hash: String,
    pub head_hash: String,
    pub txn_uid: Uuid,
    pub committed_at: String,
}

/// A remotely presented lineage claim to classify against the recorded chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentedLineage {
    pub registry_uid: RegistryUid,
    pub revision: u64,
    pub head_hash: String,
    /// True when the presenter claims this is its CURRENT head (a fresh
    /// nonce-bound hello), false for a possibly stale in-flight response.
    pub claimed_current: bool,
}

/// Plan §12.1 classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineageClassification {
    /// Matches the recorded current head exactly.
    OkCurrent,
    /// A lower revision that lies on the recorded chain: merely stale,
    /// never regresses the cache.
    OkStaleAncestor,
    /// Different RegistryUid, or same revision with a different head.
    LineageConflict,
    /// A claimed current head that is lower than, or not verifiable as a
    /// descendant of, the recorded head — rollback/clone quarantine input.
    RollbackSuspect,
}

// ---------------------------------------------------------------------------
// Leases

/// Database lease scopes per the contract's `lease_scopes.scope` strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseScope {
    Backend(BackendInstanceUid),
    Recovery(BackendInstanceUid),
    Snapshot(BackendInstanceUid),
    Decision { owner: HostUid, name_sha256: String },
    Maintenance,
}

impl LeaseScope {
    pub fn decision(owner: HostUid, exact_name: &str) -> LeaseScope {
        LeaseScope::Decision {
            owner,
            name_sha256: sha256_hex(exact_name.as_bytes()),
        }
    }

    /// The exact scope string stored in `lease_scopes.scope`.
    pub fn as_scope_string(&self) -> String {
        match self {
            LeaseScope::Backend(uid) => format!("backend:{}", uid.0),
            LeaseScope::Recovery(uid) => format!("recovery:{}", uid.0),
            LeaseScope::Snapshot(uid) => format!("snapshot:{}", uid.0),
            LeaseScope::Decision { owner, name_sha256 } => {
                format!("decision:{}:{}", owner.0, name_sha256)
            }
            LeaseScope::Maintenance => "maintenance".into(),
        }
    }

    /// The kernel lock this database scope pairs with (plan §10.1:
    /// recovery/snapshot are database scopes over the same backend-instance
    /// kernel lock; maintenance is the exclusive authority gate).
    fn kernel_matches(&self, kernel: &HeldLock) -> bool {
        if kernel.mode() != LockMode::Exclusive {
            return false;
        }
        match (self, kernel.scope()) {
            (
                LeaseScope::Backend(uid) | LeaseScope::Recovery(uid) | LeaseScope::Snapshot(uid),
                LockScope::BackendInstance(k),
            ) => uid == k,
            (
                LeaseScope::Decision { owner, name_sha256 },
                LockScope::Decision {
                    owner: k_owner,
                    name_sha256: k_sha,
                },
            ) => owner == k_owner && name_sha256 == k_sha,
            (LeaseScope::Maintenance, LockScope::AuthorityGate) => true,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseHolder {
    pub request_uid: Uuid,
    pub pid: i32,
    pub start_token: String,
    pub boot_id: Option<String>,
}

impl LeaseHolder {
    /// A holder describing the current process.
    pub fn current(request_uid: Uuid) -> Self {
        LeaseHolder {
            request_uid,
            pid: std::process::id() as i32,
            start_token: process_start_token(),
            boot_id: current_boot_id(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    pub lease_id: i64,
    pub scope: String,
    pub holder_request_uid: Uuid,
    pub fencing_token: i64,
    pub holder_pid: Option<i32>,
    pub holder_start_token: Option<String>,
    pub expires_at: String,
    pub state: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HolderLiveness {
    Alive,
    Dead,
}

/// `kill(pid, 0)` probe: EPERM still means alive; only ESRCH proves gone.
pub fn probe_pid(pid: i32) -> HolderLiveness {
    let rc = unsafe { libc::kill(pid, 0) };
    if rc == 0 {
        return HolderLiveness::Alive;
    }
    match io::Error::last_os_error().raw_os_error() {
        Some(code) if code == libc::ESRCH => HolderLiveness::Dead,
        _ => HolderLiveness::Alive,
    }
}

/// Takeover evidence per plan §10.2 step 2: the prior holder's recorded
/// PID/start token no longer owns a process. Clock expiry alone never
/// authorizes takeover; the caller must also hold the paired kernel lock,
/// which proves the predecessor cannot later resume native work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TakeoverProof {
    pub prior_pid: i32,
    pub liveness: HolderLiveness,
}

/// Stable per-process start token (used to disambiguate PID reuse).
pub fn process_start_token() -> String {
    use std::sync::OnceLock;
    static TOKEN: OnceLock<String> = OnceLock::new();
    TOKEN.get_or_init(|| Uuid::new_v4().to_string()).clone()
}

/// Linux boot ID where available; None elsewhere.
pub fn current_boot_id() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .ok()
        .map(|s| s.trim().to_string())
}

// ---------------------------------------------------------------------------
// RPC idempotency

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcResultState {
    Final,
    Unknown,
}

impl RpcResultState {
    pub fn as_str(self) -> &'static str {
        match self {
            RpcResultState::Final => "final",
            RpcResultState::Unknown => "unknown",
        }
    }

    pub fn parse(token: &str) -> Option<Self> {
        Some(match token {
            "final" => RpcResultState::Final,
            "unknown" => RpcResultState::Unknown,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RpcDisposition {
    /// First sighting of this request UID; the ledger row is recorded
    /// (result_state `unknown`) and the operation may proceed.
    New,
    /// Same UID + same method/digest: replay. `Final` returns the stored
    /// result; `Unknown` means the prior attempt is resumable.
    Replay {
        result_state: RpcResultState,
        result_json: Option<serde_json::Value>,
    },
}

// ---------------------------------------------------------------------------
// Hash chain

/// Genesis head hash for revision 0 (see module docs for the formula).
pub fn genesis_head_hash(registry_uid: RegistryUid) -> String {
    format!(
        "sha256:{}",
        sha256_hex(format!("dmux-authority-genesis\n{}\n", registry_uid.0).as_bytes())
    )
}

/// Head hash for revision `revision` (see module docs for the formula).
pub fn chain_head_hash(parent_head_hash: &str, revision: u64, txn_uid: &Uuid) -> String {
    format!(
        "sha256:{}",
        sha256_hex(format!("{parent_head_hash}\n{revision}\n{txn_uid}\n").as_bytes())
    )
}

// ---------------------------------------------------------------------------
// Registry

pub struct Registry {
    conn: Connection,
    config: RegistryConfig,
}

impl Registry {
    /// Create-or-open the registry at the explicit configured path.
    ///
    /// First run initializes identity (HostUid v4, RegistryUid v4, counters)
    /// exactly once even under concurrent first-run: schema work happens
    /// under the exclusive authority-gate kernel lock (the maintenance mode,
    /// which overlaps nothing), and the meta insert itself is additionally
    /// guarded by a `BEGIN IMMEDIATE` existence check.
    pub fn open(config: RegistryConfig) -> Result<Registry> {
        use std::os::unix::fs::DirBuilderExt;

        if let Some(parent) = config.db_path.parent()
            && !parent.as_os_str().is_empty()
            && !parent.exists()
        {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
        }
        if !config.lock_dir.exists() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&config.lock_dir)?;
        }

        let existed = config.db_path.exists();
        let conn = Connection::open(&config.db_path)?;
        if !existed {
            let _ = std::fs::set_permissions(
                &config.db_path,
                std::os::unix::fs::PermissionsExt::from_mode(0o600),
            );
        }

        // Connection settings, with bounded jittered retries: a concurrent
        // first-run's WAL switch needs a brief write lock.
        let mut attempt = 0u32;
        let mode = loop {
            match schema::apply_connection_settings(&conn, config.busy.busy_timeout) {
                Ok(mode) => break mode,
                Err(e) if is_busy(&e) && attempt < 32 => {
                    attempt += 1;
                    std::thread::sleep(backoff_delay(Duration::from_millis(5), attempt));
                }
                Err(e) => return Err(e.into()),
            }
        };
        if mode.to_ascii_lowercase() != "wal" {
            return Err(RegistryError::Corrupt(format!(
                "journal_mode is {mode:?}, not wal"
            )));
        }

        let mut registry = Registry { conn, config };
        registry.ensure_schema()?;
        Ok(registry)
    }

    fn ensure_schema(&mut self) -> Result<()> {
        if schema::user_version(&self.conn)? >= schema::SCHEMA_VERSION && self.meta_present()? {
            return Ok(());
        }
        // Maintenance: the exclusive authority gate overlaps nothing.
        let gate = locks::acquire(
            &self.config.lock_dir,
            LockScope::AuthorityGate,
            LockMode::Exclusive,
        )?;
        if schema::user_version(&self.conn)? < schema::SCHEMA_VERSION {
            // On an upgrade of an existing registry, additionally hold the
            // 'maintenance' database lease with an advanced fencing token.
            // On a fresh database the lease tables do not exist yet; the
            // exclusive gate alone serializes first-run.
            let upgrading = self.table_exists("lease_scopes")?;
            let holder = LeaseHolder::current(Uuid::new_v4());
            if upgrading {
                self.acquire_lease(
                    &LeaseScope::Maintenance,
                    &holder,
                    Duration::from_secs(300),
                    &gate,
                    None,
                )?;
            }
            schema::migrate(&mut self.conn)?;
            if upgrading {
                self.release_lease(&LeaseScope::Maintenance, holder.request_uid)?;
            }
        }
        self.init_meta_if_missing()?;
        drop(gate);
        Ok(())
    }

    fn table_exists(&self, name: &str) -> Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
            [name],
            |row| row.get(0),
        )?;
        Ok(n > 0)
    }

    fn meta_present(&self) -> Result<bool> {
        if !self.table_exists("meta")? {
            return Ok(false);
        }
        let n: i64 = self
            .conn
            .query_row("SELECT count(*) FROM meta", [], |row| row.get(0))?;
        Ok(n == 1)
    }

    fn init_meta_if_missing(&mut self) -> Result<()> {
        let host_uid = Uuid::new_v4();
        let registry_uid = RegistryUid(Uuid::new_v4());
        let genesis = genesis_head_hash(registry_uid);
        self.immediate(|tx| {
            let n: i64 = tx.query_row("SELECT count(*) FROM meta", [], |row| row.get(0))?;
            if n == 0 {
                let now = now_rfc3339();
                tx.execute(
                    "INSERT INTO meta (id, schema_version, host_uid, registry_uid, \
                     authority_revision, authority_head_hash, space_no_counter, created_at) \
                     VALUES (1, ?1, ?2, ?3, 0, ?4, 1, ?5)",
                    params![
                        schema::SCHEMA_VERSION,
                        host_uid.to_string(),
                        registry_uid.0.to_string(),
                        genesis,
                        now
                    ],
                )?;
                tx.execute(
                    "INSERT INTO hosts (host_uid, lifecycle, enrolled_at) \
                     VALUES (?1, 'enrolled', ?2)",
                    params![host_uid.to_string(), now],
                )?;
                // `a` always means the local authority (plan §6.2).
                tx.execute(
                    "INSERT INTO host_refs (ref_kind, spelling, host_uid, state, created_at, changed_at) \
                     VALUES ('alias', 'a', ?1, 'current', ?2, ?2)",
                    params![host_uid.to_string(), now],
                )?;
            }
            Ok(())
        })
    }

    /// Diagnostics/tests only. Production callers use the typed API.
    #[doc(hidden)]
    pub fn raw_connection(&self) -> &Connection {
        &self.conn
    }

    // -- short IMMEDIATE transitions with bounded jittered busy retries ----

    fn immediate<T>(
        &mut self,
        mut f: impl FnMut(&rusqlite::Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let attempts = self.config.busy.attempts.max(1);
        let base = self.config.busy.retry_base;
        for attempt in 0..attempts {
            if attempt > 0 {
                std::thread::sleep(backoff_delay(base, attempt));
            }
            let tx = match self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
            {
                Ok(tx) => tx,
                Err(e) if is_busy(&e) => continue,
                Err(e) => return Err(e.into()),
            };
            match f(&tx) {
                Ok(value) => match tx.commit() {
                    Ok(()) => return Ok(value),
                    Err(e) if is_busy(&e) => continue,
                    Err(e) => return Err(e.into()),
                },
                Err(RegistryError::Busy) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(RegistryError::Busy)
    }

    // -- identity ----------------------------------------------------------

    pub fn identity(&self) -> Result<RegistryIdentity> {
        self.conn
            .query_row(
                "SELECT host_uid, registry_uid, schema_version, created_at FROM meta WHERE id = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| RegistryError::Corrupt("meta row missing".into()))
            .and_then(|(host, reg, version, created)| {
                Ok(RegistryIdentity {
                    host_uid: HostUid(parse_uuid(&host)?),
                    registry_uid: RegistryUid(parse_uuid(&reg)?),
                    schema_version: version,
                    created_at: created,
                })
            })
    }

    // -- authority revision chain -----------------------------------------

    pub fn authority_head(&self) -> Result<AuthorityHead> {
        let (revision, head_hash): (i64, String) = self.conn.query_row(
            "SELECT authority_revision, authority_head_hash FROM meta WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok(AuthorityHead {
            revision: revision as u64,
            head_hash,
        })
    }

    pub fn revision_chain(&self) -> Result<Vec<RevisionRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT revision, parent_head_hash, head_hash, txn_uid, committed_at \
             FROM authority_revisions ORDER BY revision",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        let mut chain = Vec::new();
        for row in rows {
            let (revision, parent, head, txn, committed) = row?;
            chain.push(RevisionRecord {
                revision: revision as u64,
                parent_head_hash: parent,
                head_hash: head,
                txn_uid: parse_uuid(&txn)?,
                committed_at: committed,
            });
        }
        Ok(chain)
    }

    /// Classify a presented `(registry_uid, revision, head_hash)` against
    /// the recorded chain (plan §12.1; see [`LineageClassification`]).
    pub fn classify_lineage(&self, presented: &PresentedLineage) -> Result<LineageClassification> {
        let identity = self.identity()?;
        if presented.registry_uid != identity.registry_uid {
            return Ok(LineageClassification::LineageConflict);
        }
        let head = self.authority_head()?;
        if presented.revision > head.revision {
            // The presenter knows a successor this chain cannot verify:
            // the recorded registry may itself be rolled back or cloned.
            return Ok(LineageClassification::RollbackSuspect);
        }
        let recorded = if presented.revision == 0 {
            genesis_head_hash(identity.registry_uid)
        } else {
            self.conn
                .query_row(
                    "SELECT head_hash FROM authority_revisions WHERE revision = ?1",
                    [presented.revision as i64],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .ok_or_else(|| {
                    RegistryError::Corrupt(format!(
                        "revision {} missing from recorded chain",
                        presented.revision
                    ))
                })?
        };
        if recorded != presented.head_hash {
            // Same revision, different head: divergent lineage.
            return Ok(LineageClassification::LineageConflict);
        }
        if presented.revision == head.revision {
            return Ok(LineageClassification::OkCurrent);
        }
        if presented.claimed_current {
            // A fresh claimed-current head at a lower revision.
            return Ok(LineageClassification::RollbackSuspect);
        }
        Ok(LineageClassification::OkStaleAncestor)
    }

    // -- backend instances -------------------------------------------------

    /// Resolve this authority's already-registered instance for `backend`
    /// without creating or mutating anything. The one-per-owner unique index
    /// makes zero-or-one rows a storage invariant; corrupt UUID text is never
    /// accepted as an identity.
    pub fn backend_instance_for_backend(
        &self,
        backend: Backend,
    ) -> Result<Option<BackendInstanceUid>> {
        self.conn
            .query_row(
                "SELECT b.backend_instance_uid FROM backend_instances AS b \
                 JOIN meta AS m ON m.id = 1 AND m.host_uid = b.owner_host_uid \
                 WHERE b.backend = ?1",
                [backend.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .as_deref()
            .map(|uid| parse_uuid(uid).map(BackendInstanceUid))
            .transpose()
    }

    /// Get-or-create the single managed instance for `backend` on this
    /// owner (plan §2.15; `backend_instances_one_per_owner_uq`).
    pub fn register_backend_instance(
        &mut self,
        backend: Backend,
        socket_path: Option<&str>,
        service_label: Option<&str>,
    ) -> Result<BackendInstanceUid> {
        let fresh = Uuid::new_v4();
        self.immediate(|tx| {
            let owner: String =
                tx.query_row("SELECT host_uid FROM meta WHERE id = 1", [], |row| row.get(0))?;
            if let Some(existing) = tx
                .query_row(
                    "SELECT backend_instance_uid FROM backend_instances \
                     WHERE owner_host_uid = ?1 AND backend = ?2",
                    params![owner, backend.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
            {
                return Ok(BackendInstanceUid(parse_uuid(&existing)?));
            }
            let now = now_rfc3339();
            tx.execute(
                "INSERT INTO backend_instances \
                 (backend_instance_uid, owner_host_uid, backend, socket_path, service_label, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    fresh.to_string(),
                    owner,
                    backend.as_str(),
                    socket_path,
                    service_label,
                    now
                ],
            )?;
            advance_revision(tx, &now)?;
            Ok(BackendInstanceUid(fresh))
        })
    }

    /// Publish a server incarnation for a managed backend instance: the
    /// current epoch plus its PID/start-token and socket dev/ino witnesses
    /// (ADR 001/002 replacement detection). Overwrites all five columns —
    /// a restart with a fresh epoch fully replaces the previous incarnation
    /// (the old epoch is gone; stale-ref invalidation stays caller-side).
    ///
    /// Advances the authority revision chain: which server incarnation is
    /// authoritative for an instance is identity, exactly like registering
    /// the instance was (module-docs advance policy).
    pub fn publish_backend_server(
        &mut self,
        instance: BackendInstanceUid,
        epoch: ServerEpoch,
        pid: Option<i64>,
        start_token: Option<&str>,
        socket_dev: Option<i64>,
        socket_ino: Option<i64>,
    ) -> Result<()> {
        self.immediate(|tx| {
            let now = now_rfc3339();
            let changed = tx.execute(
                "UPDATE backend_instances SET server_epoch = ?2, server_pid = ?3, \
                 server_start_token = ?4, socket_dev = ?5, socket_ino = ?6 \
                 WHERE backend_instance_uid = ?1",
                params![
                    instance.0.to_string(),
                    epoch.0.to_string(),
                    pid,
                    start_token,
                    socket_dev,
                    socket_ino
                ],
            )?;
            if changed != 1 {
                return Err(RegistryError::NotFound {
                    what: format!("backend instance {}", instance.0),
                });
            }
            advance_revision(tx, &now)?;
            Ok(())
        })
    }

    /// Read back the published server incarnation for an instance
    /// (all-`None` when stopped/never published).
    pub fn backend_server(&self, instance: BackendInstanceUid) -> Result<BackendServerRecord> {
        self.conn
            .query_row(
                "SELECT server_epoch, server_pid, server_start_token, socket_dev, socket_ino \
                 FROM backend_instances WHERE backend_instance_uid = ?1",
                [instance.0.to_string()],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| RegistryError::NotFound {
                what: format!("backend instance {}", instance.0),
            })
            .and_then(|(epoch, pid, token, dev, ino)| {
                Ok(BackendServerRecord {
                    server_epoch: epoch
                        .as_deref()
                        .map(|e| parse_uuid(e).map(ServerEpoch))
                        .transpose()?,
                    server_pid: pid,
                    server_start_token: token,
                    socket_dev: dev,
                    socket_ino: ino,
                })
            })
    }

    /// The static registration record for an instance: backend kind plus
    /// the socket/service endpoints the provider scope is constructed from.
    pub fn backend_instance_info(
        &self,
        instance: BackendInstanceUid,
    ) -> Result<BackendInstanceInfo> {
        self.conn
            .query_row(
                "SELECT backend, owner_host_uid, socket_path, service_label, created_at \
                 FROM backend_instances WHERE backend_instance_uid = ?1",
                [instance.0.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| RegistryError::NotFound {
                what: format!("backend instance {}", instance.0),
            })
            .and_then(|(backend, owner, socket, service, created)| {
                Ok(BackendInstanceInfo {
                    backend: token_enum(&backend)?,
                    owner: HostUid(parse_uuid(&owner)?),
                    socket_path: socket,
                    service_label: service,
                    created_at: created,
                })
            })
    }

    // -- pane stamps (plan §10.3) -------------------------------------------

    /// Record (or refresh) one pane's stamp acknowledgement for a Space
    /// under a server epoch — an upsert on the
    /// `(space_uid, server_epoch, pane_handle)` key that refreshes
    /// `stamped_at`. Observation-derived diagnostics, exactly like
    /// [`Registry::set_space_health`]: never advances the authority
    /// revision. The Space must exist (typed [`RegistryError::NotFound`]).
    pub fn record_pane_stamp(
        &mut self,
        space_uid: SpaceUid,
        server_epoch: ServerEpoch,
        pane_handle: &str,
    ) -> Result<()> {
        let handle = pane_handle.to_string();
        self.immediate(|tx| {
            let now = now_rfc3339();
            tx.execute(
                "INSERT INTO pane_stamps (space_uid, server_epoch, pane_handle, stamped_at) \
                 VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(space_uid, server_epoch, pane_handle) \
                 DO UPDATE SET stamped_at = excluded.stamped_at",
                params![
                    space_uid.0.to_string(),
                    server_epoch.0.to_string(),
                    handle,
                    now
                ],
            )
            .map_err(|e| match e {
                rusqlite::Error::SqliteFailure(f, Some(ref message))
                    if f.code == rusqlite::ErrorCode::ConstraintViolation
                        && message.contains("FOREIGN KEY") =>
                {
                    RegistryError::NotFound {
                        what: format!("space {}", space_uid.0),
                    }
                }
                other => other.into(),
            })?;
            Ok(())
        })
    }

    /// Every stamp acknowledgement for a Space under one server epoch —
    /// the input to the caller-side health recompute. Stamps from other
    /// epochs are never returned.
    pub fn pane_stamps(
        &self,
        space_uid: SpaceUid,
        server_epoch: ServerEpoch,
    ) -> Result<Vec<PaneStampRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT pane_handle, stamped_at FROM pane_stamps \
             WHERE space_uid = ?1 AND server_epoch = ?2 ORDER BY pane_handle",
        )?;
        let rows = stmt.query_map(
            params![space_uid.0.to_string(), server_epoch.0.to_string()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )?;
        let mut stamps = Vec::new();
        for row in rows {
            let (pane_handle, stamped_at) = row?;
            stamps.push(PaneStampRow {
                space_uid,
                server_epoch,
                pane_handle,
                stamped_at,
            });
        }
        Ok(stamps)
    }

    // -- identity allocation ----------------------------------------------

    /// Reserve identity for a new Space: SpaceUid (UUIDv7) plus the next
    /// SpaceNo from the meta counter, committed as `lifecycle=reserved` with
    /// a `create/prepared` journal row (plan §10.2 create step 1).
    /// A failed creation later consumes its number via [`abort_create`];
    /// numbers and UIDs are never reused.
    pub fn reserve_space(
        &mut self,
        name: &str,
        backend_instance: BackendInstanceUid,
        request_uid: Uuid,
    ) -> Result<SpaceReservation> {
        self.reserve_space_kind(name, backend_instance, request_uid, OperationKind::Create)
    }

    /// [`Registry::reserve_space`] with an explicit journal kind: `create`
    /// for `dmux new`, `adopt`/`rebind` for the explicit external-adoption
    /// entry points (plan §10.3), which reserve identity exactly like a
    /// create but must be reconciled by their own resume duty. Kinds that
    /// operate on an EXISTING Space (rename/remove/normalize/stamp) make no
    /// sense for a fresh reservation and are rejected with the typed
    /// [`RegistryError::KindNotAllowed`], no side effects.
    pub fn reserve_space_kind(
        &mut self,
        name: &str,
        backend_instance: BackendInstanceUid,
        request_uid: Uuid,
        kind: OperationKind,
    ) -> Result<SpaceReservation> {
        match kind {
            OperationKind::Create | OperationKind::Adopt | OperationKind::Rebind => {}
            OperationKind::Rename
            | OperationKind::Remove
            | OperationKind::Normalize
            | OperationKind::Stamp => {
                return Err(RegistryError::KindNotAllowed {
                    kind,
                    allowed: "create/adopt/rebind",
                });
            }
        }
        let space_uid = SpaceUid(Uuid::now_v7());
        let operation_uid = Uuid::new_v4();
        let payload = serde_json::json!({
            "name": name,
            "backend_instance": backend_instance.0.to_string(),
        })
        .to_string();
        self.immediate(|tx| {
            let now = now_rfc3339();
            let owner: String =
                tx.query_row("SELECT host_uid FROM meta WHERE id = 1", [], |row| {
                    row.get(0)
                })?;
            let no: i64 = tx.query_row(
                "SELECT space_no_counter FROM meta WHERE id = 1",
                [],
                |row| row.get(0),
            )?;
            tx.execute("UPDATE meta SET space_no_counter = ?1", [no + 1])?;
            tx.execute(
                "INSERT INTO spaces (space_uid, owner_host_uid, space_no, backend_instance_id, \
                 logical_name, lifecycle, health, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, 'reserved', 'unknown', ?6, ?6)",
                params![
                    space_uid.0.to_string(),
                    owner,
                    no,
                    backend_instance.0.to_string(),
                    name,
                    now
                ],
            )
            .map_err(|e| match RegistryError::from(e) {
                RegistryError::NameConflict { .. } => RegistryError::NameConflict {
                    name: name.to_string(),
                },
                other => other,
            })?;
            tx.execute(
                "INSERT INTO operations (operation_uid, space_uid, kind, operation_state, \
                 request_uid, payload_json, started_at, updated_at) \
                 VALUES (?1, ?2, ?3, 'prepared', ?4, ?5, ?6, ?6)",
                params![
                    operation_uid.to_string(),
                    space_uid.0.to_string(),
                    kind.as_str(),
                    request_uid.to_string(),
                    payload,
                    now
                ],
            )?;
            advance_revision(tx, &now)?;
            let space_no = NonZeroU64::new(no as u64)
                .ok_or_else(|| RegistryError::Corrupt("space_no_counter reached 0".into()))?;
            Ok(SpaceReservation {
                space_uid,
                space_no: SpaceNo(space_no),
                operation_uid,
            })
        })
    }

    /// Complete a create: stamp/bind the verified live native resource,
    /// mark the Space active, finish the journal row (plan §10.2 create
    /// steps 4–5). A dmux-created resource starts fully stamped: `healthy`.
    pub fn finalize_create(
        &mut self,
        space_uid: SpaceUid,
        operation_uid: Uuid,
        binding: &NativeBindingSpec,
    ) -> Result<()> {
        self.finalize_reservation(
            space_uid,
            operation_uid,
            binding,
            &[OperationKind::Create],
            "create",
            Health::Healthy,
        )
    }

    /// Complete an adoption or expert rebind: bind the verified native
    /// resource and mark the Space `active`, but `health = unstamped`
    /// (plan §10.3): after stamping/binding, an adopted Space stays
    /// active+live+unstamped until a complete scan proves every live pane
    /// acknowledged its stamp — [`Registry::set_space_health`] flips it to
    /// `healthy` only then. Requires the reservation's journal row to be
    /// kind adopt/rebind; anything else is the typed
    /// [`RegistryError::KindNotAllowed`].
    pub fn finalize_adopt(
        &mut self,
        space_uid: SpaceUid,
        operation_uid: Uuid,
        binding: &NativeBindingSpec,
    ) -> Result<()> {
        self.finalize_reservation(
            space_uid,
            operation_uid,
            binding,
            &[OperationKind::Adopt, OperationKind::Rebind],
            "adopt/rebind",
            Health::Unstamped,
        )
    }

    /// Shared finalization for reservation-consuming kinds: reserved →
    /// active with the given health, current binding inserted, journal row
    /// completed, revision advanced — one transaction.
    fn finalize_reservation(
        &mut self,
        space_uid: SpaceUid,
        operation_uid: Uuid,
        binding: &NativeBindingSpec,
        allowed: &[OperationKind],
        allowed_names: &'static str,
        health: Health,
    ) -> Result<()> {
        self.immediate(|tx| {
            let now = now_rfc3339();
            require_unfinished_op_of(tx, operation_uid, space_uid, allowed, allowed_names)?;
            let changed = tx.execute(
                "UPDATE spaces SET lifecycle = 'active', health = ?3, updated_at = ?2 \
                 WHERE space_uid = ?1 AND lifecycle = 'reserved'",
                params![space_uid.0.to_string(), now, health_token(health)],
            )?;
            if changed != 1 {
                return Err(RegistryError::NotFound {
                    what: format!("reserved space {}", space_uid.0),
                });
            }
            let instance: String = tx.query_row(
                "SELECT backend_instance_id FROM spaces WHERE space_uid = ?1",
                [space_uid.0.to_string()],
                |row| row.get(0),
            )?;
            tx.execute(
                "INSERT INTO native_bindings (space_uid, backend_instance_id, native_token, \
                 native_kind, binding_state, server_epoch, observation, observed_at, bound_at) \
                 VALUES (?1, ?2, ?3, ?4, 'current', ?5, 'live', ?6, ?6)",
                params![
                    space_uid.0.to_string(),
                    instance,
                    binding.native_token,
                    binding.native_kind.as_str(),
                    binding.server_epoch.map(|e| e.0.to_string()),
                    now
                ],
            )
            .map_err(|e| match RegistryError::from(e) {
                RegistryError::NativeTokenConflict { .. } => RegistryError::NativeTokenConflict {
                    native_token: binding.native_token.clone(),
                },
                other => other,
            })?;
            finish_op(tx, operation_uid, OperationState::Completed, &now)?;
            advance_revision(tx, &now)?;
            Ok(())
        })
    }

    /// Abort a reservation (create, adopt, or rebind): it becomes
    /// `aborted`, permanently consuming its SpaceUid and SpaceNo (gaps are
    /// intentional).
    pub fn abort_create(&mut self, space_uid: SpaceUid, operation_uid: Uuid) -> Result<()> {
        self.immediate(|tx| {
            let now = now_rfc3339();
            require_unfinished_op_of(
                tx,
                operation_uid,
                space_uid,
                &[
                    OperationKind::Create,
                    OperationKind::Adopt,
                    OperationKind::Rebind,
                ],
                "create/adopt/rebind",
            )?;
            let changed = tx.execute(
                "UPDATE spaces SET lifecycle = 'aborted', updated_at = ?2 \
                 WHERE space_uid = ?1 AND lifecycle = 'reserved'",
                params![space_uid.0.to_string(), now],
            )?;
            if changed != 1 {
                return Err(RegistryError::NotFound {
                    what: format!("reserved space {}", space_uid.0),
                });
            }
            finish_op(tx, operation_uid, OperationState::Aborted, &now)?;
            advance_revision(tx, &now)?;
            Ok(())
        })
    }

    // -- health --------------------------------------------------------------

    /// Set a Space's health (e.g. `unstamped` → `healthy` once a complete
    /// scan proves every live pane acknowledged its stamp, plan §10.3, or
    /// `healthy` → `multi_window` on a one-window-invariant violation).
    /// Valid only for non-terminal lifecycles: a `deleted`/`aborted` row is
    /// immutable history and rejects with [`RegistryError::NotFound`].
    /// Advances `updated_at`.
    ///
    /// Revision policy (pinned by test): health does NOT advance the
    /// authority chain. It is observation-derived state — pane-stamp
    /// acknowledgements and scans — like binding observations and journal
    /// bookkeeping, not identity; the identity-bearing adoption transition
    /// is [`Registry::finalize_adopt`], which does advance.
    pub fn set_space_health(&mut self, space_uid: SpaceUid, health: Health) -> Result<()> {
        self.immediate(|tx| {
            let now = now_rfc3339();
            let changed = tx.execute(
                "UPDATE spaces SET health = ?2, updated_at = ?3 \
                 WHERE space_uid = ?1 \
                   AND lifecycle IN ('reserved','active','deleting','conflict')",
                params![space_uid.0.to_string(), health_token(health), now],
            )?;
            if changed != 1 {
                return Err(RegistryError::NotFound {
                    what: format!("non-terminal space {}", space_uid.0),
                });
            }
            Ok(())
        })
    }

    // -- generic journal ----------------------------------------------------

    /// Record a `prepared` journal row for `kind` on an existing Space.
    /// The partial index `operations_one_unfinished_uq` enforces one
    /// unfinished operation per Space; a violation surfaces as
    /// [`RegistryError::OperationInProgress`].
    pub fn begin_operation(
        &mut self,
        space_uid: SpaceUid,
        kind: OperationKind,
        request_uid: Uuid,
        payload: &serde_json::Value,
    ) -> Result<Uuid> {
        let operation_uid = Uuid::new_v4();
        let payload = payload.to_string();
        self.immediate(|tx| {
            let now = now_rfc3339();
            insert_operation(
                tx,
                operation_uid,
                space_uid,
                kind,
                request_uid,
                &payload,
                &now,
            )?;
            advance_revision(tx, &now)?;
            Ok(operation_uid)
        })
    }

    /// Move a journal row along the legal transition matrix
    /// (`OperationState::can_transition_to`); anything else is
    /// [`RegistryError::InvalidTransition`].
    pub fn transition_operation(&mut self, operation_uid: Uuid, to: OperationState) -> Result<()> {
        self.immediate(|tx| {
            let now = now_rfc3339();
            let from = op_state(tx, operation_uid)?;
            if !from.can_transition_to(to) {
                return Err(RegistryError::InvalidTransition { from, to });
            }
            finish_op(tx, operation_uid, to, &now)?;
            Ok(())
        })
    }

    /// The single unfinished journal row for a Space, if any — the crash
    /// reconciliation entry point: feed its kind/state to
    /// [`reconcile::resume_duty`].
    pub fn unfinished_operation(&self, space_uid: SpaceUid) -> Result<Option<OperationRow>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {OP_COLUMNS} FROM operations \
                     WHERE space_uid = ?1 \
                       AND operation_state IN ('prepared','running','unknown')"
                ),
                [space_uid.0.to_string()],
                map_operation_row,
            )
            .optional()?
            .map(finish_operation_row)
            .transpose()
    }

    pub fn operation(&self, operation_uid: Uuid) -> Result<OperationRow> {
        self.conn
            .query_row(
                &format!("SELECT {OP_COLUMNS} FROM operations WHERE operation_uid = ?1"),
                [operation_uid.to_string()],
                map_operation_row,
            )
            .optional()?
            .map(finish_operation_row)
            .transpose()?
            .ok_or_else(|| RegistryError::NotFound {
                what: format!("operation {operation_uid}"),
            })
    }

    // -- rename -------------------------------------------------------------

    /// Record rename intent: old/new names in the journal payload
    /// (plan §10.2 rename step 1). Fails on a live-name conflict up front.
    pub fn begin_rename(
        &mut self,
        space_uid: SpaceUid,
        new_name: &str,
        request_uid: Uuid,
    ) -> Result<Uuid> {
        let operation_uid = Uuid::new_v4();
        self.immediate(|tx| {
            let now = now_rfc3339();
            let (old_name, instance): (String, String) = tx
                .query_row(
                    "SELECT logical_name, backend_instance_id FROM spaces \
                     WHERE space_uid = ?1 AND lifecycle IN ('reserved','active')",
                    [space_uid.0.to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?
                .ok_or_else(|| RegistryError::NotFound {
                    what: format!("live space {}", space_uid.0),
                })?;
            let taken: i64 = tx.query_row(
                "SELECT count(*) FROM spaces \
                 WHERE backend_instance_id = ?1 AND logical_name = ?2 \
                   AND lifecycle IN ('reserved','active','deleting','conflict') \
                   AND space_uid <> ?3",
                params![instance, new_name, space_uid.0.to_string()],
                |row| row.get(0),
            )?;
            if taken > 0 {
                return Err(RegistryError::NameConflict {
                    name: new_name.to_string(),
                });
            }
            let payload = serde_json::json!({ "old": old_name, "new": new_name }).to_string();
            insert_operation(
                tx,
                operation_uid,
                space_uid,
                OperationKind::Rename,
                request_uid,
                &payload,
                &now,
            )?;
            advance_revision(tx, &now)?;
            Ok(operation_uid)
        })
    }

    /// Commit the registry side of a rename: update the current name,
    /// append `space_name_history`, complete the journal row. Identity
    /// (SpaceUid/SpaceNo/backend/owner) never changes.
    pub fn commit_rename(&mut self, space_uid: SpaceUid, operation_uid: Uuid) -> Result<()> {
        self.immediate(|tx| {
            let now = now_rfc3339();
            let row = require_unfinished_op(tx, operation_uid, space_uid, OperationKind::Rename)?;
            let payload: serde_json::Value = serde_json::from_str(&row.payload_json)
                .map_err(|e| RegistryError::Corrupt(format!("rename payload: {e}")))?;
            let (old_name, new_name) = match (payload["old"].as_str(), payload["new"].as_str()) {
                (Some(old), Some(new)) => (old.to_string(), new.to_string()),
                _ => {
                    return Err(RegistryError::Corrupt(
                        "rename payload missing old/new".into(),
                    ));
                }
            };
            let changed = tx.execute(
                "UPDATE spaces SET logical_name = ?2, updated_at = ?3 WHERE space_uid = ?1",
                params![space_uid.0.to_string(), new_name, now],
            )
            .map_err(|e| match RegistryError::from(e) {
                RegistryError::NameConflict { .. } => RegistryError::NameConflict {
                    name: new_name.clone(),
                },
                other => other,
            })?;
            if changed != 1 {
                return Err(RegistryError::NotFound {
                    what: format!("space {}", space_uid.0),
                });
            }
            tx.execute(
                "INSERT INTO space_name_history (space_uid, old_name, new_name, operation_uid, changed_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    space_uid.0.to_string(),
                    old_name,
                    new_name,
                    operation_uid.to_string(),
                    now
                ],
            )?;
            finish_op(tx, operation_uid, OperationState::Completed, &now)?;
            advance_revision(tx, &now)?;
            Ok(())
        })
    }

    // -- remove --------------------------------------------------------------

    /// Record `deleting` intent BEFORE killing anything (plan §10.2 remove
    /// step 1).
    pub fn begin_remove(&mut self, space_uid: SpaceUid, request_uid: Uuid) -> Result<Uuid> {
        let operation_uid = Uuid::new_v4();
        self.immediate(|tx| {
            let now = now_rfc3339();
            let changed = tx.execute(
                "UPDATE spaces SET lifecycle = 'deleting', updated_at = ?2 \
                 WHERE space_uid = ?1 AND lifecycle IN ('reserved','active')",
                params![space_uid.0.to_string(), now],
            )?;
            if changed != 1 {
                return Err(RegistryError::NotFound {
                    what: format!("live space {}", space_uid.0),
                });
            }
            insert_operation(
                tx,
                operation_uid,
                space_uid,
                OperationKind::Remove,
                request_uid,
                &serde_json::json!({}).to_string(),
                &now,
            )?;
            advance_revision(tx, &now)?;
            Ok(operation_uid)
        })
    }

    /// Only after the caller VERIFIED native absence: mark `deleted`, sever
    /// the current binding, retain the tombstone forever (plan §10.2 remove
    /// step 4). Rows are never deleted; UIDs/numbers never reused.
    pub fn complete_remove(&mut self, space_uid: SpaceUid, operation_uid: Uuid) -> Result<()> {
        self.immediate(|tx| {
            let now = now_rfc3339();
            require_unfinished_op(tx, operation_uid, space_uid, OperationKind::Remove)?;
            let changed = tx.execute(
                "UPDATE spaces SET lifecycle = 'deleted', deleted_at = ?2, updated_at = ?2 \
                 WHERE space_uid = ?1 AND lifecycle = 'deleting'",
                params![space_uid.0.to_string(), now],
            )?;
            if changed != 1 {
                return Err(RegistryError::NotFound {
                    what: format!("deleting space {}", space_uid.0),
                });
            }
            tx.execute(
                "UPDATE native_bindings SET binding_state = 'severed', observation = 'absent', \
                 observed_at = ?2 WHERE space_uid = ?1 AND binding_state = 'current'",
                params![space_uid.0.to_string(), now],
            )?;
            finish_op(tx, operation_uid, OperationState::Completed, &now)?;
            advance_revision(tx, &now)?;
            Ok(())
        })
    }

    // -- space queries -------------------------------------------------------

    pub fn space(&self, space_uid: SpaceUid) -> Result<SpaceRow> {
        self.conn
            .query_row(
                &format!("SELECT {SPACE_COLUMNS} FROM spaces WHERE space_uid = ?1"),
                [space_uid.0.to_string()],
                map_space_row,
            )
            .optional()?
            .map(finish_space_row)
            .transpose()?
            .ok_or_else(|| RegistryError::NotFound {
                what: format!("space {}", space_uid.0),
            })
    }

    pub fn spaces(&self) -> Result<Vec<SpaceRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {SPACE_COLUMNS} FROM spaces ORDER BY space_no"
        ))?;
        let rows = stmt.query_map([], map_space_row)?;
        let mut spaces = Vec::new();
        for row in rows {
            spaces.push(finish_space_row(row?)?);
        }
        Ok(spaces)
    }

    /// The one live-lifecycle row for `(instance, name)` if any — the
    /// occupancy `spaces_live_name_uq` guards.
    pub fn live_space_by_name(
        &self,
        backend_instance: BackendInstanceUid,
        name: &str,
    ) -> Result<Option<SpaceRow>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {SPACE_COLUMNS} FROM spaces \
                     WHERE backend_instance_id = ?1 AND logical_name = ?2 \
                       AND lifecycle IN ('reserved','active','deleting','conflict')"
                ),
                params![backend_instance.0.to_string(), name],
                map_space_row,
            )
            .optional()?
            .map(finish_space_row)
            .transpose()
    }

    pub fn current_binding(&self, space_uid: SpaceUid) -> Result<Option<BindingRow>> {
        self.conn
            .query_row(
                "SELECT binding_id, space_uid, native_token, native_kind, binding_state, observation \
                 FROM native_bindings WHERE space_uid = ?1 AND binding_state = 'current'",
                [space_uid.0.to_string()],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()?
            .map(|(id, space, token, kind, state, observation)| {
                Ok(BindingRow {
                    binding_id: id,
                    space_uid: SpaceUid(parse_uuid(&space)?),
                    native_token: token,
                    native_kind: NativeKind::parse(&kind)
                        .ok_or_else(|| RegistryError::Corrupt(format!("native_kind {kind:?}")))?,
                    binding_state: BindingState::parse(&state)
                        .ok_or_else(|| RegistryError::Corrupt(format!("binding_state {state:?}")))?,
                    observation: token_enum(&observation)?,
                })
            })
            .transpose()
    }

    // -- rpc idempotency ledger ---------------------------------------------

    /// Record (or replay) an RPC request. Same UID + same method/digest
    /// replays the stored disposition; reuse with different content is
    /// rejected (plan §12.1).
    pub fn record_rpc_request(
        &mut self,
        request_uid: Uuid,
        method: &str,
        payload_sha256: &str,
    ) -> Result<RpcDisposition> {
        self.immediate(|tx| {
            let existing = tx
                .query_row(
                    "SELECT method, payload_sha256, result_state, result_json \
                     FROM rpc_requests WHERE request_uid = ?1",
                    [request_uid.to_string()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<String>>(3)?,
                        ))
                    },
                )
                .optional()?;
            match existing {
                None => {
                    tx.execute(
                        "INSERT INTO rpc_requests (request_uid, method, payload_sha256, \
                         result_state, received_at) VALUES (?1, ?2, ?3, 'unknown', ?4)",
                        params![
                            request_uid.to_string(),
                            method,
                            payload_sha256,
                            now_rfc3339()
                        ],
                    )?;
                    Ok(RpcDisposition::New)
                }
                Some((m, digest, state, json)) => {
                    if m != method || digest != payload_sha256 {
                        return Err(RegistryError::IdempotencyReuse { request_uid });
                    }
                    let result_state = RpcResultState::parse(&state)
                        .ok_or_else(|| RegistryError::Corrupt(format!("result_state {state:?}")))?;
                    let result_json = json
                        .map(|j| {
                            serde_json::from_str(&j)
                                .map_err(|e| RegistryError::Corrupt(format!("stored result: {e}")))
                        })
                        .transpose()?;
                    Ok(RpcDisposition::Replay {
                        result_state,
                        result_json,
                    })
                }
            }
        })
    }

    /// Bind the final result to the request UID; later replays return it.
    pub fn finish_rpc_request(
        &mut self,
        request_uid: Uuid,
        result: &serde_json::Value,
        operation_uid: Option<Uuid>,
    ) -> Result<()> {
        let result = result.to_string();
        self.immediate(|tx| {
            let changed = tx.execute(
                "UPDATE rpc_requests SET result_state = 'final', result_json = ?2, \
                 operation_uid = COALESCE(?3, operation_uid), finished_at = ?4 \
                 WHERE request_uid = ?1",
                params![
                    request_uid.to_string(),
                    result,
                    operation_uid.map(|u| u.to_string()),
                    now_rfc3339()
                ],
            )?;
            if changed != 1 {
                return Err(RegistryError::NotFound {
                    what: format!("rpc request {request_uid}"),
                });
            }
            Ok(())
        })
    }

    // -- leases --------------------------------------------------------------

    /// Acquire (or resume, or take over) the database lease for `scope`.
    ///
    /// The caller must present the paired EXCLUSIVE kernel lock — that lock
    /// is the non-stealable exclusion; the lease row records ownership and
    /// hands out the next monotonically increasing fencing token from
    /// `lease_scopes.last_fencing_token`.
    ///
    /// Takeover from a different prior holder additionally requires
    /// [`TakeoverProof`] that the recorded holder PID is gone. Clock expiry
    /// alone never authorizes takeover: an expired-but-alive holder still
    /// refuses (plan §10.2 steps 1–3).
    pub fn acquire_lease(
        &mut self,
        scope: &LeaseScope,
        holder: &LeaseHolder,
        ttl: Duration,
        kernel: &HeldLock,
        takeover: Option<&TakeoverProof>,
    ) -> Result<Lease> {
        let scope_string = scope.as_scope_string();
        if !scope.kernel_matches(kernel) {
            return Err(RegistryError::KernelLockMismatch {
                scope: scope_string,
            });
        }
        self.immediate(|tx| {
            let now = now_rfc3339();
            let expires = rfc3339_utc(SystemTime::now() + ttl);
            tx.execute(
                "INSERT OR IGNORE INTO lease_scopes (scope, last_fencing_token) VALUES (?1, 0)",
                [&scope_string],
            )?;
            let held = tx
                .query_row(
                    "SELECT lease_id, holder_request_uid, holder_pid FROM leases \
                     WHERE scope = ?1 AND state = 'held'",
                    [&scope_string],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<i32>>(2)?,
                        ))
                    },
                )
                .optional()?;
            if let Some((lease_id, holder_request, holder_pid)) = held {
                if parse_uuid(&holder_request)? == holder.request_uid {
                    // Same-request resume/renew: same fencing token.
                    tx.execute(
                        "UPDATE leases SET renewed_at = ?2, expires_at = ?3 WHERE lease_id = ?1",
                        params![lease_id, now, expires],
                    )?;
                    return read_lease(tx, lease_id);
                }
                let proven_dead = matches!(
                    (takeover, holder_pid),
                    (
                        Some(TakeoverProof {
                            prior_pid,
                            liveness: HolderLiveness::Dead,
                        }),
                        Some(recorded)
                    ) if *prior_pid == recorded
                );
                if !proven_dead {
                    return Err(RegistryError::LeaseHeld {
                        scope: scope_string.clone(),
                        holder_pid,
                    });
                }
                tx.execute(
                    "UPDATE leases SET state = 'superseded' WHERE lease_id = ?1",
                    [lease_id],
                )?;
            }
            let token: i64 = tx.query_row(
                "SELECT last_fencing_token FROM lease_scopes WHERE scope = ?1",
                [&scope_string],
                |row| row.get(0),
            )?;
            let token = token + 1;
            tx.execute(
                "UPDATE lease_scopes SET last_fencing_token = ?2 WHERE scope = ?1",
                params![scope_string, token],
            )?;
            tx.execute(
                "INSERT INTO leases (scope, holder_request_uid, fencing_token, holder_pid, \
                 holder_start_token, boot_id, expires_at, renewed_at, state) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'held')",
                params![
                    scope_string,
                    holder.request_uid.to_string(),
                    token,
                    holder.pid,
                    holder.start_token,
                    holder.boot_id,
                    expires,
                    now
                ],
            )?;
            let lease_id = tx.last_insert_rowid();
            read_lease(tx, lease_id)
        })
    }

    /// Renew the held lease (same holder, same fencing token).
    pub fn renew_lease(
        &mut self,
        scope: &LeaseScope,
        holder_request_uid: Uuid,
        ttl: Duration,
    ) -> Result<Lease> {
        let scope_string = scope.as_scope_string();
        self.immediate(|tx| {
            let expires = rfc3339_utc(SystemTime::now() + ttl);
            let now = now_rfc3339();
            let lease_id: Option<i64> = tx
                .query_row(
                    "SELECT lease_id FROM leases \
                     WHERE scope = ?1 AND state = 'held' AND holder_request_uid = ?2",
                    params![scope_string, holder_request_uid.to_string()],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(lease_id) = lease_id else {
                return Err(RegistryError::NotFound {
                    what: format!("held lease {scope_string} for {holder_request_uid}"),
                });
            };
            tx.execute(
                "UPDATE leases SET renewed_at = ?2, expires_at = ?3 WHERE lease_id = ?1",
                params![lease_id, now, expires],
            )?;
            read_lease(tx, lease_id)
        })
    }

    pub fn release_lease(&mut self, scope: &LeaseScope, holder_request_uid: Uuid) -> Result<()> {
        let scope_string = scope.as_scope_string();
        self.immediate(|tx| {
            let changed = tx.execute(
                "UPDATE leases SET state = 'released' \
                 WHERE scope = ?1 AND state = 'held' AND holder_request_uid = ?2",
                params![scope_string, holder_request_uid.to_string()],
            )?;
            if changed != 1 {
                return Err(RegistryError::NotFound {
                    what: format!("held lease {scope_string} for {holder_request_uid}"),
                });
            }
            Ok(())
        })
    }

    pub fn current_lease(&self, scope: &LeaseScope) -> Result<Option<Lease>> {
        let scope_string = scope.as_scope_string();
        self.conn
            .query_row(
                "SELECT lease_id FROM leases WHERE scope = ?1 AND state = 'held'",
                [&scope_string],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .map(|lease_id| {
                self.conn
                    .query_row(
                        &format!("SELECT {LEASE_COLUMNS} FROM leases WHERE lease_id = ?1"),
                        [lease_id],
                        map_lease_row,
                    )
                    .optional()?
                    .map(finish_lease_row)
                    .transpose()?
                    .ok_or_else(|| RegistryError::Corrupt("lease vanished".into()))
            })
            .transpose()
    }

    /// The last fencing token handed out for a scope (0 if never granted).
    pub fn last_fencing_token(&self, scope: &LeaseScope) -> Result<i64> {
        Ok(self
            .conn
            .query_row(
                "SELECT last_fencing_token FROM lease_scopes WHERE scope = ?1",
                [scope.as_scope_string()],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0))
    }

    // -- backup --------------------------------------------------------------

    /// WAL-safe online backup: a checked WAL checkpoint (bounded retries on
    /// busy, result verified) followed by SQLite's online backup API — never
    /// a file copy of the main database alone. The destination is
    /// integrity-checked before returning.
    pub fn backup_to(&self, dest: &Path) -> Result<BackupReport> {
        let mut checkpoint_attempts = 0u32;
        let (wal_frames, checkpointed) = loop {
            let result: std::result::Result<(i64, i64, i64), rusqlite::Error> = self
                .conn
                .query_row("PRAGMA wal_checkpoint(FULL)", [], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                });
            match result {
                Ok((0, wal, ckpt)) => break (wal, ckpt),
                Ok((_busy, _, _)) => {}
                Err(e) if is_busy(&e) => {}
                Err(e) => return Err(e.into()),
            }
            checkpoint_attempts += 1;
            if checkpoint_attempts >= self.config.busy.attempts.max(8) {
                return Err(RegistryError::Busy);
            }
            std::thread::sleep(backoff_delay(
                self.config.busy.retry_base,
                checkpoint_attempts,
            ));
        };

        let existed = dest.exists();
        let mut dst = Connection::open(dest)?;
        if !existed {
            let _ =
                std::fs::set_permissions(dest, std::os::unix::fs::PermissionsExt::from_mode(0o600));
        }
        {
            let backup = rusqlite::backup::Backup::new(&self.conn, &mut dst)?;
            backup.run_to_completion(64, Duration::from_millis(5), None)?;
        }
        let verdict: String = dst.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if verdict != "ok" {
            return Err(RegistryError::Corrupt(format!(
                "backup integrity_check: {verdict}"
            )));
        }
        Ok(BackupReport {
            checkpoint_attempts,
            wal_frames,
            checkpointed_frames: checkpointed,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackupReport {
    /// Busy retries the checked checkpoint needed before succeeding.
    pub checkpoint_attempts: u32,
    /// WAL frame count reported by the checkpoint.
    pub wal_frames: i64,
    /// Frames actually checkpointed (equals `wal_frames` when clean).
    pub checkpointed_frames: i64,
}

// ---------------------------------------------------------------------------
// Internal SQL helpers

const OP_COLUMNS: &str = "operation_uid, space_uid, kind, operation_state, request_uid, \
                          payload_json, fencing_token, started_at, updated_at, finished_at";
const SPACE_COLUMNS: &str = "space_uid, owner_host_uid, space_no, backend_instance_id, \
                             logical_name, lifecycle, health, created_at, updated_at, deleted_at";
const LEASE_COLUMNS: &str = "lease_id, scope, holder_request_uid, fencing_token, holder_pid, \
                             holder_start_token, expires_at, state";

type RawOperationRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    Option<i64>,
    String,
    String,
    Option<String>,
);

fn map_operation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawOperationRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
    ))
}

fn finish_operation_row(raw: RawOperationRow) -> Result<OperationRow> {
    let (op, space, kind, state, request, payload, fence, started, updated, finished) = raw;
    Ok(OperationRow {
        operation_uid: parse_uuid(&op)?,
        space_uid: SpaceUid(parse_uuid(&space)?),
        kind: OperationKind::parse(&kind)
            .ok_or_else(|| RegistryError::Corrupt(format!("operation kind {kind:?}")))?,
        state: OperationState::parse(&state)
            .ok_or_else(|| RegistryError::Corrupt(format!("operation state {state:?}")))?,
        request_uid: parse_uuid(&request)?,
        payload_json: payload,
        fencing_token: fence,
        started_at: started,
        updated_at: updated,
        finished_at: finished,
    })
}

type RawSpaceRow = (
    String,
    String,
    i64,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
);

fn map_space_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawSpaceRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
    ))
}

fn finish_space_row(raw: RawSpaceRow) -> Result<SpaceRow> {
    let (space, owner, no, instance, name, lifecycle, health, created, updated, deleted) = raw;
    Ok(SpaceRow {
        space_uid: SpaceUid(parse_uuid(&space)?),
        owner: HostUid(parse_uuid(&owner)?),
        space_no: SpaceNo(
            NonZeroU64::new(no as u64)
                .ok_or_else(|| RegistryError::Corrupt(format!("space_no {no}")))?,
        ),
        backend_instance: BackendInstanceUid(parse_uuid(&instance)?),
        logical_name: name,
        lifecycle: token_enum(&lifecycle)?,
        health: token_enum(&health)?,
        created_at: created,
        updated_at: updated,
        deleted_at: deleted,
    })
}

type RawLeaseRow = (
    i64,
    String,
    String,
    i64,
    Option<i32>,
    Option<String>,
    String,
    String,
);

fn map_lease_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawLeaseRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    ))
}

fn finish_lease_row(raw: RawLeaseRow) -> Result<Lease> {
    let (lease_id, scope, request, token, pid, start, expires, state) = raw;
    Ok(Lease {
        lease_id,
        scope,
        holder_request_uid: parse_uuid(&request)?,
        fencing_token: token,
        holder_pid: pid,
        holder_start_token: start,
        expires_at: expires,
        state,
    })
}

fn read_lease(tx: &Connection, lease_id: i64) -> Result<Lease> {
    let raw = tx.query_row(
        &format!("SELECT {LEASE_COLUMNS} FROM leases WHERE lease_id = ?1"),
        [lease_id],
        map_lease_row,
    )?;
    finish_lease_row(raw)
}

fn insert_operation(
    tx: &Connection,
    operation_uid: Uuid,
    space_uid: SpaceUid,
    kind: OperationKind,
    request_uid: Uuid,
    payload_json: &str,
    now: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO operations (operation_uid, space_uid, kind, operation_state, \
         request_uid, payload_json, started_at, updated_at) \
         VALUES (?1, ?2, ?3, 'prepared', ?4, ?5, ?6, ?6)",
        params![
            operation_uid.to_string(),
            space_uid.0.to_string(),
            kind.as_str(),
            request_uid.to_string(),
            payload_json,
            now
        ],
    )
    .map_err(|e| match RegistryError::from(e) {
        RegistryError::OperationInProgress { .. } => {
            RegistryError::OperationInProgress { space_uid }
        }
        other => other,
    })?;
    Ok(())
}

fn op_state(tx: &Connection, operation_uid: Uuid) -> Result<OperationState> {
    let token: Option<String> = tx
        .query_row(
            "SELECT operation_state FROM operations WHERE operation_uid = ?1",
            [operation_uid.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    let token = token.ok_or_else(|| RegistryError::NotFound {
        what: format!("operation {operation_uid}"),
    })?;
    OperationState::parse(&token)
        .ok_or_else(|| RegistryError::Corrupt(format!("operation state {token:?}")))
}

fn require_unfinished_op(
    tx: &Connection,
    operation_uid: Uuid,
    space_uid: SpaceUid,
    kind: OperationKind,
) -> Result<OperationRow> {
    require_unfinished_op_of(tx, operation_uid, space_uid, &[kind], kind.as_str())
}

/// [`require_unfinished_op`] accepting a set of kinds: a wrong Space or a
/// finished row is [`RegistryError::Corrupt`] (the caller's handle does not
/// describe reality); a live row of the wrong kind is the typed
/// [`RegistryError::KindNotAllowed`].
fn require_unfinished_op_of(
    tx: &Connection,
    operation_uid: Uuid,
    space_uid: SpaceUid,
    allowed: &[OperationKind],
    allowed_names: &'static str,
) -> Result<OperationRow> {
    let raw = tx
        .query_row(
            &format!("SELECT {OP_COLUMNS} FROM operations WHERE operation_uid = ?1"),
            [operation_uid.to_string()],
            map_operation_row,
        )
        .optional()?
        .ok_or_else(|| RegistryError::NotFound {
            what: format!("operation {operation_uid}"),
        })?;
    let row = finish_operation_row(raw)?;
    if row.space_uid != space_uid || row.state.is_terminal() {
        return Err(RegistryError::Corrupt(format!(
            "operation {operation_uid} is {} {} on {}, expected unfinished {} on {}",
            row.kind, row.state, row.space_uid.0, allowed_names, space_uid.0
        )));
    }
    if !allowed.contains(&row.kind) {
        return Err(RegistryError::KindNotAllowed {
            kind: row.kind,
            allowed: allowed_names,
        });
    }
    Ok(row)
}

/// The exact `spaces.health` token for a [`Health`] (registry-v1.sql CHECK
/// set).
fn health_token(health: Health) -> &'static str {
    match health {
        Health::Healthy => "healthy",
        Health::MultiWindow => "multi_window",
        Health::NativeKeyCollision => "native_key_collision",
        Health::Unstamped => "unstamped",
        Health::Unknown => "unknown",
    }
}

fn finish_op(tx: &Connection, operation_uid: Uuid, to: OperationState, now: &str) -> Result<()> {
    let finished: Option<&str> = if to.is_terminal() { Some(now) } else { None };
    tx.execute(
        "UPDATE operations SET operation_state = ?2, updated_at = ?3, finished_at = ?4 \
         WHERE operation_uid = ?1",
        params![operation_uid.to_string(), to.as_str(), now, finished],
    )?;
    Ok(())
}

/// Advance the authority hash chain inside a mutating transaction (see the
/// module docs for the exact formula and advance policy).
fn advance_revision(tx: &Connection, now: &str) -> Result<(u64, String)> {
    let (revision, head): (i64, String) = tx.query_row(
        "SELECT authority_revision, authority_head_hash FROM meta WHERE id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let next = (revision as u64) + 1;
    let txn_uid = Uuid::new_v4();
    let new_head = chain_head_hash(&head, next, &txn_uid);
    tx.execute(
        "INSERT INTO authority_revisions (revision, parent_head_hash, head_hash, txn_uid, committed_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![next as i64, head, new_head, txn_uid.to_string(), now],
    )?;
    tx.execute(
        "UPDATE meta SET authority_revision = ?1, authority_head_hash = ?2",
        params![next as i64, new_head],
    )?;
    Ok((next, new_head))
}

fn parse_uuid(text: &str) -> Result<Uuid> {
    Uuid::parse_str(text).map_err(|e| RegistryError::Corrupt(format!("uuid {text:?}: {e}")))
}

/// Parse one of the plan's snake_case tokens into its serde enum.
fn token_enum<T: serde::de::DeserializeOwned>(token: &str) -> Result<T> {
    serde_json::from_value(serde_json::Value::String(token.to_string()))
        .map_err(|e| RegistryError::Corrupt(format!("token {token:?}: {e}")))
}

fn backoff_delay(base: Duration, attempt: u32) -> Duration {
    let base = base.max(Duration::from_millis(1));
    let scaled = base.saturating_mul(attempt.min(8));
    scaled + jitter(base)
}

fn jitter(cap: Duration) -> Duration {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let seed = nanos ^ ((std::process::id() as u64) << 17);
    Duration::from_nanos(seed % cap.as_nanos().max(1) as u64)
}

// ---------------------------------------------------------------------------
// RFC 3339 UTC timestamps (std-only)

pub fn now_rfc3339() -> String {
    rfc3339_utc(SystemTime::now())
}

/// Seconds-precision RFC 3339 UTC, e.g. `2026-08-16T12:34:56Z`.
pub fn rfc3339_utc(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's `civil_from_days` (days since 1970-01-01 to y/m/d).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_matches_known_instants() {
        assert_eq!(rfc3339_utc(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        // 2000-03-01T00:00:00Z = 951868800 (leap-century boundary).
        let t = UNIX_EPOCH + Duration::from_secs(951_868_800);
        assert_eq!(rfc3339_utc(t), "2000-03-01T00:00:00Z");
        // 2024-02-29T23:59:59Z = 1709251199 (leap day).
        let t = UNIX_EPOCH + Duration::from_secs(1_709_251_199);
        assert_eq!(rfc3339_utc(t), "2024-02-29T23:59:59Z");
    }

    #[test]
    fn hash_chain_formula_is_deterministic_and_documented() {
        let registry = RegistryUid(Uuid::nil());
        let genesis = genesis_head_hash(registry);
        assert_eq!(
            genesis,
            format!(
                "sha256:{}",
                sha256_hex(format!("dmux-authority-genesis\n{}\n", Uuid::nil()).as_bytes())
            )
        );
        let txn = Uuid::nil();
        let head1 = chain_head_hash(&genesis, 1, &txn);
        assert_eq!(
            head1,
            format!(
                "sha256:{}",
                sha256_hex(format!("{genesis}\n1\n{txn}\n").as_bytes())
            )
        );
        // Deterministic: same inputs, same head.
        assert_eq!(head1, chain_head_hash(&genesis, 1, &txn));
        // Any input change changes the head.
        assert_ne!(head1, chain_head_hash(&genesis, 2, &txn));
    }

    #[test]
    fn lease_scope_strings_match_the_contract() {
        let uid = BackendInstanceUid(Uuid::nil());
        assert_eq!(
            LeaseScope::Backend(uid).as_scope_string(),
            format!("backend:{}", Uuid::nil())
        );
        assert_eq!(
            LeaseScope::Recovery(uid).as_scope_string(),
            format!("recovery:{}", Uuid::nil())
        );
        assert_eq!(
            LeaseScope::Snapshot(uid).as_scope_string(),
            format!("snapshot:{}", Uuid::nil())
        );
        assert_eq!(LeaseScope::Maintenance.as_scope_string(), "maintenance");
        let owner = HostUid(Uuid::nil());
        assert_eq!(
            LeaseScope::decision(owner, "proj").as_scope_string(),
            format!("decision:{}:{}", Uuid::nil(), sha256_hex(b"proj"))
        );
    }

    #[test]
    fn probe_pid_distinguishes_self_from_gone() {
        assert_eq!(probe_pid(std::process::id() as i32), HolderLiveness::Alive);
        // PID from far beyond any real pid space on both platforms.
        assert_eq!(probe_pid(99_999_999), HolderLiveness::Dead);
    }
}
