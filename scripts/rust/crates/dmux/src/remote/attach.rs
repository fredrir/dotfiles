//! Owner-side `_attach` endpoint (plan §12.1): the single-use-token PTY
//! attach channel. Hashes the presented token, redeems it atomically, then
//! verifies — in order — that the token was minted by THIS authority
//! (HostUid), that the Space is still active and bound, and that the
//! recorded server epoch still equals the LIVE tmux server epoch (a
//! restarted server refuses). Only then does it `exec` the exact
//! owner-recorded attach argv.
//!
//! It accepts no native target and no command text from the client — the
//! argv comes only from the redeemed record. Every refusal is one stderr
//! line plus a typed exit; a redeemed-but-refused token stays consumed
//! (single use is not negotiable).

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::backend::tmux::{TmuxProvider, TmuxServerIdentity};
use crate::bootstrap::MarkerContext;
use crate::connect_cli::{FrozenConnectTarget, OwnerExecPlan, TmuxExecKind, VerifiedConnectChild};
use crate::error::{ErrorCode, TypedError};
use crate::locks::{LockMode, LockScope, OrderedLocks};
use crate::model::{
    Backend, BackendInstanceUid, ChildKind, HostUid, Lifecycle, ProviderHandle, ServerEpoch,
    SpaceNo, SpaceUid,
};
use crate::operations::{OperationEnv, SpaceHierarchy};
use crate::refs::{ChildRefShape, child_suffix, parse_ref};
use crate::registry::{
    AttachRedemption, RedeemedAttach, Registry, RegistryConfig, now_rfc3339, sha256::sha256_hex,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const CLIENT_RECORD_VERSION: u32 = 1;
const CLIENT_RECORD_DIR: &str = "tmux-clients";
// tmux 3.7b on Linux rewrites control-character and non-ASCII bytes in
// `-F` output to `_`. Keep the field separator printable ASCII so the same
// owner/client correlation rows are byte-stable on macOS and Linux.
const TMUX_FORMAT_SEPARATOR: &str = "__DMUX_FIELD_7F4A9C2E__";
const CLIENT_LIST_FORMAT: &str = "#{client_pid}__DMUX_FIELD_7F4A9C2E__#{client_tty}__DMUX_FIELD_7F4A9C2E__#{client_name}__DMUX_FIELD_7F4A9C2E__#{session_id}__DMUX_FIELD_7F4A9C2E__#{window_id}__DMUX_FIELD_7F4A9C2E__#{pane_id}";
const ACTIVE_CONTEXT_FORMAT: &str =
    "#{session_id}__DMUX_FIELD_7F4A9C2E__#{window_id}__DMUX_FIELD_7F4A9C2E__#{pane_id}";

/// Private owner-side correlation record published immediately before the
/// current process execs tmux. PID is therefore the future tmux client PID;
/// `/bin/ps` start time disambiguates PID reuse, and tty is an independent
/// equality witness. The client UID is only the filename/locator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TmuxClientRecord {
    pub record_version: u32,
    pub client_uid: Uuid,
    pub host_uid: HostUid,
    /// Attach-time audit hint. The UID locator may update this after a GUI
    /// switch, while the deterministic PID/start hard link intentionally
    /// remains an immutable attach snapshot. Neither is presentation
    /// authority; hook refresh derives the current Space from the live row.
    pub space_uid: SpaceUid,
    pub backend_instance_uid: BackendInstanceUid,
    pub server_epoch: ServerEpoch,
    pub client_pid: u32,
    pub process_start_token: String,
    pub client_tty: String,
    pub recorded_at: String,
}

/// Exact target fields supplied only after registry/live-provider
/// validation. Native session identity never comes from the GUI request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxClientTarget {
    pub host_uid: HostUid,
    pub space_uid: SpaceUid,
    pub space_no: SpaceNo,
    pub backend_instance_uid: BackendInstanceUid,
    pub server_epoch: ServerEpoch,
    pub namespace: String,
    pub native_session: String,
    /// Complete owner-authoritative native child set permitted for this
    /// operation. An exact marker has one member, Group selection has every
    /// live Split in that Group, and a Space switch/removal fallback has the
    /// complete live hierarchy. The exact client row must match one member;
    /// an empty set never means "any child".
    pub active_children: Vec<TmuxClientChildTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TmuxClientChildTarget {
    pub window: String,
    pub pane: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorrelatedTmuxClient {
    pub client_uid: Uuid,
    pub client_pid: u32,
    pub client_tty: String,
    pub current_session: String,
}

/// Pre-exec controller correlation reserved without writing any OSC user
/// variable. The tmux hook is the sole publisher of the destination marker,
/// after it proves that this exact client is live and attached.
#[derive(Debug, Clone)]
pub struct ControllerCorrelationReservation {
    client_uid: Uuid,
    /// Present only for a fresh local attach. Remote `_attach` publishes its
    /// record on the owner; LocalSwitch reuses an already-live record.
    local_record: Option<(PathBuf, TmuxClientRecord)>,
}

impl ControllerCorrelationReservation {
    pub fn client_uid(&self) -> Uuid {
        self.client_uid
    }
}

/// Exact facts expanded by a tmux client-scoped hook. None is authority on
/// its own; the refresh API requires every field to match the deterministic
/// attach record, live list-client row, registry binding, and server epoch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TmuxHookClientClaim {
    pub namespace: String,
    pub hook_client: String,
    pub client_pid: u32,
    pub client_tty: String,
    pub session_id: String,
    pub window_id: String,
    pub pane_id: String,
}

#[derive(Debug, Clone)]
struct VerifiedAttachTarget {
    host_uid: HostUid,
    space_uid: SpaceUid,
    space_no: SpaceNo,
    backend_instance_uid: BackendInstanceUid,
    server_epoch: ServerEpoch,
    namespace: String,
    native_session: String,
    child: Option<VerifiedConnectChild>,
}

#[derive(Debug, Clone)]
pub struct AttachArgs {
    pub token: String,
    pub data_dir: Option<PathBuf>,
    pub lock_dir: Option<PathBuf>,
}

/// Run the attach endpoint. On success this function does not return (it
/// replaces the process with the owner-generated attach command); every
/// failure path returns the mapped exit code after one line on stderr.
pub fn run(args: &AttachArgs) -> i32 {
    crate::remote::normalize_utf8_locale();
    match attach(args) {
        Ok(never) => match never {},
        Err((code, message)) => {
            eprintln!("dmux _attach: {message}");
            i32::from(code.exit_status().code())
        }
    }
}

enum Never {}

fn attach(args: &AttachArgs) -> Result<Never, (ErrorCode, String)> {
    let env = match (&args.data_dir, &args.lock_dir) {
        (Some(data), Some(lock)) => OperationEnv {
            db_path: data.join("registry.sqlite3"),
            lock_dir: lock.clone(),
        },
        _ => OperationEnv::production()
            .map_err(|e| (ErrorCode::OperationFailed, format!("environment: {e}")))?,
    };
    let mut registry = Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir))
        .map_err(|e| (e.error_code(), format!("registry: {e}")))?;

    // Never persist or log the raw token; only its sha256 is looked up.
    let token_hash = sha256_hex(args.token.trim().as_bytes());
    let redemption = registry
        .redeem_attach_token(&token_hash, &now_rfc3339())
        .map_err(|e| (e.error_code(), format!("redeem: {e}")))?;
    let redeemed = match redemption {
        AttachRedemption::Redeemed(redeemed) => redeemed,
        AttachRedemption::Replayed => {
            return Err((
                ErrorCode::AuthFailed,
                "attach token already redeemed; replay refused".to_string(),
            ));
        }
        AttachRedemption::Expired => {
            return Err((ErrorCode::AuthFailed, "attach token expired".to_string()));
        }
        AttachRedemption::Revoked => {
            return Err((ErrorCode::AuthFailed, "attach token revoked".to_string()));
        }
        AttachRedemption::Unknown => {
            return Err((ErrorCode::AuthFailed, "unknown attach token".to_string()));
        }
    };
    let verified = verify(&registry, &redeemed)?;
    // Keep the pre-exec owner/live child proof, but do not publish it yet.
    // Only the client-attached hook can prove that the exec below actually
    // became this exact live tmux client; that hook publishes UID + marker.
    let _verified_marker = resolve_current_tmux_marker(
        verified.host_uid,
        verified.space_uid,
        verified.space_no,
        verified.server_epoch,
        &verified.namespace,
        &verified.native_session,
        verified.child.as_ref(),
    )
    .map_err(|error| {
        (
            error.code,
            format!("resolving initial tmux GUI context: {}", error.message),
        )
    })?;
    publish_current_client_record(
        &env.lock_dir,
        redeemed.request_uid,
        verified.host_uid,
        verified.space_uid,
        verified.backend_instance_uid,
        verified.server_epoch,
    )
    .map_err(|error| {
        (
            error.code,
            format!(
                "registering exact tmux client correlation: {}",
                error.message
            ),
        )
    })?;

    // Exec the EXACT owner-recorded argv; nothing from the client.
    let (program, argv_rest) = redeemed
        .attach_argv
        .split_first()
        .ok_or((ErrorCode::OperationFailed, "empty attach argv".to_string()))?;
    use std::os::unix::process::CommandExt;
    let error = std::process::Command::new(program).args(argv_rest).exec();
    Err((
        ErrorCode::OperationFailed,
        format!("exec {program}: {error}"),
    ))
}

/// The post-redemption verification chain (plan §12.1). Any failure here
/// refuses the attach; the single-use token is already consumed.
fn verify(
    registry: &Registry,
    redeemed: &RedeemedAttach,
) -> Result<VerifiedAttachTarget, (ErrorCode, String)> {
    let identity = registry
        .identity()
        .map_err(|e| (e.error_code(), format!("identity: {e}")))?;
    if redeemed.host_uid != identity.host_uid {
        return Err((
            ErrorCode::HostIdentityChanged,
            format!(
                "token was minted for host {} but this authority is {}",
                redeemed.host_uid.0, identity.host_uid.0
            ),
        ));
    }
    let space = registry
        .space(redeemed.space_uid)
        .map_err(|e| (ErrorCode::SpaceAbsent, format!("space: {e}")))?;
    if space.lifecycle != Lifecycle::Active {
        return Err((
            ErrorCode::SpaceAbsent,
            format!(
                "space is no longer active ({})",
                lifecycle_token(space.lifecycle)
            ),
        ));
    }
    let binding = registry
        .current_binding(redeemed.space_uid)
        .map_err(|e| (e.error_code(), format!("binding: {e}")))?
        .ok_or((
            ErrorCode::SpaceAbsent,
            "space has no current native binding".to_string(),
        ))?;
    // The recorded argv targets the bound session; a rebind since issue
    // invalidates the plan.
    if redeemed.attach_argv.last() != Some(&binding.native_token) {
        return Err((
            ErrorCode::SpaceAbsent,
            "space was rebound since the plan was issued".to_string(),
        ));
    }
    // LIVE server re-probe: the published incarnation must still be running
    // and its epoch must equal the one recorded in the token. A restarted
    // server (fresh incarnation or fresh epoch) refuses.
    let info = registry
        .backend_instance_info(space.backend_instance)
        .map_err(|e| (e.error_code(), format!("instance: {e}")))?;
    if info.backend != Backend::Tmux {
        return Err((
            ErrorCode::ProviderUnavailable,
            "attach tokens are tmux-only".to_string(),
        ));
    }
    let namespace = info.socket_path.ok_or((
        ErrorCode::ProviderUnavailable,
        "managed tmux instance has no recorded namespace".to_string(),
    ))?;
    let record = registry
        .backend_server(space.backend_instance)
        .map_err(|e| (e.error_code(), format!("server record: {e}")))?;
    let published_epoch = record.server_epoch.ok_or((
        ErrorCode::BackendEpochChanged,
        "tmux server has no published epoch".to_string(),
    ))?;
    if published_epoch != redeemed.server_epoch {
        return Err((
            ErrorCode::BackendEpochChanged,
            format!(
                "token epoch {} but the published server epoch is {}",
                redeemed.server_epoch.0, published_epoch.0
            ),
        ));
    }
    let expected_identity = TmuxServerIdentity {
        pid: record
            .server_pid
            .and_then(|pid| u32::try_from(pid).ok())
            .ok_or((
                ErrorCode::BackendEpochChanged,
                "published incarnation has no recorded pid".to_string(),
            ))?,
        start_token: record.server_start_token.clone().ok_or((
            ErrorCode::BackendEpochChanged,
            "published incarnation has no recorded start token".to_string(),
        ))?,
    };
    let provider: TmuxProvider<_> = TmuxProvider::new(namespace.clone());
    provider
        .verify_epoch(&namespace, redeemed.server_epoch, &expected_identity)
        .map_err(|e| match e {
            crate::backend::ProviderError::EpochChanged { .. }
            | crate::backend::ProviderError::WrongInstance { .. } => (
                ErrorCode::BackendEpochChanged,
                "tmux server restarted since the plan was issued".to_string(),
            ),
            other => (
                ErrorCode::ProviderUnavailable,
                format!("tmux server probe failed: {other:?}"),
            ),
        })?;
    let child = parse_recorded_attach_child(
        &redeemed.attach_argv,
        &namespace,
        &binding.native_token,
        redeemed.server_epoch,
    )
    .map_err(|error| (error.code, error.message))?;
    Ok(VerifiedAttachTarget {
        host_uid: identity.host_uid,
        space_uid: redeemed.space_uid,
        space_no: space.space_no,
        backend_instance_uid: space.backend_instance,
        server_epoch: redeemed.server_epoch,
        namespace,
        native_session: binding.native_token,
        child,
    })
}

/// Recover the exact child focus from the owner-generated argv stored with
/// an attach token.  This is intentionally a closed parser for the three
/// command shapes emitted by `attach_plan`; no shell text or client input is
/// interpreted, and an unfamiliar future shape fails closed.
fn parse_recorded_attach_child(
    argv: &[String],
    namespace: &str,
    native_session: &str,
    epoch: ServerEpoch,
) -> Result<Option<VerifiedConnectChild>, TypedError> {
    if argv.len() < 6
        || argv[0] != "tmux"
        || argv[1] != "-L"
        || argv[2] != namespace
        || argv[argv.len() - 3] != "attach-session"
        || argv[argv.len() - 2] != "-t"
        || argv[argv.len() - 1] != native_session
    {
        return Err(TypedError::new(
            ErrorCode::ProtocolMismatch,
            "recorded attach argv is not the exact owner tmux attach shape",
        ));
    }
    let middle = &argv[3..argv.len() - 3];
    if middle.is_empty() {
        return Ok(None);
    }
    let window_prefix = format!("{native_session}:@");
    if middle.len() != 4 && middle.len() != 8
        || middle[0] != "select-window"
        || middle[1] != "-t"
        || middle[3] != ";"
    {
        return Err(TypedError::new(
            ErrorCode::ProtocolMismatch,
            "recorded attach argv carries an unrecognized focus sequence",
        ));
    }
    let window = parse_canonical_native_id(&middle[2], &window_prefix)?;
    if middle.len() == 4 {
        return Ok(Some(VerifiedConnectChild::Group {
            epoch,
            handle: ProviderHandle::Tx(window),
        }));
    }
    if middle[4] != "select-pane" || middle[5] != "-t" || middle[7] != ";" {
        return Err(TypedError::new(
            ErrorCode::ProtocolMismatch,
            "recorded attach argv carries an unrecognized Split focus sequence",
        ));
    }
    let pane = parse_canonical_native_id(&middle[6], "%")?;
    Ok(Some(VerifiedConnectChild::Split {
        epoch,
        group: ProviderHandle::Tx(window),
        split: ProviderHandle::Tx(pane),
    }))
}

fn parse_canonical_native_id(value: &str, prefix: &str) -> Result<u64, TypedError> {
    let digits = value.strip_prefix(prefix).ok_or_else(|| {
        TypedError::new(
            ErrorCode::ProtocolMismatch,
            "tmux native identity has an unexpected prefix",
        )
    })?;
    if digits.is_empty()
        || digits.len() > 1 && digits.starts_with('0')
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(TypedError::new(
            ErrorCode::ProtocolMismatch,
            "tmux native identity is not canonical decimal",
        ));
    }
    digits.parse().map_err(|_| {
        TypedError::new(
            ErrorCode::ProtocolMismatch,
            "tmux native identity exceeds the supported range",
        )
    })
}

/// Resolve the context that the impending attach will display. For an
/// explicit child, the owner-proved focus is used; otherwise tmux resolves
/// the session's current active window/pane. The display query proves the
/// resulting session/window/pane tuple exists together immediately before
/// exec.
fn resolve_current_tmux_marker(
    host_uid: HostUid,
    space_uid: SpaceUid,
    space_no: SpaceNo,
    epoch: ServerEpoch,
    namespace: &str,
    native_session: &str,
    child: Option<&VerifiedConnectChild>,
) -> Result<MarkerContext, TypedError> {
    let (query, expected_window, expected_pane) = match child {
        None => (native_session.to_string(), None, None),
        Some(VerifiedConnectChild::Group {
            epoch: child_epoch,
            handle: ProviderHandle::Tx(window),
        }) if *child_epoch == epoch => (format!("{native_session}:@{window}"), Some(*window), None),
        Some(VerifiedConnectChild::Split {
            epoch: child_epoch,
            group: ProviderHandle::Tx(window),
            split: ProviderHandle::Tx(pane),
        }) if *child_epoch == epoch => (format!("%{pane}"), Some(*window), Some(*pane)),
        Some(_) => {
            return Err(TypedError::new(
                ErrorCode::BackendEpochChanged,
                "initial tmux child witness is not a same-epoch tmux handle",
            ));
        }
    };
    let output = Command::new("tmux")
        .args([
            "-L",
            namespace,
            "display-message",
            "-p",
            "-t",
            &query,
            ACTIVE_CONTEXT_FORMAT,
        ])
        .env("LC_ALL", "C.UTF-8")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| io_error("resolving initial tmux active context", error))?;
    if !output.status.success() {
        return Err(TypedError::new(
            ErrorCode::ProviderUnavailable,
            format!(
                "tmux active-context query failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    let stdout = std::str::from_utf8(&output.stdout).map_err(|_| {
        TypedError::new(
            ErrorCode::OperationFailed,
            "tmux active-context query is not UTF-8",
        )
    })?;
    let lines: Vec<_> = stdout.lines().filter(|line| !line.is_empty()).collect();
    let [line] = lines.as_slice() else {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "tmux active-context query did not identify exactly one target",
        ));
    };
    let fields: Vec<_> = line.split(TMUX_FORMAT_SEPARATOR).collect();
    let [session, window, pane] = fields.as_slice() else {
        return Err(TypedError::new(
            ErrorCode::OperationFailed,
            "tmux active-context query returned a malformed tuple",
        ));
    };
    if *session != native_session {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "tmux active-context query crossed the exact Space session",
        ));
    }
    let window_id = parse_canonical_native_id(window, "@")?;
    let pane_id = parse_canonical_native_id(pane, "%")?;
    if expected_window.is_some_and(|expected| expected != window_id)
        || expected_pane.is_some_and(|expected| expected != pane_id)
    {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "tmux active-context query differs from the owner-proved child focus",
        ));
    }
    Ok(MarkerContext {
        host_uid,
        space_uid,
        space_no,
        backend: Backend::Tmux,
        domain: None,
        server_epoch: epoch,
        group_ref: child_suffix(&ChildRefShape {
            kind: ChildKind::Group,
            epoch,
            handle: ProviderHandle::Tx(window_id),
        }),
        split_ref: child_suffix(&ChildRefShape {
            kind: ChildKind::Split,
            epoch,
            handle: ProviderHandle::Tx(pane_id),
        }),
    })
}

fn revalidate_local_plan_marker(
    env: &OperationEnv,
    target: &FrozenConnectTarget,
) -> Result<MarkerContext, TypedError> {
    if target.backend != Backend::Tmux {
        return Err(TypedError::new(
            ErrorCode::BackendMismatch,
            "tmux controller correlation received a non-tmux exec target",
        ));
    }
    let registry = Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir))
        .map_err(|error| TypedError::new(error.error_code(), format!("registry: {error}")))?;
    let identity = registry
        .identity()
        .map_err(|error| TypedError::new(error.error_code(), format!("identity: {error}")))?;
    let space = registry.space(target.space_uid).map_err(|error| {
        TypedError::new(error.error_code(), format!("tmux target Space: {error}"))
    })?;
    if identity.host_uid != target.owner
        || space.owner != target.owner
        || space.space_no != target.space_no
        || space.backend_instance != target.backend_instance_uid
        || space.lifecycle != Lifecycle::Active
    {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "local tmux exec target changed owner/Space/binding before handoff",
        ));
    }
    let binding = registry
        .current_binding(target.space_uid)
        .map_err(|error| TypedError::new(error.error_code(), format!("binding: {error}")))?
        .ok_or_else(|| TypedError::new(ErrorCode::SpaceAbsent, "tmux Space has no binding"))?;
    let info = registry
        .backend_instance_info(target.backend_instance_uid)
        .map_err(|error| TypedError::new(error.error_code(), format!("instance: {error}")))?;
    if info.backend != Backend::Tmux
        || info.owner != target.owner
        || info.socket_path.as_deref() != Some(target.binding.endpoint.as_str())
        || binding.native_token != target.binding.native_token
    {
        return Err(TypedError::new(
            ErrorCode::WrongBackendInstance,
            "local tmux exec target no longer matches its registered backend binding",
        ));
    }
    let published = registry
        .backend_server(target.backend_instance_uid)
        .map_err(|error| TypedError::new(error.error_code(), format!("server record: {error}")))?;
    if published.server_epoch != Some(target.server_epoch) {
        return Err(TypedError::new(
            ErrorCode::BackendEpochChanged,
            "local tmux server epoch changed before terminal handoff",
        ));
    }
    let expected_identity = TmuxServerIdentity {
        pid: published
            .server_pid
            .and_then(|pid| u32::try_from(pid).ok())
            .ok_or_else(|| {
                TypedError::new(
                    ErrorCode::BackendEpochChanged,
                    "local tmux server has no published PID",
                )
            })?,
        start_token: published.server_start_token.ok_or_else(|| {
            TypedError::new(
                ErrorCode::BackendEpochChanged,
                "local tmux server has no published start token",
            )
        })?,
    };
    TmuxProvider::new(target.binding.endpoint.clone())
        .verify_epoch(
            &target.binding.endpoint,
            target.server_epoch,
            &expected_identity,
        )
        .map_err(|error| match error {
            crate::backend::ProviderError::EpochChanged { .. }
            | crate::backend::ProviderError::WrongInstance { .. } => TypedError::new(
                ErrorCode::BackendEpochChanged,
                "local tmux server restarted before terminal handoff",
            ),
            other => TypedError::new(
                ErrorCode::ProviderUnavailable,
                format!("local tmux server probe failed: {other:?}"),
            ),
        })?;
    resolve_current_tmux_marker(
        target.owner,
        target.space_uid,
        target.space_no,
        target.server_epoch,
        &target.binding.endpoint,
        &target.binding.native_token,
        target.child.as_ref(),
    )
}

/// Reserve the exact controller correlation for a feature-on terminal
/// handoff without emitting any OSC user variable.
///
/// A fresh local attach records this process PID/start-token/TTY before exec;
/// a remote attach reuses the owner-minted request UID that `_attach` will
/// publish on the peer; LocalSwitch reuses the UID from an already-bound GUI
/// source. The managed tmux client hook remains the sole destination-marker
/// publisher, and runs only after the client is demonstrably attached.
///
/// `existing_client_uid` is accepted only for LocalSwitch. It must come from
/// the exact pre-OSC GUI source witness, not ambient text. A headless or
/// unmanaged LocalSwitch supplies `None` and receives no GUI correlation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControllerReservationMode {
    None,
    FreshLocal,
    ExistingLocal(Uuid),
    Remote(Uuid),
}

fn controller_reservation_mode(
    kind: TmuxExecKind,
    has_wezterm_pane: bool,
    inside_tmux: bool,
    existing_client_uid: Option<Uuid>,
    remote_client_uid: Option<Uuid>,
) -> Result<ControllerReservationMode, TypedError> {
    if !has_wezterm_pane {
        return if existing_client_uid.is_some() {
            Err(TypedError::new(
                ErrorCode::IdentityConflict,
                "GUI source supplied a tmux client UID without WEZTERM_PANE",
            ))
        } else {
            Ok(ControllerReservationMode::None)
        };
    }
    match kind {
        TmuxExecKind::LocalSwitch => {
            if !inside_tmux {
                return Err(TypedError::new(
                    ErrorCode::WrongBackendInstance,
                    "local switch correlation requires the existing tmux client",
                ));
            }
            Ok(existing_client_uid.map_or(
                ControllerReservationMode::None,
                ControllerReservationMode::ExistingLocal,
            ))
        }
        TmuxExecKind::LocalAttach => {
            if inside_tmux || existing_client_uid.is_some() {
                return Err(TypedError::new(
                    ErrorCode::WrongBackendInstance,
                    "cannot reserve a fresh tmux client through an existing tmux client",
                ));
            }
            Ok(ControllerReservationMode::FreshLocal)
        }
        TmuxExecKind::RemoteAttach => {
            if inside_tmux || existing_client_uid.is_some() {
                return Err(TypedError::new(
                    ErrorCode::WrongBackendInstance,
                    "cannot reserve a remote tmux client through an existing tmux client",
                ));
            }
            remote_client_uid
                .map(ControllerReservationMode::Remote)
                .ok_or_else(|| {
                    TypedError::new(
                        ErrorCode::ProtocolMismatch,
                        "remote tmux exec plan omitted its attach witness",
                    )
                })
        }
    }
}

pub fn reserve_controller_correlation(
    plan: &OwnerExecPlan,
    existing_client_uid: Option<Uuid>,
) -> Result<Option<ControllerCorrelationReservation>, TypedError> {
    let remote_client_uid = plan.remote_witness().map(|witness| witness.request_uid);
    let mode = controller_reservation_mode(
        plan.kind(),
        std::env::var_os("WEZTERM_PANE").is_some(),
        std::env::var_os("TMUX").is_some(),
        existing_client_uid,
        remote_client_uid,
    )?;
    match mode {
        ControllerReservationMode::None => Ok(None),
        ControllerReservationMode::ExistingLocal(client_uid) => {
            let env = OperationEnv::production().map_err(|error| {
                TypedError::new(ErrorCode::OperationFailed, format!("environment: {error}"))
            })?;
            let target = plan.target();
            let mut locks = OrderedLocks::new(&env.lock_dir);
            locks
                .acquire(LockScope::AuthorityGate, LockMode::Shared)
                .and_then(|_| {
                    locks.acquire(
                        LockScope::BackendInstance(target.backend_instance_uid),
                        LockMode::Shared,
                    )
                })
                .map_err(|error| {
                    TypedError::new(
                        ErrorCode::OperationFailed,
                        format!("tmux switch correlation fence: {error}"),
                    )
                })?;
            revalidate_local_plan_marker(&env, target)?;
            validate_existing_switch_record(&env.lock_dir, client_uid, target)?;
            Ok(Some(ControllerCorrelationReservation {
                client_uid,
                local_record: None,
            }))
        }
        ControllerReservationMode::FreshLocal => {
            let client_uid = Uuid::new_v4();
            let env = OperationEnv::production().map_err(|error| {
                TypedError::new(ErrorCode::OperationFailed, format!("environment: {error}"))
            })?;
            let target = plan.target();
            let mut locks = OrderedLocks::new(&env.lock_dir);
            locks
                .acquire(LockScope::AuthorityGate, LockMode::Shared)
                .and_then(|_| {
                    locks.acquire(
                        LockScope::BackendInstance(target.backend_instance_uid),
                        LockMode::Shared,
                    )
                })
                .map_err(|error| {
                    TypedError::new(
                        ErrorCode::OperationFailed,
                        format!("tmux controller correlation fence: {error}"),
                    )
                })?;
            revalidate_local_plan_marker(&env, target)?;
            let record = publish_current_client_record(
                &env.lock_dir,
                client_uid,
                target.owner,
                target.space_uid,
                target.backend_instance_uid,
                target.server_epoch,
            )?;
            Ok(Some(ControllerCorrelationReservation {
                client_uid,
                local_record: Some((env.lock_dir, record)),
            }))
        }
        ControllerReservationMode::Remote(client_uid) => {
            Ok(Some(ControllerCorrelationReservation {
                client_uid,
                local_record: None,
            }))
        }
    }
}

fn validate_existing_switch_record(
    runtime_dir: &Path,
    client_uid: Uuid,
    target: &FrozenConnectTarget,
) -> Result<(), TypedError> {
    let record = read_client_record(runtime_dir, client_uid)?;
    if record.host_uid != target.owner
        || record.backend_instance_uid != target.backend_instance_uid
        || record.server_epoch != target.server_epoch
    {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "local switch client UID belongs to another owner/backend incarnation",
        ));
    }
    let process_record =
        read_process_client_record(runtime_dir, record.client_pid, &record.process_start_token)?;
    if process_record.client_uid != record.client_uid
        || process_record.host_uid != record.host_uid
        || process_record.backend_instance_uid != record.backend_instance_uid
        || process_record.server_epoch != record.server_epoch
        || process_record.client_pid != record.client_pid
        || process_record.process_start_token != record.process_start_token
        || process_record.client_tty != record.client_tty
        || process_record.recorded_at != record.recorded_at
    {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "local switch PID/start record differs from its exact UID record",
        ));
    }
    live_client_for_record(&record, &target.binding.endpoint)?;
    Ok(())
}

/// Cancel only a fresh local correlation that has not been handed to exec.
/// Remote reservations own no local record and LocalSwitch must retain the
/// existing live client's record. Exact bytes and hard-link identity are
/// checked before either pathname is removed.
pub fn cancel_controller_correlation_reservation(
    reservation: &ControllerCorrelationReservation,
) -> Result<bool, TypedError> {
    let Some((runtime_dir, expected)) = &reservation.local_record else {
        return Ok(false);
    };
    let uid_path = client_record_path(runtime_dir, expected.client_uid);
    let process_path = client_process_record_path(
        runtime_dir,
        expected.client_pid,
        &expected.process_start_token,
    );
    let uid_record = read_client_record(runtime_dir, expected.client_uid)?;
    let process_record = read_process_client_record(
        runtime_dir,
        expected.client_pid,
        &expected.process_start_token,
    )?;
    let uid_metadata = fs::symlink_metadata(&uid_path)
        .map_err(|error| io_error("verifying canceled tmux UID record", error))?;
    let process_metadata = fs::symlink_metadata(&process_path)
        .map_err(|error| io_error("verifying canceled tmux process record", error))?;
    if &uid_record != expected
        || &process_record != expected
        || uid_metadata.dev() != process_metadata.dev()
        || uid_metadata.ino() != process_metadata.ino()
    {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "refusing to cancel a changed tmux correlation reservation",
        ));
    }
    fs::remove_file(&uid_path)
        .map_err(|error| io_error("removing canceled tmux UID record", error))?;
    fs::remove_file(&process_path)
        .map_err(|error| io_error("removing canceled tmux process record", error))?;
    Ok(true)
}

/// Publish the exact current-process/tty witness with create-new semantics.
/// The temp file is 0600 before bytes are written; a same-directory hard
/// link atomically publishes it without replacing an existing UID record.
pub fn publish_current_client_record(
    runtime_dir: &Path,
    client_uid: Uuid,
    host_uid: HostUid,
    space_uid: SpaceUid,
    backend_instance_uid: BackendInstanceUid,
    server_epoch: ServerEpoch,
) -> Result<TmuxClientRecord, TypedError> {
    let record = TmuxClientRecord {
        record_version: CLIENT_RECORD_VERSION,
        client_uid,
        host_uid,
        space_uid,
        backend_instance_uid,
        server_epoch,
        client_pid: std::process::id(),
        process_start_token: process_start_token(std::process::id())?,
        client_tty: current_tty()?,
        recorded_at: now_rfc3339(),
    };
    publish_record(runtime_dir, &record)?;
    Ok(record)
}

fn client_record_dir(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(CLIENT_RECORD_DIR)
}

fn client_record_path(runtime_dir: &Path, client_uid: Uuid) -> PathBuf {
    client_record_dir(runtime_dir).join(format!("{client_uid}.json"))
}

/// Deterministic inverse locator used only by tmux client hooks, which have
/// an exact PID but no access to the attach-time UUID. The start token is
/// hashed so filesystem names disclose neither timestamps nor spaces; PID
/// reuse derives a different path and is rejected again from record bytes.
pub fn client_process_record_path(
    runtime_dir: &Path,
    client_pid: u32,
    process_start_token: &str,
) -> PathBuf {
    let digest = sha256_hex(process_start_token.as_bytes());
    client_record_dir(runtime_dir).join(format!("pid-{client_pid}-{digest}.json"))
}

fn ensure_private_record_dir(runtime_dir: &Path) -> Result<PathBuf, TypedError> {
    let dir = client_record_dir(runtime_dir);
    match fs::symlink_metadata(&dir) {
        Ok(metadata) => {
            if !metadata.is_dir()
                || metadata.uid() != unsafe { libc::geteuid() }
                || metadata.mode() & 0o777 != 0o700
            {
                return Err(TypedError::new(
                    ErrorCode::OperationFailed,
                    format!(
                        "tmux client record directory {} is not a current-user-owned 0700 directory",
                        dir.display()
                    ),
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&dir)
                .map_err(|error| io_error("creating tmux client record directory", error))?;
            let metadata = fs::symlink_metadata(&dir)
                .map_err(|error| io_error("verifying tmux client record directory", error))?;
            if !metadata.is_dir()
                || metadata.uid() != unsafe { libc::geteuid() }
                || metadata.mode() & 0o777 != 0o700
            {
                return Err(TypedError::new(
                    ErrorCode::OperationFailed,
                    "tmux client record directory failed post-create verification",
                ));
            }
        }
        Err(error) => {
            return Err(io_error("reading tmux client record directory", error));
        }
    }
    Ok(dir)
}

fn publish_record(runtime_dir: &Path, record: &TmuxClientRecord) -> Result<(), TypedError> {
    let dir = ensure_private_record_dir(runtime_dir)?;
    let final_path = client_record_path(runtime_dir, record.client_uid);
    let process_path =
        client_process_record_path(runtime_dir, record.client_pid, &record.process_start_token);
    let temp_path = dir.join(format!(
        ".{}.{}.tmp",
        record.client_uid,
        Uuid::new_v4().simple()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temp_path)
            .map_err(|error| io_error("creating private tmux client record", error))?;
        let bytes = serde_json::to_vec(record).map_err(|error| {
            TypedError::new(
                ErrorCode::OperationFailed,
                format!("serializing tmux client record: {error}"),
            )
        })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| io_error("writing private tmux client record", error))?;
        let metadata = file
            .metadata()
            .map_err(|error| io_error("verifying private tmux client record", error))?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o777 != 0o600
        {
            return Err(TypedError::new(
                ErrorCode::OperationFailed,
                "tmux client record temp file failed private-mode verification",
            ));
        }
        fs::hard_link(&temp_path, &final_path)
            .map_err(|error| io_error("atomically publishing tmux client record", error))?;
        if let Err(error) = fs::hard_link(&temp_path, &process_path) {
            let _ = fs::remove_file(&final_path);
            return Err(io_error(
                "atomically publishing deterministic tmux client process record",
                error,
            ));
        }
        Ok(())
    })();
    let _ = fs::remove_file(&temp_path);
    result
}

pub fn read_client_record(
    runtime_dir: &Path,
    client_uid: Uuid,
) -> Result<TmuxClientRecord, TypedError> {
    ensure_private_record_dir(runtime_dir)?;
    let path = client_record_path(runtime_dir, client_uid);
    let record = read_record_at(&path)?;
    if record.record_version != CLIENT_RECORD_VERSION || record.client_uid != client_uid {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "tmux client record version/UID does not match its exact path",
        ));
    }
    Ok(record)
}

fn read_process_client_record(
    runtime_dir: &Path,
    client_pid: u32,
    process_start_token: &str,
) -> Result<TmuxClientRecord, TypedError> {
    ensure_private_record_dir(runtime_dir)?;
    let path = client_process_record_path(runtime_dir, client_pid, process_start_token);
    let record = read_record_at(&path)?;
    if record.record_version != CLIENT_RECORD_VERSION
        || record.client_pid != client_pid
        || record.process_start_token != process_start_token
    {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "tmux process record does not match its deterministic PID/start-token path",
        ));
    }
    Ok(record)
}

fn read_record_at(path: &Path) -> Result<TmuxClientRecord, TypedError> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
        .map_err(|error| {
            let code = if error.kind() == io::ErrorKind::NotFound {
                ErrorCode::NotFound
            } else {
                ErrorCode::OperationFailed
            };
            TypedError::new(code, format!("reading tmux client record: {error}"))
        })?;
    let metadata = file
        .metadata()
        .map_err(|error| io_error("verifying tmux client record", error))?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o777 != 0o600
    {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "tmux client record is not a current-user-owned 0600 regular file",
        ));
    }
    let record: TmuxClientRecord = serde_json::from_reader(file).map_err(|error| {
        TypedError::new(
            ErrorCode::IdentityConflict,
            format!("tmux client record is malformed: {error}"),
        )
    })?;
    Ok(record)
}

/// Validate a UID against the exact originating Space and one live tmux
/// client row. PID, process-start token, and tty must all agree; no
/// uniqueness, ordinal, active-client, or tty-only fallback exists.
pub fn correlate_client(
    runtime_dir: &Path,
    client_uid: Uuid,
    expected: &TmuxClientTarget,
) -> Result<CorrelatedTmuxClient, TypedError> {
    let record = read_client_record(runtime_dir, client_uid)?;
    if record.host_uid != expected.host_uid
        || record.backend_instance_uid != expected.backend_instance_uid
        || record.server_epoch != expected.server_epoch
    {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "tmux client UID belongs to a different owner/backend incarnation",
        ));
    }
    let row = live_client_for_record(&record, &expected.namespace)?;
    if row.session != expected.native_session {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "tmux client is no longer attached to the marker's exact Space",
        ));
    }
    if !active_child_matches(expected, &row) {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "GUI marker is not the exact correlated client's active tmux window/pane",
        ));
    }
    Ok(CorrelatedTmuxClient {
        client_uid,
        client_pid: record.client_pid,
        client_tty: record.client_tty,
        current_session: row.session,
    })
}

/// Publish the current owner-validated tmux child to the one exact outer
/// terminal. The caller supplies a complete authoritative child set for the
/// intended post-action state; the live client row must match one member
/// before and after the OSC write. This is used when GUI actions change
/// focus out of band and therefore cannot rely on tmux hook-client fields.
pub fn publish_correlated_client_context(
    runtime_dir: &Path,
    client_uid: Uuid,
    expected: &TmuxClientTarget,
) -> Result<MarkerContext, TypedError> {
    let record = read_client_record(runtime_dir, client_uid)?;
    if record.host_uid != expected.host_uid
        || record.backend_instance_uid != expected.backend_instance_uid
        || record.server_epoch != expected.server_epoch
    {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "tmux client UID belongs to a different context-publication incarnation",
        ));
    }
    let row = live_client_for_record(&record, &expected.namespace)?;
    if row.session != expected.native_session || !active_child_matches(expected, &row) {
        return Err(TypedError::new(
            ErrorCode::PostconditionFailed,
            "exact tmux client did not reach an owner-authorized post-action child",
        ));
    }
    let marker = marker_for_client_row(expected, &row);
    emit_context_to_exact_client(&record, &expected.namespace, &row, &marker)?;
    let after = live_client_for_record(&record, &expected.namespace)?;
    if after != row || !active_child_matches(expected, &after) {
        return Err(TypedError::new(
            ErrorCode::PostconditionFailed,
            "exact tmux client changed after its post-action context publication",
        ));
    }
    Ok(marker)
}

/// Publish a session-global focus change to every dmux-correlated client on
/// that exact session. Native tmux child focus is shared by all attached
/// clients, so updating only the GUI invoker would leave peers with stale
/// outer markers. Each peer is discovered from the live client row through
/// its deterministic PID/start-token path; the private directory is never
/// scanned to select authority, and clients without a dmux record are simply
/// native/unmanaged consumers with no outer marker to update.
pub fn publish_correlated_session_contexts(
    runtime_dir: &Path,
    invoking_client_uid: Uuid,
    expected: &TmuxClientTarget,
) -> Result<(MarkerContext, usize), TypedError> {
    let invoking = read_client_record(runtime_dir, invoking_client_uid)?;
    if invoking.host_uid != expected.host_uid
        || invoking.backend_instance_uid != expected.backend_instance_uid
        || invoking.server_epoch != expected.server_epoch
    {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "invoking tmux client belongs to a different session-refresh incarnation",
        ));
    }
    let invoking_row = live_client_for_record(&invoking, &expected.namespace)?;
    if invoking_row.session != expected.native_session
        || !active_child_matches(expected, &invoking_row)
    {
        return Err(TypedError::new(
            ErrorCode::PostconditionFailed,
            "invoking tmux client did not reach the owner-authorized session child",
        ));
    }

    let rows: Vec<_> = list_clients(&expected.namespace)?
        .into_iter()
        .filter(|row| row.session == expected.native_session)
        .collect();
    let marker = marker_for_client_row(expected, &invoking_row);
    let mut published = 0usize;
    for row in rows {
        if !active_child_matches(expected, &row) {
            return Err(TypedError::new(
                ErrorCode::PostconditionFailed,
                "a peer tmux client observes a child outside the owner-authorized session hierarchy",
            ));
        }
        let start_token = process_start_token(row.pid)?;
        let record = match read_process_client_record(runtime_dir, row.pid, &start_token) {
            Ok(record) => record,
            Err(error) if error.code == ErrorCode::NotFound => continue,
            Err(error) => return Err(error),
        };
        let uid_record = read_client_record(runtime_dir, record.client_uid)?;
        if record.host_uid != expected.host_uid
            || record.backend_instance_uid != expected.backend_instance_uid
            || record.server_epoch != expected.server_epoch
            || record.client_pid != row.pid
            || record.process_start_token != start_token
            || record.client_tty != row.tty
            || uid_record.host_uid != record.host_uid
            || uid_record.client_uid != record.client_uid
            || uid_record.backend_instance_uid != record.backend_instance_uid
            || uid_record.server_epoch != record.server_epoch
            || uid_record.client_pid != record.client_pid
            || uid_record.process_start_token != record.process_start_token
            || uid_record.client_tty != record.client_tty
            || uid_record.recorded_at != record.recorded_at
        {
            return Err(TypedError::new(
                ErrorCode::IdentityConflict,
                "affected tmux peer record differs from its exact live row/UID record",
            ));
        }
        emit_context_to_exact_client(&uid_record, &expected.namespace, &row, &marker)?;
        published += 1;
    }
    if published == 0 {
        return Err(TypedError::new(
            ErrorCode::PostconditionFailed,
            "session context refresh published no correlated tmux clients",
        ));
    }
    correlate_client(runtime_dir, invoking_client_uid, expected)?;
    Ok((marker, published))
}

/// Switch only the pre-correlated client, then repeat the complete process
/// and list-clients proof and require the exact destination session as the
/// postcondition.
pub fn switch_correlated_client(
    runtime_dir: &Path,
    client_uid: Uuid,
    from: &TmuxClientTarget,
    to: &TmuxClientTarget,
) -> Result<CorrelatedTmuxClient, TypedError> {
    if from.host_uid != to.host_uid
        || from.backend_instance_uid != to.backend_instance_uid
        || from.server_epoch != to.server_epoch
        || from.namespace != to.namespace
    {
        return Err(TypedError::new(
            ErrorCode::WrongBackendInstance,
            "tmux client switch cannot cross owner/backend instance/server epoch",
        ));
    }
    let mut record = read_client_record(runtime_dir, client_uid)?;
    if record.host_uid != from.host_uid
        || record.backend_instance_uid != from.backend_instance_uid
        || record.server_epoch != from.server_epoch
    {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "tmux client UID does not belong to this exact switch incarnation",
        ));
    }
    let before = live_client_for_record(&record, &from.namespace)?;
    if before.session == from.native_session {
        if !active_child_matches(from, &before) {
            return Err(TypedError::new(
                ErrorCode::IdentityConflict,
                "GUI marker is not the exact correlated client's active tmux window/pane",
            ));
        }
        let output = Command::new("tmux")
            .args([
                "-L",
                &from.namespace,
                "switch-client",
                "-c",
                &record.client_tty,
                "-t",
                &to.native_session,
            ])
            .env("LC_ALL", "C.UTF-8")
            .stdin(Stdio::null())
            .output()
            .map_err(|error| io_error("running exact tmux switch-client", error))?;
        if !output.status.success() {
            return Err(TypedError::new(
                ErrorCode::OperationFailed,
                format!(
                    "tmux switch-client failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ));
        }
    } else if before.session != to.native_session {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "tmux client moved to a third Space during presentation",
        ));
    }
    let after = live_client_for_record(&record, &from.namespace)?;
    if after.session != to.native_session || !active_child_matches(to, &after) {
        return Err(TypedError::new(
            ErrorCode::PostconditionFailed,
            "tmux switch-client did not leave the exact client on an owner-authorized destination child",
        ));
    }
    record.space_uid = to.space_uid;
    replace_record(runtime_dir, &record)?;
    publish_correlated_client_context(runtime_dir, client_uid, to)?;
    correlate_client(runtime_dir, client_uid, to)
}

/// Detach only the exact UID-correlated client. Returns `true` when this
/// call performed the native detach and `false` when a lost-ack retry found
/// the exact process no longer represented by any live client row. The
/// owner pane inventory and every other client row are exact postconditions.
pub fn detach_correlated_client(
    runtime_dir: &Path,
    client_uid: Uuid,
    expected: &TmuxClientTarget,
) -> Result<bool, TypedError> {
    let record = read_client_record(runtime_dir, client_uid)?;
    if record.host_uid != expected.host_uid
        || record.backend_instance_uid != expected.backend_instance_uid
        || record.server_epoch != expected.server_epoch
    {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "tmux detach UID belongs to a different owner/backend incarnation",
        ));
    }
    let current_start = match process_start_token(record.client_pid) {
        Ok(token) => Some(token),
        Err(error) if error.code == ErrorCode::ProviderUnavailable => None,
        Err(error) => return Err(error),
    };
    if current_start
        .as_ref()
        .is_some_and(|token| token != &record.process_start_token)
    {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "tmux detach client PID was reused",
        ));
    }
    let mut before_clients = list_clients(&expected.namespace)?;
    let matches: Vec<_> = before_clients
        .iter()
        .filter(|row| row.pid == record.client_pid && row.tty == record.client_tty)
        .cloned()
        .collect();
    if matches.is_empty() {
        // Idempotent lost-ack reconciliation: absence is accepted only for
        // the exact persisted PID+tty; no different row is selected.
        return Ok(false);
    }
    let [client] = matches.as_slice() else {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "tmux detach record matches multiple live clients",
        ));
    };
    if current_start.is_none()
        || client.session != expected.native_session
        || !active_child_matches(expected, client)
    {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "tmux detach marker is not the exact client's active session/window/pane",
        ));
    }
    let before_panes = pane_inventory(&expected.namespace)?;
    let output = Command::new("tmux")
        .args([
            "-L",
            &expected.namespace,
            "detach-client",
            "-t",
            &record.client_tty,
        ])
        .env("LC_ALL", "C.UTF-8")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| io_error("detaching exact tmux client", error))?;
    if !output.status.success() {
        return Err(TypedError::new(
            ErrorCode::OperationFailed,
            format!(
                "tmux detach-client failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    before_clients.retain(|row| !(row.pid == record.client_pid && row.tty == record.client_tty));
    sort_client_rows(&mut before_clients);
    let mut after_clients = list_clients(&expected.namespace)?;
    sort_client_rows(&mut after_clients);
    if after_clients != before_clients {
        return Err(TypedError::new(
            ErrorCode::PostconditionFailed,
            "tmux detach changed another client or left the exact client attached",
        ));
    }
    if pane_inventory(&expected.namespace)? != before_panes {
        return Err(TypedError::new(
            ErrorCode::PostconditionFailed,
            "tmux client detach changed the owner pane inventory",
        ));
    }
    Ok(true)
}

fn sort_client_rows(rows: &mut [ClientRow]) {
    rows.sort_by(|a, b| {
        (&a.pid, &a.tty, &a.name, &a.session, &a.window, &a.pane)
            .cmp(&(&b.pid, &b.tty, &b.name, &b.session, &b.window, &b.pane))
    });
}

fn pane_inventory(namespace: &str) -> Result<Vec<String>, TypedError> {
    let output = Command::new("tmux")
        .args([
            "-L",
            namespace,
            "list-panes",
            "-a",
            "-F",
            ACTIVE_CONTEXT_FORMAT,
        ])
        .env("LC_ALL", "C.UTF-8")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| io_error("listing tmux pane inventory", error))?;
    if !output.status.success() {
        return Err(TypedError::new(
            ErrorCode::ProviderUnavailable,
            format!(
                "tmux list-panes failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    let stdout = std::str::from_utf8(&output.stdout).map_err(|_| {
        TypedError::new(
            ErrorCode::OperationFailed,
            "tmux pane inventory is not UTF-8",
        )
    })?;
    let mut rows = Vec::new();
    for line in stdout.lines().filter(|line| !line.is_empty()) {
        let fields: Vec<_> = line.split(TMUX_FORMAT_SEPARATOR).collect();
        let [session, window, pane] = fields.as_slice() else {
            return Err(TypedError::new(
                ErrorCode::OperationFailed,
                "tmux pane inventory returned a malformed row",
            ));
        };
        parse_canonical_native_id(session, "$")?;
        parse_canonical_native_id(window, "@")?;
        parse_canonical_native_id(pane, "%")?;
        rows.push(line.to_string());
    }
    rows.sort();
    Ok(rows)
}

/// Refresh the outer Wez marker for one exact tmux hook client. The hook's
/// PID is a deterministic locator only: its current `/bin/ps` start token
/// selects the private hard-link path, then every persisted and live field
/// is independently revalidated. No directory scan, tty-only match, client
/// ordinal, or cross-host fallback exists.
pub fn refresh_controller_context_from_tmux_hook(
    env: &OperationEnv,
    claim: &TmuxHookClientClaim,
) -> Result<MarkerContext, TypedError> {
    validate_hook_claim(claim)?;
    let start_token = process_start_token(claim.client_pid)?;
    let record = read_process_client_record(&env.lock_dir, claim.client_pid, &start_token)?;
    let uid_record = read_client_record(&env.lock_dir, record.client_uid)?;
    if record.host_uid != uid_record.host_uid
        || record.backend_instance_uid != uid_record.backend_instance_uid
        || record.server_epoch != uid_record.server_epoch
        || record.client_pid != uid_record.client_pid
        || record.process_start_token != uid_record.process_start_token
        || record.client_tty != uid_record.client_tty
        || record.recorded_at != uid_record.recorded_at
        || record.client_tty != claim.client_tty
    {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "deterministic tmux process record differs from its UID record/hook claim",
        ));
    }

    let mut locks = OrderedLocks::new(&env.lock_dir);
    if !locks
        .try_acquire(LockScope::AuthorityGate, LockMode::Shared)
        .map_err(|error| {
            TypedError::new(
                ErrorCode::OperationFailed,
                format!("tmux hook authority fence: {error}"),
            )
        })?
    {
        return Err(TypedError::new(
            ErrorCode::OperationInProgress,
            "tmux hook refresh skipped during authority maintenance",
        ));
    }
    if !locks
        .try_acquire(
            LockScope::BackendInstance(record.backend_instance_uid),
            LockMode::Shared,
        )
        .map_err(|error| {
            TypedError::new(
                ErrorCode::OperationFailed,
                format!("tmux hook backend fence: {error}"),
            )
        })?
    {
        return Err(TypedError::new(
            ErrorCode::OperationInProgress,
            "tmux hook refresh skipped while the backend is mutating or recovering",
        ));
    }
    let registry = Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir))
        .map_err(|error| TypedError::new(error.error_code(), format!("registry: {error}")))?;
    let identity = registry
        .identity()
        .map_err(|error| TypedError::new(error.error_code(), format!("identity: {error}")))?;
    let info = registry
        .backend_instance_info(record.backend_instance_uid)
        .map_err(|error| TypedError::new(error.error_code(), format!("instance: {error}")))?;
    if identity.host_uid != record.host_uid
        || info.owner != record.host_uid
        || info.backend != Backend::Tmux
        || info.socket_path.as_deref() != Some(claim.namespace.as_str())
    {
        return Err(TypedError::new(
            ErrorCode::WrongBackendInstance,
            "tmux hook client does not belong to this exact registered owner/namespace",
        ));
    }
    let published = registry
        .backend_server(record.backend_instance_uid)
        .map_err(|error| TypedError::new(error.error_code(), format!("server record: {error}")))?;
    if published.server_epoch != Some(record.server_epoch) {
        return Err(TypedError::new(
            ErrorCode::BackendEpochChanged,
            "tmux hook client record belongs to a stale server epoch",
        ));
    }
    let expected_identity = TmuxServerIdentity {
        pid: published
            .server_pid
            .and_then(|pid| u32::try_from(pid).ok())
            .ok_or_else(|| {
                TypedError::new(
                    ErrorCode::BackendEpochChanged,
                    "tmux hook backend has no published server PID",
                )
            })?,
        start_token: published.server_start_token.ok_or_else(|| {
            TypedError::new(
                ErrorCode::BackendEpochChanged,
                "tmux hook backend has no published server start token",
            )
        })?,
    };
    TmuxProvider::new(claim.namespace.clone())
        .verify_epoch(&claim.namespace, record.server_epoch, &expected_identity)
        .map_err(|error| match error {
            crate::backend::ProviderError::EpochChanged { .. }
            | crate::backend::ProviderError::WrongInstance { .. } => TypedError::new(
                ErrorCode::BackendEpochChanged,
                "tmux hook backend restarted before marker refresh",
            ),
            other => TypedError::new(
                ErrorCode::ProviderUnavailable,
                format!("tmux hook backend probe failed: {other:?}"),
            ),
        })?;

    let row = live_client_for_record(&record, &claim.namespace)?;
    if row.name != claim.hook_client
        || row.pid != claim.client_pid
        || row.tty != claim.client_tty
        || row.session != claim.session_id
        || row.window != claim.window_id
        || row.pane != claim.pane_id
    {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "tmux hook facts differ from the exact live client row",
        ));
    }

    let mut matched = None;
    for space in registry.spaces().map_err(|error| {
        TypedError::new(error.error_code(), format!("listing tmux Spaces: {error}"))
    })? {
        if space.lifecycle != Lifecycle::Active
            || space.backend_instance != record.backend_instance_uid
        {
            continue;
        }
        let binding = registry.current_binding(space.space_uid).map_err(|error| {
            TypedError::new(error.error_code(), format!("reading tmux binding: {error}"))
        })?;
        if binding
            .as_ref()
            .map(|binding| binding.native_token.as_str())
            == Some(row.session.as_str())
        {
            if matched.replace(space).is_some() {
                return Err(TypedError::new(
                    ErrorCode::IdentityConflict,
                    "live tmux client session maps to multiple current Space bindings",
                ));
            }
        }
    }
    let space = matched.ok_or_else(|| {
        TypedError::new(
            ErrorCode::SpaceAbsent,
            "live tmux client session has no active current Space binding",
        )
    })?;
    if space.owner != record.host_uid {
        return Err(TypedError::new(
            ErrorCode::HostIdentityChanged,
            "tmux hook Space owner differs from its client record",
        ));
    }
    let target = TmuxClientTarget {
        host_uid: record.host_uid,
        space_uid: space.space_uid,
        space_no: space.space_no,
        backend_instance_uid: record.backend_instance_uid,
        server_epoch: record.server_epoch,
        namespace: claim.namespace.clone(),
        native_session: row.session.clone(),
        active_children: vec![TmuxClientChildTarget {
            window: row.window.clone(),
            pane: row.pane.clone(),
        }],
    };
    let marker = marker_for_client_row(&target, &row);
    emit_context_to_exact_client(&record, &claim.namespace, &row, &marker)?;
    Ok(marker)
}

fn validate_hook_claim(claim: &TmuxHookClientClaim) -> Result<(), TypedError> {
    if claim.client_pid == 0
        || claim.namespace.is_empty()
        || claim.namespace.chars().any(char::is_control)
        || claim.hook_client.is_empty()
        || claim.hook_client.chars().any(char::is_control)
        || !claim.client_tty.starts_with("/dev/")
        || claim.client_tty.chars().any(char::is_control)
    {
        return Err(TypedError::new(
            ErrorCode::InvalidRef,
            "tmux hook client identity is empty or malformed",
        ));
    }
    parse_canonical_native_id(&claim.session_id, "$")?;
    parse_canonical_native_id(&claim.window_id, "@")?;
    parse_canonical_native_id(&claim.pane_id, "%")?;
    Ok(())
}

/// Resolve the only native child pairs that an exact correlated-client
/// action may publish. The hierarchy is already owner-fenced and epoch
/// qualified; this helper additionally rejects malformed refs, duplicate
/// native pairs, missing requested children, and empty selections.
pub fn tmux_client_children_from_hierarchy(
    hierarchy: &SpaceHierarchy,
    group_ref: Option<&str>,
    split_ref: Option<&str>,
) -> Result<Vec<TmuxClientChildTarget>, TypedError> {
    if split_ref.is_some() && group_ref.is_none() {
        return Err(TypedError::new(
            ErrorCode::InvalidRef,
            "an exact tmux Split target requires its Group parent",
        ));
    }

    let parse_child = |value: &str, kind: ChildKind| -> Result<ChildRefShape, TypedError> {
        let parsed = parse_ref(&format!("1/{value}")).map_err(|error| {
            TypedError::new(
                ErrorCode::InvalidRef,
                format!("malformed tmux child ref {value:?}: {error:?}"),
            )
        })?;
        let child = parsed.child.ok_or_else(|| {
            TypedError::new(ErrorCode::InvalidRef, "tmux child ref omitted its suffix")
        })?;
        if child.kind != kind || child.epoch != hierarchy.server_epoch {
            return Err(TypedError::new(
                ErrorCode::BackendEpochChanged,
                "tmux child ref kind/epoch differs from the live hierarchy",
            ));
        }
        Ok(child)
    };

    let mut matched_group = group_ref.is_none();
    let mut matched_split = split_ref.is_none();
    let mut children = Vec::new();
    for group in &hierarchy.groups {
        let group_child = parse_child(&group.group_ref, ChildKind::Group)?;
        let ProviderHandle::Tx(window) = group_child.handle else {
            return Err(TypedError::new(
                ErrorCode::InvalidRef,
                "tmux hierarchy carries a non-tmux Group handle",
            ));
        };
        if group_ref.is_some_and(|expected| expected != group.group_ref) {
            continue;
        }
        matched_group = true;
        for split in &group.splits {
            let split_child = parse_child(&split.split_ref, ChildKind::Split)?;
            let ProviderHandle::Tx(pane) = split_child.handle else {
                return Err(TypedError::new(
                    ErrorCode::InvalidRef,
                    "tmux hierarchy carries a non-tmux Split handle",
                ));
            };
            if split_ref.is_some_and(|expected| expected != split.split_ref) {
                continue;
            }
            matched_split = true;
            children.push(TmuxClientChildTarget {
                window: format!("@{window}"),
                pane: format!("%{pane}"),
            });
        }
    }
    if !matched_group || !matched_split || children.is_empty() {
        return Err(TypedError::new(
            ErrorCode::NotFound,
            "requested tmux Group/Split is absent from the complete live hierarchy",
        ));
    }
    children.sort();
    let original_len = children.len();
    children.dedup();
    if children.len() != original_len {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "tmux hierarchy maps multiple logical children to one native window/pane",
        ));
    }
    Ok(children)
}

fn active_child_matches(target: &TmuxClientTarget, row: &ClientRow) -> bool {
    !target.active_children.is_empty()
        && target
            .active_children
            .iter()
            .any(|child| child.window == row.window && child.pane == row.pane)
}

fn marker_for_client_row(target: &TmuxClientTarget, row: &ClientRow) -> MarkerContext {
    let window = row
        .window
        .strip_prefix('@')
        .and_then(|value| value.parse::<u64>().ok())
        .expect("validated tmux window ID");
    let pane = row
        .pane
        .strip_prefix('%')
        .and_then(|value| value.parse::<u64>().ok())
        .expect("validated tmux pane ID");
    MarkerContext {
        host_uid: target.host_uid,
        space_uid: target.space_uid,
        space_no: target.space_no,
        backend: Backend::Tmux,
        domain: None,
        server_epoch: target.server_epoch,
        group_ref: child_suffix(&ChildRefShape {
            kind: ChildKind::Group,
            epoch: target.server_epoch,
            handle: ProviderHandle::Tx(window),
        }),
        split_ref: child_suffix(&ChildRefShape {
            kind: ChildKind::Split,
            epoch: target.server_epoch,
            handle: ProviderHandle::Tx(pane),
        }),
    }
}

fn emit_context_to_exact_client(
    record: &TmuxClientRecord,
    namespace: &str,
    expected: &ClientRow,
    marker: &MarkerContext,
) -> Result<(), TypedError> {
    let mut terminal = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NOCTTY)
        .open(&record.client_tty)
        .map_err(|error| io_error("opening exact tmux client terminal", error))?;
    let metadata = terminal
        .metadata()
        .map_err(|error| io_error("verifying exact tmux client terminal", error))?;
    if !metadata.file_type().is_char_device() || metadata.uid() != unsafe { libc::geteuid() } {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "tmux client tty is not a current-user-owned character device",
        ));
    }
    let before_emit = live_client_for_record(record, namespace)?;
    if before_emit != *expected {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "tmux client changed while publishing its exact GUI context",
        ));
    }
    emit_tmux_client_uid(&mut terminal, record.client_uid, false)?;
    emit_marker_context(&mut terminal, marker, false)?;
    let after_emit = live_client_for_record(record, namespace)?;
    if after_emit != *expected {
        return Err(TypedError::new(
            ErrorCode::PostconditionFailed,
            "tmux client changed while its GUI context was being published",
        ));
    }
    Ok(())
}

fn live_client_for_record(
    record: &TmuxClientRecord,
    namespace: &str,
) -> Result<ClientRow, TypedError> {
    let observed_start = process_start_token(record.client_pid)?;
    if observed_start != record.process_start_token {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "tmux client PID was reused or changed process identity",
        ));
    }
    let rows = list_clients(namespace)?;
    let mut matches: Vec<_> = rows
        .into_iter()
        .filter(|row| row.pid == record.client_pid && row.tty == record.client_tty)
        .collect();
    if matches.len() != 1 {
        return Err(TypedError::new(
            ErrorCode::ProviderUnavailable,
            format!(
                "tmux client UID matched {} live clients by exact PID+tty; expected one",
                matches.len()
            ),
        ));
    }
    Ok(matches.remove(0))
}

fn replace_record(runtime_dir: &Path, record: &TmuxClientRecord) -> Result<(), TypedError> {
    let dir = ensure_private_record_dir(runtime_dir)?;
    let final_path = client_record_path(runtime_dir, record.client_uid);
    let temp_path = dir.join(format!(
        ".{}.{}.replace",
        record.client_uid,
        Uuid::new_v4().simple()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temp_path)
            .map_err(|error| io_error("creating replacement tmux client record", error))?;
        serde_json::to_writer(&mut file, record).map_err(|error| {
            TypedError::new(
                ErrorCode::OperationFailed,
                format!("serializing replacement tmux client record: {error}"),
            )
        })?;
        file.sync_all()
            .map_err(|error| io_error("syncing replacement tmux client record", error))?;
        let metadata = file
            .metadata()
            .map_err(|error| io_error("verifying replacement tmux client record", error))?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o777 != 0o600
        {
            return Err(TypedError::new(
                ErrorCode::OperationFailed,
                "replacement tmux client record failed private-mode verification",
            ));
        }
        fs::rename(&temp_path, &final_path)
            .map_err(|error| io_error("atomically replacing tmux client record", error))?;
        let metadata = fs::symlink_metadata(&final_path)
            .map_err(|error| io_error("verifying replaced tmux client record", error))?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o777 != 0o600
        {
            return Err(TypedError::new(
                ErrorCode::OperationFailed,
                "replaced tmux client record failed post-publish verification",
            ));
        }
        Ok(())
    })();
    let _ = fs::remove_file(&temp_path);
    result
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClientRow {
    pid: u32,
    tty: String,
    name: String,
    session: String,
    window: String,
    pane: String,
}

fn list_clients(namespace: &str) -> Result<Vec<ClientRow>, TypedError> {
    if namespace.is_empty() || namespace.chars().any(char::is_control) {
        return Err(TypedError::new(
            ErrorCode::WrongBackendInstance,
            "tmux namespace is empty or contains control characters",
        ));
    }
    let output = Command::new("tmux")
        .args(["-L", namespace, "list-clients", "-F", CLIENT_LIST_FORMAT])
        .env("LC_ALL", "C.UTF-8")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| io_error("listing exact tmux clients", error))?;
    if !output.status.success() {
        return Err(TypedError::new(
            ErrorCode::ProviderUnavailable,
            format!(
                "tmux list-clients failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    let stdout = std::str::from_utf8(&output.stdout).map_err(|_| {
        TypedError::new(
            ErrorCode::OperationFailed,
            "tmux list-clients output is not UTF-8",
        )
    })?;
    let mut rows = Vec::new();
    for line in stdout.lines().filter(|line| !line.is_empty()) {
        let fields: Vec<_> = line.split(TMUX_FORMAT_SEPARATOR).collect();
        let [pid, tty, name, session, window, pane] = fields.as_slice() else {
            return Err(TypedError::new(
                ErrorCode::OperationFailed,
                "tmux list-clients returned a malformed row",
            ));
        };
        let pid = pid.parse::<u32>().map_err(|_| {
            TypedError::new(
                ErrorCode::OperationFailed,
                "tmux list-clients returned a malformed client PID",
            )
        })?;
        if pid == 0
            || !tty.starts_with("/dev/")
            || tty.chars().any(char::is_control)
            || name.is_empty()
            || name.chars().any(char::is_control)
            || session.is_empty()
            || session.chars().any(char::is_control)
            || !window.starts_with('@')
            || window[1..].is_empty()
            || !window[1..].bytes().all(|byte| byte.is_ascii_digit())
            || !pane.starts_with('%')
            || pane[1..].is_empty()
            || !pane[1..].bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(TypedError::new(
                ErrorCode::OperationFailed,
                "tmux list-clients returned an invalid identity field",
            ));
        }
        rows.push(ClientRow {
            pid,
            tty: (*tty).to_string(),
            name: (*name).to_string(),
            session: (*session).to_string(),
            window: (*window).to_string(),
            pane: (*pane).to_string(),
        });
    }
    Ok(rows)
}

fn process_start_token(pid: u32) -> Result<String, TypedError> {
    let output = Command::new("/bin/ps")
        .env_clear()
        .env("LC_ALL", "C")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| io_error("reading tmux client process identity", error))?;
    if !output.status.success() {
        return Err(TypedError::new(
            ErrorCode::ProviderUnavailable,
            format!("tmux client process {pid} is no longer alive"),
        ));
    }
    let token = std::str::from_utf8(&output.stdout)
        .map_err(|_| {
            TypedError::new(
                ErrorCode::OperationFailed,
                "tmux client process start token is not UTF-8",
            )
        })?
        .trim()
        .to_string();
    if token.is_empty() || token.chars().any(char::is_control) {
        return Err(TypedError::new(
            ErrorCode::ProviderUnavailable,
            format!("tmux client process {pid} has no exact start token"),
        ));
    }
    Ok(token)
}

fn current_tty() -> Result<String, TypedError> {
    let tty_bin = if Path::new("/usr/bin/tty").exists() {
        "/usr/bin/tty"
    } else {
        "/bin/tty"
    };
    let output = Command::new(tty_bin)
        .stdin(Stdio::inherit())
        .output()
        .map_err(|error| io_error("identifying attach PTY", error))?;
    let tty = std::str::from_utf8(&output.stdout)
        .map_err(|_| TypedError::new(ErrorCode::OperationFailed, "attach PTY is not UTF-8"))?
        .trim()
        .to_string();
    if !output.status.success() || !tty.starts_with("/dev/") || tty.chars().any(char::is_control) {
        return Err(TypedError::new(
            ErrorCode::ProviderUnavailable,
            "tmux client correlation requires one exact controlling PTY",
        ));
    }
    Ok(tty)
}

/// Encode the hook-published client UID as a Wez SetUserVar. The value uses
/// standard padded base64; if a caller is already inside tmux, the frozen
/// DCS passthrough recipe doubles each ESC before the outer ST.
pub fn emit_tmux_client_uid(
    writer: &mut dyn Write,
    client_uid: Uuid,
    in_tmux: bool,
) -> Result<(), TypedError> {
    emit_user_var(
        writer,
        "dmux_tmux_client_uid",
        &client_uid.to_string(),
        in_tmux,
    )
    .and_then(|()| writer.flush())
    .map_err(|error| io_error("stamping tmux client UID into the outer Wez pane", error))
}

/// Stamp a complete owner-resolved context only after the managed tmux hook
/// has proved the exact client is live and attached. Pre-exec attach paths
/// deliberately never call this helper.
fn emit_marker_context(
    writer: &mut dyn Write,
    marker: &MarkerContext,
    in_tmux: bool,
) -> Result<(), TypedError> {
    let values = [
        ("dmux_context_version", "1".to_string()),
        ("dmux_host_uid", marker.host_uid.0.to_string()),
        ("dmux_space_uid", marker.space_uid.0.to_string()),
        ("dmux_space_no", marker.space_no.to_string()),
        ("dmux_backend", marker.backend.as_str().to_string()),
        ("dmux_domain", marker.domain.clone().unwrap_or_default()),
        ("dmux_server_epoch", marker.server_epoch.0.to_string()),
        ("dmux_group_ref", marker.group_ref.clone()),
        ("dmux_split_ref", marker.split_ref.clone()),
    ];
    for (name, value) in values {
        emit_user_var(writer, name, &value, in_tmux)
            .map_err(|error| io_error("stamping initial tmux marker", error))?;
    }
    writer
        .flush()
        .map_err(|error| io_error("flushing initial tmux marker", error))
}

fn emit_user_var(writer: &mut dyn Write, name: &str, value: &str, in_tmux: bool) -> io::Result<()> {
    let osc = format!(
        "\x1b]1337;SetUserVar={name}={}\x07",
        base64(value.as_bytes())
    );
    let encoded = if in_tmux {
        format!("\x1bPtmux;{}\x1b\\", osc.replace('\x1b', "\x1b\x1b"))
    } else {
        osc
    };
    writer.write_all(encoded.as_bytes())
}

fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(chunk.get(1).copied().unwrap_or(0)) << 8)
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn io_error(context: &str, error: io::Error) -> TypedError {
    TypedError::new(ErrorCode::OperationFailed, format!("{context}: {error}"))
}

fn lifecycle_token(lifecycle: Lifecycle) -> &'static str {
    match lifecycle {
        Lifecycle::Reserved => "reserved",
        Lifecycle::Active => "active",
        Lifecycle::Deleting => "deleting",
        Lifecycle::Deleted => "deleted",
        Lifecycle::Conflict => "conflict",
        Lifecycle::Aborted => "aborted",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::{Duration, Instant};

    struct ScratchTmux {
        namespace: String,
        children: Vec<std::process::Child>,
    }

    impl ScratchTmux {
        fn new() -> Self {
            let namespace = format!("dmux-client-test-{}", Uuid::new_v4().simple());
            for session in ["one", "two"] {
                let output = Command::new("tmux")
                    .args(["-L", &namespace, "new-session", "-d", "-s", session])
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "starting scratch tmux: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            Self {
                namespace,
                children: Vec::new(),
            }
        }

        fn attach(&mut self, session: &str) {
            let mut command = Command::new("script");
            command
                .env("DMUX_CLIENT_TEST_NAMESPACE", &self.namespace)
                .env("DMUX_CLIENT_TEST_SESSION", session)
                .env("TERM", "xterm-256color");
            #[cfg(target_os = "macos")]
            command.args([
                "-q",
                "/dev/null",
                "env",
                "sh",
                "-c",
                "exec tmux -L \"$DMUX_CLIENT_TEST_NAMESPACE\" attach-session -t \"$DMUX_CLIENT_TEST_SESSION\"",
            ]);
            #[cfg(target_os = "linux")]
            command.args([
                "-q",
                "-c",
                "exec tmux -L \"$DMUX_CLIENT_TEST_NAMESPACE\" attach-session -t \"$DMUX_CLIENT_TEST_SESSION\"",
                "/dev/null",
            ]);
            let child = command
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            self.children.push(child);
            let expected = self.children.len();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if list_clients(&self.namespace).is_ok_and(|rows| rows.len() >= expected) {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "scratch tmux client did not attach"
                );
                thread::sleep(Duration::from_millis(20));
            }
        }

        fn session_id(&self, session: &str) -> String {
            let output = Command::new("tmux")
                .args([
                    "-L",
                    &self.namespace,
                    "display-message",
                    "-p",
                    "-t",
                    session,
                    "#{session_id}",
                ])
                .output()
                .unwrap();
            assert!(output.status.success());
            String::from_utf8(output.stdout).unwrap().trim().to_string()
        }
    }

    impl Drop for ScratchTmux {
        fn drop(&mut self) {
            let _ = Command::new("tmux")
                .args(["-L", &self.namespace, "kill-server"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            for child in &mut self.children {
                let _ = child.wait();
            }
        }
    }

    fn ids() -> (HostUid, BackendInstanceUid, ServerEpoch, SpaceUid, SpaceUid) {
        (
            HostUid(Uuid::new_v4()),
            BackendInstanceUid(Uuid::new_v4()),
            ServerEpoch(Uuid::new_v4()),
            SpaceUid(Uuid::new_v4()),
            SpaceUid(Uuid::new_v4()),
        )
    }

    fn record_for(
        row: &ClientRow,
        client_uid: Uuid,
        host_uid: HostUid,
        instance: BackendInstanceUid,
        epoch: ServerEpoch,
        space_uid: SpaceUid,
    ) -> TmuxClientRecord {
        TmuxClientRecord {
            record_version: CLIENT_RECORD_VERSION,
            client_uid,
            host_uid,
            space_uid,
            backend_instance_uid: instance,
            server_epoch: epoch,
            client_pid: row.pid,
            process_start_token: process_start_token(row.pid).unwrap(),
            client_tty: row.tty.clone(),
            recorded_at: now_rfc3339(),
        }
    }

    #[test]
    fn client_uid_osc_is_standard_base64_and_tmux_passthrough_exact() {
        let uid = Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap();
        let value = base64(uid.to_string().as_bytes());
        let raw = format!("\x1b]1337;SetUserVar=dmux_tmux_client_uid={value}\x07");
        let mut direct = Vec::new();
        emit_tmux_client_uid(&mut direct, uid, false).unwrap();
        assert_eq!(direct, raw.as_bytes());

        let mut wrapped = Vec::new();
        emit_tmux_client_uid(&mut wrapped, uid, true).unwrap();
        assert_eq!(
            wrapped,
            format!("\x1bPtmux;{}\x1b\\", raw.replace('\x1b', "\x1b\x1b")).as_bytes()
        );
    }

    #[test]
    fn correlation_reservation_modes_never_require_a_pre_exec_marker_publish() {
        let existing = Uuid::new_v4();
        let remote = Uuid::new_v4();
        assert_eq!(
            controller_reservation_mode(TmuxExecKind::LocalAttach, true, false, None, None,)
                .unwrap(),
            ControllerReservationMode::FreshLocal
        );
        assert_eq!(
            controller_reservation_mode(
                TmuxExecKind::RemoteAttach,
                true,
                false,
                None,
                Some(remote),
            )
            .unwrap(),
            ControllerReservationMode::Remote(remote)
        );
        assert_eq!(
            controller_reservation_mode(
                TmuxExecKind::LocalSwitch,
                true,
                true,
                Some(existing),
                None,
            )
            .unwrap(),
            ControllerReservationMode::ExistingLocal(existing)
        );
        assert_eq!(
            controller_reservation_mode(TmuxExecKind::LocalSwitch, true, true, None, None,)
                .unwrap(),
            ControllerReservationMode::None
        );
        assert_eq!(
            controller_reservation_mode(TmuxExecKind::LocalAttach, false, false, None, None,)
                .unwrap(),
            ControllerReservationMode::None
        );
        assert!(
            controller_reservation_mode(TmuxExecKind::LocalAttach, true, true, None, None,)
                .is_err()
        );
    }

    #[test]
    fn hook_marker_is_owner_resolved_and_fully_encoded() {
        let tmux = ScratchTmux::new();
        let (host, _, epoch, space, _) = ids();
        let session = tmux.session_id("one");
        let marker = resolve_current_tmux_marker(
            host,
            space,
            SpaceNo(std::num::NonZeroU64::new(7).unwrap()),
            epoch,
            &tmux.namespace,
            &session,
            None,
        )
        .unwrap();
        assert_eq!(marker.backend, Backend::Tmux);
        assert_eq!(marker.domain, None);
        assert!(marker.group_ref.starts_with(&format!("g{}.", epoch.0)));
        assert!(marker.split_ref.starts_with(&format!("p{}.", epoch.0)));

        let argv = vec![
            "tmux".to_string(),
            "-L".to_string(),
            tmux.namespace.clone(),
            "select-window".to_string(),
            "-t".to_string(),
            format!(
                "{session}:{}",
                marker
                    .group_ref
                    .split('.')
                    .nth(1)
                    .unwrap()
                    .replacen("tx-", "@", 1)
            ),
            ";".to_string(),
            "select-pane".to_string(),
            "-t".to_string(),
            marker
                .split_ref
                .split('.')
                .nth(1)
                .unwrap()
                .replacen("tx-", "%", 1),
            ";".to_string(),
            "attach-session".to_string(),
            "-t".to_string(),
            session.clone(),
        ];
        let child = parse_recorded_attach_child(&argv, &tmux.namespace, &session, epoch).unwrap();
        assert!(matches!(child, Some(VerifiedConnectChild::Split { .. })));

        let mut wire = Vec::new();
        emit_marker_context(&mut wire, &marker, false).unwrap();
        let wire = String::from_utf8(wire).unwrap();
        for field in [
            "dmux_context_version",
            "dmux_host_uid",
            "dmux_space_uid",
            "dmux_space_no",
            "dmux_backend",
            "dmux_domain",
            "dmux_server_epoch",
            "dmux_group_ref",
            "dmux_split_ref",
        ] {
            assert!(wire.contains(&format!("SetUserVar={field}=")));
        }
    }

    #[test]
    fn canceled_fresh_reservation_removes_only_its_exact_record_links() {
        let mut tmux = ScratchTmux::new();
        tmux.attach("one");
        let row = list_clients(&tmux.namespace).unwrap().remove(0);
        let runtime = tempfile::tempdir().unwrap();
        let (host, instance, epoch, space, _) = ids();
        let client_uid = Uuid::new_v4();
        let record = record_for(&row, client_uid, host, instance, epoch, space);
        publish_record(runtime.path(), &record).unwrap();
        let uid_path = client_record_path(runtime.path(), client_uid);
        let process_path = client_process_record_path(
            runtime.path(),
            record.client_pid,
            &record.process_start_token,
        );
        let reservation = ControllerCorrelationReservation {
            client_uid,
            local_record: Some((runtime.path().to_path_buf(), record)),
        };

        assert!(cancel_controller_correlation_reservation(&reservation).unwrap());
        assert!(!uid_path.exists());
        assert!(!process_path.exists());
    }

    #[test]
    fn exact_pid_start_tty_switches_only_registered_scratch_client() {
        let mut tmux = ScratchTmux::new();
        tmux.attach("one");
        tmux.attach("one");
        let runtime = tempfile::tempdir().unwrap();
        let rows = list_clients(&tmux.namespace).unwrap();
        assert_eq!(rows.len(), 2);
        let chosen = &rows[0];
        let untouched = &rows[1];
        let (host, instance, epoch, one_space, two_space) = ids();
        let one_session = tmux.session_id("one");
        let two_session = tmux.session_id("two");
        let uid = Uuid::new_v4();
        let record = record_for(chosen, uid, host, instance, epoch, one_space);
        publish_record(runtime.path(), &record).unwrap();

        let path = client_record_path(runtime.path(), uid);
        let process_path = client_process_record_path(
            runtime.path(),
            record.client_pid,
            &record.process_start_token,
        );
        let metadata = fs::symlink_metadata(&path).unwrap();
        assert_eq!(metadata.mode() & 0o777, 0o600);
        let process_metadata = fs::symlink_metadata(&process_path).unwrap();
        assert_eq!(process_metadata.mode() & 0o777, 0o600);
        assert_eq!(metadata.ino(), process_metadata.ino());
        assert_eq!(
            fs::symlink_metadata(client_record_dir(runtime.path()))
                .unwrap()
                .mode()
                & 0o777,
            0o700
        );
        assert!(publish_record(runtime.path(), &record).is_err());
        assert_eq!(read_client_record(runtime.path(), uid).unwrap(), record);

        let from = TmuxClientTarget {
            host_uid: host,
            space_uid: one_space,
            space_no: SpaceNo(std::num::NonZeroU64::new(1).unwrap()),
            backend_instance_uid: instance,
            server_epoch: epoch,
            namespace: tmux.namespace.clone(),
            native_session: one_session.clone(),
            active_children: vec![TmuxClientChildTarget {
                window: chosen.window.clone(),
                pane: chosen.pane.clone(),
            }],
        };
        let to = TmuxClientTarget {
            space_uid: two_space,
            space_no: SpaceNo(std::num::NonZeroU64::new(2).unwrap()),
            native_session: two_session.clone(),
            active_children: vec![TmuxClientChildTarget {
                window: Command::new("tmux")
                    .args([
                        "-L",
                        &tmux.namespace,
                        "display-message",
                        "-p",
                        "-t",
                        "two",
                        "#{window_id}",
                    ])
                    .output()
                    .unwrap()
                    .stdout
                    .split(|byte| byte.is_ascii_whitespace())
                    .next()
                    .map(|value| String::from_utf8(value.to_vec()).unwrap())
                    .unwrap(),
                pane: Command::new("tmux")
                    .args([
                        "-L",
                        &tmux.namespace,
                        "display-message",
                        "-p",
                        "-t",
                        "two",
                        "#{pane_id}",
                    ])
                    .output()
                    .unwrap()
                    .stdout
                    .split(|byte| byte.is_ascii_whitespace())
                    .next()
                    .map(|value| String::from_utf8(value.to_vec()).unwrap())
                    .unwrap(),
            }],
            ..from.clone()
        };
        correlate_client(runtime.path(), uid, &from).unwrap();
        let switched = switch_correlated_client(runtime.path(), uid, &from, &to).unwrap();
        assert_eq!(switched.current_session, two_session);
        assert_eq!(
            read_client_record(runtime.path(), uid).unwrap().space_uid,
            two_space
        );
        let immutable_process_record: TmuxClientRecord =
            serde_json::from_slice(&fs::read(process_path).unwrap()).unwrap();
        assert_eq!(immutable_process_record.space_uid, one_space);

        let rows = list_clients(&tmux.namespace).unwrap();
        assert_eq!(
            rows.iter()
                .find(|row| row.pid == chosen.pid)
                .unwrap()
                .session,
            two_session
        );
        assert_eq!(
            rows.iter()
                .find(|row| row.pid == untouched.pid)
                .unwrap()
                .session,
            one_session
        );
        // Lost-ack reconciliation is idempotent: the exact client is already
        // on `to`, so no second native transition is needed.
        switch_correlated_client(runtime.path(), uid, &from, &to).unwrap();

        let rows = list_clients(&tmux.namespace).unwrap();
        let active = rows.iter().find(|row| row.pid == chosen.pid).unwrap();
        let exact_to = TmuxClientTarget {
            active_children: vec![TmuxClientChildTarget {
                window: active.window.clone(),
                pane: active.pane.clone(),
            }],
            ..to
        };
        let panes_before = pane_inventory(&tmux.namespace).unwrap();
        assert!(detach_correlated_client(runtime.path(), uid, &exact_to).unwrap());
        assert!(!detach_correlated_client(runtime.path(), uid, &exact_to).unwrap());
        let rows = list_clients(&tmux.namespace).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pid, untouched.pid);
        assert_eq!(pane_inventory(&tmux.namespace).unwrap(), panes_before);
    }

    #[test]
    fn stale_epoch_start_token_and_tty_are_each_terminal() {
        let mut tmux = ScratchTmux::new();
        tmux.attach("one");
        let epoch_runtime = tempfile::tempdir().unwrap();
        let row = list_clients(&tmux.namespace).unwrap().remove(0);
        let (host, instance, epoch, space, _) = ids();
        let target = TmuxClientTarget {
            host_uid: host,
            space_uid: space,
            space_no: SpaceNo(std::num::NonZeroU64::new(1).unwrap()),
            backend_instance_uid: instance,
            server_epoch: epoch,
            namespace: tmux.namespace.clone(),
            native_session: tmux.session_id("one"),
            active_children: vec![TmuxClientChildTarget {
                window: row.window.clone(),
                pane: row.pane.clone(),
            }],
        };

        let epoch_uid = Uuid::new_v4();
        publish_record(
            epoch_runtime.path(),
            &record_for(
                &row,
                epoch_uid,
                host,
                instance,
                ServerEpoch(Uuid::new_v4()),
                space,
            ),
        )
        .unwrap();
        assert_eq!(
            correlate_client(epoch_runtime.path(), epoch_uid, &target)
                .unwrap_err()
                .code,
            ErrorCode::IdentityConflict
        );

        let start_uid = Uuid::new_v4();
        let start_runtime = tempfile::tempdir().unwrap();
        let mut bad_start = record_for(&row, start_uid, host, instance, epoch, space);
        bad_start.process_start_token.push_str(" stale");
        publish_record(start_runtime.path(), &bad_start).unwrap();
        assert_eq!(
            correlate_client(start_runtime.path(), start_uid, &target)
                .unwrap_err()
                .code,
            ErrorCode::IdentityConflict
        );

        let tty_uid = Uuid::new_v4();
        let tty_runtime = tempfile::tempdir().unwrap();
        let mut bad_tty = record_for(&row, tty_uid, host, instance, epoch, space);
        bad_tty.client_tty.push_str("-stale");
        publish_record(tty_runtime.path(), &bad_tty).unwrap();
        assert_eq!(
            correlate_client(tty_runtime.path(), tty_uid, &target)
                .unwrap_err()
                .code,
            ErrorCode::ProviderUnavailable
        );

        let active_uid = Uuid::new_v4();
        let active_runtime = tempfile::tempdir().unwrap();
        publish_record(
            active_runtime.path(),
            &record_for(&row, active_uid, host, instance, epoch, space),
        )
        .unwrap();
        let mut hidden_marker = target.clone();
        hidden_marker.active_children[0].pane = "%999999".to_string();
        assert_eq!(
            correlate_client(active_runtime.path(), active_uid, &hidden_marker)
                .unwrap_err()
                .code,
            ErrorCode::IdentityConflict
        );
    }
}
