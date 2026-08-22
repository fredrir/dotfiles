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
    // The socket witness is taken in the same probe (ADR 012 WS-A.9): it is
    // published beside pid/start token and is what later readers `stat`
    // against, the way the wez descriptor's dev/ino are.
    let incarnation = provider
        .server_incarnation(namespace)
        .map_err(|e| BootstrapError::ServerStopped(format!("{e:?}")))?;
    let identity = incarnation.identity.clone();
    let socket_dev = i64::try_from(incarnation.socket_dev).ok();
    let socket_ino = i64::try_from(incarnation.socket_ino).ok();

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
                && record.server_start_token.as_deref() == Some(identity.start_token.as_str())
                && record.socket_dev == socket_dev
                && record.socket_ino == socket_ino;
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
            socket_dev,
            socket_ino,
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
use crate::model::{BackendInstanceUid, ChildKind, HostUid, ProviderHandle, SpaceNo, SpaceUid};
use crate::refs::{ChildRefShape, child_suffix};
use crate::registry::{NativeBindingSpec, NativeKind, sha256::sha256_hex};
use crate::resolve::{ClassSummary, summarize_backend};

#[derive(Debug)]
pub enum OpError {
    NameConflict(String),
    Indeterminate(String),
    Bootstrap(String),
    NotFound(String),
    Lock(String),
    Registry(String),
    Provider(String),
    /// P8a: the operation is legal but deliberately refused — a hidden
    /// remove cascade (plan §7.2) or a blocked action on an
    /// unstamped/conflicted Space (plan §10.3). The detail names the
    /// parent-level command to use instead.
    Refused(String),
    /// P8a: an epoch-qualified child ref from a previous server
    /// incarnation. Stale refs fail, never retarget (plan §6.3).
    StaleRef(String),
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
            OpError::Refused(d) => ("refused", d),
            OpError::StaleRef(d) => ("stale ref", d),
        };
        write!(f, "{kind}: {detail}")
    }
}

fn reg_err(e: impl std::fmt::Display) -> OpError {
    OpError::Registry(e.to_string())
}

/// A recovery generation is durable authority state, not merely the
/// coordinator's kernel-lock lifetime.  Every write path calls this after
/// taking the common backend-instance lock and before touching native state;
/// otherwise a crashed/failed coordinator could drop its process lock and
/// ordinary mutations would run through its still-unfinished journal.
fn require_no_unfinished_recovery(
    registry: &Registry,
    instance: crate::model::BackendInstanceUid,
) -> Result<(), OpError> {
    if let Some((generation, rows)) = registry
        .unfinished_recovery_for_instance(instance)
        .map_err(reg_err)?
    {
        let root_state = rows
            .iter()
            .find(|row| {
                row.manifest_node_path == crate::registry::recovery::RECOVERY_GENERATION_PATH
            })
            .map(|row| row.node_state.as_str())
            .unwrap_or("unknown");
        return Err(OpError::Refused(format!(
            "backend instance {} has unfinished recovery generation {} ({root_state}); use `dmux recovery status` and explicitly resume or abort it",
            instance.0, generation.generation_uid
        )));
    }
    Ok(())
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

/// One owner-verified provider target supplied to the cross-backend create
/// seam.  The caller may discover endpoints and instantiate adapters, but
/// this structure is not authority: [`create_space_owner_fenced`] checks the
/// backend/instance/endpoint tuple against the registry again while holding
/// the exact-name decision lock.
#[derive(Clone, Copy)]
pub struct OwnerCreateTarget<'a> {
    pub backend: Backend,
    pub instance: BackendInstanceUid,
    pub provider: &'a dyn Provider,
    pub scope: &'a InventoryScope,
}

fn create_digest(backend: Backend, req: &CreateRequest) -> String {
    sha256_hex(
        format!(
            "new\x1f{}\x1f{}\x1f{:?}\x1f{:?}",
            req.name, backend, req.cwd, req.program
        )
        .as_bytes(),
    )
}

fn replayed_create(
    registry: &mut Registry,
    req: &CreateRequest,
    backend: Backend,
    allow_opposite_selectable: Option<bool>,
) -> Result<Option<CreatedSpace>, OpError> {
    let digest = match allow_opposite_selectable {
        None => create_digest(backend, req),
        Some(allow) => sha256_hex(
            format!(
                "{}\x1fallow_opposite_selectable={allow}",
                create_digest(backend, req)
            )
            .as_bytes(),
        ),
    };
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
            Ok(Some(replayed))
        }
        // New request, or an unknown-state row being resumed with the same
        // request UID: both proceed into the fenced flow below.
        _ => Ok(None),
    }
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

    if let Some(replayed) = replayed_create(&mut registry, req, backend, None)? {
        return Ok(replayed);
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
    require_no_unfinished_recovery(&registry, instance)?;

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
            let epoch = inv.server_epoch.ok_or_else(|| {
                OpError::Indeterminate("managed create requires an epoched server".into())
            })?;
            require_pinned_epoch(scope, epoch)?
        }
        other => return Err(OpError::Indeterminate(format!("{backend} scan: {other:?}"))),
    };

    create_space_locked(
        env,
        &mut registry,
        identity.host_uid,
        provider,
        scope,
        backend,
        instance,
        epoch,
        req,
        |_, _, _| Ok(()),
    )
}

/// Reservation, bootstrap correlation, native creation and finalization.
/// Its caller retains every §10.1 lock for the full call.  `postcheck` runs
/// after the helper acknowledgement but before registry finalization, so a
/// cross-backend caller can prove both inventories again without opening a
/// race window.
#[allow(clippy::too_many_arguments)]
fn create_space_locked<F>(
    env: &OperationEnv,
    registry: &mut Registry,
    owner: HostUid,
    provider: &dyn Provider,
    scope: &InventoryScope,
    backend: Backend,
    instance: BackendInstanceUid,
    epoch: ServerEpoch,
    req: &CreateRequest,
    postcheck: F,
) -> Result<CreatedSpace, OpError>
where
    F: FnOnce(&Registry, &crate::backend::NativeBinding, SpaceUid) -> Result<(), OpError>,
{
    let reservation = registry
        .reserve_space(&req.name, instance, req.request_uid)
        .map_err(reg_err)?;
    let native_token = match backend {
        Backend::Wez => adoption_key(owner, reservation.space_uid),
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
                registry,
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
                registry,
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
            host_uid: owner,
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
            registry,
            BootstrapState::Timeout,
            OpError::Bootstrap(format!("helper gone before payload: {e:?}")),
        ));
    }
    if bootstrap::read_ack(&paths, std::time::Duration::from_secs(10))
        .map_err(|e| OpError::Bootstrap(e.to_string()))?
        .is_none()
    {
        return Err(fail(
            registry,
            BootstrapState::Timeout,
            OpError::Bootstrap("helper never acknowledged the payload".into()),
        ));
    }
    registry
        .bootstrap_state(boot_uid, BootstrapState::Acked)
        .map_err(|e| OpError::Bootstrap(e.message))?;

    if let Err(error) = postcheck(registry, &binding, reservation.space_uid) {
        // Native state exists, so aborting the reservation would turn the
        // resource into an unmanaged orphan.  Keep the create journal
        // unfinished and mark the bootstrap witness conflicted; repair can
        // now inspect the exact reservation/binding attempt.  The retained
        // decision/backend locks ensure no competing owner mutation ran
        // between the post-scan and this durable refusal.
        let _ = registry.bootstrap_state(boot_uid, BootstrapState::Conflict);
        bootstrap::cleanup(&paths);
        return Err(error);
    }

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

fn validate_owner_create_target(
    registry: &Registry,
    owner: HostUid,
    target: OwnerCreateTarget<'_>,
) -> Result<(), OpError> {
    if target.scope.backend != target.backend {
        return Err(OpError::Refused(format!(
            "{} provider target carries a {} inventory scope",
            target.backend, target.scope.backend
        )));
    }
    let info = registry
        .backend_instance_info(target.instance)
        .map_err(reg_err)?;
    if info.owner != owner || info.backend != target.backend {
        return Err(OpError::Refused(format!(
            "backend instance {} is not this owner's {} instance",
            target.instance.0, target.backend
        )));
    }
    if info.socket_path.as_deref() != Some(target.scope.endpoint.as_str()) {
        return Err(OpError::Refused(format!(
            "{} target endpoint {:?} does not equal the owner registry endpoint {:?}",
            target.backend, target.scope.endpoint, info.socket_path
        )));
    }
    Ok(())
}

fn scan_epoch_for_create(
    target: OwnerCreateTarget<'_>,
    outcome: &InventoryOutcome,
    selected: bool,
) -> Result<Option<ServerEpoch>, OpError> {
    match outcome {
        InventoryOutcome::Complete(inventory) => {
            let epoch = inventory.server_epoch.ok_or_else(|| {
                OpError::Indeterminate(format!(
                    "{} managed inventory is complete but unepoched",
                    target.backend
                ))
            })?;
            // WS-A.10: the epoch returned here is what the create journals
            // into `bootstrap_requests.server_epoch` and decides the name
            // collision on, so it must be the one the scope was pinned to —
            // never whatever an unvouched endpoint answered.
            Ok(Some(require_pinned_epoch(target.scope, epoch)?))
        }
        InventoryOutcome::ServerStopped { .. } if !selected => Ok(None),
        other => Err(OpError::Indeterminate(format!(
            "{} scan: {other:?}",
            target.backend
        ))),
    }
}

fn exact_name_candidates(
    registry: &Registry,
    instance: BackendInstanceUid,
    name: &str,
) -> Result<
    Vec<(
        crate::registry::SpaceRow,
        Option<crate::registry::BindingRow>,
        bool,
    )>,
    OpError,
> {
    registry
        .spaces()
        .map_err(reg_err)?
        .into_iter()
        .filter(|space| space.backend_instance == instance && space.logical_name == name)
        .map(|space| {
            let binding = registry.current_binding(space.space_uid).map_err(reg_err)?;
            let unfinished = registry
                .unfinished_operation(space.space_uid)
                .map_err(reg_err)?
                .is_some();
            Ok((space, binding, unfinished))
        })
        .collect()
}

fn summarize_owner_target(
    registry: &Registry,
    target: OwnerCreateTarget<'_>,
    scan: &InventoryOutcome,
    name: &str,
) -> Result<ClassSummary, OpError> {
    let candidates = exact_name_candidates(registry, target.instance, name)?;
    Ok(summarize_backend(scan, &candidates, name))
}

/// Result of one owner decision-fenced exact-name lookup. It contains only
/// stable managed identities and typed partition state; native tokens never
/// cross the owner boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnerNewLookup {
    pub wez: ClassSummary,
    pub tmux: ClassSummary,
}

/// Owner-side implementation of remote/local NEW_LOOKUP. Both provider
/// inventories are obtained while the exact logical-name decision lease and
/// all registered backend-instance shared locks are held in canonical order.
/// A missing backend target is accepted only when the registry proves that
/// no durable instance of that kind exists.
pub fn lookup_new_owner_fenced(
    env: &OperationEnv,
    wez: Option<OwnerCreateTarget<'_>>,
    tmux: Option<OwnerCreateTarget<'_>>,
    name: &str,
) -> Result<OwnerNewLookup, OpError> {
    let registry =
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).map_err(reg_err)?;
    let identity = registry.identity().map_err(reg_err)?;
    let mut locks = OrderedLocks::new(&env.lock_dir);
    locks
        .acquire(LockScope::AuthorityGate, LockMode::Shared)
        .map_err(|error| OpError::Lock(error.to_string()))?;
    locks
        .acquire_decisions(identity.host_uid, &[name], LockMode::Exclusive)
        .map_err(|error| OpError::Lock(error.to_string()))?;

    let mut targets = Vec::new();
    for (backend, supplied) in [(Backend::Wez, wez), (Backend::Tmux, tmux)] {
        let durable = registry
            .backend_instance_for_backend(backend)
            .map_err(reg_err)?;
        match (durable, supplied) {
            (None, None) => {}
            (None, Some(_)) => {
                return Err(OpError::Refused(format!(
                    "caller supplied an unregistered {backend} lookup target"
                )));
            }
            (Some(instance), None) => {
                return Err(OpError::Indeterminate(format!(
                    "owner has durable {backend} instance {}; its inventory target is required",
                    instance.0
                )));
            }
            (Some(instance), Some(target)) if target.instance != instance => {
                return Err(OpError::Refused(format!(
                    "{backend} lookup target {} is not durable instance {}",
                    target.instance.0, instance.0
                )));
            }
            (Some(_), Some(target)) if target.backend != backend => {
                return Err(OpError::Refused(format!(
                    "lookup target kind is {}, expected {backend}",
                    target.backend
                )));
            }
            (Some(_), Some(target)) => {
                validate_owner_create_target(&registry, identity.host_uid, target)?;
                targets.push(target);
            }
        }
    }
    targets.sort_by_key(|target| target.instance.0);
    for target in &targets {
        locks
            .acquire(
                LockScope::BackendInstance(target.instance),
                LockMode::Shared,
            )
            .map_err(|error| OpError::Lock(error.to_string()))?;
        require_no_unfinished_recovery(&registry, target.instance)?;
    }

    // Obtain every observation before classifying either side. This is
    // synchronous owner code today; neither failure can become proof that
    // the opposite backend is empty.
    let wez_scan = wez.map(|target| target.provider.inventory(target.scope));
    let tmux_scan = tmux.map(|target| target.provider.inventory(target.scope));
    let wez = match (wez, wez_scan.as_ref()) {
        (Some(target), Some(scan)) => summarize_owner_target(&registry, target, scan, name)?,
        (None, None) => ClassSummary::NoMatch,
        _ => unreachable!(),
    };
    let tmux = match (tmux, tmux_scan.as_ref()) {
        (Some(target), Some(scan)) => summarize_owner_target(&registry, target, scan, name)?,
        (None, None) => ClassSummary::NoMatch,
        _ => unreachable!(),
    };
    Ok(OwnerNewLookup { wez, tmux })
}

fn postcheck_owner_create(
    registry: &Registry,
    selected: OwnerCreateTarget<'_>,
    opposite: Option<OwnerCreateTarget<'_>>,
    name: &str,
    space_uid: SpaceUid,
    binding: &crate::backend::NativeBinding,
    allowed_opposite: Option<(SpaceUid, String)>,
) -> Result<(), OpError> {
    // Obtain both observations before classifying either.  A failure on one
    // provider never turns the other one's result into proof of absence.
    let selected_scan = selected.provider.inventory(selected.scope);
    let opposite_scan = opposite.map(|target| target.provider.inventory(target.scope));
    let selected_epoch = scan_epoch_for_create(selected, &selected_scan, true)?
        .expect("a selected complete scan has an epoch");
    if selected_epoch != binding.server_epoch {
        return Err(OpError::Indeterminate(format!(
            "{} create returned epoch {} but its locked post-scan observed {}",
            selected.backend, binding.server_epoch.0, selected_epoch.0
        )));
    }
    if let (Some(target), Some(scan)) = (opposite, opposite_scan.as_ref()) {
        scan_epoch_for_create(target, scan, false)?;
    }

    let InventoryOutcome::Complete(selected_inventory) = &selected_scan else {
        unreachable!("selected scan was classified complete above")
    };
    let exact_binding_rows = selected_inventory
        .rows
        .iter()
        .filter(|row| row.native_token == binding.native_token)
        .count();
    if exact_binding_rows != 1 {
        return Err(OpError::NameConflict(format!(
            "{} create post-scan found {exact_binding_rows} rows for native token {:?}",
            selected.backend, binding.native_token
        )));
    }
    if selected_inventory
        .rows
        .iter()
        .any(|row| row.native_name == name && row.native_token != binding.native_token)
    {
        return Err(OpError::NameConflict(format!(
            "an external {} resource raced creation of name {:?}",
            selected.backend, name
        )));
    }
    if let Some(scan) = opposite_scan.as_ref() {
        let InventoryOutcome::Complete(inventory) = scan else {
            // A stopped opposite server has no live allowed binding; it
            // cannot accompany an approved selectable opposite Space.
            if allowed_opposite.is_some() {
                return Err(OpError::Indeterminate(
                    "approved opposite Space stopped during create".into(),
                ));
            }
            // No live opposite exact-name row to compare.
            return postcheck_registry_names(
                registry,
                selected,
                opposite,
                name,
                space_uid,
                allowed_opposite.as_ref(),
            );
        };
        let same_name: Vec<_> = inventory
            .rows
            .iter()
            .filter(|row| row.native_name == name)
            .collect();
        match (&allowed_opposite, same_name.as_slice()) {
            (None, []) => {}
            (Some((_, allowed_token)), [row]) if &row.native_token == allowed_token => {}
            _ => {
                return Err(OpError::NameConflict(format!(
                    "opposite-backend exact-name rows changed during creation of {:?}",
                    name
                )));
            }
        }
    }

    postcheck_registry_names(
        registry,
        selected,
        opposite,
        name,
        space_uid,
        allowed_opposite.as_ref(),
    )
}

fn postcheck_registry_names(
    registry: &Registry,
    selected: OwnerCreateTarget<'_>,
    opposite: Option<OwnerCreateTarget<'_>>,
    name: &str,
    space_uid: SpaceUid,
    allowed_opposite: Option<&(SpaceUid, String)>,
) -> Result<(), OpError> {
    let selected_row = registry
        .live_space_by_name(selected.instance, name)
        .map_err(reg_err)?
        .ok_or_else(|| {
            OpError::Registry(format!(
                "reserved Space {} vanished before create finalization",
                space_uid.0
            ))
        })?;
    if selected_row.space_uid != space_uid {
        return Err(OpError::NameConflict(format!(
            "name {:?} was rebound to Space {} during create",
            name, selected_row.space_uid.0
        )));
    }
    if let Some(opposite) = opposite {
        let current = registry
            .live_space_by_name(opposite.instance, name)
            .map_err(reg_err)?;
        match (allowed_opposite, current) {
            (None, None) => {}
            (Some((allowed_uid, allowed_token)), Some(row)) if row.space_uid == *allowed_uid => {
                let current_binding = registry
                    .current_binding(row.space_uid)
                    .map_err(reg_err)?
                    .ok_or_else(|| {
                        OpError::NameConflict(
                            "approved opposite Space lost its current binding".into(),
                        )
                    })?;
                if current_binding.native_token != *allowed_token {
                    return Err(OpError::NameConflict(
                        "approved opposite Space changed its native binding".into(),
                    ));
                }
            }
            _ => {
                return Err(OpError::NameConflict(format!(
                    "opposite backend name {:?} changed during create",
                    name
                )));
            }
        }
    } else {
        let opposite_backend = match selected.backend {
            Backend::Wez => Backend::Tmux,
            Backend::Tmux => Backend::Wez,
        };
        if let Some(instance) = registry
            .backend_instance_for_backend(opposite_backend)
            .map_err(reg_err)?
        {
            return Err(OpError::NameConflict(format!(
                "opposite {opposite_backend} instance {} appeared during create",
                instance.0
            )));
        }
    }
    Ok(())
}

/// Exact owner-fenced cross-backend create used by GUI inherited-backend
/// creation and by the remote owner RPC (plan §8.2 / §10.1).
///
/// This is intentionally create-only: callers resolve an existing Space
/// before entering it.  Any live managed or unmanaged exact-name match on
/// either backend refuses before reservation.  `selected` is the inherited
/// backend and is the only adapter whose `create` method can be invoked;
/// there is no opposite-backend fallback path.
///
/// `opposite=None` is authoritative only when this function proves, while
/// holding the exact-name decision lock, that no durable opposite managed
/// instance row exists.  If such a row exists, its exact target is required
/// and both inventories are scanned under all instance locks.
pub fn create_space_owner_fenced(
    env: &OperationEnv,
    selected: OwnerCreateTarget<'_>,
    opposite: Option<OwnerCreateTarget<'_>>,
    allow_opposite_selectable: bool,
    req: &CreateRequest,
) -> Result<CreatedSpace, OpError> {
    if opposite.is_some_and(|target| target.backend == selected.backend) {
        return Err(OpError::Refused(
            "opposite create target names the selected backend".into(),
        ));
    }

    let mut registry =
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).map_err(reg_err)?;
    let identity = registry.identity().map_err(reg_err)?;
    if let Some(replayed) = replayed_create(
        &mut registry,
        req,
        selected.backend,
        Some(allow_opposite_selectable),
    )? {
        return Ok(replayed);
    }

    let mut locks = OrderedLocks::new(&env.lock_dir);
    locks
        .acquire(LockScope::AuthorityGate, LockMode::Shared)
        .map_err(|error| OpError::Lock(error.to_string()))?;
    locks
        .acquire_decisions(identity.host_uid, &[req.name.as_str()], LockMode::Exclusive)
        .map_err(|error| OpError::Lock(error.to_string()))?;

    // A concurrent retry with the same request UID may have completed while
    // this invocation waited for the decision lock.  Re-read the ledger
    // before treating its now-live native row as a name collision.
    if let Some(replayed) = replayed_create(
        &mut registry,
        req,
        selected.backend,
        Some(allow_opposite_selectable),
    )? {
        return Ok(replayed);
    }

    let durable_selected = registry
        .backend_instance_for_backend(selected.backend)
        .map_err(reg_err)?
        .ok_or_else(|| {
            OpError::Refused(format!(
                "owner has no durable {} backend instance",
                selected.backend
            ))
        })?;
    if durable_selected != selected.instance {
        return Err(OpError::Refused(format!(
            "selected {} instance {} is not the owner's durable instance {}",
            selected.backend, selected.instance.0, durable_selected.0
        )));
    }
    validate_owner_create_target(&registry, identity.host_uid, selected)?;

    let opposite_backend = match selected.backend {
        Backend::Wez => Backend::Tmux,
        Backend::Tmux => Backend::Wez,
    };
    let durable_opposite = registry
        .backend_instance_for_backend(opposite_backend)
        .map_err(reg_err)?;
    let opposite = match (durable_opposite, opposite) {
        (None, None) => None,
        (None, Some(_)) => {
            return Err(OpError::Refused(format!(
                "caller supplied an unregistered {opposite_backend} create target"
            )));
        }
        (Some(instance), None) => {
            return Err(OpError::Refused(format!(
                "owner has durable {opposite_backend} instance {}; its determinate inventory is required",
                instance.0
            )));
        }
        (Some(instance), Some(target)) if target.instance != instance => {
            return Err(OpError::Refused(format!(
                "opposite {opposite_backend} target {} is not the owner's durable instance {}",
                target.instance.0, instance.0
            )));
        }
        (Some(_), Some(target)) if target.backend != opposite_backend => {
            return Err(OpError::Refused(format!(
                "opposite target is {}, expected {opposite_backend}",
                target.backend
            )));
        }
        (Some(_), Some(target)) => {
            validate_owner_create_target(&registry, identity.host_uid, target)?;
            Some(target)
        }
    };

    // §10.1: selected exclusive, opposite shared, always in canonical
    // BackendInstanceUid order.  Opposite creates need that shared lock
    // exclusively, so the decision remains stable through our post-scan.
    let mut instance_locks = vec![(selected.instance, LockMode::Exclusive)];
    if let Some(target) = opposite {
        if target.instance == selected.instance {
            return Err(OpError::Refused(
                "both backend kinds resolve to one backend instance".into(),
            ));
        }
        instance_locks.push((target.instance, LockMode::Shared));
    }
    instance_locks.sort_by_key(|(instance, _)| instance.0);
    for (instance, mode) in instance_locks {
        locks
            .acquire(LockScope::BackendInstance(instance), mode)
            .map_err(|error| OpError::Lock(error.to_string()))?;
    }
    require_no_unfinished_recovery(&registry, selected.instance)?;
    if let Some(target) = opposite {
        require_no_unfinished_recovery(&registry, target.instance)?;
    }

    // The registry-published incarnations are verified against the live
    // servers before either is listed (WS-A.9), and the epochs the scans
    // answer under must be the published ones: a create decides a name and
    // journals a bootstrap row from these scans.
    let selected_published =
        verify_published_incarnation(&registry, selected.instance, selected.scope)?;
    let opposite_published = opposite
        .map(|target| verify_published_incarnation(&registry, target.instance, target.scope))
        .transpose()?;

    // Read both providers before classifying either result.  A determinate
    // stopped opposite server is empty; the selected server must be a
    // complete, epoched inventory because it is about to mutate.
    let selected_scan = selected.provider.inventory(selected.scope);
    let opposite_scan = opposite.map(|target| target.provider.inventory(target.scope));
    let selected_epoch = scan_epoch_for_create(selected, &selected_scan, true)?
        .expect("a selected complete scan has an epoch");
    require_published_epoch(&selected_published, selected_epoch)?;
    if let (Some(target), Some(scan)) = (opposite, opposite_scan.as_ref())
        && let Some(opposite_epoch) = scan_epoch_for_create(target, scan, false)?
    {
        let published = opposite_published
            .as_ref()
            .expect("verified with the target");
        require_published_epoch(published, opposite_epoch)?;
    }

    // Join the determinate native scans with durable authority state before
    // consuming SpaceUid/SpaceNo. Selected must be empty. The explicit
    // collision acknowledgement may preserve exactly one opposite managed,
    // selectable row; it never waives unmanaged/blocking/indeterminate state.
    match summarize_owner_target(&registry, selected, &selected_scan, &req.name)? {
        ClassSummary::NoMatch => {}
        summary => {
            return Err(OpError::NameConflict(format!(
                "selected {} exact-name state is {summary:?}",
                selected.backend
            )));
        }
    }
    let mut allowed_opposite = None;
    if let (Some(target), Some(scan)) = (opposite, opposite_scan.as_ref()) {
        match summarize_owner_target(&registry, target, scan, &req.name)? {
            ClassSummary::NoMatch => {}
            ClassSummary::Selectable { space, .. } if allow_opposite_selectable => {
                let binding = registry
                    .current_binding(space)
                    .map_err(reg_err)?
                    .ok_or_else(|| {
                        OpError::NameConflict(
                            "selectable opposite Space lost its current binding".into(),
                        )
                    })?;
                allowed_opposite = Some((space, binding.native_token));
            }
            summary => {
                return Err(OpError::NameConflict(format!(
                    "opposite {opposite_backend} exact-name state is {summary:?}"
                )));
            }
        }
    }

    create_space_locked(
        env,
        &mut registry,
        identity.host_uid,
        selected.provider,
        selected.scope,
        selected.backend,
        selected.instance,
        selected_epoch,
        req,
        |registry, binding, space_uid| {
            postcheck_owner_create(
                registry,
                selected,
                opposite,
                &req.name,
                space_uid,
                binding,
                allowed_opposite.clone(),
            )
        },
    )
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
    require_no_unfinished_recovery(&registry, instance)?;

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

    // tmux renames the session natively; the binding it is handed carries
    // the registry's recorded epoch (WS-A.8), decided before the rename is
    // journaled so a stale binding never strands an unfinished operation.
    let tmux_native = if backend == Backend::Tmux {
        let binding = registry
            .current_binding(space_uid)
            .map_err(reg_err)?
            .ok_or_else(|| OpError::NotFound("no current native binding".into()))?;
        let epoch = match binding_epoch_for_adapter(&mut registry, scope, &binding, |_| Ok(false))?
        {
            BindingVerdict::Pinned(epoch) => epoch,
            BindingVerdict::AbsentUnderPin => {
                return Err(OpError::NotFound(format!(
                    "{} is not live under the published incarnation",
                    binding.native_token
                )));
            }
        };
        Some(crate::backend::NativeBinding {
            native_token: binding.native_token,
            server_epoch: epoch,
            root_group: ProviderHandle::Tx(0),
            root_split: ProviderHandle::Tx(0),
        })
    } else {
        None
    };

    let operation_uid = registry
        .begin_rename(space_uid, new_name, request_uid)
        .map_err(reg_err)?;
    if let Some(native) = tmux_native {
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
    remove_space_inner(env, provider, scope, backend, space_uid, request_uid, None)
}

/// Resume only the exact unfinished remove after acknowledgement/process
/// loss.  This is intentionally a separate seam: callers may not turn an
/// arbitrary `deleting` row into authority to kill.  The same lock order,
/// recovery-journal guard, native absence proof, and final-Wez empty floor
/// as a fresh remove are applied before the tombstone is completed.
pub fn resume_remove_space(
    env: &OperationEnv,
    provider: &dyn Provider,
    scope: &InventoryScope,
    backend: Backend,
    space_uid: SpaceUid,
    request_uid: Uuid,
    operation_uid: Uuid,
) -> Result<(), OpError> {
    remove_space_inner(
        env,
        provider,
        scope,
        backend,
        space_uid,
        request_uid,
        Some(operation_uid),
    )
}

#[allow(clippy::too_many_arguments)]
fn remove_space_inner(
    env: &OperationEnv,
    provider: &dyn Provider,
    scope: &InventoryScope,
    backend: Backend,
    space_uid: SpaceUid,
    request_uid: Uuid,
    resume_operation: Option<Uuid>,
) -> Result<(), OpError> {
    let mut registry =
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).map_err(reg_err)?;
    let instance = registry
        .register_backend_instance(backend, Some(&scope.endpoint), None)
        .map_err(reg_err)?;
    let space = registry
        .space(space_uid)
        .map_err(|e| OpError::NotFound(e.to_string()))?;
    if space.backend_instance != instance {
        return Err(OpError::Refused(format!(
            "Space {} belongs to backend instance {}, not {}",
            space_uid.0, space.backend_instance.0, instance.0
        )));
    }

    let mut locks = OrderedLocks::new(&env.lock_dir);
    locks
        .acquire(LockScope::AuthorityGate, LockMode::Shared)
        .map_err(|e| OpError::Lock(e.to_string()))?;
    locks
        .acquire(LockScope::BackendInstance(instance), LockMode::Exclusive)
        .map_err(|e| OpError::Lock(e.to_string()))?;
    require_no_unfinished_recovery(&registry, instance)?;
    locks
        .acquire(LockScope::Space(space_uid), LockMode::Exclusive)
        .map_err(|e| OpError::Lock(e.to_string()))?;

    // The binding handed to the provider carries the registry's recorded
    // epoch (WS-A.8). Decided before the deleting intent is journaled, so a
    // stale tmux binding is refused without stranding an unfinished remove;
    // a wez key that a complete pinned scan no longer lists has nothing live
    // to kill and the explicit removal proceeds straight to its tombstone.
    let native = match registry.current_binding(space_uid).map_err(reg_err)? {
        Some(binding) => match binding_epoch_for_adapter(&mut registry, scope, &binding, |pin| {
            native_token_live_under(provider, scope, pin, &binding.native_token)
        })? {
            BindingVerdict::Pinned(epoch) => Some(crate::backend::NativeBinding {
                native_token: binding.native_token,
                server_epoch: epoch,
                root_group: ProviderHandle::Tx(0),
                root_split: ProviderHandle::Tx(0),
            }),
            BindingVerdict::AbsentUnderPin => None,
        },
        None => None,
    };

    let operation_uid = match resume_operation {
        None => registry
            .begin_remove(space_uid, request_uid)
            .map_err(reg_err)?,
        Some(operation_uid) => {
            let current = registry.space(space_uid).map_err(reg_err)?;
            let operation = registry.operation(operation_uid).map_err(reg_err)?;
            if current.lifecycle != crate::model::Lifecycle::Deleting
                || operation.space_uid != space_uid
                || operation.kind != crate::model::OperationKind::Remove
                || operation.request_uid != request_uid
                || operation.state.is_terminal()
            {
                return Err(OpError::Refused(format!(
                    "remove resume {} does not own the exact unfinished delete for Space {}",
                    operation_uid, space_uid.0
                )));
            }
            operation_uid
        }
    };
    if let Some(native) = native {
        match provider.remove(scope, &native) {
            Ok(()) => {}
            Err(crate::backend::ProviderError::NotFound { .. }) if resume_operation.is_some() => {}
            Err(e) => {
                // Non-convergence or provider failure: the operation stays
                // journaled (deleting + unfinished op); never tombstone.
                return Err(OpError::Provider(format!("{e:?}")));
            }
        }
    }

    // P10 intentional-empty guard (§15.3). First prove emptiness while the
    // exact backend-instance lock excludes every create/restore/snapshot
    // writer; below, the tombstone and its resulting recovery floor commit
    // in one registry transaction. If this is the final durable Wez Space,
    // an indeterminate scan is not a successful remove: leaving the journal
    // in `deleting` is safer than allowing a later cold start to resurrect a
    // manifest we could not fence below.
    let final_wez_empty_epoch = if backend == Backend::Wez {
        let final_durable_space = !registry.spaces().map_err(reg_err)?.iter().any(|row| {
            row.backend_instance == instance
                && row.space_uid != space_uid
                && row.lifecycle == crate::model::Lifecycle::Active
        });
        if final_durable_space {
            classify_final_wez_empty_scan(scope.expected_epoch(), provider.inventory(scope))?
        } else {
            None
        }
    } else {
        None
    };

    if let Some(epoch) = final_wez_empty_epoch {
        let backend_scope = LockScope::BackendInstance(instance);
        let kernel = locks.held(&backend_scope).ok_or_else(|| {
            OpError::Lock("intentional-empty update lost its backend-instance lock".into())
        })?;
        registry
            .complete_remove_intentionally_empty(space_uid, operation_uid, instance, epoch, kernel)
            .map_err(reg_err)?;
    } else {
        registry
            .complete_remove(space_uid, operation_uid)
            .map_err(reg_err)?;
    }
    Ok(())
}

fn classify_final_wez_empty_scan(
    expected_epoch: Option<ServerEpoch>,
    outcome: InventoryOutcome,
) -> Result<Option<ServerEpoch>, OpError> {
    match outcome {
        InventoryOutcome::Complete(inv) => {
            let live_epoch = inv.server_epoch.ok_or_else(|| {
                OpError::Indeterminate("final Wez removal requires an epoched empty scan".into())
            })?;
            let expected = expected_epoch.ok_or_else(|| {
                OpError::Indeterminate("final Wez removal requires the current server epoch".into())
            })?;
            if live_epoch != expected {
                return Err(OpError::Indeterminate(format!(
                    "final Wez removal scan changed epoch from {} to {}",
                    expected.0, live_epoch.0
                )));
            }
            Ok(inv.rows.is_empty().then_some(live_epoch))
        }
        other => Err(OpError::Indeterminate(format!(
            "final Wez removal could not prove intentional empty: {other:?}"
        ))),
    }
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

// The adoption refusals §10.3 gives their own remedies travel as a leading
// token in the detail, exactly as `cas_capability_missing` already does:
// `OpError` carries no identity/invalid-name variant, and six verbs match it
// exhaustively, so widening it is not this layer's call. `adopt_cli::typed`
// lifts these back into the plan's codes (§16.2).
pub const ADOPT_IDENTITY_CONFLICT: &str = "native_identity_conflict";
pub const ADOPT_MARKER_CONFLICT: &str = "marker_conflict";
pub const ADOPT_UNRENDERABLE_NAME: &str = "unrenderable_native_name";

/// Case 13's pre-mutation guard: a native token that already carries a
/// current binding is an explicit conflict. `bindings_current_native_uq`
/// states the same rule, but enforces it only at finalization — by then the
/// markers (tmux) or the CAS rename (Wez) have already rewritten a live
/// resource that belongs to another Space, and no verb reaps the wreckage.
fn require_unbound_native(
    registry: &Registry,
    instance: crate::model::BackendInstanceUid,
    native_token: &str,
) -> Result<(), OpError> {
    let Some(bound) = registry
        .current_binding_by_native(instance, native_token)
        .map_err(reg_err)?
    else {
        return Ok(());
    };
    let held = registry.space(bound.space_uid).map_err(reg_err)?;
    Err(OpError::NameConflict(format!(
        "{ADOPT_IDENTITY_CONFLICT}: {native_token} is already bound to Space {} ({}, {:?})",
        held.space_no, bound.space_uid.0, held.logical_name
    )))
}

/// §2.12 and case 6: the name has to be free on *both* providers, which is
/// what `new` checks and adopt did not. `new` offers `--allow-name-collision`
/// to take the collision deliberately; `dmux adopt` has no such flag yet
/// (adding one is a `main.rs` change), so the remedy here is `--name`.
fn require_no_cross_backend_name(
    registry: &Registry,
    backend: Backend,
    name: &str,
) -> Result<(), OpError> {
    let opposite = match backend {
        Backend::Wez => Backend::Tmux,
        Backend::Tmux => Backend::Wez,
    };
    let Some(instance) = registry
        .backend_instance_for_backend(opposite)
        .map_err(reg_err)?
    else {
        return Ok(());
    };
    match registry
        .live_space_by_name(instance, name)
        .map_err(reg_err)?
    {
        None => Ok(()),
        Some(existing) => Err(OpError::NameConflict(format!(
            "name {:?} is held on {opposite} by Space {} ({}); adopt it under an explicit \
             --name (adopt carries no --allow-name-collision acknowledgement)",
            name, existing.space_no, existing.space_uid.0
        ))),
    }
}

/// An inherited native name keeps its legacy spelling (§10.3), so the
/// `new`-name grammar cannot apply to it — but it still has to survive the
/// line-oriented renderers `ls`/receipts use. Operator-chosen `--name` is
/// held to the full grammar one layer up, where `invalid_name` is spellable.
fn require_renderable_name(name: &str) -> Result<(), OpError> {
    if name.trim().is_empty() || name.chars().any(char::is_control) {
        return Err(OpError::NameConflict(format!(
            "{ADOPT_UNRENDERABLE_NAME}: native name {name:?} is blank or holds control \
             characters; adopt it with an explicit --name"
        )));
    }
    Ok(())
}

/// Case 13 again, from the resource's side: a session already advertising
/// `@dmux_*` identity belongs to some authority, and overwriting its markers
/// is precisely the silent rebind §10.3 forbids. The one exception is this
/// registry's own abandoned stamp — the Space it names is no longer live
/// here — because that is what a torn adoption leaves behind and re-adopting
/// is the documented repair for it.
fn require_no_foreign_stamp(
    registry: &Registry,
    identity: &crate::registry::RegistryIdentity,
    session: &str,
    markers: &crate::backend::tmux::SpaceMarkerReadback,
) -> Result<(), OpError> {
    let conflict = |detail: String| {
        Err(OpError::NameConflict(format!(
            "{ADOPT_MARKER_CONFLICT}: session {session} {detail}; resolve the collision before \
             adopting (plan §10.3)"
        )))
    };
    let Some(space_uid) = markers.space_uid.as_deref() else {
        // No identity claimed. A partial stamp without a Space UID names
        // nothing adoptable and is overwritten with the rest.
        return Ok(());
    };
    let ours = markers.host_uid.as_deref() == Some(identity.host_uid.0.to_string().as_str())
        && markers.registry_uid.as_deref() == Some(identity.registry_uid.0.to_string().as_str());
    if !ours {
        return conflict(format!(
            "already carries foreign dmux markers (host {:?}, registry {:?}, space {space_uid})",
            markers.host_uid, markers.registry_uid
        ));
    }
    let live = space_uid
        .parse()
        .ok()
        .map(|uid| registry.space(SpaceUid(uid)))
        .transpose()
        .or_else(|e| match e {
            crate::registry::RegistryError::NotFound { .. } => Ok(None),
            other => Err(reg_err(other)),
        })?
        .filter(|row| row.lifecycle.occupies_name());
    match live {
        None => Ok(()),
        Some(row) => conflict(format!(
            "still carries this registry's markers for live Space {} ({})",
            row.space_no, row.space_uid.0
        )),
    }
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
    require_renderable_name(&name)?;

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
    require_no_unfinished_recovery(&registry, instance)?;

    // Everything that can refuse this adoption is decided here, under the
    // lease and before a single byte of the live session changes: identity
    // first (the session may already be some Space's), then both name
    // occupancies, then the session's own claim to identity.
    require_unbound_native(&registry, instance, session_id)?;
    if let Some(existing) = registry
        .live_space_by_name(instance, &name)
        .map_err(reg_err)?
    {
        return Err(OpError::NameConflict(format!(
            "name {:?} is held by Space {}",
            name, existing.space_uid.0
        )));
    }
    require_no_cross_backend_name(&registry, Backend::Tmux, &name)?;
    let existing_markers = provider
        .read_markers(scope, session_id)
        .map_err(|e| OpError::Provider(format!("{e:?}")))?;
    require_no_foreign_stamp(&registry, &identity, session_id, &existing_markers)?;

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
    if let Err(e) = registry.finalize_adopt(
        reservation.space_uid,
        reservation.operation_uid,
        &NativeBindingSpec {
            native_token: session_id.to_string(),
            native_kind: NativeKind::TmuxSessionId,
            server_epoch: Some(epoch),
        },
    ) {
        // Without this the reservation stays `reserved` forever — it holds
        // the name against every later attempt and no verb reaps it. The
        // markers are already on the session and tmux options cannot be
        // unset through this provider; aborting is what makes them this
        // registry's *abandoned* stamp, which `require_no_foreign_stamp`
        // deliberately lets a retry overwrite.
        let _ = registry.abort_create(reservation.space_uid, reservation.operation_uid);
        return Err(OpError::Registry(format!(
            "{e}; Space {} aborted and session {session_id} still carries its stamp — re-run \
             `dmux adopt` to reclaim it",
            reservation.space_uid.0
        )));
    }
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
    require_renderable_name(&name)?;

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
    require_no_unfinished_recovery(&registry, instance)?;

    // Same ordering rule as tmux: the CAS rename below is a real mutation,
    // so identity is settled first. A workspace already carrying an opaque
    // key is some Space's binding — renaming it to a *new* key would strand
    // that Space on a token nothing answers to.
    require_unbound_native(&registry, instance, source_workspace)?;
    if let Some(existing) = registry
        .live_space_by_name(instance, &name)
        .map_err(reg_err)?
    {
        return Err(OpError::NameConflict(format!(
            "name {:?} is held by Space {}",
            name, existing.space_uid.0
        )));
    }
    require_no_cross_backend_name(&registry, Backend::Wez, &name)?;
    let reservation = registry
        .reserve_space_kind(
            &name,
            instance,
            request_uid,
            crate::model::OperationKind::Adopt,
        )
        .map_err(reg_err)?;
    // The exact key `reconcile_apply` will look for if this holder dies
    // between the CAS below and `finalize_adopt`.
    let opaque_key = adoption_key(identity.host_uid, reservation.space_uid);

    match provider.cas_rename_workspace(scope, window_id, source_workspace, &opaque_key, true) {
        Ok(CasRenameOutcome::Renamed) => {}
        // A window that vanished under the CAS is gone, not contested; the
        // tmux path answers the same disappearance with `not_found`.
        Ok(CasRenameOutcome::NoSuchWindow) => {
            let _ = registry.abort_create(reservation.space_uid, reservation.operation_uid);
            return Err(OpError::NotFound(format!(
                "workspace {source_workspace:?} vanished before the atomic rename \
                 (zero mutation)"
            )));
        }
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
    if let Err(e) = registry.finalize_adopt(
        reservation.space_uid,
        reservation.operation_uid,
        &NativeBindingSpec {
            native_token: opaque_key.clone(),
            native_kind: NativeKind::WezWorkspaceKey,
            server_epoch: Some(epoch),
        },
    ) {
        // The rename already landed, so an abort alone would leave the
        // workspace wearing an opaque key for a Space that never existed —
        // neither managed nor recoverably unmanaged. Put the name back
        // first, under the same CAS guard so a racer's workspace is never
        // touched, and say so when that compensation itself fails.
        let restored = matches!(
            provider.cas_rename_workspace(scope, window_id, &opaque_key, source_workspace, true),
            Ok(CasRenameOutcome::Renamed)
        );
        let _ = registry.abort_create(reservation.space_uid, reservation.operation_uid);
        return Err(OpError::Registry(format!(
            "{e}; Space {} aborted and workspace {}",
            reservation.space_uid.0,
            if restored {
                format!("restored to {source_workspace:?}")
            } else {
                format!(
                    "still named {opaque_key:?} — rename it back to {source_workspace:?} before \
                     retrying"
                )
            }
        )));
    }
    Ok(AdoptedSpace {
        space_uid: reservation.space_uid,
        space_no: reservation.space_no,
        name,
        native_token: opaque_key,
    })
}

// ---------------------------------------------------------------------------
// P8a: child (Group/Split) operations, hierarchy reads, and marker context
// (plan §7.2, §11.3, §13.1). Same fenced skeleton as the Space flows: rpc
// ledger → §10.1 locks → registry + complete same-epoch scan guards →
// journaled child bootstrap → provider mutation → witness/correlate →
// payload/ack. Children are epoch-qualified live refs, never durable rows.

use crate::backend::{NativeSpaceRow, SplitDirection, SplitSpec};

/// Where a child's working directory came from (plan §11.3; JSON reports
/// `cwd_source` and any fallback is visible, never silent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CwdSource {
    Explicit,
    TargetSplit,
    OwnerHome,
    /// No usable directory was derivable; the backend's own default applies
    /// (tmux: the invoking client's cwd — pinned in the provider suite).
    NativeDefault,
}

/// §11.3 fallback tail: owner home, visibly, before the native default.
fn home_fallback() -> (Option<String>, CwdSource) {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() => (Some(home), CwdSource::OwnerHome),
        _ => (None, CwdSource::NativeDefault),
    }
}

pub struct GroupNewRequest {
    pub request_uid: Uuid,
    pub space_uid: SpaceUid,
    pub cwd: Option<String>,
    /// User program; empty means a login shell.
    pub program: Vec<String>,
    pub helper_bin: String,
}

pub struct SplitNewRequest {
    pub request_uid: Uuid,
    pub space_uid: SpaceUid,
    /// The target Group (epoch-qualified). A stale epoch fails, never
    /// retargets (plan §6.3).
    pub group: ChildRefShape,
    pub direction: SplitDirection,
    pub percent: Option<u8>,
    pub cwd: Option<String>,
    pub program: Vec<String>,
    pub helper_bin: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CreatedChild {
    pub space_uid: SpaceUid,
    pub kind: ChildKind,
    /// Epoch-qualified refs: the (new or parent) Group and the new Split.
    pub group_ref: String,
    pub split_ref: String,
    pub cwd_source: CwdSource,
    #[serde(default)]
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RemovedChild {
    pub space_uid: SpaceUid,
    pub kind: ChildKind,
    pub handle: String,
    #[serde(default)]
    pub replayed: bool,
}

/// Read-only hierarchy of one Space under its current epoch (plan §7.2
/// `group ls` / `split ls` / `ls --tree`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SpaceHierarchy {
    pub space_uid: SpaceUid,
    pub server_epoch: ServerEpoch,
    pub groups: Vec<HierarchyGroup>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HierarchyGroup {
    pub group_ref: String,
    pub title: Option<String>,
    pub splits: Vec<HierarchySplit>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HierarchySplit {
    pub split_ref: String,
    pub title: Option<String>,
    pub cwd: Option<String>,
}

/// Authority-backed interpretation of one GUI pane marker.  The marker is
/// still only a locator: callers may use these display fields or construct a
/// signed presentation request only after this function has matched the
/// durable Space, published backend incarnation, and exact live Group/Split
/// parentage under the common backend read fence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedMarker {
    pub context: crate::bootstrap::MarkerContext,
    pub logical_name: String,
    pub backend_instance: crate::model::BackendInstanceUid,
    pub health: crate::model::Health,
    pub group_count: usize,
    pub split_count: usize,
    pub group_name: Option<String>,
}

/// Load an Active, currently bound Space or fail typed.
fn load_bound_space(
    registry: &mut Registry,
    space_uid: SpaceUid,
) -> Result<(crate::registry::SpaceRow, crate::registry::BindingRow), OpError> {
    let row = registry.space(space_uid).map_err(reg_err)?;
    if row.lifecycle != crate::model::Lifecycle::Active {
        return Err(OpError::NotFound(format!(
            "space {} is {:?}, not active",
            space_uid.0, row.lifecycle
        )));
    }
    let binding = registry
        .current_binding(space_uid)
        .map_err(reg_err)?
        .ok_or_else(|| {
            OpError::NotFound(format!("space {} has no current binding", space_uid.0))
        })?;
    Ok((row, binding))
}

/// What the registry's recorded binding epoch says about handing the
/// binding to an adapter under `scope` (see [`binding_epoch_for_adapter`]).
enum BindingVerdict {
    /// Hand the adapter a `NativeBinding` carrying this epoch — the
    /// registry's recorded value, which equals the scope's pin.
    Pinned(ServerEpoch),
    /// The binding was recorded under another incarnation and a complete
    /// scan under the pin lists no such native resource: there is nothing
    /// live to hand an adapter. Only a removal may proceed from here, and
    /// without a native command.
    AbsentUnderPin,
}

/// The epoch carried by the `NativeBinding` handed to an adapter is the
/// REGISTRY's recorded binding epoch, never the scan's own word and never
/// the pin copied across (review findings #5/#18, ADR 012 WS-A.8): the
/// adapters' `binding_epoch` compares that value against the scope's pin,
/// and that comparison is only a fence when the two have independent
/// sources. The registry row is also consulted here first, so a stale
/// binding is refused before any journal row or native command.
///
/// A binding recorded under the pinned epoch is handed as is. One recorded
/// under another incarnation (or none) depends on the native kind:
///
/// * `tmux_session_id` — server-minted and recycled by the next
///   incarnation (plan §11.2: a restart invalidates prior refs). Nothing can
///   prove a `$N` on the new server is this Space, so it is refused typed;
///   the Space is absent until `dmux repair rebind` names its new session.
/// * `wez_workspace_key` — registry-minted identity that survives a restart
///   (cold recovery restores the key, plan §15.3 step 8). The caller proves
///   the key live by a complete scan under the pin (`live_under_pin`); the
///   recorded epoch is then refreshed as observation metadata
///   (`Registry::observe_binding_epoch`) and handed. A key the pinned scan
///   does not list is [`BindingVerdict::AbsentUnderPin`].
fn binding_epoch_for_adapter(
    registry: &mut Registry,
    scope: &InventoryScope,
    binding: &crate::registry::BindingRow,
    live_under_pin: impl FnOnce(ServerEpoch) -> Result<bool, OpError>,
) -> Result<BindingVerdict, OpError> {
    let pin = scope.expected_epoch().ok_or_else(|| {
        OpError::Indeterminate(
            "a native binding is handed to a provider only under a scope pinned to the \
             registry-published server epoch"
                .into(),
        )
    })?;
    let recorded = registry
        .current_binding_epoch(binding.space_uid)
        .map_err(reg_err)?;
    if recorded == Some(pin) {
        return Ok(BindingVerdict::Pinned(pin));
    }
    let recorded_text = recorded
        .map(|epoch| epoch.0.to_string())
        .unwrap_or_else(|| "<none>".to_string());
    match binding.native_kind {
        NativeKind::TmuxSessionId => Err(OpError::StaleRef(format!(
            "binding {} of space {} was recorded under server epoch {recorded_text} but the \
             published incarnation is {}; a tmux session id does not survive a server restart \
             (plan §11.2), so the Space is absent until `dmux repair rebind` names its session",
            binding.native_token, binding.space_uid.0, pin.0
        ))),
        NativeKind::WezWorkspaceKey => {
            if live_under_pin(pin)? {
                registry
                    .observe_binding_epoch(binding.space_uid, pin)
                    .map_err(reg_err)?;
                Ok(BindingVerdict::Pinned(pin))
            } else {
                Ok(BindingVerdict::AbsentUnderPin)
            }
        }
    }
}

/// Whether a complete scan under exactly `pin` lists `native_token`. Any
/// other outcome — incomplete, unepoched, or answered under another epoch —
/// is indeterminate, never "absent".
fn native_token_live_under(
    provider: &dyn Provider,
    scope: &InventoryScope,
    pin: ServerEpoch,
    native_token: &str,
) -> Result<bool, OpError> {
    match provider.inventory(scope) {
        InventoryOutcome::Complete(inv) => {
            let observed = inv.server_epoch.ok_or_else(|| {
                OpError::Indeterminate("managed scan is complete but unepoched".into())
            })?;
            require_pinned_epoch(scope, observed)?;
            debug_assert_eq!(observed, pin);
            Ok(inv.rows.iter().any(|row| row.native_token == native_token))
        }
        other => Err(OpError::Indeterminate(format!("scan: {other:?}"))),
    }
}

/// Child mutations are blocked on unstamped/conflicted Spaces (plan §10.3);
/// listing and context remain allowed.
fn require_child_mutable(row: &crate::registry::SpaceRow) -> Result<(), OpError> {
    if row.health != crate::model::Health::Healthy {
        return Err(OpError::Refused(format!(
            "space {} health is {:?}; child operations are blocked until every \
             pane acknowledges its marker (`dmux context stamp`) or the space \
             is repaired",
            row.space_uid.0, row.health
        )));
    }
    Ok(())
}

/// Complete same-epoch scan that must contain the Space's native token.
/// Returns the live epoch and the Space's native row.
fn scan_space_row(
    provider: &dyn Provider,
    scope: &InventoryScope,
    native_token: &str,
) -> Result<(ServerEpoch, NativeSpaceRow), OpError> {
    match provider.inventory(scope) {
        InventoryOutcome::Complete(inv) => {
            let epoch = inv.server_epoch.ok_or_else(|| {
                OpError::Indeterminate("child operations require an epoched server".into())
            })?;
            let row = inv
                .rows
                .into_iter()
                .find(|r| r.native_token == native_token)
                .ok_or_else(|| {
                    OpError::NotFound(format!("native resource {native_token} not in the scan"))
                })?;
            Ok((epoch, row))
        }
        other => Err(OpError::Indeterminate(format!("scan: {other:?}"))),
    }
}

/// The registry-published incarnation of `instance`, verified against the
/// live server before it is trusted (ADR 001/002; ADR 012 WS-A.9, review
/// finding #11). For tmux the witnesses `tmux_bootstrap` recorded — pid,
/// start token, socket dev/ino — are compared against a fresh probe of the
/// namespace and a fresh `stat` of its socket, the way the wez sites compare
/// the ready descriptor: a replaced server that merely presents the old
/// `@dmux_server_epoch` is refused here, before it is even listed. The probe
/// runs only when the registry holds socket witnesses for the instance; a
/// row published before WS-A.9 carries none and is verified by epoch alone
/// ([`require_published_epoch`]).
fn verify_published_incarnation(
    registry: &Registry,
    instance: BackendInstanceUid,
    scope: &InventoryScope,
) -> Result<crate::registry::BackendServerRecord, OpError> {
    let published = registry.backend_server(instance).map_err(reg_err)?;
    if scope.backend == Backend::Tmux
        && (published.socket_dev.is_some() || published.socket_ino.is_some())
    {
        let probe: TmuxProvider<SystemRunner> = TmuxProvider::new(scope.endpoint.as_str());
        let live = probe.server_incarnation(&scope.endpoint).map_err(|e| {
            OpError::Indeterminate(format!(
                "tmux incarnation probe on namespace {:?}: {e:?}",
                scope.endpoint
            ))
        })?;
        let recorded = (
            published.server_pid,
            published.server_start_token.clone(),
            published.socket_dev,
            published.socket_ino,
        );
        let observed = (
            Some(i64::from(live.identity.pid)),
            Some(live.identity.start_token.clone()),
            i64::try_from(live.socket_dev).ok(),
            i64::try_from(live.socket_ino).ok(),
        );
        if recorded != observed {
            return Err(OpError::StaleRef(format!(
                "tmux server on namespace {:?} is not the registry-published incarnation: \
                 registry pid {:?} start {:?} socket dev/ino {:?}/{:?}; live pid {} start {:?} \
                 socket {:?} dev/ino {}/{} — the published incarnation is stale (ADR 012 §3.1 \
                 state F); re-run `dmux _tmux-bootstrap` on the live server",
                scope.endpoint,
                published.server_pid,
                published.server_start_token,
                published.socket_dev,
                published.socket_ino,
                live.identity.pid,
                live.identity.start_token,
                live.socket_path,
                live.socket_dev,
                live.socket_ino
            )));
        }
    }
    Ok(published)
}

/// The live epoch a scan answered under must be the one the registry
/// publishes for the instance; anything else is an incarnation nothing
/// verified (finding #8's "stale live epoch" and "registry NULL" cases).
fn require_published_epoch(
    published: &crate::registry::BackendServerRecord,
    live: ServerEpoch,
) -> Result<(), OpError> {
    if published.server_epoch != Some(live) {
        return Err(OpError::Indeterminate(format!(
            "live epoch {} is not the registry-published backend incarnation ({})",
            live.0,
            published
                .server_epoch
                .map(|epoch| epoch.0.to_string())
                .unwrap_or_else(|| "unpublished".to_string())
        )));
    }
    Ok(())
}

/// The complete scan every child verb mutates from, fenced the three ways
/// plan §11.2 names ("rechecks socket, PID/start token, and epoch
/// immediately before mutation"): the published incarnation is verified
/// against the live server first, so a replaced server is never listed;
/// the scan must answer under the scope's pin; and that pin must be the
/// epoch the registry publishes.
fn fenced_space_scan(
    registry: &Registry,
    provider: &dyn Provider,
    scope: &InventoryScope,
    instance: BackendInstanceUid,
    native_token: &str,
) -> Result<(ServerEpoch, NativeSpaceRow), OpError> {
    let published = verify_published_incarnation(registry, instance, scope)?;
    let (observed, native_row) = scan_space_row(provider, scope, native_token)?;
    let epoch = require_pinned_epoch(scope, observed)?;
    require_published_epoch(&published, epoch)?;
    Ok((epoch, native_row))
}

/// Stale-epoch rejection (plan §6.3): an epoch-qualified child ref from a
/// previous incarnation fails, never retargets.
fn require_live_epoch(child: &ChildRefShape, live: ServerEpoch) -> Result<(), OpError> {
    if child.epoch != live {
        return Err(OpError::StaleRef(format!(
            "child ref epoch {} does not match the live server epoch {}; \
             refresh with `dmux ls --tree`",
            child.epoch.0, live.0
        )));
    }
    Ok(())
}

fn make_ref(kind: ChildKind, epoch: ServerEpoch, handle: &ProviderHandle) -> String {
    child_suffix(&ChildRefShape {
        kind,
        epoch,
        handle: handle.clone(),
    })
}

struct ChildLocks;

impl ChildLocks {
    /// §10.1 order for a child mutation: gate shared → backend-instance
    /// exclusive → space exclusive. No decision lock: child titles are not
    /// authority names.
    fn acquire(
        locks: &mut OrderedLocks,
        registry: &Registry,
        instance: crate::model::BackendInstanceUid,
        space: SpaceUid,
    ) -> Result<(), OpError> {
        locks
            .acquire(LockScope::AuthorityGate, LockMode::Shared)
            .map_err(|e| OpError::Lock(e.to_string()))?;
        locks
            .acquire(LockScope::BackendInstance(instance), LockMode::Exclusive)
            .map_err(|e| OpError::Lock(e.to_string()))?;
        require_no_unfinished_recovery(registry, instance)?;
        locks
            .acquire(LockScope::Space(space), LockMode::Exclusive)
            .map_err(|e| OpError::Lock(e.to_string()))?;
        Ok(())
    }
}

/// Replay guard shared by the child mutations: returns the original result
/// when the request UID already completed.
fn child_replay<T: serde::de::DeserializeOwned>(
    registry: &mut Registry,
    request_uid: Uuid,
    method: &str,
    digest: &str,
    mark_replayed: impl FnOnce(&mut T),
) -> Result<Option<T>, OpError> {
    match registry
        .record_rpc_request(request_uid, method, digest)
        .map_err(reg_err)?
    {
        crate::registry::RpcDisposition::Replay {
            result_json: Some(result),
            ..
        } => {
            let mut value: T =
                serde_json::from_value(result).map_err(|e| OpError::Registry(e.to_string()))?;
            mark_replayed(&mut value);
            Ok(Some(value))
        }
        _ => Ok(None),
    }
}

/// The shared child-bootstrap tail: witness → correlate → payload/ack →
/// completed. `group`/`split` are the provider-verified handles of the new
/// pane's Group and the new Split itself.
#[allow(clippy::too_many_arguments)]
fn finish_child_bootstrap(
    registry: &mut Registry,
    paths: &bootstrap::BootstrapPaths,
    boot_uid: Uuid,
    identity: &crate::registry::RegistryIdentity,
    backend: Backend,
    epoch: ServerEpoch,
    space_uid: SpaceUid,
    space_no: SpaceNo,
    group: &ProviderHandle,
    split: &ProviderHandle,
) -> Result<(String, String), OpError> {
    let fail = |registry: &mut Registry, state: BootstrapState, err: OpError| {
        let _ = registry.bootstrap_state(boot_uid, state);
        bootstrap::cleanup(paths);
        err
    };

    let pane_env = bootstrap::read_pane_env(paths, std::time::Duration::from_secs(10))
        .map_err(|e| OpError::Bootstrap(e.to_string()))?;
    if let Some(record) = &pane_env {
        let inherited = record
            .wezterm_pane
            .as_deref()
            .or(record.tmux_pane.as_deref());
        if let Some(env_id) = inherited
            && !witness_matches(split, env_id)
        {
            return Err(fail(
                registry,
                BootstrapState::Conflict,
                OpError::Bootstrap(format!(
                    "helper inherited pane {env_id} but the provider verified {split}"
                )),
            ));
        }
    }

    let group_ref = make_ref(ChildKind::Group, epoch, group);
    let split_ref = make_ref(ChildKind::Split, epoch, split);
    registry
        .bootstrap_correlated(boot_uid, &group_ref, &split_ref)
        .map_err(|e| OpError::Bootstrap(e.message))?;

    let payload = BootstrapResult {
        request_uid: boot_uid,
        context: MarkerContext {
            host_uid: identity.host_uid,
            space_uid,
            space_no,
            backend,
            domain: None,
            server_epoch: epoch,
            group_ref: group_ref.clone(),
            split_ref: split_ref.clone(),
        },
    };
    if let Err(e) = bootstrap::send_result(paths, &payload, std::time::Duration::from_secs(10)) {
        return Err(fail(
            registry,
            BootstrapState::Timeout,
            OpError::Bootstrap(format!("helper gone before payload: {e:?}")),
        ));
    }
    if bootstrap::read_ack(paths, std::time::Duration::from_secs(10))
        .map_err(|e| OpError::Bootstrap(e.to_string()))?
        .is_none()
    {
        return Err(fail(
            registry,
            BootstrapState::Timeout,
            OpError::Bootstrap("helper never acknowledged the payload".into()),
        ));
    }
    registry
        .bootstrap_state(boot_uid, BootstrapState::Acked)
        .map_err(|e| OpError::Bootstrap(e.message))?;
    registry
        .bootstrap_state(boot_uid, BootstrapState::Completed)
        .map_err(|e| OpError::Bootstrap(e.message))?;
    bootstrap::cleanup(paths);
    Ok((group_ref, split_ref))
}

/// `dmux group new` (plan §7.2): spawn one new Group whose first pane runs
/// the journaled bootstrap helper; backend is inherited, never chosen.
pub fn group_new(
    env: &OperationEnv,
    provider: &dyn Provider,
    scope: &InventoryScope,
    req: &GroupNewRequest,
) -> Result<CreatedChild, OpError> {
    let mut registry =
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).map_err(reg_err)?;
    let identity = registry.identity().map_err(reg_err)?;
    let digest = sha256_hex(
        format!(
            "group_new\x1f{}\x1f{:?}\x1f{:?}",
            req.space_uid.0, req.cwd, req.program
        )
        .as_bytes(),
    );
    if let Some(replayed) =
        child_replay::<CreatedChild>(&mut registry, req.request_uid, "group_new", &digest, |c| {
            c.replayed = true
        })?
    {
        return Ok(replayed);
    }

    let (row, binding) = load_bound_space(&mut registry, req.space_uid)?;
    let instance = row.backend_instance;
    let backend = scope.backend;
    require_child_mutable(&row)?;

    let mut locks = OrderedLocks::new(&env.lock_dir);
    ChildLocks::acquire(&mut locks, &registry, instance, req.space_uid)?;

    // Check-first (ADR 012 §3.4, WS-A.11). Everything the post-mutation
    // `split_list` below can refuse — an unpinned scope and an epoch the
    // live server does not serve — is decided here, before the bootstrap
    // row is journaled and before the native window exists. In the old
    // order that refusal landed after `provider.group_new`, so every
    // `dmux group new` under an unpinned scope created a window, aborted,
    // and left an orphan window plus a live `pane-bootstrap`.
    let (epoch, native_row) =
        fenced_space_scan(&registry, provider, scope, instance, &binding.native_token)?;
    // The scan above proved the binding's token live under the pin; the
    // registry's recorded binding epoch decides whether that token is this
    // Space at all (WS-A.8). Refused before the journal row below exists.
    let binding_epoch =
        match binding_epoch_for_adapter(&mut registry, scope, &binding, |_| Ok(true))? {
            BindingVerdict::Pinned(epoch) => epoch,
            BindingVerdict::AbsentUnderPin => {
                return Err(OpError::Indeterminate(format!(
                    "{} was listed by the scan but is not live under the pinned epoch",
                    binding.native_token
                )));
            }
        };
    let pre_groups: std::collections::HashSet<String> = native_row
        .groups
        .iter()
        .map(|g| g.handle.to_string())
        .collect();

    let boot_uid = Uuid::new_v4();
    registry
        .bootstrap_issue(&IssuedRequest {
            request_uid: boot_uid,
            operation_uid: None,
            space_uid: Some(req.space_uid),
            backend_instance: instance,
            server_epoch: epoch,
            intended_parent: Some(binding.native_token.clone()),
            recovery_generation: None,
            manifest_node_path: None,
        })
        .map_err(|e| OpError::Bootstrap(e.message))?;
    let paths = bootstrap::prepare(&env.lock_dir, boot_uid)
        .map_err(|e| OpError::Bootstrap(e.to_string()))?;
    let abort = |registry: &mut Registry, state: BootstrapState, err: OpError| {
        let _ = registry.bootstrap_state(boot_uid, state);
        bootstrap::cleanup(&paths);
        err
    };

    let program = if req.program.is_empty() {
        vec!["/bin/sh".to_string(), "-l".to_string()]
    } else {
        req.program.clone()
    };
    // §11.3: the CLI passes the invoking Split's cwd explicitly when run
    // inside the Space; here explicit wins, then owner home — never the
    // dmux process cwd (tmux would inherit it silently otherwise).
    let (cwd, cwd_source) = match &req.cwd {
        Some(explicit) => (Some(explicit.clone()), CwdSource::Explicit),
        None => home_fallback(),
    };
    let spec = CreateSpec {
        native_token: binding.native_token.clone(),
        cwd,
        bootstrap_argv: bootstrap::helper_argv(&req.helper_bin, boot_uid, &program),
    };
    let native_binding = crate::backend::NativeBinding {
        native_token: binding.native_token.clone(),
        server_epoch: binding_epoch,
        root_group: native_row
            .groups
            .first()
            .map(|g| g.handle.clone())
            .ok_or_else(|| {
                OpError::Indeterminate(format!(
                    "{} has no groups in the scan",
                    binding.native_token
                ))
            })?,
        root_split: native_row
            .groups
            .first()
            .and_then(|g| g.splits.first())
            .map(|s| s.handle.clone())
            .ok_or_else(|| {
                OpError::Indeterminate(format!(
                    "{} has no splits in the scan",
                    binding.native_token
                ))
            })?,
    };
    let group = match provider.group_new(scope, &native_binding, &spec) {
        Ok(handle) => handle,
        Err(e) => {
            return Err(abort(
                &mut registry,
                BootstrapState::Aborted,
                OpError::Provider(format!("{e:?}")),
            ));
        }
    };
    if pre_groups.contains(&group.to_string()) {
        return Err(abort(
            &mut registry,
            BootstrapState::Conflict,
            OpError::Provider(format!("group_new returned pre-existing handle {group}")),
        ));
    }
    registry
        .bootstrap_spawned(
            boot_uid,
            &serde_json::json!({ "group": group.to_string() }).to_string(),
        )
        .map_err(|e| OpError::Bootstrap(e.message))?;

    // The fresh Group must hold exactly one Split: that is the new pane.
    let split = match provider.split_list(scope, &group) {
        Ok(rows) if rows.len() == 1 => rows[0].handle.clone(),
        Ok(rows) => {
            return Err(abort(
                &mut registry,
                BootstrapState::Conflict,
                OpError::Provider(format!(
                    "fresh group {group} lists {} splits, wanted exactly 1",
                    rows.len()
                )),
            ));
        }
        Err(e) => {
            return Err(abort(
                &mut registry,
                BootstrapState::Conflict,
                OpError::Provider(format!("{e:?}")),
            ));
        }
    };

    let (group_ref, split_ref) = finish_child_bootstrap(
        &mut registry,
        &paths,
        boot_uid,
        &identity,
        backend,
        epoch,
        req.space_uid,
        row.space_no,
        &group,
        &split,
    )?;
    let created = CreatedChild {
        space_uid: req.space_uid,
        kind: ChildKind::Group,
        group_ref,
        split_ref,
        cwd_source,
        replayed: false,
    };
    registry
        .finish_rpc_request(
            req.request_uid,
            &serde_json::to_value(&created).map_err(|e| OpError::Registry(e.to_string()))?,
            None,
        )
        .map_err(reg_err)?;
    Ok(created)
}

/// `dmux split new` (plan §7.2): split within the target Group. tmux is
/// targeted at the Group's first Split (the adapter splits panes); wez at
/// the Group tab itself (the adapter anchors its first pane).
pub fn split_new(
    env: &OperationEnv,
    provider: &dyn Provider,
    scope: &InventoryScope,
    req: &SplitNewRequest,
) -> Result<CreatedChild, OpError> {
    if req.group.kind != ChildKind::Group {
        return Err(OpError::NotFound(format!(
            "split new targets a Group ref, got a {:?} ref",
            req.group.kind
        )));
    }
    let mut registry =
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).map_err(reg_err)?;
    let identity = registry.identity().map_err(reg_err)?;
    let digest = sha256_hex(
        format!(
            "split_new\x1f{}\x1f{}\x1f{:?}\x1f{:?}\x1f{:?}\x1f{:?}",
            req.space_uid.0, req.group.handle, req.direction, req.percent, req.cwd, req.program
        )
        .as_bytes(),
    );
    if let Some(replayed) =
        child_replay::<CreatedChild>(&mut registry, req.request_uid, "split_new", &digest, |c| {
            c.replayed = true
        })?
    {
        return Ok(replayed);
    }

    let (row, binding) = load_bound_space(&mut registry, req.space_uid)?;
    let instance = row.backend_instance;
    let backend = scope.backend;
    require_child_mutable(&row)?;

    let mut locks = OrderedLocks::new(&env.lock_dir);
    ChildLocks::acquire(&mut locks, &registry, instance, req.space_uid)?;

    let (epoch, native_row) = fenced_space_scan(
        &registry,
        provider,
        scope,
        row.backend_instance,
        &binding.native_token,
    )?;
    require_live_epoch(&req.group, epoch)?;
    let parent = native_row
        .groups
        .iter()
        .find(|g| g.handle == req.group.handle)
        .ok_or_else(|| {
            OpError::NotFound(format!(
                "group {} is not part of {}",
                req.group.handle, binding.native_token
            ))
        })?;
    let pre_splits: std::collections::HashSet<String> =
        parent.splits.iter().map(|s| s.handle.to_string()).collect();

    // cwd inheritance (plan §11.3): explicit wins; otherwise the target
    // Split's cwd; otherwise owner home, visibly — never the dmux process
    // cwd (the tmux native default, pinned in the provider suite).
    let (cwd, cwd_source) = match (&req.cwd, parent.splits.first().and_then(|s| s.cwd.clone())) {
        (Some(explicit), _) => (Some(explicit.clone()), CwdSource::Explicit),
        (None, Some(inherited)) => (Some(inherited), CwdSource::TargetSplit),
        (None, None) => home_fallback(),
    };

    let boot_uid = Uuid::new_v4();
    registry
        .bootstrap_issue(&IssuedRequest {
            request_uid: boot_uid,
            operation_uid: None,
            space_uid: Some(req.space_uid),
            backend_instance: instance,
            server_epoch: epoch,
            intended_parent: Some(req.group.handle.to_string()),
            recovery_generation: None,
            manifest_node_path: None,
        })
        .map_err(|e| OpError::Bootstrap(e.message))?;
    let paths = bootstrap::prepare(&env.lock_dir, boot_uid)
        .map_err(|e| OpError::Bootstrap(e.to_string()))?;
    let abort = |registry: &mut Registry, state: BootstrapState, err: OpError| {
        let _ = registry.bootstrap_state(boot_uid, state);
        bootstrap::cleanup(&paths);
        err
    };

    let program = if req.program.is_empty() {
        vec!["/bin/sh".to_string(), "-l".to_string()]
    } else {
        req.program.clone()
    };
    let split_spec = SplitSpec {
        spec: CreateSpec {
            native_token: binding.native_token.clone(),
            cwd,
            bootstrap_argv: bootstrap::helper_argv(&req.helper_bin, boot_uid, &program),
        },
        direction: req.direction,
        percent: req.percent,
    };
    // Adapter target semantics differ (pinned in the provider suites): the
    // tmux adapter splits an exact pane; the wez adapter anchors the tab's
    // first pane itself.
    let target = match backend {
        Backend::Tmux => parent
            .splits
            .first()
            .map(|s| s.handle.clone())
            .ok_or_else(|| {
                OpError::Indeterminate(format!("group {} lists no splits", req.group.handle))
            })?,
        Backend::Wez => req.group.handle.clone(),
    };
    let split = match provider.split_new(scope, &target, &split_spec) {
        Ok(handle) => handle,
        Err(e) => {
            return Err(abort(
                &mut registry,
                BootstrapState::Aborted,
                OpError::Provider(format!("{e:?}")),
            ));
        }
    };
    if pre_splits.contains(&split.to_string()) {
        return Err(abort(
            &mut registry,
            BootstrapState::Conflict,
            OpError::Provider(format!("split_new returned pre-existing handle {split}")),
        ));
    }
    registry
        .bootstrap_spawned(
            boot_uid,
            &serde_json::json!({ "split": split.to_string() }).to_string(),
        )
        .map_err(|e| OpError::Bootstrap(e.message))?;

    let (group_ref, split_ref) = finish_child_bootstrap(
        &mut registry,
        &paths,
        boot_uid,
        &identity,
        backend,
        epoch,
        req.space_uid,
        row.space_no,
        &req.group.handle,
        &split,
    )?;
    let created = CreatedChild {
        space_uid: req.space_uid,
        kind: ChildKind::Split,
        group_ref,
        split_ref,
        cwd_source,
        replayed: false,
    };
    registry
        .finish_rpc_request(
            req.request_uid,
            &serde_json::to_value(&created).map_err(|e| OpError::Registry(e.to_string()))?,
            None,
        )
        .map_err(reg_err)?;
    Ok(created)
}

/// `dmux group rename` (plan §7.2): a Group title is presentation, not an
/// authority name — no decision lock, no registry row.
pub fn group_rename(
    env: &OperationEnv,
    provider: &dyn Provider,
    scope: &InventoryScope,
    space_uid: SpaceUid,
    group: &ChildRefShape,
    title: &str,
    request_uid: Uuid,
) -> Result<(), OpError> {
    let mut registry =
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).map_err(reg_err)?;
    let digest = sha256_hex(
        format!(
            "group_rename\x1f{}\x1f{}\x1f{title}",
            space_uid.0, group.handle
        )
        .as_bytes(),
    );
    if child_replay::<serde_json::Value>(
        &mut registry,
        request_uid,
        "group_rename",
        &digest,
        |_| {},
    )?
    .is_some()
    {
        return Ok(());
    }
    let (row, binding) = load_bound_space(&mut registry, space_uid)?;
    require_child_mutable(&row)?;
    let mut locks = OrderedLocks::new(&env.lock_dir);
    ChildLocks::acquire(&mut locks, &registry, row.backend_instance, space_uid)?;
    let (epoch, native_row) = fenced_space_scan(
        &registry,
        provider,
        scope,
        row.backend_instance,
        &binding.native_token,
    )?;
    require_live_epoch(group, epoch)?;
    if !native_row.groups.iter().any(|g| g.handle == group.handle) {
        return Err(OpError::NotFound(format!(
            "group {} is not part of {}",
            group.handle, binding.native_token
        )));
    }
    provider
        .group_rename(scope, &group.handle, title)
        .map_err(|e| OpError::Provider(format!("{e:?}")))?;
    registry
        .finish_rpc_request(request_uid, &serde_json::json!({ "renamed": true }), None)
        .map_err(reg_err)?;
    Ok(())
}

/// `dmux group rm` (plan §7.2). Removing the last Group would remove the
/// Space; that hidden cascade is refused toward `dmux rm`.
pub fn group_remove(
    env: &OperationEnv,
    provider: &dyn Provider,
    scope: &InventoryScope,
    space_uid: SpaceUid,
    group: &ChildRefShape,
    request_uid: Uuid,
) -> Result<RemovedChild, OpError> {
    let mut registry =
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).map_err(reg_err)?;
    let digest = sha256_hex(format!("group_rm\x1f{}\x1f{}", space_uid.0, group.handle).as_bytes());
    if let Some(replayed) =
        child_replay::<RemovedChild>(&mut registry, request_uid, "group_rm", &digest, |c| {
            c.replayed = true
        })?
    {
        return Ok(replayed);
    }
    let (row, binding) = load_bound_space(&mut registry, space_uid)?;
    require_child_mutable(&row)?;
    let mut locks = OrderedLocks::new(&env.lock_dir);
    ChildLocks::acquire(&mut locks, &registry, row.backend_instance, space_uid)?;
    let (epoch, native_row) = fenced_space_scan(
        &registry,
        provider,
        scope,
        row.backend_instance,
        &binding.native_token,
    )?;
    require_live_epoch(group, epoch)?;
    if !native_row.groups.iter().any(|g| g.handle == group.handle) {
        return Err(OpError::NotFound(format!(
            "group {} is not part of {}",
            group.handle, binding.native_token
        )));
    }
    if native_row.groups.len() == 1 {
        return Err(OpError::Refused(format!(
            "group {} is the last Group of {:?}; removing it would remove the \
             Space — use `dmux rm {:?}` instead",
            group.handle, row.logical_name, row.logical_name
        )));
    }
    provider
        .group_remove(scope, &group.handle)
        .map_err(|e| OpError::Provider(format!("{e:?}")))?;
    let removed = RemovedChild {
        space_uid,
        kind: ChildKind::Group,
        handle: group.handle.to_string(),
        replayed: false,
    };
    registry
        .finish_rpc_request(
            request_uid,
            &serde_json::to_value(&removed).map_err(|e| OpError::Registry(e.to_string()))?,
            None,
        )
        .map_err(reg_err)?;
    Ok(removed)
}

/// `dmux split rm` (plan §7.2). Removing the last Split of a Group would
/// remove the Group; refused toward `dmux group rm`.
pub fn split_remove(
    env: &OperationEnv,
    provider: &dyn Provider,
    scope: &InventoryScope,
    space_uid: SpaceUid,
    split: &ChildRefShape,
    request_uid: Uuid,
) -> Result<RemovedChild, OpError> {
    let mut registry =
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).map_err(reg_err)?;
    let digest = sha256_hex(format!("split_rm\x1f{}\x1f{}", space_uid.0, split.handle).as_bytes());
    if let Some(replayed) =
        child_replay::<RemovedChild>(&mut registry, request_uid, "split_rm", &digest, |c| {
            c.replayed = true
        })?
    {
        return Ok(replayed);
    }
    let (row, binding) = load_bound_space(&mut registry, space_uid)?;
    require_child_mutable(&row)?;
    let mut locks = OrderedLocks::new(&env.lock_dir);
    ChildLocks::acquire(&mut locks, &registry, row.backend_instance, space_uid)?;
    let (epoch, native_row) = fenced_space_scan(
        &registry,
        provider,
        scope,
        row.backend_instance,
        &binding.native_token,
    )?;
    require_live_epoch(split, epoch)?;
    let parent = native_row
        .groups
        .iter()
        .find(|g| g.splits.iter().any(|s| s.handle == split.handle))
        .ok_or_else(|| {
            OpError::NotFound(format!(
                "split {} is not part of {}",
                split.handle, binding.native_token
            ))
        })?;
    if parent.splits.len() == 1 {
        return Err(OpError::Refused(format!(
            "split {} is the last Split of group {}; removing it would remove \
             the Group — use `dmux group rm` instead",
            split.handle, parent.handle
        )));
    }
    provider
        .split_remove(scope, &split.handle)
        .map_err(|e| OpError::Provider(format!("{e:?}")))?;
    let removed = RemovedChild {
        space_uid,
        kind: ChildKind::Split,
        handle: split.handle.to_string(),
        replayed: false,
    };
    registry
        .finish_rpc_request(
            request_uid,
            &serde_json::to_value(&removed).map_err(|e| OpError::Registry(e.to_string()))?,
            None,
        )
        .map_err(reg_err)?;
    Ok(removed)
}

// ---------------------------------------------------------------------------
// P9 exact logical child control. These are owner mutations/presentation
// state changes, so provider methods are never called naked: every path uses
// gate shared -> backend-instance exclusive -> Space exclusive, refuses a
// durable unfinished recovery, validates the epoch-qualified child against
// one complete Space tree, and journals same-request replay.

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ActivatedGroup {
    pub space_uid: SpaceUid,
    pub server_epoch: ServerEpoch,
    pub group_ref: String,
    #[serde(default)]
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SelectedSplit {
    pub space_uid: SpaceUid,
    pub server_epoch: ServerEpoch,
    pub group_ref: String,
    /// `None` is the native edge no-op; an ordinal fallback is forbidden.
    pub split_ref: Option<String>,
    #[serde(default)]
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ResizedSplit {
    pub space_uid: SpaceUid,
    pub server_epoch: ServerEpoch,
    pub split_ref: String,
    pub changed: bool,
    #[serde(default)]
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ZoomedSplit {
    pub space_uid: SpaceUid,
    pub server_epoch: ServerEpoch,
    pub split_ref: String,
    pub zoomed: bool,
    #[serde(default)]
    pub replayed: bool,
}

/// The one epoch a mutation may act under, or a registry row be minted
/// from: the epoch the scope was pinned to, confirmed by a complete scan
/// that answered under exactly that epoch. An unpinned scope is never
/// mutated — nothing in the registry vouches for what answered on it — and
/// a scan that answered under another epoch is a replaced server. Both
/// adapters refuse the same two things inside their own verbs (tmux
/// `required_epoch`, wez `required_action_epoch`); this is the operations
/// layer deciding them *first*, before any journal row or native command,
/// so a refusal can never land after a mutation (ADR 012 §3.4, WS-A.10/11).
fn require_pinned_epoch(
    scope: &InventoryScope,
    live_epoch: ServerEpoch,
) -> Result<ServerEpoch, OpError> {
    match scope.expected_epoch() {
        Some(expected) if expected == live_epoch => Ok(expected),
        Some(expected) => Err(OpError::StaleRef(format!(
            "scope is pinned to server epoch {} but the live server answered under {}",
            expected.0, live_epoch.0
        ))),
        None => Err(OpError::Indeterminate(
            "managed mutations require a scope pinned to the registry-published server epoch; \
             an unmanaged endpoint is listable but never mutated"
                .into(),
        )),
    }
}

fn split_parent<'a>(
    native: &'a crate::backend::NativeSpaceRow,
    split: &ProviderHandle,
) -> Option<&'a crate::backend::NativeGroupRow> {
    native
        .groups
        .iter()
        .find(|group| group.splits.iter().any(|row| &row.handle == split))
}

pub fn group_activate_exact(
    env: &OperationEnv,
    provider: &dyn Provider,
    scope: &InventoryScope,
    space_uid: SpaceUid,
    group: &ChildRefShape,
    request_uid: Uuid,
) -> Result<ActivatedGroup, OpError> {
    if group.kind != ChildKind::Group {
        return Err(OpError::Refused(
            "group activation requires a Group ref".into(),
        ));
    }
    let mut registry =
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).map_err(reg_err)?;
    let digest =
        sha256_hex(format!("group_activate\x1f{}\x1f{}", space_uid.0, group.handle).as_bytes());
    if let Some(replayed) = child_replay::<ActivatedGroup>(
        &mut registry,
        request_uid,
        "group_activate",
        &digest,
        |value| value.replayed = true,
    )? {
        return Ok(replayed);
    }
    let (row, binding) = load_bound_space(&mut registry, space_uid)?;
    require_child_mutable(&row)?;
    let mut locks = OrderedLocks::new(&env.lock_dir);
    ChildLocks::acquire(&mut locks, &registry, row.backend_instance, space_uid)?;
    let (epoch, native) = fenced_space_scan(
        &registry,
        provider,
        scope,
        row.backend_instance,
        &binding.native_token,
    )?;
    require_live_epoch(group, epoch)?;
    if !native.groups.iter().any(|row| row.handle == group.handle) {
        return Err(OpError::NotFound(format!(
            "group {} is not part of {}",
            group.handle, binding.native_token
        )));
    }
    let witness = provider
        .activate_group_exact(scope, &group.handle)
        .map_err(|error| OpError::Provider(format!("{error:?}")))?;
    if witness.server_epoch != epoch || witness.target != group.handle {
        return Err(OpError::Provider(
            "exact group activation returned a mismatched witness".into(),
        ));
    }
    let result = ActivatedGroup {
        space_uid,
        server_epoch: epoch,
        group_ref: make_ref(ChildKind::Group, epoch, &group.handle),
        replayed: false,
    };
    registry
        .finish_rpc_request(
            request_uid,
            &serde_json::to_value(&result).map_err(|error| OpError::Registry(error.to_string()))?,
            None,
        )
        .map_err(reg_err)?;
    Ok(result)
}

pub fn split_direction(
    env: &OperationEnv,
    provider: &dyn Provider,
    scope: &InventoryScope,
    space_uid: SpaceUid,
    origin: &ChildRefShape,
    direction: crate::backend::SplitDirection,
    request_uid: Uuid,
) -> Result<SelectedSplit, OpError> {
    if origin.kind != ChildKind::Split {
        return Err(OpError::Refused(
            "directional selection requires a Split ref".into(),
        ));
    }
    let mut registry =
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).map_err(reg_err)?;
    let digest = sha256_hex(
        format!(
            "split_direction\x1f{}\x1f{}\x1f{direction:?}",
            space_uid.0, origin.handle
        )
        .as_bytes(),
    );
    if let Some(replayed) = child_replay::<SelectedSplit>(
        &mut registry,
        request_uid,
        "split_direction",
        &digest,
        |value| value.replayed = true,
    )? {
        return Ok(replayed);
    }
    let (row, binding) = load_bound_space(&mut registry, space_uid)?;
    require_child_mutable(&row)?;
    let mut locks = OrderedLocks::new(&env.lock_dir);
    ChildLocks::acquire(&mut locks, &registry, row.backend_instance, space_uid)?;
    let (epoch, native) = fenced_space_scan(
        &registry,
        provider,
        scope,
        row.backend_instance,
        &binding.native_token,
    )?;
    require_live_epoch(origin, epoch)?;
    let parent = split_parent(&native, &origin.handle).ok_or_else(|| {
        OpError::NotFound(format!(
            "split {} is not part of {}",
            origin.handle, binding.native_token
        ))
    })?;
    let witness = provider
        .select_split_direction(scope, &origin.handle, direction)
        .map_err(|error| OpError::Provider(format!("{error:?}")))?;
    if witness.server_epoch != epoch || witness.origin != origin.handle {
        return Err(OpError::Provider(
            "directional selection returned a mismatched origin witness".into(),
        ));
    }
    if let Some(target) = &witness.target
        && !parent.splits.iter().any(|split| &split.handle == target)
    {
        return Err(OpError::Provider(format!(
            "directional target {target} is outside the origin Group"
        )));
    }
    let result = SelectedSplit {
        space_uid,
        server_epoch: epoch,
        group_ref: make_ref(ChildKind::Group, epoch, &parent.handle),
        split_ref: witness
            .target
            .as_ref()
            .map(|target| make_ref(ChildKind::Split, epoch, target)),
        replayed: false,
    };
    registry
        .finish_rpc_request(
            request_uid,
            &serde_json::to_value(&result).map_err(|error| OpError::Registry(error.to_string()))?,
            None,
        )
        .map_err(reg_err)?;
    Ok(result)
}

pub fn split_resize(
    env: &OperationEnv,
    provider: &dyn Provider,
    scope: &InventoryScope,
    space_uid: SpaceUid,
    split: &ChildRefShape,
    direction: crate::backend::SplitDirection,
    amount: u16,
    request_uid: Uuid,
) -> Result<ResizedSplit, OpError> {
    if split.kind != ChildKind::Split || amount == 0 {
        return Err(OpError::Refused(
            "split resize requires a Split ref and positive amount".into(),
        ));
    }
    let mut registry =
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).map_err(reg_err)?;
    let digest = sha256_hex(
        format!(
            "split_resize\x1f{}\x1f{}\x1f{direction:?}\x1f{amount}",
            space_uid.0, split.handle
        )
        .as_bytes(),
    );
    if let Some(replayed) = child_replay::<ResizedSplit>(
        &mut registry,
        request_uid,
        "split_resize",
        &digest,
        |value| value.replayed = true,
    )? {
        return Ok(replayed);
    }
    let (row, binding) = load_bound_space(&mut registry, space_uid)?;
    require_child_mutable(&row)?;
    let mut locks = OrderedLocks::new(&env.lock_dir);
    ChildLocks::acquire(&mut locks, &registry, row.backend_instance, space_uid)?;
    let (epoch, native) = fenced_space_scan(
        &registry,
        provider,
        scope,
        row.backend_instance,
        &binding.native_token,
    )?;
    require_live_epoch(split, epoch)?;
    split_parent(&native, &split.handle).ok_or_else(|| {
        OpError::NotFound(format!(
            "split {} is not part of {}",
            split.handle, binding.native_token
        ))
    })?;
    let witness = provider
        .resize_split_exact(scope, &split.handle, direction, amount)
        .map_err(|error| OpError::Provider(format!("{error:?}")))?;
    if witness.server_epoch != epoch || witness.target != split.handle {
        return Err(OpError::Provider(
            "split resize returned a mismatched witness".into(),
        ));
    }
    let result = ResizedSplit {
        space_uid,
        server_epoch: epoch,
        split_ref: make_ref(ChildKind::Split, epoch, &split.handle),
        changed: witness.changed,
        replayed: false,
    };
    registry
        .finish_rpc_request(
            request_uid,
            &serde_json::to_value(&result).map_err(|error| OpError::Registry(error.to_string()))?,
            None,
        )
        .map_err(reg_err)?;
    Ok(result)
}

pub fn split_zoom(
    env: &OperationEnv,
    provider: &dyn Provider,
    scope: &InventoryScope,
    space_uid: SpaceUid,
    split: &ChildRefShape,
    request_uid: Uuid,
) -> Result<ZoomedSplit, OpError> {
    if split.kind != ChildKind::Split {
        return Err(OpError::Refused("split zoom requires a Split ref".into()));
    }
    let mut registry =
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).map_err(reg_err)?;
    let digest =
        sha256_hex(format!("split_zoom\x1f{}\x1f{}", space_uid.0, split.handle).as_bytes());
    if let Some(replayed) =
        child_replay::<ZoomedSplit>(&mut registry, request_uid, "split_zoom", &digest, |value| {
            value.replayed = true
        })?
    {
        return Ok(replayed);
    }
    let (row, binding) = load_bound_space(&mut registry, space_uid)?;
    require_child_mutable(&row)?;
    let mut locks = OrderedLocks::new(&env.lock_dir);
    ChildLocks::acquire(&mut locks, &registry, row.backend_instance, space_uid)?;
    let (epoch, native) = fenced_space_scan(
        &registry,
        provider,
        scope,
        row.backend_instance,
        &binding.native_token,
    )?;
    require_live_epoch(split, epoch)?;
    split_parent(&native, &split.handle).ok_or_else(|| {
        OpError::NotFound(format!(
            "split {} is not part of {}",
            split.handle, binding.native_token
        ))
    })?;
    let witness = provider
        .toggle_split_zoom_exact(scope, &split.handle)
        .map_err(|error| OpError::Provider(format!("{error:?}")))?;
    if witness.server_epoch != epoch || witness.target != split.handle {
        return Err(OpError::Provider(
            "split zoom returned a mismatched witness".into(),
        ));
    }
    let result = ZoomedSplit {
        space_uid,
        server_epoch: epoch,
        split_ref: make_ref(ChildKind::Split, epoch, &split.handle),
        zoomed: witness.zoomed,
        replayed: false,
    };
    registry
        .finish_rpc_request(
            request_uid,
            &serde_json::to_value(&result).map_err(|error| OpError::Registry(error.to_string()))?,
            None,
        )
        .map_err(reg_err)?;
    Ok(result)
}

/// Read-only hierarchy of one Space (plan §7.2 listings): registry row plus
/// complete same-epoch scan under the common backend read fence. Recovery
/// is surfaced immediately rather than observing or waiting on a partial
/// tree.
pub fn hierarchy(
    env: &OperationEnv,
    provider: &dyn Provider,
    scope: &InventoryScope,
    space_uid: SpaceUid,
) -> Result<SpaceHierarchy, OpError> {
    let mut registry =
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).map_err(reg_err)?;
    let instance = registry.space(space_uid).map_err(reg_err)?.backend_instance;
    let mut locks = OrderedLocks::new(&env.lock_dir);
    locks
        .acquire(LockScope::AuthorityGate, LockMode::Shared)
        .map_err(|e| OpError::Lock(e.to_string()))?;
    if !locks
        .try_acquire(LockScope::BackendInstance(instance), LockMode::Shared)
        .map_err(|e| OpError::Lock(e.to_string()))?
    {
        return Err(OpError::Indeterminate(format!(
            "backend instance {} is recovering or mutating",
            instance.0
        )));
    }
    let (_row, binding) = load_bound_space(&mut registry, space_uid)?;
    let published = verify_published_incarnation(&registry, instance, scope)?;
    let (epoch, native_row) = scan_space_row(provider, scope, &binding.native_token)?;
    require_published_epoch(&published, epoch)?;
    Ok(SpaceHierarchy {
        space_uid,
        server_epoch: epoch,
        groups: native_row
            .groups
            .iter()
            .map(|g| HierarchyGroup {
                group_ref: make_ref(ChildKind::Group, epoch, &g.handle),
                title: g.title.clone(),
                splits: g
                    .splits
                    .iter()
                    .map(|s| HierarchySplit {
                        split_ref: make_ref(ChildKind::Split, epoch, &s.handle),
                        title: s.title.clone(),
                        cwd: s.cwd.clone(),
                    })
                    .collect(),
            })
            .collect(),
    })
}

/// Revalidate an untrusted v1 GUI marker against the local owner authority
/// and a complete live scan.  A recovery holder makes the non-blocking
/// backend read fence unavailable, which is reported as `recovering`
/// instead of waiting on or observing a half-restored tree.
pub fn validate_marker_context(
    env: &OperationEnv,
    provider: &dyn Provider,
    scope: &InventoryScope,
    marker: &crate::bootstrap::MarkerContext,
) -> Result<ValidatedMarker, OpError> {
    let mut registry =
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).map_err(reg_err)?;
    let identity = registry.identity().map_err(reg_err)?;
    if marker.host_uid != identity.host_uid {
        return Err(OpError::Refused(format!(
            "marker HostUid {} is not this authority {}",
            marker.host_uid.0, identity.host_uid.0
        )));
    }

    // Resolve the backend-instance scope once, then re-read all mutable rows
    // after the fence is held.  An authority mutation of this same instance
    // necessarily takes the conflicting exclusive backend lock.
    let initial = registry.space(marker.space_uid).map_err(reg_err)?;
    let instance = initial.backend_instance;
    let mut locks = OrderedLocks::new(&env.lock_dir);
    locks
        .acquire(LockScope::AuthorityGate, LockMode::Shared)
        .map_err(|e| OpError::Lock(e.to_string()))?;
    if !locks
        .try_acquire(LockScope::BackendInstance(instance), LockMode::Shared)
        .map_err(|e| OpError::Lock(e.to_string()))?
    {
        return Err(OpError::Indeterminate(format!(
            "backend instance {} is recovering or mutating",
            instance.0
        )));
    }

    let (row, binding) = load_bound_space(&mut registry, marker.space_uid)?;
    if row.backend_instance != instance
        || row.space_no != marker.space_no
        || scope.backend != marker.backend
    {
        return Err(OpError::StaleRef(
            "marker Space number/backend no longer matches its authority row".into(),
        ));
    }
    let info = registry.backend_instance_info(instance).map_err(reg_err)?;
    if info.owner != marker.host_uid || info.backend != marker.backend {
        return Err(OpError::StaleRef(
            "marker owner/backend does not match its backend instance".into(),
        ));
    }

    let published = verify_published_incarnation(&registry, instance, scope)?;
    let (live_epoch, native) = scan_space_row(provider, scope, &binding.native_token)?;
    if live_epoch != marker.server_epoch {
        return Err(OpError::StaleRef(format!(
            "marker epoch {} does not match live epoch {}",
            marker.server_epoch.0, live_epoch.0
        )));
    }
    require_published_epoch(&published, live_epoch)?;

    let group_count = native.groups.len();
    let split_count = native.groups.iter().map(|group| group.splits.len()).sum();
    let mut group_name = None;
    let mut split_parents = Vec::new();
    let mut matching_groups = 0usize;
    for group in &native.groups {
        let group_ref = make_ref(ChildKind::Group, live_epoch, &group.handle);
        if group_ref == marker.group_ref {
            matching_groups += 1;
            group_name = group.title.clone();
        }
        for split in &group.splits {
            if make_ref(ChildKind::Split, live_epoch, &split.handle) == marker.split_ref {
                split_parents.push(group_ref.clone());
            }
        }
    }
    if matching_groups != 1
        || split_parents.len() != 1
        || split_parents.first() != Some(&marker.group_ref)
    {
        return Err(OpError::StaleRef(format!(
            "marker child correlation is not unique (group matches {matching_groups}, split parents {:?})",
            split_parents
        )));
    }

    Ok(ValidatedMarker {
        context: marker.clone(),
        logical_name: row.logical_name,
        backend_instance: instance,
        health: row.health,
        group_count,
        split_count,
        group_name,
    })
}

/// `dmux _context` (plan §13.1): revalidate the invoking pane against the
/// authoritative registry and a complete same-epoch scan, then return the
/// current marker set. Works on unstamped (adopted) Spaces — that is how
/// they heal. A missing pane or a dead Space is a typed error, never a
/// guessed marker.
pub fn context_read(
    env: &OperationEnv,
    provider: &dyn Provider,
    scope: &InventoryScope,
    space_uid: SpaceUid,
    pane_env_id: &str,
) -> Result<MarkerContext, OpError> {
    let mut registry =
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).map_err(reg_err)?;
    let identity = registry.identity().map_err(reg_err)?;
    let instance = registry.space(space_uid).map_err(reg_err)?.backend_instance;
    let mut locks = OrderedLocks::new(&env.lock_dir);
    // `_context` runs synchronously from the interactive prompt.  It must
    // never wait behind authority maintenance: the marker is only a locator
    // hint, so a busy gate is an indeterminate read and the prompt can retry
    // on its next bounded refresh instead of hanging the shell.
    if !locks
        .try_acquire(LockScope::AuthorityGate, LockMode::Shared)
        .map_err(|e| OpError::Lock(e.to_string()))?
    {
        return Err(OpError::Indeterminate(
            "authority maintenance is in progress".into(),
        ));
    }
    if !locks
        .try_acquire(LockScope::BackendInstance(instance), LockMode::Shared)
        .map_err(|e| OpError::Lock(e.to_string()))?
    {
        return Err(OpError::Indeterminate(format!(
            "backend instance {} is recovering or mutating",
            instance.0
        )));
    }
    let (row, binding) = load_bound_space(&mut registry, space_uid)?;
    let published = verify_published_incarnation(&registry, instance, scope)?;
    let (epoch, native_row) = scan_space_row(provider, scope, &binding.native_token)?;
    require_published_epoch(&published, epoch)?;
    let (group, split) = native_row
        .groups
        .iter()
        .find_map(|g| {
            g.splits
                .iter()
                .find(|s| witness_matches(&s.handle, pane_env_id))
                .map(|s| (g.handle.clone(), s.handle.clone()))
        })
        .ok_or_else(|| {
            OpError::NotFound(format!(
                "pane {pane_env_id} is not part of {}",
                binding.native_token
            ))
        })?;
    Ok(MarkerContext {
        host_uid: identity.host_uid,
        space_uid,
        space_no: row.space_no,
        backend: scope.backend,
        domain: None,
        server_epoch: epoch,
        group_ref: make_ref(ChildKind::Group, epoch, &group),
        split_ref: make_ref(ChildKind::Split, epoch, &split),
    })
}

/// `dmux context stamp` (plan §10.3): acknowledge the invoking pane's
/// marker for an adopted Space. Records the pane stamp under the current
/// epoch and promotes health to `healthy` only when a complete scan proves
/// every live pane has one current stamp.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StampOutcome {
    pub context: MarkerContext,
    pub health: crate::model::Health,
    /// Live panes still lacking a current-epoch stamp.
    pub pending_panes: usize,
}

pub fn context_stamp(
    env: &OperationEnv,
    provider: &dyn Provider,
    scope: &InventoryScope,
    space_uid: SpaceUid,
    pane_env_id: &str,
) -> Result<StampOutcome, OpError> {
    let mut registry =
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).map_err(reg_err)?;
    let identity = registry.identity().map_err(reg_err)?;
    let instance = registry.space(space_uid).map_err(reg_err)?.backend_instance;
    let mut locks = OrderedLocks::new(&env.lock_dir);
    ChildLocks::acquire(&mut locks, &registry, instance, space_uid)?;
    let (row, binding) = load_bound_space(&mut registry, space_uid)?;
    let published = verify_published_incarnation(&registry, instance, scope)?;
    let (epoch, native_row) = scan_space_row(provider, scope, &binding.native_token)?;
    require_published_epoch(&published, epoch)?;
    let (group, split) = native_row
        .groups
        .iter()
        .find_map(|g| {
            g.splits
                .iter()
                .find(|s| witness_matches(&s.handle, pane_env_id))
                .map(|s| (g.handle.clone(), s.handle.clone()))
        })
        .ok_or_else(|| {
            OpError::NotFound(format!(
                "pane {pane_env_id} is not part of {}",
                binding.native_token
            ))
        })?;

    registry
        .record_pane_stamp(space_uid, epoch, &split.to_string())
        .map_err(reg_err)?;
    let stamped: std::collections::HashSet<String> = registry
        .pane_stamps(space_uid, epoch)
        .map_err(reg_err)?
        .into_iter()
        .map(|r| r.pane_handle)
        .collect();
    let pending_panes = native_row
        .groups
        .iter()
        .flat_map(|g| g.splits.iter())
        .filter(|s| !stamped.contains(&s.handle.to_string()))
        .count();
    let mut health = row.health;
    if pending_panes == 0 && row.health == crate::model::Health::Unstamped {
        registry
            .set_space_health(space_uid, crate::model::Health::Healthy)
            .map_err(reg_err)?;
        health = crate::model::Health::Healthy;
    }
    Ok(StampOutcome {
        context: MarkerContext {
            host_uid: identity.host_uid,
            space_uid,
            space_no: row.space_no,
            backend: scope.backend,
            domain: None,
            server_epoch: epoch,
            group_ref: make_ref(ChildKind::Group, epoch, &group),
            split_ref: make_ref(ChildKind::Split, epoch, &split),
        },
        health,
        pending_panes,
    })
}

// ---------------------------------------------------------------------------
// P8a: local normalization (plan §10.3). Preview is read-only; apply runs
// under the exclusive fence, is idempotent by request UID, and never leaves
// a resource half-managed — non-convergence stays quarantined.

use crate::backend::NormalizePlan;

/// Read-only preview of the deterministic tab-to-window merge plan. An
/// empty `moves` list means the resource is already one-window.
pub fn normalize_preview(
    provider: &dyn Provider,
    scope: &InventoryScope,
    native_token: &str,
) -> Result<NormalizePlan, OpError> {
    provider
        .normalize_plan(scope, native_token)
        .map_err(|e| OpError::Provider(format!("{e:?}")))
}

/// Apply a previously shown plan (plan §10.3: plan → confirm → same
/// exclusive fence → prove exactly one window). When the resource is a
/// managed Space quarantined as `multi_window`, a proven merge restores its
/// health; an unmanaged resource simply becomes one-window (and thereby
/// adoptable). Failure changes no registry state.
pub fn normalize_apply(
    env: &OperationEnv,
    provider: &dyn Provider,
    scope: &InventoryScope,
    plan: &NormalizePlan,
    request_uid: Uuid,
) -> Result<(), OpError> {
    let mut registry =
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).map_err(reg_err)?;
    let digest = sha256_hex(
        format!(
            "normalize\x1f{}\x1f{}\x1f{}",
            plan.native_token,
            plan.server_epoch.0,
            plan.moves.len()
        )
        .as_bytes(),
    );
    if child_replay::<serde_json::Value>(&mut registry, request_uid, "normalize", &digest, |_| {})?
        .is_some()
    {
        return Ok(());
    }

    let instance = registry
        .register_backend_instance(scope.backend, Some(&scope.endpoint), None)
        .map_err(reg_err)?;
    // A managed Space quarantined on this native token heals on success.
    let mut managed: Option<crate::registry::SpaceRow> = None;
    for row in registry.spaces().map_err(reg_err)? {
        if row.lifecycle != crate::model::Lifecycle::Active {
            continue;
        }
        if let Some(binding) = registry.current_binding(row.space_uid).map_err(reg_err)?
            && binding.native_token == plan.native_token
        {
            managed = Some(row);
            break;
        }
    }

    let mut locks = OrderedLocks::new(&env.lock_dir);
    locks
        .acquire(LockScope::AuthorityGate, LockMode::Shared)
        .map_err(|e| OpError::Lock(e.to_string()))?;
    locks
        .acquire(LockScope::BackendInstance(instance), LockMode::Exclusive)
        .map_err(|e| OpError::Lock(e.to_string()))?;
    require_no_unfinished_recovery(&registry, instance)?;
    if let Some(row) = &managed {
        locks
            .acquire(LockScope::Space(row.space_uid), LockMode::Exclusive)
            .map_err(|e| OpError::Lock(e.to_string()))?;
    }

    provider
        .normalize_apply(scope, plan)
        .map_err(|e| OpError::Provider(format!("{e:?}")))?;

    if let Some(row) = &managed
        && row.health == crate::model::Health::MultiWindow
    {
        registry
            .set_space_health(row.space_uid, crate::model::Health::Healthy)
            .map_err(reg_err)?;
    }
    registry
        .finish_rpc_request(
            request_uid,
            &serde_json::json!({ "normalized": true }),
            None,
        )
        .map_err(reg_err)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// P8b: batch repair — detect every multi-window Wez resource (managed or
// unmanaged), record managed quarantine, and normalize in a previewed batch
// (plan §10.3, §17 step 9, §18 P8b). The gate is "zero unresolved managed
// multi-window Spaces".

/// One multi-window resource found by a complete owner scan.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RepairTarget {
    pub native_token: String,
    /// The bound managed Space, when one exists (its health is recorded as
    /// `multi_window` at detection time).
    pub managed: Option<SpaceUid>,
    pub plan: NormalizePlan,
}

/// Complete wez scan → every multi-window resource with its deterministic
/// merge plan. Detection RECORDS managed quarantine (`health=multi_window`)
/// so the state is visible even if the operator defers the merge.
pub fn repair_scan_wez(
    env: &OperationEnv,
    provider: &dyn Provider,
    scope: &InventoryScope,
) -> Result<Vec<RepairTarget>, OpError> {
    let mut registry =
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).map_err(reg_err)?;
    if scope.backend != Backend::Wez {
        return Err(OpError::Refused(
            "multi-window repair is defined only for Wez".into(),
        ));
    }
    // The scope's endpoint may come from the hidden `--socket` seam, so it
    // is compared against the registry before anything is fenced: the
    // owner's one Wez instance is fenced and healed only on the endpoint it
    // is recorded at — never instance A's lock over endpoint B (ADR 012
    // WS-A.12; review report 07's `register_backend_instance` residual).
    // Only a registry with no Wez instance at all registers first contact.
    let instance = match registry
        .backend_instance_for_backend(Backend::Wez)
        .map_err(reg_err)?
    {
        Some(instance) => {
            let info = registry.backend_instance_info(instance).map_err(reg_err)?;
            if info.socket_path.as_deref() != Some(scope.endpoint.as_str()) {
                return Err(OpError::Refused(format!(
                    "managed wez backend instance {} is recorded at endpoint {}, not {:?}; \
                     refusing to fence or scan another endpoint under it",
                    instance.0,
                    info.socket_path
                        .as_deref()
                        .map(|endpoint| format!("{endpoint:?}"))
                        .unwrap_or_else(|| "<none>".to_string()),
                    scope.endpoint
                )));
            }
            instance
        }
        None => registry
            .register_backend_instance(Backend::Wez, Some(&scope.endpoint), None)
            .map_err(reg_err)?,
    };
    let mut locks = OrderedLocks::new(&env.lock_dir);
    locks
        .acquire(LockScope::AuthorityGate, LockMode::Shared)
        .map_err(|e| OpError::Lock(e.to_string()))?;
    locks
        .acquire(LockScope::BackendInstance(instance), LockMode::Exclusive)
        .map_err(|e| OpError::Lock(e.to_string()))?;
    require_no_unfinished_recovery(&registry, instance)?;
    let rows = match provider.inventory(scope) {
        InventoryOutcome::Complete(inv) => inv.rows,
        other => return Err(OpError::Indeterminate(format!("wez scan: {other:?}"))),
    };
    let spaces = registry.spaces().map_err(reg_err)?;
    let mut targets = Vec::new();
    for row in rows.into_iter().filter(|r| r.multi_window) {
        let mut managed = None;
        for space in spaces
            .iter()
            .filter(|s| s.lifecycle == crate::model::Lifecycle::Active)
        {
            if let Some(binding) = registry.current_binding(space.space_uid).map_err(reg_err)?
                && binding.native_token == row.native_token
            {
                managed = Some(space.space_uid);
                if space.health != crate::model::Health::MultiWindow {
                    registry
                        .set_space_health(space.space_uid, crate::model::Health::MultiWindow)
                        .map_err(reg_err)?;
                }
                break;
            }
        }
        let plan = normalize_preview(provider, scope, &row.native_token)?;
        targets.push(RepairTarget {
            native_token: row.native_token,
            managed,
            plan,
        });
    }
    Ok(targets)
}

/// Per-target batch outcome (plan §16: per-target results, partial success
/// is visible, never silent).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RepairResult {
    pub native_token: String,
    pub managed: Option<SpaceUid>,
    pub outcome: String,
    pub ok: bool,
}

/// Apply every previewed plan; a failed target stays quarantined and never
/// stops the rest. A healed managed Space's health is restored by
/// `normalize_apply` itself.
pub fn repair_normalize_batch(
    env: &OperationEnv,
    provider: &dyn Provider,
    scope: &InventoryScope,
    targets: &[RepairTarget],
) -> Vec<RepairResult> {
    targets
        .iter()
        .map(|target| {
            let result = normalize_apply(env, provider, scope, &target.plan, Uuid::new_v4());
            RepairResult {
                native_token: target.native_token.clone(),
                managed: target.managed,
                outcome: match &result {
                    Ok(()) => "normalized".to_string(),
                    Err(e) => e.to_string(),
                },
                ok: result.is_ok(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Crash reconciliation (plan §10.2/§10.3, cases 11/13/39)
//
// A holder killed between `reserve_space_kind` and `abort_create` leaves a
// `reserved` Space beside a `prepared` journal row, and nothing reaped it:
// `rm` answered `operation_in_progress`, `rename` answered `repair_required`,
// and a replayed `adopt` answered `name_conflict` forever — the logical name
// was burned with no operator remedy. `dmux repair reconcile` is that remedy.
//
// Every decision below belongs to `registry::reconcile`, the frozen decision
// table. This module only gathers the evidence that table asks for and
// performs the registry half of its answer; it never forms a second opinion.

use crate::backend::ProviderResult;
use crate::backend::wez::CasRenameOutcome;
use crate::model::{Lifecycle, OperationKind, OperationState};
use crate::registry::reconcile::{self, CreateDecision, CreateScan, RenameDecision, ResumeDuty};

/// One unfinished journal row, the Space it strands, and the duty
/// [`reconcile::resume_duty`] assigns it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReconcileTarget {
    pub operation_uid: Uuid,
    pub request_uid: Uuid,
    pub space_uid: SpaceUid,
    pub space_no: SpaceNo,
    pub logical_name: String,
    pub backend: Backend,
    pub backend_instance: BackendInstanceUid,
    pub kind: OperationKind,
    pub state: OperationState,
    pub lifecycle: Lifecycle,
    /// [`ResumeDuty::as_str`] — the frozen table's answer, shown so an
    /// operator sees which rule is about to run.
    pub duty: &'static str,
    /// A live process still holds this Space's §10.1 decision lock: the
    /// operation is running, not crashed, and must not be touched.
    pub in_flight: bool,
    pub started_at: String,
}

/// What reconciliation did to one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconcileOutcome {
    /// The reservation was released: Space `aborted`, journal row `aborted`,
    /// logical name free again. The SpaceUid/SpaceNo stay spent (§8.2 gaps).
    ReservationReleased,
    /// The native rename had not run, so closing the row restores exactly
    /// the pre-rename state — the Space keeps the name it still has.
    RenameRolledBack,
    /// The native rename had landed; only the registry side was missing.
    RenameCommitted,
    /// The remove was resumed to a verified tombstone.
    RemoveCompleted,
    /// The row finished between the preview and the apply — a concurrent
    /// reconcile, or a second run of this one. This is what makes running
    /// twice a no-op instead of a double abort.
    AlreadyResolved,
    /// A live holder owns the §10.1 locks. Listed, never touched.
    SkippedInFlight,
    /// The evidence the table demands was unobtainable, or the table said
    /// conflict. Nothing changed (§10.2: never a fabricated success).
    FailedClosed,
}

impl ReconcileOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            ReconcileOutcome::ReservationReleased => "reservation_released",
            ReconcileOutcome::RenameRolledBack => "rename_rolled_back",
            ReconcileOutcome::RenameCommitted => "rename_committed",
            ReconcileOutcome::RemoveCompleted => "remove_completed",
            ReconcileOutcome::AlreadyResolved => "already_resolved",
            ReconcileOutcome::SkippedInFlight => "skipped_in_flight",
            ReconcileOutcome::FailedClosed => "failed_closed",
        }
    }

    /// Whether the row is off the operator's plate. A skip and a fail-closed
    /// are not failures of the pass, but they are not resolutions either, so
    /// the caller reports them as §16.3 partial rather than success.
    pub fn resolved(self) -> bool {
        !matches!(
            self,
            ReconcileOutcome::SkippedInFlight | ReconcileOutcome::FailedClosed
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReconcileResult {
    pub operation_uid: Uuid,
    pub space_uid: SpaceUid,
    pub logical_name: String,
    pub kind: OperationKind,
    pub duty: &'static str,
    pub outcome: ReconcileOutcome,
    /// The evidence behind `outcome`, in the operator's words.
    pub detail: String,
    pub ok: bool,
}

/// The compensating half of a crashed Wez adoption: the same fork CAS rename
/// [`adopt_wez`]'s own failure path uses to put a workspace's name back, plus
/// the sole-window lookup that verb needs. Taken as a trait object so
/// [`reconcile_apply`] keeps one signature for both backends — tmux has no
/// such verb and needs none, because its adoption mutation is an option stamp,
/// never a rename.
pub trait WorkspaceRestore {
    fn sole_window_id(&self, scope: &InventoryScope, native_token: &str) -> ProviderResult<u64>;

    fn cas_rename_workspace(
        &self,
        scope: &InventoryScope,
        window_id: u64,
        expected_workspace: &str,
        new_workspace: &str,
        expect_sole_window: bool,
    ) -> ProviderResult<crate::backend::wez::CasRenameOutcome>;
}

impl<R: crate::backend::wez::WezRunner> WorkspaceRestore for crate::backend::wez::WezProvider<R> {
    fn sole_window_id(&self, scope: &InventoryScope, native_token: &str) -> ProviderResult<u64> {
        crate::backend::wez::WezProvider::sole_window_id(self, scope, native_token)
    }

    fn cas_rename_workspace(
        &self,
        scope: &InventoryScope,
        window_id: u64,
        expected_workspace: &str,
        new_workspace: &str,
        expect_sole_window: bool,
    ) -> ProviderResult<crate::backend::wez::CasRenameOutcome> {
        crate::backend::wez::WezProvider::cas_rename_workspace(
            self,
            scope,
            window_id,
            expected_workspace,
            new_workspace,
            expect_sole_window,
        )
    }
}

/// The Space's backend as reconciliation may use it: the scan every duty
/// reasons from, and — on a backend that has one — the CAS rename a landed
/// adoption has to be compensated with.
pub struct ReconcileBackend<'a> {
    pub provider: &'a dyn Provider,
    pub scope: &'a InventoryScope,
    /// `None` on tmux, and on any Wez endpoint whose CAS verb could not be
    /// obtained: a landed adoption rename then has no compensation and fails
    /// closed rather than being released behind an opaque key.
    pub restore: Option<&'a dyn WorkspaceRestore>,
}

impl<'a> ReconcileBackend<'a> {
    /// A backend that can be scanned but not CAS-renamed (tmux).
    pub fn scan_only(provider: &'a dyn Provider, scope: &'a InventoryScope) -> Self {
        Self {
            provider,
            scope,
            restore: None,
        }
    }

    /// A backend whose adoption rename can be undone (Wez, fork CAS).
    pub fn restorable(
        provider: &'a dyn Provider,
        scope: &'a InventoryScope,
        restore: &'a dyn WorkspaceRestore,
    ) -> Self {
        Self {
            provider,
            scope,
            restore: Some(restore),
        }
    }
}

/// Preview: every unfinished journal row with its duty and whether a live
/// process still owns it. Read-only — nothing is mutated and no lock outlives
/// the probe, so this is safe to run before the operator answers the prompt.
pub fn reconcile_scan(env: &OperationEnv) -> Result<Vec<ReconcileTarget>, OpError> {
    let registry =
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).map_err(reg_err)?;
    let identity = registry.identity().map_err(reg_err)?;
    let mut targets = Vec::new();
    for row in registry.unfinished_operations().map_err(reg_err)? {
        let space = registry.space(row.space_uid).map_err(reg_err)?;
        let info = registry
            .backend_instance_info(space.backend_instance)
            .map_err(reg_err)?;
        targets.push(ReconcileTarget {
            operation_uid: row.operation_uid,
            request_uid: row.request_uid,
            space_uid: space.space_uid,
            space_no: space.space_no,
            logical_name: space.logical_name.clone(),
            backend: info.backend,
            backend_instance: space.backend_instance,
            kind: row.kind,
            state: row.state,
            lifecycle: space.lifecycle,
            duty: reconcile::resume_duty(row.kind, row.state).as_str(),
            in_flight: probe_in_flight(
                env,
                identity.host_uid,
                &space.logical_name,
                space.backend_instance,
            )?,
            started_at: row.started_at,
        });
    }
    Ok(targets)
}

/// Crashed, or still running? Nothing durable tells them apart — a `prepared`
/// row looks identical either way, and wall-clock age is not evidence (a slow
/// bootstrap is not a crash). So ask the kernel: a §10.1 mutation holds its
/// scopes exclusively for the whole call, and the kernel drops them the
/// instant the holder dies. "We can take them" is exactly "nobody is running".
///
/// The probe must test *every* scope [`reconcile_apply`] will need, not just
/// the decision lock: `remove_space_inner` takes the authority gate, the
/// backend-instance lock and the Space lock and never a decision lock, so a
/// live `dmux rm` is invisible to a decision-only probe. The preview would
/// then call a running remove `crashed` while the apply — which does try the
/// instance lock — skips it, and the operator would be confirming a row whose
/// stated condition is false. A busy instance therefore reads as in flight
/// even when the busy holder is some *other* Space on that instance: that is
/// exactly what the apply will do with it, and preview and apply agreeing is
/// the whole contract of a confirmed verb.
fn probe_in_flight(
    env: &OperationEnv,
    owner: HostUid,
    name: &str,
    instance: BackendInstanceUid,
) -> Result<bool, OpError> {
    let mut locks = OrderedLocks::new(&env.lock_dir);
    // Shared, so this waits only on an exclusive authority writer (backup or
    // restore), never on another operation.
    locks
        .acquire(LockScope::AuthorityGate, LockMode::Shared)
        .map_err(|e| OpError::Lock(e.to_string()))?;
    if !locks
        .try_acquire(LockScope::decision(owner, name), LockMode::Exclusive)
        .map_err(|e| OpError::Lock(e.to_string()))?
    {
        return Ok(true);
    }
    let free = locks
        .try_acquire(LockScope::BackendInstance(instance), LockMode::Exclusive)
        .map_err(|e| OpError::Lock(e.to_string()))?;
    Ok(!free)
}

/// Resolve one stranded row. `backend` is the Space's provider/scope when one
/// could be reached; `None` is not fatal for a tmux adoption reservation —
/// which mutates nothing a scan could observe — but every duty that needs
/// native evidence fails closed without it rather than guess.
pub fn reconcile_apply(
    env: &OperationEnv,
    target: &ReconcileTarget,
    backend: Option<ReconcileBackend<'_>>,
) -> ReconcileResult {
    let report = |outcome: ReconcileOutcome, detail: String| ReconcileResult {
        operation_uid: target.operation_uid,
        space_uid: target.space_uid,
        logical_name: target.logical_name.clone(),
        kind: target.kind,
        duty: target.duty,
        outcome,
        detail,
        ok: outcome.resolved(),
    };
    let failed = |detail: String| report(ReconcileOutcome::FailedClosed, detail);

    let mut registry = match Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)) {
        Ok(registry) => registry,
        Err(e) => return failed(format!("registry: {e}")),
    };
    let owner = match registry.identity() {
        Ok(identity) => identity.host_uid,
        Err(e) => return failed(format!("registry: {e}")),
    };

    // The fence, in §10.1 order. The authority gate is taken *shared*, so it
    // blocks only against an exclusive authority writer (backup/restore),
    // which by §10.1 overlaps nothing anyway. Every operation-owned scope
    // below it is acquired non-blocking on purpose: a busy scope is a live
    // holder, and repair waits for nobody.
    let mut locks = OrderedLocks::new(&env.lock_dir);
    if let Err(e) = locks.acquire(LockScope::AuthorityGate, LockMode::Shared) {
        return failed(format!("kernel lock: {e}"));
    }
    match locks.try_acquire(
        LockScope::decision(owner, &target.logical_name),
        LockMode::Exclusive,
    ) {
        Ok(true) => {}
        Ok(false) => {
            return report(
                ReconcileOutcome::SkippedInFlight,
                format!(
                    "a live holder owns the decision lock for {:?}",
                    target.logical_name
                ),
            );
        }
        Err(e) => return failed(format!("kernel lock: {e}")),
    }
    match locks.try_acquire(
        LockScope::BackendInstance(target.backend_instance),
        LockMode::Exclusive,
    ) {
        Ok(true) => {}
        Ok(false) => {
            return report(
                ReconcileOutcome::SkippedInFlight,
                format!("backend instance {} is busy", target.backend_instance.0),
            );
        }
        Err(e) => return failed(format!("kernel lock: {e}")),
    }

    // Re-read under the fence: the preview is advisory, and a row that
    // finished in between must not be reconciled a second time.
    let row = match registry.operation(target.operation_uid) {
        Ok(row) => row,
        Err(e) => return failed(format!("registry: {e}")),
    };
    if row.state.is_terminal() {
        return report(
            ReconcileOutcome::AlreadyResolved,
            format!("journal row is already {}", row.state.as_str()),
        );
    }
    let space = match registry.space(target.space_uid) {
        Ok(space) => space,
        Err(e) => return failed(format!("registry: {e}")),
    };
    let binding = match registry.current_binding(space.space_uid) {
        Ok(binding) => binding,
        Err(e) => return failed(format!("registry: {e}")),
    };

    match reconcile::resume_duty(row.kind, row.state) {
        // §10.3 reconciles an adoption by source token, destination key and
        // epoch — so the destination key has to be *looked for*, per backend.
        // A reservation that never reached `finalize_adopt` has no binding,
        // but "no binding" does not mean "no mutation": `adopt_wez` lands its
        // CAS rename to the opaque key BEFORE it binds, and its own failure
        // path compensates with a reverse CAS precisely because an abort
        // alone would leave the workspace wearing an opaque key for a Space
        // that never existed — neither managed nor recoverably unmanaged.
        // Reconciliation owes the crashed holder that same compensation.
        ResumeDuty::AdoptionReconcile => {
            if space.lifecycle != Lifecycle::Reserved || binding.is_some() {
                return failed(format!(
                    "{} journal row on a {:?} Space with {} binding — conflict, not a stranded reservation",
                    row.kind.as_str(),
                    space.lifecycle,
                    if binding.is_some() { "a current" } else { "no" },
                ));
            }
            let release = |registry: &mut Registry, evidence: String| match registry
                .abort_create(space.space_uid, row.operation_uid)
            {
                Ok(()) => report(
                    ReconcileOutcome::ReservationReleased,
                    format!("{evidence}; name {:?} is free again", space.logical_name),
                ),
                Err(e) => failed(format!("registry: {e}")),
            };
            match target.backend {
                // tmux adoption renames nothing: it stamps `@dmux_*` options
                // on the source session and then binds. A crash leaves those
                // options behind naming an `aborted` Space, which
                // `Lifecycle::occupies_name` excludes and `dmux adopt`
                // deliberately overwrites — the session keeps its own name
                // throughout, so it is unmanaged and stays addressable.
                Backend::Tmux => release(
                    &mut registry,
                    format!(
                        "unmanaged: a tmux {} renames nothing, so the source session still \
                         answers to its own name; any @dmux_* stamp the crashed holder wrote \
                         names this aborted Space and a re-run of `dmux adopt` overwrites it",
                        row.kind.as_str()
                    ),
                ),
                Backend::Wez => {
                    let opaque_key = adoption_key(owner, space.space_uid);
                    let Some(backend) = backend else {
                        return failed(format!(
                            "the wez server could not be reached, so whether the {} renamed \
                             workspace → {opaque_key:?} is unknown; releasing now could strand \
                             a workspace under an opaque key for a Space that never existed. \
                             Start the managed server and re-run, or rename that workspace \
                             back by hand first",
                            row.kind.as_str()
                        ));
                    };
                    let rows = match backend.provider.inventory(backend.scope) {
                        InventoryOutcome::Complete(inv) => inv.rows,
                        other => {
                            return failed(format!(
                                "wez scan: {other:?} — cannot tell whether the {} renamed a \
                                 workspace to {opaque_key:?}",
                                row.kind.as_str()
                            ));
                        }
                    };
                    if !rows.iter().any(|r| r.native_token == opaque_key) {
                        return release(
                            &mut registry,
                            format!(
                                "unmanaged: a complete wez scan shows no workspace under the \
                                 reservation's key {opaque_key:?}, so the atomic rename never \
                                 landed and nothing was left renamed"
                            ),
                        );
                    }
                    // The rename landed. Undo it before the row closes, under
                    // the same `--if-workspace`/`--if-sole-window` guard the
                    // adopt used, so a racer's workspace is never touched.
                    let Some(restore) = backend.restore else {
                        return failed(format!(
                            "workspace {opaque_key:?} still wears this reservation's opaque key \
                             and this endpoint offers no CAS rename to put it back; rename it \
                             yourself, then re-run `dmux repair reconcile`"
                        ));
                    };
                    // The source token the reverse CAS should aim at is not
                    // journaled (§10.3 asks for it; `reserve_space_kind`
                    // records only name/instance), so the reservation's own
                    // logical name is the closest recorded truth: identical to
                    // the source workspace unless the operator passed
                    // `--name`. Merging into a live workspace of that name
                    // would be a mutation nobody asked for, so a collision
                    // fails closed instead.
                    let restored_to = space.logical_name.clone();
                    if rows.iter().any(|r| r.native_token == restored_to) {
                        return failed(format!(
                            "workspace {opaque_key:?} still wears this reservation's opaque key, \
                             but a live workspace is already named {restored_to:?}, so putting \
                             the name back would merge two workspaces; rename one of them and \
                             re-run `dmux repair reconcile`"
                        ));
                    }
                    let window_id = match restore.sole_window_id(backend.scope, &opaque_key) {
                        Ok(window_id) => window_id,
                        Err(e) => {
                            return failed(format!(
                                "workspace {opaque_key:?} still wears this reservation's opaque \
                                 key and its sole window could not be resolved ({e:?}); the \
                                 rename cannot be undone, so the row stays open"
                            ));
                        }
                    };
                    match restore.cas_rename_workspace(
                        backend.scope,
                        window_id,
                        &opaque_key,
                        &restored_to,
                        true,
                    ) {
                        Ok(CasRenameOutcome::Renamed) => release(
                            &mut registry,
                            format!(
                                "unmanaged: the {} had renamed a workspace to {opaque_key:?}, \
                                 and the compensating atomic rename put it back to \
                                 {restored_to:?} (the reservation's logical name — the source \
                                 token itself is not journaled)",
                                row.kind.as_str()
                            ),
                        ),
                        Ok(other) => failed(format!(
                            "workspace {opaque_key:?} still wears this reservation's opaque key: \
                             the compensating rename to {restored_to:?} was refused ({other:?}, \
                             zero mutation); the row stays open rather than strand it"
                        )),
                        Err(e) => failed(format!(
                            "workspace {opaque_key:?} still wears this reservation's opaque key: \
                             the compensating rename to {restored_to:?} failed ({e:?}); the row \
                             stays open rather than strand it"
                        )),
                    }
                }
            }
        }

        ResumeDuty::CreateKeyedLookup => {
            if space.lifecycle != Lifecycle::Reserved {
                return failed(format!(
                    "create journal row on a {:?} Space, not a reservation",
                    space.lifecycle
                ));
            }
            let (scan, evidence) = match &backend {
                Some(backend) => {
                    create_keyed_scan(&registry, owner, &space, backend.provider, backend.scope)
                }
                None => (
                    CreateScan::Indeterminate,
                    "the Space's backend could not be reached for the keyed lookup".to_string(),
                ),
            };
            match reconcile::decide_create(scan) {
                // Zero matches PERMITS one re-create under the same fence.
                // Repair deliberately takes only the weaker half of that
                // permission: it spawns nothing and releases the reservation,
                // leaving the operator to decide whether the Space is still
                // wanted. `dmux new` is not a repair action.
                CreateDecision::RetryCreate => {
                    match registry.abort_create(space.space_uid, row.operation_uid) {
                        Ok(()) => report(
                            ReconcileOutcome::ReservationReleased,
                            format!("{evidence}; name {:?} is free again", space.logical_name),
                        ),
                        Err(e) => failed(format!("registry: {e}")),
                    }
                }
                // Binding an orphan needs the bootstrap acknowledgement that
                // proves dmux created it *and* that no user program is
                // running in it unwitnessed, which `create_space_locked` has
                // and a repair pass does not. That refusal stands — but
                // "fails closed" must not mean "burned forever": without a
                // named way out the name stays unusable by `new`, `rm`,
                // `rename` and `adopt` alike, which is the damage this verb
                // exists to end. §10.3's own remedy for asserting identity
                // over an unmanaged resource is `repair rebind`, which is not
                // implemented yet, so the refusal also names the route that
                // works today and preserves the orphan: move it off the
                // reserved key, which turns this row into the zero-match case
                // above, then re-adopt it deliberately.
                CreateDecision::RebindAndFinalize => failed(format!(
                    "{evidence}; repair will not bind a native resource it cannot prove dmux \
                     created — that assertion is `dmux repair rebind` (plan §10.3), which is \
                     not implemented yet. Until it is, free the name {name:?} without losing \
                     the resource: rename it off the reserved key ({hint}), re-run \
                     `dmux repair reconcile` (the keyed lookup then finds nothing and releases \
                     the reservation), then `dmux adopt` it back under your own confirmation",
                    name = space.logical_name,
                    hint = orphan_rename_hint(
                        target.backend,
                        backend.as_ref().map(|b| b.scope),
                        &create_key(owner, &space, target.backend)
                    ),
                )),
                CreateDecision::FailClosed => failed(evidence),
            }
        }

        ResumeDuty::RenameObserveStates => {
            let payload: serde_json::Value = match serde_json::from_str(&row.payload_json) {
                Ok(payload) => payload,
                Err(e) => return failed(format!("rename payload: {e}")),
            };
            let (old, new) = match (payload["old"].as_str(), payload["new"].as_str()) {
                (Some(old), Some(new)) => (old.to_string(), new.to_string()),
                _ => return failed("rename payload missing old/new".into()),
            };
            let (decision, evidence) = match target.backend {
                // A Wez rename never touches the native side — the opaque
                // workspace key is not the logical name — so the old native
                // state is intact by construction and the new one cannot
                // exist. The table reads that as old-only.
                Backend::Wez => (
                    reconcile::decide_rename(true, false),
                    "wez renames touch no native name".to_string(),
                ),
                Backend::Tmux => {
                    let Some(backend) = &backend else {
                        return failed(
                            "the Space's backend could not be reached to observe the old/new names"
                                .into(),
                        );
                    };
                    let rows = match backend.provider.inventory(backend.scope) {
                        InventoryOutcome::Complete(inv) => inv.rows,
                        other => {
                            return failed(format!("tmux scan: {other:?}"));
                        }
                    };
                    let old_exists = rows.iter().any(|r| r.native_name == old);
                    let new_exists = rows.iter().any(|r| r.native_name == new);
                    (
                        reconcile::decide_rename(old_exists, new_exists),
                        format!("tmux shows old={old_exists} new={new_exists}"),
                    )
                }
            };
            match decision {
                // The native step never ran, so the Space still carries the
                // old name and closing the row IS the rollback.
                RenameDecision::RetryNativeRename => {
                    match registry.transition_operation(row.operation_uid, OperationState::Aborted)
                    {
                        Ok(()) => report(
                            ReconcileOutcome::RenameRolledBack,
                            format!("{evidence}; {old:?} → {new:?} never landed"),
                        ),
                        Err(e) => failed(format!("registry: {e}")),
                    }
                }
                RenameDecision::CommitRegistryRename => {
                    match registry.commit_rename(space.space_uid, row.operation_uid) {
                        Ok(()) => report(
                            ReconcileOutcome::RenameCommitted,
                            format!("{evidence}; registry caught up to {new:?}"),
                        ),
                        Err(e) => failed(format!("registry: {e}")),
                    }
                }
                RenameDecision::ConflictBothExist => failed(format!(
                    "{evidence}; both {old:?} and {new:?} exist — picking one could destroy an external resource"
                )),
                RenameDecision::ConflictNeitherExists => {
                    failed(format!("{evidence}; neither {old:?} nor {new:?} exists"))
                }
            }
        }

        ResumeDuty::RemoveVerifyAbsence => {
            let Some(backend) = &backend else {
                return failed(
                    "the Space's backend could not be reached to prove the resource is gone".into(),
                );
            };
            // `resume_remove_space` takes the very locks this fence holds,
            // and OFD locks do not nest across open descriptions even inside
            // one process — a blocking re-acquisition would wait on us
            // forever. Hand the fence over instead; the resume re-validates
            // the exact unfinished row under its own.
            locks.release_all();
            match resume_remove_space(
                env,
                backend.provider,
                backend.scope,
                target.backend,
                space.space_uid,
                row.request_uid,
                row.operation_uid,
            ) {
                Ok(()) => report(
                    ReconcileOutcome::RemoveCompleted,
                    format!("verified absence; {:?} is tombstoned", space.logical_name),
                ),
                Err(e) => failed(e.to_string()),
            }
        }

        ResumeDuty::Nothing => report(
            ReconcileOutcome::AlreadyResolved,
            format!("journal row is already {}", row.state.as_str()),
        ),
    }
}

/// The opaque destination key a Wez adoption renames its source workspace to
/// (`adopt_wez`), and the key a Wez create reserves for its spawn. One
/// formula, spelled once: reconciliation looks for exactly what those two
/// wrote.
fn adoption_key(owner: HostUid, space_uid: SpaceUid) -> String {
    format!("dmux:{}:{}", owner.0, space_uid.0)
}

/// The key a crashed create's keyed lookup must search for.
fn create_key(owner: HostUid, space: &crate::registry::SpaceRow, backend: Backend) -> String {
    match backend {
        Backend::Wez => adoption_key(owner, space.space_uid),
        Backend::Tmux => space.logical_name.clone(),
    }
}

/// The exact native command that moves an orphan off the reserved key, so a
/// refusal can name a remedy the operator can paste rather than a principle.
fn orphan_rename_hint(backend: Backend, scope: Option<&InventoryScope>, key: &str) -> String {
    match backend {
        Backend::Tmux => match scope {
            Some(scope) => format!(
                "tmux -L {} rename-session -t {key} {key}.orphan",
                scope.endpoint
            ),
            None => format!("tmux rename-session -t {key} {key}.orphan"),
        },
        Backend::Wez => format!(
            "rename the workspace {key:?} in the GUI, or with the fork's \
             `wezterm cli rename-workspace`"
        ),
    }
}

/// The complete keyed lookup §10.2 create step 3 demands, by the key the
/// reservation fixed: Wez's opaque `dmux:<owner>:<space_uid>` (unique to this
/// reservation) or, on tmux, the session name the spawn was asked for.
fn create_keyed_scan(
    registry: &Registry,
    owner: HostUid,
    space: &crate::registry::SpaceRow,
    provider: &dyn Provider,
    scope: &InventoryScope,
) -> (CreateScan, String) {
    let key = create_key(owner, space, scope.backend);
    let rows = match provider.inventory(scope) {
        InventoryOutcome::Complete(inv) => inv.rows,
        other => {
            return (
                CreateScan::Indeterminate,
                format!("{} scan: {other:?}", scope.backend),
            );
        }
    };
    let matched: Vec<&NativeSpaceRow> = rows
        .iter()
        .filter(|row| match scope.backend {
            Backend::Wez => row.native_token == key,
            Backend::Tmux => row.native_name == key,
        })
        .collect();
    match matched.as_slice() {
        [] => (
            CreateScan::ZeroMatches,
            format!(
                "a complete {} scan shows no resource under the reserved key {key:?}",
                scope.backend
            ),
        ),
        [one] => {
            let bound = registry
                .current_binding_by_native(space.backend_instance, &one.native_token)
                .ok()
                .flatten();
            if one.multi_window || bound.is_some() {
                (
                    CreateScan::MultipleOrConflicting,
                    format!("{key:?} exists but is multi-window or already bound"),
                )
            } else {
                (
                    CreateScan::OneConforming,
                    format!("one conforming resource still carries the reserved key {key:?}"),
                )
            }
        }
        many => (
            CreateScan::MultipleOrConflicting,
            format!("{} resources carry the reserved key {key:?}", many.len()),
        ),
    }
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::backend::{
        Capabilities, NativeBinding, NativeGroupRow, NativeInventory, NativeSpaceRow,
        NativeSplitRow, PresentationTarget, ProviderError, ProviderResult, SplitSpec,
    };

    use super::*;

    struct CreateGateProvider {
        backend: Backend,
        outcome: InventoryOutcome,
        scans: AtomicUsize,
        creates: AtomicUsize,
    }

    impl CreateGateProvider {
        fn new(backend: Backend, outcome: InventoryOutcome) -> Self {
            Self {
                backend,
                outcome,
                scans: AtomicUsize::new(0),
                creates: AtomicUsize::new(0),
            }
        }
    }

    impl Provider for CreateGateProvider {
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                backend: self.backend,
                cas_rename: false,
                probed: Vec::new(),
            }
        }

        fn inventory(&self, _scope: &InventoryScope) -> InventoryOutcome {
            self.scans.fetch_add(1, Ordering::SeqCst);
            self.outcome.clone()
        }

        fn create(
            &self,
            _scope: &InventoryScope,
            _spec: &CreateSpec,
        ) -> ProviderResult<NativeBinding> {
            self.creates.fetch_add(1, Ordering::SeqCst);
            Err(ProviderError::NativeFailure {
                detail: "create must not be reached by a preflight gate test".into(),
            })
        }

        fn prepare_presentation(
            &self,
            _scope: &InventoryScope,
            _binding: &NativeBinding,
            _child: Option<&ProviderHandle>,
        ) -> ProviderResult<PresentationTarget> {
            Err(ProviderError::NativeFailure {
                detail: "unused".into(),
            })
        }

        fn rename(
            &self,
            _scope: &InventoryScope,
            _binding: &NativeBinding,
            _new_native_name: &str,
        ) -> ProviderResult<()> {
            Err(ProviderError::NativeFailure {
                detail: "unused".into(),
            })
        }

        fn remove(&self, _scope: &InventoryScope, _binding: &NativeBinding) -> ProviderResult<()> {
            Err(ProviderError::NativeFailure {
                detail: "unused".into(),
            })
        }

        fn group_list(
            &self,
            _scope: &InventoryScope,
            _binding: &NativeBinding,
        ) -> ProviderResult<Vec<NativeGroupRow>> {
            Err(ProviderError::NativeFailure {
                detail: "unused".into(),
            })
        }

        fn group_new(
            &self,
            _scope: &InventoryScope,
            _binding: &NativeBinding,
            _spec: &CreateSpec,
        ) -> ProviderResult<ProviderHandle> {
            Err(ProviderError::NativeFailure {
                detail: "unused".into(),
            })
        }

        fn group_activate(
            &self,
            _scope: &InventoryScope,
            _handle: &ProviderHandle,
        ) -> ProviderResult<()> {
            Err(ProviderError::NativeFailure {
                detail: "unused".into(),
            })
        }

        fn group_rename(
            &self,
            _scope: &InventoryScope,
            _handle: &ProviderHandle,
            _title: &str,
        ) -> ProviderResult<()> {
            Err(ProviderError::NativeFailure {
                detail: "unused".into(),
            })
        }

        fn group_remove(
            &self,
            _scope: &InventoryScope,
            _handle: &ProviderHandle,
        ) -> ProviderResult<()> {
            Err(ProviderError::NativeFailure {
                detail: "unused".into(),
            })
        }

        fn split_list(
            &self,
            _scope: &InventoryScope,
            _group: &ProviderHandle,
        ) -> ProviderResult<Vec<NativeSplitRow>> {
            Err(ProviderError::NativeFailure {
                detail: "unused".into(),
            })
        }

        fn split_new(
            &self,
            _scope: &InventoryScope,
            _group: &ProviderHandle,
            _spec: &SplitSpec,
        ) -> ProviderResult<ProviderHandle> {
            Err(ProviderError::NativeFailure {
                detail: "unused".into(),
            })
        }

        fn split_activate(
            &self,
            _scope: &InventoryScope,
            _handle: &ProviderHandle,
        ) -> ProviderResult<()> {
            Err(ProviderError::NativeFailure {
                detail: "unused".into(),
            })
        }

        fn split_remove(
            &self,
            _scope: &InventoryScope,
            _handle: &ProviderHandle,
        ) -> ProviderResult<()> {
            Err(ProviderError::NativeFailure {
                detail: "unused".into(),
            })
        }

        fn inspect(
            &self,
            _scope: &InventoryScope,
            _binding: &NativeBinding,
        ) -> ProviderResult<NativeSpaceRow> {
            Err(ProviderError::NativeFailure {
                detail: "unused".into(),
            })
        }
    }

    fn empty_inventory(epoch: ServerEpoch) -> InventoryOutcome {
        InventoryOutcome::Complete(NativeInventory {
            server_epoch: Some(epoch),
            rows: Vec::new(),
        })
    }

    fn gate_test_env() -> (tempfile::TempDir, tempfile::TempDir, OperationEnv) {
        let data = tempfile::tempdir().unwrap();
        let locks = tempfile::tempdir().unwrap();
        let env = OperationEnv {
            db_path: data.path().join("registry.sqlite3"),
            lock_dir: locks.path().to_path_buf(),
        };
        (data, locks, env)
    }

    fn gate_request(name: &str) -> CreateRequest {
        CreateRequest {
            request_uid: Uuid::new_v4(),
            name: name.into(),
            cwd: None,
            program: Vec::new(),
            helper_bin: "/unused/pane-bootstrap".into(),
        }
    }

    #[test]
    fn owner_fenced_create_scans_both_and_refuses_opposite_managed_name_before_allocation() {
        let (_data, _locks, env) = gate_test_env();
        let epoch = ServerEpoch(Uuid::new_v4());
        let mut registry =
            Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
        let tmux_instance = registry
            .register_backend_instance(Backend::Tmux, Some("tmux-gate"), None)
            .unwrap();
        let wez_instance = registry
            .register_backend_instance(Backend::Wez, Some("/tmp/wez-gate.sock"), None)
            .unwrap();
        registry
            .publish_backend_server(tmux_instance, epoch, Some(4242), Some("start"), None, None)
            .unwrap();
        registry
            .publish_backend_server(wez_instance, epoch, Some(4243), Some("start"), None, None)
            .unwrap();
        let held = registry
            .reserve_space("collision", wez_instance, Uuid::new_v4())
            .unwrap();
        drop(registry);

        let tmux = CreateGateProvider::new(Backend::Tmux, empty_inventory(epoch));
        let wez = CreateGateProvider::new(Backend::Wez, empty_inventory(epoch));
        let tmux_scope = InventoryScope::managed(Backend::Tmux, "tmux-gate", epoch);
        let wez_scope = InventoryScope::managed(Backend::Wez, "/tmp/wez-gate.sock", epoch);
        let error = create_space_owner_fenced(
            &env,
            OwnerCreateTarget {
                backend: Backend::Tmux,
                instance: tmux_instance,
                provider: &tmux,
                scope: &tmux_scope,
            },
            Some(OwnerCreateTarget {
                backend: Backend::Wez,
                instance: wez_instance,
                provider: &wez,
                scope: &wez_scope,
            }),
            false,
            &gate_request("collision"),
        )
        .unwrap_err();
        assert!(matches!(error, OpError::NameConflict(_)), "{error}");
        assert_eq!(tmux.scans.load(Ordering::SeqCst), 1);
        assert_eq!(wez.scans.load(Ordering::SeqCst), 1);
        assert_eq!(tmux.creates.load(Ordering::SeqCst), 0);
        let spaces = Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir))
            .unwrap()
            .spaces()
            .unwrap();
        assert_eq!(spaces.len(), 1, "refusal must not allocate another Space");
        assert_eq!(spaces[0].space_uid, held.space_uid);
    }

    #[test]
    fn owner_lookup_surfaces_selectable_and_unmanaged_exact_name_without_native_ids() {
        let (_data, _locks, env) = gate_test_env();
        let epoch = ServerEpoch(Uuid::from_u128(72));
        let mut registry =
            Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
        let tmux_instance = registry
            .register_backend_instance(Backend::Tmux, Some("tmux-gate"), None)
            .unwrap();
        let wez_instance = registry
            .register_backend_instance(Backend::Wez, Some("/tmp/wez-gate.sock"), None)
            .unwrap();
        registry
            .publish_backend_server(tmux_instance, epoch, Some(4242), Some("start"), None, None)
            .unwrap();
        registry
            .publish_backend_server(wez_instance, epoch, Some(4243), Some("start"), None, None)
            .unwrap();
        let reservation = registry
            .reserve_space("collision", wez_instance, Uuid::new_v4())
            .unwrap();
        registry
            .finalize_create(
                reservation.space_uid,
                reservation.operation_uid,
                &NativeBindingSpec {
                    native_token: "managed-opposite".into(),
                    native_kind: NativeKind::WezWorkspaceKey,
                    server_epoch: Some(epoch),
                },
            )
            .unwrap();
        drop(registry);

        let tmux = CreateGateProvider::new(Backend::Tmux, empty_inventory(epoch));
        let wez = CreateGateProvider::new(
            Backend::Wez,
            InventoryOutcome::Complete(NativeInventory {
                server_epoch: Some(epoch),
                rows: vec![NativeSpaceRow {
                    native_token: "managed-opposite".into(),
                    native_name: "collision".into(),
                    groups: Vec::new(),
                    multi_window: false,
                }],
            }),
        );
        let tmux_scope = InventoryScope::managed(Backend::Tmux, "tmux-gate", epoch);
        let wez_scope = InventoryScope::managed(Backend::Wez, "/tmp/wez-gate.sock", epoch);
        let lookup = lookup_new_owner_fenced(
            &env,
            Some(OwnerCreateTarget {
                backend: Backend::Wez,
                instance: wez_instance,
                provider: &wez,
                scope: &wez_scope,
            }),
            Some(OwnerCreateTarget {
                backend: Backend::Tmux,
                instance: tmux_instance,
                provider: &tmux,
                scope: &tmux_scope,
            }),
            "collision",
        )
        .unwrap();
        assert_eq!(
            lookup.wez,
            ClassSummary::Selectable {
                space: reservation.space_uid,
                no: reservation.space_no,
            }
        );
        assert_eq!(lookup.tmux, ClassSummary::NoMatch);

        let unmanaged_tmux = CreateGateProvider::new(
            Backend::Tmux,
            InventoryOutcome::Complete(NativeInventory {
                server_epoch: Some(epoch),
                rows: vec![NativeSpaceRow {
                    native_token: "external".into(),
                    native_name: "collision".into(),
                    groups: Vec::new(),
                    multi_window: false,
                }],
            }),
        );
        let lookup = lookup_new_owner_fenced(
            &env,
            Some(OwnerCreateTarget {
                backend: Backend::Wez,
                instance: wez_instance,
                provider: &wez,
                scope: &wez_scope,
            }),
            Some(OwnerCreateTarget {
                backend: Backend::Tmux,
                instance: tmux_instance,
                provider: &unmanaged_tmux,
                scope: &tmux_scope,
            }),
            "collision",
        )
        .unwrap();
        assert_eq!(
            lookup.tmux,
            ClassSummary::Blocking {
                reason: crate::resolve::BlockReason::UnmanagedSameName,
                space: None,
            }
        );
    }

    #[test]
    fn collision_acknowledgement_allows_only_opposite_selectable_managed_row() {
        let (_data, _locks, env) = gate_test_env();
        let epoch = ServerEpoch(Uuid::from_u128(73));
        let mut registry =
            Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
        let tmux_instance = registry
            .register_backend_instance(Backend::Tmux, Some("tmux-gate"), None)
            .unwrap();
        let wez_instance = registry
            .register_backend_instance(Backend::Wez, Some("/tmp/wez-gate.sock"), None)
            .unwrap();
        registry
            .publish_backend_server(tmux_instance, epoch, Some(4242), Some("start"), None, None)
            .unwrap();
        registry
            .publish_backend_server(wez_instance, epoch, Some(4243), Some("start"), None, None)
            .unwrap();
        let reservation = registry
            .reserve_space("collision", wez_instance, Uuid::new_v4())
            .unwrap();
        registry
            .finalize_create(
                reservation.space_uid,
                reservation.operation_uid,
                &NativeBindingSpec {
                    native_token: "managed-opposite".into(),
                    native_kind: NativeKind::WezWorkspaceKey,
                    server_epoch: Some(epoch),
                },
            )
            .unwrap();
        drop(registry);
        let tmux = CreateGateProvider::new(Backend::Tmux, empty_inventory(epoch));
        let wez = CreateGateProvider::new(
            Backend::Wez,
            InventoryOutcome::Complete(NativeInventory {
                server_epoch: Some(epoch),
                rows: vec![NativeSpaceRow {
                    native_token: "managed-opposite".into(),
                    native_name: "collision".into(),
                    groups: Vec::new(),
                    multi_window: false,
                }],
            }),
        );
        let tmux_scope = InventoryScope::managed(Backend::Tmux, "tmux-gate", epoch);
        let wez_scope = InventoryScope::managed(Backend::Wez, "/tmp/wez-gate.sock", epoch);
        let refused_request = gate_request("collision");
        let refused = create_space_owner_fenced(
            &env,
            OwnerCreateTarget {
                backend: Backend::Tmux,
                instance: tmux_instance,
                provider: &tmux,
                scope: &tmux_scope,
            },
            Some(OwnerCreateTarget {
                backend: Backend::Wez,
                instance: wez_instance,
                provider: &wez,
                scope: &wez_scope,
            }),
            false,
            &refused_request,
        )
        .unwrap_err();
        assert!(matches!(refused, OpError::NameConflict(_)), "{refused}");
        assert_eq!(tmux.creates.load(Ordering::SeqCst), 0);
        let changed_replay = create_space_owner_fenced(
            &env,
            OwnerCreateTarget {
                backend: Backend::Tmux,
                instance: tmux_instance,
                provider: &tmux,
                scope: &tmux_scope,
            },
            Some(OwnerCreateTarget {
                backend: Backend::Wez,
                instance: wez_instance,
                provider: &wez,
                scope: &wez_scope,
            }),
            true,
            &refused_request,
        )
        .unwrap_err();
        assert!(matches!(changed_replay, OpError::Registry(_)));

        let request = gate_request("collision");
        let error = create_space_owner_fenced(
            &env,
            OwnerCreateTarget {
                backend: Backend::Tmux,
                instance: tmux_instance,
                provider: &tmux,
                scope: &tmux_scope,
            },
            Some(OwnerCreateTarget {
                backend: Backend::Wez,
                instance: wez_instance,
                provider: &wez,
                scope: &wez_scope,
            }),
            true,
            &request,
        )
        .unwrap_err();
        assert!(matches!(error, OpError::Provider(_)), "{error}");
        assert_eq!(tmux.creates.load(Ordering::SeqCst), 1);
        assert_eq!(wez.creates.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn owner_fenced_create_refuses_opposite_unmanaged_name_and_indeterminate_scan_without_allocation()
     {
        for (tag, opposite_outcome, expected) in [
            (
                "unmanaged",
                InventoryOutcome::Complete(NativeInventory {
                    server_epoch: Some(ServerEpoch(Uuid::from_u128(71))),
                    rows: vec![NativeSpaceRow {
                        native_token: "external-opposite".into(),
                        native_name: "collision".into(),
                        groups: Vec::new(),
                        multi_window: false,
                    }],
                }),
                "name",
            ),
            (
                "indeterminate",
                InventoryOutcome::Unreachable {
                    detail: "injected opposite outage".into(),
                },
                "indeterminate",
            ),
        ] {
            let (_data, _locks, env) = gate_test_env();
            let epoch = ServerEpoch(Uuid::from_u128(71));
            let mut registry =
                Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
            let tmux_instance = registry
                .register_backend_instance(Backend::Tmux, Some("tmux-gate"), None)
                .unwrap();
            let wez_instance = registry
                .register_backend_instance(Backend::Wez, Some("/tmp/wez-gate.sock"), None)
                .unwrap();
            registry
                .publish_backend_server(tmux_instance, epoch, Some(4242), Some("start"), None, None)
                .unwrap();
            registry
                .publish_backend_server(wez_instance, epoch, Some(4243), Some("start"), None, None)
                .unwrap();
            drop(registry);
            let tmux = CreateGateProvider::new(Backend::Tmux, empty_inventory(epoch));
            let wez = CreateGateProvider::new(Backend::Wez, opposite_outcome);
            let tmux_scope = InventoryScope::managed(Backend::Tmux, "tmux-gate", epoch);
            let wez_scope = InventoryScope::managed(Backend::Wez, "/tmp/wez-gate.sock", epoch);
            let error = create_space_owner_fenced(
                &env,
                OwnerCreateTarget {
                    backend: Backend::Tmux,
                    instance: tmux_instance,
                    provider: &tmux,
                    scope: &tmux_scope,
                },
                Some(OwnerCreateTarget {
                    backend: Backend::Wez,
                    instance: wez_instance,
                    provider: &wez,
                    scope: &wez_scope,
                }),
                false,
                &gate_request("collision"),
            )
            .unwrap_err();
            match expected {
                "name" => assert!(matches!(error, OpError::NameConflict(_)), "{tag}: {error}"),
                _ => assert!(matches!(error, OpError::Indeterminate(_)), "{tag}: {error}"),
            }
            assert_eq!(tmux.scans.load(Ordering::SeqCst), 1, "{tag}");
            assert_eq!(wez.scans.load(Ordering::SeqCst), 1, "{tag}");
            assert_eq!(tmux.creates.load(Ordering::SeqCst), 0, "{tag}");
            assert!(
                Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir))
                    .unwrap()
                    .spaces()
                    .unwrap()
                    .is_empty(),
                "{tag}: a preflight refusal must consume no identity"
            );
        }
    }

    #[test]
    fn owner_fenced_create_requires_every_durable_opposite_target() {
        let (_data, _locks, env) = gate_test_env();
        let epoch = ServerEpoch(Uuid::new_v4());
        let mut registry =
            Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
        let tmux_instance = registry
            .register_backend_instance(Backend::Tmux, Some("tmux-gate"), None)
            .unwrap();
        let wez_instance = registry
            .register_backend_instance(Backend::Wez, Some("/tmp/wez-gate.sock"), None)
            .unwrap();
        registry
            .publish_backend_server(tmux_instance, epoch, Some(4242), Some("start"), None, None)
            .unwrap();
        registry
            .publish_backend_server(wez_instance, epoch, Some(4243), Some("start"), None, None)
            .unwrap();
        drop(registry);
        let tmux = CreateGateProvider::new(Backend::Tmux, empty_inventory(epoch));
        let scope = InventoryScope::managed(Backend::Tmux, "tmux-gate", epoch);
        let error = create_space_owner_fenced(
            &env,
            OwnerCreateTarget {
                backend: Backend::Tmux,
                instance: tmux_instance,
                provider: &tmux,
                scope: &scope,
            },
            None,
            false,
            &gate_request("missing-opposite"),
        )
        .unwrap_err();
        assert!(matches!(error, OpError::Refused(_)), "{error}");
        assert_eq!(tmux.scans.load(Ordering::SeqCst), 0);
        assert!(
            Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir))
                .unwrap()
                .spaces()
                .unwrap()
                .is_empty()
        );
    }

    // -- crash reconciliation (plan §10.2/§10.3, cases 11/13/39) ------------
    //
    // Every setup below is the shape a SIGKILL leaves behind: a journal row
    // opened, the process gone before it could be finished. Before
    // `reconcile_scan`/`reconcile_apply` existed nothing in production ever
    // called `registry::reconcile`, so none of these rows had a reaper.

    fn tmux_scope(epoch: ServerEpoch) -> InventoryScope {
        InventoryScope::managed(Backend::Tmux, "tmux-recon", epoch)
    }

    fn tmux_inventory(epoch: ServerEpoch, names: &[&str]) -> InventoryOutcome {
        InventoryOutcome::Complete(NativeInventory {
            server_epoch: Some(epoch),
            rows: names
                .iter()
                .map(|name| NativeSpaceRow {
                    native_token: format!("${name}"),
                    native_name: (*name).to_string(),
                    groups: Vec::new(),
                    multi_window: false,
                })
                .collect(),
        })
    }

    /// The exact state the adversarial pass produced by killing `dmux adopt`
    /// between `reserve_space_kind` and `abort_create`.
    fn stranded_adoption(
        env: &OperationEnv,
    ) -> (BackendInstanceUid, crate::registry::SpaceReservation) {
        let mut registry =
            Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
        let instance = registry
            .register_backend_instance(Backend::Tmux, Some("tmux-recon"), None)
            .unwrap();
        let reservation = registry
            .reserve_space_kind("legacy", instance, Uuid::new_v4(), OperationKind::Adopt)
            .unwrap();
        (instance, reservation)
    }

    #[test]
    fn a_crashed_adoption_reservation_is_released_and_gives_its_name_back() {
        let (_data, _locks, env) = gate_test_env();
        let (instance, reservation) = stranded_adoption(&env);

        let targets = reconcile_scan(&env).unwrap();
        assert_eq!(targets.len(), 1, "{targets:?}");
        assert_eq!(targets[0].kind, OperationKind::Adopt);
        assert_eq!(targets[0].state, OperationState::Prepared);
        assert_eq!(targets[0].lifecycle, Lifecycle::Reserved);
        assert_eq!(targets[0].duty, "adoption_reconcile");
        assert!(!targets[0].in_flight);

        let result = reconcile_apply(&env, &targets[0], None);
        assert_eq!(
            result.outcome,
            ReconcileOutcome::ReservationReleased,
            "{result:?}"
        );
        assert!(result.ok);

        let mut registry =
            Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
        assert_eq!(
            registry.space(reservation.space_uid).unwrap().lifecycle,
            Lifecycle::Aborted
        );
        assert_eq!(
            registry.operation(reservation.operation_uid).unwrap().state,
            OperationState::Aborted
        );
        assert!(registry.unfinished_operations().unwrap().is_empty());
        // The damage that had no remedy: `name_conflict` forever. The
        // SpaceUid/SpaceNo stay spent — a §8.2 gap, not reuse.
        let replacement = registry
            .reserve_space("legacy", instance, Uuid::new_v4())
            .unwrap();
        assert_ne!(replacement.space_uid, reservation.space_uid);
        assert_ne!(replacement.space_no, reservation.space_no);
        // The detail is evidence, not a slogan: a tmux adopt stamps options
        // and renames nothing, and the release says exactly that. "Never
        // bound a native resource" would be false the moment the stamp
        // landed — and it is the same sentence that, on Wez, would be
        // covering for a workspace left under an opaque key.
        assert!(result.detail.contains("renames nothing"), "{result:?}");
        assert!(result.detail.contains("@dmux_"), "{result:?}");
    }

    /// A crashed Wez adopt, in the shape `adopt_wez` can leave: identity
    /// reserved, and — depending on where the SIGKILL fell — the source
    /// workspace already CAS-renamed to the reservation's opaque key.
    fn stranded_wez_adoption(
        env: &OperationEnv,
    ) -> (
        BackendInstanceUid,
        crate::registry::SpaceReservation,
        String,
    ) {
        let mut registry =
            Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
        let owner = registry.identity().unwrap().host_uid;
        let instance = registry
            .register_backend_instance(Backend::Wez, Some("/run/dmux/wez.sock"), None)
            .unwrap();
        let reservation = registry
            .reserve_space_kind("legacy", instance, Uuid::new_v4(), OperationKind::Adopt)
            .unwrap();
        let key = adoption_key(owner, reservation.space_uid);
        (instance, reservation, key)
    }

    fn wez_scope(epoch: ServerEpoch) -> InventoryScope {
        InventoryScope::managed(Backend::Wez, "/run/dmux/wez.sock", epoch)
    }

    fn wez_inventory(epoch: ServerEpoch, keys: &[&str]) -> InventoryOutcome {
        InventoryOutcome::Complete(NativeInventory {
            server_epoch: Some(epoch),
            rows: keys
                .iter()
                .map(|key| NativeSpaceRow {
                    native_token: (*key).to_string(),
                    native_name: (*key).to_string(),
                    groups: Vec::new(),
                    multi_window: false,
                })
                .collect(),
        })
    }

    /// The fork CAS verb, scripted. Records every compensation attempt so a
    /// test can prove the reverse rename was issued with the same
    /// `--if-workspace`/`--if-sole-window` guard `adopt_wez` uses.
    struct RestoreSpy {
        window: Option<u64>,
        outcome: ProviderResult<CasRenameOutcome>,
        calls: std::cell::RefCell<Vec<(u64, String, String, bool)>>,
    }

    impl RestoreSpy {
        fn renaming(window: u64) -> Self {
            Self {
                window: Some(window),
                outcome: Ok(CasRenameOutcome::Renamed),
                calls: std::cell::RefCell::new(Vec::new()),
            }
        }

        fn refusing(window: u64, outcome: CasRenameOutcome) -> Self {
            Self {
                window: Some(window),
                outcome: Ok(outcome),
                calls: std::cell::RefCell::new(Vec::new()),
            }
        }
    }

    impl WorkspaceRestore for RestoreSpy {
        fn sole_window_id(
            &self,
            _scope: &InventoryScope,
            native_token: &str,
        ) -> ProviderResult<u64> {
            self.window.ok_or_else(|| ProviderError::NotFound {
                native_ref: native_token.to_string(),
            })
        }

        fn cas_rename_workspace(
            &self,
            _scope: &InventoryScope,
            window_id: u64,
            expected_workspace: &str,
            new_workspace: &str,
            expect_sole_window: bool,
        ) -> ProviderResult<CasRenameOutcome> {
            self.calls.borrow_mut().push((
                window_id,
                expected_workspace.to_string(),
                new_workspace.to_string(),
                expect_sole_window,
            ));
            self.outcome.clone()
        }
    }

    /// The damage `adopt_wez`'s own failure path compensates for: a workspace
    /// wearing an opaque key for a Space that never existed. Reconciliation
    /// owes it the same reverse CAS before it closes the row.
    #[test]
    fn a_crashed_wez_adoption_puts_the_workspace_back_before_freeing_the_name() {
        let (_data, _locks, env) = gate_test_env();
        let epoch = ServerEpoch(Uuid::from_u128(96));
        let (_instance, reservation, key) = stranded_wez_adoption(&env);

        // The CAS had landed: the workspace is listed under the opaque key.
        let provider = CreateGateProvider::new(Backend::Wez, wez_inventory(epoch, &[&key]));
        let scope = wez_scope(epoch);
        let restore = RestoreSpy::renaming(11);
        let targets = reconcile_scan(&env).unwrap();
        let result = reconcile_apply(
            &env,
            &targets[0],
            Some(ReconcileBackend::restorable(&provider, &scope, &restore)),
        );
        assert_eq!(
            result.outcome,
            ReconcileOutcome::ReservationReleased,
            "{result:?}"
        );
        assert!(result.detail.contains("put it back"), "{result:?}");
        assert_eq!(
            restore.calls.borrow().as_slice(),
            [(11, key.clone(), "legacy".to_string(), true)],
            "the compensation must be the same guarded reverse CAS adopt_wez performs"
        );

        let registry = Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
        assert_eq!(
            registry.space(reservation.space_uid).unwrap().lifecycle,
            Lifecycle::Aborted
        );
    }

    /// If the compensation cannot land, the workspace is still wearing the
    /// opaque key — releasing anyway would be the fabricated success §10.2
    /// forbids, so the row stays open and the pass is not `ok`.
    #[test]
    fn a_wez_adoption_whose_workspace_cannot_be_restored_is_never_reported_ok() {
        let (_data, _locks, env) = gate_test_env();
        let epoch = ServerEpoch(Uuid::from_u128(97));
        let (_instance, reservation, key) = stranded_wez_adoption(&env);

        let provider = CreateGateProvider::new(Backend::Wez, wez_inventory(epoch, &[&key]));
        let scope = wez_scope(epoch);
        let restore = RestoreSpy::refusing(
            11,
            CasRenameOutcome::WorkspaceMismatch {
                actual: "somebody-else".into(),
            },
        );
        let targets = reconcile_scan(&env).unwrap();
        let result = reconcile_apply(
            &env,
            &targets[0],
            Some(ReconcileBackend::restorable(&provider, &scope, &restore)),
        );
        assert_eq!(result.outcome, ReconcileOutcome::FailedClosed, "{result:?}");
        assert!(!result.ok);
        assert!(result.detail.contains(&key), "{result:?}");

        let registry = Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
        assert_eq!(
            registry.space(reservation.space_uid).unwrap().lifecycle,
            Lifecycle::Reserved
        );
        assert_eq!(
            registry.operation(reservation.operation_uid).unwrap().state,
            OperationState::Prepared
        );
    }

    /// Without the server there is no evidence either way, and "no binding"
    /// is not evidence: the rename lands before the binding does.
    #[test]
    fn a_wez_adoption_is_never_released_on_registry_evidence_alone() {
        let (_data, _locks, env) = gate_test_env();
        let (_instance, reservation, key) = stranded_wez_adoption(&env);

        let targets = reconcile_scan(&env).unwrap();
        let result = reconcile_apply(&env, &targets[0], None);
        assert_eq!(result.outcome, ReconcileOutcome::FailedClosed, "{result:?}");
        assert!(result.detail.contains(&key), "{result:?}");

        let registry = Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
        assert_eq!(
            registry.space(reservation.space_uid).unwrap().lifecycle,
            Lifecycle::Reserved
        );
    }

    /// The other half of the same evidence: a complete scan that shows no
    /// workspace under the key proves the rename never landed, and the
    /// reservation is released with nothing left renamed.
    #[test]
    fn a_wez_adoption_that_never_renamed_is_released_on_a_complete_scan() {
        let (_data, _locks, env) = gate_test_env();
        let epoch = ServerEpoch(Uuid::from_u128(98));
        let (_instance, reservation, _key) = stranded_wez_adoption(&env);

        let provider = CreateGateProvider::new(Backend::Wez, wez_inventory(epoch, &["unrelated"]));
        let scope = wez_scope(epoch);
        let restore = RestoreSpy::renaming(11);
        let targets = reconcile_scan(&env).unwrap();
        let result = reconcile_apply(
            &env,
            &targets[0],
            Some(ReconcileBackend::restorable(&provider, &scope, &restore)),
        );
        assert_eq!(
            result.outcome,
            ReconcileOutcome::ReservationReleased,
            "{result:?}"
        );
        assert!(restore.calls.borrow().is_empty(), "nothing to compensate");

        let registry = Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
        assert_eq!(
            registry.space(reservation.space_uid).unwrap().lifecycle,
            Lifecycle::Aborted
        );
    }

    /// Putting the name back must never merge the orphan into somebody
    /// else's live workspace of that name.
    #[test]
    fn a_wez_compensation_refuses_to_merge_into_a_live_workspace_of_that_name() {
        let (_data, _locks, env) = gate_test_env();
        let epoch = ServerEpoch(Uuid::from_u128(99));
        let (_instance, reservation, key) = stranded_wez_adoption(&env);

        let provider =
            CreateGateProvider::new(Backend::Wez, wez_inventory(epoch, &[&key, "legacy"]));
        let scope = wez_scope(epoch);
        let restore = RestoreSpy::renaming(11);
        let targets = reconcile_scan(&env).unwrap();
        let result = reconcile_apply(
            &env,
            &targets[0],
            Some(ReconcileBackend::restorable(&provider, &scope, &restore)),
        );
        assert_eq!(result.outcome, ReconcileOutcome::FailedClosed, "{result:?}");
        assert!(restore.calls.borrow().is_empty(), "{result:?}");

        let registry = Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
        assert_eq!(
            registry.space(reservation.space_uid).unwrap().lifecycle,
            Lifecycle::Reserved
        );
    }

    /// `remove_space_inner` holds the authority gate, the backend-instance
    /// lock and the Space lock — and never a decision lock. A preview that
    /// only asks about decision locks calls that live remove `crashed`, then
    /// the apply skips it: preview and apply disagreeing about the one field
    /// the operator confirms against.
    #[test]
    fn a_live_remove_is_previewed_as_in_flight_not_as_crashed() {
        let (_data, _locks, env) = gate_test_env();
        let (instance, _reservation) = stranded_adoption(&env);

        let mut holder = OrderedLocks::new(&env.lock_dir);
        holder
            .acquire(LockScope::AuthorityGate, LockMode::Shared)
            .unwrap();
        holder
            .acquire(LockScope::BackendInstance(instance), LockMode::Exclusive)
            .unwrap();

        let targets = reconcile_scan(&env).unwrap();
        assert!(targets[0].in_flight, "{targets:?}");
        let result = reconcile_apply(&env, &targets[0], None);
        assert_eq!(
            result.outcome,
            ReconcileOutcome::SkippedInFlight,
            "{result:?}"
        );
        drop(holder);
    }

    #[test]
    fn reconciling_twice_neither_double_aborts_nor_resurrects() {
        let (_data, _locks, env) = gate_test_env();
        let (_instance, reservation) = stranded_adoption(&env);
        let targets = reconcile_scan(&env).unwrap();
        assert!(reconcile_apply(&env, &targets[0], None).ok);

        let registry = Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
        let settled = registry.authority_head().unwrap().revision;
        drop(registry);

        // A second pass sees nothing at all; replaying the stale preview row
        // reports the row as already resolved rather than aborting it again.
        assert!(reconcile_scan(&env).unwrap().is_empty());
        let again = reconcile_apply(&env, &targets[0], None);
        assert_eq!(
            again.outcome,
            ReconcileOutcome::AlreadyResolved,
            "{again:?}"
        );
        assert!(again.ok);

        let registry = Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
        assert_eq!(registry.authority_head().unwrap().revision, settled);
        assert_eq!(
            registry.space(reservation.space_uid).unwrap().lifecycle,
            Lifecycle::Aborted
        );
    }

    #[test]
    fn a_row_a_live_holder_still_owns_is_listed_and_left_alone() {
        let (_data, _locks, env) = gate_test_env();
        let (_instance, reservation) = stranded_adoption(&env);
        let registry = Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
        let owner = registry.identity().unwrap().host_uid;
        drop(registry);

        // Stand in for the still-running holder: hold exactly the §10.1
        // locks a live adopt holds for the whole of its call.
        let mut holder = OrderedLocks::new(&env.lock_dir);
        holder
            .acquire(LockScope::AuthorityGate, LockMode::Shared)
            .unwrap();
        holder
            .acquire_decisions(owner, &["legacy"], LockMode::Exclusive)
            .unwrap();

        let targets = reconcile_scan(&env).unwrap();
        assert!(targets[0].in_flight, "{targets:?}");
        let result = reconcile_apply(&env, &targets[0], None);
        assert_eq!(
            result.outcome,
            ReconcileOutcome::SkippedInFlight,
            "{result:?}"
        );
        assert!(!result.ok);

        let registry = Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
        assert_eq!(
            registry.space(reservation.space_uid).unwrap().lifecycle,
            Lifecycle::Reserved
        );
        assert_eq!(
            registry.operation(reservation.operation_uid).unwrap().state,
            OperationState::Prepared
        );
        drop(holder);
    }

    #[test]
    fn a_crashed_create_reservation_is_freed_only_by_a_complete_zero_match_scan() {
        let (_data, _locks, env) = gate_test_env();
        let epoch = ServerEpoch(Uuid::from_u128(91));
        let mut registry =
            Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
        let instance = registry
            .register_backend_instance(Backend::Tmux, Some("tmux-recon"), None)
            .unwrap();
        let reservation = registry
            .reserve_space("ghost", instance, Uuid::new_v4())
            .unwrap();
        drop(registry);

        let targets = reconcile_scan(&env).unwrap();
        assert_eq!(targets[0].duty, "create_keyed_lookup");

        // No provider: the keyed lookup §10.2 demands is unobtainable, so
        // the table's Indeterminate arm fails closed and the row survives.
        let blind = reconcile_apply(&env, &targets[0], None);
        assert_eq!(blind.outcome, ReconcileOutcome::FailedClosed, "{blind:?}");
        let registry = Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
        assert_eq!(
            registry.space(reservation.space_uid).unwrap().lifecycle,
            Lifecycle::Reserved
        );
        drop(registry);

        let provider = CreateGateProvider::new(Backend::Tmux, empty_inventory(epoch));
        let scope = tmux_scope(epoch);
        let freed = reconcile_apply(
            &env,
            &targets[0],
            Some(ReconcileBackend::scan_only(&provider, &scope)),
        );
        assert_eq!(
            freed.outcome,
            ReconcileOutcome::ReservationReleased,
            "{freed:?}"
        );
        // Repair releases; it never spawns a replacement.
        assert_eq!(provider.creates.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_crashed_create_whose_orphan_still_exists_is_never_silently_released() {
        let (_data, _locks, env) = gate_test_env();
        let epoch = ServerEpoch(Uuid::from_u128(92));
        let mut registry =
            Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
        let instance = registry
            .register_backend_instance(Backend::Tmux, Some("tmux-recon"), None)
            .unwrap();
        let reservation = registry
            .reserve_space("ghost", instance, Uuid::new_v4())
            .unwrap();
        drop(registry);

        let provider =
            CreateGateProvider::new(Backend::Tmux, tmux_inventory(epoch, &["ghost", "other"]));
        let scope = tmux_scope(epoch);
        let targets = reconcile_scan(&env).unwrap();
        let result = reconcile_apply(
            &env,
            &targets[0],
            Some(ReconcileBackend::scan_only(&provider, &scope)),
        );
        assert_eq!(result.outcome, ReconcileOutcome::FailedClosed, "{result:?}");
        assert!(result.detail.contains("reserved key"), "{result:?}");
        // Failing closed is right; dead-ending is not. Before this, the name
        // was refused by `new`, `rm`, `rename` and `adopt` alike with no verb
        // able to free it and no remedy named — the refusal has to point at
        // §10.3's own answer and at the route that works before it exists.
        assert!(result.detail.contains("repair rebind"), "{result:?}");
        assert!(
            result
                .detail
                .contains("tmux -L tmux-recon rename-session -t ghost ghost.orphan"),
            "{result:?}"
        );
        assert!(result.detail.contains("repair reconcile"), "{result:?}");
        assert!(result.detail.contains("dmux adopt"), "{result:?}");

        let registry = Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
        assert_eq!(
            registry.space(reservation.space_uid).unwrap().lifecycle,
            Lifecycle::Reserved
        );
    }

    /// The route the refusal above names has to actually work: move the
    /// orphan off the reserved key and the very same row becomes the
    /// zero-match case, which releases the name. "Fails closed" must mean
    /// "not yet", never "burned forever".
    #[test]
    fn the_remedy_the_create_refusal_names_actually_frees_the_name() {
        let (_data, _locks, env) = gate_test_env();
        let epoch = ServerEpoch(Uuid::from_u128(90));
        let mut registry =
            Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
        let instance = registry
            .register_backend_instance(Backend::Tmux, Some("tmux-recon"), None)
            .unwrap();
        let reservation = registry
            .reserve_space("ghost", instance, Uuid::new_v4())
            .unwrap();
        drop(registry);

        let scope = tmux_scope(epoch);
        let targets = reconcile_scan(&env).unwrap();
        let refused = reconcile_apply(
            &env,
            &targets[0],
            Some(ReconcileBackend::scan_only(
                &CreateGateProvider::new(Backend::Tmux, tmux_inventory(epoch, &["ghost"])),
                &scope,
            )),
        );
        assert_eq!(
            refused.outcome,
            ReconcileOutcome::FailedClosed,
            "{refused:?}"
        );

        // The operator renames the orphan out of the way, exactly as the
        // refusal spelled it, and re-runs the verb.
        let renamed =
            CreateGateProvider::new(Backend::Tmux, tmux_inventory(epoch, &["ghost.orphan"]));
        let targets = reconcile_scan(&env).unwrap();
        let freed = reconcile_apply(
            &env,
            &targets[0],
            Some(ReconcileBackend::scan_only(&renamed, &scope)),
        );
        assert_eq!(
            freed.outcome,
            ReconcileOutcome::ReservationReleased,
            "{freed:?}"
        );
        assert_eq!(renamed.creates.load(Ordering::SeqCst), 0);

        let mut registry =
            Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
        assert_eq!(
            registry.space(reservation.space_uid).unwrap().lifecycle,
            Lifecycle::Aborted
        );
        // The name is usable again — by `new`, and (the orphan being
        // unmanaged and intact) by `dmux adopt`.
        registry
            .reserve_space("ghost", instance, Uuid::new_v4())
            .unwrap();
    }

    /// An active tmux Space with a current binding, plus a `prepared` rename
    /// row: the shape a crash between `begin_rename` and `commit_rename`
    /// leaves.
    fn stranded_rename(env: &OperationEnv, epoch: ServerEpoch) -> (SpaceUid, Uuid) {
        let mut registry =
            Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
        let instance = registry
            .register_backend_instance(Backend::Tmux, Some("tmux-recon"), None)
            .unwrap();
        let reservation = registry
            .reserve_space("before", instance, Uuid::new_v4())
            .unwrap();
        registry
            .finalize_create(
                reservation.space_uid,
                reservation.operation_uid,
                &NativeBindingSpec {
                    native_token: "$before".into(),
                    native_kind: NativeKind::TmuxSessionId,
                    server_epoch: Some(epoch),
                },
            )
            .unwrap();
        let operation_uid = registry
            .begin_rename(reservation.space_uid, "after", Uuid::new_v4())
            .unwrap();
        (reservation.space_uid, operation_uid)
    }

    #[test]
    fn a_crashed_rename_rolls_back_when_the_native_step_never_ran() {
        let (_data, _locks, env) = gate_test_env();
        let epoch = ServerEpoch(Uuid::from_u128(93));
        let (space_uid, operation_uid) = stranded_rename(&env, epoch);

        let provider = CreateGateProvider::new(Backend::Tmux, tmux_inventory(epoch, &["before"]));
        let scope = tmux_scope(epoch);
        let targets = reconcile_scan(&env).unwrap();
        assert_eq!(targets[0].duty, "rename_observe_states");
        let result = reconcile_apply(
            &env,
            &targets[0],
            Some(ReconcileBackend::scan_only(&provider, &scope)),
        );
        assert_eq!(
            result.outcome,
            ReconcileOutcome::RenameRolledBack,
            "{result:?}"
        );

        let registry = Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
        assert_eq!(registry.space(space_uid).unwrap().logical_name, "before");
        assert_eq!(
            registry.operation(operation_uid).unwrap().state,
            OperationState::Aborted
        );
    }

    #[test]
    fn a_crashed_rename_commits_when_the_native_step_had_landed() {
        let (_data, _locks, env) = gate_test_env();
        let epoch = ServerEpoch(Uuid::from_u128(94));
        let (space_uid, operation_uid) = stranded_rename(&env, epoch);

        let provider = CreateGateProvider::new(Backend::Tmux, tmux_inventory(epoch, &["after"]));
        let scope = tmux_scope(epoch);
        let targets = reconcile_scan(&env).unwrap();
        let result = reconcile_apply(
            &env,
            &targets[0],
            Some(ReconcileBackend::scan_only(&provider, &scope)),
        );
        assert_eq!(
            result.outcome,
            ReconcileOutcome::RenameCommitted,
            "{result:?}"
        );

        let registry = Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
        assert_eq!(registry.space(space_uid).unwrap().logical_name, "after");
        assert_eq!(
            registry.operation(operation_uid).unwrap().state,
            OperationState::Completed
        );
    }

    #[test]
    fn a_crashed_rename_with_both_names_live_refuses_to_choose() {
        let (_data, _locks, env) = gate_test_env();
        let epoch = ServerEpoch(Uuid::from_u128(95));
        let (space_uid, operation_uid) = stranded_rename(&env, epoch);

        // Somebody else created `after` while we were dead: committing would
        // silently claim their session (plan §10.2).
        let provider =
            CreateGateProvider::new(Backend::Tmux, tmux_inventory(epoch, &["before", "after"]));
        let scope = tmux_scope(epoch);
        let targets = reconcile_scan(&env).unwrap();
        let result = reconcile_apply(
            &env,
            &targets[0],
            Some(ReconcileBackend::scan_only(&provider, &scope)),
        );
        assert_eq!(result.outcome, ReconcileOutcome::FailedClosed, "{result:?}");

        let registry = Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap();
        assert_eq!(registry.space(space_uid).unwrap().logical_name, "before");
        assert_eq!(
            registry.operation(operation_uid).unwrap().state,
            OperationState::Prepared
        );
    }

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

    #[test]
    fn final_wez_empty_floor_requires_complete_same_epoch_zero_rows() {
        let epoch = ServerEpoch(Uuid::from_u128(41));
        let empty = InventoryOutcome::Complete(crate::backend::NativeInventory {
            server_epoch: Some(epoch),
            rows: Vec::new(),
        });
        assert_eq!(
            classify_final_wez_empty_scan(Some(epoch), empty).unwrap(),
            Some(epoch)
        );

        let nonempty = InventoryOutcome::Complete(crate::backend::NativeInventory {
            server_epoch: Some(epoch),
            rows: vec![crate::backend::NativeSpaceRow {
                native_token: "dmux:still-live".into(),
                native_name: "dmux:still-live".into(),
                groups: Vec::new(),
                multi_window: false,
            }],
        });
        assert_eq!(
            classify_final_wez_empty_scan(Some(epoch), nonempty).unwrap(),
            None
        );

        let changed = InventoryOutcome::Complete(crate::backend::NativeInventory {
            server_epoch: Some(ServerEpoch(Uuid::from_u128(42))),
            rows: Vec::new(),
        });
        assert!(classify_final_wez_empty_scan(Some(epoch), changed).is_err());
        assert!(
            classify_final_wez_empty_scan(
                Some(epoch),
                InventoryOutcome::Unreachable {
                    detail: "fault".into(),
                },
            )
            .is_err()
        );
    }

    #[test]
    fn a_durable_old_epoch_recovery_blocks_after_restart_and_process_lock_drop() {
        use std::time::Duration;

        use crate::locks::{LockMode, LockScope, acquire};
        use crate::registry::recovery::RecoveryGenerationSpec;
        use crate::registry::{LeaseHolder, LeaseScope};

        let data = tempfile::tempdir().unwrap();
        let lock_dir = tempfile::tempdir().unwrap();
        let mut registry = Registry::open(RegistryConfig::new(
            data.path().join("registry.sqlite3"),
            lock_dir.path(),
        ))
        .unwrap();
        let instance = registry
            .register_backend_instance(Backend::Wez, Some("/tmp/recovery.sock"), Some("test"))
            .unwrap();
        let epoch = ServerEpoch(Uuid::new_v4());
        registry
            .publish_backend_server(instance, epoch, Some(123), Some("start"), None, None)
            .unwrap();

        let kernel = acquire(
            lock_dir.path(),
            LockScope::BackendInstance(instance),
            LockMode::Exclusive,
        )
        .unwrap();
        let holder = LeaseHolder::current(Uuid::new_v4());
        let lease = registry
            .acquire_lease(
                &LeaseScope::Recovery(instance),
                &holder,
                Duration::from_secs(30),
                &kernel,
                None,
            )
            .unwrap();
        let generation = RecoveryGenerationSpec {
            generation_uid: Uuid::new_v4(),
            backend_instance: instance,
            server_epoch: epoch,
            manifest_id: "sha256:unfinished".into(),
        };
        registry.begin_recovery(&generation, &[], &lease).unwrap();
        drop(kernel); // coordinator crash: process fence is gone, journal is not.

        // A backend restart must not hide the durable generation merely
        // because the newly published incarnation has a different epoch.
        let restarted_epoch = ServerEpoch(Uuid::new_v4());
        registry
            .publish_backend_server(
                instance,
                restarted_epoch,
                Some(456),
                Some("restart"),
                None,
                None,
            )
            .unwrap();
        assert_ne!(restarted_epoch, generation.server_epoch);

        let error = require_no_unfinished_recovery(&registry, instance).unwrap_err();
        assert!(matches!(error, OpError::Refused(_)));
        assert!(
            error
                .to_string()
                .contains(&generation.generation_uid.to_string())
        );
        assert!(error.to_string().contains("pending"));
    }
}
