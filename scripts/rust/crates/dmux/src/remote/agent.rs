//! Owner-side `_agent` endpoint (plan §12.1): reads ONE JSON request
//! envelope on stdin, writes ONE response envelope on stdout, exits with
//! the mapped typed status. Never an interactive transport — remote tmux
//! presentation goes through the `_attach` single-use-token channel.
//!
//! Contract points implemented here (ADR 008 §3, ADR 009 §4):
//! - protocol v1 requires an EXACT version match (CLI flag and envelope);
//! - canonical payload bytes = `serde_json::to_string(&payload)` of the
//!   received payload value; `payload_sha256` must equal their sha256;
//! - mutations are idempotent by the ENVELOPE `request_uid` end to end
//!   (`new` through the `operations::create_space` ledger; `rename`/`rm`/
//!   `attach_plan` through the agent's own ledger rows) — UID reuse with
//!   different content is the typed `idempotency_reuse` error;
//! - every response carries the full authority identity fields, and errors
//!   are `TypedError`s inside the envelope, never bare stderr;
//! - the client's `backend` choice is product-level only (and only for
//!   `new`); namespaces, sockets, helper paths, and epochs are always
//!   owner-resolved, and children inherit their Space's backend. P8b: both
//!   backends are served when the owner has a verifiable instance — a
//!   backend the owner cannot serve is a typed refusal, never a fallback.
//!   `DMUX_WEZ_BIN`/`DMUX_WEZ_CONFIG`/`DMUX_HELPER_BIN` are owner-side
//!   test seams (like `DMUX_RUNTIME_DIR`), never client input.

use std::io::Read;
use std::path::PathBuf;

use rusqlite::OptionalExtension;
use serde_json::Value;
use uuid::Uuid;

use crate::backend::tmux::{TmuxProvider, TmuxServerIdentity};
use crate::backend::wez::WezProvider;
use crate::backend::{InventoryOutcome, InventoryScope, Provider, ProviderError, SplitDirection};
use crate::error::{ErrorCode, TypedError};
use crate::model::{
    Backend, BackendInstanceUid, ChildKind, HostUid, RegistryUid, ServerEpoch, SpaceUid,
};
use crate::operations::{
    self as ops, CreateRequest, OpError, OperationEnv, create_space, remove_space, rename_space,
};
use crate::refs::{ChildRefShape, parse_ref};
use crate::registry::{
    AttachTokenSpec, Registry, RegistryConfig, RegistryError, RpcDisposition, RpcResultState,
    SpaceRow, now_rfc3339, rfc3339_utc,
};
use crate::remote::protocol::{
    self, AttachPlan, AttachPlanPayload, BackendStatus, ChainLink, Envelope, GroupNewPayload,
    GroupRenamePayload, GroupRenameResult, GroupRmPayload, HelloInfo, HelloPayload,
    HierarchyPayload, NewPayload, PROTOCOL_VERSION, RenamePayload, RenameResult, RmPayload,
    RmResult, ScanSummary, SpaceInfo, SpacesInfo, SplitNewPayload, SplitRmPayload,
    canonical_payload_sha256,
};

/// Attach tokens are short-lived (plan §12.1 "short-lived attach plan").
const ATTACH_TOKEN_TTL_SECS: u64 = 60;

/// Arguments the hidden `dmux _agent` subcommand collects. `data_dir` and
/// `lock_dir` are the same test seams `_tmux-bootstrap` exposes; production
/// resolves the registry through `OperationEnv::production()`.
#[derive(Debug, Clone)]
pub struct AgentArgs {
    pub protocol: u32,
    pub method: String,
    pub data_dir: Option<PathBuf>,
    pub lock_dir: Option<PathBuf>,
}

/// Run the agent endpoint. Returns the process exit code; the response
/// envelope (payload or typed error) has already been written to stdout.
pub fn run(args: &AgentArgs) -> i32 {
    crate::remote::normalize_utf8_locale();
    let mut raw = String::new();
    let read = std::io::stdin().read_to_string(&mut raw);
    let (envelope, code) = match read {
        Ok(_) => serve(args, &raw),
        Err(e) => degraded(
            Uuid::nil(),
            &args.method,
            TypedError::new(ErrorCode::OperationFailed, format!("reading request: {e}")),
        ),
    };
    match serde_json::to_string(&envelope) {
        Ok(doc) => println!("{doc}"),
        Err(e) => println!(
            "{}",
            serde_json::json!({
                "protocol_version": PROTOCOL_VERSION,
                "error": { "code": "operation_failed",
                           "message": format!("serializing response: {e}") }
            })
        ),
    }
    code
}

/// Resolve the storage/lock seams exactly like `_tmux-bootstrap` does.
pub fn resolve_env(args: &AgentArgs) -> std::io::Result<OperationEnv> {
    match (&args.data_dir, &args.lock_dir) {
        (Some(data), Some(lock)) => Ok(OperationEnv {
            db_path: data.join("registry.sqlite3"),
            lock_dir: lock.clone(),
        }),
        _ => OperationEnv::production(),
    }
}

// ---------------------------------------------------------------------------
// Serving

struct AgentCx {
    env: OperationEnv,
    registry: Registry,
}

/// What one handler produced: the response payload plus the identity
/// qualifiers the envelope should carry for this method.
struct Reply {
    payload: Value,
    backend_instance: Option<BackendInstanceUid>,
    server_epoch: Option<ServerEpoch>,
}

impl Reply {
    fn plain(payload: Value) -> Reply {
        Reply {
            payload,
            backend_instance: None,
            server_epoch: None,
        }
    }
}

fn serve(args: &AgentArgs, raw: &str) -> (Envelope, i32) {
    // Identity first: even a refused request answers with a full envelope.
    let env = match resolve_env(args) {
        Ok(env) => env,
        Err(e) => {
            return degraded(
                Uuid::nil(),
                &args.method,
                TypedError::new(ErrorCode::OperationFailed, format!("environment: {e}")),
            );
        }
    };
    let registry = match Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)) {
        Ok(registry) => registry,
        Err(e) => {
            return degraded(
                Uuid::nil(),
                &args.method,
                TypedError::new(e.error_code(), format!("registry: {e}")),
            );
        }
    };
    let mut cx = AgentCx { env, registry };

    let request: Envelope = match serde_json::from_str(raw.trim()) {
        Ok(envelope) => envelope,
        Err(e) => {
            let error = TypedError::new(
                ErrorCode::Usage,
                format!("request is not one JSON envelope: {e}"),
            );
            return respond(&mut cx, Uuid::nil(), &args.method, None, None, Err(error));
        }
    };
    let request_uid = request.request_uid;

    let outcome = validate(args, &request).and_then(|payload| dispatch(&mut cx, &request, payload));
    let (backend_instance, server_epoch, result) = match outcome {
        Ok(reply) => (
            reply.backend_instance,
            reply.server_epoch,
            Ok(reply.payload),
        ),
        Err(error) => (None, None, Err(error)),
    };
    respond(
        &mut cx,
        request_uid,
        &request.method,
        backend_instance,
        server_epoch,
        result,
    )
}

/// Protocol/envelope validation (plan §12.1). Order: exact protocol match,
/// method agreement, well-formedness, canonical payload digest.
fn validate(args: &AgentArgs, request: &Envelope) -> Result<Value, TypedError> {
    if args.protocol != PROTOCOL_VERSION || request.protocol_version != PROTOCOL_VERSION {
        return Err(TypedError::new(
            ErrorCode::ProtocolMismatch,
            format!(
                "agent speaks protocol {PROTOCOL_VERSION} exactly; caller sent \
                 --protocol {} / envelope {}",
                args.protocol, request.protocol_version
            ),
        ));
    }
    if request.method != args.method {
        return Err(TypedError::new(
            ErrorCode::Usage,
            format!(
                "envelope method {:?} does not match invoked method {:?}",
                request.method, args.method
            ),
        ));
    }
    if request.error.is_some() {
        return Err(TypedError::new(
            ErrorCode::Usage,
            "a request envelope must carry payload, not error",
        ));
    }
    let payload = request.payload.clone().ok_or_else(|| {
        TypedError::new(ErrorCode::Usage, "a request envelope must carry a payload")
    })?;
    let digest = canonical_payload_sha256(&payload);
    if digest != request.payload_sha256 {
        return Err(TypedError::new(
            ErrorCode::Usage,
            format!(
                "payload_sha256 mismatch: envelope claims {} but the canonical \
                 payload bytes hash to {digest}",
                request.payload_sha256
            ),
        ));
    }
    Ok(payload)
}

fn dispatch(cx: &mut AgentCx, request: &Envelope, payload: Value) -> Result<Reply, TypedError> {
    match request.method.as_str() {
        protocol::methods::HELLO => hello(cx, payload),
        protocol::methods::SPACES => spaces(cx),
        protocol::methods::NEW => new_space(cx, request, payload),
        protocol::methods::RENAME => rename(cx, request, payload),
        protocol::methods::RM => remove(cx, request, payload),
        protocol::methods::ATTACH_PLAN => attach_plan(cx, request, payload),
        protocol::methods::HIERARCHY => hierarchy_read(cx, request, payload),
        protocol::methods::GROUP_NEW => group_new(cx, request, payload),
        protocol::methods::GROUP_RENAME => group_rename(cx, request, payload),
        protocol::methods::GROUP_RM => group_rm(cx, request, payload),
        protocol::methods::SPLIT_NEW => split_new(cx, request, payload),
        protocol::methods::SPLIT_RM => split_rm(cx, request, payload),
        other => Err(TypedError::new(
            ErrorCode::Usage,
            format!("unknown agent method {other:?}"),
        )),
    }
}

// ---------------------------------------------------------------------------
// Response envelopes

/// Build the one response envelope, always carrying the full authority
/// identity fields, and map the exit status.
fn respond(
    cx: &mut AgentCx,
    request_uid: Uuid,
    method: &str,
    backend_instance: Option<BackendInstanceUid>,
    server_epoch: Option<ServerEpoch>,
    result: Result<Value, TypedError>,
) -> (Envelope, i32) {
    let identity = cx.registry.identity();
    let head = cx.registry.authority_head();
    let (host_uid, registry_uid) = match &identity {
        Ok(identity) => (identity.host_uid, identity.registry_uid),
        Err(_) => (HostUid(Uuid::nil()), RegistryUid(Uuid::nil())),
    };
    let (revision, head_hash) = match &head {
        Ok(head) => (head.revision, head.head_hash.clone()),
        Err(_) => (0, String::new()),
    };
    let (payload, error, code) = match result {
        Ok(payload) => (Some(payload), None, 0),
        Err(error) => {
            let code = i32::from(error.code.exit_status().code());
            (None, Some(error), code)
        }
    };
    // The digest field covers whichever single document half is present.
    let digest = match (&payload, &error) {
        (Some(payload), None) => canonical_payload_sha256(payload),
        (None, Some(error)) => canonical_payload_sha256(
            &serde_json::to_value(error).unwrap_or_else(|_| serde_json::json!({})),
        ),
        _ => unreachable!("exactly one of payload/error is set above"),
    };
    let envelope = Envelope {
        protocol_version: PROTOCOL_VERSION,
        request_uid,
        method: method.to_string(),
        payload_sha256: digest,
        host_uid,
        registry_uid,
        authority_revision: revision,
        authority_head_hash: head_hash,
        backend_instance_uid: backend_instance,
        server_epoch,
        capabilities: capabilities(&cx.registry),
        payload,
        error,
    };
    (envelope, code)
}

/// Emergency envelope when no registry identity is available at all.
fn degraded(request_uid: Uuid, method: &str, error: TypedError) -> (Envelope, i32) {
    let code = i32::from(error.code.exit_status().code());
    let digest = canonical_payload_sha256(
        &serde_json::to_value(&error).unwrap_or_else(|_| serde_json::json!({})),
    );
    (
        Envelope {
            protocol_version: PROTOCOL_VERSION,
            request_uid,
            method: method.to_string(),
            payload_sha256: digest,
            host_uid: HostUid(Uuid::nil()),
            registry_uid: RegistryUid(Uuid::nil()),
            authority_revision: 0,
            authority_head_hash: String::new(),
            backend_instance_uid: None,
            server_epoch: None,
            capabilities: Vec::new(),
            payload: None,
            error: Some(error),
        },
        code,
    )
}

fn capabilities(registry: &Registry) -> Vec<String> {
    let mut caps = vec![format!("proto:{PROTOCOL_VERSION}")];
    for backend in [Backend::Tmux, Backend::Wez] {
        if let Ok(Some(_)) = find_instance(registry, backend) {
            caps.push(backend.as_str().to_string());
        }
    }
    caps
}

// ---------------------------------------------------------------------------
// Error mapping

fn typed_registry(e: RegistryError) -> TypedError {
    TypedError::new(e.error_code(), e.to_string())
}

fn typed_provider(e: ProviderError) -> TypedError {
    match e {
        ProviderError::EpochChanged { expected, observed } => TypedError::new(
            ErrorCode::BackendEpochChanged,
            format!(
                "server epoch changed: expected {}, observed {:?}",
                expected.0,
                observed.map(|e| e.0)
            ),
        ),
        ProviderError::WrongInstance { detail } => {
            TypedError::new(ErrorCode::WrongBackendInstance, detail)
        }
        ProviderError::NotFound { native_ref } => TypedError::new(
            ErrorCode::SpaceAbsent,
            format!("native resource absent: {native_ref}"),
        ),
        ProviderError::MultiWindow {
            native_ref,
            window_count,
        } => TypedError::new(
            ErrorCode::RepairRequired,
            format!("{native_ref} spans {window_count} windows"),
        ),
        ProviderError::NativeFailure { detail } if detail.contains("no tmux server") => {
            TypedError::new(ErrorCode::ProviderUnavailable, detail)
        }
        ProviderError::NativeFailure { detail } => {
            TypedError::new(ErrorCode::OperationFailed, detail)
        }
        ProviderError::PostconditionFailed { detail } => {
            TypedError::new(ErrorCode::PostconditionFailed, detail)
        }
        ProviderError::Timeout { detail } => TypedError::new(ErrorCode::OperationFailed, detail),
    }
}

/// `operations::OpError` carries registry failures as display strings; the
/// stable substrings below come from `RegistryError`'s Display impl (the
/// typed source is one frozen crate away, so this mapping is pinned by the
/// local-agent tests).
fn typed_op(e: OpError) -> TypedError {
    match e {
        OpError::NameConflict(d) => TypedError::new(ErrorCode::NameConflict, d),
        OpError::Indeterminate(d) => TypedError::new(ErrorCode::ProviderUnavailable, d),
        OpError::Bootstrap(d) => {
            TypedError::new(ErrorCode::OperationFailed, format!("bootstrap: {d}"))
        }
        OpError::NotFound(d) => TypedError::new(ErrorCode::NotFound, d),
        OpError::Lock(d) => {
            TypedError::new(ErrorCode::OperationFailed, format!("kernel lock: {d}"))
        }
        OpError::Provider(d) => {
            TypedError::new(ErrorCode::OperationFailed, format!("provider: {d}"))
        }
        // Cascade/unstamped refusals (plan §7.2/§10.3): the caller must use
        // the parent-level remove or repair/stamp the Space first.
        OpError::Refused(d) => TypedError::new(ErrorCode::RepairRequired, d),
        // A stale epoch-qualified ref is exactly the invalidated-epoch
        // condition (plan §6.3): fail, never retarget.
        OpError::StaleRef(d) => TypedError::new(ErrorCode::BackendEpochChanged, d),
        OpError::Registry(d) => {
            let code = if d.contains("reused with different content") {
                ErrorCode::IdempotencyReuse
            } else if d.contains("occupied by a live Space") {
                ErrorCode::NameConflict
            } else if d.contains("unfinished operation already exists") {
                ErrorCode::OperationInProgress
            } else if d.contains("registry busy") {
                ErrorCode::RegistryBusy
            } else {
                ErrorCode::OperationFailed
            };
            TypedError::new(code, d)
        }
    }
}

fn parse_payload<T: serde::de::DeserializeOwned>(payload: Value) -> Result<T, TypedError> {
    serde_json::from_value(payload)
        .map_err(|e| TypedError::new(ErrorCode::Usage, format!("payload: {e}")))
}

// ---------------------------------------------------------------------------
// Backend-instance / epoch verification

/// Read-only lookup of the single managed instance for (owner, backend).
/// Deliberately NOT `register_backend_instance`: an RPC probe must never
/// create the instance row (a socketless row would shadow the namespace a
/// later `_tmux-bootstrap` records). Mutations refuse when nothing is
/// bootstrapped; nothing is ever allocated from the RPC path.
fn find_instance(
    registry: &Registry,
    backend: Backend,
) -> Result<Option<(BackendInstanceUid, Option<String>)>, TypedError> {
    let identity = registry.identity().map_err(typed_registry)?;
    let row = registry
        .raw_connection()
        .query_row(
            "SELECT backend_instance_uid, socket_path FROM backend_instances \
             WHERE owner_host_uid = ?1 AND backend = ?2",
            rusqlite::params![identity.host_uid.0.to_string(), backend.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .map_err(|e| TypedError::new(ErrorCode::OperationFailed, e.to_string()))?;
    row.map(|(uid, socket)| {
        Uuid::parse_str(&uid)
            .map(|uid| (BackendInstanceUid(uid), socket))
            .map_err(|e| TypedError::new(ErrorCode::OperationFailed, e.to_string()))
    })
    .transpose()
}

/// One live-verified backend target: the owner's managed instance, its
/// exact endpoint, and the verified current server epoch.
struct Target {
    backend: Backend,
    instance: BackendInstanceUid,
    endpoint: String,
    epoch: ServerEpoch,
}

fn scope_for(target: &Target) -> InventoryScope {
    InventoryScope {
        backend: target.backend,
        endpoint: target.endpoint.clone(),
        expected_epoch: Some(target.epoch),
    }
}

/// Wez binary/config resolution. `DMUX_WEZ_BIN`/`DMUX_WEZ_CONFIG` are
/// owner-side TEST seams (like `DMUX_RUNTIME_DIR`): checked before the
/// production paths so scratch stock mux servers can be driven; production
/// never sets them.
fn wez_paths() -> (String, String) {
    let (mut bin, mut config) = crate::runtime::production_wez_paths();
    if let Ok(v) = std::env::var("DMUX_WEZ_BIN")
        && !v.is_empty()
    {
        bin = v;
    }
    if let Ok(v) = std::env::var("DMUX_WEZ_CONFIG")
        && !v.is_empty()
    {
        config = v;
    }
    (bin, config)
}

/// Backend-instance/epoch verification dispatcher: every mutation resolves
/// its target through here BEFORE anything is created, and a stale claimed
/// instance/epoch is a typed refusal.
fn verified_target(
    registry: &Registry,
    backend: Backend,
    claimed_instance: Option<BackendInstanceUid>,
    claimed_epoch: Option<ServerEpoch>,
) -> Result<(Target, Box<dyn Provider>), TypedError> {
    match backend {
        Backend::Tmux => verified_tmux_target(registry, claimed_instance, claimed_epoch),
        Backend::Wez => verified_wez_target(registry, claimed_instance, claimed_epoch),
    }
}

/// The tmux half of the verification matrix: the managed instance must
/// exist with a recorded namespace and a published epoch; the LIVE server
/// incarnation (pid/start token) and epoch option must still match that
/// publication; and any instance/epoch the client claimed in its envelope
/// must equal the verified values.
fn verified_tmux_target(
    registry: &Registry,
    claimed_instance: Option<BackendInstanceUid>,
    claimed_epoch: Option<ServerEpoch>,
) -> Result<(Target, Box<dyn Provider>), TypedError> {
    let (instance, socket) = find_instance(registry, Backend::Tmux)?.ok_or_else(|| {
        TypedError::new(
            ErrorCode::ProviderUnavailable,
            "no managed tmux backend instance on this owner; bootstrap the server first",
        )
    })?;
    let namespace = socket.ok_or_else(|| {
        TypedError::new(
            ErrorCode::ProviderUnavailable,
            "managed tmux instance has no recorded -L namespace",
        )
    })?;
    if let Some(claimed) = claimed_instance
        && claimed != instance
    {
        return Err(TypedError::new(
            ErrorCode::WrongBackendInstance,
            format!(
                "claimed backend instance {} but this owner's tmux instance is {}",
                claimed.0, instance.0
            ),
        ));
    }
    let record = registry.backend_server(instance).map_err(typed_registry)?;
    let epoch = record.server_epoch.ok_or_else(|| {
        TypedError::new(
            ErrorCode::ProviderUnavailable,
            "tmux server has no published epoch; run dmux _tmux-bootstrap",
        )
    })?;
    let expected_identity = TmuxServerIdentity {
        pid: record
            .server_pid
            .and_then(|pid| u32::try_from(pid).ok())
            .ok_or_else(|| {
                TypedError::new(
                    ErrorCode::ProviderUnavailable,
                    "published tmux incarnation has no recorded pid",
                )
            })?,
        start_token: record.server_start_token.clone().ok_or_else(|| {
            TypedError::new(
                ErrorCode::ProviderUnavailable,
                "published tmux incarnation has no recorded start token",
            )
        })?,
    };
    let provider = TmuxProvider::new(namespace.clone());
    // LIVE re-probe: a restarted/replaced server refuses here.
    provider
        .verify_epoch(&namespace, epoch, &expected_identity)
        .map_err(typed_provider)?;
    if let Some(claimed) = claimed_epoch
        && claimed != epoch
    {
        return Err(TypedError::new(
            ErrorCode::BackendEpochChanged,
            format!(
                "claimed server epoch {} but the live verified epoch is {}",
                claimed.0, epoch.0
            ),
        ));
    }
    Ok((
        Target {
            backend: Backend::Tmux,
            instance,
            endpoint: namespace,
            epoch,
        },
        Box::new(provider),
    ))
}

/// The wez half of the verification matrix. The endpoint comes from the
/// registry's wez instance row, falling back to the runtime descriptor
/// (never from the client). The live epoch comes from a complete
/// sentinel-verified scan; a stale registry publication or a stale claimed
/// instance/epoch refuses.
fn verified_wez_target(
    registry: &Registry,
    claimed_instance: Option<BackendInstanceUid>,
    claimed_epoch: Option<ServerEpoch>,
) -> Result<(Target, Box<dyn Provider>), TypedError> {
    let (instance, socket) = find_instance(registry, Backend::Wez)?.ok_or_else(|| {
        TypedError::new(
            ErrorCode::ProviderUnavailable,
            "no managed wez backend instance on this owner",
        )
    })?;
    let endpoint = socket
        .or_else(|| {
            crate::runtime::read_wez_descriptor()
                .ok()
                .flatten()
                .map(|d| d.socket)
        })
        .ok_or_else(|| {
            TypedError::new(
                ErrorCode::ProviderUnavailable,
                "managed wez instance has no recorded socket and no runtime descriptor",
            )
        })?;
    if let Some(claimed) = claimed_instance
        && claimed != instance
    {
        return Err(TypedError::new(
            ErrorCode::WrongBackendInstance,
            format!(
                "claimed backend instance {} but this owner's wez instance is {}",
                claimed.0, instance.0
            ),
        ));
    }
    let (bin, config) = wez_paths();
    let provider = WezProvider::new(bin, config);
    let probe = InventoryScope {
        backend: Backend::Wez,
        endpoint: endpoint.clone(),
        expected_epoch: None,
    };
    let epoch = match provider.inventory(&probe) {
        InventoryOutcome::Complete(inv) => inv.server_epoch.ok_or_else(|| {
            TypedError::new(
                ErrorCode::ProviderUnavailable,
                "wez server presents no sentinel epoch",
            )
        })?,
        other => {
            return Err(TypedError::new(
                ErrorCode::ProviderUnavailable,
                format!("wez scan: {other:?}"),
            ));
        }
    };
    // A published incarnation that no longer matches the live sentinel is a
    // replaced server: refuse rather than serve stale identity.
    let record = registry.backend_server(instance).map_err(typed_registry)?;
    if let Some(published) = record.server_epoch
        && published != epoch
    {
        return Err(TypedError::new(
            ErrorCode::BackendEpochChanged,
            format!(
                "published wez epoch {} but the live sentinel epoch is {}",
                published.0, epoch.0
            ),
        ));
    }
    if let Some(claimed) = claimed_epoch
        && claimed != epoch
    {
        return Err(TypedError::new(
            ErrorCode::BackendEpochChanged,
            format!(
                "claimed server epoch {} but the live verified epoch is {}",
                claimed.0, epoch.0
            ),
        ));
    }
    Ok((
        Target {
            backend: Backend::Wez,
            instance,
            endpoint,
            epoch,
        },
        Box::new(provider),
    ))
}

/// Resolve a Space's OWN backend target (children inherit; the client never
/// chooses) and verify the envelope's instance/epoch claims against it.
fn space_target(
    cx: &mut AgentCx,
    space_uid: SpaceUid,
    request: &Envelope,
) -> Result<(SpaceRow, Target, Box<dyn Provider>), TypedError> {
    let space = cx.registry.space(space_uid).map_err(typed_registry)?;
    let info = cx
        .registry
        .backend_instance_info(space.backend_instance)
        .map_err(typed_registry)?;
    let (target, provider) = verified_target(
        &cx.registry,
        info.backend,
        request.backend_instance_uid,
        request.server_epoch,
    )?;
    if space.backend_instance != target.instance {
        return Err(TypedError::new(
            ErrorCode::WrongBackendInstance,
            "space belongs to a different backend instance",
        ));
    }
    Ok((space, target, provider))
}

/// The pane-bootstrap helper is installed beside dmux (ADR 009 §4).
/// `DMUX_HELPER_BIN` is an owner-side TEST seam (like `DMUX_RUNTIME_DIR`):
/// the wez mux server does not propagate server env into panes, so the wez
/// test leg shims the helper — clients never choose owner paths.
fn helper_bin() -> Result<String, TypedError> {
    if let Ok(shim) = std::env::var("DMUX_HELPER_BIN")
        && !shim.is_empty()
    {
        return Ok(shim);
    }
    let exe = std::env::current_exe()
        .map_err(|e| TypedError::new(ErrorCode::OperationFailed, format!("current_exe: {e}")))?;
    let sibling = exe.with_file_name("pane-bootstrap");
    if sibling.exists() {
        return Ok(sibling.display().to_string());
    }
    Ok("pane-bootstrap".to_string())
}

/// Parse a canonical §6.3 child suffix of the expected kind.
fn parse_child(suffix: &str, want: ChildKind) -> Result<ChildRefShape, TypedError> {
    let parsed = parse_ref(&format!("x/{suffix}")).map_err(|e| {
        TypedError::new(
            ErrorCode::InvalidRef,
            format!("child ref {suffix:?}: {e:?}"),
        )
    })?;
    let child = parsed.child.ok_or_else(|| {
        TypedError::new(
            ErrorCode::InvalidRef,
            format!("{suffix:?} is not a child ref"),
        )
    })?;
    if child.kind != want {
        return Err(TypedError::new(
            ErrorCode::InvalidRef,
            format!("{suffix:?} is a {:?} ref; expected {want:?}", child.kind),
        ));
    }
    Ok(child)
}

fn parse_direction(direction: Option<&str>) -> Result<SplitDirection, TypedError> {
    Ok(match direction {
        None => SplitDirection::Down,
        Some("left") => SplitDirection::Left,
        Some("right") => SplitDirection::Right,
        Some("up") => SplitDirection::Up,
        Some("down") => SplitDirection::Down,
        Some(other) => {
            return Err(TypedError::new(
                ErrorCode::Usage,
                format!("direction {other:?} is not one of left|right|up|down"),
            ));
        }
    })
}

fn check_percent(percent: Option<u8>) -> Result<(), TypedError> {
    if let Some(p) = percent
        && !(1..=99).contains(&p)
    {
        return Err(TypedError::new(
            ErrorCode::Usage,
            format!("percent {p} is outside 1..=99"),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Methods

fn hello(cx: &mut AgentCx, payload: Value) -> Result<Reply, TypedError> {
    let hello: HelloPayload = parse_payload(payload)?;
    let identity = cx.registry.identity().map_err(typed_registry)?;
    let head = cx.registry.authority_head().map_err(typed_registry)?;
    let chain = cx
        .registry
        .revision_chain()
        .map_err(typed_registry)?
        .into_iter()
        .map(|record| ChainLink {
            revision: record.revision,
            parent_head_hash: record.parent_head_hash,
            head_hash: record.head_hash,
            txn_uid: record.txn_uid,
        })
        .collect();
    let mut backends = Vec::new();
    for backend in [Backend::Tmux, Backend::Wez] {
        if let Some((instance, socket)) = find_instance(&cx.registry, backend)? {
            let record = cx
                .registry
                .backend_server(instance)
                .map_err(typed_registry)?;
            backends.push(BackendStatus {
                backend,
                backend_instance_uid: instance,
                server_epoch: record.server_epoch,
                socket_path: socket,
            });
        }
    }
    let info = HelloInfo {
        host_uid: identity.host_uid,
        registry_uid: identity.registry_uid,
        authority_revision: head.revision,
        authority_head_hash: head.head_hash,
        protocol_version: PROTOCOL_VERSION,
        agent_version: env!("CARGO_PKG_VERSION").to_string(),
        capabilities: capabilities(&cx.registry),
        backends,
        revision_chain: chain,
        nonce: hello.nonce,
    };
    Ok(Reply::plain(serde_json::to_value(info).map_err(|e| {
        TypedError::new(ErrorCode::OperationFailed, e.to_string())
    })?))
}

fn spaces(cx: &mut AgentCx) -> Result<Reply, TypedError> {
    let rows = cx.registry.spaces().map_err(typed_registry)?;
    let mut spaces = Vec::with_capacity(rows.len());
    for row in rows {
        let backend = cx
            .registry
            .backend_instance_info(row.backend_instance)
            .map_err(typed_registry)?
            .backend;
        let native_token = cx
            .registry
            .current_binding(row.space_uid)
            .map_err(typed_registry)?
            .map(|binding| binding.native_token);
        spaces.push(SpaceInfo {
            space_uid: row.space_uid,
            space_no: row.space_no.get(),
            name: row.logical_name,
            backend,
            backend_instance_uid: row.backend_instance,
            lifecycle: row.lifecycle,
            health: row.health,
            native_token,
        });
    }
    let mut scans = Vec::new();
    match find_instance(&cx.registry, Backend::Tmux)? {
        Some((_, Some(namespace))) => {
            let provider = TmuxProvider::new(namespace.clone());
            let scope = InventoryScope {
                backend: Backend::Tmux,
                endpoint: namespace,
                expected_epoch: None,
            };
            scans.push(scan_summary(Backend::Tmux, &provider.inventory(&scope)));
        }
        Some((_, None)) => scans.push(ScanSummary {
            backend: Backend::Tmux,
            outcome: "unavailable".into(),
            detail: Some("managed instance has no recorded namespace".into()),
            rows: None,
            server_epoch: None,
        }),
        None => {}
    }
    match find_instance(&cx.registry, Backend::Wez)? {
        Some((_, socket)) => {
            let endpoint = socket.or_else(|| {
                crate::runtime::read_wez_descriptor()
                    .ok()
                    .flatten()
                    .map(|d| d.socket)
            });
            match endpoint {
                Some(endpoint) => {
                    let (bin, config) = wez_paths();
                    let provider = WezProvider::new(bin, config);
                    let scope = InventoryScope {
                        backend: Backend::Wez,
                        endpoint,
                        expected_epoch: None,
                    };
                    scans.push(scan_summary(Backend::Wez, &provider.inventory(&scope)));
                }
                None => scans.push(ScanSummary {
                    backend: Backend::Wez,
                    outcome: "unavailable".into(),
                    detail: Some("no recorded socket and no runtime descriptor".into()),
                    rows: None,
                    server_epoch: None,
                }),
            }
        }
        None => {}
    }
    let info = SpacesInfo { spaces, scans };
    Ok(Reply::plain(serde_json::to_value(info).map_err(|e| {
        TypedError::new(ErrorCode::OperationFailed, e.to_string())
    })?))
}

fn scan_summary(backend: Backend, outcome: &InventoryOutcome) -> ScanSummary {
    let (token, detail, rows, epoch) = match outcome {
        InventoryOutcome::Complete(inventory) => (
            "complete",
            None,
            Some(inventory.rows.len() as u64),
            inventory.server_epoch,
        ),
        InventoryOutcome::ServerStopped { detail } => {
            ("server_stopped", Some(detail.clone()), None, None)
        }
        InventoryOutcome::Unreachable { detail } => {
            ("unreachable", Some(detail.clone()), None, None)
        }
        InventoryOutcome::AuthFailed { detail } => {
            ("auth_failed", Some(detail.clone()), None, None)
        }
        InventoryOutcome::HostKeyIdentityFailed { detail } => {
            ("host_key_identity_failed", Some(detail.clone()), None, None)
        }
        InventoryOutcome::CommandMissing { detail } => {
            ("command_missing", Some(detail.clone()), None, None)
        }
        InventoryOutcome::VersionMismatch { detail } => {
            ("version_mismatch", Some(detail.clone()), None, None)
        }
        InventoryOutcome::ProtocolMismatch { detail } => {
            ("protocol_mismatch", Some(detail.clone()), None, None)
        }
        InventoryOutcome::Malformed { detail } => ("malformed", Some(detail.clone()), None, None),
        InventoryOutcome::Timeout { detail } => ("timeout", Some(detail.clone()), None, None),
        InventoryOutcome::PermissionFailure { detail } => {
            ("permission_failure", Some(detail.clone()), None, None)
        }
    };
    ScanSummary {
        backend,
        outcome: token.to_string(),
        detail,
        rows,
        server_epoch: epoch,
    }
}

fn new_space(cx: &mut AgentCx, request: &Envelope, payload: Value) -> Result<Reply, TypedError> {
    let new: NewPayload = parse_payload(payload)?;
    // `backend` is the client's PRODUCT-level choice (ADR 009 §4); native
    // details are owner-resolved through the verification matrix. A backend
    // the owner cannot serve is a typed refusal, never a fallback.
    let (target, provider) = verified_target(
        &cx.registry,
        new.backend,
        request.backend_instance_uid,
        request.server_epoch,
    )?;
    let scope = scope_for(&target);
    // End-to-end idempotency: the ENVELOPE request UID is the operation
    // request UID; create_space owns the ledger row for method "new".
    let created = create_space(
        &cx.env,
        provider.as_ref(),
        &scope,
        target.backend,
        &CreateRequest {
            request_uid: request.request_uid,
            name: new.name,
            cwd: new.cwd,
            program: new.program,
            helper_bin: helper_bin()?,
        },
    )
    .map_err(typed_op)?;
    Ok(Reply {
        payload: serde_json::to_value(&created)
            .map_err(|e| TypedError::new(ErrorCode::OperationFailed, e.to_string()))?,
        backend_instance: Some(target.instance),
        server_epoch: Some(target.epoch),
    })
}

fn rename(cx: &mut AgentCx, request: &Envelope, payload: Value) -> Result<Reply, TypedError> {
    let rename: RenamePayload = parse_payload(payload)?;
    let disposition = cx
        .registry
        .record_rpc_request(
            request.request_uid,
            protocol::methods::RENAME,
            &request.payload_sha256,
        )
        .map_err(typed_registry)?;
    if let RpcDisposition::Replay {
        result_state: RpcResultState::Final,
        result_json: Some(stored),
    } = &disposition
    {
        let mut result: RenameResult = serde_json::from_value(stored.clone())
            .map_err(|e| TypedError::new(ErrorCode::OperationFailed, e.to_string()))?;
        result.replayed = true;
        return Ok(Reply::plain(serde_json::to_value(result).map_err(|e| {
            TypedError::new(ErrorCode::OperationFailed, e.to_string())
        })?));
    }
    let replay_unknown = matches!(
        disposition,
        RpcDisposition::Replay {
            result_state: RpcResultState::Unknown,
            ..
        }
    );

    let (space, target, provider) = space_target(cx, rename.space_uid, request)?;
    let scope = scope_for(&target);

    // Ack-loss / crash reconciliation (plan §12.1: a retry reconciles the
    // original mutation rather than repeating it blindly).
    let unfinished = cx
        .registry
        .unfinished_operation(rename.space_uid)
        .map_err(typed_registry)?;
    let mut replayed_result = false;
    match unfinished {
        Some(op)
            if op.kind == crate::model::OperationKind::Rename
                && op.request_uid == request.request_uid =>
        {
            // Resume: repeat the (idempotent) native rename where the
            // backend has one — wez logical renames are registry-only
            // (plan §2.5) — then commit.
            if target.backend == Backend::Tmux {
                let binding = cx
                    .registry
                    .current_binding(rename.space_uid)
                    .map_err(typed_registry)?
                    .ok_or_else(|| {
                        TypedError::new(ErrorCode::SpaceAbsent, "no current native binding")
                    })?;
                let native = crate::backend::NativeBinding {
                    native_token: binding.native_token,
                    server_epoch: target.epoch,
                    root_group: crate::model::ProviderHandle::Tx(0),
                    root_split: crate::model::ProviderHandle::Tx(0),
                };
                provider
                    .rename(&scope, &native, &rename.new_name)
                    .map_err(typed_provider)?;
            }
            cx.registry
                .commit_rename(rename.space_uid, op.operation_uid)
                .map_err(typed_registry)?;
        }
        _ if replay_unknown && space.logical_name == rename.new_name => {
            // The mutation completed; only the ledger finish was lost.
            replayed_result = true;
        }
        _ => {
            rename_space(
                &cx.env,
                provider.as_ref(),
                &scope,
                target.backend,
                rename.space_uid,
                &rename.new_name,
                request.request_uid,
            )
            .map_err(typed_op)?;
        }
    }
    let stored = RenameResult {
        space_uid: rename.space_uid,
        name: rename.new_name.clone(),
        replayed: false,
    };
    cx.registry
        .finish_rpc_request(
            request.request_uid,
            &serde_json::to_value(&stored)
                .map_err(|e| TypedError::new(ErrorCode::OperationFailed, e.to_string()))?,
            None,
        )
        .map_err(typed_registry)?;
    let result = RenameResult {
        replayed: replayed_result,
        ..stored
    };
    Ok(Reply {
        payload: serde_json::to_value(result)
            .map_err(|e| TypedError::new(ErrorCode::OperationFailed, e.to_string()))?,
        backend_instance: Some(target.instance),
        server_epoch: Some(target.epoch),
    })
}

fn remove(cx: &mut AgentCx, request: &Envelope, payload: Value) -> Result<Reply, TypedError> {
    let rm: RmPayload = parse_payload(payload)?;
    let disposition = cx
        .registry
        .record_rpc_request(
            request.request_uid,
            protocol::methods::RM,
            &request.payload_sha256,
        )
        .map_err(typed_registry)?;
    if let RpcDisposition::Replay {
        result_state: RpcResultState::Final,
        result_json: Some(stored),
    } = &disposition
    {
        let mut result: RmResult = serde_json::from_value(stored.clone())
            .map_err(|e| TypedError::new(ErrorCode::OperationFailed, e.to_string()))?;
        result.replayed = true;
        return Ok(Reply::plain(serde_json::to_value(result).map_err(|e| {
            TypedError::new(ErrorCode::OperationFailed, e.to_string())
        })?));
    }
    let is_replay = matches!(disposition, RpcDisposition::Replay { .. });

    // The Deleted/Aborted early paths must not require a live backend (a
    // replayed rm of a removed Space answers even if the server is gone).
    let space = cx.registry.space(rm.space_uid).map_err(typed_registry)?;
    let finish = |cx: &mut AgentCx, replayed: bool| -> Result<Reply, TypedError> {
        let stored = RmResult {
            space_uid: rm.space_uid,
            removed: true,
            replayed: false,
        };
        cx.registry
            .finish_rpc_request(
                request.request_uid,
                &serde_json::to_value(&stored)
                    .map_err(|e| TypedError::new(ErrorCode::OperationFailed, e.to_string()))?,
                None,
            )
            .map_err(typed_registry)?;
        let result = RmResult { replayed, ..stored };
        Ok(Reply::plain(serde_json::to_value(result).map_err(|e| {
            TypedError::new(ErrorCode::OperationFailed, e.to_string())
        })?))
    };

    match space.lifecycle {
        crate::model::Lifecycle::Deleted => {
            if is_replay {
                // Completed earlier; only the ledger finish was lost.
                return finish(cx, true);
            }
            return Err(TypedError::new(
                ErrorCode::SpaceDeleted,
                format!("space {} is already deleted", rm.space_uid.0),
            ));
        }
        crate::model::Lifecycle::Aborted => {
            return Err(TypedError::new(
                ErrorCode::SpaceDeleted,
                format!("space {} was aborted", rm.space_uid.0),
            ));
        }
        _ => {}
    }

    let (space, target, provider) = space_target(cx, rm.space_uid, request)?;
    let scope = scope_for(&target);

    if space.lifecycle == crate::model::Lifecycle::Deleting {
        // Crash between `deleting` intent and the tombstone: resume only
        // the SAME request; anything else is a foreign unfinished remove.
        let unfinished = cx
            .registry
            .unfinished_operation(rm.space_uid)
            .map_err(typed_registry)?;
        match unfinished {
            Some(op)
                if op.kind == crate::model::OperationKind::Remove
                    && op.request_uid == request.request_uid =>
            {
                if let Some(binding) = cx
                    .registry
                    .current_binding(rm.space_uid)
                    .map_err(typed_registry)?
                {
                    let native = crate::backend::NativeBinding {
                        native_token: binding.native_token,
                        server_epoch: target.epoch,
                        root_group: crate::model::ProviderHandle::Tx(0),
                        root_split: crate::model::ProviderHandle::Tx(0),
                    };
                    match provider.remove(&scope, &native) {
                        Ok(()) => {}
                        Err(ProviderError::NotFound { .. }) => {}
                        Err(e) => return Err(typed_provider(e)),
                    }
                }
                cx.registry
                    .complete_remove(rm.space_uid, op.operation_uid)
                    .map_err(typed_registry)?;
                return finish(cx, false);
            }
            _ => {
                return Err(TypedError::new(
                    ErrorCode::OperationInProgress,
                    "another unfinished remove owns this space",
                ));
            }
        }
    }

    remove_space(
        &cx.env,
        provider.as_ref(),
        &scope,
        target.backend,
        rm.space_uid,
        request.request_uid,
    )
    .map_err(typed_op)?;
    let reply = finish(cx, false)?;
    Ok(Reply {
        backend_instance: Some(target.instance),
        server_epoch: Some(target.epoch),
        ..reply
    })
}

fn attach_plan(cx: &mut AgentCx, request: &Envelope, payload: Value) -> Result<Reply, TypedError> {
    let plan_req: AttachPlanPayload = parse_payload(payload)?;
    let disposition = cx
        .registry
        .record_rpc_request(
            request.request_uid,
            protocol::methods::ATTACH_PLAN,
            &request.payload_sha256,
        )
        .map_err(typed_registry)?;
    if let RpcDisposition::Replay {
        result_state: RpcResultState::Final,
        result_json: Some(stored),
    } = &disposition
    {
        // The raw token was returned exactly once and only its sha256 was
        // persisted: the replayed plan carries an empty token.
        let mut plan: AttachPlan = serde_json::from_value(stored.clone())
            .map_err(|e| TypedError::new(ErrorCode::OperationFailed, e.to_string()))?;
        plan.replayed = true;
        return Ok(Reply::plain(serde_json::to_value(plan).map_err(|e| {
            TypedError::new(ErrorCode::OperationFailed, e.to_string())
        })?));
    }

    let identity = cx.registry.identity().map_err(typed_registry)?;
    // The PTY channel is tmux-only (plan §12.1): wez presentation is GUI
    // domain attachment and never uses `_attach`.
    let space = cx
        .registry
        .space(plan_req.space_uid)
        .map_err(typed_registry)?;
    let info = cx
        .registry
        .backend_instance_info(space.backend_instance)
        .map_err(typed_registry)?;
    if info.backend != Backend::Tmux {
        return Err(TypedError::new(
            ErrorCode::ProviderUnavailable,
            "attach_plan serves tmux Spaces only; wez presentation is GUI \
             domain attachment (plan §12.1)",
        ));
    }
    match space.lifecycle {
        crate::model::Lifecycle::Active => {}
        crate::model::Lifecycle::Deleted | crate::model::Lifecycle::Aborted => {
            return Err(TypedError::new(
                ErrorCode::SpaceDeleted,
                format!("space {} is deleted", plan_req.space_uid.0),
            ));
        }
        other => {
            return Err(TypedError::new(
                ErrorCode::OperationInProgress,
                format!("space is {other:?}, not active"),
            ));
        }
    }
    let binding = cx
        .registry
        .current_binding(plan_req.space_uid)
        .map_err(typed_registry)?
        .ok_or_else(|| {
            TypedError::new(
                ErrorCode::SpaceAbsent,
                "space has no current native binding",
            )
        })?;
    let (target, provider) = verified_tmux_target(
        &cx.registry,
        request.backend_instance_uid,
        request.server_epoch,
    )?;
    if space.backend_instance != target.instance {
        return Err(TypedError::new(
            ErrorCode::WrongBackendInstance,
            "space belongs to a different backend instance",
        ));
    }
    // The binding must still be live on the verified server.
    let scope = scope_for(&target);
    let native = crate::backend::NativeBinding {
        native_token: binding.native_token.clone(),
        server_epoch: target.epoch,
        root_group: crate::model::ProviderHandle::Tx(0),
        root_split: crate::model::ProviderHandle::Tx(0),
    };
    provider.inspect(&scope, &native).map_err(typed_provider)?;

    // Mint the single-use token; only its sha256 is ever persisted.
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let token_hash = crate::registry::sha256::sha256_hex(token.as_bytes());
    let issued_at = now_rfc3339();
    let expires_at = rfc3339_utc(
        std::time::SystemTime::now() + std::time::Duration::from_secs(ATTACH_TOKEN_TTL_SECS),
    );
    let route = plan_req.route.unwrap_or_else(|| "local".to_string());
    let attach_argv = vec![
        "tmux".to_string(),
        "-L".to_string(),
        target.endpoint.clone(),
        "attach-session".to_string(),
        "-t".to_string(),
        binding.native_token.clone(),
    ];
    cx.registry
        .issue_attach_token(&AttachTokenSpec {
            token_hash,
            request_uid: request.request_uid,
            host_uid: identity.host_uid,
            space_uid: plan_req.space_uid,
            server_epoch: target.epoch,
            route: route.clone(),
            attach_argv,
            issued_at,
            expires_at: expires_at.clone(),
        })
        .map_err(typed_registry)?;
    let plan = AttachPlan {
        request_uid: request.request_uid,
        host_uid: identity.host_uid,
        space_uid: plan_req.space_uid,
        server_epoch: target.epoch,
        route,
        expires_at,
        token,
        replayed: false,
    };
    // Persist the token-FREE plan in the idempotency ledger.
    let mut stored = plan.clone();
    stored.token = String::new();
    cx.registry
        .finish_rpc_request(
            request.request_uid,
            &serde_json::to_value(&stored)
                .map_err(|e| TypedError::new(ErrorCode::OperationFailed, e.to_string()))?,
            None,
        )
        .map_err(typed_registry)?;
    Ok(Reply {
        payload: serde_json::to_value(&plan)
            .map_err(|e| TypedError::new(ErrorCode::OperationFailed, e.to_string()))?,
        backend_instance: Some(target.instance),
        server_epoch: Some(target.epoch),
    })
}

// ---------------------------------------------------------------------------
// P8b remote hierarchy methods. Each maps to the matching operations::*
// function; the ENVELOPE request UID is the operation request UID (the
// operations-layer child ledger owns replay — the agent never
// double-records), the Space's OWN backend instance resolves the provider
// (children inherit; the client never chooses), and every target passes
// the backend-instance/epoch verification matrix first.

fn to_reply<T: serde::Serialize>(target: &Target, value: &T) -> Result<Reply, TypedError> {
    Ok(Reply {
        payload: serde_json::to_value(value)
            .map_err(|e| TypedError::new(ErrorCode::OperationFailed, e.to_string()))?,
        backend_instance: Some(target.instance),
        server_epoch: Some(target.epoch),
    })
}

fn hierarchy_read(
    cx: &mut AgentCx,
    request: &Envelope,
    payload: Value,
) -> Result<Reply, TypedError> {
    let p: HierarchyPayload = parse_payload(payload)?;
    let (_space, target, provider) = space_target(cx, p.space_uid, request)?;
    let scope = scope_for(&target);
    let tree = ops::hierarchy(&cx.env, provider.as_ref(), &scope, p.space_uid).map_err(typed_op)?;
    to_reply(&target, &tree)
}

fn group_new(cx: &mut AgentCx, request: &Envelope, payload: Value) -> Result<Reply, TypedError> {
    let p: GroupNewPayload = parse_payload(payload)?;
    let (_space, target, provider) = space_target(cx, p.space_uid, request)?;
    let scope = scope_for(&target);
    let created = ops::group_new(
        &cx.env,
        provider.as_ref(),
        &scope,
        &ops::GroupNewRequest {
            request_uid: request.request_uid,
            space_uid: p.space_uid,
            cwd: p.cwd,
            program: p.program,
            helper_bin: helper_bin()?,
        },
    )
    .map_err(typed_op)?;
    to_reply(&target, &created)
}

fn split_new(cx: &mut AgentCx, request: &Envelope, payload: Value) -> Result<Reply, TypedError> {
    let p: SplitNewPayload = parse_payload(payload)?;
    let group = parse_child(&p.group_ref, ChildKind::Group)?;
    let direction = parse_direction(p.direction.as_deref())?;
    check_percent(p.percent)?;
    let (_space, target, provider) = space_target(cx, p.space_uid, request)?;
    let scope = scope_for(&target);
    let created = ops::split_new(
        &cx.env,
        provider.as_ref(),
        &scope,
        &ops::SplitNewRequest {
            request_uid: request.request_uid,
            space_uid: p.space_uid,
            group,
            direction,
            percent: p.percent,
            cwd: p.cwd,
            program: p.program,
            helper_bin: helper_bin()?,
        },
    )
    .map_err(typed_op)?;
    to_reply(&target, &created)
}

fn group_rename(cx: &mut AgentCx, request: &Envelope, payload: Value) -> Result<Reply, TypedError> {
    let p: GroupRenamePayload = parse_payload(payload)?;
    let group = parse_child(&p.group_ref, ChildKind::Group)?;
    let (_space, target, provider) = space_target(cx, p.space_uid, request)?;
    let scope = scope_for(&target);
    ops::group_rename(
        &cx.env,
        provider.as_ref(),
        &scope,
        p.space_uid,
        &group,
        &p.title,
        request.request_uid,
    )
    .map_err(typed_op)?;
    to_reply(
        &target,
        &GroupRenameResult {
            space_uid: p.space_uid,
            group_ref: p.group_ref,
            title: p.title,
        },
    )
}

fn group_rm(cx: &mut AgentCx, request: &Envelope, payload: Value) -> Result<Reply, TypedError> {
    let p: GroupRmPayload = parse_payload(payload)?;
    let group = parse_child(&p.group_ref, ChildKind::Group)?;
    let (_space, target, provider) = space_target(cx, p.space_uid, request)?;
    let scope = scope_for(&target);
    let removed = ops::group_remove(
        &cx.env,
        provider.as_ref(),
        &scope,
        p.space_uid,
        &group,
        request.request_uid,
    )
    .map_err(typed_op)?;
    to_reply(&target, &removed)
}

fn split_rm(cx: &mut AgentCx, request: &Envelope, payload: Value) -> Result<Reply, TypedError> {
    let p: SplitRmPayload = parse_payload(payload)?;
    let split = parse_child(&p.split_ref, ChildKind::Split)?;
    let (_space, target, provider) = space_target(cx, p.space_uid, request)?;
    let scope = scope_for(&target);
    let removed = ops::split_remove(
        &cx.env,
        provider.as_ref(),
        &scope,
        p.space_uid,
        &split,
        request.request_uid,
    )
    .map_err(typed_op)?;
    to_reply(&target, &removed)
}
