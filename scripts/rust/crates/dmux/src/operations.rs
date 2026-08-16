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
            inv.server_epoch.ok_or_else(|| {
                OpError::Indeterminate("managed create requires an epoched server".into())
            })?
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
        Backend::Wez => format!("dmux:{}:{}", owner.0, reservation.space_uid.0),
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
            if let Some(expected) = target.scope.expected_epoch
                && expected != epoch
            {
                return Err(OpError::Indeterminate(format!(
                    "{} scan changed epoch: expected {} observed {}",
                    target.backend, expected.0, epoch.0
                )));
            }
            Ok(Some(epoch))
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

    // Read both providers before classifying either result.  A determinate
    // stopped opposite server is empty; the selected server must be a
    // complete, epoched inventory because it is about to mutate.
    let selected_scan = selected.provider.inventory(selected.scope);
    let opposite_scan = opposite.map(|target| target.provider.inventory(target.scope));
    let selected_epoch = scan_epoch_for_create(selected, &selected_scan, true)?
        .expect("a selected complete scan has an epoch");
    if let (Some(target), Some(scan)) = (opposite, opposite_scan.as_ref()) {
        scan_epoch_for_create(target, scan, false)?;
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
    let binding = registry.current_binding(space_uid).map_err(reg_err)?;
    if let Some(binding) = binding {
        let native = crate::backend::NativeBinding {
            native_token: binding.native_token,
            server_epoch: scope.expected_epoch.ok_or_else(|| {
                OpError::Indeterminate("remove requires the current epoch".into())
            })?,
            root_group: ProviderHandle::Tx(0),
            root_split: ProviderHandle::Tx(0),
        };
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

    // P10 intentional-empty guard (§15.3).  The tombstone must be durable
    // before its revision can become the recovery floor, but we first prove
    // emptiness while the exact backend-instance lock still excludes every
    // create/restore/snapshot writer.  If this is the final durable Wez
    // Space, an indeterminate scan is not a successful remove: leaving the
    // journal in `deleting` is safer than allowing a later cold start to
    // resurrect a manifest we could not fence below.
    let final_wez_empty_epoch = if backend == Backend::Wez {
        let final_durable_space = !registry.spaces().map_err(reg_err)?.iter().any(|row| {
            row.backend_instance == instance
                && row.space_uid != space_uid
                && row.lifecycle == crate::model::Lifecycle::Active
        });
        if final_durable_space {
            classify_final_wez_empty_scan(scope.expected_epoch, provider.inventory(scope))?
        } else {
            None
        }
    } else {
        None
    };

    registry
        .complete_remove(space_uid, operation_uid)
        .map_err(reg_err)?;

    if let Some(epoch) = final_wez_empty_epoch {
        let backend_scope = LockScope::BackendInstance(instance);
        let kernel = locks.held(&backend_scope).ok_or_else(|| {
            OpError::Lock("intentional-empty update lost its backend-instance lock".into())
        })?;
        registry
            .record_current_intentional_empty_revision(instance, epoch, kernel)
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
    require_no_unfinished_recovery(&registry, instance)?;

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
    require_no_unfinished_recovery(&registry, instance)?;

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

    let (epoch, native_row) = scan_space_row(provider, scope, &binding.native_token)?;
    if let Some(expected) = scope.expected_epoch
        && expected != epoch
    {
        return Err(OpError::StaleRef(format!(
            "expected epoch {} but the live server is {}",
            expected.0, epoch.0
        )));
    }
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
        server_epoch: epoch,
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

    let (epoch, native_row) = scan_space_row(provider, scope, &binding.native_token)?;
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
    let (epoch, native_row) = scan_space_row(provider, scope, &binding.native_token)?;
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
    let (epoch, native_row) = scan_space_row(provider, scope, &binding.native_token)?;
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
    let (epoch, native_row) = scan_space_row(provider, scope, &binding.native_token)?;
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

fn require_exact_action_epoch(
    scope: &InventoryScope,
    live_epoch: ServerEpoch,
) -> Result<(), OpError> {
    match scope.expected_epoch {
        Some(expected) if expected == live_epoch => Ok(()),
        Some(expected) => Err(OpError::StaleRef(format!(
            "action expected server epoch {} but the live server is {}",
            expected.0, live_epoch.0
        ))),
        None => Err(OpError::Indeterminate(
            "exact logical child actions require a pinned server epoch".into(),
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
    let (epoch, native) = scan_space_row(provider, scope, &binding.native_token)?;
    require_exact_action_epoch(scope, epoch)?;
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
    let (epoch, native) = scan_space_row(provider, scope, &binding.native_token)?;
    require_exact_action_epoch(scope, epoch)?;
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
    let (epoch, native) = scan_space_row(provider, scope, &binding.native_token)?;
    require_exact_action_epoch(scope, epoch)?;
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
    let (epoch, native) = scan_space_row(provider, scope, &binding.native_token)?;
    require_exact_action_epoch(scope, epoch)?;
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
    let (epoch, native_row) = scan_space_row(provider, scope, &binding.native_token)?;
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

    let (live_epoch, native) = scan_space_row(provider, scope, &binding.native_token)?;
    if live_epoch != marker.server_epoch {
        return Err(OpError::StaleRef(format!(
            "marker epoch {} does not match live epoch {}",
            marker.server_epoch.0, live_epoch.0
        )));
    }
    let published = registry.backend_server(instance).map_err(reg_err)?;
    if published.server_epoch != Some(live_epoch) {
        return Err(OpError::Indeterminate(format!(
            "live epoch {} is not the registry-published backend incarnation",
            live_epoch.0
        )));
    }

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
    let (row, binding) = load_bound_space(&mut registry, space_uid)?;
    let (epoch, native_row) = scan_space_row(provider, scope, &binding.native_token)?;
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
    let (epoch, native_row) = scan_space_row(provider, scope, &binding.native_token)?;
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
    let instance = registry
        .register_backend_instance(Backend::Wez, Some(&scope.endpoint), None)
        .map_err(reg_err)?;
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
        let held = registry
            .reserve_space("collision", wez_instance, Uuid::new_v4())
            .unwrap();
        drop(registry);

        let tmux = CreateGateProvider::new(Backend::Tmux, empty_inventory(epoch));
        let wez = CreateGateProvider::new(Backend::Wez, empty_inventory(epoch));
        let tmux_scope = InventoryScope {
            backend: Backend::Tmux,
            endpoint: "tmux-gate".into(),
            expected_epoch: Some(epoch),
        };
        let wez_scope = InventoryScope {
            backend: Backend::Wez,
            endpoint: "/tmp/wez-gate.sock".into(),
            expected_epoch: Some(epoch),
        };
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
        let tmux_scope = InventoryScope {
            backend: Backend::Tmux,
            endpoint: "tmux-gate".into(),
            expected_epoch: Some(epoch),
        };
        let wez_scope = InventoryScope {
            backend: Backend::Wez,
            endpoint: "/tmp/wez-gate.sock".into(),
            expected_epoch: Some(epoch),
        };
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
        let tmux_scope = InventoryScope {
            backend: Backend::Tmux,
            endpoint: "tmux-gate".into(),
            expected_epoch: Some(epoch),
        };
        let wez_scope = InventoryScope {
            backend: Backend::Wez,
            endpoint: "/tmp/wez-gate.sock".into(),
            expected_epoch: Some(epoch),
        };
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
            drop(registry);
            let tmux = CreateGateProvider::new(Backend::Tmux, empty_inventory(epoch));
            let wez = CreateGateProvider::new(Backend::Wez, opposite_outcome);
            let tmux_scope = InventoryScope {
                backend: Backend::Tmux,
                endpoint: "tmux-gate".into(),
                expected_epoch: Some(epoch),
            };
            let wez_scope = InventoryScope {
                backend: Backend::Wez,
                endpoint: "/tmp/wez-gate.sock".into(),
                expected_epoch: Some(epoch),
            };
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
        registry
            .register_backend_instance(Backend::Wez, Some("/tmp/wez-gate.sock"), None)
            .unwrap();
        drop(registry);
        let tmux = CreateGateProvider::new(Backend::Tmux, empty_inventory(epoch));
        let scope = InventoryScope {
            backend: Backend::Tmux,
            endpoint: "tmux-gate".into(),
            expected_epoch: Some(epoch),
        };
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
