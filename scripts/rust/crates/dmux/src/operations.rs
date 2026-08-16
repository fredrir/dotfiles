//! Owner-side fenced operations. P5 delivers the tmux server-epoch
//! bootstrap (`dmux _tmux-bootstrap`, plan §11.2); P6 adds the fenced
//! create/rename/remove flows on top of the same skeleton.
//!
//! Root-owned (plan §19).

use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::backend::tmux::{EpochSetOutcome, SystemRunner, TmuxProvider};
use crate::locks::{LockMode, LockScope, OrderedLocks};
use crate::model::{Backend, ServerEpoch};
use crate::registry::{Registry, RegistryConfig};

/// Outcome of one `_tmux-bootstrap` run (plan §11.2). Every variant leaves
/// the registry binding equal to the server's observed state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TmuxBootstrapOutcome {
    /// Fresh incarnation: epoch minted, stamped, published, verified.
    Bootstrapped { epoch: ServerEpoch },
    /// Option already equalled the registry binding for this incarnation.
    AlreadyBound { epoch: ServerEpoch },
    /// Option present but the registry knew a different incarnation or
    /// epoch: the observed binding was published, which invalidates every
    /// prior child ref minted under the old epoch (plan §11.2). Never
    /// overwrites the option.
    Rebound {
        epoch: ServerEpoch,
        previous: Option<ServerEpoch>,
    },
}

#[derive(Debug)]
pub enum BootstrapError {
    /// No running server for the namespace: nothing to bootstrap. `ls`
    /// lists a stopped namespace via inventory; the hook simply loses the
    /// race with server death.
    ServerStopped(String),
    Lock(String),
    Registry(String),
    Provider(String),
}

impl std::fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BootstrapError::ServerStopped(d) => write!(f, "tmux server stopped: {d}"),
            BootstrapError::Lock(d) => write!(f, "kernel lock: {d}"),
            BootstrapError::Registry(d) => write!(f, "registry: {d}"),
            BootstrapError::Provider(d) => write!(f, "tmux: {d}"),
        }
    }
}

/// Explicit storage/lock locations so tests inject scratch dirs; production
/// callers build this from `registry::production_db_path()` +
/// `runtime::dmux_runtime_dir()`.
pub struct OperationEnv {
    pub db_path: PathBuf,
    pub lock_dir: PathBuf,
}

impl OperationEnv {
    pub fn production() -> std::io::Result<OperationEnv> {
        let db_path = crate::registry::production_db_path().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "no XDG data home resolvable")
        })?;
        Ok(OperationEnv {
            db_path,
            lock_dir: crate::runtime::dmux_runtime_dir()?,
        })
    }
}

/// The §11.2 epoch bootstrap for one managed tmux namespace. The exact
/// sequence, per the P5 handoffs: open registry → ensure the tmux backend
/// instance → take the authority gate (shared) and the backend-instance
/// kernel lock (exclusive) → probe server identity UNDER the lock →
/// `set_epoch_if_absent` → publish the observed binding → `verify_epoch`.
pub fn tmux_bootstrap(
    env: &OperationEnv,
    namespace: &str,
) -> Result<TmuxBootstrapOutcome, BootstrapError> {
    let mut registry = Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir))
        .map_err(|e| BootstrapError::Registry(e.to_string()))?;
    let instance = registry
        .register_backend_instance(Backend::Tmux, Some(namespace), None)
        .map_err(|e| BootstrapError::Registry(e.to_string()))?;

    let mut locks = OrderedLocks::new(&env.lock_dir);
    locks
        .acquire(LockScope::AuthorityGate, LockMode::Shared)
        .map_err(|e| BootstrapError::Lock(e.to_string()))?;
    locks
        .acquire(LockScope::BackendInstance(instance), LockMode::Exclusive)
        .map_err(|e| BootstrapError::Lock(e.to_string()))?;

    let provider: TmuxProvider<SystemRunner> = TmuxProvider::new(namespace);
    // Identity under the lock, so identity and epoch bind to one incarnation.
    let identity = provider
        .server_identity(namespace)
        .map_err(|e| BootstrapError::ServerStopped(format!("{e:?}")))?;

    let minted = ServerEpoch(Uuid::new_v4());
    let outcome = provider
        .set_epoch_if_absent(namespace, minted)
        .map_err(|e| BootstrapError::Provider(format!("{e:?}")))?;
    let (observed_epoch, previous) = match outcome {
        EpochSetOutcome::Set => (minted, None),
        EpochSetOutcome::AlreadySet(existing) => {
            let record = registry
                .backend_server(instance)
                .map_err(|e| BootstrapError::Registry(e.to_string()))?;
            let same_incarnation = record.server_pid == Some(identity.pid as i64)
                && record.server_start_token.as_deref() == Some(identity.start_token.as_str());
            if same_incarnation && record.server_epoch == Some(existing) {
                // Fully bound already: adopt/no-op.
                provider
                    .verify_epoch(namespace, existing, &identity)
                    .map_err(|e| BootstrapError::Provider(format!("{e:?}")))?;
                return Ok(TmuxBootstrapOutcome::AlreadyBound { epoch: existing });
            }
            (existing, record.server_epoch)
        }
    };

    registry
        .publish_backend_server(
            instance,
            observed_epoch,
            Some(identity.pid as i64),
            Some(&identity.start_token),
            None,
            None,
        )
        .map_err(|e| BootstrapError::Registry(e.to_string()))?;
    provider
        .verify_epoch(namespace, observed_epoch, &identity)
        .map_err(|e| BootstrapError::Provider(format!("{e:?}")))?;

    Ok(match previous {
        None if matches!(outcome, EpochSetOutcome::Set) => TmuxBootstrapOutcome::Bootstrapped {
            epoch: observed_epoch,
        },
        previous => TmuxBootstrapOutcome::Rebound {
            epoch: observed_epoch,
            previous,
        },
    })
}

// ---------------------------------------------------------------------------
// Fenced Space operations (plan §10.2, P6). All owner-local; explicit
// backend; the P4 resolver/policy integration joins them at the CLI cutover.

use crate::backend::{CreateSpec, InventoryOutcome, InventoryScope, Provider};
use crate::bootstrap::{
    self, BootstrapJournal, BootstrapResult, BootstrapState, IssuedRequest, MarkerContext,
};
use crate::model::{ChildKind, ProviderHandle, SpaceNo, SpaceUid};
use crate::refs::{ChildRefShape, child_suffix};
use crate::registry::{NativeBindingSpec, NativeKind, sha256::sha256_hex};

#[derive(Debug)]
pub enum OpError {
    NameConflict(String),
    Indeterminate(String),
    Bootstrap(String),
    NotFound(String),
    Lock(String),
    Registry(String),
    Provider(String),
}

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (kind, detail) = match self {
            OpError::NameConflict(d) => ("name conflict", d),
            OpError::Indeterminate(d) => ("inventory indeterminate", d),
            OpError::Bootstrap(d) => ("bootstrap", d),
            OpError::NotFound(d) => ("not found", d),
            OpError::Lock(d) => ("kernel lock", d),
            OpError::Registry(d) => ("registry", d),
            OpError::Provider(d) => ("provider", d),
        };
        write!(f, "{kind}: {detail}")
    }
}

fn reg_err(e: impl std::fmt::Display) -> OpError {
    OpError::Registry(e.to_string())
}

pub struct CreateRequest {
    /// Client idempotency key: a replay returns the original result.
    pub request_uid: Uuid,
    pub name: String,
    pub cwd: Option<String>,
    /// User program; empty means a login shell.
    pub program: Vec<String>,
    /// Absolute path of the pane-bootstrap helper binary.
    pub helper_bin: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CreatedSpace {
    pub space_uid: SpaceUid,
    pub space_no: SpaceNo,
    pub backend: Backend,
    pub native_token: String,
    pub group_ref: String,
    pub split_ref: String,
    /// True when this call replayed a completed request (ack loss).
    #[serde(default)]
    pub replayed: bool,
}

/// The §10.2 create sequence: idempotency ledger → §10.1 lock order →
/// same-name guard against durable + complete live inventory → reserve →
/// journaled bootstrap → provider create (spawns the helper, never the user
/// program) → three-way correlation → FIFO payload + ack → finalize.
pub fn create_space(
    env: &OperationEnv,
    provider: &dyn Provider,
    scope: &InventoryScope,
    backend: Backend,
    req: &CreateRequest,
) -> Result<CreatedSpace, OpError> {
    let mut registry =
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).map_err(reg_err)?;
    let identity = registry.identity().map_err(reg_err)?;
    let instance = registry
        .register_backend_instance(backend, Some(&scope.endpoint), None)
        .map_err(reg_err)?;

    let digest = sha256_hex(
        format!(
            "new\x1f{}\x1f{}\x1f{:?}\x1f{:?}",
            req.name, backend, req.cwd, req.program
        )
        .as_bytes(),
    );
    match registry
        .record_rpc_request(req.request_uid, "new", &digest)
        .map_err(reg_err)?
    {
        crate::registry::RpcDisposition::Replay {
            result_json: Some(result),
            ..
        } => {
            let mut replayed: CreatedSpace =
                serde_json::from_value(result).map_err(|e| OpError::Registry(e.to_string()))?;
            replayed.replayed = true;
            return Ok(replayed);
        }
        // New request, or an unknown-state row being resumed with the same
        // request UID: both proceed into the fenced flow below.
        _ => {}
    }

    let mut locks = OrderedLocks::new(&env.lock_dir);
    locks
        .acquire(LockScope::AuthorityGate, LockMode::Shared)
        .map_err(|e| OpError::Lock(e.to_string()))?;
    locks
        .acquire_decisions(identity.host_uid, &[&req.name], LockMode::Exclusive)
        .map_err(|e| OpError::Lock(e.to_string()))?;
    locks
        .acquire(LockScope::BackendInstance(instance), LockMode::Exclusive)
        .map_err(|e| OpError::Lock(e.to_string()))?;

    // Durable same-name guard (the live-name unique index backs this).
    if let Some(existing) = registry
        .live_space_by_name(instance, &req.name)
        .map_err(reg_err)?
    {
        return Err(OpError::NameConflict(format!(
            "name {:?} is held by Space {} (SpaceNo {})",
            req.name, existing.space_uid.0, existing.space_no
        )));
    }
    // Complete-inventory guard: creation fails closed on indeterminacy
    // (plan §2.10) and on an unmanaged same-name native row.
    let epoch = match provider.inventory(scope) {
        InventoryOutcome::Complete(inv) => {
            if inv.rows.iter().any(|r| r.native_name == req.name) {
                return Err(OpError::NameConflict(format!(
                    "unmanaged {backend} resource already carries name {:?}",
                    req.name
                )));
            }
            inv.server_epoch.ok_or_else(|| {
                OpError::Indeterminate("managed create requires an epoched server".into())
            })?
        }
        other => return Err(OpError::Indeterminate(format!("{backend} scan: {other:?}"))),
    };

    let reservation = registry
        .reserve_space(&req.name, instance, req.request_uid)
        .map_err(reg_err)?;
    let native_token = match backend {
        Backend::Wez => format!("dmux:{}:{}", identity.host_uid.0, reservation.space_uid.0),
        Backend::Tmux => req.name.clone(),
    };

    let boot_uid = Uuid::new_v4();
    registry
        .bootstrap_issue(&IssuedRequest {
            request_uid: boot_uid,
            operation_uid: Some(reservation.operation_uid),
            space_uid: Some(reservation.space_uid),
            backend_instance: instance,
            server_epoch: epoch,
            intended_parent: None,
            recovery_generation: None,
            manifest_node_path: None,
        })
        .map_err(|e| OpError::Bootstrap(e.message))?;
    let paths = bootstrap::prepare(&env.lock_dir, boot_uid)
        .map_err(|e| OpError::Bootstrap(e.to_string()))?;

    let program = if req.program.is_empty() {
        vec!["/bin/sh".to_string(), "-l".to_string()]
    } else {
        req.program.clone()
    };
    let spec = CreateSpec {
        native_token: native_token.clone(),
        cwd: req.cwd.clone(),
        bootstrap_argv: bootstrap::helper_argv(&req.helper_bin, boot_uid, &program),
    };

    let fail = |registry: &mut Registry, state: BootstrapState, err: OpError| {
        let _ = registry.bootstrap_state(boot_uid, state);
        let _ = registry.abort_create(reservation.space_uid, reservation.operation_uid);
        bootstrap::cleanup(&paths);
        err
    };

    let binding = match provider.create(scope, &spec) {
        Ok(binding) => binding,
        Err(e) => {
            return Err(fail(
                &mut registry,
                BootstrapState::Aborted,
                OpError::Provider(format!("{e:?}")),
            ));
        }
    };
    registry
        .bootstrap_spawned(
            boot_uid,
            &serde_json::json!({
                "native_token": binding.native_token,
                "root_group": binding.root_group.to_string(),
                "root_split": binding.root_split.to_string(),
            })
            .to_string(),
        )
        .map_err(|e| OpError::Bootstrap(e.message))?;

    // Third witness: the helper's inherited pane id must agree with the
    // provider-verified spawn result (ADR 004 three-way rule; the title
    // scan is the provider's own postcondition).
    let pane_env = bootstrap::read_pane_env(&paths, std::time::Duration::from_secs(10))
        .map_err(|e| OpError::Bootstrap(e.to_string()))?;
    if let Some(record) = &pane_env {
        let inherited = record
            .wezterm_pane
            .as_deref()
            .or(record.tmux_pane.as_deref());
        if let Some(env_id) = inherited
            && !witness_matches(&binding.root_split, env_id)
        {
            return Err(fail(
                &mut registry,
                BootstrapState::Conflict,
                OpError::Bootstrap(format!(
                    "helper inherited pane {env_id} but the provider verified {}",
                    binding.root_split
                )),
            ));
        }
    }

    let group_ref = child_suffix(&ChildRefShape {
        kind: ChildKind::Group,
        epoch,
        handle: binding.root_group.clone(),
    });
    let split_ref = child_suffix(&ChildRefShape {
        kind: ChildKind::Split,
        epoch,
        handle: binding.root_split.clone(),
    });
    registry
        .bootstrap_correlated(boot_uid, &group_ref, &split_ref)
        .map_err(|e| OpError::Bootstrap(e.message))?;

    let payload = BootstrapResult {
        request_uid: boot_uid,
        context: MarkerContext {
            host_uid: identity.host_uid,
            space_uid: reservation.space_uid,
            space_no: reservation.space_no,
            backend,
            domain: None,
            server_epoch: epoch,
            group_ref: group_ref.clone(),
            split_ref: split_ref.clone(),
        },
    };
    if let Err(e) = bootstrap::send_result(&paths, &payload, std::time::Duration::from_secs(10)) {
        return Err(fail(
            &mut registry,
            BootstrapState::Timeout,
            OpError::Bootstrap(format!("helper gone before payload: {e:?}")),
        ));
    }
    if bootstrap::read_ack(&paths, std::time::Duration::from_secs(10))
        .map_err(|e| OpError::Bootstrap(e.to_string()))?
        .is_none()
    {
        return Err(fail(
            &mut registry,
            BootstrapState::Timeout,
            OpError::Bootstrap("helper never acknowledged the payload".into()),
        ));
    }
    registry
        .bootstrap_state(boot_uid, BootstrapState::Acked)
        .map_err(|e| OpError::Bootstrap(e.message))?;

    registry
        .finalize_create(
            reservation.space_uid,
            reservation.operation_uid,
            &NativeBindingSpec {
                native_token: binding.native_token.clone(),
                native_kind: match backend {
                    Backend::Wez => NativeKind::WezWorkspaceKey,
                    Backend::Tmux => NativeKind::TmuxSessionId,
                },
                server_epoch: Some(epoch),
            },
        )
        .map_err(reg_err)?;
    registry
        .bootstrap_state(boot_uid, BootstrapState::Completed)
        .map_err(|e| OpError::Bootstrap(e.message))?;
    bootstrap::cleanup(&paths);

    let created = CreatedSpace {
        space_uid: reservation.space_uid,
        space_no: reservation.space_no,
        backend,
        native_token: binding.native_token,
        group_ref,
        split_ref,
        replayed: false,
    };
    registry
        .finish_rpc_request(
            req.request_uid,
            &serde_json::to_value(&created).map_err(|e| OpError::Registry(e.to_string()))?,
            Some(reservation.operation_uid),
        )
        .map_err(reg_err)?;
    Ok(created)
}

/// A provider handle and a helper-inherited pane id agree when their
/// numeric components match (`Wz(3)` vs WEZTERM_PANE="3"; `Tx(3)` vs
/// TMUX_PANE="%3").
fn witness_matches(handle: &ProviderHandle, env_id: &str) -> bool {
    let digits: String = env_id.chars().filter(|c| c.is_ascii_digit()).collect();
    match handle {
        ProviderHandle::Wz(n) | ProviderHandle::Tx(n) => digits == n.to_string(),
        ProviderHandle::Opaque(_) => false,
    }
}

/// Fenced rename (plan §10.2): decision locks for BOTH names in exact-byte
/// order, native rename where the backend has one (tmux), registry-only for
/// Wez (plan §2.5), then commit with history.
pub fn rename_space(
    env: &OperationEnv,
    provider: &dyn Provider,
    scope: &InventoryScope,
    backend: Backend,
    space_uid: SpaceUid,
    new_name: &str,
    request_uid: Uuid,
) -> Result<(), OpError> {
    let mut registry =
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).map_err(reg_err)?;
    let identity = registry.identity().map_err(reg_err)?;
    let instance = registry
        .register_backend_instance(backend, Some(&scope.endpoint), None)
        .map_err(reg_err)?;
    let space = registry
        .space(space_uid)
        .map_err(|e| OpError::NotFound(e.to_string()))?;

    let mut locks = OrderedLocks::new(&env.lock_dir);
    locks
        .acquire(LockScope::AuthorityGate, LockMode::Shared)
        .map_err(|e| OpError::Lock(e.to_string()))?;
    locks
        .acquire_decisions(
            identity.host_uid,
            &[space.logical_name.as_str(), new_name],
            LockMode::Exclusive,
        )
        .map_err(|e| OpError::Lock(e.to_string()))?;
    locks
        .acquire(LockScope::BackendInstance(instance), LockMode::Exclusive)
        .map_err(|e| OpError::Lock(e.to_string()))?;

    if let Some(existing) = registry
        .live_space_by_name(instance, new_name)
        .map_err(reg_err)?
        && existing.space_uid != space_uid
    {
        return Err(OpError::NameConflict(format!(
            "name {:?} is held by Space {}",
            new_name, existing.space_uid.0
        )));
    }

    let operation_uid = registry
        .begin_rename(space_uid, new_name, request_uid)
        .map_err(reg_err)?;
    if backend == Backend::Tmux {
        let binding = registry
            .current_binding(space_uid)
            .map_err(reg_err)?
            .ok_or_else(|| OpError::NotFound("no current native binding".into()))?;
        let native = crate::backend::NativeBinding {
            native_token: binding.native_token,
            server_epoch: scope.expected_epoch.ok_or_else(|| {
                OpError::Indeterminate("rename requires the current epoch".into())
            })?,
            root_group: ProviderHandle::Tx(0),
            root_split: ProviderHandle::Tx(0),
        };
        provider
            .rename(scope, &native, new_name)
            .map_err(|e| OpError::Provider(format!("{e:?}")))?;
    }
    registry
        .commit_rename(space_uid, operation_uid)
        .map_err(reg_err)
}

/// Fenced remove (plan §14): `deleting` intent journaled before any kill,
/// provider removal with verified absence, tombstone only afterwards.
pub fn remove_space(
    env: &OperationEnv,
    provider: &dyn Provider,
    scope: &InventoryScope,
    backend: Backend,
    space_uid: SpaceUid,
    request_uid: Uuid,
) -> Result<(), OpError> {
    let mut registry =
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).map_err(reg_err)?;
    let instance = registry
        .register_backend_instance(backend, Some(&scope.endpoint), None)
        .map_err(reg_err)?;
    let _space = registry
        .space(space_uid)
        .map_err(|e| OpError::NotFound(e.to_string()))?;
    let binding = registry.current_binding(space_uid).map_err(reg_err)?;

    let mut locks = OrderedLocks::new(&env.lock_dir);
    locks
        .acquire(LockScope::AuthorityGate, LockMode::Shared)
        .map_err(|e| OpError::Lock(e.to_string()))?;
    locks
        .acquire(LockScope::BackendInstance(instance), LockMode::Exclusive)
        .map_err(|e| OpError::Lock(e.to_string()))?;
    locks
        .acquire(LockScope::Space(space_uid), LockMode::Exclusive)
        .map_err(|e| OpError::Lock(e.to_string()))?;

    let operation_uid = registry
        .begin_remove(space_uid, request_uid)
        .map_err(reg_err)?;
    if let Some(binding) = binding {
        let native = crate::backend::NativeBinding {
            native_token: binding.native_token,
            server_epoch: scope.expected_epoch.ok_or_else(|| {
                OpError::Indeterminate("remove requires the current epoch".into())
            })?,
            root_group: ProviderHandle::Tx(0),
            root_split: ProviderHandle::Tx(0),
        };
        if let Err(e) = provider.remove(scope, &native) {
            // Non-convergence or provider failure: the operation stays
            // journaled (deleting + unfinished op); never tombstone.
            return Err(OpError::Provider(format!("{e:?}")));
        }
    }
    registry
        .complete_remove(space_uid, operation_uid)
        .map_err(reg_err)
}

// ---------------------------------------------------------------------------
// Explicit adoption (plan §10.3, P6). Listing never adopts; these are the
// only ordinary entry points that allocate identity for an external
// resource. The Space lands `active + unstamped` until every pane
// acknowledges its stamp (health flips via scan-driven `set_space_health`).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptedSpace {
    pub space_uid: SpaceUid,
    pub space_no: SpaceNo,
    pub name: String,
    pub native_token: String,
}

/// Adopt an external tmux session by exact session id (`$N`): reserve
/// identity under the decision+instance fences, stamp the `@dmux_*` session
/// options, verify the stamp readback, then bind. Markers + immutable
/// session id preserve identity across external renames (plan §10.3).
pub fn adopt_tmux<R: crate::backend::tmux::TmuxRunner>(
    env: &OperationEnv,
    provider: &crate::backend::tmux::TmuxProvider<R>,
    scope: &InventoryScope,
    session_id: &str,
    name_override: Option<&str>,
    request_uid: Uuid,
) -> Result<AdoptedSpace, OpError> {
    let mut registry =
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).map_err(reg_err)?;
    let identity = registry.identity().map_err(reg_err)?;
    let instance = registry
        .register_backend_instance(Backend::Tmux, Some(&scope.endpoint), None)
        .map_err(reg_err)?;

    // Re-resolve the exact session in a complete scan (plan §7.4: adopt
    // re-resolves the token before acquiring its operation lease).
    let (native_name, epoch) = match Provider::inventory(provider, scope) {
        InventoryOutcome::Complete(inv) => {
            let row = inv
                .rows
                .iter()
                .find(|r| r.native_token == session_id)
                .ok_or_else(|| OpError::NotFound(format!("no session {session_id}")))?;
            let epoch = inv.server_epoch.ok_or_else(|| {
                OpError::Indeterminate("adoption requires an epoched server".into())
            })?;
            (row.native_name.clone(), epoch)
        }
        other => return Err(OpError::Indeterminate(format!("tmux scan: {other:?}"))),
    };
    let name = name_override.unwrap_or(&native_name).to_string();

    let mut locks = OrderedLocks::new(&env.lock_dir);
    locks
        .acquire(LockScope::AuthorityGate, LockMode::Shared)
        .map_err(|e| OpError::Lock(e.to_string()))?;
    locks
        .acquire_decisions(identity.host_uid, &[name.as_str()], LockMode::Exclusive)
        .map_err(|e| OpError::Lock(e.to_string()))?;
    locks
        .acquire(LockScope::BackendInstance(instance), LockMode::Exclusive)
        .map_err(|e| OpError::Lock(e.to_string()))?;

    if let Some(existing) = registry
        .live_space_by_name(instance, &name)
        .map_err(reg_err)?
    {
        return Err(OpError::NameConflict(format!(
            "name {:?} is held by Space {}",
            name, existing.space_uid.0
        )));
    }
    let reservation = registry
        .reserve_space_kind(
            &name,
            instance,
            request_uid,
            crate::model::OperationKind::Adopt,
        )
        .map_err(reg_err)?;

    let markers = crate::backend::tmux::SpaceMarkers {
        host_uid: identity.host_uid.0.to_string(),
        registry_uid: identity.registry_uid.0.to_string(),
        space_uid: reservation.space_uid.0.to_string(),
        space_no: reservation.space_no.to_string(),
    };
    let stamped = provider
        .stamp_markers(scope, session_id, &markers)
        .and_then(|()| provider.read_markers(scope, session_id));
    match stamped {
        Ok(readback) if readback.space_uid.as_deref() == Some(&markers.space_uid) => {}
        other => {
            let _ = registry.abort_create(reservation.space_uid, reservation.operation_uid);
            return Err(OpError::Provider(format!(
                "marker stamp verification failed: {other:?}"
            )));
        }
    }
    registry
        .finalize_adopt(
            reservation.space_uid,
            reservation.operation_uid,
            &NativeBindingSpec {
                native_token: session_id.to_string(),
                native_kind: NativeKind::TmuxSessionId,
                server_epoch: Some(epoch),
            },
        )
        .map_err(reg_err)?;
    Ok(AdoptedSpace {
        space_uid: reservation.space_uid,
        space_no: reservation.space_no,
        name,
        native_token: session_id.to_string(),
    })
}

/// Adopt an external Wez workspace via the fork's atomic CAS rename to the
/// opaque key (plan §10.3, ADR 006): capability-probed, sole-window
/// enforced server-side, zero mutation on every non-`Renamed` outcome.
pub fn adopt_wez<R: crate::backend::wez::WezRunner>(
    env: &OperationEnv,
    provider: &crate::backend::wez::WezProvider<R>,
    scope: &InventoryScope,
    source_workspace: &str,
    name_override: Option<&str>,
    request_uid: Uuid,
) -> Result<AdoptedSpace, OpError> {
    use crate::backend::wez::CasRenameOutcome;

    let mut registry =
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).map_err(reg_err)?;
    let identity = registry.identity().map_err(reg_err)?;
    let instance = registry
        .register_backend_instance(Backend::Wez, Some(&scope.endpoint), None)
        .map_err(reg_err)?;

    if !provider
        .probe_cas_rename(scope)
        .map_err(|e| OpError::Provider(format!("{e:?}")))?
    {
        return Err(OpError::Provider(
            "cas_capability_missing: the managed server lacks the fork CAS verb (ADR 006); \
             Wez adoption stays disabled"
                .into(),
        ));
    }
    let (epoch, window_id) = match Provider::inventory(provider, scope) {
        InventoryOutcome::Complete(inv) => {
            let row = inv
                .rows
                .iter()
                .find(|r| r.native_token == source_workspace)
                .ok_or_else(|| OpError::NotFound(format!("no workspace {source_workspace:?}")))?;
            if row.multi_window {
                return Err(OpError::Provider(format!(
                    "workspace {source_workspace:?} spans multiple windows: normalize first \
                     (plan §10.3); adoption refused"
                )));
            }
            let epoch = inv.server_epoch.ok_or_else(|| {
                OpError::Indeterminate("adoption requires an epoched server".into())
            })?;
            let window = provider
                .sole_window_id(scope, source_workspace)
                .map_err(|e| OpError::Provider(format!("{e:?}")))?;
            (epoch, window)
        }
        other => return Err(OpError::Indeterminate(format!("wez scan: {other:?}"))),
    };
    let name = name_override.unwrap_or(source_workspace).to_string();

    let mut locks = OrderedLocks::new(&env.lock_dir);
    locks
        .acquire(LockScope::AuthorityGate, LockMode::Shared)
        .map_err(|e| OpError::Lock(e.to_string()))?;
    locks
        .acquire_decisions(identity.host_uid, &[name.as_str()], LockMode::Exclusive)
        .map_err(|e| OpError::Lock(e.to_string()))?;
    locks
        .acquire(LockScope::BackendInstance(instance), LockMode::Exclusive)
        .map_err(|e| OpError::Lock(e.to_string()))?;

    if let Some(existing) = registry
        .live_space_by_name(instance, &name)
        .map_err(reg_err)?
    {
        return Err(OpError::NameConflict(format!(
            "name {:?} is held by Space {}",
            name, existing.space_uid.0
        )));
    }
    let reservation = registry
        .reserve_space_kind(
            &name,
            instance,
            request_uid,
            crate::model::OperationKind::Adopt,
        )
        .map_err(reg_err)?;
    let opaque_key = format!("dmux:{}:{}", identity.host_uid.0, reservation.space_uid.0);

    match provider.cas_rename_workspace(scope, window_id, source_workspace, &opaque_key, true) {
        Ok(CasRenameOutcome::Renamed) => {}
        Ok(other) => {
            let _ = registry.abort_create(reservation.space_uid, reservation.operation_uid);
            return Err(OpError::NameConflict(format!(
                "atomic adoption lost its race (zero mutation): {other:?}"
            )));
        }
        Err(e) => {
            let _ = registry.abort_create(reservation.space_uid, reservation.operation_uid);
            return Err(OpError::Provider(format!("{e:?}")));
        }
    }
    registry
        .finalize_adopt(
            reservation.space_uid,
            reservation.operation_uid,
            &NativeBindingSpec {
                native_token: opaque_key.clone(),
                native_kind: NativeKind::WezWorkspaceKey,
                server_epoch: Some(epoch),
            },
        )
        .map_err(reg_err)?;
    Ok(AdoptedSpace {
        space_uid: reservation.space_uid,
        space_no: reservation.space_no,
        name,
        native_token: opaque_key,
    })
}

/// Derive the `-L` namespace from a tmux socket path when the hook runs
/// inside the server (`$TMUX` is `<socket-path>,<pid>,<session>`): sockets
/// under the standard `tmux-<uid>` directory map to their basename; any
/// other path is not a `-L` namespace and is rejected (the caller must pass
/// `--namespace` explicitly for `-S` servers).
pub fn namespace_from_tmux_env(tmux_env: &str) -> Option<String> {
    let socket = tmux_env.split(',').next()?;
    let path = Path::new(socket);
    let parent = path.parent()?.file_name()?.to_str()?;
    if !parent.starts_with("tmux-") {
        return None;
    }
    Some(path.file_name()?.to_str()?.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_derivation_from_tmux_env() {
        assert_eq!(
            namespace_from_tmux_env("/private/tmp/tmux-501/dmux-managed,45159,0"),
            Some("dmux-managed".into())
        );
        assert_eq!(
            namespace_from_tmux_env("/tmp/tmux-1000/other,1,2"),
            Some("other".into())
        );
        // -S servers outside the standard dir are not -L namespaces.
        assert_eq!(namespace_from_tmux_env("/var/run/custom.sock,1,2"), None);
        assert_eq!(namespace_from_tmux_env(""), None);
    }
}
