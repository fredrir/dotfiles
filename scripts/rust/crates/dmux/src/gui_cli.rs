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
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::backend::{InventoryOutcome, InventoryScope, Provider, SplitDirection};
use crate::bootstrap::MarkerContext;
use crate::connect_cli::{
    FrozenBinding, FrozenConnectTarget, OwnerConnectQuery, OwnerLocator, PresentationMode,
    PresentationReceipt, RequestedChild, VerifiedConnectChild,
};
use crate::error::{ErrorCode, TypedError};
use crate::gui::{
    self, BridgeDomainState, BridgeHeartbeat, BridgeInstanceSelection, BridgeSelection,
    GuiCliOrigin, GuiDomainManifestRow, GuiSpaceRow, GuiStatusCache, GuiStatusDisplay,
    RemoteDomainSource,
};
use crate::history::History;
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
    ChildRefShape, HostToken, ParsedRef, SpaceRefShape, canonical_uri, child_suffix, parse_ref,
    validate_new_name,
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
    SplitResizeResult, SplitRmPayload, SplitZoomPayload, SplitZoomResult,
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
    Spaces,
    /// Present one existing Space; never creates on a miss.
    Present {
        #[arg(long)]
        space: String,
    },
    /// Create a Space on the active marker's exact owner/backend.
    SpaceNew {
        #[arg(long)]
        name: String,
        #[arg(long)]
        dir: Option<String>,
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
        gui::parse_origin_json(raw)
            .map_err(typed_gui)
            .and_then(|origin| authority.bind_origin(&origin))
            .and_then(|bound| authority.execute_bound(&bound, command))
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

enum SpaceMatcher {
    Uid(SpaceUid),
    Number(SpaceNo),
    Name(String),
}

impl SpaceMatcher {
    fn matches(&self, candidate: &AuthorityMarker) -> bool {
        self.matches_fields(
            candidate.marker.space_uid,
            candidate.marker.space_no,
            &candidate.logical_name,
        )
    }

    fn matches_fields(&self, space_uid: SpaceUid, space_no: SpaceNo, logical_name: &str) -> bool {
        match self {
            SpaceMatcher::Uid(uid) => space_uid == *uid,
            SpaceMatcher::Number(no) => space_no == *no,
            SpaceMatcher::Name(name) => logical_name == name,
        }
    }
}

fn connect_locator_matches(locator: &OwnerLocator, candidate: &AuthorityMarker) -> bool {
    match locator {
        OwnerLocator::Uid(uid) => candidate.marker.space_uid == *uid,
        OwnerLocator::Number(no) => candidate.marker.space_no == *no,
        OwnerLocator::Name(name) => candidate.logical_name == *name,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DomainAuthority {
    host_uid: HostUid,
    backend_instance: BackendInstanceUid,
    server_epoch: ServerEpoch,
}

#[derive(Debug, Clone)]
struct SummonTarget {
    authority: AuthorityMarker,
    domain: String,
    alternate_domains: Vec<String>,
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
    must_hide: bool,
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
        must_hide: contains_tmux,
    })
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

fn choose_compatible_presentation_row<'a>(
    configured_domains: Option<&BTreeMap<String, BridgeDomainState>>,
    fresh_route: &str,
    candidates: &[&'a GuiDomainManifestRow],
) -> Result<&'a GuiDomainManifestRow, TypedError> {
    if candidates.is_empty() {
        return Err(unavailable(
            "no exact-build compatible GUI route exists for this remote Wez instance",
        ));
    }
    let attached: Vec<_> = candidates
        .iter()
        .filter(|row| {
            configured_domains
                .and_then(|domains| domains.get(&row.name))
                .is_some_and(|state| state.state == "Attached")
        })
        .copied()
        .collect();
    match attached.as_slice() {
        [attached] => Ok(*attached),
        [] => Ok(candidates
            .iter()
            .find(|row| row.name == fresh_route)
            .copied()
            .unwrap_or(candidates[0])),
        _ => Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "more than one compatible route is already attached for the target backend instance",
        )),
    }
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
        let authoritative =
            self.validate_authority_marker_in_domain(&ambient, Some(&selection.domain))?;
        let candidate = GuiCliOrigin {
            protocol_version: gui::BRIDGE_PROTOCOL_VERSION,
            gui_instance: selection.gui_instance,
            pane_id: selection.pane_id,
            domain: selection.domain,
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
            Backend::Tmux => info
                .socket_path
                .ok_or_else(|| unavailable("managed tmux backend has no recorded namespace"))?,
            Backend::Wez => {
                let descriptor = crate::runtime::read_wez_descriptor_in(&self.runtime_dir)
                    .map_err(|error| unavailable(format!("managed Wez descriptor: {error}")))?
                    .ok_or_else(|| unavailable("managed Wez descriptor is absent"))?;
                descriptor
                    .require_ready()
                    .map_err(|error| unavailable(format!("managed Wez descriptor: {error}")))?;
                let descriptor_instance = descriptor
                    .backend_instance_uid
                    .as_deref()
                    .and_then(|value| Uuid::parse_str(value).ok())
                    .map(BackendInstanceUid);
                let descriptor_epoch = Uuid::parse_str(&descriptor.epoch).ok().map(ServerEpoch);
                if descriptor_instance != Some(instance) {
                    return Err(TypedError::new(
                        ErrorCode::WrongBackendInstance,
                        "managed Wez descriptor names a different backend instance",
                    ));
                }
                if descriptor_epoch != Some(epoch) {
                    return Err(TypedError::new(
                        ErrorCode::BackendEpochChanged,
                        "managed Wez descriptor names a different server epoch",
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
        Ok((
            provider,
            InventoryScope {
                backend,
                endpoint,
                expected_epoch: Some(epoch),
            },
        ))
    }

    /// Resolve the registered opposite backend without allocating an
    /// instance. `None` remains only a hint: the owner-fenced operation
    /// re-queries the registry while holding the exact-name decision lock
    /// and accepts it only when the opposite instance is still absent.
    fn local_opposite_create_target(
        &self,
        selected: Backend,
    ) -> Result<Option<OwnedCreateTarget>, TypedError> {
        let opposite = match selected {
            Backend::Wez => Backend::Tmux,
            Backend::Tmux => Backend::Wez,
        };
        let registry = self.registry()?;
        let Some(instance) = registry
            .backend_instance_for_backend(opposite)
            .map_err(typed_registry)?
        else {
            return Ok(None);
        };
        let info = registry
            .backend_instance_info(instance)
            .map_err(typed_registry)?;
        if info.backend != opposite {
            return Err(TypedError::new(
                ErrorCode::WrongBackendInstance,
                "opposite backend instance changed kind during GUI create preflight",
            ));
        }
        let endpoint = info.socket_path.ok_or_else(|| {
            unavailable(format!(
                "registered opposite {opposite} backend has no inventory endpoint"
            ))
        })?;
        let expected_epoch = registry
            .backend_server(instance)
            .map_err(typed_registry)?
            .server_epoch;
        let provider: Box<dyn Provider> = match opposite {
            Backend::Tmux => Box::new(crate::backend::tmux::TmuxProvider::new(endpoint.clone())),
            Backend::Wez => Box::new(crate::backend::wez::WezProvider::new(
                &self.wezterm_bin,
                self.wezterm_config.clone(),
            )),
        };
        Ok(Some(OwnedCreateTarget {
            backend: opposite,
            instance,
            provider,
            scope: InventoryScope {
                backend: opposite,
                endpoint,
                expected_epoch,
            },
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
            self.validate_remote_marker(marker, gui_domain)
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
            Some(heartbeat_domains),
            &authority.fresh_route,
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

    fn gui_space_rows(&self, heartbeat: &BridgeHeartbeat) -> Result<Vec<GuiSpaceRow>, TypedError> {
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
                if space.marker.backend != Backend::Wez {
                    return false;
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
        gui::validate_space_rows(&rows).map_err(typed_gui)?;
        Ok(rows)
    }

    fn resolve_host_token(
        &self,
        token: Option<&HostToken>,
        default: HostUid,
    ) -> Result<HostUid, TypedError> {
        let Some(token) = token else {
            return Ok(default);
        };
        match token {
            HostToken::Uid(uid) => {
                self.enrolled_host(*uid)?;
                Ok(*uid)
            }
            HostToken::AliasOrLabel(spelling) => {
                let matches: Vec<_> = self
                    .registry()?
                    .hosts()
                    .map_err(typed_registry)?
                    .into_iter()
                    .filter(|host| {
                        host.lifecycle == HostLifecycle::Enrolled
                            && (host.alias.as_deref() == Some(spelling)
                                || host.label.as_deref() == Some(spelling))
                    })
                    .collect();
                match matches.as_slice() {
                    [host] => Ok(host.host_uid),
                    [] => Err(TypedError::new(
                        ErrorCode::NotFound,
                        format!("no enrolled host {spelling:?}"),
                    )),
                    _ => Err(TypedError::new(
                        ErrorCode::AmbiguousTarget,
                        format!("host spelling {spelling:?} is ambiguous"),
                    )),
                }
            }
        }
    }

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
        let (host_uid, matcher) = match parsed.space {
            SpaceRefShape::Canonical { host, space } => (host, SpaceMatcher::Uid(space)),
            SpaceRefShape::Numbered { host, no } => {
                let owner = self.resolve_host_token(host.as_ref(), default_host)?;
                (owner, SpaceMatcher::Number(no))
            }
            SpaceRefShape::Named { host, name } => {
                let owner = self.resolve_host_token(host.as_ref(), default_host)?;
                (owner, SpaceMatcher::Name(name))
            }
        };
        let host = self.enrolled_host(host_uid)?;
        let identity = self.registry()?.identity().map_err(typed_registry)?;
        let owner_spaces = if host_uid == identity.host_uid {
            self.local_space_markers()?
        } else {
            // An explicit host-qualified lookup must surface that owner's
            // route/authority failure. It must not be converted to a false
            // local-looking NotFound by the best-effort all-host picker.
            self.remote_space_markers(&host)?
        };
        let mut candidates: Vec<_> = owner_spaces
            .into_iter()
            .filter(|candidate| candidate.marker.host_uid == host_uid && matcher.matches(candidate))
            .collect();
        match candidates.len() {
            0 => Err(TypedError::new(
                ErrorCode::NotFound,
                format!("no live Space matches {reference:?}"),
            )),
            1 => Ok(candidates.remove(0)),
            _ => Err(TypedError::new(
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
        let (host_uid, matcher) = match parsed.space {
            SpaceRefShape::Canonical { host, space } => (host, SpaceMatcher::Uid(space)),
            SpaceRefShape::Numbered { host, no } => (
                self.resolve_host_token(host.as_ref(), default_host)?,
                SpaceMatcher::Number(no),
            ),
            SpaceRefShape::Named { host, name } => (
                self.resolve_host_token(host.as_ref(), default_host)?,
                SpaceMatcher::Name(name),
            ),
        };
        self.enrolled_host(host_uid)?;

        let registry = self.registry()?;
        let identity = registry.identity().map_err(typed_registry)?;
        let mut candidates = Vec::new();
        if host_uid == identity.host_uid {
            for row in registry
                .spaces()
                .map_err(typed_registry)?
                .into_iter()
                .filter(|row| row.lifecycle == Lifecycle::Active)
            {
                if !matcher.matches_fields(row.space_uid, row.space_no, &row.logical_name) {
                    continue;
                }
                let backend = registry
                    .backend_instance_info(row.backend_instance)
                    .map_err(typed_registry)?
                    .backend;
                candidates.push(ColdSpaceIdentity {
                    host_uid,
                    space_uid: row.space_uid,
                    backend,
                });
            }
        } else {
            // `spaces` is an owner read through identity/lineage-validated
            // routing. It performs no local service or GUI mutation.
            let spaces = self.remote_spaces(host_uid)?;
            for space in spaces
                .spaces
                .into_iter()
                .filter(|space| space.lifecycle == Lifecycle::Active)
            {
                let Some(space_no) = std::num::NonZeroU64::new(space.space_no).map(SpaceNo) else {
                    return Err(TypedError::new(
                        ErrorCode::ProtocolMismatch,
                        "owner returned SpaceNo zero during cold target preflight",
                    ));
                };
                if matcher.matches_fields(space.space_uid, space_no, &space.name) {
                    candidates.push(ColdSpaceIdentity {
                        host_uid,
                        space_uid: space.space_uid,
                        backend: space.backend,
                    });
                }
            }
        }

        match candidates.as_slice() {
            [target] => Ok(*target),
            [] => Err(TypedError::new(
                ErrorCode::NotFound,
                format!("no durable active Space matches {reference:?}"),
            )),
            _ => Err(TypedError::new(
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
                let locator_match = match &query.locator {
                    OwnerLocator::Uid(uid) => row.space_uid == *uid,
                    OwnerLocator::Number(no) => row.space_no == *no,
                    OwnerLocator::Name(name) => row.logical_name == *name,
                };
                if locator_match {
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
        // Reuse an already-attached compatible route first. Attaching the
        // same backend instance through a second route would transiently
        // duplicate it and the Lua bridge correctly refuses that state. If
        // none is attached, prefer the fresh owner-validated marker route,
        // then the manifest's deterministic compatible order.
        let selected = choose_compatible_presentation_row(
            Some(&heartbeat.domains),
            &target.route,
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
                    configured_domains,
                    &authority.route,
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

    fn cold_bridge_present(
        &self,
        instance: &BridgeInstanceSelection,
        target: &SummonTarget,
        launcher_request_uid: Uuid,
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
        let origin = gui::cold_launcher_origin(
            instance.gui_instance.clone(),
            u64::from(unsafe { libc::geteuid() }),
            u64::from(std::process::id()),
            crate::registry::process_start_token(),
            launcher_request_uid,
            target.domain.clone(),
            target.authority.backend_instance,
        )
        .map_err(typed_gui)?;
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
                let instance = match live.as_slice() {
                    [instance] => instance.clone(),
                    [] => {
                        crate::gui_lifecycle::launch_attach_only_gui(
                            &self.runtime_dir,
                            &ready,
                            &self.wezterm_bin,
                            &self.gui_config,
                            Uuid::new_v4(),
                        )?
                        .instance
                    }
                    many => {
                        return Err(TypedError::new(
                            ErrorCode::IdentityConflict,
                            format!(
                                "{} live GUI instances exist; new presentation preflight refuses to guess",
                                many.len()
                            ),
                        ));
                    }
                };

                if local_owner {
                    require_preflight_domain(&instance.domains, LOCAL_WEZ_DOMAIN)?;
                    Ok(WezPresentationPreflight {
                        owner,
                        backend_instance_uid: ready.backend_instance_uid,
                        server_epoch: ready.server_epoch,
                        gui_instance: instance.gui_instance,
                        domain: LOCAL_WEZ_DOMAIN.to_string(),
                        alternate_domains: Vec::new(),
                        mode,
                    })
                } else {
                    let remote = self.remote_wez_preflight(owner)?;
                    let before = remote_before.expect("remote owner preflight was established");
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
                        gui_instance: instance.gui_instance,
                        domain,
                        alternate_domains,
                        mode,
                    })
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
        let (instance, launched_gui) = match live.as_slice() {
            [instance] => (instance.clone(), false),
            [] => {
                // Refuse an absent/ambiguous target before opening a GUI.
                // The same target is reselected against the launched
                // instance's exact configured domain set below.
                self.choose_summon_target(self.summon_targets(&ready, &manifest, None)?)?;
                let launched = crate::gui_lifecycle::launch_attach_only_gui(
                    &self.runtime_dir,
                    &ready,
                    &self.wezterm_bin,
                    &self.gui_config,
                    launcher_request_uid,
                )?;
                (launched.instance, true)
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
        let ack =
            match self.cold_bridge_present(&instance, &target, launcher_request_uid, None, None) {
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

    fn cold_present_explicit(&mut self, reference: &str) -> Result<Value, TypedError> {
        self.partial_result = None;
        let registry = self.registry()?;
        let identity = registry.identity().map_err(typed_registry)?;
        let (preflight, ready) = enter_cold_wez_lifecycle(
            self.resolve_cold_space_identity(reference, identity.host_uid),
            |_| {
                crate::gui_lifecycle::ensure_ready_wez_service(
                    &registry,
                    &self.runtime_dir,
                    &self.wezterm_bin,
                    &self.wezterm_config,
                )
            },
        )?;

        // Service readiness may advance a stopped local Wez incarnation.
        // Re-resolve the exact durable identity through complete live owner
        // authority and require that it is still the same Wez Space.
        let authority = self.resolve_space(
            &canonical_uri(preflight.host_uid, preflight.space_uid),
            preflight.host_uid,
        )?;
        if authority.marker.host_uid != preflight.host_uid
            || authority.marker.space_uid != preflight.space_uid
            || authority.marker.backend != preflight.backend
            || authority.marker.backend != Backend::Wez
        {
            return Err(TypedError::new(
                ErrorCode::IdentityConflict,
                "cold target identity/backend changed between durable preflight and live revalidation",
            ));
        }
        let manifest = self.remote_domain_manifest()?;
        self.summon_target_for_authority(&authority, &ready, &manifest, None)?
            .ok_or_else(|| {
                unavailable(
                    "the exact Wez target has no owner-validated compatible GUI domain route",
                )
            })?;

        let heartbeat_source = crate::gui_lifecycle::RuntimeHeartbeatSource;
        let live = crate::gui_lifecycle::HeartbeatSource::live_instances(
            &heartbeat_source,
            &self.runtime_dir,
        )?;
        let launcher_request_uid = Uuid::new_v4();
        let (instance, launched_gui) = match live.as_slice() {
            [instance] => (instance.clone(), false),
            [] => {
                let launched = crate::gui_lifecycle::launch_attach_only_gui(
                    &self.runtime_dir,
                    &ready,
                    &self.wezterm_bin,
                    &self.gui_config,
                    launcher_request_uid,
                )?;
                (launched.instance, true)
            }
            many => {
                return Err(TypedError::new(
                    ErrorCode::IdentityConflict,
                    format!(
                        "{} live GUI instances exist; --launch-gui refuses to guess",
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
                    "target domain rebinding",
                    unavailable(
                        "the exact Wez target domain is absent/incompatible in the selected GUI instance",
                    ),
                ));
            }
            None => {
                return Err(unavailable(
                    "the exact Wez target domain is absent/incompatible in the selected GUI instance",
                ));
            }
        };
        let ack =
            match self.cold_bridge_present(&instance, &target, launcher_request_uid, None, None) {
                Ok(ack) => ack,
                Err(error) if launched_gui => {
                    return Err(self.launched_gui_partial("explicit presentation", error));
                }
                Err(error) => return Err(error),
            };
        Ok(serde_json::json!({
            "launched_gui": launched_gui,
            "presented": ack,
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
        self.summon_target_for_authority(&authority, &ready, &manifest, None)?
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
            [instance] => (instance.clone(), false),
            [] => {
                let launched = crate::gui_lifecycle::launch_attach_only_gui(
                    &self.runtime_dir,
                    &ready,
                    &self.wezterm_bin,
                    &self.gui_config,
                    launcher_request_uid,
                )?;
                (launched.instance, true)
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
            launcher_request_uid,
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
        let origin = gui::in_gui_origin(&bound.selection, &bound.authority.marker);
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

    fn bridge_detach_domain(
        &self,
        bound: &BoundGuiOrigin,
        domain: &str,
        instance: BackendInstanceUid,
        epoch: ServerEpoch,
    ) -> Result<Value, TypedError> {
        let origin = gui::in_gui_origin(&bound.selection, &bound.authority.marker);
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
        bound: &BoundGuiOrigin,
        domains: &[String],
        all_persistent_domains: Option<&BTreeSet<String>>,
    ) -> Result<BridgeHeartbeat, TypedError> {
        let started = Instant::now();
        loop {
            let observation = match gui::read_instance_heartbeat(
                &self.runtime_dir,
                &bound.selection.gui_instance,
            ) {
                Ok(heartbeat) => {
                    if heartbeat.pid != bound.selection.pid
                        || heartbeat.process_start_token != bound.selection.process_start_token
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
                        bound.selection.gui_instance
                    )
                }
                Err(error) => error.to_string(),
            };
            if started.elapsed() >= gui::ACK_TIMEOUT {
                return Err(TypedError::new(
                    ErrorCode::PostconditionFailed,
                    format!(
                        "post-detach heartbeat proof timed out for GUI {}: {observation}",
                        bound.selection.gui_instance
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
    ) -> Result<Value, TypedError> {
        validate_new_name(name).map_err(|error| {
            TypedError::new(
                ErrorCode::InvalidName,
                format!("invalid new Space name {name:?}: {error:?}"),
            )
        })?;
        // `_gui space-new` has no no-connect mode. Refuse a backend whose
        // presentation needs a PTY handoff before performing the mutation.
        if bound.authority.marker.backend != Backend::Wez {
            return Err(unavailable(
                "creating a tmux Space from the GUI cannot safely hand off its PTY; use dmux new from a terminal",
            ));
        }
        // Compatibility/route selection is a precondition, not a
        // post-mutation presentation attempt.
        let presentation = self.presentation_domain(&bound.heartbeat, &bound.authority)?;
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
                let opposite = self.local_opposite_create_target(Backend::Wez)?;
                operations::create_space_owner_fenced(
                    &self.env,
                    OwnerCreateTarget {
                        backend: Backend::Wez,
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
                        backend: Backend::Wez,
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
                self.bridge_present_selected(
                    bound,
                    &target,
                    Some(&created.group_ref),
                    Some(&created.split_ref),
                    Some(presentation),
                )
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
        if bound.authority.marker.backend == Backend::Wez
            && let Some(split_ref) = split_ref.as_deref()
        {
            self.bridge_present(bound, &bound.authority, Some(&group_ref), Some(split_ref))?;
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
        Ok(serde_json::json!({
            "split_ref": bound.authority.marker.split_ref,
            "removed": true,
        }))
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

        if domain == LOCAL_WEZ_DOMAIN {
            let registry = self.registry()?;
            let identity = registry.identity().map_err(typed_registry)?;
            let descriptor = crate::runtime::read_wez_descriptor_in(&self.runtime_dir)
                .map_err(|error| unavailable(format!("managed Wez descriptor: {error}")))?
                .ok_or_else(|| unavailable("managed Wez descriptor is absent"))?;
            descriptor
                .require_ready()
                .map_err(|error| unavailable(format!("managed Wez descriptor: {error}")))?;
            let instance = descriptor
                .backend_instance_uid
                .as_deref()
                .and_then(|value| Uuid::parse_str(value).ok())
                .map(BackendInstanceUid)
                .ok_or_else(|| {
                    TypedError::new(
                        ErrorCode::WrongBackendInstance,
                        "managed Wez descriptor omitted its backend instance",
                    )
                })?;
            let epoch = Uuid::parse_str(&descriptor.epoch)
                .map(ServerEpoch)
                .map_err(|error| {
                    TypedError::new(
                        ErrorCode::BackendEpochChanged,
                        format!("managed Wez descriptor epoch: {error}"),
                    )
                })?;
            let info = registry
                .backend_instance_info(instance)
                .map_err(typed_registry)?;
            let server = registry.backend_server(instance).map_err(typed_registry)?;
            if info.backend != Backend::Wez
                || info.owner != identity.host_uid
                || server.server_epoch != Some(epoch)
            {
                return Err(TypedError::new(
                    ErrorCode::WrongBackendInstance,
                    "local GUI domain descriptor differs from its registered Wez authority",
                ));
            }
            let (provider, scope) = self.local_provider(instance, Backend::Wez, epoch)?;
            match provider.inventory(&scope) {
                InventoryOutcome::Complete(inventory) if inventory.server_epoch == Some(epoch) => {}
                outcome => {
                    return Err(unavailable(format!(
                        "local GUI domain sentinel proof was not complete at epoch {}: {outcome:?}",
                        epoch.0
                    )));
                }
            }
            return Ok(DomainAuthority {
                host_uid: identity.host_uid,
                backend_instance: instance,
                server_epoch: epoch,
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
        Ok(DomainAuthority {
            host_uid: row.host_uid,
            backend_instance: row.backend_instance_uid,
            server_epoch: epoch,
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
        for before in snapshot {
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
        }
        Ok(())
    }

    fn disconnect(&self, bound: &BoundGuiOrigin, whole_domain: bool) -> Result<Value, TypedError> {
        if !whole_domain {
            if bound.authority.marker.backend != Backend::Wez {
                return Err(unavailable(
                    "invoking_client_unavailable: GUI tmux disconnect has no exact current-client tty/process witness",
                ));
            }
            let Some(previous) = self.history.previous(bound.authority.marker.host_uid) else {
                return Ok(serde_json::json!({
                    "nothing_else_to_present": true,
                    "hint": "use disconnect --domain to detach the current imported domain",
                }));
            };
            if previous == bound.authority.marker.space_uid {
                return Ok(serde_json::json!({
                    "nothing_else_to_present": true,
                    "hint": "use disconnect --domain to detach the current imported domain",
                }));
            }
            let target = match self.resolve_space(
                &canonical_uri(bound.authority.marker.host_uid, previous),
                bound.authority.marker.host_uid,
            ) {
                Ok(target) => target,
                Err(error) if error.code == ErrorCode::NotFound => {
                    return Ok(serde_json::json!({
                        "nothing_else_to_present": true,
                        "hint": "the previous Space is no longer attached; use disconnect --domain",
                    }));
                }
                Err(error) => return Err(error),
            };
            let attached = bound.heartbeat.panes.iter().any(|pane| {
                pane.context.host_uid == target.marker.host_uid
                    && pane.context.space_uid == target.marker.space_uid
                    && pane.context.server_epoch == target.marker.server_epoch
            });
            if !attached {
                return Ok(serde_json::json!({
                    "nothing_else_to_present": true,
                    "hint": "the previous Space is not already attached; use disconnect --domain",
                }));
            }
            let ack = self.bridge_present(bound, &target, None, None)?;
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
        self.prove_gui_domains_detached(bound, &[domain.to_string()], None)?;
        self.prove_snapshot_survived(&snapshot)?;
        Ok(serde_json::json!({
            "detached": ack,
            "surviving_splits": snapshot.len(),
        }))
    }

    fn safe_quit(&self, bound: &BoundGuiOrigin) -> Result<Value, TypedError> {
        let manifest = self.remote_domain_manifest()?;
        let mut persistent_domains = BTreeSet::from([LOCAL_WEZ_DOMAIN.to_string()]);
        persistent_domains.extend(
            manifest
                .iter()
                .filter(|row| row.compatible && row.remote_wezterm_path.is_some())
                .map(|row| row.name.clone()),
        );
        let snapshot = self.snapshot_markers(&bound.heartbeat, None)?;
        let contains_tmux = snapshot
            .iter()
            .any(|pane| pane.authority.marker.backend == Backend::Tmux);
        let mut domains = Vec::new();
        let mut domain_authorities = BTreeMap::new();
        for domain in &persistent_domains {
            let Some(state) = bound.heartbeat.domains.get(domain) else {
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
        let origin = gui::in_gui_origin(&bound.selection, &bound.authority.marker);
        let mut detach = gui::request_document(
            "safe_quit",
            serde_json::json!({ "phase": "detach", "domains": domains.clone() }),
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
            &bound.selection.gui_instance,
            &mut detach,
            gui::ACK_TIMEOUT,
        )
        .map_err(typed_gui)?;

        // The signed ack alone is not the postcondition: require a fresh
        // heartbeat from this exact GUI process to show every requested
        // domain detached and empty before proving owner survival.
        self.prove_gui_domains_detached(bound, &domains, Some(&domain_plan.full_persistent_set))?;

        // No finish request is issued unless every exact owner Split from
        // the before-snapshot remains in the same backend incarnation.
        self.prove_snapshot_survived(&snapshot)?;

        let platform_action = if domain_plan.must_hide {
            "hide"
        } else {
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
        };
        let mut finish = gui::request_document(
            "safe_quit",
            serde_json::json!({
                "phase": "finish",
                "platform_action": platform_action,
                "proof_uid": proof_uid,
            }),
            origin,
        )
        .map_err(typed_gui)?;
        gui::call_instance(
            &self.runtime_dir,
            &bound.selection.gui_instance,
            &mut finish,
            gui::ACK_TIMEOUT,
        )
        .map_err(typed_gui)
    }
}

impl<I: RouteInvoker> GuiAuthority for ProductionGuiAuthority<I> {
    type Bound = BoundGuiOrigin;

    fn bind_origin(&mut self, origin: &GuiCliOrigin) -> Result<Self::Bound, TypedError> {
        let authority =
            self.validate_authority_marker_in_domain(&origin.marker, Some(&origin.domain))?;
        let (selection, heartbeat) =
            gui::bind_cli_origin_with_heartbeat(&self.runtime_dir, origin, &authority.marker)
                .map_err(typed_gui)?;
        Ok(BoundGuiOrigin {
            origin: origin.clone(),
            selection,
            heartbeat,
            authority,
        })
    }

    fn execute_bound(
        &mut self,
        bound: &Self::Bound,
        command: &GuiCommand,
    ) -> Result<Value, TypedError> {
        self.partial_result = None;
        match command {
            GuiCommand::Context { cache } => {
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
            GuiCommand::Spaces => Ok(serde_json::json!({
                "spaces": self.gui_space_rows(&bound.heartbeat)?,
            })),
            GuiCommand::Present { space } => {
                let target = self.resolve_space(space, bound.authority.marker.host_uid)?;
                let ack = self.bridge_present(bound, &target, None, None)?;
                Ok(serde_json::json!({ "presented": ack }))
            }
            GuiCommand::SpaceNew { name, dir } => {
                self.create_space_for_origin(bound, name, dir.as_deref())
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

/// Feature-on public `dmux con TARGET --launch-gui` adapter. The target is
/// owner/live-resolved before any GUI launch, tmux is rejected with the
/// exact backend-mismatch class, and a post-launch presentation failure is
/// reported as `partial_result` rather than hidden or retried by creation.
pub fn present_cold_production(reference: &str) -> GuiResponse {
    let mut authority = match ProductionGuiAuthority::production() {
        Ok(authority) => authority,
        Err(error) => return GuiResponse::failure(error),
    };
    match authority.cold_present_explicit(reference) {
        Ok(result) => GuiResponse::success(result),
        Err(error) if error.code == ErrorCode::PartialResult => {
            match authority.take_partial_result() {
                Some(partial) => GuiResponse::failure_with_result(error, Some(partial)),
                None => GuiResponse::failure(TypedError::new(
                    ErrorCode::OperationFailed,
                    format!(
                        "cold partial-result action omitted its required result document: {}",
                        error.message
                    ),
                )),
            }
        }
        Err(error) => GuiResponse::failure(error),
    }
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

    fn origin(marker: &MarkerContext) -> GuiCliOrigin {
        GuiCliOrigin {
            protocol_version: 1,
            gui_instance: "gui-test-1".into(),
            pane_id: 51,
            domain: "dmux".into(),
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
                pane_count: 1,
                valid_marker_pane_count: 1,
                system_pane_count: 0,
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
            gui::in_gui_origin(&selection, &marker),
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
        assert!(mixed.must_hide);

        let tmux_only = safe_quit_domain_plan(persistent.clone(), Vec::new(), true).unwrap();
        assert!(tmux_only.detach.is_empty());
        assert_eq!(tmux_only.full_persistent_set, persistent);
        assert!(tmux_only.must_hide);

        let unsafe_empty =
            safe_quit_domain_plan(BTreeSet::from(["dmux".into()]), Vec::new(), false).unwrap_err();
        assert_eq!(unsafe_empty.code, ErrorCode::BridgeUnavailable);
    }

    #[test]
    fn detach_postcheck_rejects_a_missing_configured_persistent_domain() {
        let detached = BridgeDomainState {
            state: "Detached".into(),
            has_any_panes: false,
            pane_count: 0,
            valid_marker_pane_count: 0,
            system_pane_count: 0,
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

    #[test]
    fn attached_compatible_route_wins_then_fresh_verified_route_wins() {
        let host_uid = marker().host_uid;
        let backend_instance_uid =
            BackendInstanceUid(Uuid::parse_str("44444444-4444-4444-8444-444444444444").unwrap());
        let rows = gui::build_domain_manifest(vec![
            RemoteDomainSource {
                name: "dmux-b-usb".into(),
                remote_address: "10.77.77.2".into(),
                username: "fredrir".into(),
                remote_wezterm_path: Some("/usr/bin/wezterm".into()),
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
        .unwrap();
        let candidates: Vec<_> = rows.iter().collect();
        let detached = BridgeDomainState {
            state: "Detached".into(),
            has_any_panes: false,
            pane_count: 0,
            valid_marker_pane_count: 0,
            system_pane_count: 0,
        };
        let attached = BridgeDomainState {
            state: "Attached".into(),
            has_any_panes: true,
            pane_count: 1,
            valid_marker_pane_count: 0,
            system_pane_count: 1,
        };
        let heartbeat = |domains| BridgeHeartbeat {
            protocol_version: 1,
            gui_instance: "gui-test-1".into(),
            pid: std::process::id(),
            process_start_token: "test-token".into(),
            updated_at: 1,
            panes: Vec::new(),
            domains,
        };

        let mixed = heartbeat(BTreeMap::from([
            ("dmux-b-usb".into(), detached.clone()),
            ("dmux-b-ts".into(), attached),
        ]));
        let selected =
            choose_compatible_presentation_row(Some(&mixed.domains), "dmux-b-usb", &candidates)
                .unwrap();
        assert_eq!(selected.name, "dmux-b-ts");

        let none_attached = heartbeat(BTreeMap::from([
            ("dmux-b-usb".into(), detached.clone()),
            ("dmux-b-ts".into(), detached),
        ]));
        let selected = choose_compatible_presentation_row(
            Some(&none_attached.domains),
            "dmux-b-ts",
            &candidates,
        )
        .unwrap();
        assert_eq!(selected.name, "dmux-b-ts");
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
            pane_count: 0,
            valid_marker_pane_count: 0,
            system_pane_count: 0,
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
}
