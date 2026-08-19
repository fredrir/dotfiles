//! `dmux adopt NATIVE_REF` — the only ordinary adoption entry point
//! (plan §10.3, §7.4).
//!
//! The token is opaque: it is re-resolved in a fresh complete owner scan
//! under the adoption lease and never handed to a backend as a command
//! string. This layer decodes the ref, builds the owner target from the
//! registry, and calls one fenced operation; the re-resolution itself is
//! `operations::adopt_tmux`/`adopt_wez`, which own the lease. Both land the
//! Space `active + unstamped`, so the receipt says so and points at
//! `dmux context stamp`.
//!
//! Owned by the P6 adoption agent (plan §19.3).

use serde_json::{Value, json};
use uuid::Uuid;

use crate::backend::InventoryScope;
use crate::backend::tmux::TmuxProvider;
use crate::backend::wez::{SystemRunner, WezProvider, WezRunner};
use crate::error::{ErrorCode, ExitStatus, TypedError};
use crate::model::Backend;
use crate::operations::{
    ADOPT_IDENTITY_CONFLICT, ADOPT_MARKER_CONFLICT, ADOPT_UNRENDERABLE_NAME, AdoptedSpace, OpError,
    OperationEnv, adopt_tmux, adopt_wez,
};
use crate::output::{self, OutputFormat};
use crate::registry::{Registry, RegistryConfig, SpaceRow};

/// The body below is real, so the binary's Wez-first arm dispatches here.
pub const IMPLEMENTED: bool = true;

pub struct AdoptArgs {
    /// `-H/--host`: alias, label, or HostUid; `None` is the local authority.
    pub host: Option<String>,
    /// `native:<backend>:<base64url-no-padding>`, parsed by
    /// [`crate::output::parse_native_ref`].
    pub native_ref: String,
    /// Logical name for the adopted Space; the native name when omitted.
    pub name: Option<String>,
}

/// The wezterm CLI adoption drives. Production resolves the managed
/// bin/config and spawns real subprocesses; the tests substitute a canned
/// runner so the CAS refusals below are provable without a fork build.
pub struct WezCli<R: WezRunner> {
    pub bin: String,
    pub config: String,
    pub runner: R,
}

/// One rendered adoption outcome. Returned rather than printed so a test
/// can pin the receipt — including the pending-pane hint — without
/// capturing the process's own stdout.
pub struct AdoptOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

pub fn adopt(format: Option<OutputFormat>, args: AdoptArgs) -> ExitStatus {
    let env = match OperationEnv::production() {
        Ok(env) => env,
        Err(e) => {
            let error = TypedError::new(ErrorCode::OperationFailed, e.to_string());
            return emit(render(format, 0, Err(error)));
        }
    };
    let (bin, config) = crate::runtime::production_wez_paths();
    emit(adopt_in(
        &env,
        WezCli {
            bin,
            config,
            runner: SystemRunner,
        },
        format,
        args,
    ))
}

/// [`adopt`] against an explicit registry/runtime and wezterm CLI.
pub fn adopt_in<R: WezRunner>(
    env: &OperationEnv,
    wez: WezCli<R>,
    format: Option<OutputFormat>,
    args: AdoptArgs,
) -> AdoptOutput {
    let outcome = run(env, wez, &args);
    // Read after the operation: a successful adoption advanced the chain,
    // and a refusal must still report the revision it refused against.
    render(format, revision(env), outcome)
}

fn emit(output: AdoptOutput) -> ExitStatus {
    print!("{}", output.stdout);
    eprint!("{}", output.stderr);
    output.status
}

/// The adoption receipt (plan §16.2). Adoption is owner-local, so the
/// compact ref is always the bare SpaceNo the local authority `a` renders.
struct Receipt {
    row: SpaceRow,
    backend: Backend,
    native_ref: String,
    native_token: String,
}

impl Receipt {
    fn compact_ref(&self) -> String {
        self.row.space_no.get().to_string()
    }

    /// The §10.3 landing condition, not a suggestion: the Space stays
    /// `unstamped` until every pane that predates adoption acknowledges.
    fn stamp_hint(&self) -> String {
        format!("dmux context stamp {}", self.compact_ref())
    }

    fn json(&self) -> Value {
        json!({
            "uri": crate::refs::canonical_uri(self.row.owner, self.row.space_uid),
            "portable_ref": format!("{}:{}", self.row.owner.0, self.row.space_no),
            "compact_ref": self.compact_ref(),
            "space_uid": self.row.space_uid.0.to_string(),
            "space_no": self.row.space_no.get(),
            "name": self.row.logical_name,
            "backend": self.backend.as_str(),
            "native_ref": self.native_ref,
            "native_token": self.native_token,
            "lifecycle": self.row.lifecycle,
            "health": self.row.health,
            "pending_stamp_command": self.stamp_hint(),
        })
    }
}

fn run<R: WezRunner>(
    env: &OperationEnv,
    wez: WezCli<R>,
    args: &AdoptArgs,
) -> Result<Receipt, TypedError> {
    // Decode before anything else: a token that is not an exact native ref
    // must reach neither the registry nor a provider (plan §7.4).
    let (backend, native_token) = output::parse_native_ref(&args.native_ref)?;

    // An operator-chosen name is a *new* managed name and answers to the
    // grammar `new` enforces — otherwise `--name 7` mints a Space the
    // numeric-ref grammar (§7.3) permanently shadows. The inherited native
    // name is deliberately exempt: §10.3 keeps legacy spellings, and the
    // operation only holds it to renderability.
    if let Some(name) = &args.name {
        crate::refs::validate_new_name(name).map_err(|error| {
            TypedError::new(
                ErrorCode::InvalidName,
                format!("invalid new name {name:?}: {error:?}"),
            )
        })?;
    }

    let registry = open(env)?;
    if let Some(host) = &args.host {
        let row = crate::remote::hosts::resolve_host(&registry, host)?;
        let local = registry.identity().map_err(reg)?.host_uid;
        if row.host_uid != local {
            return Err(TypedError::new(
                ErrorCode::ProtocolMismatch,
                format!(
                    "adoption is owner-local (plan §2.6) and the agent protocol carries no \
                     ADOPT method, so --host {host} cannot adopt; run `dmux adopt` on that host"
                ),
            ));
        }
    }
    let scope = owner_scope(&registry, backend)?;
    drop(registry);

    let request_uid = Uuid::new_v4();
    let adopted = match backend {
        Backend::Tmux => adopt_tmux(
            env,
            &TmuxProvider::new(scope.endpoint.clone()),
            &scope,
            &native_token,
            args.name.as_deref(),
            request_uid,
        ),
        Backend::Wez => adopt_wez(
            env,
            &WezProvider::with_runner(&wez.bin, &wez.config, wez.runner),
            &scope,
            &native_token,
            args.name.as_deref(),
            request_uid,
        ),
    }
    .map_err(|e| typed(&e, &args.native_ref))?;
    receipt(env, backend, args, adopted)
}

/// Report the durable row rather than the operation's return value: the
/// receipt's `active/unstamped` claim has to be what the registry holds.
fn receipt(
    env: &OperationEnv,
    backend: Backend,
    args: &AdoptArgs,
    adopted: AdoptedSpace,
) -> Result<Receipt, TypedError> {
    let row = open(env)?.space(adopted.space_uid).map_err(reg)?;
    Ok(Receipt {
        row,
        backend,
        native_ref: args.native_ref.clone(),
        native_token: adopted.native_token,
    })
}

/// Endpoint and published epoch for one local backend, built from the
/// registry exactly as `ls` builds its scan target.
fn owner_scope(registry: &Registry, backend: Backend) -> Result<InventoryScope, TypedError> {
    let unavailable = |detail: String| TypedError::new(ErrorCode::ProviderUnavailable, detail);
    let instance = registry
        .backend_instance_for_backend(backend)
        .map_err(reg)?
        .ok_or_else(|| {
            unavailable(format!(
                "no managed {backend} backend instance is registered"
            ))
        })?;
    let endpoint = registry
        .backend_instance_info(instance)
        .map_err(reg)?
        .socket_path
        .ok_or_else(|| unavailable(format!("the managed {backend} instance has no endpoint")))?;
    Ok(InventoryScope {
        backend,
        endpoint,
        expected_epoch: registry.backend_server(instance).map_err(reg)?.server_epoch,
    })
}

fn open(env: &OperationEnv) -> Result<Registry, TypedError> {
    Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir))
        .map_err(|e| TypedError::new(e.error_code(), e.to_string()))
}

fn reg(e: crate::registry::RegistryError) -> TypedError {
    TypedError::new(e.error_code(), e.to_string())
}

fn revision(env: &OperationEnv) -> u64 {
    open(env)
        .and_then(|r| r.authority_head().map_err(reg))
        .map_or(0, |head| head.revision)
}

/// The operation's stringly errors as the plan's typed codes. The generic
/// tail matches `new_cli`/`rm_cli` exactly — adoption is not entitled to its
/// own weaker partition — and the refusals §10.3 gives distinct remedies are
/// lifted out of it: a server without the fork CAS verb is a build
/// incompatibility, a multi-window resource is repairable, and a resource
/// that already belongs to someone is an identity conflict, not a name one.
fn typed(err: &OpError, native_ref: &str) -> TypedError {
    let (code, message) = match err {
        OpError::Provider(detail) if detail.contains("cas_capability_missing") => (
            ErrorCode::VersionMismatch,
            format!(
                "the managed WezTerm server lacks the fork CAS rename verb (ADR 006), so this \
                 workspace stays unmanaged (plan §2.7): {detail}"
            ),
        ),
        OpError::Provider(detail) if detail.contains("spans multiple windows") => (
            ErrorCode::RepairRequired,
            format!("run `dmux repair normalize {native_ref}` first: {detail}"),
        ),
        OpError::NameConflict(detail)
            if detail.starts_with(ADOPT_IDENTITY_CONFLICT)
                || detail.starts_with(ADOPT_MARKER_CONFLICT) =>
        {
            (ErrorCode::IdentityConflict, detail.clone())
        }
        OpError::NameConflict(detail) if detail.starts_with(ADOPT_UNRENDERABLE_NAME) => {
            (ErrorCode::InvalidName, detail.clone())
        }
        OpError::NotFound(detail) => (ErrorCode::NotFound, detail.clone()),
        OpError::NameConflict(detail) => (ErrorCode::NameConflict, detail.clone()),
        OpError::Indeterminate(detail) => (ErrorCode::ProviderUnavailable, detail.clone()),
        OpError::Refused(detail) => (ErrorCode::OperationInProgress, detail.clone()),
        OpError::StaleRef(detail) => (ErrorCode::BackendEpochChanged, detail.clone()),
        OpError::Registry(detail) if detail.contains("registry busy") => {
            (ErrorCode::RegistryBusy, detail.clone())
        }
        other => (ErrorCode::OperationFailed, other.to_string()),
    };
    let mut error = TypedError::new(code, message);
    error.target = Some(native_ref.to_string());
    error
}

fn render(
    format: Option<OutputFormat>,
    authority_revision: u64,
    outcome: Result<Receipt, TypedError>,
) -> AdoptOutput {
    if format == Some(OutputFormat::Json) {
        let (ok, result, errors) = match &outcome {
            Ok(receipt) => (true, receipt.json(), Vec::new()),
            Err(error) => (false, Value::Null, vec![error.clone()]),
        };
        return AdoptOutput {
            status: output::document_exit(ok, ok, &errors),
            stdout: format!(
                "{}\n",
                output::document("adopt", ok, result, &errors, authority_revision)
            ),
            stderr: String::new(),
        };
    }
    match outcome {
        Ok(receipt) => AdoptOutput {
            status: ExitStatus::Success,
            stdout: format!(
                "adopted {} {:?} ({}) as {}\n{}: every pane that predates adoption must run `{}`\n",
                receipt.compact_ref(),
                receipt.row.logical_name,
                receipt.backend,
                crate::refs::canonical_uri(receipt.row.owner, receipt.row.space_uid),
                token(receipt.row.health),
                receipt.stamp_hint(),
            ),
            stderr: String::new(),
        },
        Err(error) => AdoptOutput {
            status: error.code.exit_status(),
            stdout: String::new(),
            stderr: format!("dmux: {}\n", error.message),
        },
    }
}

/// One spelling for both renderers: the JSON tokens are contract (§16.2),
/// so the human line must not invent a second vocabulary for the same state.
fn token(value: impl serde::Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".into())
}
