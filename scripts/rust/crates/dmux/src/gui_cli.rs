//! Authority-facing controller for the hidden `_gui` command (plan P9).
//!
//! Lua supplies a pane marker only as an untrusted locator.  Every bound
//! command first revalidates that marker against its owner authority and a
//! complete live inventory, then binds it to the exact fresh GUI heartbeat.
//! The bridge remains a presentation plane: owner mutations go through the
//! fenced operations layer (or the owner RPC), and signed bridge requests
//! can only attach/detach/activate/focus/quit.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::backend::wez::{IdentityExpectation, WezProvider};
use crate::backend::{InventoryOutcome, InventoryScope, Provider, ProviderError, SplitDirection};
use crate::bootstrap::MarkerContext;
use crate::connect_cli::{
    FrozenBinding, FrozenConnectTarget, OwnerConnectQuery, OwnerLocator, PresentationMode,
    PresentationReceipt, RequestedChild, VerifiedConnectChild,
};
use crate::error::{ErrorCode, TypedError};
use crate::gui::{
    self, BridgeDomainState, BridgeHeartbeat, BridgeInstanceSelection, BridgePane, BridgeSelection,
    GuiCliOrigin, GuiDomainManifestRow, GuiError, GuiResidentCliOrigin, GuiSpaceRow,
    GuiStatusCache, GuiStatusDisplay, RemoteDomainSource,
};
use crate::history::{GuiHistoryTarget, History, PendingGuiTransition};
use crate::locks::{LockMode, OrderedLocks};
use crate::model::{
    Backend, BackendInstanceUid, ChildKind, Health, HostUid, Lifecycle, ServerEpoch, SpaceNo,
    SpaceUid,
};
use crate::new_cli::{NewPresentationMode, WezPresentationPreflight};
use crate::operations::{
    self, CreateRequest, CreatedChild, CreatedSpace, GroupNewRequest, OperationEnv,
    OwnerCreateTarget, SpaceHierarchy, SplitNewRequest,
};
use crate::policy::{CreationContext, LocalEnv, RemoteEnv, RouteState};
use crate::refs::{
    ChildRefShape, HostToken, ParsedRef, canonical_uri, child_suffix, parse_ref, validate_new_name,
};
use crate::registry::{
    HostLifecycle, HostRow, NetworkClass, Registry, RegistryConfig, RouteRow, Transport,
};
use crate::remote::client::{
    AgentInvocation, PeerExpectation, RouteInvoker, SshInvoker, call_over_pinned_route,
    call_over_routes, request_envelope,
};
use crate::remote::protocol::{
    self, Envelope, GroupActivatePayload, GroupActivateResult, GroupNewPayload, GroupRenamePayload,
    GroupRmPayload, HelloInfo, HelloPayload, HierarchyPayload, NewPayload, RmPayload, SpaceInfo,
    SpacesInfo, SplitDirectionPayload, SplitDirectionResult, SplitNewPayload, SplitResizePayload,
    SplitResizeResult, SplitRmPayload, SplitZoomPayload, SplitZoomResult, TmuxClientDetachPayload,
    TmuxClientDetachResult, TmuxClientRefreshPayload, TmuxClientRefreshResult,
    TmuxClientStatusPayload, TmuxClientStatusResult, TmuxClientSwitchPayload,
    TmuxClientSwitchResult, WezNativePaneWitness, WezNativeTreePayload, WezNativeTreeResult,
};
use crate::resolve::{
    HostContext, RefResolution, SpaceCandidate, SpaceSelector, resolve_enrolled_host,
    resolve_space_ref,
};

pub const GUI_SCHEMA_VERSION: u64 = 1;
const REMOTE_DEADLINE: Duration = Duration::from_secs(30);

/// `_gui` parses its own trailing argv so even usage failures can emit the
/// one frozen JSON response rather than clap's human stderr document.
#[derive(Debug, Parser)]
#[command(name = "dmux _gui", disable_help_subcommand = true)]
struct GuiArgv {
    #[command(subcommand)]
    command: GuiCommand,
}

#[derive(Debug, Clone, Subcommand, PartialEq, Eq)]
pub enum GuiCommand {
    /// Revalidate the active pane and optionally refresh its status cache.
    Context {
        #[arg(long)]
        cache: bool,
    },
    /// List owner-validated live Spaces for the picker.
    Spaces {
        #[arg(long, value_parser = canonical_uuid_arg)]
        tmux_client_uid: Option<Uuid>,
    },
    /// Present one existing Space; never creates on a miss.
    Present {
        #[arg(long)]
        space: String,
        #[arg(long, value_parser = canonical_uuid_arg)]
        tmux_client_uid: Option<Uuid>,
    },
    /// Create a Space on the active marker's exact owner/backend.
    SpaceNew {
        #[arg(long)]
        name: String,
        #[arg(long)]
        dir: Option<String>,
        #[arg(long, value_parser = canonical_uuid_arg)]
        tmux_client_uid: Option<Uuid>,
    },
    GroupNew,
    GroupSelect {
        #[arg(long, value_parser = ["next", "prev", "last"], conflicts_with = "index")]
        relative: Option<String>,
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=9), conflicts_with = "relative")]
        index: Option<u32>,
    },
    GroupRename {
        #[arg(long)]
        name: String,
    },
    GroupRemove {
        #[arg(long)]
        confirmed: bool,
        #[arg(long, requires = "confirmed")]
        escalate_space: bool,
    },
    SplitNew {
        #[arg(long, value_parser = ["left", "right", "up", "down"])]
        direction: String,
    },
    SplitSelect {
        #[arg(long, value_parser = ["left", "right", "up", "down"])]
        direction: String,
    },
    SplitResize {
        #[arg(long, value_parser = ["left", "right", "up", "down"])]
        direction: String,
        #[arg(long, value_parser = clap::value_parser!(u16).range(1..))]
        amount: u16,
    },
    SplitZoom,
    SplitRemove {
        #[arg(long)]
        confirmed: bool,
    },
    Disconnect {
        #[arg(long)]
        domain: bool,
    },
    SafeQuit,
    /// Build the dynamic route manifest. This is the only config-time verb.
    Domains,
    /// Summon the one resident GUI through the cold-launch broker path.
    Summon,
}

impl GuiCommand {
    fn needs_origin(&self) -> bool {
        !matches!(self, GuiCommand::Domains | GuiCommand::Summon)
    }
}

fn canonical_uuid_arg(value: &str) -> Result<Uuid, String> {
    let parsed = Uuid::parse_str(value).map_err(|error| error.to_string())?;
    if parsed.to_string() != value {
        return Err("must be canonical lowercase hyphenated UUID text".to_string());
    }
    Ok(parsed)
}

/// Exact one-document response consumed by the Lua controller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuiResponse {
    pub schema_version: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl GuiResponse {
    pub fn success(result: Value) -> Self {
        GuiResponse {
            schema_version: GUI_SCHEMA_VERSION,
            ok: true,
            result: Some(result),
            error: None,
            message: None,
        }
    }

    pub fn failure(error: TypedError) -> Self {
        Self::failure_with_result(error, None)
    }

    fn failure_with_result(error: TypedError, result: Option<Value>) -> Self {
        GuiResponse {
            schema_version: GUI_SCHEMA_VERSION,
            ok: false,
            result,
            error: Some(error.code.as_str().to_string()),
            message: Some(error.message),
        }
    }

    pub fn exit_code(&self) -> u8 {
        if self.ok {
            return 0;
        }
        self.error
            .as_deref()
            .and_then(error_code_from_token)
            .map(|code| code.exit_status().code())
            .unwrap_or(1)
    }
}

fn error_code_from_token(token: &str) -> Option<ErrorCode> {
    use ErrorCode::*;
    Some(match token {
        "usage" => Usage,
        "invalid_ref" => InvalidRef,
        "invalid_name" => InvalidName,
        "not_found" => NotFound,
        "space_absent" => SpaceAbsent,
        "space_deleted" => SpaceDeleted,
        "ambiguous_target" => AmbiguousTarget,
        "name_conflict" => NameConflict,
        "backend_mismatch" => BackendMismatch,
        "identity_conflict" => IdentityConflict,
        "repair_required" => RepairRequired,
        "operation_in_progress" => OperationInProgress,
        "idempotency_reuse" => IdempotencyReuse,
        "confirmation_required" => ConfirmationRequired,
        "confirmation_declined" => ConfirmationDeclined,
        "provider_unavailable" => ProviderUnavailable,
        "route_unavailable" => RouteUnavailable,
        "bridge_unavailable" => BridgeUnavailable,
        "auth_failed" => AuthFailed,
        "host_identity_changed" => HostIdentityChanged,
        "version_mismatch" => VersionMismatch,
        "protocol_mismatch" => ProtocolMismatch,
        "operation_failed" => OperationFailed,
        "registry_busy" => RegistryBusy,
        "backend_epoch_changed" => BackendEpochChanged,
        "wrong_backend_instance" => WrongBackendInstance,
        "postcondition_failed" => PostconditionFailed,
        "partial_result" => PartialResult,
        _ => return None,
    })
}

/// Test seam: dispatch ordering is independent of registry, ssh, or the
/// spool. A failed `bind_origin` guarantees `execute_bound` is never called.
pub trait GuiAuthority {
    type Bound;

    fn bind_origin(&mut self, origin: &GuiCliOrigin) -> Result<Self::Bound, TypedError>;

    fn execute_bound(
        &mut self,
        bound: &Self::Bound,
        command: &GuiCommand,
    ) -> Result<Value, TypedError>;

    fn execute_unbound(&mut self, command: &GuiCommand) -> Result<Value, TypedError>;

    /// Pane-free application lifecycle requests use the exact resident GUI
    /// process/lease identity. Ordinary authorities keep this unavailable.
    fn execute_resident(
        &mut self,
        _origin: &GuiResidentCliOrigin,
        _command: &GuiCommand,
    ) -> Result<Value, TypedError> {
        Err(TypedError::new(
            ErrorCode::Usage,
            "resident GUI lifecycle origin is unavailable for this authority",
        ))
    }

    /// Only a mutation that durably completed before presentation failed
    /// may populate this. The default keeps all ordinary failures free of a
    /// result document.
    fn take_partial_result(&mut self) -> Option<Value> {
        None
    }
}

/// Parse the exact Lua origin and dispatch. Origin-required actions always
/// bind before reaching an action implementation.
pub fn dispatch<A: GuiAuthority>(
    authority: &mut A,
    origin_json: Option<&str>,
    command: &GuiCommand,
) -> GuiResponse {
    let result = if command.needs_origin() {
        let raw = match origin_json {
            Some(raw) => raw,
            None => {
                return GuiResponse::failure(TypedError::new(
                    ErrorCode::Usage,
                    "this GUI action requires --origin-json",
                ));
            }
        };
        let resident = serde_json::from_str::<Value>(raw)
            .ok()
            .and_then(|value| value.get("kind").and_then(Value::as_str).map(str::to_owned))
            .as_deref()
            == Some("resident_gui");
        if resident {
            if !matches!(command, GuiCommand::SafeQuit) {
                Err(TypedError::new(
                    ErrorCode::Usage,
                    "resident GUI origin is restricted to safe-quit",
                ))
            } else {
                gui::parse_resident_origin_json(raw)
                    .map_err(typed_gui)
                    .and_then(|origin| authority.execute_resident(&origin, command))
            }
        } else {
            gui::parse_origin_json(raw)
                .map_err(typed_gui)
                .and_then(|origin| authority.bind_origin(&origin))
                .and_then(|bound| authority.execute_bound(&bound, command))
        }
    } else if origin_json.is_some() {
        Err(TypedError::new(
            ErrorCode::Usage,
            "domains/summon are config or cold-launch actions and reject --origin-json",
        ))
    } else {
        authority.execute_unbound(command)
    };

    match result {
        Ok(result) => GuiResponse::success(result),
        Err(error) if error.code == ErrorCode::PartialResult => {
            match authority.take_partial_result() {
                Some(partial) => GuiResponse::failure_with_result(error, Some(partial)),
                None => GuiResponse::failure(TypedError::new(
                    ErrorCode::OperationFailed,
                    format!(
                        "partial-result action omitted its required result document: {}",
                        error.message
                    ),
                )),
            }
        }
        Err(error) => GuiResponse::failure(error),
    }
}

/// Parse only the verb/trailing flags. The caller owns the hidden top-level
/// `_gui --origin-json` flag, but forwards every remaining token here.
pub fn parse_command(argv: &[String]) -> Result<GuiCommand, TypedError> {
    let mut complete = vec!["dmux _gui".to_string()];
    complete.extend_from_slice(argv);
    GuiArgv::try_parse_from(complete)
        .map(|parsed| parsed.command)
        .map_err(|error| TypedError::new(ErrorCode::Usage, error.to_string()))
}

/// Serialize exactly one compact JSON response and return its contract exit
/// status. This deliberately writes stdout even for typed failures.
pub fn write_response(response: &GuiResponse) -> u8 {
    match serde_json::to_string(response) {
        Ok(document) => {
            println!("{document}");
            response.exit_code()
        }
        Err(error) => {
            // `GuiResponse` contains only infallible JSON data. Retain the
            // one-document invariant even if a future field violates that.
            println!(
                "{}",
                serde_json::json!({
                    "schema_version": GUI_SCHEMA_VERSION,
                    "ok": false,
                    "error": "operation_failed",
                    "message": format!("serializing GUI response: {error}"),
                })
            );
            1
        }
    }
}

fn typed_gui(error: gui::GuiError) -> TypedError {
    let code = match error {
        gui::GuiError::InvalidRequest(_) | gui::GuiError::MessageTooLarge(_) => ErrorCode::Usage,
        gui::GuiError::InvalidInstance(_) => ErrorCode::IdentityConflict,
        gui::GuiError::BridgeUnavailable(_) | gui::GuiError::Timeout { .. } => {
            ErrorCode::BridgeUnavailable
        }
        gui::GuiError::Rejected { ref code, .. } if code == "not_found" => ErrorCode::NotFound,
        gui::GuiError::Rejected { .. } => ErrorCode::BridgeUnavailable,
        gui::GuiError::Io(_) | gui::GuiError::InvalidAck(_) => ErrorCode::OperationFailed,
    };
    TypedError::new(code, error.to_string())
}

fn typed_registry(error: impl std::fmt::Display) -> TypedError {
    TypedError::new(ErrorCode::OperationFailed, format!("registry: {error}"))
}

fn typed_operation(error: operations::OpError) -> TypedError {
    use operations::OpError;
    let code = match &error {
        OpError::NameConflict(_) => ErrorCode::NameConflict,
        OpError::Indeterminate(_) => ErrorCode::ProviderUnavailable,
        OpError::NotFound(_) => ErrorCode::NotFound,
        OpError::Refused(_) => ErrorCode::RepairRequired,
        OpError::StaleRef(_) => ErrorCode::BackendEpochChanged,
        OpError::Registry(detail) if detail.contains("registry busy") => ErrorCode::RegistryBusy,
        OpError::Registry(detail) if detail.contains("reused with different content") => {
            ErrorCode::IdempotencyReuse
        }
        OpError::Registry(detail) if detail.contains("unfinished operation") => {
            ErrorCode::OperationInProgress
        }
        OpError::Bootstrap(_) | OpError::Lock(_) | OpError::Provider(_) | OpError::Registry(_) => {
            ErrorCode::OperationFailed
        }
    };
    TypedError::new(code, error.to_string())
}

fn typed_wez_native_tree(error: ProviderError) -> TypedError {
    let code = match error {
        ProviderError::EpochChanged { .. } => ErrorCode::BackendEpochChanged,
        ProviderError::WrongInstance { .. } => ErrorCode::WrongBackendInstance,
        ProviderError::NotFound { .. } => ErrorCode::SpaceAbsent,
        ProviderError::PostconditionFailed { .. } => ErrorCode::PostconditionFailed,
        ProviderError::Timeout { .. }
        | ProviderError::NativeFailure { .. }
        | ProviderError::MultiWindow { .. } => ErrorCode::ProviderUnavailable,
    };
    TypedError::new(code, format!("wez native-tree proof: {error:?}"))
}

fn unavailable(message: impl Into<String>) -> TypedError {
    TypedError::new(ErrorCode::ProviderUnavailable, message)
}

fn ambient_marker_from_env() -> Result<MarkerContext, TypedError> {
    fn required(name: &str) -> Result<String, TypedError> {
        std::env::var(name).map_err(|_| {
            TypedError::new(
                ErrorCode::InvalidRef,
                format!("ambient GUI action requires exact {name}"),
            )
        })
    }

    if required("DMUX_CONTEXT_VERSION")? != "1" {
        return Err(TypedError::new(
            ErrorCode::InvalidRef,
            "ambient pane marker DMUX_CONTEXT_VERSION must be 1",
        ));
    }
    let parse_uuid = |name: &str| -> Result<Uuid, TypedError> {
        let raw = required(name)?;
        let parsed = Uuid::parse_str(&raw).map_err(|error| {
            TypedError::new(
                ErrorCode::InvalidRef,
                format!("ambient pane marker {name} is not a UUID: {error}"),
            )
        })?;
        if raw != parsed.to_string() {
            return Err(TypedError::new(
                ErrorCode::InvalidRef,
                format!("ambient pane marker {name} is not canonical lowercase UUID text"),
            ));
        }
        Ok(parsed)
    };
    let space_no_raw = required("DMUX_SPACE_NO")?;
    let space_no_value = space_no_raw.parse::<u64>().map_err(|error| {
        TypedError::new(
            ErrorCode::InvalidRef,
            format!("ambient pane marker DMUX_SPACE_NO is malformed: {error}"),
        )
    })?;
    let space_no = std::num::NonZeroU64::new(space_no_value).ok_or_else(|| {
        TypedError::new(
            ErrorCode::InvalidRef,
            "ambient pane marker DMUX_SPACE_NO must be nonzero",
        )
    })?;
    if space_no_raw != space_no.to_string() {
        return Err(TypedError::new(
            ErrorCode::InvalidRef,
            "ambient pane marker DMUX_SPACE_NO is not canonical decimal",
        ));
    }
    let backend = match required("DMUX_BACKEND")?.as_str() {
        "wez" => Backend::Wez,
        "tmux" => Backend::Tmux,
        _ => {
            return Err(TypedError::new(
                ErrorCode::InvalidRef,
                "ambient pane marker DMUX_BACKEND must be wez or tmux",
            ));
        }
    };
    let domain = match required("DMUX_DOMAIN")? {
        value if value.is_empty() => None,
        value => Some(value),
    };
    Ok(MarkerContext {
        host_uid: HostUid(parse_uuid("DMUX_HOST_UID")?),
        space_uid: SpaceUid(parse_uuid("DMUX_SPACE_UID")?),
        space_no: SpaceNo(space_no),
        backend,
        domain,
        server_epoch: ServerEpoch(parse_uuid("DMUX_SERVER_EPOCH")?),
        group_ref: required("DMUX_GROUP_REF")?,
        split_ref: required("DMUX_SPLIT_REF")?,
    })
}

// ---------------------------------------------------------------------------
// Production authority implementation

const LOCAL_WEZ_DOMAIN: &str = "dmux";

#[derive(Debug, Clone)]
enum AuthorityLocation {
    Local,
    Remote,
}

/// Whether a live-correlated marker is the Space a locator names.
fn connect_locator_matches(locator: &OwnerLocator, candidate: &AuthorityMarker) -> bool {
    locator.matches(
        candidate.marker.space_uid,
        candidate.marker.space_no,
        &candidate.logical_name,
    )
}

/// Markers are correlated from active rows only, so as the resolver's
/// candidate a marker is always `active`.
impl SpaceCandidate for AuthorityMarker {
    fn space_uid(&self) -> SpaceUid {
        self.marker.space_uid
    }

    fn space_no(&self) -> SpaceNo {
        self.marker.space_no
    }

    fn logical_name(&self) -> &str {
        &self.logical_name
    }

    fn lifecycle(&self) -> Lifecycle {
        Lifecycle::Active
    }
}

/// One durable active Space as the cold (no provider, no GUI) resolver sees
/// it: local rows name their backend by instance, which is read only for
/// the one row the lookup selects; an owner's remote answer carries it.
struct ColdCandidate {
    space_uid: SpaceUid,
    space_no: SpaceNo,
    logical_name: String,
    backend: ColdBackend,
}

enum ColdBackend {
    Known(Backend),
    Registered(BackendInstanceUid),
}

impl SpaceCandidate for ColdCandidate {
    fn space_uid(&self) -> SpaceUid {
        self.space_uid
    }

    fn space_no(&self) -> SpaceNo {
        self.space_no
    }

    fn logical_name(&self) -> &str {
        &self.logical_name
    }

    fn lifecycle(&self) -> Lifecycle {
        Lifecycle::Active
    }
}

fn frozen_connect_query(target: &FrozenConnectTarget) -> OwnerConnectQuery {
    let child = target.child.as_ref().map(|child| match child {
        VerifiedConnectChild::Group { epoch, handle } => RequestedChild {
            kind: ChildKind::Group,
            epoch: *epoch,
            handle: handle.clone(),
        },
        VerifiedConnectChild::Split { epoch, split, .. } => RequestedChild {
            kind: ChildKind::Split,
            epoch: *epoch,
            handle: split.clone(),
        },
    });
    OwnerConnectQuery {
        owner: target.owner,
        locator: OwnerLocator::Uid(target.space_uid),
        backend_filter: None,
        child,
    }
}

fn frozen_connect_child_refs(target: &FrozenConnectTarget) -> (Option<String>, Option<String>) {
    match target.child.as_ref() {
        None => (None, None),
        Some(VerifiedConnectChild::Group { epoch, handle }) => (
            Some(child_suffix(&ChildRefShape {
                kind: ChildKind::Group,
                epoch: *epoch,
                handle: handle.clone(),
            })),
            None,
        ),
        Some(VerifiedConnectChild::Split {
            epoch,
            group,
            split,
        }) => (
            Some(child_suffix(&ChildRefShape {
                kind: ChildKind::Group,
                epoch: *epoch,
                handle: group.clone(),
            })),
            Some(child_suffix(&ChildRefShape {
                kind: ChildKind::Split,
                epoch: *epoch,
                handle: split.clone(),
            })),
        ),
    }
}

fn require_same_frozen_connect_target(
    expected: &FrozenConnectTarget,
    actual: &FrozenConnectTarget,
) -> Result<(), TypedError> {
    if expected == actual {
        return Ok(());
    }
    let code = if expected.server_epoch != actual.server_epoch {
        ErrorCode::BackendEpochChanged
    } else if expected.backend_instance_uid != actual.backend_instance_uid {
        ErrorCode::WrongBackendInstance
    } else if expected.backend != actual.backend {
        ErrorCode::BackendMismatch
    } else {
        ErrorCode::IdentityConflict
    };
    Err(TypedError::new(
        code,
        "owner/live connect target changed after it was frozen",
    ))
}

fn bridge_acknowledgement(value: &Value) -> Result<String, TypedError> {
    value
        .get("uid")
        .and_then(Value::as_str)
        .filter(|uid| !uid.is_empty() && !uid.bytes().any(|byte| byte.is_ascii_control()))
        .map(str::to_string)
        .ok_or_else(|| {
            TypedError::new(
                ErrorCode::ProtocolMismatch,
                "authenticated GUI acknowledgement omitted its request uid",
            )
        })
}

/// One marker after owner registry/live correlation. Native pane/window IDs
/// are deliberately absent; bridge presentation consumes only the canonical
/// marker refs and backend incarnation.
#[derive(Debug, Clone)]
struct AuthorityMarker {
    marker: MarkerContext,
    backend_instance: BackendInstanceUid,
    logical_name: String,
    health: Health,
    hierarchy: SpaceHierarchy,
    owner_alias: String,
    owner_label: String,
    route: String,
    location: AuthorityLocation,
}

#[derive(Debug, Clone)]
struct SnapshotMarker {
    authority: AuthorityMarker,
    gui_domain: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DomainAuthority {
    host_uid: HostUid,
    backend_instance: BackendInstanceUid,
    server_epoch: ServerEpoch,
    native_tree: WezNativeTreeResult,
}

fn require_native_tree_survived(
    domain: &str,
    before: &WezNativeTreeResult,
    after: &WezNativeTreeResult,
) -> Result<(), TypedError> {
    if before.backend_instance_uid != after.backend_instance_uid
        || before.server_epoch != after.server_epoch
        || before.sentinel_window_id != after.sentinel_window_id
        || before.sentinel_tab_id != after.sentinel_tab_id
        || before.sentinel_pane_id != after.sentinel_pane_id
    {
        return Err(TypedError::new(
            ErrorCode::PostconditionFailed,
            format!("GUI domain {domain:?} changed exact sentinel/backend incarnation"),
        ));
    }
    for pane in &before.panes {
        if after
            .panes
            .iter()
            .filter(|candidate| candidate.pane_id == pane.pane_id)
            .count()
            != 1
        {
            return Err(TypedError::new(
                ErrorCode::PostconditionFailed,
                format!(
                    "GUI domain {domain:?} lost or duplicated pre-detach physical pane {}",
                    pane.pane_id
                ),
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct SummonTarget {
    authority: AuthorityMarker,
    domain: String,
    alternate_domains: Vec<String>,
}

fn require_complete_hierarchy_survived(
    space_uid: SpaceUid,
    before: &SpaceHierarchy,
    after: &SpaceHierarchy,
) -> Result<(), TypedError> {
    if before.space_uid != after.space_uid || before.server_epoch != after.server_epoch {
        return Err(TypedError::new(
            ErrorCode::PostconditionFailed,
            format!(
                "owner Space/epoch changed while detaching GUI presentation for Space {}",
                space_uid.0
            ),
        ));
    }
    for before_group in &before.groups {
        let matching_groups: Vec<_> = after
            .groups
            .iter()
            .filter(|group| group.group_ref == before_group.group_ref)
            .collect();
        let [after_group] = matching_groups.as_slice() else {
            return Err(TypedError::new(
                ErrorCode::PostconditionFailed,
                format!(
                    "owner Group {} disappeared or became ambiguous while detaching GUI presentation for Space {}",
                    before_group.group_ref, space_uid.0
                ),
            ));
        };
        for before_split in &before_group.splits {
            if after_group
                .splits
                .iter()
                .filter(|split| split.split_ref == before_split.split_ref)
                .count()
                != 1
            {
                return Err(TypedError::new(
                    ErrorCode::PostconditionFailed,
                    format!(
                        "owner Split {} disappeared, moved parent, or became ambiguous while detaching GUI presentation for Space {}",
                        before_split.split_ref, space_uid.0
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn merge_snapshot_hierarchy(
    accumulated: &mut SpaceHierarchy,
    observed: &SpaceHierarchy,
) -> Result<(), TypedError> {
    if accumulated.space_uid != observed.space_uid
        || accumulated.server_epoch != observed.server_epoch
    {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "snapshot scans for one Space disagree on SpaceUid/server epoch",
        ));
    }
    for observed_group in &observed.groups {
        for other in &accumulated.groups {
            if other.group_ref != observed_group.group_ref
                && observed_group.splits.iter().any(|split| {
                    other
                        .splits
                        .iter()
                        .any(|existing| existing.split_ref == split.split_ref)
                })
            {
                return Err(TypedError::new(
                    ErrorCode::IdentityConflict,
                    "snapshot scans place one Split under different Groups",
                ));
            }
        }
        match accumulated
            .groups
            .iter_mut()
            .find(|group| group.group_ref == observed_group.group_ref)
        {
            Some(group) => {
                for split in &observed_group.splits {
                    if !group
                        .splits
                        .iter()
                        .any(|existing| existing.split_ref == split.split_ref)
                    {
                        group.splits.push(split.clone());
                    }
                }
            }
            None => accumulated.groups.push(observed_group.clone()),
        }
    }
    Ok(())
}

/// Owner/backend facts frozen by one nonce-bound remote hello plus the
/// controller/owner exact-build presentation gate.  Keeping the selected
/// route alongside the compatible manifest lets preflight apply the same
/// attached-route-first rule as ordinary presentation without inventing a
/// placeholder Space marker before reservation.
#[derive(Debug, Clone)]
struct RemoteWezPreflight {
    backend_instance: BackendInstanceUid,
    server_epoch: ServerEpoch,
    fresh_route: String,
    manifest: Vec<GuiDomainManifestRow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WezRouteProbe {
    Usable,
    TransportFailed,
    AuthOrCompatFailed,
}

struct OwnedCreateTarget {
    backend: Backend,
    instance: BackendInstanceUid,
    provider: Box<dyn Provider>,
    scope: InventoryScope,
}

impl OwnedCreateTarget {
    fn borrowed(&self) -> OwnerCreateTarget<'_> {
        OwnerCreateTarget {
            backend: self.backend,
            instance: self.instance,
            provider: self.provider.as_ref(),
            scope: &self.scope,
        }
    }
}

/// Durable identity sufficient to reject a missing, ambiguous, or tmux
/// cold-presentation target before the fixed Wez service is started. Live
/// backend epoch/hierarchy authority is deliberately re-established after
/// service readiness; it is not inferred from this preflight record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ColdSpaceIdentity {
    host_uid: HostUid,
    space_uid: SpaceUid,
    backend: Backend,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SafeQuitDomainPlan {
    detach: Vec<String>,
    full_persistent_set: BTreeSet<String>,
}

/// Finalize the already authority-proved persistent-domain selection. tmux
/// changes only the finish action: it never suppresses detachment of Wez
/// domains in a mixed GUI, and an empty detach list is valid only when the
/// complete before-snapshot proves tmux panes are being preserved.
fn safe_quit_domain_plan(
    full_persistent_set: BTreeSet<String>,
    active_persistent_domains: impl IntoIterator<Item = String>,
    contains_tmux: bool,
) -> Result<SafeQuitDomainPlan, TypedError> {
    let mut detach: Vec<_> = active_persistent_domains.into_iter().collect();
    detach.sort();
    detach.dedup();
    if detach
        .iter()
        .any(|domain| !full_persistent_set.contains(domain))
    {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "safe quit selected an active domain outside its configured persistent set",
        ));
    }
    if detach.is_empty() && !contains_tmux {
        return Err(TypedError::new(
            ErrorCode::BridgeUnavailable,
            "safe quit found no imported persistent domain to detach/prove",
        ));
    }
    Ok(SafeQuitDomainPlan {
        detach,
        full_persistent_set,
    })
}

fn safe_quit_platform_action() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "hide"
    }
    #[cfg(target_os = "linux")]
    {
        "quit"
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        "hide"
    }
}

fn heartbeat_proves_domains_detached(
    heartbeat: &BridgeHeartbeat,
    requested: &[String],
    all_persistent_domains: Option<&BTreeSet<String>>,
) -> bool {
    let is_detached = |domain: &String| {
        heartbeat.domains.get(domain).is_some_and(|state| {
            state.state == "Detached"
                && !state.has_any_panes
                && state.pane_count == 0
                && state.valid_marker_pane_count == 0
                && state.system_pane_count == 0
        }) && !heartbeat.panes.iter().any(|pane| &pane.domain == domain)
    };
    let detached = requested.iter().all(is_detached);
    let full_set_detached =
        all_persistent_domains.is_none_or(|persistent| persistent.iter().all(is_detached));
    let unknown_active_persistent = all_persistent_domains.is_some_and(|known| {
        heartbeat.domains.iter().any(|(name, state)| {
            name != "local"
                && !known.contains(name)
                && (state.state == "Attached" || state.has_any_panes)
        })
    });
    detached && full_set_detached && !unknown_active_persistent
}

/// §8.4 tries verified routes in one fixed order: USB, then Tailscale, then
/// any other explicitly enrolled route.
fn route_class_rank(network_class: &str) -> u8 {
    match network_class {
        "usb" => 0,
        "tailscale" => 1,
        _ => 2,
    }
}

/// Pick the one route this operation plan will present through.
///
/// Every candidate must name the owner and backend instance the caller has
/// already validated (`host_uid`, `backend_instance`). A row for any other
/// identity is an `IdentityConflict` even when every candidate agrees with
/// every other: the rows are presentation config derived from the registry,
/// and only the caller's revalidated authority says which instance the Space
/// lives on (plan §13.2; ADR 012 §3.5). Checking candidates merely against
/// each other would let a manifest that names one wrong instance throughout
/// present a Space on a server nothing verified. An incompatible row is
/// refused with its recorded reason before ordering, so a caller that forgot
/// to filter cannot present through a build-mismatched route.
///
/// Two enrolled routes reaching that one `backend_instance_uid` is §8.4's
/// design, not a conflict: the manifest validators already prove one HostUid
/// maps to one backend instance, so a second row is another way to reach the
/// same server, never a second identity.
///
/// The freshly proven route wins because `fresh_route` is the route the owner
/// handshake just completed over after §8.4's own priority walk, so it is the
/// only candidate with live transport evidence; an already-`Attached` domain is
/// deliberately not preferred, since after USB removal that is exactly the dead
/// route acceptance case 20 must abandon. Remaining candidates fall back to the
/// same USB/Tailscale/other order, then the manifest's stable `(priority,
/// route_id)` sequence. The bridge detaches whichever alternate is stale before
/// attaching the winner (§12.3), so selecting away from an attached route is
/// safe.
fn choose_compatible_presentation_row<'a>(
    fresh_route: &str,
    host_uid: HostUid,
    backend_instance: BackendInstanceUid,
    candidates: &[&'a GuiDomainManifestRow],
) -> Result<&'a GuiDomainManifestRow, TypedError> {
    if candidates.is_empty() {
        return Err(unavailable(
            "no exact-build compatible GUI route exists for this remote Wez instance",
        ));
    }
    for row in candidates {
        if row.host_uid != host_uid || row.backend_instance_uid != backend_instance {
            return Err(TypedError::new(
                ErrorCode::IdentityConflict,
                "presentation route names another owner or backend instance than the validated authority",
            ));
        }
        if !row.compatible {
            return Err(unavailable(
                row.unavailable_reason
                    .as_deref()
                    .unwrap_or("remote Wez route is incompatible"),
            ));
        }
    }
    candidates
        .iter()
        .copied()
        .min_by_key(|row| {
            (
                row.name != fresh_route,
                route_class_rank(&row.network_class),
                row.priority,
                row.route_id,
            )
        })
        .ok_or_else(|| unavailable("no exact-build compatible GUI route remains after ordering"))
}

fn preflight_domain_state(state: &BridgeDomainState) -> bool {
    matches!(state.state.as_str(), "Attached" | "Detached")
}

fn require_preflight_domain(
    domains: &BTreeMap<String, BridgeDomainState>,
    domain: &str,
) -> Result<(), TypedError> {
    let state = domains.get(domain).ok_or_else(|| {
        unavailable(format!(
            "presentation domain {domain:?} is absent from the exact fresh GUI heartbeat"
        ))
    })?;
    if !preflight_domain_state(state) {
        return Err(TypedError::new(
            ErrorCode::PostconditionFailed,
            format!(
                "presentation domain {domain:?} is in transient state {:?}",
                state.state
            ),
        ));
    }
    Ok(())
}

fn stale_or_absent_bridge_error(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::InvalidRef
            | ErrorCode::NotFound
            | ErrorCode::SpaceAbsent
            | ErrorCode::SpaceDeleted
            | ErrorCode::BridgeUnavailable
            | ErrorCode::BackendEpochChanged
            | ErrorCode::WrongBackendInstance
            | ErrorCode::BackendMismatch
    )
}

fn classify_new_route_error(error: TypedError) -> Result<WezRouteProbe, TypedError> {
    match error.code {
        ErrorCode::RouteUnavailable => Ok(WezRouteProbe::TransportFailed),
        ErrorCode::AuthFailed
        | ErrorCode::VersionMismatch
        | ErrorCode::ProviderUnavailable
        | ErrorCode::BridgeUnavailable
        | ErrorCode::PostconditionFailed => Ok(WezRouteProbe::AuthOrCompatFailed),
        // In particular, identity/lineage/protocol/malformed-response errors
        // remain exact terminal errors. They are never collapsed into the
        // headless or route-absent tmux rows.
        _ => Err(error),
    }
}

/// Put the lifecycle side effect behind the durable Wez-target gate. Keeping
/// this tiny seam injectable makes the no-start/no-launch ordering directly
/// testable without weakening production owner validation.
fn enter_cold_wez_lifecycle<T>(
    preflight: Result<ColdSpaceIdentity, TypedError>,
    runner: impl FnOnce(&ColdSpaceIdentity) -> Result<T, TypedError>,
) -> Result<(ColdSpaceIdentity, T), TypedError> {
    let target = preflight?;
    if target.backend != Backend::Wez {
        return Err(TypedError::new(
            ErrorCode::BackendMismatch,
            "--launch-gui accepts only an existing Wez Space; tmux requires the native terminal attach path",
        ));
    }
    let result = runner(&target)?;
    Ok((target, result))
}

impl AuthorityMarker {
    fn display(&self) -> GuiStatusDisplay {
        let group = self
            .hierarchy
            .groups
            .iter()
            .find(|group| group.group_ref == self.marker.group_ref);
        GuiStatusDisplay {
            logical_ref: format!("{}{}", self.owner_alias, self.marker.space_no),
            space_name: self.logical_name.clone(),
            backend: self.marker.backend,
            owner_alias: self.owner_alias.clone(),
            owner_label: self.owner_label.clone(),
            route: self.route.clone(),
            group_count: u32::try_from(self.hierarchy.groups.len()).unwrap_or(u32::MAX),
            split_count: u32::try_from(
                self.hierarchy
                    .groups
                    .iter()
                    .map(|group| group.splits.len())
                    .sum::<usize>(),
            )
            .unwrap_or(u32::MAX),
            group_name: group.and_then(|group| group.title.clone()),
        }
    }
}

/// Binding retained only for the duration of one `_gui` invocation. The
/// heartbeat is the exact fresh document read while binding the marker.
#[derive(Debug, Clone)]
pub struct BoundGuiOrigin {
    origin: GuiCliOrigin,
    selection: BridgeSelection,
    heartbeat: BridgeHeartbeat,
    authority: AuthorityMarker,
}

/// Production dependencies are explicit so tests can substitute scratch
/// registry/runtime/state directories and a fake route invoker.
pub struct ProductionGuiAuthority<I = SshInvoker> {
    env: OperationEnv,
    runtime_dir: PathBuf,
    history: History,
    wezterm_bin: String,
    wezterm_config: String,
    gui_config: PathBuf,
    helper_bin: String,
    invoker: I,
    partial_result: Option<Value>,
}

impl ProductionGuiAuthority<SshInvoker> {
    pub fn production() -> Result<Self, TypedError> {
        let env = OperationEnv::production()
            .map_err(|error| TypedError::new(ErrorCode::OperationFailed, error.to_string()))?;
        let runtime_dir = crate::runtime::dmux_runtime_dir()
            .map_err(|error| TypedError::new(ErrorCode::OperationFailed, error.to_string()))?;
        let state_dir = History::default_dir().ok_or_else(|| {
            TypedError::new(
                ErrorCode::OperationFailed,
                "HOME/XDG_STATE_HOME is unavailable for GUI Space history",
            )
        })?;
        let (wezterm_bin, wezterm_config) = crate::runtime::production_wez_paths();
        let gui_config = std::env::var_os("DMUX_WEZ_GUI_CONFIG")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join("dotfiles/shared/wezterm/wezterm.lua"))
            })
            .ok_or_else(|| {
                TypedError::new(
                    ErrorCode::OperationFailed,
                    "HOME/DMUX_WEZ_GUI_CONFIG is unavailable for cold GUI launch",
                )
            })?;
        let helper_bin = std::env::var("DMUX_HELPER_BIN").unwrap_or_else(|_| {
            std::env::current_exe()
                .ok()
                .map(|path| path.with_file_name("pane-bootstrap"))
                .filter(|path| path.exists())
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "pane-bootstrap".to_string())
        });
        Ok(Self {
            env,
            runtime_dir,
            history: History::new(state_dir),
            wezterm_bin,
            wezterm_config,
            gui_config,
            helper_bin,
            invoker: SshInvoker::default(),
            partial_result: None,
        })
    }
}

impl<I: RouteInvoker> ProductionGuiAuthority<I> {
    #[allow(clippy::too_many_arguments)]
    pub fn with_dependencies(
        env: OperationEnv,
        runtime_dir: PathBuf,
        state_dir: PathBuf,
        wezterm_bin: String,
        wezterm_config: String,
        gui_config: PathBuf,
        helper_bin: String,
        invoker: I,
    ) -> Self {
        Self {
            env,
            runtime_dir,
            history: History::new(state_dir),
            wezterm_bin,
            wezterm_config,
            gui_config,
            helper_bin,
            invoker,
            partial_result: None,
        }
    }

    fn registry(&self) -> Result<Registry, TypedError> {
        Registry::open(RegistryConfig::new(&self.env.db_path, &self.env.lock_dir))
            .map_err(typed_registry)
    }

    /// Construct a CLI origin for a public command invoked from a managed
    /// GUI pane without trusting ambient data for GUI-local identity. The
    /// environment supplies only the complete owner marker; owner authority
    /// revalidates it first, then the unique fresh heartbeat supplies the
    /// exact GUI instance, pane id, and actual imported domain.
    fn ambient_origin(&self) -> Result<GuiCliOrigin, TypedError> {
        let ambient = ambient_marker_from_env()?;
        let authoritative = self.validate_authority_marker(&ambient)?;
        if authoritative.marker != ambient {
            return Err(TypedError::new(
                ErrorCode::IdentityConflict,
                "ambient pane marker changed during owner revalidation",
            ));
        }
        let selection = gui::discover_in_gui_instance(&self.runtime_dir, &authoritative.marker)
            .map_err(typed_gui)?;
        let authoritative = match ambient.backend {
            Backend::Wez => {
                self.validate_authority_marker_in_domain(&ambient, Some(&selection.domain))?
            }
            // A tmux marker names its tmux owner. `selection.domain` is the
            // physical outer Wez container (often local for a remote SSH
            // client), not an owner Wez route.
            Backend::Tmux => self.validate_authority_marker(&ambient)?,
        };
        let candidate = GuiCliOrigin {
            protocol_version: gui::BRIDGE_PROTOCOL_VERSION,
            gui_instance: selection.gui_instance,
            pane_id: selection.pane_id,
            domain: selection.domain,
            tmux_client_uid: None,
            marker: authoritative.marker,
        };
        // Reuse the strict serialized-origin validator so this adapter has
        // exactly the same field/canonicalization contract as Lua.
        let encoded = serde_json::to_string(&candidate)
            .map_err(|error| TypedError::new(ErrorCode::OperationFailed, error.to_string()))?;
        gui::parse_origin_json(&encoded).map_err(typed_gui)
    }

    /// A missing, incomplete, or stale ambient GUI marker is a positive
    /// headless/untrusted observation for the local §8.3 row. Identity,
    /// lineage, protocol, and registry failures remain terminal and can
    /// never be translated into permission to create tmux remotely.
    fn creation_bridge(&mut self) -> Result<Option<BoundGuiOrigin>, TypedError> {
        let origin = match self.ambient_origin() {
            Ok(origin) => origin,
            Err(error) if stale_or_absent_bridge_error(error.code) => return Ok(None),
            Err(error) => return Err(error),
        };
        match <Self as GuiAuthority>::bind_origin(self, &origin) {
            Ok(bound) => Ok(Some(bound)),
            Err(error) if stale_or_absent_bridge_error(error.code) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Read-only local compatibility proof. This deliberately does not call
    /// the lifecycle ensure seam: a stopped/missing/stale local service is
    /// the headless tmux row, not authority to start Wez during backend
    /// selection. Exact descriptor/registry identity disagreements remain
    /// errors rather than being hidden as "not compatible".
    fn local_wez_service_compatible(&self) -> Result<bool, TypedError> {
        let registry = self.registry()?;
        let Some(instance) = registry
            .backend_instance_for_backend(Backend::Wez)
            .map_err(typed_registry)?
        else {
            return Ok(false);
        };
        let Some(epoch) = registry
            .backend_server(instance)
            .map_err(typed_registry)?
            .server_epoch
        else {
            return Ok(false);
        };
        let (provider, scope) = match self.local_provider(instance, Backend::Wez, epoch) {
            Ok(provider) => provider,
            Err(error) if error.code == ErrorCode::ProviderUnavailable => return Ok(false),
            Err(error) => return Err(error),
        };
        match provider.inventory(&scope) {
            InventoryOutcome::Complete(inventory) if inventory.server_epoch == Some(epoch) => {
                Ok(true)
            }
            InventoryOutcome::Complete(_) => Err(TypedError::new(
                ErrorCode::BackendEpochChanged,
                "local Wez compatibility inventory changed server epoch",
            )),
            InventoryOutcome::ServerStopped { .. }
            | InventoryOutcome::Unreachable { .. }
            | InventoryOutcome::CommandMissing { .. }
            | InventoryOutcome::VersionMismatch { .. }
            | InventoryOutcome::Timeout { .. } => Ok(false),
            InventoryOutcome::AuthFailed { detail }
            | InventoryOutcome::PermissionFailure { detail } => {
                Err(TypedError::new(ErrorCode::AuthFailed, detail))
            }
            InventoryOutcome::HostKeyIdentityFailed { detail } => {
                Err(TypedError::new(ErrorCode::HostIdentityChanged, detail))
            }
            InventoryOutcome::ProtocolMismatch { detail }
            | InventoryOutcome::Malformed { detail } => {
                Err(TypedError::new(ErrorCode::ProtocolMismatch, detail))
            }
        }
    }

    fn local_provider(
        &self,
        instance: BackendInstanceUid,
        backend: Backend,
        epoch: ServerEpoch,
    ) -> Result<(Box<dyn Provider>, InventoryScope), TypedError> {
        let registry = self.registry()?;
        let info = registry
            .backend_instance_info(instance)
            .map_err(typed_registry)?;
        if info.backend != backend {
            return Err(TypedError::new(
                ErrorCode::BackendMismatch,
                "Space backend differs from its registered backend instance",
            ));
        }
        let endpoint = match backend {
            Backend::Tmux => info.socket_path.ok_or_else(|| {
                unavailable(crate::backend::scope::ManagedTarget::unaddressable_detail(
                    Backend::Tmux,
                    instance,
                ))
            })?,
            Backend::Wez => {
                let server = registry.backend_server(instance).map_err(typed_registry)?;
                if server.server_epoch != Some(epoch) {
                    return Err(TypedError::new(
                        ErrorCode::BackendEpochChanged,
                        "registered Wez server epoch differs from the selected authority",
                    ));
                }
                let descriptor = crate::runtime::read_verified_ready_wez_descriptor_in(
                    &self.runtime_dir,
                    instance.0,
                    epoch.0,
                )
                .map_err(|error| unavailable(format!("managed Wez descriptor: {error}")))?
                .ok_or_else(|| unavailable("managed Wez descriptor is absent"))?;
                if info.socket_path.as_deref() != Some(descriptor.socket.as_str())
                    || server.server_pid != Some(i64::from(descriptor.pid))
                    || server.server_start_token.as_deref() != Some(descriptor.start_token.as_str())
                    || server.socket_dev
                        != descriptor
                            .socket_dev
                            .and_then(|value| i64::try_from(value).ok())
                    || server.socket_ino
                        != descriptor
                            .socket_ino
                            .and_then(|value| i64::try_from(value).ok())
                {
                    return Err(TypedError::new(
                        ErrorCode::WrongBackendInstance,
                        "managed Wez descriptor differs from the registered socket/process incarnation",
                    ));
                }
                descriptor.socket
            }
        };
        let provider: Box<dyn Provider> = match backend {
            Backend::Tmux => Box::new(crate::backend::tmux::TmuxProvider::new(endpoint.clone())),
            Backend::Wez => Box::new(crate::backend::wez::WezProvider::new(
                &self.wezterm_bin,
                self.wezterm_config.clone(),
            )),
        };
        Ok((provider, InventoryScope::managed(backend, endpoint, epoch)))
    }

    /// Resolve the registered opposite backend without allocating an
    /// instance. `None` remains only a hint: the owner-fenced operation
    /// re-queries the registry while holding the exact-name decision lock
    /// and accepts it only when the opposite instance is still absent.
    ///
    /// The target exists so that a same-named Space on the other backend is
    /// detected before anything is reserved or spawned (§8.2 steps 3–8,
    /// cases 5–7). A registered, addressable opposite instance whose server
    /// epoch was never published is therefore refused here, before the
    /// fenced create runs, with `backend_epoch_changed`: an inventory nothing
    /// verified cannot establish "no collision", and a scan under an unpinned
    /// scope would accept whatever server answered (review finding #15).
    /// Only `backend::scope::resolve_managed` turns the row into a scope; this
    /// wrapper's own job is constructing the provider.
    fn local_opposite_create_target(
        &self,
        selected: Backend,
    ) -> Result<Option<OwnedCreateTarget>, TypedError> {
        use crate::backend::scope::{self, ManagedTarget};

        let opposite = match selected {
            Backend::Wez => Backend::Tmux,
            Backend::Tmux => Backend::Wez,
        };
        let registry = self.registry()?;
        let (instance, scope) =
            match scope::resolve_managed(&registry, opposite).map_err(typed_registry)? {
                ManagedTarget::Managed { instance, scope } => (instance, scope),
                ManagedTarget::Unpublished(instance) => {
                    return Err(TypedError::new(
                        ErrorCode::BackendEpochChanged,
                        ManagedTarget::unpublished_detail(opposite, instance),
                    ));
                }
                ManagedTarget::Unaddressable(_) => {
                    return Err(unavailable(format!(
                        "registered opposite {opposite} backend has no inventory endpoint"
                    )));
                }
                // No opposite instance: nothing to collide with. The fenced
                // create re-proves this under the decision lock.
                ManagedTarget::Unregistered => return Ok(None),
            };
        if scope.backend != opposite {
            return Err(TypedError::new(
                ErrorCode::WrongBackendInstance,
                "opposite backend instance changed kind during GUI create preflight",
            ));
        }
        let provider: Box<dyn Provider> = match opposite {
            Backend::Tmux => Box::new(crate::backend::tmux::TmuxProvider::new(
                scope.endpoint.clone(),
            )),
            Backend::Wez => Box::new(crate::backend::wez::WezProvider::new(
                &self.wezterm_bin,
                self.wezterm_config.clone(),
            )),
        };
        Ok(Some(OwnedCreateTarget {
            backend: opposite,
            instance,
            provider,
            scope,
        }))
    }

    fn enrolled_host(&self, host_uid: HostUid) -> Result<HostRow, TypedError> {
        self.registry()?
            .hosts()
            .map_err(typed_registry)?
            .into_iter()
            .find(|host| host.host_uid == host_uid && host.lifecycle == HostLifecycle::Enrolled)
            .ok_or_else(|| {
                TypedError::new(
                    ErrorCode::HostIdentityChanged,
                    format!("host {} is not enrolled", host_uid.0),
                )
            })
    }

    fn host_names(host: &HostRow) -> Result<(String, String), TypedError> {
        let alias = host.alias.clone().ok_or_else(|| {
            TypedError::new(
                ErrorCode::HostIdentityChanged,
                format!("enrolled host {} has no current alias", host.host_uid.0),
            )
        })?;
        let label = host.label.clone().unwrap_or_else(|| alias.clone());
        Ok((alias, label))
    }

    fn validate_local_marker(&self, marker: &MarkerContext) -> Result<AuthorityMarker, TypedError> {
        let registry = self.registry()?;
        let identity = registry.identity().map_err(typed_registry)?;
        if marker.host_uid != identity.host_uid {
            return Err(TypedError::new(
                ErrorCode::HostIdentityChanged,
                "marker does not belong to the local authority",
            ));
        }
        let row = registry.space(marker.space_uid).map_err(typed_registry)?;
        let (provider, scope) =
            self.local_provider(row.backend_instance, marker.backend, marker.server_epoch)?;
        let validated =
            operations::validate_marker_context(&self.env, provider.as_ref(), &scope, marker)
                .map_err(typed_operation)?;
        let hierarchy =
            operations::hierarchy(&self.env, provider.as_ref(), &scope, marker.space_uid)
                .map_err(typed_operation)?;
        let host = self.enrolled_host(identity.host_uid)?;
        let (owner_alias, owner_label) = Self::host_names(&host)?;
        Ok(AuthorityMarker {
            marker: validated.context,
            backend_instance: validated.backend_instance,
            logical_name: validated.logical_name,
            health: validated.health,
            hierarchy,
            owner_alias,
            owner_label,
            route: "local".to_string(),
            location: AuthorityLocation::Local,
        })
    }

    fn remote_call<T: for<'de> Deserialize<'de>>(
        &self,
        host_uid: HostUid,
        method: &str,
        payload: Value,
        claimed_instance: Option<BackendInstanceUid>,
        claimed_epoch: Option<ServerEpoch>,
        fresh_hello: bool,
    ) -> Result<(T, Envelope, i64), TypedError> {
        let mut registry = self.registry()?;
        let identity = registry.identity().map_err(typed_registry)?;
        let head = registry.authority_head().map_err(typed_registry)?;
        let mut request = request_envelope(&identity, &head, method, Uuid::new_v4(), payload);
        request.backend_instance_uid = claimed_instance;
        request.server_epoch = claimed_epoch;
        let invocation = AgentInvocation::new(method);
        let outcome = call_over_routes(
            &mut registry,
            &PeerExpectation {
                host_uid,
                need_capability: None,
                claimed_current: fresh_hello,
            },
            &request,
            &self.invoker,
            &invocation,
            REMOTE_DEADLINE,
        )?;
        let value = outcome.envelope.payload.clone().ok_or_else(|| {
            TypedError::new(
                ErrorCode::ProtocolMismatch,
                "successful owner response omitted payload",
            )
        })?;
        let parsed = serde_json::from_value(value).map_err(|error| {
            TypedError::new(
                ErrorCode::ProtocolMismatch,
                format!("owner {method} payload: {error}"),
            )
        })?;
        Ok((parsed, outcome.envelope, outcome.route_id))
    }

    fn remote_hello(&self, host_uid: HostUid) -> Result<(HelloInfo, i64), TypedError> {
        let nonce = Uuid::new_v4();
        let (hello, envelope, route_id): (HelloInfo, _, _) = self.remote_call(
            host_uid,
            protocol::methods::HELLO,
            serde_json::to_value(HelloPayload { nonce: Some(nonce) })
                .expect("HelloPayload serializes"),
            None,
            None,
            true,
        )?;
        if hello.nonce != Some(nonce)
            || hello.host_uid != host_uid
            || hello.host_uid != envelope.host_uid
            || hello.registry_uid != envelope.registry_uid
            || hello.authority_revision != envelope.authority_revision
            || hello.authority_head_hash != envelope.authority_head_hash
            || hello.protocol_version != protocol::PROTOCOL_VERSION
        {
            return Err(TypedError::new(
                ErrorCode::HostIdentityChanged,
                "fresh hello payload does not match its nonce-bound identity envelope",
            ));
        }
        Ok((hello, route_id))
    }

    /// Non-fallback hello used only by the §8.3 pre-selection probe.  USB
    /// absence/failure must not be hidden by a successful Tailscale route,
    /// so each enrolled route is tested under the same identity/lineage and
    /// nonce checks as the ordinary route walker but is pinned by row id.
    fn remote_hello_pinned(
        &self,
        host_uid: HostUid,
        route_id: i64,
    ) -> Result<HelloInfo, TypedError> {
        let nonce = Uuid::new_v4();
        let mut registry = self.registry()?;
        let identity = registry.identity().map_err(typed_registry)?;
        let head = registry.authority_head().map_err(typed_registry)?;
        let request = request_envelope(
            &identity,
            &head,
            protocol::methods::HELLO,
            Uuid::new_v4(),
            serde_json::to_value(HelloPayload { nonce: Some(nonce) })
                .expect("HelloPayload serializes"),
        );
        let outcome = call_over_pinned_route(
            &mut registry,
            &PeerExpectation {
                host_uid,
                need_capability: None,
                claimed_current: true,
            },
            route_id,
            &request,
            &self.invoker,
            &AgentInvocation::new(protocol::methods::HELLO),
            REMOTE_DEADLINE,
        )?;
        let value = outcome.envelope.payload.clone().ok_or_else(|| {
            TypedError::new(
                ErrorCode::ProtocolMismatch,
                "successful pinned hello omitted its payload",
            )
        })?;
        let hello: HelloInfo = serde_json::from_value(value).map_err(|error| {
            TypedError::new(
                ErrorCode::ProtocolMismatch,
                format!("owner pinned hello payload: {error}"),
            )
        })?;
        if hello.nonce != Some(nonce)
            || hello.host_uid != host_uid
            || hello.host_uid != outcome.envelope.host_uid
            || hello.registry_uid != outcome.envelope.registry_uid
            || hello.authority_revision != outcome.envelope.authority_revision
            || hello.authority_head_hash != outcome.envelope.authority_head_hash
            || hello.protocol_version != protocol::PROTOCOL_VERSION
        {
            return Err(TypedError::new(
                ErrorCode::HostIdentityChanged,
                "pinned fresh hello payload does not match its nonce-bound identity envelope",
            ));
        }
        Ok(hello)
    }

    fn remote_spaces(&self, host_uid: HostUid) -> Result<SpacesInfo, TypedError> {
        let (spaces, _, _): (SpacesInfo, _, _) = self.remote_call(
            host_uid,
            protocol::methods::SPACES,
            serde_json::json!({}),
            None,
            None,
            false,
        )?;
        Ok(spaces)
    }

    fn remote_hierarchy(
        &self,
        host_uid: HostUid,
        space_uid: SpaceUid,
        instance: BackendInstanceUid,
        epoch: ServerEpoch,
    ) -> Result<SpaceHierarchy, TypedError> {
        let (hierarchy, envelope, _): (SpaceHierarchy, _, _) = self.remote_call(
            host_uid,
            protocol::methods::HIERARCHY,
            serde_json::to_value(HierarchyPayload { space_uid })
                .expect("HierarchyPayload serializes"),
            Some(instance),
            Some(epoch),
            false,
        )?;
        if envelope.backend_instance_uid != Some(instance)
            || envelope.server_epoch != Some(epoch)
            || hierarchy.space_uid != space_uid
            || hierarchy.server_epoch != epoch
        {
            return Err(TypedError::new(
                ErrorCode::BackendEpochChanged,
                "hierarchy response differs from the claimed backend instance/epoch",
            ));
        }
        Ok(hierarchy)
    }

    fn correlate_remote_marker(
        &self,
        marker: &MarkerContext,
        space: &SpaceInfo,
        hierarchy: &SpaceHierarchy,
        owner_alias: String,
        owner_label: String,
        route: String,
    ) -> Result<AuthorityMarker, TypedError> {
        if space.space_uid != marker.space_uid
            || space.space_no != marker.space_no.get()
            || space.backend != marker.backend
            || space.lifecycle != Lifecycle::Active
            || hierarchy.space_uid != marker.space_uid
            || hierarchy.server_epoch != marker.server_epoch
        {
            return Err(TypedError::new(
                ErrorCode::BackendEpochChanged,
                "remote marker no longer matches its authority Space/epoch",
            ));
        }
        let groups: Vec<_> = hierarchy
            .groups
            .iter()
            .filter(|group| group.group_ref == marker.group_ref)
            .collect();
        let split_parents: Vec<_> = hierarchy
            .groups
            .iter()
            .filter(|group| {
                group
                    .splits
                    .iter()
                    .any(|split| split.split_ref == marker.split_ref)
            })
            .collect();
        if groups.len() != 1
            || split_parents.len() != 1
            || split_parents[0].group_ref != marker.group_ref
        {
            return Err(TypedError::new(
                ErrorCode::BackendEpochChanged,
                "remote marker Group/Split correlation is absent or ambiguous",
            ));
        }
        Ok(AuthorityMarker {
            marker: marker.clone(),
            backend_instance: space.backend_instance_uid,
            logical_name: space.name.clone(),
            health: space.health,
            hierarchy: hierarchy.clone(),
            owner_alias,
            owner_label,
            route,
            location: AuthorityLocation::Remote,
        })
    }

    fn validate_remote_marker(
        &self,
        marker: &MarkerContext,
        gui_domain: Option<&str>,
    ) -> Result<AuthorityMarker, TypedError> {
        let host = self.enrolled_host(marker.host_uid)?;
        let (owner_alias, owner_label) = Self::host_names(&host)?;
        if let (Some(marker_domain), Some(actual_domain)) = (marker.domain.as_deref(), gui_domain)
            && marker_domain != actual_domain
        {
            return Err(TypedError::new(
                ErrorCode::HostIdentityChanged,
                "owner marker domain differs from the actual GUI domain",
            ));
        }

        let registry = self.registry()?;
        let routes = registry
            .routes_for(marker.host_uid)
            .map_err(typed_registry)?;
        let requested_domain = gui_domain.or(marker.domain.as_deref());
        if let Some(domain) = requested_domain {
            let route_matches = routes
                .iter()
                .filter(|route| route.enabled && route.wez_domain.as_deref() == Some(domain))
                .count();
            if route_matches != 1 {
                return Err(TypedError::new(
                    ErrorCode::HostIdentityChanged,
                    "actual GUI domain is not one exact enabled enrolled route",
                ));
            }
        }

        let (hello, route_id) = self.remote_hello(marker.host_uid)?;
        let route = requested_domain.map(str::to_string).unwrap_or_else(|| {
            routes
                .iter()
                .find(|route| route.route_id == route_id && route.enabled)
                .and_then(|route| {
                    route
                        .wez_domain
                        .clone()
                        .or_else(|| Some(route.endpoint.clone()))
                })
                .unwrap_or_else(|| "owner-validated-route".to_string())
        });
        let spaces = self.remote_spaces(marker.host_uid)?;
        let matches: Vec<&SpaceInfo> = spaces
            .spaces
            .iter()
            .filter(|space| space.space_uid == marker.space_uid)
            .collect();
        let [space] = matches.as_slice() else {
            return Err(TypedError::new(
                ErrorCode::NotFound,
                "remote marker Space is absent or ambiguous",
            ));
        };
        let backend_matches: Vec<_> = hello
            .backends
            .iter()
            .filter(|backend| {
                backend.backend == marker.backend
                    && backend.backend_instance_uid == space.backend_instance_uid
                    && backend.server_epoch == Some(marker.server_epoch)
            })
            .collect();
        if backend_matches.len() != 1 {
            return Err(TypedError::new(
                ErrorCode::BackendEpochChanged,
                "fresh owner hello did not prove the marker backend instance/epoch",
            ));
        }
        let complete = spaces.scans.iter().any(|scan| {
            scan.backend == marker.backend
                && scan.outcome == "complete"
                && scan.server_epoch == Some(marker.server_epoch)
        });
        if !complete {
            return Err(unavailable(
                "remote owner did not return a complete same-epoch inventory",
            ));
        }
        let hierarchy = self.remote_hierarchy(
            marker.host_uid,
            marker.space_uid,
            space.backend_instance_uid,
            marker.server_epoch,
        )?;
        self.correlate_remote_marker(marker, space, &hierarchy, owner_alias, owner_label, route)
    }

    fn validate_authority_marker(
        &self,
        marker: &MarkerContext,
    ) -> Result<AuthorityMarker, TypedError> {
        self.validate_authority_marker_in_domain(marker, None)
    }

    fn validate_authority_marker_in_domain(
        &self,
        marker: &MarkerContext,
        gui_domain: Option<&str>,
    ) -> Result<AuthorityMarker, TypedError> {
        let identity = self.registry()?.identity().map_err(typed_registry)?;
        if marker.host_uid == identity.host_uid {
            self.validate_local_marker(marker)
        } else {
            // A tmux marker's owner authority is the exact tmux
            // instance/epoch/client. Its heartbeat domain is only the
            // physical outer Wez container and may be local while the tmux
            // owner is remote, so it must never be interpreted as an
            // enrolled Wez route for that owner.
            self.validate_remote_marker(
                marker,
                marker_owner_route_domain(marker.backend, gui_domain),
            )
        }
    }

    fn remote_domain_manifest(&self) -> Result<Vec<GuiDomainManifestRow>, TypedError> {
        let controller = crate::remote::wez_compat::probe_wezterm_capabilities(
            &self.wezterm_bin,
            crate::remote::wez_compat::DEFAULT_PROBE_DEADLINE,
        );
        let registry = self.registry()?;
        let identity = registry.identity().map_err(typed_registry)?;
        let hosts = registry.hosts().map_err(typed_registry)?;
        let mut sources = Vec::new();
        for host in hosts.into_iter().filter(|host| {
            host.lifecycle == HostLifecycle::Enrolled && host.host_uid != identity.host_uid
        }) {
            // A route enters the GUI config only after a fresh nonce-bound,
            // identity/lineage-validated hello. Stale peer cache is never a
            // domain source.
            let Ok((hello, _)) = self.remote_hello(host.host_uid) else {
                continue;
            };
            let wez_backends: Vec<_> = hello
                .backends
                .iter()
                .filter(|backend| {
                    backend.backend == Backend::Wez
                        && backend.server_epoch.is_some()
                        && backend
                            .socket_path
                            .as_deref()
                            .is_some_and(|path| Path::new(path).is_absolute())
                })
                .collect();
            let [backend] = wez_backends.as_slice() else {
                continue;
            };
            let remote_path =
                crate::remote::wez_compat::reported_remote_wezterm_path(&hello.capabilities)
                    .ok()
                    .flatten();
            let (compatible, unavailable_reason) = match &controller {
                Ok(controller) => {
                    let assessment = crate::remote::wez_compat::assess_automatic_remote_wez_hello(
                        controller, &hello,
                    );
                    match assessment.typed_error() {
                        None => (true, None),
                        Some(error) => (false, Some(error.message)),
                    }
                }
                Err(error) => (
                    false,
                    Some(format!(
                        "controller WezTerm capability probe failed: {error}; automatic fallback is forbidden"
                    )),
                ),
            };
            // A refused owner advertises no attachable endpoint, so its
            // rows carry no proxy command to build one from.
            let managed_socket = if compatible {
                crate::remote::wez_compat::reported_managed_wez_socket(&hello.capabilities)
                    .ok()
                    .flatten()
            } else {
                None
            };
            for route in registry
                .routes_for(host.host_uid)
                .map_err(typed_registry)?
                .into_iter()
                .filter(|route| route.enabled && route.transport != Transport::Local)
            {
                let (Some(name), Some(username)) = (route.wez_domain, route.username) else {
                    continue;
                };
                sources.push(RemoteDomainSource {
                    name,
                    remote_address: route.endpoint,
                    username,
                    remote_wezterm_path: remote_path.clone(),
                    managed_socket: managed_socket.clone(),
                    host_uid: host.host_uid,
                    backend_instance_uid: backend.backend_instance_uid,
                    route_id: route.route_id,
                    priority: route.priority,
                    transport: route.transport,
                    network_class: route.network_class,
                    compatible,
                    unavailable_reason: unavailable_reason.clone(),
                });
            }
        }
        gui::build_domain_manifest(sources).map_err(typed_gui)
    }

    /// Prove one remote owner's Wez incarnation and build only the domain
    /// rows derived from the same nonce-bound hello.  Unlike the picker-wide
    /// manifest, an error for the selected owner is terminal: preflight must
    /// not turn a failed owner read into an empty best-effort result.
    fn remote_wez_preflight(&self, owner: HostUid) -> Result<RemoteWezPreflight, TypedError> {
        self.enrolled_host(owner)?;
        let registry = self.registry()?;
        let identity = registry.identity().map_err(typed_registry)?;
        if owner == identity.host_uid {
            return Err(TypedError::new(
                ErrorCode::BackendMismatch,
                "remote Wez presentation preflight received the local owner",
            ));
        }

        let controller = crate::remote::wez_compat::probe_wezterm_capabilities(
            &self.wezterm_bin,
            crate::remote::wez_compat::DEFAULT_PROBE_DEADLINE,
        )
        .map_err(|error| {
            unavailable(format!(
                "controller WezTerm capability probe failed: {error}; automatic fallback is forbidden"
            ))
        })?;
        let (hello, route_id) = self.remote_hello(owner)?;
        crate::remote::wez_compat::require_automatic_remote_wez_hello(&controller, &hello)?;
        let remote_wezterm_path =
            crate::remote::wez_compat::reported_remote_wezterm_path(&hello.capabilities)
                .map_err(|error| unavailable(format!("remote Wez executable fact: {error}")))?
                .ok_or_else(|| unavailable("remote hello omitted its canonical Wez executable"))?;
        let managed_socket =
            crate::remote::wez_compat::reported_managed_wez_socket(&hello.capabilities)
                .map_err(|error| unavailable(format!("remote managed Wez socket fact: {error}")))?
                .ok_or_else(|| unavailable("remote hello omitted its managed Wez socket"))?;

        let wez_backends: Vec<_> = hello
            .backends
            .iter()
            .filter(|backend| backend.backend == Backend::Wez)
            .collect();
        let [backend] = wez_backends.as_slice() else {
            return Err(TypedError::new(
                ErrorCode::WrongBackendInstance,
                format!(
                    "fresh owner hello reported {} managed Wez backend instances",
                    wez_backends.len()
                ),
            ));
        };
        let server_epoch = backend.server_epoch.ok_or_else(|| {
            unavailable("selected owner's managed Wez backend is not in a ready server epoch")
        })?;
        let socket = backend.socket_path.as_deref().ok_or_else(|| {
            unavailable("selected owner's managed Wez backend omitted its socket path")
        })?;
        if !Path::new(socket).is_absolute() || socket.chars().any(char::is_control) {
            return Err(TypedError::new(
                ErrorCode::ProtocolMismatch,
                "selected owner's managed Wez socket is not a strict absolute path",
            ));
        }
        // Both facts come from the owner's one managed instance row, so a
        // disagreement means the endpoint the domains would dial is not the
        // instance whose epoch this preflight verified.
        if socket != managed_socket {
            return Err(TypedError::new(
                ErrorCode::ProtocolMismatch,
                "selected owner's backend socket differs from its reported proxy endpoint",
            ));
        }

        let routes = registry.routes_for(owner).map_err(typed_registry)?;
        let verified_route = routes
            .iter()
            .find(|route| {
                route.route_id == route_id
                    && route.enabled
                    && route.transport != Transport::Local
            })
            .ok_or_else(|| {
                TypedError::new(
                    ErrorCode::HostIdentityChanged,
                    "fresh hello completed over a route that is no longer an exact enabled remote route",
                )
            })?;
        let fresh_route = verified_route.wez_domain.clone().unwrap_or_default();

        let sources: Vec<_> = routes
            .into_iter()
            .filter(|route| route.enabled && route.transport != Transport::Local)
            .filter_map(|route| {
                let (Some(name), Some(username)) = (route.wez_domain, route.username) else {
                    return None;
                };
                Some(RemoteDomainSource {
                    name,
                    remote_address: route.endpoint,
                    username,
                    remote_wezterm_path: Some(remote_wezterm_path.clone()),
                    managed_socket: Some(managed_socket.clone()),
                    host_uid: owner,
                    backend_instance_uid: backend.backend_instance_uid,
                    route_id: route.route_id,
                    priority: route.priority,
                    transport: route.transport,
                    network_class: route.network_class,
                    compatible: true,
                    unavailable_reason: None,
                })
            })
            .collect();
        let manifest = gui::build_domain_manifest(sources).map_err(typed_gui)?;
        if manifest.is_empty() {
            return Err(unavailable(
                "selected owner has no enabled GUI-configurable Wez route",
            ));
        }
        Ok(RemoteWezPreflight {
            backend_instance: backend.backend_instance_uid,
            server_epoch,
            fresh_route,
            manifest,
        })
    }

    fn remote_preflight_domain(
        &self,
        owner: HostUid,
        heartbeat_domains: &BTreeMap<String, BridgeDomainState>,
        authority: &RemoteWezPreflight,
    ) -> Result<(String, Vec<String>), TypedError> {
        let candidates: Vec<_> = authority
            .manifest
            .iter()
            .filter(|row| {
                row.compatible
                    && row.remote_wezterm_path.is_some()
                    && row.host_uid == owner
                    && row.backend_instance_uid == authority.backend_instance
                    && heartbeat_domains
                        .get(&row.name)
                        .is_some_and(preflight_domain_state)
            })
            .collect();
        let selected = choose_compatible_presentation_row(
            &authority.fresh_route,
            owner,
            authority.backend_instance,
            &candidates,
        )?;
        let alternate_domains = selected
            .alternate_domains
            .iter()
            .filter(|name| {
                heartbeat_domains
                    .get(*name)
                    .is_some_and(preflight_domain_state)
            })
            .cloned()
            .collect();
        Ok((selected.name.clone(), alternate_domains))
    }

    fn remote_cold_launch_domain(
        &self,
        owner: HostUid,
        authority: &RemoteWezPreflight,
    ) -> Result<(String, Vec<String>), TypedError> {
        let candidates: Vec<_> = authority
            .manifest
            .iter()
            .filter(|row| {
                row.compatible
                    && row.remote_wezterm_path.is_some()
                    && row.host_uid == owner
                    && row.backend_instance_uid == authority.backend_instance
            })
            .collect();
        let selected = choose_compatible_presentation_row(
            &authority.fresh_route,
            owner,
            authority.backend_instance,
            &candidates,
        )?;
        Ok((selected.name.clone(), selected.alternate_domains.clone()))
    }

    fn probe_new_wez_route(
        &self,
        owner: HostUid,
        route: &RouteRow,
        heartbeat_domains: &BTreeMap<String, BridgeDomainState>,
        controller: &crate::remote::wez_compat::WezCapabilityReport,
    ) -> Result<WezRouteProbe, TypedError> {
        let checked = (|| {
            let hello = self.remote_hello_pinned(owner, route.route_id)?;
            crate::remote::wez_compat::require_automatic_remote_wez_hello(controller, &hello)?;
            let backends: Vec<_> = hello
                .backends
                .iter()
                .filter(|backend| backend.backend == Backend::Wez)
                .collect();
            let [backend] = backends.as_slice() else {
                return Err(TypedError::new(
                    ErrorCode::WrongBackendInstance,
                    format!(
                        "fresh pinned hello reported {} managed Wez backend instances",
                        backends.len()
                    ),
                ));
            };
            if backend.server_epoch.is_none()
                || backend.socket_path.as_deref().is_none_or(|socket| {
                    !Path::new(socket).is_absolute() || socket.chars().any(char::is_control)
                })
            {
                return Err(unavailable(
                    "fresh pinned hello did not report one ready absolute-socket Wez incarnation",
                ));
            }
            if route.username.as_deref().is_none_or(str::is_empty) {
                return Err(unavailable(
                    "selected route has no username for an automatic Wez domain",
                ));
            }
            let domain = route
                .wez_domain
                .as_deref()
                .ok_or_else(|| unavailable("selected route has no persisted Wez domain name"))?;
            require_preflight_domain(heartbeat_domains, domain)
        })();
        match checked {
            Ok(()) => Ok(WezRouteProbe::Usable),
            Err(error) => classify_new_route_error(error),
        }
    }

    fn probe_new_wez_routes(
        &self,
        owner: HostUid,
        routes: &[RouteRow],
        heartbeat_domains: &BTreeMap<String, BridgeDomainState>,
        controller: &crate::remote::wez_compat::WezCapabilityReport,
    ) -> Result<WezRouteProbe, TypedError> {
        let mut saw_transport_failure = false;
        for route in routes {
            match self.probe_new_wez_route(owner, route, heartbeat_domains, controller)? {
                WezRouteProbe::Usable => return Ok(WezRouteProbe::Usable),
                WezRouteProbe::TransportFailed => saw_transport_failure = true,
                WezRouteProbe::AuthOrCompatFailed => {
                    // Reachable auth/version/capability failure is terminal;
                    // it is never permission to try another route or tmux.
                    return Ok(WezRouteProbe::AuthOrCompatFailed);
                }
            }
        }
        debug_assert!(saw_transport_failure || routes.is_empty());
        Ok(WezRouteProbe::TransportFailed)
    }

    fn new_creation_context(
        &mut self,
        owner: HostUid,
        explicit_backend: Option<Backend>,
        launch_gui: bool,
    ) -> Result<CreationContext, TypedError> {
        self.enrolled_host(owner)?;
        let registry = self.registry()?;
        let identity = registry.identity().map_err(typed_registry)?;
        let remote_owner = owner != identity.host_uid;

        if launch_gui && explicit_backend == Some(Backend::Tmux) {
            return Err(TypedError::new(
                ErrorCode::Usage,
                "--launch-gui is valid only with the Wez backend",
            ));
        }

        // Explicit cold Wez is already a frozen product decision. Establish
        // its attach-only lifecycle and exact route capability now so the
        // policy does not require a logically impossible ambient origin.
        // The later pre-mutation preflight repeats/revalidates the witness;
        // neither pass reserves identity or creates a Space/user pane.
        if launch_gui && explicit_backend == Some(Backend::Wez) {
            let witness = self.preflight_new_wez_presentation(owner, NewPresentationMode::Cold)?;
            if witness.owner != owner || witness.mode != NewPresentationMode::Cold {
                return Err(TypedError::new(
                    ErrorCode::ProtocolMismatch,
                    "cold Wez eligibility returned a differently scoped witness",
                ));
            }
            if !remote_owner {
                return Ok(CreationContext {
                    explicit_backend,
                    local: LocalEnv {
                        trusted_gui_bridge: true,
                        wez_service_compatible: true,
                    },
                    remote: None,
                });
            }
            let matching_routes: Vec<_> = registry
                .routes_for(owner)
                .map_err(typed_registry)?
                .into_iter()
                .filter(|route| {
                    route.enabled
                        && route.transport != Transport::Local
                        && route.wez_domain.as_deref() == Some(witness.domain.as_str())
                })
                .collect();
            let [selected_route] = matching_routes.as_slice() else {
                return Err(TypedError::new(
                    ErrorCode::HostIdentityChanged,
                    "cold Wez witness domain is not one exact enabled enrolled route",
                ));
            };
            let selected_usb = selected_route.network_class == NetworkClass::Usb;
            return Ok(CreationContext {
                explicit_backend,
                local: LocalEnv {
                    trusted_gui_bridge: true,
                    wez_service_compatible: true,
                },
                remote: Some(RemoteEnv {
                    plain_ssh: false,
                    trusted_wez_controller: true,
                    usb: if selected_usb {
                        RouteState::PositivelyUsable
                    } else {
                        RouteState::ProbeFailed
                    },
                    verified_alternate_route: !selected_usb,
                }),
            });
        }

        // An explicit tmux request is already a complete product decision;
        // GUI and network probes cannot refine it and must not introduce an
        // unrelated failure before the owner-fenced create scan.
        if explicit_backend == Some(Backend::Tmux) {
            return Ok(CreationContext {
                explicit_backend,
                local: LocalEnv {
                    trusted_gui_bridge: false,
                    wez_service_compatible: false,
                },
                remote: remote_owner.then_some(RemoteEnv {
                    plain_ssh: true,
                    trusted_wez_controller: false,
                    usb: RouteState::ProbeFailed,
                    verified_alternate_route: false,
                }),
            });
        }

        let bridge = self.creation_bridge()?;
        let trusted_gui_bridge = bridge.is_some();
        if !remote_owner {
            let wez_service_compatible = if trusted_gui_bridge {
                self.local_wez_service_compatible()?
            } else {
                false
            };
            return Ok(CreationContext {
                explicit_backend,
                local: LocalEnv {
                    trusted_gui_bridge,
                    wez_service_compatible,
                },
                remote: None,
            });
        }

        let Some(bound) = bridge else {
            return Ok(CreationContext {
                explicit_backend,
                local: LocalEnv {
                    trusted_gui_bridge: false,
                    wez_service_compatible: false,
                },
                remote: Some(RemoteEnv {
                    plain_ssh: true,
                    trusted_wez_controller: false,
                    usb: RouteState::ProbeFailed,
                    verified_alternate_route: false,
                }),
            });
        };

        let controller = match crate::remote::wez_compat::probe_wezterm_capabilities(
            &self.wezterm_bin,
            crate::remote::wez_compat::DEFAULT_PROBE_DEADLINE,
        ) {
            Ok(controller) => controller,
            Err(_) => {
                return Ok(CreationContext {
                    explicit_backend,
                    local: LocalEnv {
                        trusted_gui_bridge: true,
                        wez_service_compatible: false,
                    },
                    remote: Some(RemoteEnv {
                        plain_ssh: false,
                        trusted_wez_controller: false,
                        usb: RouteState::AuthOrCompatFailed,
                        verified_alternate_route: false,
                    }),
                });
            }
        };
        let mut routes: Vec<_> = registry
            .routes_for(owner)
            .map_err(typed_registry)?
            .into_iter()
            .filter(|route| route.enabled && route.transport != Transport::Local)
            .collect();
        routes.sort_by_key(|route| (route.priority, route.route_id));
        let usb_routes: Vec<_> = routes
            .iter()
            .filter(|route| route.network_class == NetworkClass::Usb)
            .cloned()
            .collect();
        let usb = if usb_routes.is_empty() {
            RouteState::PositivelyAbsent
        } else {
            match self.probe_new_wez_routes(
                owner,
                &usb_routes,
                &bound.heartbeat.domains,
                &controller,
            )? {
                WezRouteProbe::Usable => RouteState::PositivelyUsable,
                WezRouteProbe::TransportFailed => RouteState::ProbeFailed,
                WezRouteProbe::AuthOrCompatFailed => RouteState::AuthOrCompatFailed,
            }
        };
        let verified_alternate_route =
            if explicit_backend == Some(Backend::Wez) && usb != RouteState::PositivelyUsable {
                let alternate_routes: Vec<_> = routes
                    .into_iter()
                    .filter(|route| route.network_class != NetworkClass::Usb)
                    .collect();
                !alternate_routes.is_empty()
                    && self.probe_new_wez_routes(
                        owner,
                        &alternate_routes,
                        &bound.heartbeat.domains,
                        &controller,
                    )? == WezRouteProbe::Usable
            } else {
                false
            };
        Ok(CreationContext {
            explicit_backend,
            local: LocalEnv {
                trusted_gui_bridge: true,
                wez_service_compatible: false,
            },
            remote: Some(RemoteEnv {
                plain_ssh: false,
                trusted_wez_controller: true,
                usb,
                verified_alternate_route,
            }),
        })
    }

    fn marker_attached(heartbeat: &BridgeHeartbeat, marker: &MarkerContext) -> bool {
        heartbeat.panes.iter().any(|pane| {
            pane.context.host_uid == marker.host_uid
                && pane.context.space_uid == marker.space_uid
                && pane.context.server_epoch == marker.server_epoch
        })
    }

    fn health_token(health: Health) -> String {
        serde_json::to_value(health)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_else(|| "unknown".to_string())
    }

    fn local_space_markers(&self) -> Result<Vec<AuthorityMarker>, TypedError> {
        let registry = self.registry()?;
        let identity = registry.identity().map_err(typed_registry)?;
        let host = self.enrolled_host(identity.host_uid)?;
        let (owner_alias, owner_label) = Self::host_names(&host)?;
        let mut result = Vec::new();
        for row in registry
            .spaces()
            .map_err(typed_registry)?
            .into_iter()
            .filter(|row| row.lifecycle == Lifecycle::Active)
        {
            let info = match registry.backend_instance_info(row.backend_instance) {
                Ok(info) => info,
                Err(_) => continue,
            };
            let server = match registry.backend_server(row.backend_instance) {
                Ok(server) => server,
                Err(_) => continue,
            };
            let Some(epoch) = server.server_epoch else {
                continue;
            };
            let Ok((provider, scope)) =
                self.local_provider(row.backend_instance, info.backend, epoch)
            else {
                continue;
            };
            let Ok(hierarchy) =
                operations::hierarchy(&self.env, provider.as_ref(), &scope, row.space_uid)
            else {
                continue;
            };
            let Some(group) = hierarchy.groups.first() else {
                continue;
            };
            let Some(split) = group.splits.first() else {
                continue;
            };
            result.push(AuthorityMarker {
                marker: MarkerContext {
                    host_uid: identity.host_uid,
                    space_uid: row.space_uid,
                    space_no: row.space_no,
                    backend: info.backend,
                    domain: (info.backend == Backend::Wez).then(|| LOCAL_WEZ_DOMAIN.to_string()),
                    server_epoch: epoch,
                    group_ref: group.group_ref.clone(),
                    split_ref: split.split_ref.clone(),
                },
                backend_instance: row.backend_instance,
                logical_name: row.logical_name,
                health: row.health,
                hierarchy,
                owner_alias: owner_alias.clone(),
                owner_label: owner_label.clone(),
                route: "local".to_string(),
                location: AuthorityLocation::Local,
            });
        }
        result.sort_by_key(|space| space.marker.space_no);
        Ok(result)
    }

    fn remote_space_markers(&self, host: &HostRow) -> Result<Vec<AuthorityMarker>, TypedError> {
        let (owner_alias, owner_label) = Self::host_names(host)?;
        let (hello, route_id) = self.remote_hello(host.host_uid)?;
        let spaces = self.remote_spaces(host.host_uid)?;
        let registry = self.registry()?;
        let route = registry
            .routes_for(host.host_uid)
            .map_err(typed_registry)?
            .into_iter()
            .find(|route| route.route_id == route_id)
            .and_then(|route| route.wez_domain.or(Some(route.endpoint)))
            .unwrap_or_else(|| "remote".to_string());
        let mut result = Vec::new();
        for space in spaces
            .spaces
            .iter()
            .filter(|space| space.lifecycle == Lifecycle::Active)
        {
            let epochs: Vec<_> = hello
                .backends
                .iter()
                .filter(|backend| {
                    backend.backend == space.backend
                        && backend.backend_instance_uid == space.backend_instance_uid
                })
                .filter_map(|backend| backend.server_epoch)
                .collect();
            let [epoch] = epochs.as_slice() else {
                continue;
            };
            if !spaces.scans.iter().any(|scan| {
                scan.backend == space.backend
                    && scan.outcome == "complete"
                    && scan.server_epoch == Some(*epoch)
            }) {
                continue;
            }
            let Ok(hierarchy) = self.remote_hierarchy(
                host.host_uid,
                space.space_uid,
                space.backend_instance_uid,
                *epoch,
            ) else {
                continue;
            };
            let Some(group) = hierarchy.groups.first() else {
                continue;
            };
            let Some(split) = group.splits.first() else {
                continue;
            };
            let marker = MarkerContext {
                host_uid: host.host_uid,
                space_uid: space.space_uid,
                space_no: SpaceNo(std::num::NonZeroU64::new(space.space_no).ok_or_else(|| {
                    TypedError::new(ErrorCode::ProtocolMismatch, "owner returned SpaceNo zero")
                })?),
                backend: space.backend,
                domain: (space.backend == Backend::Wez).then(|| route.clone()),
                server_epoch: *epoch,
                group_ref: group.group_ref.clone(),
                split_ref: split.split_ref.clone(),
            };
            result.push(self.correlate_remote_marker(
                &marker,
                space,
                &hierarchy,
                owner_alias.clone(),
                owner_label.clone(),
                route.clone(),
            )?);
        }
        result.sort_by_key(|space| space.marker.space_no);
        Ok(result)
    }

    fn all_space_markers(&self) -> Result<Vec<AuthorityMarker>, TypedError> {
        let registry = self.registry()?;
        let identity = registry.identity().map_err(typed_registry)?;
        let hosts = registry.hosts().map_err(typed_registry)?;
        let mut spaces = self.local_space_markers()?;
        for host in hosts.into_iter().filter(|host| {
            host.lifecycle == HostLifecycle::Enrolled && host.host_uid != identity.host_uid
        }) {
            // The picker never substitutes stale cache rows for a failed
            // authority read. Other enrolled hosts remain usable.
            if let Ok(mut remote) = self.remote_space_markers(&host) {
                spaces.append(&mut remote);
            }
        }
        Ok(spaces)
    }

    fn gui_space_rows(
        &self,
        heartbeat: &BridgeHeartbeat,
        tmux_scope: Option<(HostUid, BackendInstanceUid, ServerEpoch)>,
    ) -> Result<Vec<GuiSpaceRow>, TypedError> {
        let spaces = self.all_space_markers()?;
        let needs_remote_manifest = spaces.iter().any(|space| {
            space.marker.backend == Backend::Wez
                && matches!(&space.location, AuthorityLocation::Remote)
        });
        let manifest = if needs_remote_manifest {
            self.remote_domain_manifest()?
        } else {
            Vec::new()
        };
        let rows: Vec<GuiSpaceRow> = spaces
            .iter()
            .filter(|space| {
                if space.marker.backend == Backend::Tmux {
                    return tmux_scope.is_some_and(|(host, instance, epoch)| {
                        space.marker.host_uid == host
                            && space.backend_instance == instance
                            && space.marker.server_epoch == epoch
                    });
                }
                match &space.location {
                    AuthorityLocation::Local => heartbeat.domains.contains_key(LOCAL_WEZ_DOMAIN),
                    AuthorityLocation::Remote => manifest.iter().any(|row| {
                        row.compatible
                            && row.host_uid == space.marker.host_uid
                            && row.backend_instance_uid == space.backend_instance
                            && heartbeat.domains.contains_key(&row.name)
                    }),
                }
            })
            .map(|space| GuiSpaceRow {
                stable_ref: format!("{}{}", space.owner_alias, space.marker.space_no),
                name: space.logical_name.clone(),
                backend: space.marker.backend,
                owner_alias: space.owner_alias.clone(),
                owner_label: space.owner_label.clone(),
                route: space.route.clone(),
                attached: Self::marker_attached(heartbeat, &space.marker),
                health: Self::health_token(space.health),
            })
            .collect();
        // Row shape is shared across both typed backends; correlated tmux
        // eligibility was already proved by the authority-layer filter.
        gui::validate_space_rows(&rows).map_err(typed_gui)?;
        Ok(rows)
    }

    /// The host table's answer for a token a ref embeds: a UID must be
    /// enrolled (the identity-class refusal this file has always given); an
    /// alias or label resolves by the production rule.
    fn resolve_host_token(&self, token: &HostToken) -> Result<HostUid, TypedError> {
        match token {
            HostToken::Uid(uid) => self.enrolled_host(*uid).map(|host| host.host_uid),
            HostToken::AliasOrLabel(_) => {
                let hosts = self.registry()?.hosts().map_err(typed_registry)?;
                resolve_enrolled_host(&hosts, token)
            }
        }
    }

    /// Resolve a Space ref to its live-correlated owner marker. Scoping and
    /// lookup are the resolver's (`resolve::resolve_space_ref`, ADR 012
    /// WS-D.3); `default_host` is the owner a bare ref means here — the
    /// bound GUI's authority, not `--host`.
    fn resolve_space(
        &self,
        reference: &str,
        default_host: HostUid,
    ) -> Result<AuthorityMarker, TypedError> {
        let parsed: ParsedRef = parse_ref(reference).map_err(|error| {
            TypedError::new(
                ErrorCode::InvalidRef,
                format!("invalid Space ref {reference:?}: {error:?}"),
            )
        })?;
        if parsed.child.is_some() {
            return Err(TypedError::new(
                ErrorCode::InvalidRef,
                "present --space requires a Space ref without a child suffix",
            ));
        }
        let identity = self.registry()?.identity().map_err(typed_registry)?;
        let (_, resolution) = resolve_space_ref(
            SpaceSelector::Shape(&parsed.space),
            HostContext {
                local: default_host,
                explicit: None,
            },
            |token| self.resolve_host_token(token),
            |owner| {
                let host = self.enrolled_host(owner)?;
                let markers = if owner == identity.host_uid {
                    self.local_space_markers()?
                } else {
                    // An explicit host-qualified lookup must surface that
                    // owner's route/authority failure. It must not be
                    // converted to a false local-looking NotFound by the
                    // best-effort all-host picker.
                    self.remote_space_markers(&host)?
                };
                Ok(markers
                    .into_iter()
                    .filter(|candidate| candidate.marker.host_uid == owner)
                    .collect())
            },
        )?;
        match resolution {
            RefResolution::Space(marker) => Ok(marker),
            RefResolution::NotFound | RefResolution::Deleted(_) => Err(TypedError::new(
                ErrorCode::NotFound,
                format!("no live Space matches {reference:?}"),
            )),
            RefResolution::AmbiguousName(_) => Err(TypedError::new(
                ErrorCode::AmbiguousTarget,
                format!("Space ref {reference:?} matches more than one backend"),
            )),
        }
    }

    /// Resolve only durable owner identity/backend facts. This deliberately
    /// avoids local provider construction, the managed Wez descriptor, and
    /// every GUI lifecycle seam so a stopped local Wez instance can still be
    /// proven as the requested backend before it is started.
    fn resolve_cold_space_identity(
        &self,
        reference: &str,
        default_host: HostUid,
    ) -> Result<ColdSpaceIdentity, TypedError> {
        let parsed: ParsedRef = parse_ref(reference).map_err(|error| {
            TypedError::new(
                ErrorCode::InvalidRef,
                format!("invalid Space ref {reference:?}: {error:?}"),
            )
        })?;
        if parsed.child.is_some() {
            return Err(TypedError::new(
                ErrorCode::InvalidRef,
                "--launch-gui requires a Space ref without a child suffix",
            ));
        }
        let registry = self.registry()?;
        let identity = registry.identity().map_err(typed_registry)?;
        let (scoped, resolution) = resolve_space_ref(
            SpaceSelector::Shape(&parsed.space),
            HostContext {
                local: default_host,
                explicit: None,
            },
            |token| self.resolve_host_token(token),
            |owner| {
                self.enrolled_host(owner)?;
                if owner == identity.host_uid {
                    Ok(registry
                        .spaces()
                        .map_err(typed_registry)?
                        .into_iter()
                        .filter(|row| row.lifecycle == Lifecycle::Active)
                        .map(|row| ColdCandidate {
                            space_uid: row.space_uid,
                            space_no: row.space_no,
                            logical_name: row.logical_name,
                            backend: ColdBackend::Registered(row.backend_instance),
                        })
                        .collect())
                } else {
                    // `spaces` is an owner read through identity/lineage-
                    // validated routing. It performs no local service or GUI
                    // mutation.
                    self.remote_spaces(owner)?
                        .spaces
                        .into_iter()
                        .filter(|space| space.lifecycle == Lifecycle::Active)
                        .map(|space| {
                            let space_no = std::num::NonZeroU64::new(space.space_no)
                                .map(SpaceNo)
                                .ok_or_else(|| {
                                TypedError::new(
                                    ErrorCode::ProtocolMismatch,
                                    "owner returned SpaceNo zero during cold target preflight",
                                )
                            })?;
                            Ok(ColdCandidate {
                                space_uid: space.space_uid,
                                space_no,
                                logical_name: space.name,
                                backend: ColdBackend::Known(space.backend),
                            })
                        })
                        .collect()
                }
            },
        )?;
        match resolution {
            RefResolution::Space(candidate) => Ok(ColdSpaceIdentity {
                host_uid: scoped.owner,
                space_uid: candidate.space_uid,
                backend: match candidate.backend {
                    ColdBackend::Known(backend) => backend,
                    ColdBackend::Registered(instance) => {
                        registry
                            .backend_instance_info(instance)
                            .map_err(typed_registry)?
                            .backend
                    }
                },
            }),
            RefResolution::NotFound | RefResolution::Deleted(_) => Err(TypedError::new(
                ErrorCode::NotFound,
                format!("no durable active Space matches {reference:?}"),
            )),
            RefResolution::AmbiguousName(_) => Err(TypedError::new(
                ErrorCode::AmbiguousTarget,
                format!("Space ref {reference:?} matches more than one backend"),
            )),
        }
    }

    fn resolve_connect_authority(
        &self,
        query: &OwnerConnectQuery,
    ) -> Result<AuthorityMarker, TypedError> {
        let host = self.enrolled_host(query.owner)?;
        let registry = self.registry()?;
        let identity = registry.identity().map_err(typed_registry)?;
        let owner_markers = if query.owner == identity.host_uid {
            self.local_space_markers()?
        } else {
            self.remote_space_markers(&host)?
        };
        let locator_matches: Vec<_> = owner_markers
            .into_iter()
            .filter(|candidate| connect_locator_matches(&query.locator, candidate))
            .collect();
        let mut candidates: Vec<_> = locator_matches
            .iter()
            .filter(|candidate| {
                query
                    .backend_filter
                    .is_none_or(|backend| candidate.marker.backend == backend)
            })
            .cloned()
            .collect();

        // Compare the live-correlated set with every durable row in the
        // exact owner scope. A second same-name active record whose provider
        // scan failed must make the lookup unavailable/ambiguous; it cannot
        // disappear and let the surviving backend win accidentally.
        let durable_all: Vec<(Lifecycle, Backend)> = if query.owner == identity.host_uid {
            let mut rows = Vec::new();
            for row in registry.spaces().map_err(typed_registry)? {
                let info = registry
                    .backend_instance_info(row.backend_instance)
                    .map_err(typed_registry)?;
                if query
                    .locator
                    .matches(row.space_uid, row.space_no, &row.logical_name)
                {
                    rows.push((row.lifecycle, info.backend));
                }
            }
            rows
        } else {
            self.remote_spaces(query.owner)?
                .spaces
                .into_iter()
                .filter_map(|space| {
                    let space_no = std::num::NonZeroU64::new(space.space_no).map(SpaceNo);
                    let locator_match = match &query.locator {
                        OwnerLocator::Uid(uid) => space.space_uid == *uid,
                        OwnerLocator::Number(no) => space_no == Some(*no),
                        OwnerLocator::Name(name) => space.name == *name,
                    };
                    locator_match.then_some((space.lifecycle, space.backend))
                })
                .collect()
        };
        let durable_filtered: Vec<_> = durable_all
            .iter()
            .filter(|(_, backend)| {
                query
                    .backend_filter
                    .is_none_or(|expected| *backend == expected)
            })
            .copied()
            .collect();
        if query.backend_filter.is_some() && durable_filtered.is_empty() && !durable_all.is_empty()
        {
            return Err(TypedError::new(
                ErrorCode::BackendMismatch,
                "connect backend constraint contradicts the durable owner Space",
            ));
        }
        if durable_filtered.iter().any(|(lifecycle, _)| {
            matches!(
                lifecycle,
                Lifecycle::Reserved | Lifecycle::Deleting | Lifecycle::Conflict
            )
        }) {
            return Err(TypedError::new(
                ErrorCode::RepairRequired,
                "the selected owner scope contains a blocking non-active Space record",
            ));
        }
        let active_durable = durable_filtered
            .iter()
            .filter(|(lifecycle, _)| *lifecycle == Lifecycle::Active)
            .count();
        if active_durable > candidates.len() {
            return Err(unavailable(
                "one or more matching active Spaces could not be proven by complete live owner scans",
            ));
        }
        match candidates.len() {
            1 => Ok(candidates.remove(0)),
            count if count > 1 => Err(TypedError::new(
                ErrorCode::AmbiguousTarget,
                "owner-scoped connect query matches more than one live backend",
            )),
            _ if query.backend_filter.is_some() && locator_matches.len() == 1 => {
                Err(TypedError::new(
                    ErrorCode::BackendMismatch,
                    "connect backend constraint contradicts the owner-verified Space",
                ))
            }
            _ => {
                if durable_filtered.iter().any(|(lifecycle, _)| {
                    matches!(lifecycle, Lifecycle::Deleted | Lifecycle::Aborted)
                }) {
                    Err(TypedError::new(
                        ErrorCode::SpaceDeleted,
                        "the selected Space is terminal and cannot be connected",
                    ))
                } else {
                    Err(TypedError::new(
                        ErrorCode::NotFound,
                        "no owner-validated Space matches the connect query",
                    ))
                }
            }
        }
    }

    fn correlate_connect_child(
        authority: &AuthorityMarker,
        requested: Option<&RequestedChild>,
    ) -> Result<Option<VerifiedConnectChild>, TypedError> {
        let Some(requested) = requested else {
            return Ok(None);
        };
        if requested.epoch != authority.marker.server_epoch {
            return Err(TypedError::new(
                ErrorCode::BackendEpochChanged,
                "requested child belongs to another backend epoch",
            ));
        }
        let suffix = child_suffix(&ChildRefShape {
            kind: requested.kind,
            epoch: requested.epoch,
            handle: requested.handle.clone(),
        });
        match requested.kind {
            ChildKind::Group => {
                let matches = authority
                    .hierarchy
                    .groups
                    .iter()
                    .filter(|group| group.group_ref == suffix)
                    .count();
                if matches != 1 {
                    return Err(TypedError::new(
                        ErrorCode::NotFound,
                        "requested Group is absent or ambiguous in the exact live hierarchy",
                    ));
                }
                Ok(Some(VerifiedConnectChild::Group {
                    epoch: requested.epoch,
                    handle: requested.handle.clone(),
                }))
            }
            ChildKind::Split => {
                let parents: Vec<_> = authority
                    .hierarchy
                    .groups
                    .iter()
                    .filter(|group| group.splits.iter().any(|split| split.split_ref == suffix))
                    .collect();
                let [parent] = parents.as_slice() else {
                    return Err(TypedError::new(
                        ErrorCode::NotFound,
                        "requested Split is absent or has ambiguous live parentage",
                    ));
                };
                let group = Self::parse_child(&parent.group_ref, ChildKind::Group)?.handle;
                Ok(Some(VerifiedConnectChild::Split {
                    epoch: requested.epoch,
                    group,
                    split: requested.handle.clone(),
                }))
            }
        }
    }

    fn frozen_binding_for_authority(
        &self,
        authority: &AuthorityMarker,
    ) -> Result<FrozenBinding, TypedError> {
        match authority.location {
            AuthorityLocation::Local => {
                let registry = self.registry()?;
                let binding = registry
                    .current_binding(authority.marker.space_uid)
                    .map_err(typed_registry)?
                    .ok_or_else(|| {
                        TypedError::new(
                            ErrorCode::SpaceAbsent,
                            "owner-validated local Space has no current native binding",
                        )
                    })?;
                let (provider, scope) = self.local_bound_provider(authority)?;
                match provider.inventory(&scope) {
                    InventoryOutcome::Complete(inventory)
                        if inventory.server_epoch == Some(authority.marker.server_epoch)
                            && inventory
                                .rows
                                .iter()
                                .filter(|row| row.native_token == binding.native_token)
                                .count()
                                == 1 => {}
                    outcome => {
                        return Err(unavailable(format!(
                            "local connect binding was not uniquely present in the exact live scan: {outcome:?}"
                        )));
                    }
                }
                Ok(FrozenBinding {
                    native_token: binding.native_token,
                    endpoint: scope.endpoint,
                })
            }
            AuthorityLocation::Remote => {
                let (hello, _) = self.remote_hello(authority.marker.host_uid)?;
                let spaces = self.remote_spaces(authority.marker.host_uid)?;
                let matches: Vec<_> = spaces
                    .spaces
                    .iter()
                    .filter(|space| {
                        space.space_uid == authority.marker.space_uid
                            && space.backend == authority.marker.backend
                            && space.backend_instance_uid == authority.backend_instance
                            && space.lifecycle == Lifecycle::Active
                    })
                    .collect();
                let [space] = matches.as_slice() else {
                    return Err(TypedError::new(
                        ErrorCode::IdentityConflict,
                        "remote connect binding Space is absent or ambiguous",
                    ));
                };
                let native_token = space.native_token.clone().ok_or_else(|| {
                    unavailable("remote owner omitted the current binding from its complete scan")
                })?;
                if !spaces.scans.iter().any(|scan| {
                    scan.backend == authority.marker.backend
                        && scan.outcome == "complete"
                        && scan.server_epoch == Some(authority.marker.server_epoch)
                }) {
                    return Err(unavailable(
                        "remote owner did not return a complete same-epoch binding scan",
                    ));
                }
                let backends: Vec<_> = hello
                    .backends
                    .iter()
                    .filter(|backend| {
                        backend.backend == authority.marker.backend
                            && backend.backend_instance_uid == authority.backend_instance
                            && backend.server_epoch == Some(authority.marker.server_epoch)
                    })
                    .collect();
                let [backend] = backends.as_slice() else {
                    return Err(TypedError::new(
                        ErrorCode::WrongBackendInstance,
                        "remote hello did not prove the connect binding backend incarnation",
                    ));
                };
                let endpoint = backend.socket_path.clone().ok_or_else(|| {
                    unavailable("remote hello omitted the connect binding endpoint")
                })?;
                Ok(FrozenBinding {
                    native_token,
                    endpoint,
                })
            }
        }
    }

    fn resolve_connect_query_with_authority(
        &self,
        query: &OwnerConnectQuery,
    ) -> Result<(AuthorityMarker, FrozenConnectTarget), TypedError> {
        let authority = self.resolve_connect_authority(query)?;
        let binding = self.frozen_binding_for_authority(&authority)?;
        let child = Self::correlate_connect_child(&authority, query.child.as_ref())?;
        let frozen = FrozenConnectTarget {
            owner: authority.marker.host_uid,
            space_uid: authority.marker.space_uid,
            space_no: authority.marker.space_no,
            logical_name: authority.logical_name.clone(),
            backend: authority.marker.backend,
            backend_instance_uid: authority.backend_instance,
            server_epoch: authority.marker.server_epoch,
            binding,
            child,
        };
        Ok((authority, frozen))
    }

    fn resolve_connect_query(
        &self,
        query: &OwnerConnectQuery,
    ) -> Result<FrozenConnectTarget, TypedError> {
        self.resolve_connect_query_with_authority(query)
            .map(|(_, target)| target)
    }

    fn revalidate_frozen_connect_target(
        &self,
        target: &FrozenConnectTarget,
    ) -> Result<(AuthorityMarker, FrozenConnectTarget), TypedError> {
        let (authority, refreshed) =
            self.resolve_connect_query_with_authority(&frozen_connect_query(target))?;
        require_same_frozen_connect_target(target, &refreshed)?;
        Ok((authority, refreshed))
    }

    fn presentation_domain(
        &self,
        heartbeat: &BridgeHeartbeat,
        target: &AuthorityMarker,
    ) -> Result<(String, Vec<String>), TypedError> {
        if target.marker.backend != Backend::Wez {
            return Err(TypedError::new(
                ErrorCode::BackendMismatch,
                "tmux presentation is not a GUI bridge action; use dmux con/attach from a terminal",
            ));
        }
        if matches!(target.location, AuthorityLocation::Local) {
            return Ok((LOCAL_WEZ_DOMAIN.to_string(), Vec::new()));
        }
        let manifest = self.remote_domain_manifest()?;
        let candidates: Vec<_> = manifest
            .iter()
            .filter(|row| {
                row.host_uid == target.marker.host_uid
                    && row.backend_instance_uid == target.backend_instance
                    && row.compatible
                    && heartbeat.domains.contains_key(&row.name)
            })
            .collect();
        if candidates.is_empty() {
            return Err(unavailable(
                "no exact-build compatible GUI route exists for this remote Wez instance",
            ));
        }
        // The marker's route is the one the owner handshake just proved, so it
        // leads §8.4's order here; the bridge detaches any stale alternate to
        // the same backend instance before attaching it.
        let selected = choose_compatible_presentation_row(
            &target.route,
            target.marker.host_uid,
            target.backend_instance,
            &candidates,
        )?;
        let alternates = selected
            .alternate_domains
            .iter()
            .filter(|name| heartbeat.domains.contains_key(*name))
            .cloned()
            .collect();
        Ok((selected.name.clone(), alternates))
    }

    fn summon_target_for_authority(
        &self,
        authority: &AuthorityMarker,
        ready: &crate::gui_lifecycle::ReadyWezService,
        manifest: &[GuiDomainManifestRow],
        configured_domains: Option<&BTreeMap<String, BridgeDomainState>>,
    ) -> Result<Option<SummonTarget>, TypedError> {
        if authority.marker.backend != Backend::Wez {
            return Ok(None);
        }
        match &authority.location {
            AuthorityLocation::Local => {
                if authority.backend_instance != ready.backend_instance_uid
                    || authority.marker.server_epoch != ready.server_epoch
                    || configured_domains
                        .is_some_and(|domains| !domains.contains_key(LOCAL_WEZ_DOMAIN))
                {
                    return Ok(None);
                }
                Ok(Some(SummonTarget {
                    authority: authority.clone(),
                    domain: LOCAL_WEZ_DOMAIN.to_string(),
                    alternate_domains: Vec::new(),
                }))
            }
            AuthorityLocation::Remote => {
                let candidates: Vec<_> = manifest
                    .iter()
                    .filter(|row| {
                        row.compatible
                            && row.remote_wezterm_path.is_some()
                            && row.host_uid == authority.marker.host_uid
                            && row.backend_instance_uid == authority.backend_instance
                            && configured_domains
                                .is_none_or(|domains| domains.contains_key(&row.name))
                    })
                    .collect();
                if candidates.is_empty() {
                    return Ok(None);
                }
                let selected = choose_compatible_presentation_row(
                    &authority.route,
                    authority.marker.host_uid,
                    authority.backend_instance,
                    &candidates,
                )?;
                let alternate_domains = selected
                    .alternate_domains
                    .iter()
                    .filter(|name| {
                        configured_domains.is_none_or(|domains| domains.contains_key(*name))
                    })
                    .cloned()
                    .collect();
                Ok(Some(SummonTarget {
                    authority: authority.clone(),
                    domain: selected.name.clone(),
                    alternate_domains,
                }))
            }
        }
    }

    fn summon_targets(
        &self,
        ready: &crate::gui_lifecycle::ReadyWezService,
        manifest: &[GuiDomainManifestRow],
        configured_domains: Option<&BTreeMap<String, BridgeDomainState>>,
    ) -> Result<Vec<SummonTarget>, TypedError> {
        let mut targets = Vec::new();
        for authority in self.all_space_markers()? {
            if let Some(target) =
                self.summon_target_for_authority(&authority, ready, manifest, configured_domains)?
            {
                targets.push(target);
            }
        }
        Ok(targets)
    }

    fn choose_summon_target(
        &self,
        mut candidates: Vec<SummonTarget>,
    ) -> Result<SummonTarget, TypedError> {
        if let Some(history) = self.history.last_gui_presented() {
            let matching: Vec<_> = candidates
                .iter()
                .enumerate()
                .filter(|(_, candidate)| {
                    candidate.authority.marker.host_uid == history.host_uid
                        && candidate.authority.marker.space_uid == history.space_uid
                })
                .map(|(index, _)| index)
                .collect();
            match matching.as_slice() {
                [index] => return Ok(candidates.swap_remove(*index)),
                [] => {}
                _ => {
                    return Err(TypedError::new(
                        ErrorCode::IdentityConflict,
                        "global GUI history target matches multiple live authorities",
                    ));
                }
            }
        }
        match candidates.len() {
            0 => Err(TypedError::new(
                ErrorCode::NotFound,
                "summon has no live GUI-presentable Wez Space",
            )),
            1 => Ok(candidates.remove(0)),
            count => Err(TypedError::new(
                ErrorCode::AmbiguousTarget,
                format!(
                    "summon history is absent/invalid and {count} live GUI-presentable Wez Spaces remain"
                ),
            )),
        }
    }

    fn cold_launch_intent_for_summon_target(
        &self,
        target: &SummonTarget,
        launcher_request_uid: Uuid,
    ) -> Result<crate::gui_lifecycle::ColdLaunchIntent, TypedError> {
        let preflight = WezPresentationPreflight {
            owner: target.authority.marker.host_uid,
            backend_instance_uid: target.authority.backend_instance,
            server_epoch: target.authority.marker.server_epoch,
            gui_instance: format!("gui-{}", launcher_request_uid.simple()),
            domain: target.domain.clone(),
            alternate_domains: target.alternate_domains.clone(),
            mode: NewPresentationMode::Cold,
        };
        let frozen = FrozenConnectTarget {
            owner: target.authority.marker.host_uid,
            space_uid: target.authority.marker.space_uid,
            space_no: target.authority.marker.space_no,
            logical_name: target.authority.logical_name.clone(),
            backend: target.authority.marker.backend,
            backend_instance_uid: target.authority.backend_instance,
            server_epoch: target.authority.marker.server_epoch,
            binding: self.frozen_binding_for_authority(&target.authority)?,
            child: None,
        };
        crate::gui_lifecycle::ColdLaunchIntent::from_existing_target(
            &preflight,
            &frozen,
            launcher_request_uid,
        )
    }

    fn establish_resident_gui(
        &self,
        witness: &crate::gui_lifecycle::ColdLauncherWitness,
    ) -> Result<Value, TypedError> {
        let intent = witness.intent();
        let origin = gui::cold_launcher_origin(
            witness.gui_instance().to_string(),
            witness.process().uid,
            u64::from(witness.process().pid),
            witness.process().start_token.clone(),
            witness.launcher_request_uid(),
            intent.domain().to_string(),
            intent.owner(),
            intent.backend_instance_uid(),
            intent.server_epoch(),
            intent.space_uid(),
        )
        .map_err(typed_gui)?;
        let mut target = serde_json::json!({
            "backend_instance_uid": intent.backend_instance_uid(),
            "domain": intent.domain(),
            "host_uid": intent.owner(),
            "server_epoch": intent.server_epoch(),
        });
        if let Some(space_uid) = intent.space_uid() {
            target["space_uid"] =
                serde_json::to_value(space_uid).expect("SpaceUid always serializes");
        }
        let mut request =
            gui::request_document("establish_resident", target, origin).map_err(typed_gui)?;
        gui::call_instance(
            &self.runtime_dir,
            witness.gui_instance(),
            &mut request,
            gui::ACK_TIMEOUT,
        )
        .map_err(typed_gui)
    }

    fn prove_resident_gui(&self, instance: &BridgeInstanceSelection) -> Result<Value, TypedError> {
        let origin = gui::resident_gui_origin(instance);
        let mut request =
            gui::request_document("ping", serde_json::json!({}), origin).map_err(typed_gui)?;
        gui::call_instance(
            &self.runtime_dir,
            &instance.gui_instance,
            &mut request,
            gui::ACK_TIMEOUT,
        )
        .map_err(typed_gui)
    }

    fn cold_bridge_present(
        &self,
        instance: &BridgeInstanceSelection,
        target: &SummonTarget,
        group_ref: Option<&str>,
        split_ref: Option<&str>,
    ) -> Result<Value, TypedError> {
        if split_ref.is_some() && group_ref.is_none() {
            return Err(TypedError::new(
                ErrorCode::InvalidRef,
                "cold Split presentation requires its exact Group parent",
            ));
        }
        let domain_state = instance.domains.get(&target.domain).ok_or_else(|| {
            TypedError::new(
                ErrorCode::BridgeUnavailable,
                format!(
                    "cold presentation target domain {:?} is absent from the exact GUI heartbeat",
                    target.domain
                ),
            )
        })?;
        let action = match domain_state.state.as_str() {
            "Attached" => "activate",
            "Detached" => "present",
            other => {
                return Err(TypedError::new(
                    ErrorCode::PostconditionFailed,
                    format!("cold presentation target domain is in transient state {other:?}"),
                ));
            }
        };
        let origin = gui::resident_gui_origin(instance);
        let mut target_json = serde_json::json!({
            "backend_instance_uid": target.authority.backend_instance,
            "domain": target.domain,
            "host_uid": target.authority.marker.host_uid,
            "server_epoch": target.authority.marker.server_epoch,
            "space_uid": target.authority.marker.space_uid,
            "workspace": format!(
                "dmux:{}:{}",
                target.authority.marker.host_uid.0,
                target.authority.marker.space_uid.0
            ),
        });
        if let Some(group_ref) = group_ref {
            target_json["group_ref"] = Value::String(group_ref.to_string());
        }
        if let Some(split_ref) = split_ref {
            target_json["split_ref"] = Value::String(split_ref.to_string());
        }
        if action == "present" && !target.alternate_domains.is_empty() {
            target_json["alternate_domains"] =
                serde_json::to_value(&target.alternate_domains).expect("domain names serialize");
        }
        let mut request = gui::request_document(action, target_json, origin).map_err(typed_gui)?;
        let ack = gui::call_instance(
            &self.runtime_dir,
            &instance.gui_instance,
            &mut request,
            gui::ACK_TIMEOUT,
        )
        .map_err(typed_gui)?;
        self.history
            .record_gui_present(
                target.authority.marker.host_uid,
                target.authority.marker.space_uid,
            )
            .map_err(|error| {
                TypedError::new(
                    ErrorCode::OperationFailed,
                    format!("recording GUI Space history after cold presentation: {error}"),
                )
            })?;
        Ok(ack)
    }

    fn launched_gui_partial(&mut self, action: &str, error: TypedError) -> TypedError {
        self.partial_result = Some(serde_json::json!({
            "launched_gui": true,
            "presented": false,
        }));
        TypedError::new(
            ErrorCode::PartialResult,
            format!(
                "attach-only GUI launched, but {action} failed: {}",
                error.message
            ),
        )
    }

    /// Freeze an exact GUI presentation path before `new` reserves identity
    /// or asks an owner to mutate either backend. Ambient mode binds the
    /// invoking pane to one fresh GUI heartbeat. Cold mode may start the
    /// fixed local service and launch only the attach-only GUI, but creates
    /// no Space/pane and rechecks the selected remote owner after launch.
    fn preflight_new_wez_presentation(
        &mut self,
        owner: HostUid,
        mode: NewPresentationMode,
    ) -> Result<WezPresentationPreflight, TypedError> {
        self.partial_result = None;
        let registry = self.registry()?;
        let identity = registry.identity().map_err(typed_registry)?;
        self.enrolled_host(owner)?;
        let local_owner = owner == identity.host_uid;

        match mode {
            NewPresentationMode::Ambient => {
                let origin = self.ambient_origin()?;
                let bound = <Self as GuiAuthority>::bind_origin(self, &origin)?;
                if local_owner {
                    let ready = crate::gui_lifecycle::ensure_ready_wez_service(
                        &registry,
                        &self.runtime_dir,
                        &self.wezterm_bin,
                        &self.wezterm_config,
                    )?;
                    require_preflight_domain(&bound.heartbeat.domains, LOCAL_WEZ_DOMAIN)?;
                    Ok(WezPresentationPreflight {
                        owner,
                        backend_instance_uid: ready.backend_instance_uid,
                        server_epoch: ready.server_epoch,
                        gui_instance: bound.selection.gui_instance,
                        domain: LOCAL_WEZ_DOMAIN.to_string(),
                        alternate_domains: Vec::new(),
                        mode,
                    })
                } else {
                    let remote = self.remote_wez_preflight(owner)?;
                    let (domain, alternate_domains) =
                        self.remote_preflight_domain(owner, &bound.heartbeat.domains, &remote)?;
                    Ok(WezPresentationPreflight {
                        owner,
                        backend_instance_uid: remote.backend_instance,
                        server_epoch: remote.server_epoch,
                        gui_instance: bound.selection.gui_instance,
                        domain,
                        alternate_domains,
                        mode,
                    })
                }
            }
            NewPresentationMode::Cold => {
                // A remote incompatibility or missing route is known before
                // starting the controller's fixed service or opening a GUI.
                let remote_before = if local_owner {
                    None
                } else {
                    Some(self.remote_wez_preflight(owner)?)
                };
                let ready = crate::gui_lifecycle::ensure_ready_wez_service(
                    &registry,
                    &self.runtime_dir,
                    &self.wezterm_bin,
                    &self.wezterm_config,
                )?;
                let heartbeat_source = crate::gui_lifecycle::RuntimeHeartbeatSource;
                let live = crate::gui_lifecycle::HeartbeatSource::live_instances(
                    &heartbeat_source,
                    &self.runtime_dir,
                )?;
                match live.as_slice() {
                    [instance] => {
                        self.prove_resident_gui(instance)?;
                        if local_owner {
                            require_preflight_domain(&instance.domains, LOCAL_WEZ_DOMAIN)?;
                            Ok(WezPresentationPreflight {
                                owner,
                                backend_instance_uid: ready.backend_instance_uid,
                                server_epoch: ready.server_epoch,
                                gui_instance: instance.gui_instance.clone(),
                                domain: LOCAL_WEZ_DOMAIN.to_string(),
                                alternate_domains: Vec::new(),
                                mode,
                            })
                        } else {
                            let remote = self.remote_wez_preflight(owner)?;
                            let before = remote_before
                                .as_ref()
                                .expect("remote owner preflight was established");
                            if remote.backend_instance != before.backend_instance
                                || remote.server_epoch != before.server_epoch
                            {
                                return Err(TypedError::new(
                                    ErrorCode::BackendEpochChanged,
                                    "selected remote Wez backend changed while preparing its attach-only GUI",
                                ));
                            }
                            let (domain, alternate_domains) =
                                self.remote_preflight_domain(owner, &instance.domains, &remote)?;
                            Ok(WezPresentationPreflight {
                                owner,
                                backend_instance_uid: remote.backend_instance,
                                server_epoch: remote.server_epoch,
                                gui_instance: instance.gui_instance.clone(),
                                domain,
                                alternate_domains,
                                mode,
                            })
                        }
                    }
                    [] => {
                        let launcher_request_uid = Uuid::new_v4();
                        let (backend_instance_uid, server_epoch, domain, alternate_domains) =
                            if local_owner {
                                (
                                    ready.backend_instance_uid,
                                    ready.server_epoch,
                                    LOCAL_WEZ_DOMAIN.to_string(),
                                    Vec::new(),
                                )
                            } else {
                                let before = remote_before
                                    .as_ref()
                                    .expect("remote owner preflight was established");
                                let (domain, alternate_domains) =
                                    self.remote_cold_launch_domain(owner, before)?;
                                (
                                    before.backend_instance,
                                    before.server_epoch,
                                    domain,
                                    alternate_domains,
                                )
                            };
                        let preflight = WezPresentationPreflight {
                            owner,
                            backend_instance_uid,
                            server_epoch,
                            gui_instance: format!("gui-{}", launcher_request_uid.simple()),
                            domain,
                            alternate_domains,
                            mode,
                        };
                        let intent = crate::gui_lifecycle::ColdLaunchIntent::from_new_preflight(
                            &preflight,
                            launcher_request_uid,
                        )?;
                        let launched = crate::gui_lifecycle::launch_attach_only_gui(
                            &self.runtime_dir,
                            &ready,
                            &self.wezterm_bin,
                            &self.gui_config,
                            launcher_request_uid,
                            &intent,
                        )?;

                        self.establish_resident_gui(launched.launcher_witness())
                            .map_err(|error| {
                                self.launched_gui_partial("resident provenance", error)
                            })?;
                        let launched = launched.commit();

                        if local_owner {
                            require_preflight_domain(&launched.instance.domains, &preflight.domain)
                                .map_err(|error| {
                                    self.launched_gui_partial(
                                        "NEW local domain revalidation",
                                        error,
                                    )
                                })?;
                        } else {
                            let remote = self.remote_wez_preflight(owner).map_err(|error| {
                                self.launched_gui_partial(
                                    "NEW remote authority revalidation",
                                    error,
                                )
                            })?;
                            if remote.backend_instance != preflight.backend_instance_uid
                                || remote.server_epoch != preflight.server_epoch
                            {
                                return Err(self.launched_gui_partial(
                                    "NEW route revalidation",
                                    TypedError::new(
                                        ErrorCode::BackendEpochChanged,
                                        "selected remote Wez backend changed while preparing its attach-only GUI",
                                    ),
                                ));
                            }
                            let (domain, _) = self
                                .remote_preflight_domain(owner, &launched.instance.domains, &remote)
                                .map_err(|error| {
                                    self.launched_gui_partial(
                                        "NEW remote domain revalidation",
                                        error,
                                    )
                                })?;
                            if domain != preflight.domain {
                                return Err(self.launched_gui_partial(
                                    "NEW route rebinding",
                                    TypedError::new(
                                        ErrorCode::IdentityConflict,
                                        "launched GUI selected a different remote domain route",
                                    ),
                                ));
                            }
                        }

                        Ok(preflight)
                    }
                    many => Err(TypedError::new(
                        ErrorCode::IdentityConflict,
                        format!(
                            "{} live GUI instances exist; new presentation preflight refuses to guess",
                            many.len()
                        ),
                    )),
                }
            }
        }
    }

    fn summon(&mut self) -> Result<Value, TypedError> {
        let registry = self.registry()?;
        let ready = crate::gui_lifecycle::ensure_ready_wez_service(
            &registry,
            &self.runtime_dir,
            &self.wezterm_bin,
            &self.wezterm_config,
        )?;
        let manifest = self.remote_domain_manifest()?;
        let heartbeat_source = crate::gui_lifecycle::RuntimeHeartbeatSource;
        let live = crate::gui_lifecycle::HeartbeatSource::live_instances(
            &heartbeat_source,
            &self.runtime_dir,
        )?;
        let launcher_request_uid = Uuid::new_v4();
        let prelaunch_target =
            self.choose_summon_target(self.summon_targets(&ready, &manifest, None)?)?;
        let (instance, launched_gui) = match live.as_slice() {
            [instance] => {
                self.prove_resident_gui(instance)?;
                (instance.clone(), false)
            }
            [] => {
                let intent = self.cold_launch_intent_for_summon_target(
                    &prelaunch_target,
                    launcher_request_uid,
                )?;
                let launched = crate::gui_lifecycle::launch_attach_only_gui(
                    &self.runtime_dir,
                    &ready,
                    &self.wezterm_bin,
                    &self.gui_config,
                    launcher_request_uid,
                    &intent,
                )?;
                if let Err(error) = self.establish_resident_gui(launched.launcher_witness()) {
                    return Err(self.launched_gui_partial("resident provenance", error));
                }
                (launched.commit().instance, true)
            }
            many => {
                return Err(TypedError::new(
                    ErrorCode::IdentityConflict,
                    format!(
                        "{} live GUI instances exist; summon refuses to guess",
                        many.len()
                    ),
                ));
            }
        };
        let target_result = self
            .summon_targets(&ready, &manifest, Some(&instance.domains))
            .and_then(|targets| self.choose_summon_target(targets));
        let target = match target_result {
            Ok(target) => target,
            Err(error) if launched_gui => {
                return Err(self.launched_gui_partial("exact target rebinding", error));
            }
            Err(error) => return Err(error),
        };
        let ack = match self.cold_bridge_present(&instance, &target, None, None) {
            Ok(ack) => ack,
            Err(error) if launched_gui => {
                return Err(self.launched_gui_partial("summon presentation", error));
            }
            Err(error) => return Err(error),
        };
        Ok(serde_json::json!({
            "launched_gui": launched_gui,
            "summoned": ack,
        }))
    }

    fn present_frozen_ambient(
        &mut self,
        expected: &FrozenConnectTarget,
    ) -> Result<PresentationReceipt, TypedError> {
        self.partial_result = None;
        if expected.backend != Backend::Wez {
            return Err(TypedError::new(
                ErrorCode::BackendMismatch,
                "ambient GUI presentation accepts only a frozen Wez target",
            ));
        }
        let origin = self.ambient_origin()?;
        let bound = <Self as GuiAuthority>::bind_origin(self, &origin)?;
        let (authority, refreshed) = self.revalidate_frozen_connect_target(expected)?;
        let (group_ref, split_ref) = frozen_connect_child_refs(&refreshed);
        let ack = self.bridge_present(
            &bound,
            &authority,
            group_ref.as_deref(),
            split_ref.as_deref(),
        )?;
        PresentationReceipt::acknowledged(
            expected.clone(),
            PresentationMode::WezAmbient,
            bridge_acknowledgement(&ack)?,
        )
    }

    fn present_frozen_cold(
        &mut self,
        expected: &FrozenConnectTarget,
    ) -> Result<PresentationReceipt, TypedError> {
        self.partial_result = None;
        if expected.backend != Backend::Wez {
            return Err(TypedError::new(
                ErrorCode::BackendMismatch,
                "cold GUI presentation accepts only a frozen Wez target",
            ));
        }
        let registry = self.registry()?;
        let durable = self.resolve_cold_space_identity(
            &canonical_uri(expected.owner, expected.space_uid),
            expected.owner,
        );
        let (durable, ready) = enter_cold_wez_lifecycle(durable, |_| {
            crate::gui_lifecycle::ensure_ready_wez_service(
                &registry,
                &self.runtime_dir,
                &self.wezterm_bin,
                &self.wezterm_config,
            )
        })?;
        if durable.host_uid != expected.owner
            || durable.space_uid != expected.space_uid
            || durable.backend != expected.backend
        {
            return Err(TypedError::new(
                ErrorCode::IdentityConflict,
                "frozen cold target differs from its durable preflight identity/backend",
            ));
        }

        let (authority, refreshed) = self.revalidate_frozen_connect_target(expected)?;
        let manifest = self.remote_domain_manifest()?;
        let prelaunch_target = self
            .summon_target_for_authority(&authority, &ready, &manifest, None)?
            .ok_or_else(|| {
                unavailable(
                    "the frozen Wez target has no owner-validated compatible GUI domain route",
                )
            })?;

        let heartbeat_source = crate::gui_lifecycle::RuntimeHeartbeatSource;
        let live = crate::gui_lifecycle::HeartbeatSource::live_instances(
            &heartbeat_source,
            &self.runtime_dir,
        )?;
        let launcher_request_uid = Uuid::new_v4();
        let (instance, launched_gui) = match live.as_slice() {
            [instance] => {
                self.prove_resident_gui(instance)?;
                (instance.clone(), false)
            }
            [] => {
                let intent = self.cold_launch_intent_for_summon_target(
                    &prelaunch_target,
                    launcher_request_uid,
                )?;
                let launched = crate::gui_lifecycle::launch_attach_only_gui(
                    &self.runtime_dir,
                    &ready,
                    &self.wezterm_bin,
                    &self.gui_config,
                    launcher_request_uid,
                    &intent,
                )?;
                if let Err(error) = self.establish_resident_gui(launched.launcher_witness()) {
                    return Err(self.launched_gui_partial("resident provenance", error));
                }
                (launched.commit().instance, true)
            }
            many => {
                return Err(TypedError::new(
                    ErrorCode::IdentityConflict,
                    format!(
                        "{} live GUI instances exist; frozen cold presentation refuses to guess",
                        many.len()
                    ),
                ));
            }
        };
        let target = match self.summon_target_for_authority(
            &authority,
            &ready,
            &manifest,
            Some(&instance.domains),
        )? {
            Some(target) => target,
            None if launched_gui => {
                return Err(self.launched_gui_partial(
                    "frozen target domain rebinding",
                    unavailable(
                        "the frozen target domain is absent/incompatible in the launched GUI",
                    ),
                ));
            }
            None => {
                return Err(unavailable(
                    "the frozen target domain is absent/incompatible in the selected GUI",
                ));
            }
        };
        let (group_ref, split_ref) = frozen_connect_child_refs(&refreshed);
        let ack = match self.cold_bridge_present(
            &instance,
            &target,
            group_ref.as_deref(),
            split_ref.as_deref(),
        ) {
            Ok(ack) => ack,
            Err(error) if launched_gui => {
                return Err(self.launched_gui_partial("frozen presentation", error));
            }
            Err(error) => return Err(error),
        };
        PresentationReceipt::acknowledged(
            expected.clone(),
            PresentationMode::WezCold,
            bridge_acknowledgement(&ack)?,
        )
    }

    fn bridge_present_selected(
        &self,
        bound: &BoundGuiOrigin,
        target: &AuthorityMarker,
        group_ref: Option<&str>,
        split_ref: Option<&str>,
        selected_domain: Option<(String, Vec<String>)>,
    ) -> Result<Value, TypedError> {
        if split_ref.is_some() && group_ref.is_none() {
            return Err(TypedError::new(
                ErrorCode::InvalidRef,
                "Split presentation requires its exact Group parent",
            ));
        }
        let (domain, alternates) = match selected_domain {
            Some(selected) => selected,
            None => self.presentation_domain(&bound.heartbeat, target)?,
        };
        let mut target_json = serde_json::json!({
            "backend_instance_uid": target.backend_instance,
            "domain": domain,
            "host_uid": target.marker.host_uid,
            "server_epoch": target.marker.server_epoch,
            "space_uid": target.marker.space_uid,
            "workspace": format!("dmux:{}:{}", target.marker.host_uid.0, target.marker.space_uid.0),
        });
        if let Some(group) = group_ref {
            target_json["group_ref"] = Value::String(group.to_string());
        }
        if let Some(split) = split_ref {
            target_json["split_ref"] = Value::String(split.to_string());
        }
        let already_attached = bound
            .heartbeat
            .domains
            .get(&domain)
            .is_some_and(|state| state.state == "Attached");
        let action = if already_attached {
            "activate"
        } else {
            if !alternates.is_empty() {
                target_json["alternate_domains"] =
                    serde_json::to_value(alternates).expect("domain names serialize");
            }
            "present"
        };
        let origin = gui::in_gui_origin(
            &bound.selection,
            &bound.authority.marker,
            bound.origin.tmux_client_uid,
        );
        let mut request = gui::request_document(action, target_json, origin).map_err(typed_gui)?;
        let ack = gui::call_instance(
            &self.runtime_dir,
            &bound.selection.gui_instance,
            &mut request,
            gui::ACK_TIMEOUT,
        )
        .map_err(typed_gui)?;
        self.history
            .record_gui_present(target.marker.host_uid, target.marker.space_uid)
            .map_err(|error| {
                TypedError::new(
                    ErrorCode::OperationFailed,
                    format!("recording GUI Space history after presentation: {error}"),
                )
            })?;
        Ok(ack)
    }

    fn bridge_present(
        &self,
        bound: &BoundGuiOrigin,
        target: &AuthorityMarker,
        group_ref: Option<&str>,
        split_ref: Option<&str>,
    ) -> Result<Value, TypedError> {
        self.bridge_present_selected(bound, target, group_ref, split_ref, None)
    }

    /// Focus one already-visible outer tmux pane without attaching a domain
    /// or creating/mutating owner resources. The private client record and
    /// active @window/%pane are checked on the owner immediately before and
    /// after the signed GUI action; Lua independently requires the exact
    /// pane id, full marker, and client UID before focusing.
    fn bridge_focus_tmux_pane(
        &self,
        bound: &BoundGuiOrigin,
        target: &AuthorityMarker,
        pane: &BridgePane,
    ) -> Result<Value, TypedError> {
        if target.marker.backend != Backend::Tmux
            || pane.context != target.marker
            || pane.domain.is_empty()
        {
            return Err(TypedError::new(
                ErrorCode::BackendMismatch,
                "focus_pane requires one exact owner-validated tmux heartbeat pane",
            ));
        }
        let client_uid = pane.tmux_client_uid.ok_or_else(|| {
            unavailable("already-visible tmux pane has no attach-time exact client UID")
        })?;
        self.preflight_tmux_marker_client(target, client_uid)?;
        let origin = gui::in_gui_origin(
            &bound.selection,
            &bound.authority.marker,
            bound.origin.tmux_client_uid,
        );
        let mut request = gui::request_document(
            "focus_pane",
            serde_json::json!({
                "backend": "tmux",
                "backend_instance_uid": target.backend_instance,
                "domain": pane.domain,
                "group_ref": target.marker.group_ref,
                "host_uid": target.marker.host_uid,
                "pane_id": pane.pane_id,
                "server_epoch": target.marker.server_epoch,
                "space_no": target.marker.space_no,
                "space_uid": target.marker.space_uid,
                "split_ref": target.marker.split_ref,
                "tmux_client_uid": client_uid,
            }),
            origin,
        )
        .map_err(typed_gui)?;
        let ack = gui::call_instance(
            &self.runtime_dir,
            &bound.selection.gui_instance,
            &mut request,
            gui::ACK_TIMEOUT,
        )
        .map_err(typed_gui)?;
        self.preflight_tmux_marker_client(target, client_uid)?;
        self.history
            .record_gui_present(target.marker.host_uid, target.marker.space_uid)
            .map_err(|error| {
                TypedError::new(
                    ErrorCode::OperationFailed,
                    format!("recording exact tmux pane focus history: {error}"),
                )
            })?;
        Ok(ack)
    }

    fn bridge_detach_domain(
        &self,
        bound: &BoundGuiOrigin,
        domain: &str,
        instance: BackendInstanceUid,
        epoch: ServerEpoch,
    ) -> Result<Value, TypedError> {
        let origin = gui::in_gui_origin(
            &bound.selection,
            &bound.authority.marker,
            bound.origin.tmux_client_uid,
        );
        let mut request = gui::request_document(
            "detach_domain",
            serde_json::json!({
                "backend_instance_uid": instance,
                "domain": domain,
                "server_epoch": epoch,
            }),
            origin,
        )
        .map_err(typed_gui)?;
        gui::call_instance(
            &self.runtime_dir,
            &bound.selection.gui_instance,
            &mut request,
            gui::ACK_TIMEOUT,
        )
        .map_err(typed_gui)
    }

    /// Prove the detach against the exact GUI incarnation selected while
    /// binding the origin.  The detach ack can precede the next heartbeat,
    /// so poll only that already-bound instance for a short bounded window;
    /// never rediscover by pane membership after the origin pane vanished.
    fn prove_gui_domains_detached(
        &self,
        gui_instance: &str,
        gui_pid: u32,
        gui_process_start_token: &str,
        domains: &[String],
        all_persistent_domains: Option<&BTreeSet<String>>,
    ) -> Result<BridgeHeartbeat, TypedError> {
        let started = Instant::now();
        loop {
            let observation = match gui::read_instance_heartbeat(&self.runtime_dir, gui_instance) {
                Ok(heartbeat) => {
                    if heartbeat.pid != gui_pid
                        || heartbeat.process_start_token != gui_process_start_token
                    {
                        return Err(TypedError::new(
                            ErrorCode::IdentityConflict,
                            "the bound GUI instance changed process identity during detach",
                        ));
                    }
                    if heartbeat_proves_domains_detached(
                        &heartbeat,
                        domains,
                        all_persistent_domains,
                    ) {
                        return Ok(heartbeat);
                    }
                    format!(
                        "GUI {} has not reported every target domain Detached with zero panes",
                        gui_instance
                    )
                }
                Err(error) => error.to_string(),
            };
            if started.elapsed() >= gui::ACK_TIMEOUT {
                return Err(TypedError::new(
                    ErrorCode::PostconditionFailed,
                    format!(
                        "post-detach heartbeat proof timed out for GUI {}: {observation}",
                        gui_instance
                    ),
                ));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn parse_child(value: &str, expected: ChildKind) -> Result<ChildRefShape, TypedError> {
        let parsed = parse_ref(&format!("1/{value}")).map_err(|error| {
            TypedError::new(
                ErrorCode::InvalidRef,
                format!("malformed child ref {value:?}: {error:?}"),
            )
        })?;
        let child = parsed.child.ok_or_else(|| {
            TypedError::new(ErrorCode::InvalidRef, "child ref has no child suffix")
        })?;
        if child.kind != expected {
            return Err(TypedError::new(
                ErrorCode::InvalidRef,
                format!("expected {expected:?}, got {:?}", child.kind),
            ));
        }
        Ok(child)
    }

    fn direction(value: &str) -> Result<SplitDirection, TypedError> {
        match value {
            "left" => Ok(SplitDirection::Left),
            "right" => Ok(SplitDirection::Right),
            "up" => Ok(SplitDirection::Up),
            "down" => Ok(SplitDirection::Down),
            _ => Err(TypedError::new(
                ErrorCode::Usage,
                format!("unsupported Split direction {value:?}"),
            )),
        }
    }

    fn local_bound_provider(
        &self,
        authority: &AuthorityMarker,
    ) -> Result<(Box<dyn Provider>, InventoryScope), TypedError> {
        self.local_provider(
            authority.backend_instance,
            authority.marker.backend,
            authority.marker.server_epoch,
        )
    }

    fn remote_bound_call<T: for<'de> Deserialize<'de>>(
        &self,
        authority: &AuthorityMarker,
        method: &str,
        payload: Value,
    ) -> Result<T, TypedError> {
        let (result, envelope, _): (T, _, _) = self.remote_call(
            authority.marker.host_uid,
            method,
            payload,
            Some(authority.backend_instance),
            Some(authority.marker.server_epoch),
            false,
        )?;
        if envelope.backend_instance_uid != Some(authority.backend_instance)
            || envelope.server_epoch != Some(authority.marker.server_epoch)
        {
            return Err(TypedError::new(
                ErrorCode::BackendEpochChanged,
                format!("owner {method} response changed backend instance/epoch"),
            ));
        }
        Ok(result)
    }

    /// The §10.1 presentation read fence. Delegates to
    /// [`crate::remote::attach::open_local_read_fence`], which owns the
    /// open-before-gate rule and the reasoning behind it.
    fn local_tmux_client_read_fence(
        &self,
        instance: BackendInstanceUid,
        backend_mode: LockMode,
    ) -> Result<(Registry, OrderedLocks), TypedError> {
        crate::remote::attach::open_local_read_fence(
            &self.env,
            instance,
            backend_mode,
            "tmux GUI presentation read fence",
        )
    }

    fn tmux_client_target(
        &self,
        registry: &Registry,
        authority: &AuthorityMarker,
        group_ref: Option<&str>,
        split_ref: Option<&str>,
    ) -> Result<crate::remote::attach::TmuxClientTarget, TypedError> {
        if authority.marker.backend != Backend::Tmux {
            return Err(TypedError::new(
                ErrorCode::BackendMismatch,
                "invoking-client correlation applies only to tmux Spaces",
            ));
        }
        if !matches!(authority.location, AuthorityLocation::Local) {
            return Err(TypedError::new(
                ErrorCode::WrongBackendInstance,
                "local tmux client correlation received a remote authority",
            ));
        }
        let identity = registry.identity().map_err(typed_registry)?;
        let row = registry
            .space(authority.marker.space_uid)
            .map_err(typed_registry)?;
        let published = registry
            .backend_server(authority.backend_instance)
            .map_err(typed_registry)?;
        if identity.host_uid != authority.marker.host_uid
            || row.lifecycle != Lifecycle::Active
            || row.backend_instance != authority.backend_instance
            || published.server_epoch != Some(authority.marker.server_epoch)
        {
            return Err(TypedError::new(
                ErrorCode::BackendEpochChanged,
                "tmux client target changed owner/binding epoch under its presentation fence",
            ));
        }
        let binding = self.frozen_binding_for_authority(authority)?;
        let active_children = crate::remote::attach::tmux_client_children_from_hierarchy(
            &authority.hierarchy,
            group_ref,
            split_ref,
        )?;
        Ok(crate::remote::attach::TmuxClientTarget {
            host_uid: authority.marker.host_uid,
            space_uid: authority.marker.space_uid,
            space_no: authority.marker.space_no,
            backend_instance_uid: authority.backend_instance,
            server_epoch: authority.marker.server_epoch,
            namespace: binding.endpoint,
            native_session: binding.native_token,
            active_children,
        })
    }

    /// Preflight before any GUI-originated tmux create. The UID is only a
    /// locator: local and remote owners both validate the persisted
    /// PID/start-token/tty tuple against exactly one live client currently
    /// attached to the marker's Space.
    fn preflight_tmux_client(
        &self,
        bound: &BoundGuiOrigin,
        client_uid: Uuid,
    ) -> Result<TmuxClientStatusResult, TypedError> {
        if bound.authority.marker.backend != Backend::Tmux {
            return Err(TypedError::new(
                ErrorCode::BackendMismatch,
                "tmux client UID was supplied for a Wez Space",
            ));
        }
        self.preflight_tmux_marker_client(&bound.authority, client_uid)
    }

    /// Revalidate one exact outer tmux client against an arbitrary
    /// owner-authoritative marker. This is used both for the invoking pane
    /// and for a history-selected already-visible tmux pane; it never
    /// synthesizes an origin or chooses a client by pane count/TTY.
    fn preflight_tmux_marker_client(
        &self,
        authority: &AuthorityMarker,
        client_uid: Uuid,
    ) -> Result<TmuxClientStatusResult, TypedError> {
        if authority.marker.backend != Backend::Tmux {
            return Err(TypedError::new(
                ErrorCode::BackendMismatch,
                "exact tmux client preflight received a Wez marker",
            ));
        }
        match authority.location {
            AuthorityLocation::Local => {
                let refreshed = self.refresh_space(authority)?;
                if refreshed.marker != authority.marker
                    || refreshed.backend_instance != authority.backend_instance
                {
                    return Err(TypedError::new(
                        ErrorCode::BackendEpochChanged,
                        "tmux marker changed during invoking-client preflight",
                    ));
                }
                let (registry, _locks) = self
                    .local_tmux_client_read_fence(authority.backend_instance, LockMode::Shared)?;
                let target = self.tmux_client_target(
                    &registry,
                    &refreshed,
                    Some(&authority.marker.group_ref),
                    Some(&authority.marker.split_ref),
                )?;
                crate::remote::attach::correlate_client(&self.env.lock_dir, client_uid, &target)?;
                Ok(TmuxClientStatusResult {
                    client_uid,
                    space_uid: target.space_uid,
                    backend_instance_uid: target.backend_instance_uid,
                    server_epoch: target.server_epoch,
                    correlated: true,
                })
            }
            AuthorityLocation::Remote => {
                let status: TmuxClientStatusResult = self.remote_bound_call(
                    authority,
                    protocol::methods::TMUX_CLIENT_STATUS,
                    serde_json::to_value(TmuxClientStatusPayload {
                        client_uid,
                        space_uid: authority.marker.space_uid,
                        group_ref: authority.marker.group_ref.clone(),
                        split_ref: authority.marker.split_ref.clone(),
                    })
                    .expect("TmuxClientStatusPayload serializes"),
                )?;
                if status.client_uid != client_uid
                    || status.space_uid != authority.marker.space_uid
                    || status.backend_instance_uid != authority.backend_instance
                    || status.server_epoch != authority.marker.server_epoch
                    || !status.correlated
                {
                    return Err(TypedError::new(
                        ErrorCode::ProtocolMismatch,
                        "owner tmux client status differs from the exact GUI marker",
                    ));
                }
                Ok(status)
            }
        }
    }

    /// Out-of-band hierarchy operations do not carry reliable native tmux
    /// hook-client facts. Republish directly to the attach-time exact client
    /// and require its post-action @window/%pane to remain inside the fresh
    /// owner-authoritative child set selected by the operation.
    fn refresh_tmux_client_context(
        &self,
        bound: &BoundGuiOrigin,
        group_ref: Option<&str>,
        split_ref: Option<&str>,
    ) -> Result<MarkerContext, TypedError> {
        let client_uid = bound.origin.tmux_client_uid.ok_or_else(|| {
            unavailable(
                "invoking_client_unavailable: tmux context refresh lost its attach-time client UID",
            )
        })?;
        if bound.authority.marker.backend != Backend::Tmux {
            return Err(TypedError::new(
                ErrorCode::BackendMismatch,
                "exact tmux context refresh received a Wez origin",
            ));
        }
        match bound.authority.location {
            AuthorityLocation::Local => {
                let refreshed = self.refresh_space(&bound.authority)?;
                if refreshed.marker.host_uid != bound.authority.marker.host_uid
                    || refreshed.marker.space_uid != bound.authority.marker.space_uid
                    || refreshed.marker.backend != Backend::Tmux
                    || refreshed.backend_instance != bound.authority.backend_instance
                    || refreshed.marker.server_epoch != bound.authority.marker.server_epoch
                {
                    return Err(TypedError::new(
                        ErrorCode::BackendEpochChanged,
                        "tmux context-refresh Space changed owner/backend incarnation",
                    ));
                }
                let (registry, _locks) = self.local_tmux_client_read_fence(
                    bound.authority.backend_instance,
                    LockMode::Shared,
                )?;
                let target =
                    self.tmux_client_target(&registry, &refreshed, group_ref, split_ref)?;
                let (marker, published_clients) =
                    crate::remote::attach::publish_correlated_session_contexts(
                        &self.env.lock_dir,
                        client_uid,
                        &target,
                    )?;
                if published_clients == 0 {
                    return Err(TypedError::new(
                        ErrorCode::PostconditionFailed,
                        "tmux context refresh did not publish any correlated session client",
                    ));
                }
                let verified = self.validate_local_marker(&marker)?;
                if verified.marker != marker
                    || verified.backend_instance != bound.authority.backend_instance
                {
                    return Err(TypedError::new(
                        ErrorCode::PostconditionFailed,
                        "published tmux context did not survive owner marker revalidation",
                    ));
                }
                Ok(marker)
            }
            AuthorityLocation::Remote => {
                let result: TmuxClientRefreshResult = self.remote_bound_call(
                    &bound.authority,
                    protocol::methods::TMUX_CLIENT_REFRESH,
                    serde_json::to_value(TmuxClientRefreshPayload {
                        client_uid,
                        space_uid: bound.authority.marker.space_uid,
                        group_ref: group_ref.map(str::to_string),
                        split_ref: split_ref.map(str::to_string),
                    })
                    .expect("TmuxClientRefreshPayload serializes"),
                )?;
                if result.client_uid != client_uid
                    || result.space_uid != bound.authority.marker.space_uid
                    || result.backend_instance_uid != bound.authority.backend_instance
                    || result.server_epoch != bound.authority.marker.server_epoch
                    || !result.published
                    || result.published_clients == 0
                    || group_ref.is_some_and(|group| group != result.group_ref)
                    || split_ref.is_some_and(|split| split != result.split_ref)
                {
                    return Err(TypedError::new(
                        ErrorCode::ProtocolMismatch,
                        "owner tmux context-refresh receipt differs from its exact target",
                    ));
                }
                let marker = MarkerContext {
                    host_uid: bound.authority.marker.host_uid,
                    space_uid: result.space_uid,
                    space_no: bound.authority.marker.space_no,
                    backend: Backend::Tmux,
                    domain: None,
                    server_epoch: result.server_epoch,
                    group_ref: result.group_ref,
                    split_ref: result.split_ref,
                };
                let verified = self.validate_authority_marker(&marker)?;
                if verified.marker != marker
                    || verified.backend_instance != bound.authority.backend_instance
                {
                    return Err(TypedError::new(
                        ErrorCode::PostconditionFailed,
                        "remote published tmux context did not survive owner revalidation",
                    ));
                }
                Ok(marker)
            }
        }
    }

    fn command_tmux_client_uid(
        bound: &BoundGuiOrigin,
        supplied: Option<Uuid>,
    ) -> Result<Option<Uuid>, TypedError> {
        match bound.authority.marker.backend {
            Backend::Wez => {
                if bound.origin.tmux_client_uid.is_some() || supplied.is_some() {
                    return Err(TypedError::new(
                        ErrorCode::BackendMismatch,
                        "a Wez GUI origin must not carry a tmux client UID",
                    ));
                }
                Ok(None)
            }
            Backend::Tmux => {
                let exact = bound.origin.tmux_client_uid.ok_or_else(|| {
                    unavailable(
                        "invoking_client_unavailable: tmux GUI origin has no attach-time client UID",
                    )
                })?;
                if supplied != Some(exact) {
                    return Err(TypedError::new(
                        ErrorCode::IdentityConflict,
                        "command tmux client UID differs from the exact bound GUI origin",
                    ));
                }
                Ok(Some(exact))
            }
        }
    }

    fn finalize_pending_gui_transition_from_bound(
        &self,
        bound: &BoundGuiOrigin,
    ) -> Result<(), TypedError> {
        if bound.authority.marker.backend != Backend::Tmux {
            return Ok(());
        }
        let Some(client_uid) = bound.origin.tmux_client_uid else {
            return Ok(());
        };
        let Some(pending) = self.history.pending_gui_transition(client_uid) else {
            return Ok(());
        };
        let now = match unix_now() {
            Ok(now) => now,
            Err(error) => {
                let _ = self.history.cancel_gui_transition(client_uid);
                return Err(error);
            }
        };
        if now >= pending.expires_at {
            self.history
                .cancel_gui_transition(client_uid)
                .map_err(|error| {
                    TypedError::new(
                        ErrorCode::OperationFailed,
                        format!("canceling expired GUI transition: {error}"),
                    )
                })?;
            return Ok(());
        }
        if pending.gui_instance != bound.selection.gui_instance
            || pending.gui_pid != bound.selection.pid
            || pending.gui_process_start_token != bound.selection.process_start_token
            || pending.gui_pane_id != bound.selection.pane_id
            || pending.gui_domain != bound.selection.domain
            || pending.destination.host_uid != bound.authority.marker.host_uid
            || pending.destination.space_uid != bound.authority.marker.space_uid
            || pending.destination_backend_instance_uid != bound.authority.backend_instance
            || !pending_destination_marker_matches(&pending, &bound.authority.marker)
        {
            self.history
                .cancel_gui_transition(client_uid)
                .map_err(|error| {
                    TypedError::new(
                        ErrorCode::OperationFailed,
                        format!("canceling mismatched GUI transition: {error}"),
                    )
                })?;
            return Err(TypedError::new(
                ErrorCode::IdentityConflict,
                "current GUI tmux marker differs from its pending presentation transition",
            ));
        }
        if !self
            .history
            .complete_gui_transition(&pending)
            .map_err(|error| {
                TypedError::new(
                    ErrorCode::OperationFailed,
                    format!("finalizing pending GUI transition: {error}"),
                )
            })?
        {
            return Err(TypedError::new(
                ErrorCode::IdentityConflict,
                "pending GUI transition changed during finalization",
            ));
        }
        Ok(())
    }

    fn switch_tmux_client(
        &self,
        bound: &BoundGuiOrigin,
        client_uid: Uuid,
        destination: &AuthorityMarker,
    ) -> Result<TmuxClientSwitchResult, TypedError> {
        if bound.authority.marker.backend != Backend::Tmux
            || destination.marker.backend != Backend::Tmux
            || bound.authority.marker.host_uid != destination.marker.host_uid
            || bound.authority.backend_instance != destination.backend_instance
            || bound.authority.marker.server_epoch != destination.marker.server_epoch
        {
            return Err(TypedError::new(
                ErrorCode::WrongBackendInstance,
                "GUI tmux presentation cannot cross owner/backend instance/server epoch",
            ));
        }
        let result: TmuxClientSwitchResult = match bound.authority.location {
            AuthorityLocation::Local => {
                if !matches!(destination.location, AuthorityLocation::Local) {
                    return Err(TypedError::new(
                        ErrorCode::WrongBackendInstance,
                        "local tmux client received a remote destination",
                    ));
                }
                let from = self.refresh_space(&bound.authority)?;
                let to = self.refresh_space(destination)?;
                if from.marker != bound.authority.marker
                    || from.backend_instance != bound.authority.backend_instance
                    || to.marker.space_uid != destination.marker.space_uid
                    || to.marker.server_epoch != destination.marker.server_epoch
                    || to.backend_instance != destination.backend_instance
                {
                    return Err(TypedError::new(
                        ErrorCode::BackendEpochChanged,
                        "tmux presentation target changed under its read fence",
                    ));
                }
                let (registry, _locks) = self.local_tmux_client_read_fence(
                    bound.authority.backend_instance,
                    LockMode::Exclusive,
                )?;
                let from_target = self.tmux_client_target(
                    &registry,
                    &from,
                    Some(&bound.authority.marker.group_ref),
                    Some(&bound.authority.marker.split_ref),
                )?;
                let to_target = self.tmux_client_target(&registry, &to, None, None)?;
                crate::remote::attach::switch_correlated_client(
                    &self.env.lock_dir,
                    client_uid,
                    &from_target,
                    &to_target,
                )?;
                Ok::<TmuxClientSwitchResult, TypedError>(TmuxClientSwitchResult {
                    client_uid,
                    from_space_uid: from.marker.space_uid,
                    to_space_uid: to.marker.space_uid,
                    backend_instance_uid: to.backend_instance,
                    server_epoch: to.marker.server_epoch,
                    switched: true,
                    replayed: false,
                })
            }
            AuthorityLocation::Remote => {
                if !matches!(destination.location, AuthorityLocation::Remote) {
                    return Err(TypedError::new(
                        ErrorCode::WrongBackendInstance,
                        "remote tmux client received a local destination",
                    ));
                }
                let result: TmuxClientSwitchResult = self.remote_bound_call(
                    &bound.authority,
                    protocol::methods::TMUX_CLIENT_SWITCH,
                    serde_json::to_value(TmuxClientSwitchPayload {
                        client_uid,
                        from_space_uid: bound.authority.marker.space_uid,
                        from_group_ref: bound.authority.marker.group_ref.clone(),
                        from_split_ref: bound.authority.marker.split_ref.clone(),
                        to_space_uid: destination.marker.space_uid,
                    })
                    .expect("TmuxClientSwitchPayload serializes"),
                )?;
                if result.client_uid != client_uid
                    || result.from_space_uid != bound.authority.marker.space_uid
                    || result.to_space_uid != destination.marker.space_uid
                    || result.backend_instance_uid != destination.backend_instance
                    || result.server_epoch != destination.marker.server_epoch
                    || !result.switched
                {
                    return Err(TypedError::new(
                        ErrorCode::ProtocolMismatch,
                        "owner tmux client switch receipt differs from the exact target",
                    ));
                }
                Ok(result)
            }
        }?;
        self.history
            .record_gui_present(destination.marker.host_uid, destination.marker.space_uid)
            .map_err(|error| {
                TypedError::new(
                    ErrorCode::OperationFailed,
                    format!("recording exact tmux GUI presentation history: {error}"),
                )
            })?;
        Ok(result)
    }

    fn detach_tmux_client(
        &self,
        bound: &BoundGuiOrigin,
        client_uid: Uuid,
    ) -> Result<TmuxClientDetachResult, TypedError> {
        if bound.authority.marker.backend != Backend::Tmux {
            return Err(TypedError::new(
                ErrorCode::BackendMismatch,
                "exact tmux client detach received a Wez origin",
            ));
        }
        match bound.authority.location {
            AuthorityLocation::Local => {
                let refreshed = self.refresh_space(&bound.authority)?;
                if refreshed.marker != bound.authority.marker
                    || refreshed.backend_instance != bound.authority.backend_instance
                {
                    return Err(TypedError::new(
                        ErrorCode::BackendEpochChanged,
                        "tmux detach target changed before its mutation fence",
                    ));
                }
                let (registry, _locks) = self.local_tmux_client_read_fence(
                    bound.authority.backend_instance,
                    LockMode::Exclusive,
                )?;
                let target = self.tmux_client_target(
                    &registry,
                    &refreshed,
                    Some(&bound.authority.marker.group_ref),
                    Some(&bound.authority.marker.split_ref),
                )?;
                crate::remote::attach::detach_correlated_client(
                    &self.env.lock_dir,
                    client_uid,
                    &target,
                )?;
                Ok(TmuxClientDetachResult {
                    client_uid,
                    space_uid: target.space_uid,
                    backend_instance_uid: target.backend_instance_uid,
                    server_epoch: target.server_epoch,
                    detached: true,
                    replayed: false,
                })
            }
            AuthorityLocation::Remote => {
                let result: TmuxClientDetachResult = self.remote_bound_call(
                    &bound.authority,
                    protocol::methods::TMUX_CLIENT_DETACH,
                    serde_json::to_value(TmuxClientDetachPayload {
                        client_uid,
                        space_uid: bound.authority.marker.space_uid,
                        group_ref: bound.authority.marker.group_ref.clone(),
                        split_ref: bound.authority.marker.split_ref.clone(),
                    })
                    .expect("TmuxClientDetachPayload serializes"),
                )?;
                if result.client_uid != client_uid
                    || result.space_uid != bound.authority.marker.space_uid
                    || result.backend_instance_uid != bound.authority.backend_instance
                    || result.server_epoch != bound.authority.marker.server_epoch
                    || !result.detached
                {
                    return Err(TypedError::new(
                        ErrorCode::ProtocolMismatch,
                        "owner tmux client detach receipt differs from the exact origin",
                    ));
                }
                Ok(result)
            }
        }
    }

    fn refresh_space(&self, authority: &AuthorityMarker) -> Result<AuthorityMarker, TypedError> {
        self.resolve_space(
            &canonical_uri(authority.marker.host_uid, authority.marker.space_uid),
            authority.marker.host_uid,
        )
    }

    fn create_space_for_origin(
        &mut self,
        bound: &BoundGuiOrigin,
        name: &str,
        dir: Option<&str>,
        tmux_client_uid: Option<Uuid>,
    ) -> Result<Value, TypedError> {
        validate_new_name(name).map_err(|error| {
            TypedError::new(
                ErrorCode::InvalidName,
                format!("invalid new Space name {name:?}: {error:?}"),
            )
        })?;
        // `_gui space-new` always presents. Freeze every presentation
        // prerequisite before reserving identity: Wez needs an exact GUI
        // domain; tmux needs the attach-time invoking-client witness.
        let wez_presentation = match bound.authority.marker.backend {
            Backend::Wez => {
                if tmux_client_uid.is_some() {
                    return Err(TypedError::new(
                        ErrorCode::Usage,
                        "a Wez Space action must not carry --tmux-client-uid",
                    ));
                }
                Some(self.presentation_domain(&bound.heartbeat, &bound.authority)?)
            }
            Backend::Tmux => {
                let client_uid = tmux_client_uid.ok_or_else(|| {
                    unavailable(
                        "invoking_client_unavailable: tmux GUI creation requires its attach-time client UID",
                    )
                })?;
                self.preflight_tmux_client(bound, client_uid)?;
                None
            }
        };
        let selected_backend = bound.authority.marker.backend;
        let created: CreatedSpace = match bound.authority.location {
            AuthorityLocation::Local => {
                let cwd = match dir {
                    None => None,
                    Some(dir) => {
                        let canonical = std::fs::canonicalize(dir).map_err(|error| {
                            TypedError::new(ErrorCode::NotFound, format!("--dir {dir:?}: {error}"))
                        })?;
                        if !canonical.is_dir() {
                            return Err(TypedError::new(
                                ErrorCode::NotFound,
                                format!("--dir {dir:?} is not a directory"),
                            ));
                        }
                        Some(canonical.display().to_string())
                    }
                };
                let (provider, scope) = self.local_bound_provider(&bound.authority)?;
                let opposite = self.local_opposite_create_target(selected_backend)?;
                operations::create_space_owner_fenced(
                    &self.env,
                    OwnerCreateTarget {
                        backend: selected_backend,
                        instance: bound.authority.backend_instance,
                        provider: provider.as_ref(),
                        scope: &scope,
                    },
                    opposite.as_ref().map(OwnedCreateTarget::borrowed),
                    false,
                    &CreateRequest {
                        request_uid: Uuid::new_v4(),
                        name: name.to_string(),
                        cwd,
                        program: Vec::new(),
                        helper_bin: self.helper_bin.clone(),
                    },
                )
                .map_err(typed_operation)?
            }
            AuthorityLocation::Remote => {
                if dir.is_some() {
                    return Err(TypedError::new(
                        ErrorCode::Usage,
                        "remote GUI --dir is refused because controller-local zoxide paths are not owner paths",
                    ));
                }
                self.remote_bound_call(
                    &bound.authority,
                    protocol::methods::NEW,
                    serde_json::to_value(NewPayload {
                        name: name.to_string(),
                        backend: selected_backend,
                        cwd: None,
                        program: Vec::new(),
                        allow_name_collision: false,
                    })
                    .expect("NewPayload serializes"),
                )?
            }
        };
        let stable_ref = canonical_uri(bound.authority.marker.host_uid, created.space_uid);
        let presentation_result = self
            .resolve_space(&stable_ref, bound.authority.marker.host_uid)
            .and_then(|target| {
                if selected_backend == Backend::Wez {
                    self.bridge_present_selected(
                        bound,
                        &target,
                        Some(&created.group_ref),
                        Some(&created.split_ref),
                        wez_presentation.clone(),
                    )
                    .map(|_| ())
                } else {
                    self.switch_tmux_client(
                        bound,
                        tmux_client_uid.expect("tmux preflight required client UID"),
                        &target,
                    )
                    .map(|_| ())
                }
            });
        if let Err(error) = presentation_result {
            self.partial_result = Some(serde_json::json!({
                "created": true,
                "connected": false,
                "stable_ref": stable_ref,
                "space": created,
            }));
            return Err(TypedError::new(
                ErrorCode::PartialResult,
                format!(
                    "Space creation completed, but GUI presentation failed: {}",
                    error.message
                ),
            ));
        }
        serde_json::to_value(created)
            .map_err(|error| TypedError::new(ErrorCode::OperationFailed, error.to_string()))
    }

    fn group_new(&self, bound: &BoundGuiOrigin) -> Result<Value, TypedError> {
        let presentation = (bound.authority.marker.backend == Backend::Wez)
            .then(|| self.presentation_domain(&bound.heartbeat, &bound.authority))
            .transpose()?;
        let created: CreatedChild = match bound.authority.location {
            AuthorityLocation::Local => {
                let (provider, scope) = self.local_bound_provider(&bound.authority)?;
                operations::group_new(
                    &self.env,
                    provider.as_ref(),
                    &scope,
                    &GroupNewRequest {
                        request_uid: Uuid::new_v4(),
                        space_uid: bound.authority.marker.space_uid,
                        cwd: None,
                        program: Vec::new(),
                        helper_bin: self.helper_bin.clone(),
                    },
                )
                .map_err(typed_operation)?
            }
            AuthorityLocation::Remote => self.remote_bound_call(
                &bound.authority,
                protocol::methods::GROUP_NEW,
                serde_json::to_value(GroupNewPayload {
                    space_uid: bound.authority.marker.space_uid,
                    cwd: None,
                    program: Vec::new(),
                })
                .expect("GroupNewPayload serializes"),
            )?,
        };
        if bound.authority.marker.backend == Backend::Wez {
            let refreshed = self.refresh_space(&bound.authority)?;
            self.bridge_present_selected(
                bound,
                &refreshed,
                Some(&created.group_ref),
                Some(&created.split_ref),
                presentation,
            )?;
        } else {
            let group = Self::parse_child(&created.group_ref, ChildKind::Group)?;
            match bound.authority.location {
                AuthorityLocation::Local => {
                    let (provider, scope) = self.local_bound_provider(&bound.authority)?;
                    operations::group_activate_exact(
                        &self.env,
                        provider.as_ref(),
                        &scope,
                        bound.authority.marker.space_uid,
                        &group,
                        Uuid::new_v4(),
                    )
                    .map_err(typed_operation)?;
                }
                AuthorityLocation::Remote => {
                    let _: GroupActivateResult = self.remote_bound_call(
                        &bound.authority,
                        protocol::methods::GROUP_ACTIVATE,
                        serde_json::to_value(GroupActivatePayload {
                            space_uid: bound.authority.marker.space_uid,
                            group_ref: created.group_ref.clone(),
                        })
                        .expect("GroupActivatePayload serializes"),
                    )?;
                }
            }
            self.refresh_tmux_client_context(
                bound,
                Some(&created.group_ref),
                Some(&created.split_ref),
            )?;
        }
        serde_json::to_value(created)
            .map_err(|error| TypedError::new(ErrorCode::OperationFailed, error.to_string()))
    }

    fn group_select(
        &self,
        bound: &BoundGuiOrigin,
        relative: Option<&str>,
        index: Option<u32>,
    ) -> Result<Value, TypedError> {
        if relative.is_some() == index.is_some() {
            return Err(TypedError::new(
                ErrorCode::Usage,
                "group-select requires exactly one of --relative/--index",
            ));
        }
        let groups = &bound.authority.hierarchy.groups;
        let current = groups
            .iter()
            .position(|group| group.group_ref == bound.authority.marker.group_ref)
            .ok_or_else(|| {
                TypedError::new(
                    ErrorCode::BackendEpochChanged,
                    "active Group is absent from its revalidated hierarchy",
                )
            })?;
        let selected = if let Some(index) = index {
            groups.get(index as usize - 1).ok_or_else(|| {
                TypedError::new(
                    ErrorCode::NotFound,
                    format!("Space has no Group at index {index}"),
                )
            })?
        } else {
            let next = match relative.expect("exactly one selector") {
                "next" => (current + 1) % groups.len(),
                "prev" => (current + groups.len() - 1) % groups.len(),
                "last" => groups.len() - 1,
                other => {
                    return Err(TypedError::new(
                        ErrorCode::Usage,
                        format!("invalid Group relative selector {other:?}"),
                    ));
                }
            };
            &groups[next]
        };
        if bound.authority.marker.backend == Backend::Wez {
            self.bridge_present(bound, &bound.authority, Some(&selected.group_ref), None)?;
        } else {
            let group = Self::parse_child(&selected.group_ref, ChildKind::Group)?;
            match bound.authority.location {
                AuthorityLocation::Local => {
                    let (provider, scope) = self.local_bound_provider(&bound.authority)?;
                    operations::group_activate_exact(
                        &self.env,
                        provider.as_ref(),
                        &scope,
                        bound.authority.marker.space_uid,
                        &group,
                        Uuid::new_v4(),
                    )
                    .map_err(typed_operation)?;
                }
                AuthorityLocation::Remote => {
                    let _: GroupActivateResult = self.remote_bound_call(
                        &bound.authority,
                        protocol::methods::GROUP_ACTIVATE,
                        serde_json::to_value(GroupActivatePayload {
                            space_uid: bound.authority.marker.space_uid,
                            group_ref: selected.group_ref.clone(),
                        })
                        .expect("GroupActivatePayload serializes"),
                    )?;
                }
            }
            self.refresh_tmux_client_context(bound, Some(&selected.group_ref), None)?;
        }
        Ok(serde_json::json!({ "group_ref": selected.group_ref }))
    }

    fn group_rename(&self, bound: &BoundGuiOrigin, name: &str) -> Result<Value, TypedError> {
        if name.is_empty() || name.len() > 1024 || name.bytes().any(|byte| byte.is_ascii_control())
        {
            return Err(TypedError::new(
                ErrorCode::InvalidName,
                "Group title must be 1-1024 control-free bytes",
            ));
        }
        let group = Self::parse_child(&bound.authority.marker.group_ref, ChildKind::Group)?;
        match bound.authority.location {
            AuthorityLocation::Local => {
                let (provider, scope) = self.local_bound_provider(&bound.authority)?;
                operations::group_rename(
                    &self.env,
                    provider.as_ref(),
                    &scope,
                    bound.authority.marker.space_uid,
                    &group,
                    name,
                    Uuid::new_v4(),
                )
                .map_err(typed_operation)?;
            }
            AuthorityLocation::Remote => {
                let _: Value = self.remote_bound_call(
                    &bound.authority,
                    protocol::methods::GROUP_RENAME,
                    serde_json::to_value(GroupRenamePayload {
                        space_uid: bound.authority.marker.space_uid,
                        group_ref: bound.authority.marker.group_ref.clone(),
                        title: name.to_string(),
                    })
                    .expect("GroupRenamePayload serializes"),
                )?;
            }
        }
        Ok(serde_json::json!({
            "group_ref": bound.authority.marker.group_ref,
            "title": name,
        }))
    }

    fn group_remove(
        &self,
        bound: &BoundGuiOrigin,
        confirmed: bool,
        escalate_space: bool,
    ) -> Result<Value, TypedError> {
        if !confirmed {
            return Err(TypedError::new(
                ErrorCode::ConfirmationRequired,
                "Group removal requires the GUI's exact confirmation",
            ));
        }
        let group = Self::parse_child(&bound.authority.marker.group_ref, ChildKind::Group)?;
        if escalate_space {
            if bound.authority.hierarchy.groups.len() != 1 {
                return Err(TypedError::new(
                    ErrorCode::Usage,
                    "--escalate-space is legal only for the final Group",
                ));
            }
            match bound.authority.location {
                AuthorityLocation::Local => {
                    let (provider, scope) = self.local_bound_provider(&bound.authority)?;
                    operations::remove_space(
                        &self.env,
                        provider.as_ref(),
                        &scope,
                        bound.authority.marker.backend,
                        bound.authority.marker.space_uid,
                        Uuid::new_v4(),
                    )
                    .map_err(typed_operation)?;
                }
                AuthorityLocation::Remote => {
                    let _: Value = self.remote_bound_call(
                        &bound.authority,
                        protocol::methods::RM,
                        serde_json::to_value(RmPayload {
                            space_uid: bound.authority.marker.space_uid,
                        })
                        .expect("RmPayload serializes"),
                    )?;
                }
            }
            return Ok(serde_json::json!({
                "space_uid": bound.authority.marker.space_uid,
                "removed": true,
            }));
        }
        match bound.authority.location {
            AuthorityLocation::Local => {
                let (provider, scope) = self.local_bound_provider(&bound.authority)?;
                operations::group_remove(
                    &self.env,
                    provider.as_ref(),
                    &scope,
                    bound.authority.marker.space_uid,
                    &group,
                    Uuid::new_v4(),
                )
                .map_err(typed_operation)?;
            }
            AuthorityLocation::Remote => {
                let _: Value = self.remote_bound_call(
                    &bound.authority,
                    protocol::methods::GROUP_RM,
                    serde_json::to_value(GroupRmPayload {
                        space_uid: bound.authority.marker.space_uid,
                        group_ref: bound.authority.marker.group_ref.clone(),
                    })
                    .expect("GroupRmPayload serializes"),
                )?;
            }
        }
        if bound.authority.marker.backend == Backend::Tmux {
            self.refresh_tmux_client_context(bound, None, None)?;
        }
        Ok(serde_json::json!({
            "group_ref": bound.authority.marker.group_ref,
            "removed": true,
        }))
    }

    fn split_new(&self, bound: &BoundGuiOrigin, direction: &str) -> Result<Value, TypedError> {
        let presentation = (bound.authority.marker.backend == Backend::Wez)
            .then(|| self.presentation_domain(&bound.heartbeat, &bound.authority))
            .transpose()?;
        let parsed_direction = Self::direction(direction)?;
        let group = Self::parse_child(&bound.authority.marker.group_ref, ChildKind::Group)?;
        let created: CreatedChild = match bound.authority.location {
            AuthorityLocation::Local => {
                let (provider, scope) = self.local_bound_provider(&bound.authority)?;
                operations::split_new(
                    &self.env,
                    provider.as_ref(),
                    &scope,
                    &SplitNewRequest {
                        request_uid: Uuid::new_v4(),
                        space_uid: bound.authority.marker.space_uid,
                        group,
                        direction: parsed_direction,
                        percent: None,
                        cwd: None,
                        program: Vec::new(),
                        helper_bin: self.helper_bin.clone(),
                    },
                )
                .map_err(typed_operation)?
            }
            AuthorityLocation::Remote => self.remote_bound_call(
                &bound.authority,
                protocol::methods::SPLIT_NEW,
                serde_json::to_value(SplitNewPayload {
                    space_uid: bound.authority.marker.space_uid,
                    group_ref: bound.authority.marker.group_ref.clone(),
                    direction: Some(direction.to_string()),
                    percent: None,
                    cwd: None,
                    program: Vec::new(),
                })
                .expect("SplitNewPayload serializes"),
            )?,
        };
        if bound.authority.marker.backend == Backend::Wez {
            let refreshed = self.refresh_space(&bound.authority)?;
            self.bridge_present_selected(
                bound,
                &refreshed,
                Some(&created.group_ref),
                Some(&created.split_ref),
                presentation,
            )?;
        } else {
            self.refresh_tmux_client_context(
                bound,
                Some(&created.group_ref),
                Some(&created.split_ref),
            )?;
        }
        serde_json::to_value(created)
            .map_err(|error| TypedError::new(ErrorCode::OperationFailed, error.to_string()))
    }

    fn split_select(&self, bound: &BoundGuiOrigin, direction: &str) -> Result<Value, TypedError> {
        if bound.authority.marker.backend == Backend::Wez {
            self.presentation_domain(&bound.heartbeat, &bound.authority)?;
        }
        let parsed_direction = Self::direction(direction)?;
        let split = Self::parse_child(&bound.authority.marker.split_ref, ChildKind::Split)?;
        let (group_ref, split_ref, result) = match bound.authority.location {
            AuthorityLocation::Local => {
                let (provider, scope) = self.local_bound_provider(&bound.authority)?;
                let selected = operations::split_direction(
                    &self.env,
                    provider.as_ref(),
                    &scope,
                    bound.authority.marker.space_uid,
                    &split,
                    parsed_direction,
                    Uuid::new_v4(),
                )
                .map_err(typed_operation)?;
                let group = selected.group_ref.clone();
                let split = selected.split_ref.clone();
                let result = serde_json::to_value(selected).map_err(|error| {
                    TypedError::new(ErrorCode::OperationFailed, error.to_string())
                })?;
                (group, split, result)
            }
            AuthorityLocation::Remote => {
                let selected: SplitDirectionResult = self.remote_bound_call(
                    &bound.authority,
                    protocol::methods::SPLIT_DIRECTION,
                    serde_json::to_value(SplitDirectionPayload {
                        space_uid: bound.authority.marker.space_uid,
                        split_ref: bound.authority.marker.split_ref.clone(),
                        direction: direction.to_string(),
                    })
                    .expect("SplitDirectionPayload serializes"),
                )?;
                let group = selected.group_ref.clone();
                let split = selected.split_ref.clone();
                let result = serde_json::to_value(selected).map_err(|error| {
                    TypedError::new(ErrorCode::OperationFailed, error.to_string())
                })?;
                (group, split, result)
            }
        };
        if let Some(split_ref) = split_ref.as_deref() {
            if bound.authority.marker.backend == Backend::Wez {
                self.bridge_present(bound, &bound.authority, Some(&group_ref), Some(split_ref))?;
            } else {
                self.refresh_tmux_client_context(bound, Some(&group_ref), Some(split_ref))?;
            }
        } else if bound.authority.marker.backend == Backend::Tmux {
            // The exact edge no-op must still prove that the client remains
            // on the originating child before the next GUI key is accepted.
            self.refresh_tmux_client_context(
                bound,
                Some(&bound.authority.marker.group_ref),
                Some(&bound.authority.marker.split_ref),
            )?;
        }
        Ok(result)
    }

    fn split_resize(
        &self,
        bound: &BoundGuiOrigin,
        direction: &str,
        amount: u16,
    ) -> Result<Value, TypedError> {
        let parsed_direction = Self::direction(direction)?;
        let split = Self::parse_child(&bound.authority.marker.split_ref, ChildKind::Split)?;
        match bound.authority.location {
            AuthorityLocation::Local => {
                let (provider, scope) = self.local_bound_provider(&bound.authority)?;
                let resized = operations::split_resize(
                    &self.env,
                    provider.as_ref(),
                    &scope,
                    bound.authority.marker.space_uid,
                    &split,
                    parsed_direction,
                    amount,
                    Uuid::new_v4(),
                )
                .map_err(typed_operation)?;
                serde_json::to_value(resized)
                    .map_err(|error| TypedError::new(ErrorCode::OperationFailed, error.to_string()))
            }
            AuthorityLocation::Remote => {
                let result: SplitResizeResult = self.remote_bound_call(
                    &bound.authority,
                    protocol::methods::SPLIT_RESIZE,
                    serde_json::to_value(SplitResizePayload {
                        space_uid: bound.authority.marker.space_uid,
                        split_ref: bound.authority.marker.split_ref.clone(),
                        direction: direction.to_string(),
                        amount,
                    })
                    .expect("SplitResizePayload serializes"),
                )?;
                serde_json::to_value(result)
                    .map_err(|error| TypedError::new(ErrorCode::OperationFailed, error.to_string()))
            }
        }
    }

    fn split_zoom(&self, bound: &BoundGuiOrigin) -> Result<Value, TypedError> {
        let split = Self::parse_child(&bound.authority.marker.split_ref, ChildKind::Split)?;
        match bound.authority.location {
            AuthorityLocation::Local => {
                let (provider, scope) = self.local_bound_provider(&bound.authority)?;
                let zoomed = operations::split_zoom(
                    &self.env,
                    provider.as_ref(),
                    &scope,
                    bound.authority.marker.space_uid,
                    &split,
                    Uuid::new_v4(),
                )
                .map_err(typed_operation)?;
                serde_json::to_value(zoomed)
                    .map_err(|error| TypedError::new(ErrorCode::OperationFailed, error.to_string()))
            }
            AuthorityLocation::Remote => {
                let result: SplitZoomResult = self.remote_bound_call(
                    &bound.authority,
                    protocol::methods::SPLIT_ZOOM,
                    serde_json::to_value(SplitZoomPayload {
                        space_uid: bound.authority.marker.space_uid,
                        split_ref: bound.authority.marker.split_ref.clone(),
                    })
                    .expect("SplitZoomPayload serializes"),
                )?;
                serde_json::to_value(result)
                    .map_err(|error| TypedError::new(ErrorCode::OperationFailed, error.to_string()))
            }
        }
    }

    fn split_remove(&self, bound: &BoundGuiOrigin, confirmed: bool) -> Result<Value, TypedError> {
        if !confirmed {
            return Err(TypedError::new(
                ErrorCode::ConfirmationRequired,
                "Split removal requires the GUI's exact confirmation",
            ));
        }
        let split = Self::parse_child(&bound.authority.marker.split_ref, ChildKind::Split)?;
        match bound.authority.location {
            AuthorityLocation::Local => {
                let (provider, scope) = self.local_bound_provider(&bound.authority)?;
                operations::split_remove(
                    &self.env,
                    provider.as_ref(),
                    &scope,
                    bound.authority.marker.space_uid,
                    &split,
                    Uuid::new_v4(),
                )
                .map_err(typed_operation)?;
            }
            AuthorityLocation::Remote => {
                let _: Value = self.remote_bound_call(
                    &bound.authority,
                    protocol::methods::SPLIT_RM,
                    serde_json::to_value(SplitRmPayload {
                        space_uid: bound.authority.marker.space_uid,
                        split_ref: bound.authority.marker.split_ref.clone(),
                    })
                    .expect("SplitRmPayload serializes"),
                )?;
            }
        }
        if bound.authority.marker.backend == Backend::Tmux {
            self.refresh_tmux_client_context(bound, None, None)?;
        }
        Ok(serde_json::json!({
            "split_ref": bound.authority.marker.split_ref,
            "removed": true,
        }))
    }

    fn wez_native_tree_for_authority(
        &self,
        host_uid: HostUid,
        instance: BackendInstanceUid,
        epoch: ServerEpoch,
    ) -> Result<WezNativeTreeResult, TypedError> {
        let registry = self.registry()?;
        let identity = registry.identity().map_err(typed_registry)?;
        if host_uid == identity.host_uid {
            let info = registry
                .backend_instance_info(instance)
                .map_err(typed_registry)?;
            let server = registry.backend_server(instance).map_err(typed_registry)?;
            if info.owner != host_uid
                || info.backend != Backend::Wez
                || server.server_epoch != Some(epoch)
            {
                return Err(TypedError::new(
                    ErrorCode::WrongBackendInstance,
                    "local Wez native-tree target differs from its registry authority",
                ));
            }
            let (_, scope) = self.local_provider(instance, Backend::Wez, epoch)?;
            let server_pid = server
                .server_pid
                .and_then(|pid| u32::try_from(pid).ok())
                .ok_or_else(|| {
                    unavailable("local Wez native-tree target has no published server PID")
                })?;
            let start_token = server.server_start_token.ok_or_else(|| {
                unavailable("local Wez native-tree target has no published server start token")
            })?;
            let witness = WezProvider::new(&self.wezterm_bin, &self.wezterm_config)
                .with_identity(IdentityExpectation {
                    server_pid: Some(server_pid),
                    start_token: Some(start_token),
                })
                .native_tree_witness(&scope)
                .map_err(typed_wez_native_tree)?;
            if witness.server_epoch != epoch {
                return Err(TypedError::new(
                    ErrorCode::BackendEpochChanged,
                    "local Wez native-tree witness changed epoch",
                ));
            }
            return Ok(WezNativeTreeResult {
                backend_instance_uid: instance,
                server_epoch: epoch,
                sentinel_window_id: witness.sentinel_window_id,
                sentinel_tab_id: witness.sentinel_tab_id,
                sentinel_pane_id: witness.sentinel_pane_id,
                panes: witness
                    .panes
                    .into_iter()
                    .map(|pane| WezNativePaneWitness {
                        window_id: pane.window_id,
                        tab_id: pane.tab_id,
                        pane_id: pane.pane_id,
                    })
                    .collect(),
            });
        }

        let (result, envelope, _): (WezNativeTreeResult, _, _) = self.remote_call(
            host_uid,
            protocol::methods::WEZ_NATIVE_TREE_STATUS,
            serde_json::to_value(WezNativeTreePayload {}).expect("WezNativeTreePayload serializes"),
            Some(instance),
            Some(epoch),
            false,
        )?;
        if envelope.backend_instance_uid != Some(instance)
            || envelope.server_epoch != Some(epoch)
            || result.backend_instance_uid != instance
            || result.server_epoch != epoch
        {
            return Err(TypedError::new(
                ErrorCode::ProtocolMismatch,
                "owner Wez native-tree response differs from its exact instance/epoch claim",
            ));
        }
        Ok(result)
    }

    /// A GUI heartbeat may exempt only the one reserved sentinel pane for
    /// an attached persistent domain.  Lua reports a syntactic system-pane
    /// count; this controller independently binds that count to the exact
    /// owner backend instance and sentinel-proven epoch.
    fn prove_attached_domain_sentinel(
        &self,
        domain: &str,
        state: &BridgeDomainState,
        manifest: &[GuiDomainManifestRow],
    ) -> Result<DomainAuthority, TypedError> {
        if state.state != "Attached"
            || !state.has_any_panes
            || state.system_pane_count != 1
            || state.pane_count != state.valid_marker_pane_count.saturating_add(1)
        {
            return Err(TypedError::new(
                ErrorCode::PostconditionFailed,
                format!(
                    "GUI domain {domain:?} is not Attached with exactly one classified sentinel pane"
                ),
            ));
        }
        let gui_epoch = state.system_epoch.ok_or_else(|| {
            TypedError::new(
                ErrorCode::BackendEpochChanged,
                format!("GUI domain {domain:?} omitted its exact system epoch"),
            )
        })?;
        if state.system_workspace.as_deref()
            != Some(format!("dmux:system:{}", gui_epoch.0).as_str())
        {
            return Err(TypedError::new(
                ErrorCode::BackendEpochChanged,
                format!("GUI domain {domain:?} system workspace/epoch disagree"),
            ));
        }

        if domain == LOCAL_WEZ_DOMAIN {
            let registry = self.registry()?;
            let identity = registry.identity().map_err(typed_registry)?;
            let instance = registry
                .backend_instance_for_backend(Backend::Wez)
                .map_err(typed_registry)?
                .ok_or_else(|| unavailable("registry has no managed Wez backend instance"))?;
            let info = registry
                .backend_instance_info(instance)
                .map_err(typed_registry)?;
            let server = registry.backend_server(instance).map_err(typed_registry)?;
            let epoch = server
                .server_epoch
                .ok_or_else(|| unavailable("registered Wez backend has no live server epoch"))?;
            let descriptor = crate::runtime::read_verified_ready_wez_descriptor_in(
                &self.runtime_dir,
                instance.0,
                epoch.0,
            )
            .map_err(|error| unavailable(format!("managed Wez descriptor: {error}")))?
            .ok_or_else(|| unavailable("managed Wez descriptor is absent"))?;
            if info.backend != Backend::Wez
                || info.owner != identity.host_uid
                || info.socket_path.as_deref() != Some(descriptor.socket.as_str())
                || server.server_pid != Some(i64::from(descriptor.pid))
                || server.server_start_token.as_deref() != Some(descriptor.start_token.as_str())
                || server.socket_dev
                    != descriptor
                        .socket_dev
                        .and_then(|value| i64::try_from(value).ok())
                || server.socket_ino
                    != descriptor
                        .socket_ino
                        .and_then(|value| i64::try_from(value).ok())
                || gui_epoch != epoch
                || state.backend_instance_uid != Some(instance)
            {
                return Err(TypedError::new(
                    ErrorCode::WrongBackendInstance,
                    "local GUI domain descriptor differs from its registered Wez authority",
                ));
            }
            let native_tree =
                self.wez_native_tree_for_authority(identity.host_uid, instance, epoch)?;
            return Ok(DomainAuthority {
                host_uid: identity.host_uid,
                backend_instance: instance,
                server_epoch: epoch,
                native_tree,
            });
        }

        let candidates: Vec<_> = manifest
            .iter()
            .filter(|row| row.name == domain && row.compatible && row.remote_wezterm_path.is_some())
            .collect();
        let [row] = candidates.as_slice() else {
            return Err(TypedError::new(
                ErrorCode::HostIdentityChanged,
                format!(
                    "GUI domain {domain:?} is not one compatible owner-validated persistent route"
                ),
            ));
        };
        let (hello, _) = self.remote_hello(row.host_uid)?;
        let backends: Vec<_> = hello
            .backends
            .iter()
            .filter(|backend| {
                backend.backend == Backend::Wez
                    && backend.backend_instance_uid == row.backend_instance_uid
                    && backend.server_epoch.is_some()
            })
            .collect();
        let [backend] = backends.as_slice() else {
            return Err(TypedError::new(
                ErrorCode::WrongBackendInstance,
                format!("owner hello did not prove GUI domain {domain:?}'s exact Wez instance"),
            ));
        };
        let epoch = backend.server_epoch.expect("filtered Some server_epoch");
        if state.backend_instance_uid != Some(row.backend_instance_uid) {
            return Err(TypedError::new(
                ErrorCode::WrongBackendInstance,
                format!("GUI domain {domain:?} config instance differs from owner authority"),
            ));
        }
        if gui_epoch != epoch {
            return Err(TypedError::new(
                ErrorCode::BackendEpochChanged,
                format!(
                    "GUI domain {domain:?} displays epoch {} but owner proves {}",
                    gui_epoch.0, epoch.0
                ),
            ));
        }
        let spaces = self.remote_spaces(row.host_uid)?;
        if !spaces.scans.iter().any(|scan| {
            scan.backend == Backend::Wez
                && scan.outcome == "complete"
                && scan.server_epoch == Some(epoch)
        }) {
            return Err(unavailable(format!(
                "owner scan did not prove GUI domain {domain:?}'s sentinel epoch {}",
                epoch.0
            )));
        }
        let native_tree =
            self.wez_native_tree_for_authority(row.host_uid, row.backend_instance_uid, epoch)?;
        Ok(DomainAuthority {
            host_uid: row.host_uid,
            backend_instance: row.backend_instance_uid,
            server_epoch: epoch,
            native_tree,
        })
    }

    fn snapshot_markers(
        &self,
        heartbeat: &BridgeHeartbeat,
        only_domain: Option<&str>,
    ) -> Result<Vec<SnapshotMarker>, TypedError> {
        let mut unique = BTreeSet::new();
        let mut validated = Vec::new();
        for pane in &heartbeat.panes {
            if only_domain.is_some_and(|domain| pane.domain != domain) {
                continue;
            }
            let marker = &pane.context;
            let key = format!(
                "{}\x1f{}\x1f{}\x1f{}\x1f{}\x1f{}",
                marker.host_uid.0,
                marker.space_uid.0,
                marker.server_epoch.0,
                marker.group_ref,
                marker.split_ref,
                pane.domain,
            );
            if !unique.insert(key) {
                return Err(TypedError::new(
                    ErrorCode::IdentityConflict,
                    "two GUI panes carry the same exact owner Split marker",
                ));
            }
            validated.push(SnapshotMarker {
                authority: self.validate_authority_marker_in_domain(marker, Some(&pane.domain))?,
                gui_domain: pane.domain.clone(),
            });
        }
        Ok(validated)
    }

    fn prove_snapshot_survived(&self, snapshot: &[SnapshotMarker]) -> Result<(), TypedError> {
        // One outer GUI pane may represent one active inner tmux Split while
        // the same Space owns additional non-visible Groups/Splits. Freeze
        // and re-prove the complete canonical hierarchy once per exact
        // owner/Space/backend incarnation, not merely the visible marker.
        let mut unique: BTreeMap<String, SnapshotMarker> = BTreeMap::new();
        for before in snapshot {
            let key = format!(
                "{}\x1f{}\x1f{}\x1f{}",
                before.authority.marker.host_uid.0,
                before.authority.marker.space_uid.0,
                before.authority.backend_instance.0,
                before.authority.marker.server_epoch.0,
            );
            if let Some(existing) = unique.get_mut(&key) {
                merge_snapshot_hierarchy(
                    &mut existing.authority.hierarchy,
                    &before.authority.hierarchy,
                )?;
                continue;
            }
            unique.insert(key, before.clone());
        }
        for before in unique.into_values() {
            let after = self.validate_authority_marker_in_domain(
                &before.authority.marker,
                Some(&before.gui_domain),
            )?;
            if after.marker != before.authority.marker
                || after.backend_instance != before.authority.backend_instance
                || after.hierarchy.server_epoch != before.authority.hierarchy.server_epoch
            {
                return Err(TypedError::new(
                    ErrorCode::PostconditionFailed,
                    format!(
                        "pane-survival proof changed Space {}/Split {}",
                        before.authority.marker.space_uid.0, before.authority.marker.split_ref
                    ),
                ));
            }
            require_complete_hierarchy_survived(
                before.authority.marker.space_uid,
                &before.authority.hierarchy,
                &after.hierarchy,
            )?;
        }
        Ok(())
    }

    fn disconnect(&self, bound: &BoundGuiOrigin, whole_domain: bool) -> Result<Value, TypedError> {
        if !whole_domain {
            if bound.authority.marker.backend == Backend::Tmux {
                let client_uid = bound.origin.tmux_client_uid.ok_or_else(|| {
                    unavailable(
                        "invoking_client_unavailable: tmux disconnect lost its exact client UID",
                    )
                })?;
                let detached = self.detach_tmux_client(bound, client_uid)?;
                return Ok(serde_json::json!({ "detached": detached }));
            }
            let Some(previous) = self.history.previous_gui_presented() else {
                return Ok(serde_json::json!({
                    "nothing_else_to_present": true,
                    "hint": "use disconnect --domain to detach the current imported domain",
                }));
            };
            if previous.host_uid == bound.authority.marker.host_uid
                && previous.space_uid == bound.authority.marker.space_uid
            {
                return Ok(serde_json::json!({
                    "nothing_else_to_present": true,
                    "hint": "use disconnect --domain to detach the current imported domain",
                }));
            }
            let target = match self.resolve_space(
                &canonical_uri(previous.host_uid, previous.space_uid),
                previous.host_uid,
            ) {
                Ok(target) => target,
                Err(error)
                    if matches!(
                        error.code,
                        ErrorCode::NotFound | ErrorCode::SpaceAbsent | ErrorCode::SpaceDeleted
                    ) =>
                {
                    return Ok(serde_json::json!({
                        "nothing_else_to_present": true,
                        "hint": "the previous Space is no longer attached; use disconnect --domain",
                    }));
                }
                Err(error) => return Err(error),
            };
            let attached: Vec<&BridgePane> = bound
                .heartbeat
                .panes
                .iter()
                .filter(|pane| {
                    pane.context.host_uid == target.marker.host_uid
                        && pane.context.space_uid == target.marker.space_uid
                        && pane.context.server_epoch == target.marker.server_epoch
                        && pane.context.backend == target.marker.backend
                })
                .collect();
            if attached.is_empty() {
                return Ok(serde_json::json!({
                    "nothing_else_to_present": true,
                    "hint": "the previous Space is not already attached; use disconnect --domain",
                }));
            }
            let ack = match target.marker.backend {
                Backend::Wez => self.bridge_present(bound, &target, None, None)?,
                Backend::Tmux => {
                    let mut exact = Vec::new();
                    for pane in attached {
                        let authority = self.validate_authority_marker_in_domain(
                            &pane.context,
                            Some(&pane.domain),
                        )?;
                        if authority.marker.host_uid == target.marker.host_uid
                            && authority.marker.space_uid == target.marker.space_uid
                            && authority.backend_instance == target.backend_instance
                            && authority.marker.server_epoch == target.marker.server_epoch
                        {
                            exact.push((pane, authority));
                        }
                    }
                    let [(pane, authority)] = exact.as_slice() else {
                        if exact.is_empty() {
                            return Ok(serde_json::json!({
                                "nothing_else_to_present": true,
                                "hint": "the previous tmux Space has no exact visible client pane",
                            }));
                        }
                        return Err(TypedError::new(
                            ErrorCode::AmbiguousTarget,
                            "the previous tmux Space is visible through multiple exact clients; use the picker",
                        ));
                    };
                    self.bridge_focus_tmux_pane(bound, authority, pane)?
                }
            };
            return Ok(serde_json::json!({ "presented": ack }));
        }

        if bound.authority.marker.backend != Backend::Wez {
            return Err(unavailable(
                "--domain disconnect applies only to an imported Wez domain",
            ));
        }
        let domain = bound.origin.domain.as_str();
        let manifest = if domain == LOCAL_WEZ_DOMAIN {
            Vec::new()
        } else {
            self.remote_domain_manifest()?
        };
        let domain_state = bound.heartbeat.domains.get(domain).ok_or_else(|| {
            TypedError::new(
                ErrorCode::BridgeUnavailable,
                "current GUI domain is absent from the exact bound heartbeat",
            )
        })?;
        let domain_authority =
            self.prove_attached_domain_sentinel(domain, domain_state, &manifest)?;
        if domain_authority.host_uid != bound.authority.marker.host_uid
            || domain_authority.backend_instance != bound.authority.backend_instance
            || domain_authority.server_epoch != bound.authority.marker.server_epoch
        {
            return Err(TypedError::new(
                ErrorCode::IdentityConflict,
                "current GUI domain sentinel differs from the origin owner/backend epoch",
            ));
        }
        let snapshot = self.snapshot_markers(&bound.heartbeat, Some(domain))?;
        if snapshot.is_empty() {
            return Ok(serde_json::json!({ "nothing_attached": true }));
        }
        if snapshot.iter().any(|item| {
            item.authority.marker.host_uid != bound.authority.marker.host_uid
                || item.authority.backend_instance != bound.authority.backend_instance
                || item.authority.marker.server_epoch != bound.authority.marker.server_epoch
        }) {
            return Err(TypedError::new(
                ErrorCode::IdentityConflict,
                "one GUI domain contains markers from different authorities/backend incarnations",
            ));
        }
        let ack = self.bridge_detach_domain(
            bound,
            domain,
            bound.authority.backend_instance,
            bound.authority.marker.server_epoch,
        )?;
        self.prove_gui_domains_detached(
            &bound.selection.gui_instance,
            bound.selection.pid,
            &bound.selection.process_start_token,
            &[domain.to_string()],
            None,
        )?;
        let after_tree = self.wez_native_tree_for_authority(
            domain_authority.host_uid,
            domain_authority.backend_instance,
            domain_authority.server_epoch,
        )?;
        require_native_tree_survived(domain, &domain_authority.native_tree, &after_tree)?;
        self.prove_snapshot_survived(&snapshot)?;
        Ok(serde_json::json!({
            "detached": ack,
            "surviving_splits": snapshot.len(),
        }))
    }

    fn safe_quit_instance(
        &self,
        selection: &BridgeInstanceSelection,
        heartbeat: &BridgeHeartbeat,
        origin: Value,
    ) -> Result<Value, TypedError> {
        let manifest = self.remote_domain_manifest()?;
        let mut persistent_domains = BTreeSet::from([LOCAL_WEZ_DOMAIN.to_string()]);
        persistent_domains.extend(
            manifest
                .iter()
                .filter(|row| row.compatible && row.remote_wezterm_path.is_some())
                .map(|row| row.name.clone()),
        );
        let snapshot = self.snapshot_markers(heartbeat, None)?;
        let contains_tmux = snapshot
            .iter()
            .any(|pane| pane.authority.marker.backend == Backend::Tmux);
        let mut domains = Vec::new();
        let mut domain_authorities = BTreeMap::new();
        for domain in &persistent_domains {
            let Some(state) = heartbeat.domains.get(domain) else {
                continue;
            };
            match state.state.as_str() {
                "Detached" => continue,
                "Attached" => {
                    let authority =
                        self.prove_attached_domain_sentinel(domain, state, &manifest)?;
                    domains.push(domain.clone());
                    domain_authorities.insert(domain.clone(), authority);
                }
                other => {
                    return Err(TypedError::new(
                        ErrorCode::PostconditionFailed,
                        format!("persistent GUI domain {domain:?} is in transient state {other:?}"),
                    ));
                }
            }
        }
        for pane in &snapshot {
            if pane.authority.marker.backend == Backend::Tmux {
                continue;
            }
            let Some(domain) = domain_authorities.get(&pane.gui_domain) else {
                return Err(TypedError::new(
                    ErrorCode::IdentityConflict,
                    format!(
                        "GUI pane domain {:?} has no exact sentinel authority proof",
                        pane.gui_domain
                    ),
                ));
            };
            if domain.host_uid != pane.authority.marker.host_uid
                || domain.backend_instance != pane.authority.backend_instance
                || domain.server_epoch != pane.authority.marker.server_epoch
            {
                return Err(TypedError::new(
                    ErrorCode::IdentityConflict,
                    format!(
                        "GUI pane marker differs from domain {:?}'s sentinel authority",
                        pane.gui_domain
                    ),
                ));
            }
        }
        let domain_plan = safe_quit_domain_plan(persistent_domains, domains, contains_tmux)?;
        let domains = domain_plan.detach;
        let domain_targets: Vec<Value> = domains
            .iter()
            .map(|name| {
                let authority = domain_authorities
                    .get(name)
                    .expect("every active domain has exact authority");
                serde_json::json!({
                    "name": name,
                    "backend_instance_uid": authority.backend_instance,
                    "server_epoch": authority.server_epoch,
                })
            })
            .collect();
        let mut detach = gui::request_document(
            "safe_quit",
            serde_json::json!({ "phase": "detach", "domains": domain_targets }),
            origin.clone(),
        )
        .map_err(typed_gui)?;
        let proof_uid = detach
            .get("uid")
            .and_then(Value::as_str)
            .expect("request_document always emits uid")
            .to_string();
        gui::call_instance(
            &self.runtime_dir,
            &selection.gui_instance,
            &mut detach,
            gui::ACK_TIMEOUT,
        )
        .map_err(typed_gui)?;

        // The signed ack alone is not the postcondition: require a fresh
        // heartbeat from this exact GUI process to show every requested
        // domain detached and empty before proving owner survival.
        let post_detach = (|| {
            self.prove_gui_domains_detached(
                &selection.gui_instance,
                selection.pid,
                &selection.process_start_token,
                &domains,
                Some(&domain_plan.full_persistent_set),
            )?;

            // GUI domain detachment must not kill owner mux resources.
            for (domain, before) in &domain_authorities {
                let after = self.wez_native_tree_for_authority(
                    before.host_uid,
                    before.backend_instance,
                    before.server_epoch,
                )?;
                require_native_tree_survived(domain, &before.native_tree, &after)?;
            }
            self.prove_snapshot_survived(&snapshot)
        })();
        if let Err(post_error) = post_detach {
            let mut rollback = gui::request_document(
                "safe_quit",
                serde_json::json!({
                    "phase": "rollback",
                    "proof_uid": proof_uid,
                }),
                origin.clone(),
            )
            .map_err(typed_gui)?;
            if let Err(rollback_error) = gui::call_instance(
                &self.runtime_dir,
                &selection.gui_instance,
                &mut rollback,
                gui::ACK_TIMEOUT,
            )
            .map_err(typed_gui)
            {
                return Err(TypedError::new(
                    ErrorCode::PostconditionFailed,
                    format!(
                        "safe_quit post-detach proof failed ({}), and exact-incarnation rollback failed ({})",
                        post_error.message, rollback_error.message
                    ),
                ));
            }
            return Err(post_error);
        }

        let platform_action = safe_quit_platform_action();
        let mut finish = gui::request_document(
            "safe_quit",
            serde_json::json!({
                "phase": "finish",
                "platform_action": platform_action,
                "proof_uid": proof_uid,
            }),
            origin.clone(),
        )
        .map_err(typed_gui)?;
        match gui::call_instance(
            &self.runtime_dir,
            &selection.gui_instance,
            &mut finish,
            gui::ACK_TIMEOUT,
        )
        .map_err(typed_gui)
        {
            Ok(ack) => Ok(ack),
            Err(finish_error) => {
                let mut rollback = gui::request_document(
                    "safe_quit",
                    serde_json::json!({
                        "phase": "rollback",
                        "proof_uid": proof_uid,
                    }),
                    origin,
                )
                .map_err(typed_gui)?;
                match gui::call_instance(
                    &self.runtime_dir,
                    &selection.gui_instance,
                    &mut rollback,
                    gui::ACK_TIMEOUT,
                )
                .map_err(typed_gui)
                {
                    Ok(_) => Err(TypedError::new(
                        ErrorCode::PostconditionFailed,
                        format!(
                            "safe_quit finish acknowledgement failed; exact domains were restored: {}",
                            finish_error.message
                        ),
                    )),
                    Err(rollback_error) => Err(TypedError::new(
                        ErrorCode::PostconditionFailed,
                        format!(
                            "safe_quit finish acknowledgement failed ({}), and exact-incarnation rollback failed ({})",
                            finish_error.message, rollback_error.message
                        ),
                    )),
                }
            }
        }
    }

    fn safe_quit(&self, bound: &BoundGuiOrigin) -> Result<Value, TypedError> {
        let selection = BridgeInstanceSelection {
            gui_instance: bound.selection.gui_instance.clone(),
            pid: bound.selection.pid,
            process_start_token: bound.selection.process_start_token.clone(),
            domains: bound.heartbeat.domains.clone(),
        };
        let origin = gui::in_gui_origin(
            &bound.selection,
            &bound.authority.marker,
            bound.origin.tmux_client_uid,
        );
        self.safe_quit_instance(&selection, &bound.heartbeat, origin)
    }
}

fn marker_owner_route_domain<'a>(
    backend: Backend,
    physical_gui_domain: Option<&'a str>,
) -> Option<&'a str> {
    match backend {
        Backend::Wez => physical_gui_domain,
        Backend::Tmux => None,
    }
}

impl<I: RouteInvoker> GuiAuthority for ProductionGuiAuthority<I> {
    type Bound = BoundGuiOrigin;

    fn bind_origin(&mut self, origin: &GuiCliOrigin) -> Result<Self::Bound, TypedError> {
        let authority = match origin.marker.backend {
            Backend::Wez => {
                self.validate_authority_marker_in_domain(&origin.marker, Some(&origin.domain))?
            }
            Backend::Tmux => self.validate_authority_marker(&origin.marker)?,
        };
        let (selection, heartbeat) =
            gui::bind_cli_origin_with_heartbeat(&self.runtime_dir, origin, &authority.marker)
                .map_err(typed_gui)?;
        let bound = BoundGuiOrigin {
            origin: origin.clone(),
            selection,
            heartbeat,
            authority,
        };
        match bound.authority.marker.backend {
            Backend::Tmux => {
                let client_uid = bound.origin.tmux_client_uid.ok_or_else(|| {
                    unavailable(
                        "invoking_client_unavailable: every tmux GUI action requires its attach-time client UID",
                    )
                })?;
                self.preflight_tmux_client(&bound, client_uid)?;
            }
            Backend::Wez if bound.origin.tmux_client_uid.is_some() => {
                return Err(TypedError::new(
                    ErrorCode::BackendMismatch,
                    "a Wez GUI marker must not carry a tmux client UID",
                ));
            }
            Backend::Wez => {}
        }
        Ok(bound)
    }

    fn execute_bound(
        &mut self,
        bound: &Self::Bound,
        command: &GuiCommand,
    ) -> Result<Value, TypedError> {
        self.partial_result = None;
        // Bind-time validation prevents a stale marker from entering the
        // dispatcher. Repeat it at the execution boundary because native
        // tmux focus is not serialized by dmux filesystem fences: a client
        // may move after heartbeat binding but before a child mutation.
        if bound.authority.marker.backend == Backend::Tmux {
            let client_uid = bound.origin.tmux_client_uid.ok_or_else(|| {
                unavailable(
                    "invoking_client_unavailable: tmux GUI execution lost its attach-time client UID",
                )
            })?;
            self.preflight_tmux_client(bound, client_uid)?;
        }
        match command {
            GuiCommand::Context { cache } => {
                self.finalize_pending_gui_transition_from_bound(bound)?;
                let record = GuiStatusCache::success(
                    bound.selection.gui_instance.clone(),
                    bound.selection.pane_id,
                    bound.authority.marker.clone(),
                    bound.authority.display(),
                )
                .map_err(typed_gui)?;
                if *cache {
                    gui::write_status_cache(&self.runtime_dir, &record).map_err(typed_gui)?;
                }
                serde_json::to_value(record)
                    .map_err(|error| TypedError::new(ErrorCode::OperationFailed, error.to_string()))
            }
            GuiCommand::Spaces { tmux_client_uid } => {
                let exact_uid = Self::command_tmux_client_uid(bound, *tmux_client_uid)?;
                let tmux_scope = match bound.authority.marker.backend {
                    Backend::Wez => None,
                    Backend::Tmux => {
                        debug_assert!(exact_uid.is_some());
                        Some((
                            bound.authority.marker.host_uid,
                            bound.authority.backend_instance,
                            bound.authority.marker.server_epoch,
                        ))
                    }
                };
                Ok(serde_json::json!({
                    "spaces": self.gui_space_rows(&bound.heartbeat, tmux_scope)?,
                }))
            }
            GuiCommand::Present {
                space,
                tmux_client_uid,
            } => {
                let exact_uid = Self::command_tmux_client_uid(bound, *tmux_client_uid)?;
                let target = self.resolve_space(space, bound.authority.marker.host_uid)?;
                match target.marker.backend {
                    Backend::Wez => {
                        let ack = self.bridge_present(bound, &target, None, None)?;
                        Ok(serde_json::json!({ "presented": ack }))
                    }
                    Backend::Tmux => {
                        let client_uid = exact_uid.ok_or_else(|| {
                            unavailable(
                                "invoking_client_unavailable: a Wez origin cannot present a tmux Space without an exact attached tmux client",
                            )
                        })?;
                        let receipt = self.switch_tmux_client(bound, client_uid, &target)?;
                        Ok(serde_json::json!({ "presented": receipt }))
                    }
                }
            }
            GuiCommand::SpaceNew {
                name,
                dir,
                tmux_client_uid,
            } => {
                let exact_uid = Self::command_tmux_client_uid(bound, *tmux_client_uid)?;
                self.create_space_for_origin(bound, name, dir.as_deref(), exact_uid)
            }
            GuiCommand::GroupNew => self.group_new(bound),
            GuiCommand::GroupSelect { relative, index } => {
                self.group_select(bound, relative.as_deref(), *index)
            }
            GuiCommand::GroupRename { name } => self.group_rename(bound, name),
            GuiCommand::GroupRemove {
                confirmed,
                escalate_space,
            } => self.group_remove(bound, *confirmed, *escalate_space),
            GuiCommand::SplitNew { direction } => self.split_new(bound, direction),
            GuiCommand::SplitSelect { direction } => self.split_select(bound, direction),
            GuiCommand::SplitResize { direction, amount } => {
                self.split_resize(bound, direction, *amount)
            }
            GuiCommand::SplitZoom => self.split_zoom(bound),
            GuiCommand::SplitRemove { confirmed } => self.split_remove(bound, *confirmed),
            GuiCommand::Disconnect { domain } => self.disconnect(bound, *domain),
            GuiCommand::SafeQuit => self.safe_quit(bound),
            GuiCommand::Domains | GuiCommand::Summon => Err(TypedError::new(
                ErrorCode::Usage,
                "unbound GUI action reached the bound dispatcher",
            )),
        }
    }

    fn execute_unbound(&mut self, command: &GuiCommand) -> Result<Value, TypedError> {
        self.partial_result = None;
        match command {
            GuiCommand::Domains => Ok(serde_json::json!({
                "domains": self.remote_domain_manifest()?,
            })),
            GuiCommand::Summon => self.summon(),
            _ => Err(TypedError::new(
                ErrorCode::Usage,
                "this GUI action requires --origin-json",
            )),
        }
    }

    fn execute_resident(
        &mut self,
        origin: &GuiResidentCliOrigin,
        command: &GuiCommand,
    ) -> Result<Value, TypedError> {
        self.partial_result = None;
        if !matches!(command, GuiCommand::SafeQuit) {
            return Err(TypedError::new(
                ErrorCode::Usage,
                "resident GUI origin is restricted to safe-quit",
            ));
        }
        let heartbeat = gui::read_instance_heartbeat(&self.runtime_dir, &origin.gui_instance)
            .map_err(typed_gui)?;
        if heartbeat.gui_instance != origin.gui_instance
            || heartbeat.pid != origin.pid
            || heartbeat.process_start_token != origin.process_start_token
        {
            return Err(TypedError::new(
                ErrorCode::IdentityConflict,
                "resident GUI origin differs from its fresh exact heartbeat",
            ));
        }
        let selection = BridgeInstanceSelection {
            gui_instance: heartbeat.gui_instance.clone(),
            pid: heartbeat.pid,
            process_start_token: heartbeat.process_start_token.clone(),
            domains: heartbeat.domains.clone(),
        };
        let signed_origin = gui::resident_gui_origin(&selection);
        self.safe_quit_instance(&selection, &heartbeat, signed_origin)
    }

    fn take_partial_result(&mut self) -> Option<Value> {
        self.partial_result.take()
    }
}

/// Root binary entrypoint. It parses trailing `_gui` argv itself, writes
/// exactly one compact JSON document on stdout, and returns the frozen
/// numeric exit status.
pub fn run_production_argv(origin_json: Option<&str>, argv: &[String]) -> u8 {
    let command = match parse_command(argv) {
        Ok(command) => command,
        Err(error) => return write_response(&GuiResponse::failure(error)),
    };
    let response = match ProductionGuiAuthority::production() {
        Ok(mut authority) => dispatch(&mut authority, origin_json, &command),
        Err(error) => GuiResponse::failure(error),
    };
    write_response(&response)
}

/// Safe adapter for feature-on public `dmux con`/`dmux disconnect` when the
/// command is invoked inside a managed Wez GUI pane. It never derives the
/// actual GUI domain, pane id, or GUI instance from environment variables;
/// those come only from the unique fresh heartbeat after owner marker
/// validation. Native tmux attach/detach remains the terminal PTY path.
pub fn dispatch_ambient_production(command: &GuiCommand) -> GuiResponse {
    if !command.needs_origin() {
        return GuiResponse::failure(TypedError::new(
            ErrorCode::Usage,
            "ambient GUI dispatch accepts only an origin-bound action",
        ));
    }
    let mut authority = match ProductionGuiAuthority::production() {
        Ok(authority) => authority,
        Err(error) => return GuiResponse::failure(error),
    };
    let origin = match authority.ambient_origin() {
        Ok(origin) => origin,
        Err(error) => return GuiResponse::failure(error),
    };
    let encoded = match serde_json::to_string(&origin) {
        Ok(encoded) => encoded,
        Err(error) => {
            return GuiResponse::failure(TypedError::new(
                ErrorCode::OperationFailed,
                format!("serializing ambient GUI origin: {error}"),
            ));
        }
    };
    dispatch(&mut authority, Some(&encoded), command)
}

/// Production owner/live resolver used by the public non-creating `con`
/// orchestrator. The owner is already a stable HostUid; this function never
/// broadens lookup to another host or creates on a miss.
pub fn resolve_production_connect_query(
    query: &OwnerConnectQuery,
) -> Result<FrozenConnectTarget, TypedError> {
    ProductionGuiAuthority::production()?.resolve_connect_query(query)
}

/// Re-read the exact canonical identity, binding endpoint, epoch and child
/// parentage immediately before presentation.
pub fn revalidate_production_connect_target(
    target: &FrozenConnectTarget,
) -> Result<FrozenConnectTarget, TypedError> {
    ProductionGuiAuthority::production()?
        .revalidate_frozen_connect_target(target)
        .map(|(_, refreshed)| refreshed)
}

/// Production presentation gate used by `dmux new` before owner identity
/// reservation. It proves one selected Wez owner incarnation and one exact
/// fresh GUI/domain route; it never creates a Space or a user pane.
pub fn preflight_new_wez_presentation_production(
    owner: HostUid,
    mode: NewPresentationMode,
) -> Result<WezPresentationPreflight, TypedError> {
    ProductionGuiAuthority::production()?.preflight_new_wez_presentation(owner, mode)
}

/// Assemble §8.3 backend-decision facts for public `dmux new`. Ordinary
/// decisions are read-only. Explicit Wez `--launch-gui` may establish the
/// fixed service and attach-only GUI before returning eligibility, but still
/// reserves/creates no Space or user pane. Identity/protocol failures remain
/// terminal rather than selecting a tmux policy row.
pub fn new_creation_context_production(
    owner: HostUid,
    explicit_backend: Option<Backend>,
    launch_gui: bool,
) -> Result<CreationContext, TypedError> {
    ProductionGuiAuthority::production()?.new_creation_context(owner, explicit_backend, launch_gui)
}

/// Present one already-frozen Wez target through the uniquely bound ambient
/// GUI. Group/Split focus is taken only from the re-correlated frozen child.
pub fn present_frozen_ambient_production(
    target: &FrozenConnectTarget,
) -> Result<PresentationReceipt, TypedError> {
    ProductionGuiAuthority::production()?.present_frozen_ambient(target)
}

/// Explicit attach-only cold GUI path for one already-frozen Wez target.
/// tmux/missing targets are rejected before the fixed service or GUI runner.
pub fn present_frozen_cold_production(
    target: &FrozenConnectTarget,
) -> Result<PresentationReceipt, TypedError> {
    ProductionGuiAuthority::production()?.present_frozen_cold(target)
}

/// Exact source captured before correlation emits any OSC user-variable
/// update. Fields remain private so callers can only pass the witness back to
/// the staged/finalized transition API.
#[derive(Debug, Clone)]
pub struct GuiExecSourceWitness {
    source: GuiHistoryTarget,
    source_backend_instance_uid: BackendInstanceUid,
    selection: BridgeSelection,
    marker: MarkerContext,
    tmux_client_uid: Option<Uuid>,
}

impl GuiExecSourceWitness {
    /// Exact already-attached tmux client captured before correlation.  The
    /// exec orchestrator uses this only to reserve a local switch against
    /// the same client; `None` identifies a native Wez source pane.
    pub fn tmux_client_uid(&self) -> Option<Uuid> {
        self.tmux_client_uid
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuiExecTransitionOutcome {
    TerminalOnly,
    Staged { pending_uid: Uuid },
}

fn unix_now() -> Result<u64, TypedError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| {
            TypedError::new(ErrorCode::OperationFailed, format!("system clock: {error}"))
        })
}

fn pending_destination_marker_matches(
    pending: &PendingGuiTransition,
    observed: &MarkerContext,
) -> bool {
    let expected = &pending.destination_marker;
    if observed.host_uid != expected.host_uid
        || observed.space_uid != expected.space_uid
        || observed.space_no != expected.space_no
        || observed.backend != expected.backend
        || observed.server_epoch != expected.server_epoch
    {
        return false;
    }
    match pending.destination_child_kind {
        None => true,
        Some(ChildKind::Group) => observed.group_ref == expected.group_ref,
        Some(ChildKind::Split) => {
            observed.group_ref == expected.group_ref && observed.split_ref == expected.split_ref
        }
    }
}

fn pending_gui_transition_ttl_seconds(kind: crate::connect_cli::TmuxExecKind) -> u64 {
    match kind {
        crate::connect_cli::TmuxExecKind::RemoteAttach => 65,
        crate::connect_cli::TmuxExecKind::LocalAttach
        | crate::connect_cli::TmuxExecKind::LocalSwitch => 30,
    }
}

fn require_captured_tmux_source_live<V, P>(
    source: &GuiExecSourceWitness,
    validate: V,
    preflight_client: P,
) -> Result<(), TypedError>
where
    V: FnOnce(&MarkerContext) -> Result<AuthorityMarker, TypedError>,
    P: FnOnce(&AuthorityMarker, Uuid) -> Result<(), TypedError>,
{
    let source_uid = source.tmux_client_uid.ok_or_else(|| {
        TypedError::new(
            ErrorCode::IdentityConflict,
            "captured tmux source lost its exact client UID before staging",
        )
    })?;
    let exact_source = validate(&source.marker)?;
    if exact_source.backend_instance != source.source_backend_instance_uid {
        return Err(TypedError::new(
            ErrorCode::WrongBackendInstance,
            "captured tmux source backend instance changed before staging",
        ));
    }
    preflight_client(&exact_source, source_uid)
}

/// Capture a managed GUI source before any tmux correlation marker is
/// stamped. No-marker/flag-off terminals return `None`; partial or stale
/// managed identity is a hard error. Both Wez and already-correlated tmux
/// sources are supported (the latter covers exact local switch-client).
pub fn capture_exact_gui_exec_source_production(
    plan: &crate::connect_cli::OwnerExecPlan,
) -> Result<Option<GuiExecSourceWitness>, TypedError> {
    let target = plan.target();
    if target.backend != Backend::Tmux {
        return Err(TypedError::new(
            ErrorCode::BackendMismatch,
            "correlated GUI exec history requires an exact tmux destination",
        ));
    }
    if std::env::var("DMUX_WEZ_FIRST").as_deref() != Ok("1")
        || std::env::var_os("WEZTERM_PANE").is_none()
    {
        return Ok(None);
    }

    const MARKER_ENV: [&str; 8] = [
        "DMUX_CONTEXT_VERSION",
        "DMUX_HOST_UID",
        "DMUX_SPACE_UID",
        "DMUX_SPACE_NO",
        "DMUX_BACKEND",
        "DMUX_SERVER_EPOCH",
        "DMUX_GROUP_REF",
        "DMUX_SPLIT_REF",
    ];
    let present = MARKER_ENV
        .iter()
        .filter(|name| std::env::var_os(name).is_some())
        .count();
    if present == 0 {
        return Ok(None);
    }
    if present != MARKER_ENV.len() {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "managed GUI source marker is partial at the tmux exec boundary",
        ));
    }
    let mut authority = ProductionGuiAuthority::production()?;
    let ambient = ambient_marker_from_env()?;
    let authoritative = authority.validate_authority_marker(&ambient)?;
    if authoritative.marker != ambient {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "ambient pane marker changed during GUI exec source capture",
        ));
    }
    let selection =
        gui::discover_in_gui_instance(&authority.runtime_dir, &ambient).map_err(typed_gui)?;
    let heartbeat = gui::read_instance_heartbeat(&authority.runtime_dir, &selection.gui_instance)
        .map_err(typed_gui)?;
    let pane_matches: Vec<_> = heartbeat
        .panes
        .iter()
        .filter(|pane| {
            pane.pane_id == selection.pane_id
                && pane.domain == selection.domain
                && pane.context == authoritative.marker
        })
        .collect();
    let [pane] = pane_matches.as_slice() else {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "GUI exec source pane changed during exact heartbeat capture",
        ));
    };
    match authoritative.marker.backend {
        Backend::Tmux => {
            if plan.kind() != crate::connect_cli::TmuxExecKind::LocalSwitch {
                return Err(TypedError::new(
                    ErrorCode::WrongBackendInstance,
                    "a managed tmux source may rotate GUI history only for exact local switch-client",
                ));
            }
            if pane.tmux_client_uid.is_none() {
                return Err(TypedError::new(
                    ErrorCode::IdentityConflict,
                    "managed tmux source heartbeat omitted its exact client UID",
                ));
            }
        }
        Backend::Wez if std::env::var_os("TMUX").is_some() => {
            return Err(TypedError::new(
                ErrorCode::WrongBackendInstance,
                "nested tmux cannot use a native Wez source witness",
            ));
        }
        Backend::Wez => {}
    }
    let candidate = GuiCliOrigin {
        protocol_version: gui::BRIDGE_PROTOCOL_VERSION,
        gui_instance: selection.gui_instance,
        pane_id: selection.pane_id,
        domain: selection.domain,
        tmux_client_uid: pane.tmux_client_uid,
        marker: authoritative.marker,
    };
    let encoded = serde_json::to_string(&candidate)
        .map_err(|error| TypedError::new(ErrorCode::OperationFailed, error.to_string()))?;
    let origin = gui::parse_origin_json(&encoded).map_err(typed_gui)?;
    let bound = <ProductionGuiAuthority as GuiAuthority>::bind_origin(&mut authority, &origin)?;
    Ok(Some(GuiExecSourceWitness {
        source: GuiHistoryTarget {
            host_uid: bound.authority.marker.host_uid,
            space_uid: bound.authority.marker.space_uid,
        },
        source_backend_instance_uid: bound.authority.backend_instance,
        selection: bound.selection,
        marker: bound.authority.marker,
        tmux_client_uid: bound.origin.tmux_client_uid,
    }))
}

/// Stage, but do not publish, one exact source→tmux transition. This must run
/// after a client UID is reserved and before it or a destination marker is
/// stamped. GUI current/previous remain unchanged until the finalizer sees
/// the exact destination in the same live GUI pane.
pub fn stage_correlated_gui_exec_transition_production(
    plan: &crate::connect_cli::OwnerExecPlan,
    tmux_client_uid: Uuid,
    source: Option<&GuiExecSourceWitness>,
) -> Result<GuiExecTransitionOutcome, TypedError> {
    let Some(source) = source else {
        return Ok(GuiExecTransitionOutcome::TerminalOnly);
    };
    let target = plan.target();
    if target.backend != Backend::Tmux {
        return Err(TypedError::new(
            ErrorCode::BackendMismatch,
            "correlated GUI exec transition requires a tmux destination",
        ));
    }
    if plan.kind() == crate::connect_cli::TmuxExecKind::LocalSwitch
        && source.tmux_client_uid != Some(tmux_client_uid)
    {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "local switch client UID differs from the captured GUI source",
        ));
    }

    let authority = ProductionGuiAuthority::production()?;
    let heartbeat =
        gui::read_instance_heartbeat(&authority.runtime_dir, &source.selection.gui_instance)
            .map_err(typed_gui)?;
    if heartbeat.pid != source.selection.pid
        || heartbeat.process_start_token != source.selection.process_start_token
        || heartbeat
            .panes
            .iter()
            .filter(|pane| {
                pane.pane_id == source.selection.pane_id
                    && pane.domain == source.selection.domain
                    && pane.context == source.marker
                    && pane.tmux_client_uid == source.tmux_client_uid
            })
            .count()
            != 1
    {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "captured GUI source changed before transition staging",
        ));
    }

    let destination =
        authority.resolve_space(&canonical_uri(target.owner, target.space_uid), target.owner)?;
    if destination.marker.host_uid != target.owner
        || destination.marker.space_uid != target.space_uid
        || destination.marker.space_no != target.space_no
        || destination.marker.backend != Backend::Tmux
        || destination.backend_instance != target.backend_instance_uid
        || destination.marker.server_epoch != target.server_epoch
    {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "tmux exec destination changed before transition staging",
        ));
    }
    // Owner resolution may cross a network boundary. Repeat the exact source
    // proof after it, immediately before persisting the pending transition.
    let heartbeat =
        gui::read_instance_heartbeat(&authority.runtime_dir, &source.selection.gui_instance)
            .map_err(typed_gui)?;
    if heartbeat.pid != source.selection.pid
        || heartbeat.process_start_token != source.selection.process_start_token
        || heartbeat
            .panes
            .iter()
            .filter(|pane| {
                pane.pane_id == source.selection.pane_id
                    && pane.domain == source.selection.domain
                    && pane.context == source.marker
                    && pane.tmux_client_uid == source.tmux_client_uid
            })
            .count()
            != 1
    {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "captured GUI source changed during destination resolution",
        ));
    }
    if source.marker.backend == Backend::Tmux {
        require_captured_tmux_source_live(
            source,
            |marker| authority.validate_authority_marker(marker),
            |marker, source_uid| {
                authority
                    .preflight_tmux_marker_client(marker, source_uid)
                    .map(|_| ())
            },
        )?;
    }
    let (group_ref, split_ref) = frozen_connect_child_refs(target);
    let mut destination_marker = destination.marker.clone();
    if let Some(group_ref) = group_ref {
        destination_marker.group_ref = group_ref;
    }
    if let Some(split_ref) = split_ref {
        destination_marker.split_ref = split_ref;
    }
    authority
        .history
        .stage_gui_transition(PendingGuiTransition {
            tmux_client_uid,
            source: source.source,
            destination: GuiHistoryTarget {
                host_uid: target.owner,
                space_uid: target.space_uid,
            },
            destination_backend_instance_uid: target.backend_instance_uid,
            destination_marker,
            destination_child_kind: target.child.as_ref().map(VerifiedConnectChild::kind),
            gui_instance: source.selection.gui_instance.clone(),
            gui_pid: source.selection.pid,
            gui_process_start_token: source.selection.process_start_token.clone(),
            gui_pane_id: source.selection.pane_id,
            gui_domain: source.selection.domain.clone(),
            expires_at: unix_now()?.saturating_add(pending_gui_transition_ttl_seconds(plan.kind())),
        })
        .map_err(|error| {
            TypedError::new(
                ErrorCode::OperationFailed,
                format!("staging correlated GUI presentation history: {error}"),
            )
        })?;
    Ok(GuiExecTransitionOutcome::Staged {
        pending_uid: tmux_client_uid,
    })
}

/// Bounded monitor entry point. It records GUI history only after one fresh
/// heartbeat shows the exact destination marker and client UID in the same
/// GUI process/pane, followed by owner live revalidation. Timeout cancels the
/// pending record and leaves GUI history unchanged.
pub fn finalize_correlated_gui_exec_transition_production(
    pending_uid: Uuid,
    timeout: Duration,
) -> Result<bool, TypedError> {
    let authority = ProductionGuiAuthority::production()?;
    let Some(pending) = authority.history.pending_gui_transition(pending_uid) else {
        return Ok(false);
    };
    let deadline = Instant::now() + timeout;
    loop {
        if authority
            .history
            .pending_gui_transition(pending_uid)
            .as_ref()
            != Some(&pending)
        {
            return Ok(false);
        }
        let now = match unix_now() {
            Ok(now) => now,
            Err(error) => {
                let _ = authority.history.cancel_gui_transition(pending_uid);
                return Err(error);
            }
        };
        if now >= pending.expires_at || Instant::now() >= deadline {
            authority
                .history
                .cancel_gui_transition(pending_uid)
                .map_err(|error| {
                    TypedError::new(
                        ErrorCode::OperationFailed,
                        format!("canceling expired GUI transition: {error}"),
                    )
                })?;
            return Ok(false);
        }
        match gui::read_instance_heartbeat(&authority.runtime_dir, &pending.gui_instance) {
            Ok(heartbeat) => {
                if heartbeat.pid != pending.gui_pid
                    || heartbeat.process_start_token != pending.gui_process_start_token
                {
                    authority
                        .history
                        .cancel_gui_transition(pending_uid)
                        .map_err(|error| {
                            TypedError::new(
                                ErrorCode::OperationFailed,
                                format!("canceling raced GUI transition: {error}"),
                            )
                        })?;
                    return Err(TypedError::new(
                        ErrorCode::IdentityConflict,
                        "GUI process incarnation changed while awaiting tmux presentation",
                    ));
                }
                let matches: Vec<_> = heartbeat
                    .panes
                    .iter()
                    .filter(|pane| {
                        pane.pane_id == pending.gui_pane_id
                            && pane.domain == pending.gui_domain
                            && pane.tmux_client_uid == Some(pending_uid)
                            && pane.context.backend == Backend::Tmux
                            && pane.context.host_uid == pending.destination.host_uid
                            && pane.context.space_uid == pending.destination.space_uid
                            && pending_destination_marker_matches(&pending, &pane.context)
                    })
                    .collect();
                if let [pane] = matches.as_slice() {
                    // OSC user variables arrive sequentially. A base Space
                    // identity can therefore be new while Group/Split or
                    // the native client still reflects the source. Such a
                    // mixed witness cannot commit, but is retried within the
                    // same bounded incarnation monitor.
                    if let Ok(exact) = authority.validate_authority_marker(&pane.context) {
                        if exact.backend_instance != pending.destination_backend_instance_uid {
                            let _ = authority.history.cancel_gui_transition(pending_uid);
                            return Err(TypedError::new(
                                ErrorCode::IdentityConflict,
                                "visible tmux destination backend instance differs from the staged transition",
                            ));
                        }
                        if authority
                            .preflight_tmux_marker_client(&exact, pending_uid)
                            .is_ok()
                        {
                            return match authority.history.complete_gui_transition(&pending) {
                                Ok(completed) => Ok(completed),
                                Err(error) => {
                                    let _ = authority.history.cancel_gui_transition(pending_uid);
                                    Err(TypedError::new(
                                        ErrorCode::OperationFailed,
                                        format!("finalizing GUI transition: {error}"),
                                    ))
                                }
                            };
                        }
                    }
                }
            }
            // User-variable updates arrive as a short OSC sequence; a
            // heartbeat sampled mid-sequence is invalid but cannot commit.
            // Retry it only within this bounded monitor deadline.
            Err(GuiError::BridgeUnavailable(_)) | Err(GuiError::InvalidInstance(_)) => {}
            Err(error) => {
                let _ = authority.history.cancel_gui_transition(pending_uid);
                return Err(typed_gui(error));
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

pub fn cancel_correlated_gui_exec_transition_production(
    pending_uid: Uuid,
) -> Result<bool, TypedError> {
    let state_dir = History::default_dir().ok_or_else(|| {
        TypedError::new(
            ErrorCode::OperationFailed,
            "HOME/XDG_STATE_HOME is unavailable for GUI transition cancellation",
        )
    })?;
    History::new(state_dir)
        .cancel_gui_transition(pending_uid)
        .map_err(|error| {
            TypedError::new(
                ErrorCode::OperationFailed,
                format!("canceling GUI transition: {error}"),
            )
        })
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::num::NonZeroU64;

    use super::*;
    use crate::gui::{BridgeDomainState, BridgePane};
    use crate::model::ProviderHandle;
    use crate::remote::client::DirectInvoker;
    use crate::remote::wez_compat::{
        CAP_ACTIVATE_EXISTING, CAP_ATTACH_NO_CREATE, CAP_TMUX, CAP_WEZ, RemoteWezRefusal,
        assess_automatic_remote_wez,
    };

    fn marker() -> MarkerContext {
        let host = HostUid(Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap());
        let space = SpaceUid(Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap());
        let epoch = ServerEpoch(Uuid::parse_str("33333333-3333-4333-8333-333333333333").unwrap());
        MarkerContext {
            host_uid: host,
            space_uid: space,
            space_no: SpaceNo(NonZeroU64::new(7).unwrap()),
            backend: Backend::Wez,
            domain: Some("dmux".into()),
            server_epoch: epoch,
            group_ref: format!("g{}.wz-4", epoch.0),
            split_ref: format!("p{}.wz-5", epoch.0),
        }
    }

    #[test]
    fn physical_gui_domain_constrains_only_wez_owner_routes() {
        assert_eq!(
            marker_owner_route_domain(Backend::Wez, Some("dmux-b-usb")),
            Some("dmux-b-usb")
        );
        assert_eq!(
            marker_owner_route_domain(Backend::Tmux, Some("dmux")),
            None,
            "a local outer Wez domain is not a route fact for a remote tmux owner"
        );
    }

    fn origin(marker: &MarkerContext) -> GuiCliOrigin {
        GuiCliOrigin {
            protocol_version: 1,
            gui_instance: "gui-test-1".into(),
            pane_id: 51,
            domain: "dmux".into(),
            tmux_client_uid: None,
            marker: marker.clone(),
        }
    }

    struct RefusingAuthority {
        binds: Cell<u32>,
        actions: Cell<u32>,
    }

    struct PartialAuthority {
        partial: Option<Value>,
    }

    impl GuiAuthority for PartialAuthority {
        type Bound = ();

        fn bind_origin(&mut self, _: &GuiCliOrigin) -> Result<Self::Bound, TypedError> {
            Ok(())
        }

        fn execute_bound(&mut self, _: &(), _: &GuiCommand) -> Result<Value, TypedError> {
            self.partial = Some(serde_json::json!({
                "created": true,
                "connected": false,
                "stable_ref": "dmux://11111111-1111-4111-8111-111111111111/22222222-2222-4222-8222-222222222222",
            }));
            Err(TypedError::new(
                ErrorCode::PartialResult,
                "creation completed but presentation failed",
            ))
        }

        fn execute_unbound(&mut self, _: &GuiCommand) -> Result<Value, TypedError> {
            unreachable!("test command is bound")
        }

        fn take_partial_result(&mut self) -> Option<Value> {
            self.partial.take()
        }
    }

    impl GuiAuthority for RefusingAuthority {
        type Bound = ();

        fn bind_origin(&mut self, _: &GuiCliOrigin) -> Result<Self::Bound, TypedError> {
            self.binds.set(self.binds.get() + 1);
            Err(TypedError::new(
                ErrorCode::HostIdentityChanged,
                "test authority refusal",
            ))
        }

        fn execute_bound(&mut self, _: &(), _: &GuiCommand) -> Result<Value, TypedError> {
            self.actions.set(self.actions.get() + 1);
            Ok(serde_json::json!({}))
        }

        fn execute_unbound(&mut self, _: &GuiCommand) -> Result<Value, TypedError> {
            self.actions.set(self.actions.get() + 1);
            Ok(serde_json::json!({}))
        }
    }

    #[test]
    fn invalid_or_authority_refused_origin_reaches_no_action() {
        let mut authority = RefusingAuthority {
            binds: Cell::new(0),
            actions: Cell::new(0),
        };
        let malformed = dispatch(
            &mut authority,
            Some(r#"{"protocol_version":1,"unknown":true}"#),
            &GuiCommand::Context { cache: false },
        );
        assert!(!malformed.ok);
        assert_eq!(authority.binds.get(), 0);
        assert_eq!(authority.actions.get(), 0);

        let raw = serde_json::to_string(&origin(&marker())).unwrap();
        let refused = dispatch(&mut authority, Some(&raw), &GuiCommand::GroupNew);
        assert!(!refused.ok);
        assert_eq!(refused.error.as_deref(), Some("host_identity_changed"));
        assert_eq!(authority.binds.get(), 1);
        assert_eq!(authority.actions.get(), 0);
    }

    #[test]
    fn partial_result_keeps_the_durable_created_identity_and_exit_seven() {
        let raw = serde_json::to_string(&origin(&marker())).unwrap();
        let mut authority = PartialAuthority { partial: None };
        let response = dispatch(&mut authority, Some(&raw), &GuiCommand::GroupNew);
        assert!(!response.ok);
        assert_eq!(response.error.as_deref(), Some("partial_result"));
        assert_eq!(response.exit_code(), 7);
        let result = response.result.unwrap();
        assert_eq!(result["created"], true);
        assert_eq!(result["connected"], false);
        assert!(
            result["stable_ref"]
                .as_str()
                .unwrap()
                .starts_with("dmux://")
        );
    }

    #[test]
    fn safe_quit_refuses_disappearance_of_a_nonactive_owner_pane() {
        let marker = marker();
        let before = SpaceHierarchy {
            space_uid: marker.space_uid,
            server_epoch: marker.server_epoch,
            groups: vec![
                operations::HierarchyGroup {
                    group_ref: marker.group_ref.clone(),
                    title: Some("visible".into()),
                    splits: vec![operations::HierarchySplit {
                        split_ref: marker.split_ref.clone(),
                        title: None,
                        cwd: Some("/visible".into()),
                    }],
                },
                operations::HierarchyGroup {
                    group_ref: format!("g{}.wz-99", marker.server_epoch.0),
                    title: Some("not represented by the active marker".into()),
                    splits: vec![operations::HierarchySplit {
                        split_ref: format!("p{}.wz-100", marker.server_epoch.0),
                        title: None,
                        cwd: Some("/hidden".into()),
                    }],
                },
            ],
        };
        let mut after = before.clone();
        after.groups.pop();
        let error =
            require_complete_hierarchy_survived(marker.space_uid, &before, &after).unwrap_err();
        assert_eq!(error.code, ErrorCode::PostconditionFailed);
        assert!(require_complete_hierarchy_survived(marker.space_uid, &before, &before).is_ok());

        let mut superset = before.clone();
        superset.groups.reverse();
        superset.groups[0].title = Some("harmless metadata update".into());
        superset.groups.push(operations::HierarchyGroup {
            group_ref: format!("g{}.wz-101", marker.server_epoch.0),
            title: Some("new Group".into()),
            splits: vec![operations::HierarchySplit {
                split_ref: format!("p{}.wz-102", marker.server_epoch.0),
                title: None,
                cwd: None,
            }],
        });
        assert!(
            require_complete_hierarchy_survived(marker.space_uid, &before, &superset).is_ok(),
            "safe quit permits additive children, order changes, and metadata changes"
        );
    }

    #[test]
    fn safe_quit_refuses_sentinel_or_physical_pane_loss_including_sentinel_only_domain() {
        let instance = BackendInstanceUid(Uuid::new_v4());
        let epoch = ServerEpoch(Uuid::new_v4());
        let sentinel_only = WezNativeTreeResult {
            backend_instance_uid: instance,
            server_epoch: epoch,
            sentinel_window_id: 1,
            sentinel_tab_id: 2,
            sentinel_pane_id: 3,
            panes: vec![WezNativePaneWitness {
                window_id: 1,
                tab_id: 2,
                pane_id: 3,
            }],
        };
        let changed_sentinel = WezNativeTreeResult {
            sentinel_pane_id: 4,
            panes: vec![WezNativePaneWitness {
                window_id: 1,
                tab_id: 2,
                pane_id: 4,
            }],
            ..sentinel_only.clone()
        };
        assert_eq!(
            require_native_tree_survived("b-sentinel-only", &sentinel_only, &changed_sentinel)
                .unwrap_err()
                .code,
            ErrorCode::PostconditionFailed
        );

        let with_user = WezNativeTreeResult {
            panes: vec![
                sentinel_only.panes[0].clone(),
                WezNativePaneWitness {
                    window_id: 10,
                    tab_id: 20,
                    pane_id: 30,
                },
            ],
            ..sentinel_only.clone()
        };
        assert_eq!(
            require_native_tree_survived("a-user", &with_user, &sentinel_only)
                .unwrap_err()
                .code,
            ErrorCode::PostconditionFailed
        );
        let mut additive = with_user.clone();
        additive.panes.push(WezNativePaneWitness {
            window_id: 11,
            tab_id: 21,
            pane_id: 31,
        });
        assert!(require_native_tree_survived("a-user", &with_user, &additive).is_ok());
    }

    #[test]
    fn context_result_echoes_the_exact_authoritative_local_marker() {
        let scratch = tempfile::tempdir().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let marker = marker();
        let authority_marker = AuthorityMarker {
            marker: marker.clone(),
            backend_instance: BackendInstanceUid(
                Uuid::parse_str("44444444-4444-4444-8444-444444444444").unwrap(),
            ),
            logical_name: "project".into(),
            health: Health::Healthy,
            hierarchy: SpaceHierarchy {
                space_uid: marker.space_uid,
                server_epoch: marker.server_epoch,
                groups: vec![operations::HierarchyGroup {
                    group_ref: marker.group_ref.clone(),
                    title: Some("editor".into()),
                    splits: vec![operations::HierarchySplit {
                        split_ref: marker.split_ref.clone(),
                        title: None,
                        cwd: Some("/tmp".into()),
                    }],
                }],
            },
            owner_alias: "a".into(),
            owner_label: "macie".into(),
            route: "local".into(),
            location: AuthorityLocation::Local,
        };
        let mut domains = BTreeMap::new();
        domains.insert(
            "dmux".into(),
            BridgeDomainState {
                state: "Attached".into(),
                has_any_panes: true,
                backend_instance_uid: Some(authority_marker.backend_instance),
                pane_count: 1,
                valid_marker_pane_count: 1,
                system_pane_count: 0,
                system_workspace: None,
                system_epoch: None,
            },
        );
        let bound = BoundGuiOrigin {
            origin: origin(&marker),
            selection: BridgeSelection {
                gui_instance: "gui-test-1".into(),
                pid: std::process::id(),
                process_start_token: "test-token".into(),
                pane_id: 51,
                domain: "dmux".into(),
            },
            heartbeat: BridgeHeartbeat {
                protocol_version: 1,
                gui_instance: "gui-test-1".into(),
                pid: std::process::id(),
                process_start_token: "test-token".into(),
                updated_at: 1,
                panes: vec![BridgePane {
                    pane_id: 51,
                    domain: "dmux".into(),
                    tmux_client_uid: None,
                    context: marker.clone(),
                }],
                domains,
            },
            authority: authority_marker,
        };
        let mut production = ProductionGuiAuthority::with_dependencies(
            OperationEnv {
                db_path: scratch.path().join("registry.sqlite3"),
                lock_dir: runtime.path().to_path_buf(),
            },
            runtime.path().to_path_buf(),
            state.path().to_path_buf(),
            "/bin/false".into(),
            "/dev/null".into(),
            PathBuf::from("/dev/null"),
            "/bin/false".into(),
            DirectInvoker,
        );
        let value = production
            .execute_bound(&bound, &GuiCommand::Context { cache: false })
            .unwrap();
        let cache: GuiStatusCache = serde_json::from_value(value).unwrap();
        assert_eq!(cache.marker, marker);
        assert_eq!(cache.gui_instance, "gui-test-1");
        assert_eq!(cache.pane_id, 51);
        assert_eq!(cache.display.unwrap().logical_ref, "a7");
    }

    #[test]
    fn build_mismatch_remains_an_incompatible_domain_not_tmux_fallback() {
        let controller = vec![
            CAP_WEZ.into(),
            "wez:build:controller-build".into(),
            CAP_ATTACH_NO_CREATE.into(),
            CAP_ACTIVATE_EXISTING.into(),
        ];
        let owner = vec![
            CAP_WEZ.into(),
            CAP_TMUX.into(),
            "wez:build:owner-build".into(),
            "wez:path:/usr/bin/wezterm".into(),
            CAP_ATTACH_NO_CREATE.into(),
            CAP_ACTIVATE_EXISTING.into(),
        ];
        let assessment = assess_automatic_remote_wez(&controller, &owner);
        assert!(matches!(
            assessment.refusal,
            Some(RemoteWezRefusal::BuildMismatch { .. })
        ));
        assert!(assessment.explicit_tmux_available);
        assert!(!assessment.is_eligible());

        let rows = gui::build_domain_manifest(vec![RemoteDomainSource {
            name: "dmux-b-usb".into(),
            remote_address: "10.77.77.2".into(),
            username: "fredrir".into(),
            remote_wezterm_path: Some("/usr/bin/wezterm".into()),
            managed_socket: None,
            host_uid: marker().host_uid,
            backend_instance_uid: BackendInstanceUid(Uuid::new_v4()),
            route_id: 7,
            priority: 10,
            transport: Transport::Openssh,
            network_class: crate::registry::NetworkClass::Usb,
            compatible: false,
            unavailable_reason: Some(assessment.typed_error().unwrap().message),
        }])
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].compatible);
        assert!(rows[0].unavailable_reason.is_some());

        let owner_without_path = vec![
            CAP_WEZ.into(),
            "wez:build:controller-build".into(),
            CAP_ATTACH_NO_CREATE.into(),
            CAP_ACTIVATE_EXISTING.into(),
        ];
        let missing_path = assess_automatic_remote_wez(&controller, &owner_without_path);
        assert!(matches!(
            missing_path.refusal,
            Some(RemoteWezRefusal::OwnerPathMissing)
        ));
        let missing_path_rows = gui::build_domain_manifest(vec![RemoteDomainSource {
            name: "dmux-b-ts".into(),
            remote_address: "archie.tail.example".into(),
            username: "fredrir".into(),
            remote_wezterm_path: None,
            managed_socket: None,
            host_uid: marker().host_uid,
            backend_instance_uid: BackendInstanceUid(Uuid::new_v4()),
            route_id: 8,
            priority: 20,
            transport: Transport::Openssh,
            network_class: crate::registry::NetworkClass::Tailscale,
            compatible: false,
            unavailable_reason: Some(missing_path.typed_error().unwrap().message),
        }])
        .unwrap();
        assert!(!missing_path_rows[0].compatible);
        assert!(missing_path_rows[0].remote_wezterm_path.is_none());
        let wire = serde_json::to_value(&missing_path_rows[0]).unwrap();
        assert!(
            !wire
                .as_object()
                .unwrap()
                .contains_key("remote_wezterm_path")
        );
    }

    #[test]
    fn presentation_request_is_no_create_and_contains_only_logical_identity() {
        let marker = marker();
        let selection = BridgeSelection {
            gui_instance: "gui-test-1".into(),
            pid: 7,
            process_start_token: "token".into(),
            pane_id: 51,
            domain: "dmux".into(),
        };
        let request = gui::request_document(
            "present",
            serde_json::json!({
                "backend_instance_uid": Uuid::parse_str("44444444-4444-4444-8444-444444444444").unwrap(),
                "domain": "dmux",
                "host_uid": marker.host_uid,
                "server_epoch": marker.server_epoch,
                "space_uid": marker.space_uid,
                "workspace": format!("dmux:{}:{}", marker.host_uid.0, marker.space_uid.0),
                "group_ref": marker.group_ref.clone(),
                "split_ref": marker.split_ref.clone(),
            }),
            gui::in_gui_origin(&selection, &marker, None),
        )
        .unwrap();
        assert_eq!(request["action"], "present");
        let target = request["target"].as_object().unwrap();
        for forbidden in [
            "argv",
            "command",
            "cwd",
            "create",
            "pane_id",
            "tab_id",
            "window_id",
        ] {
            assert!(!target.contains_key(forbidden), "unexpected {forbidden}");
        }
        assert_eq!(
            target["workspace"],
            format!("dmux:{}:{}", marker.host_uid.0, marker.space_uid.0)
        );
        assert_eq!(
            provider_handle_for_test(&marker.group_ref),
            ProviderHandle::Wz(4)
        );
    }

    #[test]
    fn cold_tmux_or_missing_preflight_never_enters_service_or_gui_runner() {
        let starts = Cell::new(0_u32);
        let launches = Cell::new(0_u32);
        let tmux = ColdSpaceIdentity {
            host_uid: marker().host_uid,
            space_uid: marker().space_uid,
            backend: Backend::Tmux,
        };
        let tmux_error = enter_cold_wez_lifecycle(Ok(tmux), |_| {
            starts.set(starts.get() + 1);
            launches.set(launches.get() + 1);
            Ok(())
        })
        .unwrap_err();
        assert_eq!(tmux_error.code, ErrorCode::BackendMismatch);

        let missing_error = enter_cold_wez_lifecycle(
            Err(TypedError::new(ErrorCode::NotFound, "test missing target")),
            |_| {
                starts.set(starts.get() + 1);
                launches.set(launches.get() + 1);
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(missing_error.code, ErrorCode::NotFound);
        assert_eq!(starts.get(), 0);
        assert_eq!(launches.get(), 0);
    }

    #[test]
    fn frozen_split_presentation_keeps_exact_parent_and_epoch() {
        let marker = marker();
        let target = FrozenConnectTarget {
            owner: marker.host_uid,
            space_uid: marker.space_uid,
            space_no: marker.space_no,
            logical_name: "project".into(),
            backend: Backend::Wez,
            backend_instance_uid: BackendInstanceUid(
                Uuid::parse_str("44444444-4444-4444-8444-444444444444").unwrap(),
            ),
            server_epoch: marker.server_epoch,
            binding: FrozenBinding {
                native_token: format!("dmux:{}:{}", marker.host_uid.0, marker.space_uid.0),
                endpoint: "/tmp/dmux.sock".into(),
            },
            child: Some(VerifiedConnectChild::Split {
                epoch: marker.server_epoch,
                group: ProviderHandle::Wz(4),
                split: ProviderHandle::Wz(5),
            }),
        };
        let (group, split) = frozen_connect_child_refs(&target);
        assert_eq!(group.as_deref(), Some(marker.group_ref.as_str()));
        assert_eq!(split.as_deref(), Some(marker.split_ref.as_str()));

        let query = frozen_connect_query(&target);
        assert_eq!(query.owner, target.owner);
        assert!(matches!(query.locator, OwnerLocator::Uid(uid) if uid == target.space_uid));
        assert!(matches!(
            query.child,
            Some(RequestedChild {
                kind: ChildKind::Split,
                epoch,
                handle: ProviderHandle::Wz(5),
            }) if epoch == marker.server_epoch
        ));
    }

    #[test]
    fn safe_quit_mixed_and_tmux_only_domain_plans_are_deterministic() {
        let persistent = BTreeSet::from(["dmux".to_string(), "dmux-b-ts".to_string()]);
        let mixed =
            safe_quit_domain_plan(persistent.clone(), ["dmux-b-ts".to_string()], true).unwrap();
        assert_eq!(mixed.detach, vec!["dmux-b-ts"]);
        assert_eq!(mixed.full_persistent_set, persistent);

        let tmux_only = safe_quit_domain_plan(persistent.clone(), Vec::new(), true).unwrap();
        assert!(tmux_only.detach.is_empty());
        assert_eq!(tmux_only.full_persistent_set, persistent);

        #[cfg(target_os = "macos")]
        assert_eq!(safe_quit_platform_action(), "hide");
        #[cfg(target_os = "linux")]
        assert_eq!(safe_quit_platform_action(), "quit");

        let unsafe_empty =
            safe_quit_domain_plan(BTreeSet::from(["dmux".into()]), Vec::new(), false).unwrap_err();
        assert_eq!(unsafe_empty.code, ErrorCode::BridgeUnavailable);
    }

    #[test]
    fn detach_postcheck_rejects_a_missing_configured_persistent_domain() {
        let detached = BridgeDomainState {
            state: "Detached".into(),
            has_any_panes: false,
            backend_instance_uid: None,
            pane_count: 0,
            valid_marker_pane_count: 0,
            system_pane_count: 0,
            system_workspace: None,
            system_epoch: None,
        };
        let full = BTreeSet::from(["dmux".to_string(), "dmux-b-ts".to_string()]);
        let mut heartbeat = BridgeHeartbeat {
            protocol_version: 1,
            gui_instance: "gui-test-1".into(),
            pid: std::process::id(),
            process_start_token: "test-token".into(),
            updated_at: 1,
            panes: Vec::new(),
            domains: BTreeMap::from([("dmux".into(), detached.clone())]),
        };
        assert!(!heartbeat_proves_domains_detached(
            &heartbeat,
            &["dmux".into()],
            Some(&full),
        ));
        heartbeat.domains.insert("dmux-b-ts".into(), detached);
        assert!(heartbeat_proves_domains_detached(
            &heartbeat,
            &["dmux".into()],
            Some(&full),
        ));
    }

    fn two_route_manifest(
        host_uid: HostUid,
        backend_instance_uid: BackendInstanceUid,
    ) -> Vec<GuiDomainManifestRow> {
        gui::build_domain_manifest(vec![
            RemoteDomainSource {
                name: "dmux-b-usb".into(),
                remote_address: "10.77.77.2".into(),
                username: "fredrir".into(),
                remote_wezterm_path: Some("/usr/bin/wezterm".into()),
                managed_socket: Some("/run/user/1000/dmux/wez-dmux.sock".into()),
                host_uid,
                backend_instance_uid,
                route_id: 7,
                priority: 10,
                transport: Transport::Openssh,
                network_class: crate::registry::NetworkClass::Usb,
                compatible: true,
                unavailable_reason: None,
            },
            RemoteDomainSource {
                name: "dmux-b-ts".into(),
                remote_address: "archie.tail.example".into(),
                username: "fredrir".into(),
                remote_wezterm_path: Some("/usr/bin/wezterm".into()),
                managed_socket: Some("/run/user/1000/dmux/wez-dmux.sock".into()),
                host_uid,
                backend_instance_uid,
                route_id: 8,
                priority: 20,
                transport: Transport::Openssh,
                network_class: crate::registry::NetworkClass::Tailscale,
                compatible: true,
                unavailable_reason: None,
            },
        ])
        .unwrap()
    }

    #[test]
    fn freshly_proven_route_wins_over_a_stale_attached_route() {
        let host_uid = marker().host_uid;
        let backend_instance_uid =
            BackendInstanceUid(Uuid::parse_str("44444444-4444-4444-8444-444444444444").unwrap());
        let rows = two_route_manifest(host_uid, backend_instance_uid);
        let candidates: Vec<_> = rows.iter().collect();

        // Acceptance case 20: the cable is gone, the owner handshake completed
        // over Tailscale, and the GUI has not yet noticed that its USB client
        // transport is dead. Presenting through the still-`Attached` USB row
        // would dial a corpse; the freshly proven route is the only one with
        // live evidence.
        let selected = choose_compatible_presentation_row(
            "dmux-b-ts",
            host_uid,
            backend_instance_uid,
            &candidates,
        )
        .unwrap();
        assert_eq!(selected.name, "dmux-b-ts");

        // Two routes to one backend instance is §8.4's design, so both being
        // attached selects the fresh route instead of raising a conflict; the
        // bridge detaches the stale alternate before attaching it.
        let selected = choose_compatible_presentation_row(
            "dmux-b-usb",
            host_uid,
            backend_instance_uid,
            &candidates,
        )
        .unwrap();
        assert_eq!(selected.name, "dmux-b-usb");

        // With no fresh route among the candidates the §8.4 class order
        // decides: USB, then Tailscale, then anything else enrolled.
        let selected = choose_compatible_presentation_row(
            "dmux-b-gone",
            host_uid,
            backend_instance_uid,
            &candidates,
        )
        .unwrap();
        assert_eq!(selected.name, "dmux-b-usb");
        let tailscale_first: Vec<_> = vec![&rows[1], &rows[0]];
        let selected = choose_compatible_presentation_row(
            "dmux-b-gone",
            host_uid,
            backend_instance_uid,
            &tailscale_first,
        )
        .unwrap();
        assert_eq!(selected.name, "dmux-b-usb");
    }

    /// Every candidate is checked against the caller's validated identity,
    /// not only against the other candidates (report 06 row 15; ADR 012
    /// §3.5): a manifest whose rows all agree with each other about the
    /// wrong instance is still refused.
    #[test]
    fn presentation_routes_must_name_the_validated_authority() {
        let host_uid = marker().host_uid;
        let backend_instance_uid =
            BackendInstanceUid(Uuid::parse_str("44444444-4444-4444-8444-444444444444").unwrap());
        let rows = two_route_manifest(host_uid, backend_instance_uid);
        let candidates: Vec<_> = rows.iter().collect();

        // Both rows agree with each other; neither names the validated
        // backend instance. The old candidates-only check accepted this.
        let other_instance =
            BackendInstanceUid(Uuid::parse_str("66666666-6666-4666-8666-666666666666").unwrap());
        let error =
            choose_compatible_presentation_row("dmux-b-usb", host_uid, other_instance, &candidates)
                .unwrap_err();
        assert_eq!(error.code, ErrorCode::IdentityConflict);

        let other_host = HostUid(Uuid::parse_str("77777777-7777-4777-8777-777777777777").unwrap());
        let error = choose_compatible_presentation_row(
            "dmux-b-usb",
            other_host,
            backend_instance_uid,
            &candidates,
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::IdentityConflict);

        // One stray row for another identity among otherwise correct rows is
        // refused the same way, whichever position it occupies.
        let mut stray = rows[1].clone();
        stray.backend_instance_uid = other_instance;
        for ordered in [vec![&rows[0], &stray], vec![&stray, &rows[0]]] {
            let error = choose_compatible_presentation_row(
                "dmux-b-usb",
                host_uid,
                backend_instance_uid,
                &ordered,
            )
            .unwrap_err();
            assert_eq!(error.code, ErrorCode::IdentityConflict);
        }
        let mut stray_host = rows[1].clone();
        stray_host.host_uid = other_host;
        let error = choose_compatible_presentation_row(
            "dmux-b-usb",
            host_uid,
            backend_instance_uid,
            &[&rows[0], &stray_host],
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::IdentityConflict);

        // An incompatible row is refused with its recorded reason even when
        // it names the right identity and would otherwise win the ordering.
        let mut incompatible = rows[1].clone();
        incompatible.compatible = false;
        incompatible.unavailable_reason = Some("wez_build_mismatch".into());
        let error = choose_compatible_presentation_row(
            "dmux-b-ts",
            host_uid,
            backend_instance_uid,
            &[&rows[0], &incompatible],
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::ProviderUnavailable);
        assert_eq!(error.message, "wez_build_mismatch");

        assert_eq!(
            choose_compatible_presentation_row("dmux-b-usb", host_uid, backend_instance_uid, &[])
                .unwrap_err()
                .code,
            ErrorCode::ProviderUnavailable,
        );
    }

    #[test]
    fn creation_route_probe_never_turns_identity_or_protocol_into_headless() {
        assert_eq!(
            classify_new_route_error(TypedError::new(
                ErrorCode::RouteUnavailable,
                "usb link probe failed",
            ))
            .unwrap(),
            WezRouteProbe::TransportFailed,
        );
        assert_eq!(
            classify_new_route_error(TypedError::new(
                ErrorCode::VersionMismatch,
                "owner build differs",
            ))
            .unwrap(),
            WezRouteProbe::AuthOrCompatFailed,
        );
        for code in [
            ErrorCode::HostIdentityChanged,
            ErrorCode::IdentityConflict,
            ErrorCode::ProtocolMismatch,
            ErrorCode::OperationFailed,
        ] {
            let error = classify_new_route_error(TypedError::new(code, "terminal")).unwrap_err();
            assert_eq!(error.code, code);
        }
    }

    #[test]
    fn new_preflight_requires_an_explicit_presentable_domain_state() {
        let detached = BridgeDomainState {
            state: "Detached".into(),
            has_any_panes: false,
            backend_instance_uid: None,
            pane_count: 0,
            valid_marker_pane_count: 0,
            system_pane_count: 0,
            system_workspace: None,
            system_epoch: None,
        };
        let transient = BridgeDomainState {
            state: "Attaching".into(),
            ..detached.clone()
        };
        assert!(
            require_preflight_domain(
                &BTreeMap::from([("dmux-b-usb".into(), detached)]),
                "dmux-b-usb",
            )
            .is_ok()
        );
        let transient = require_preflight_domain(
            &BTreeMap::from([("dmux-b-usb".into(), transient)]),
            "dmux-b-usb",
        )
        .unwrap_err();
        assert_eq!(transient.code, ErrorCode::PostconditionFailed);
        let missing = require_preflight_domain(&BTreeMap::new(), "dmux-b-usb").unwrap_err();
        assert_eq!(missing.code, ErrorCode::ProviderUnavailable);
    }

    fn provider_handle_for_test(child: &str) -> ProviderHandle {
        parse_ref(&format!("1/{child}"))
            .unwrap()
            .child
            .unwrap()
            .handle
    }

    /// DLOCK-003: the GUI tmux presentation fence must open the registry
    /// BEFORE it takes the authority gate. `Registry::open` runs
    /// `ensure_schema`, which needs the gate *exclusively* whenever a
    /// migration is pending; the gate is an OFD lock, so a shared hold taken
    /// by this very thread can never be upgraded and `ensure_schema` refuses
    /// with `SchemaMaintenanceBlocked` (`OperationInProgress`).
    #[test]
    fn tmux_presentation_fence_completes_with_a_schema_migration_pending() {
        use crate::registry::schema;

        let data = tempfile::tempdir().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let db_path = data.path().join("registry.sqlite3");
        let lock_dir = runtime.path().to_path_buf();

        // Freeze the database one schema version behind this binary, so the
        // next open *must* migrate. Expressed against `SCHEMA_VERSION` rather
        // than a literal so the next schema bump keeps exercising this.
        {
            let mut conn = rusqlite::Connection::open(&db_path).unwrap();
            schema::apply_connection_settings(&conn, std::time::Duration::from_secs(5)).unwrap();
            schema::migrate_to(&mut conn, schema::SCHEMA_VERSION - 1).unwrap();
            assert_eq!(
                schema::user_version(&conn).unwrap(),
                schema::SCHEMA_VERSION - 1,
                "the fixture must leave a migration pending"
            );
        }

        let instance = BackendInstanceUid(Uuid::new_v4());
        let (fence_db, fence_locks) = (db_path.clone(), lock_dir.clone());
        let runtime_dir = runtime.path().to_path_buf();
        let state_dir = state.path().to_path_buf();

        // A regression either refuses with `OperationInProgress` or, with the
        // `ensure_schema` guard removed, blocks forever on this process's own
        // gate description — so the call runs under a hard deadline.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let gui = ProductionGuiAuthority::with_dependencies(
                OperationEnv {
                    db_path: fence_db.clone(),
                    lock_dir: fence_locks.clone(),
                },
                runtime_dir,
                state_dir,
                "/bin/false".into(),
                "/dev/null".into(),
                PathBuf::from("/dev/null"),
                "/bin/false".into(),
                DirectInvoker,
            );
            let outcome = gui
                .local_tmux_client_read_fence(instance, LockMode::Shared)
                .map(|(_registry, locks)| {
                    let scopes: Vec<String> = locks
                        .held_scopes()
                        .iter()
                        .map(|scope| scope.key())
                        .collect();
                    // The registries opened deeper inside the fenced work
                    // (`frozen_binding_for_authority`, `local_provider`) find
                    // the schema already current and never touch the gate.
                    let nested =
                        Registry::open(RegistryConfig::new(&fence_db, &fence_locks)).is_ok();
                    (scopes, nested)
                });
            let _ = tx.send(outcome);
        });
        let outcome = rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("the tmux presentation read fence did not complete within 30s");
        let (scopes, nested) = match outcome {
            Ok(value) => value,
            Err(error) => {
                panic!("a pending migration must not refuse the presentation fence: {error:?}")
            }
        };
        assert_eq!(
            scopes,
            vec![
                "authority-gate".to_string(),
                format!("backend:{}", instance.0)
            ],
            "the fence must still hold the gate and the backend-instance lock it protects reads with"
        );
        assert!(
            nested,
            "a registry opened under the fence must find the schema current"
        );
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        assert_eq!(
            schema::user_version(&conn).unwrap(),
            schema::SCHEMA_VERSION,
            "the fence must have migrated the registry on its way in"
        );
    }

    /// A GUI authority over a scratch registry and lock directory. The
    /// wezterm/helper binaries are `/bin/false` because nothing under test
    /// may spawn them: `local_opposite_create_target` constructs a provider
    /// and never invokes one.
    struct ScratchGui {
        _data: tempfile::TempDir,
        _runtime: tempfile::TempDir,
        _state: tempfile::TempDir,
        db_path: PathBuf,
        lock_dir: PathBuf,
        gui: ProductionGuiAuthority<DirectInvoker>,
    }

    impl ScratchGui {
        fn new() -> Self {
            let data = tempfile::tempdir().unwrap();
            let runtime = tempfile::tempdir().unwrap();
            let state = tempfile::tempdir().unwrap();
            let db_path = data.path().join("registry.sqlite3");
            let lock_dir = runtime.path().join("locks");
            let gui = ProductionGuiAuthority::with_dependencies(
                OperationEnv {
                    db_path: db_path.clone(),
                    lock_dir: lock_dir.clone(),
                },
                runtime.path().to_path_buf(),
                state.path().to_path_buf(),
                "/bin/false".into(),
                "/dev/null".into(),
                PathBuf::from("/dev/null"),
                "/bin/false".into(),
                DirectInvoker,
            );
            Self {
                _data: data,
                _runtime: runtime,
                _state: state,
                db_path,
                lock_dir,
                gui,
            }
        }

        fn registry(&self) -> Registry {
            Registry::open(RegistryConfig::new(&self.db_path, &self.lock_dir)).unwrap()
        }
    }

    fn opposite_of(backend: Backend) -> Backend {
        match backend {
            Backend::Wez => Backend::Tmux,
            Backend::Tmux => Backend::Wez,
        }
    }

    fn scratch_endpoint(backend: Backend) -> &'static str {
        match backend {
            Backend::Tmux => "dmux-scratch-opposite",
            Backend::Wez => "/tmp/dmux-scratch-opposite.sock",
        }
    }

    /// Review finding #15 (ADR 012 WS-A.5, `gui_cli.rs` site): the GUI
    /// create's opposite-backend collision fence used to scan a registered
    /// opposite instance whose `server_epoch` was NULL under an unmanaged
    /// scope, and `scan_epoch_for_create` waved whatever server answered
    /// through. Unpublished now means refuse — `backend_epoch_changed`, before
    /// the fenced create is entered, so no Space row and no journal entry can
    /// exist afterwards (§8.2 steps 3–8; cases 5–7).
    #[test]
    fn gui_create_refuses_an_unpublished_opposite_instance_before_any_reservation() {
        use crate::backend::scope::ManagedTarget;

        for selected in [Backend::Wez, Backend::Tmux] {
            let opposite = opposite_of(selected);
            let scratch = ScratchGui::new();
            let instance = scratch
                .registry()
                .register_backend_instance(opposite, Some(scratch_endpoint(opposite)), None)
                .unwrap();
            // Registered and addressable, but nobody has published a server
            // incarnation for it: the row exists before the mux coordinates.
            assert_eq!(
                scratch
                    .registry()
                    .backend_server(instance)
                    .unwrap()
                    .server_epoch,
                None
            );

            let error = scratch
                .gui
                .local_opposite_create_target(selected)
                .err()
                .unwrap_or_else(|| {
                    panic!("{selected}: an unpublished opposite {opposite} instance must refuse")
                });
            assert_eq!(
                error.code,
                ErrorCode::BackendEpochChanged,
                "{selected}: {error:?}"
            );
            assert_eq!(
                error.message,
                ManagedTarget::unpublished_detail(opposite, instance),
                "{selected}: the refusal is the shared unpublished text"
            );

            // Refused before `create_space_owner_fenced`: nothing was
            // reserved, journaled, or created.
            let registry = scratch.registry();
            assert!(
                registry.spaces().unwrap().is_empty(),
                "{selected}: the refusal must consume no Space identity"
            );
            assert!(
                registry.unfinished_operations().unwrap().is_empty(),
                "{selected}: the refusal must journal nothing"
            );
        }
    }

    /// Positive control for the test above: once the opposite instance has
    /// published its epoch the target is built, pinned to exactly that
    /// epoch and endpoint, and the borrowed view the fenced create consumes
    /// carries the same pin.
    #[test]
    fn gui_create_pins_a_published_opposite_instance_to_its_registry_epoch() {
        for selected in [Backend::Wez, Backend::Tmux] {
            let opposite = opposite_of(selected);
            let endpoint = scratch_endpoint(opposite);
            let scratch = ScratchGui::new();
            let mut registry = scratch.registry();
            let instance = registry
                .register_backend_instance(opposite, Some(endpoint), None)
                .unwrap();
            let epoch = ServerEpoch(Uuid::new_v4());
            registry
                .publish_backend_server(instance, epoch, Some(4242), Some("tok"), None, None)
                .unwrap();
            drop(registry);

            let target = scratch
                .gui
                .local_opposite_create_target(selected)
                .unwrap_or_else(|error| panic!("{selected}: {error:?}"))
                .unwrap_or_else(|| {
                    panic!("{selected}: a published opposite {opposite} instance is a target")
                });
            assert_eq!(target.backend, opposite, "{selected}");
            assert_eq!(target.instance, instance, "{selected}");
            assert_eq!(
                target.scope,
                InventoryScope::managed(opposite, endpoint, epoch),
                "{selected}"
            );
            let borrowed = target.borrowed();
            assert_eq!(borrowed.backend, opposite, "{selected}");
            assert_eq!(borrowed.instance, instance, "{selected}");
            assert_eq!(borrowed.scope.expected_epoch(), Some(epoch), "{selected}");
            assert_eq!(borrowed.scope.endpoint, endpoint, "{selected}");
        }
    }

    /// `Unregistered` keeps its meaning: no opposite instance is nothing to
    /// collide with, and the selected backend's own instance is never
    /// mistaken for an opposite one.
    #[test]
    fn gui_create_has_no_opposite_target_when_that_backend_is_unregistered() {
        for selected in [Backend::Wez, Backend::Tmux] {
            let scratch = ScratchGui::new();
            let mut registry = scratch.registry();
            let own = registry
                .register_backend_instance(selected, Some(scratch_endpoint(selected)), None)
                .unwrap();
            registry
                .publish_backend_server(own, ServerEpoch(Uuid::new_v4()), None, None, None, None)
                .unwrap();
            drop(registry);

            let target = scratch
                .gui
                .local_opposite_create_target(selected)
                .unwrap_or_else(|error| panic!("{selected}: {error:?}"));
            assert!(
                target.is_none(),
                "{selected}: only the selected backend is registered, so there is no opposite"
            );
        }
    }

    /// `Unaddressable` keeps the file's existing refusal: a registered
    /// opposite instance with no recorded endpoint is `provider_unavailable`,
    /// not an epoch fault and not a scan.
    #[test]
    fn gui_create_refuses_an_opposite_instance_with_no_recorded_endpoint() {
        let scratch = ScratchGui::new();
        scratch
            .registry()
            .register_backend_instance(Backend::Tmux, None, None)
            .unwrap();
        let error = scratch
            .gui
            .local_opposite_create_target(Backend::Wez)
            .err()
            .expect("an opposite instance without an endpoint must refuse");
        assert_eq!(error.code, ErrorCode::ProviderUnavailable, "{error:?}");
        assert_eq!(
            error.message,
            "registered opposite tmux backend has no inventory endpoint"
        );
    }
}
