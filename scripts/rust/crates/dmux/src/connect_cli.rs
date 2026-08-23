//! Typed, non-creating presentation orchestration for public `dmux con`.
//!
//! This module deliberately contains no provider lookup heuristics and no
//! creation API.  It turns one user selector into an owner-scoped query,
//! freezes the authority's live identity witness, revalidates that witness
//! immediately before presentation, and then selects exactly one backend
//! presentation path.  Production registry/SSH/GUI adapters live at the
//! edges; focused tests can inject them without weakening the invariants.

use std::fmt;
use std::num::NonZeroU64;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{ErrorCode, TypedError};
use crate::history::History;
use crate::model::{
    Backend, BackendInstanceUid, ChildKind, HostUid, ProviderHandle, ServerEpoch, SpaceNo, SpaceUid,
};
use crate::refs::{ChildRefShape, HostToken, parse_ref};
use crate::registry::{HostLifecycle, Registry, RegistryConfig, RouteRow, Transport};
use crate::remote::client::{
    AgentInvocation, DEFAULT_DEADLINE, PeerExpectation, RouteInvoker, SshInvoker,
    call_over_pinned_route, call_over_routes, request_envelope,
};
use crate::remote::protocol::{
    self, AttachChildRequest, AttachPlan, AttachPlanChild, AttachPlanPayload, HelloInfo,
    HelloPayload, PROTOCOL_VERSION,
};
use crate::resolve::{
    HostContext, SpaceSelector, embedded_host, require_consistent_owner, scope_space_ref,
};

/// Parse one standalone canonical child suffix (`g<epoch>.<handle>` or
/// `p<epoch>.<handle>`) for public `--group` / `--split`.  Callers never
/// fabricate a Space prefix or reinterpret a malformed child as a name.
pub fn parse_requested_child(
    value: &str,
    expected_kind: ChildKind,
) -> Result<RequestedChild, TypedError> {
    let parsed = parse_ref(&format!("x/{value}")).map_err(|error| {
        TypedError::new(
            ErrorCode::InvalidRef,
            format!("invalid child ref {value:?}: {error:?}"),
        )
    })?;
    let child = parsed.child.ok_or_else(|| {
        TypedError::new(
            ErrorCode::InvalidRef,
            format!("{value:?} is not an epoch-qualified child ref"),
        )
    })?;
    if child.kind != expected_kind {
        return Err(TypedError::new(
            ErrorCode::InvalidRef,
            format!(
                "child ref {value:?} is {:?}; expected {expected_kind:?}",
                child.kind
            ),
        ));
    }
    Ok(RequestedChild::from(child))
}

/// Host spelling accepted by the presentation resolver.  Alias/label
/// interpretation remains authority-backed; the orchestrator never guesses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum HostSelector {
    Uid(HostUid),
    AliasOrLabel(String),
}

impl From<&HostToken> for HostSelector {
    fn from(value: &HostToken) -> Self {
        match value {
            HostToken::Uid(uid) => HostSelector::Uid(*uid),
            HostToken::AliasOrLabel(value) => HostSelector::AliasOrLabel(value.clone()),
        }
    }
}

/// The three public non-creating selector forms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ConnectSelector {
    /// Normal structural ref grammar, including a possible child suffix.
    Ref(String),
    /// Literal legacy-name escape (`--name`); never structurally parsed.
    ExactName(String),
    /// Stable per-owner history (`dmux -`).
    Previous,
}

/// Epoch-qualified child requested by the caller.  Split parentage is
/// deliberately absent here: the owner discovers and proves it live.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestedChild {
    pub kind: ChildKind,
    pub epoch: ServerEpoch,
    pub handle: ProviderHandle,
}

impl From<ChildRefShape> for RequestedChild {
    fn from(value: ChildRefShape) -> Self {
        RequestedChild {
            kind: value.kind,
            epoch: value.epoch,
            handle: value.handle,
        }
    }
}

/// Public `con` request after clap-level option parsing.  `launch_gui` is an
/// explicit cold GUI request; absence always means the ambient bridge path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectRequest {
    pub selector: ConnectSelector,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explicit_host: Option<HostSelector>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_constraint: Option<Backend>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child: Option<RequestedChild>,
    #[serde(default)]
    pub launch_gui: bool,
}

/// Validate every request property that is decidable without opening the
/// registry, probing the invoking client, scanning a provider, or resolving
/// a host spelling.  Production calls this before deriving ambient tmux
/// authority so malformed CLI input cannot be masked by stale environment
/// state.  The full orchestrator calls it again for injectable callers.
pub fn preflight_connect_request(request: &ConnectRequest) -> Result<(), TypedError> {
    if request.launch_gui && request.backend_constraint == Some(Backend::Tmux) {
        return Err(TypedError::new(
            ErrorCode::Usage,
            "--launch-gui is valid only for a Wez Space",
        ));
    }

    match &request.selector {
        ConnectSelector::ExactName(name) if name.is_empty() => Err(TypedError::new(
            ErrorCode::InvalidRef,
            "exact Space name cannot be empty",
        )),
        ConnectSelector::ExactName(_) | ConnectSelector::Previous => Ok(()),
        ConnectSelector::Ref(reference) => {
            let parsed = parse_connect_ref(reference)?;
            // The one owner contradiction decidable without a host table:
            // both spelled as UIDs. Alias/label spellings are resolved, and
            // the same resolver rule applied, in `resolve_request_scope`.
            let explicit = match &request.explicit_host {
                Some(HostSelector::Uid(uid)) => Some(*uid),
                _ => None,
            };
            let embedded = match embedded_host(&parsed.space) {
                Some(HostToken::Uid(uid)) => Some(uid),
                _ => None,
            };
            require_consistent_owner(explicit, embedded)?;
            merge_requested_child(
                parsed.child.map(RequestedChild::from),
                request.child.clone(),
            )?;
            Ok(())
        }
    }
}

/// Exact owner-side lookup locator; the resolver's type, re-exported so the
/// query the owner answers is the locator §6.2 scoped (ADR 012 WS-D.3).
pub use crate::resolve::OwnerLocator;

/// Query passed to the owner authority.  A backend filter is present only
/// for name lookup, where it is authoritative disambiguation.  Stable
/// identity/number/history lookup intentionally resolves first and lets the
/// orchestrator report a contradictory backend as `backend_mismatch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerConnectQuery {
    pub owner: HostUid,
    pub locator: OwnerLocator,
    pub backend_filter: Option<Backend>,
    pub child: Option<RequestedChild>,
}

/// The exact child witness returned by the owner.  A Split includes its
/// discovered Group parent so GUI/tmux focus never guesses a first Group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VerifiedConnectChild {
    Group {
        epoch: ServerEpoch,
        handle: ProviderHandle,
    },
    Split {
        epoch: ServerEpoch,
        group: ProviderHandle,
        split: ProviderHandle,
    },
}

impl VerifiedConnectChild {
    pub fn epoch(&self) -> ServerEpoch {
        match self {
            VerifiedConnectChild::Group { epoch, .. }
            | VerifiedConnectChild::Split { epoch, .. } => *epoch,
        }
    }

    pub fn requested_handle(&self) -> &ProviderHandle {
        match self {
            VerifiedConnectChild::Group { handle, .. } => handle,
            VerifiedConnectChild::Split { split, .. } => split,
        }
    }

    pub fn kind(&self) -> ChildKind {
        match self {
            VerifiedConnectChild::Group { .. } => ChildKind::Group,
            VerifiedConnectChild::Split { .. } => ChildKind::Split,
        }
    }
}

/// Native binding plus exact backend endpoint frozen by the owner scan.
/// Neither value is accepted from user input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenBinding {
    pub native_token: String,
    /// Wez unix socket or tmux `-L` namespace, as published by the owner.
    pub endpoint: String,
}

/// Fully frozen presentation target.  Implementations of
/// [`ConnectAuthority`] may construct this only after a determinate complete
/// scan and registry/live join under the exact backend-instance read fence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenConnectTarget {
    pub owner: HostUid,
    pub space_uid: SpaceUid,
    pub space_no: SpaceNo,
    pub logical_name: String,
    pub backend: Backend,
    pub backend_instance_uid: BackendInstanceUid,
    pub server_epoch: ServerEpoch,
    pub binding: FrozenBinding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child: Option<VerifiedConnectChild>,
}

/// Exact current tmux-client witness.  Only an identical owner/instance/
/// epoch may use `switch-client`; another local tmux server is not silently
/// treated as a detached terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalTmuxClient {
    pub owner: HostUid,
    pub backend_instance_uid: BackendInstanceUid,
    pub server_epoch: ServerEpoch,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectClientContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmux_client: Option<LocalTmuxClient>,
}

/// Acknowledged GUI presentation kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresentationMode {
    WezAmbient,
    WezCold,
}

/// Completed no-create GUI presentation.  The adapter must return the exact
/// target it acknowledged; the orchestrator rejects a mismatched receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresentationReceipt {
    pub target: FrozenConnectTarget,
    pub mode: PresentationMode,
    /// Bridge/cold-launch request UID or another nonempty authenticated ack
    /// identifier.  It is evidence, not a provider-native target.
    pub acknowledgement: String,
}

impl PresentationReceipt {
    pub fn acknowledged(
        target: FrozenConnectTarget,
        mode: PresentationMode,
        acknowledgement: impl Into<String>,
    ) -> Result<Self, TypedError> {
        let acknowledgement = acknowledgement.into();
        if acknowledgement.trim().is_empty() || acknowledgement.contains('\0') {
            return Err(protocol_error(
                "presentation acknowledgement is empty or invalid",
            ));
        }
        Ok(PresentationReceipt {
            target,
            mode,
            acknowledgement,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TmuxExecKind {
    LocalAttach,
    LocalSwitch,
    RemoteAttach,
}

/// Winning-route and single-use-token witness for the remote PTY channel.
/// Debug output redacts the bearer token.
#[derive(Clone, PartialEq, Eq)]
pub struct RemoteAttachWitness {
    pub request_uid: Uuid,
    pub host_uid: HostUid,
    pub space_uid: SpaceUid,
    pub backend_instance_uid: BackendInstanceUid,
    pub server_epoch: ServerEpoch,
    pub route_id: i64,
    pub route: String,
    pub destination: String,
    pub expires_at: String,
    token: String,
}

impl RemoteAttachWitness {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        request_uid: Uuid,
        host_uid: HostUid,
        space_uid: SpaceUid,
        backend_instance_uid: BackendInstanceUid,
        server_epoch: ServerEpoch,
        route_id: i64,
        route: impl Into<String>,
        destination: impl Into<String>,
        expires_at: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        RemoteAttachWitness {
            request_uid,
            host_uid,
            space_uid,
            backend_instance_uid,
            server_epoch,
            route_id,
            route: route.into(),
            destination: destination.into(),
            expires_at: expires_at.into(),
            token: token.into(),
        }
    }

    pub fn token(&self) -> &str {
        &self.token
    }
}

impl fmt::Debug for RemoteAttachWitness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RemoteAttachWitness")
            .field("request_uid", &self.request_uid)
            .field("host_uid", &self.host_uid)
            .field("space_uid", &self.space_uid)
            .field("backend_instance_uid", &self.backend_instance_uid)
            .field("server_epoch", &self.server_epoch)
            .field("route_id", &self.route_id)
            .field("route", &self.route)
            .field("destination", &self.destination)
            .field("expires_at", &self.expires_at)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// Deferred stable-history update accompanying a terminal handoff.  The
/// caller records it immediately before `exec`; the plan never stores a
/// mutable name or row ordinal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecHistoryIntent {
    pub host_uid: HostUid,
    pub space_uid: SpaceUid,
}

/// Fully validated, owner-generated terminal handoff.  Fields are private so
/// callers cannot swap the argv, target witness, route, or history intent
/// after validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerExecPlan {
    target: FrozenConnectTarget,
    kind: TmuxExecKind,
    argv: Vec<String>,
    remote: Option<RemoteAttachWitness>,
    history: ExecHistoryIntent,
}

impl OwnerExecPlan {
    /// Validate an owner-generated local tmux attach/switch argv.
    pub fn local(
        target: FrozenConnectTarget,
        kind: TmuxExecKind,
        argv: Vec<String>,
    ) -> Result<Self, TypedError> {
        if !matches!(kind, TmuxExecKind::LocalAttach | TmuxExecKind::LocalSwitch) {
            return Err(protocol_error("local tmux plan has a non-local kind"));
        }
        validate_target_shape(&target)?;
        validate_local_tmux_argv(&target, kind, &argv)?;
        let history = ExecHistoryIntent {
            host_uid: target.owner,
            space_uid: target.space_uid,
        };
        Ok(OwnerExecPlan {
            target,
            kind,
            argv,
            remote: None,
            history,
        })
    }

    /// Validate the verified winning route and single-use `_attach` argv.
    pub fn remote(
        target: FrozenConnectTarget,
        argv: Vec<String>,
        remote: RemoteAttachWitness,
    ) -> Result<Self, TypedError> {
        validate_target_shape(&target)?;
        validate_remote_tmux_argv(&target, &argv, &remote)?;
        let history = ExecHistoryIntent {
            host_uid: target.owner,
            space_uid: target.space_uid,
        };
        Ok(OwnerExecPlan {
            target,
            kind: TmuxExecKind::RemoteAttach,
            argv,
            remote: Some(remote),
            history,
        })
    }

    pub fn target(&self) -> &FrozenConnectTarget {
        &self.target
    }

    pub fn kind(&self) -> TmuxExecKind {
        self.kind
    }

    pub fn argv(&self) -> &[String] {
        &self.argv
    }

    pub fn remote_witness(&self) -> Option<&RemoteAttachWitness> {
        self.remote.as_ref()
    }

    pub fn history_intent(&self) -> ExecHistoryIntent {
        self.history
    }

    pub fn into_argv(self) -> Vec<String> {
        self.argv
    }

    fn validate_against(
        &self,
        target: &FrozenConnectTarget,
        expected: TmuxExecKind,
    ) -> Result<(), TypedError> {
        if self.target != *target {
            return Err(protocol_error(
                "presentation planner returned an exec plan for a different target",
            ));
        }
        if self.kind != expected {
            return Err(protocol_error(format!(
                "presentation planner returned {:?}; expected {expected:?}",
                self.kind
            )));
        }
        match self.kind {
            TmuxExecKind::LocalAttach | TmuxExecKind::LocalSwitch => {
                if self.remote.is_some() {
                    return Err(protocol_error("local tmux plan contains a remote token"));
                }
                validate_local_tmux_argv(target, self.kind, &self.argv)
            }
            TmuxExecKind::RemoteAttach => {
                let remote = self.remote.as_ref().ok_or_else(|| {
                    protocol_error("remote tmux plan omitted its token/route witness")
                })?;
                validate_remote_tmux_argv(target, &self.argv, remote)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectOutcome {
    Completed(PresentationReceipt),
    Exec(OwnerExecPlan),
}

/// Stable history read seam.  Production uses [`History`]; tests never need
/// to touch the filesystem.
pub trait ConnectHistory {
    fn previous(&self, host: HostUid) -> Option<SpaceUid>;
}

impl ConnectHistory for History {
    fn previous(&self, host: HostUid) -> Option<SpaceUid> {
        History::previous(self, host)
    }
}

/// Read-only owner authority.  `resolve_live` must use a determinate current
/// provider scan, return typed scan/health errors verbatim, validate the
/// binding and requested child, and perform no creation or mutation.
pub trait ConnectAuthority {
    fn local_host_uid(&mut self) -> Result<HostUid, TypedError>;

    /// Resolve a current enrolled host spelling.  Full UIDs are still
    /// checked for enrollment/identity rather than trusted blindly.
    fn resolve_host(&mut self, selector: &HostSelector) -> Result<HostUid, TypedError>;

    fn resolve_live(
        &mut self,
        query: &OwnerConnectQuery,
    ) -> Result<FrozenConnectTarget, TypedError>;

    /// Repeat the exact registry/live proof under the same owner and
    /// backend-instance fence immediately before presentation.
    fn revalidate_live(
        &mut self,
        target: &FrozenConnectTarget,
    ) -> Result<FrozenConnectTarget, TypedError>;
}

/// Backend presentation adapters.  There is intentionally no fallback
/// method: the orchestrator calls exactly one method after backend freeze.
pub trait ConnectPresenter {
    /// Safe in-GUI adapter (normally `gui_cli::dispatch_ambient_production`).
    fn present_wez_ambient(
        &mut self,
        target: &FrozenConnectTarget,
    ) -> Result<PresentationReceipt, TypedError>;

    /// Explicit targeted cold helper (normally
    /// `gui_cli::present_frozen_cold_production`); never an unqualified GUI
    /// summon.
    fn present_wez_cold(
        &mut self,
        target: &FrozenConnectTarget,
    ) -> Result<PresentationReceipt, TypedError>;

    fn prepare_local_tmux(
        &mut self,
        target: &FrozenConnectTarget,
        kind: TmuxExecKind,
    ) -> Result<OwnerExecPlan, TypedError>;

    /// Mint an attach plan on the owner, bind it to the verified winning
    /// route, and return the exact local `ssh -t ... _attach --token ...`
    /// argv.  No native tmux target crosses this seam.
    fn prepare_remote_tmux(
        &mut self,
        target: &FrozenConnectTarget,
    ) -> Result<OwnerExecPlan, TypedError>;
}

/// Convenience supertrait for production/new orchestrators that use one
/// stateful adapter for both authority reads and presentation.
pub trait ConnectRuntime: ConnectAuthority + ConnectPresenter {}

impl<T: ConnectAuthority + ConnectPresenter + ?Sized> ConnectRuntime for T {}

struct SplitConnectRuntime<'a> {
    authority: &'a mut dyn ConnectAuthority,
    presenter: &'a mut dyn ConnectPresenter,
}

impl ConnectAuthority for SplitConnectRuntime<'_> {
    fn local_host_uid(&mut self) -> Result<HostUid, TypedError> {
        self.authority.local_host_uid()
    }

    fn resolve_host(&mut self, selector: &HostSelector) -> Result<HostUid, TypedError> {
        self.authority.resolve_host(selector)
    }

    fn resolve_live(
        &mut self,
        query: &OwnerConnectQuery,
    ) -> Result<FrozenConnectTarget, TypedError> {
        self.authority.resolve_live(query)
    }

    fn revalidate_live(
        &mut self,
        target: &FrozenConnectTarget,
    ) -> Result<FrozenConnectTarget, TypedError> {
        self.authority.revalidate_live(target)
    }
}

impl ConnectPresenter for SplitConnectRuntime<'_> {
    fn present_wez_ambient(
        &mut self,
        target: &FrozenConnectTarget,
    ) -> Result<PresentationReceipt, TypedError> {
        self.presenter.present_wez_ambient(target)
    }

    fn present_wez_cold(
        &mut self,
        target: &FrozenConnectTarget,
    ) -> Result<PresentationReceipt, TypedError> {
        self.presenter.present_wez_cold(target)
    }

    fn prepare_local_tmux(
        &mut self,
        target: &FrozenConnectTarget,
        kind: TmuxExecKind,
    ) -> Result<OwnerExecPlan, TypedError> {
        self.presenter.prepare_local_tmux(target, kind)
    }

    fn prepare_remote_tmux(
        &mut self,
        target: &FrozenConnectTarget,
    ) -> Result<OwnerExecPlan, TypedError> {
        self.presenter.prepare_remote_tmux(target)
    }
}

/// Production adapter shared by the read-only authority and presentation
/// traits.  GUI authority/revalidation stays in `gui_cli`; this type owns
/// only public-connect routing and terminal tmux handoff planning.
pub struct ProductionConnectAdapter<I = SshInvoker> {
    env: crate::operations::OperationEnv,
    invoker: I,
    remote_bin: String,
}

impl ProductionConnectAdapter<SshInvoker> {
    pub fn production() -> Result<Self, TypedError> {
        let env = crate::operations::OperationEnv::production()
            .map_err(|error| TypedError::new(ErrorCode::OperationFailed, error.to_string()))?;
        Ok(ProductionConnectAdapter {
            env,
            invoker: SshInvoker::default(),
            remote_bin: "dmux".to_string(),
        })
    }
}

impl<I: RouteInvoker> ProductionConnectAdapter<I> {
    /// Injectable constructor for scratch owner/transport integration tests.
    /// The invoker still receives only fixed `_agent` methods; it cannot
    /// supply a native tmux target.
    pub fn with_invoker(
        env: crate::operations::OperationEnv,
        invoker: I,
        remote_bin: impl Into<String>,
    ) -> Self {
        ProductionConnectAdapter {
            env,
            invoker,
            remote_bin: remote_bin.into(),
        }
    }

    fn registry(&self) -> Result<Registry, TypedError> {
        Registry::open(RegistryConfig::new(&self.env.db_path, &self.env.lock_dir))
            .map_err(|error| TypedError::new(error.error_code(), error.to_string()))
    }

    fn host_row(&self, selector: &HostSelector) -> Result<crate::registry::HostRow, TypedError> {
        let registry = self.registry()?;
        let matches: Vec<_> = registry
            .hosts()
            .map_err(|error| TypedError::new(error.error_code(), error.to_string()))?
            .into_iter()
            .filter(|row| row.lifecycle == HostLifecycle::Enrolled)
            .filter(|row| match selector {
                HostSelector::Uid(uid) => row.host_uid == *uid,
                HostSelector::AliasOrLabel(spelling) => {
                    row.alias.as_deref() == Some(spelling) || row.label.as_deref() == Some(spelling)
                }
            })
            .collect();
        match matches.as_slice() {
            [one] => Ok(one.clone()),
            [] => Err(TypedError::new(
                ErrorCode::NotFound,
                format!("no enrolled host matches {selector:?}"),
            )),
            _ => Err(TypedError::new(
                ErrorCode::AmbiguousTarget,
                format!("host spelling {selector:?} matches more than one enrolled owner"),
            )),
        }
    }

    fn fresh_remote_hello(
        &self,
        registry: &mut Registry,
        host_uid: HostUid,
    ) -> Result<(HelloInfo, i64), TypedError> {
        let identity = registry
            .identity()
            .map_err(|error| TypedError::new(error.error_code(), error.to_string()))?;
        let head = registry
            .authority_head()
            .map_err(|error| TypedError::new(error.error_code(), error.to_string()))?;
        let nonce = Uuid::new_v4();
        let request_uid = Uuid::new_v4();
        let payload = serde_json::to_value(HelloPayload { nonce: Some(nonce) })
            .map_err(|error| protocol_error(format!("serializing hello: {error}")))?;
        let request = request_envelope(
            &identity,
            &head,
            protocol::methods::HELLO,
            request_uid,
            payload,
        );
        let mut invocation = AgentInvocation::new(protocol::methods::HELLO);
        invocation.remote_bin = self.remote_bin.clone();
        let outcome = call_over_routes(
            registry,
            &PeerExpectation {
                host_uid,
                need_capability: None,
                claimed_current: true,
            },
            &request,
            &self.invoker,
            &invocation,
            DEFAULT_DEADLINE,
        )?;
        if outcome.envelope.method != protocol::methods::HELLO {
            return Err(protocol_error("owner hello response changed the method"));
        }
        let hello: HelloInfo = serde_json::from_value(
            outcome
                .envelope
                .payload
                .ok_or_else(|| protocol_error("owner hello omitted its payload"))?,
        )
        .map_err(|error| protocol_error(format!("owner hello payload: {error}")))?;
        if hello.host_uid != host_uid
            || hello.protocol_version != PROTOCOL_VERSION
            || hello.nonce != Some(nonce)
        {
            return Err(protocol_error(
                "owner hello identity/protocol/nonce differs from the request",
            ));
        }
        Ok((hello, outcome.route_id))
    }

    fn winning_route(
        &self,
        registry: &Registry,
        host_uid: HostUid,
        route_id: i64,
    ) -> Result<RouteRow, TypedError> {
        registry
            .routes_for(host_uid)
            .map_err(|error| TypedError::new(error.error_code(), error.to_string()))?
            .into_iter()
            .find(|route| {
                route.route_id == route_id
                    && route.enabled
                    && route.host_uid == host_uid
                    && route.transport != Transport::Local
            })
            .ok_or_else(|| {
                TypedError::new(
                    ErrorCode::RouteUnavailable,
                    format!("winning route {route_id} is no longer an enabled remote route"),
                )
            })
    }

    fn remote_attach_plan(
        &self,
        target: &FrozenConnectTarget,
    ) -> Result<(AttachPlan, RouteRow), TypedError> {
        let mut registry = self.registry()?;
        let (hello, route_id) = self.fresh_remote_hello(&mut registry, target.owner)?;
        let matching_backends: Vec<_> = hello
            .backends
            .iter()
            .filter(|backend| {
                backend.backend == Backend::Tmux
                    && backend.backend_instance_uid == target.backend_instance_uid
                    && backend.server_epoch == Some(target.server_epoch)
            })
            .collect();
        if matching_backends.len() != 1 {
            return Err(TypedError::new(
                ErrorCode::BackendEpochChanged,
                "fresh owner hello did not prove the frozen tmux instance/epoch",
            ));
        }
        let route = self.winning_route(&registry, target.owner, route_id)?;
        let route_name = route
            .wez_domain
            .clone()
            .unwrap_or_else(|| route.endpoint.clone());
        let identity = registry
            .identity()
            .map_err(|error| TypedError::new(error.error_code(), error.to_string()))?;
        let head = registry
            .authority_head()
            .map_err(|error| TypedError::new(error.error_code(), error.to_string()))?;
        let request_uid = Uuid::new_v4();
        let payload = serde_json::to_value(AttachPlanPayload {
            space_uid: target.space_uid,
            route: Some(route_name.clone()),
            child: target.child.as_ref().map(|child| AttachChildRequest {
                kind: child.kind(),
                epoch: child.epoch(),
                handle: child.requested_handle().clone(),
            }),
        })
        .map_err(|error| protocol_error(format!("serializing attach plan: {error}")))?;
        let mut request = request_envelope(
            &identity,
            &head,
            protocol::methods::ATTACH_PLAN,
            request_uid,
            payload,
        );
        request.backend_instance_uid = Some(target.backend_instance_uid);
        request.server_epoch = Some(target.server_epoch);
        let mut invocation = AgentInvocation::new(protocol::methods::ATTACH_PLAN);
        invocation.remote_bin = self.remote_bin.clone();
        let outcome = call_over_pinned_route(
            &mut registry,
            &PeerExpectation {
                host_uid: target.owner,
                need_capability: None,
                claimed_current: false,
            },
            route_id,
            &request,
            &self.invoker,
            &invocation,
            DEFAULT_DEADLINE,
        )?;
        if outcome.route_id != route_id
            || outcome.envelope.method != protocol::methods::ATTACH_PLAN
            || outcome.envelope.backend_instance_uid != Some(target.backend_instance_uid)
            || outcome.envelope.server_epoch != Some(target.server_epoch)
        {
            return Err(protocol_error(
                "owner attach-plan envelope differs from the frozen target/route",
            ));
        }
        let plan: AttachPlan = serde_json::from_value(
            outcome
                .envelope
                .payload
                .ok_or_else(|| protocol_error("owner attach plan omitted its payload"))?,
        )
        .map_err(|error| protocol_error(format!("owner attach-plan payload: {error}")))?;
        if plan.request_uid != request_uid
            || plan.host_uid != target.owner
            || plan.space_uid != target.space_uid
            || plan.server_epoch != target.server_epoch
            || plan.route != route_name
            || plan.token.is_empty()
            || plan.replayed
            || plan.child.as_ref().map(protocol_child_as_verified) != target.child.clone()
        {
            return Err(protocol_error(
                "owner returned a stale, replayed, or differently targeted attach plan",
            ));
        }
        Ok((plan, route))
    }
}

impl<I: RouteInvoker> ConnectAuthority for ProductionConnectAdapter<I> {
    fn local_host_uid(&mut self) -> Result<HostUid, TypedError> {
        self.registry()?
            .identity()
            .map(|identity| identity.host_uid)
            .map_err(|error| TypedError::new(error.error_code(), error.to_string()))
    }

    fn resolve_host(&mut self, selector: &HostSelector) -> Result<HostUid, TypedError> {
        self.host_row(selector).map(|row| row.host_uid)
    }

    fn resolve_live(
        &mut self,
        query: &OwnerConnectQuery,
    ) -> Result<FrozenConnectTarget, TypedError> {
        crate::gui_cli::resolve_production_connect_query(query)
    }

    fn revalidate_live(
        &mut self,
        target: &FrozenConnectTarget,
    ) -> Result<FrozenConnectTarget, TypedError> {
        crate::gui_cli::revalidate_production_connect_target(target)
    }
}

impl<I: RouteInvoker> ConnectPresenter for ProductionConnectAdapter<I> {
    fn present_wez_ambient(
        &mut self,
        target: &FrozenConnectTarget,
    ) -> Result<PresentationReceipt, TypedError> {
        crate::gui_cli::present_frozen_ambient_production(target)
    }

    fn present_wez_cold(
        &mut self,
        target: &FrozenConnectTarget,
    ) -> Result<PresentationReceipt, TypedError> {
        crate::gui_cli::present_frozen_cold_production(target)
    }

    fn prepare_local_tmux(
        &mut self,
        target: &FrozenConnectTarget,
        kind: TmuxExecKind,
    ) -> Result<OwnerExecPlan, TypedError> {
        let verb = match kind {
            TmuxExecKind::LocalAttach => "attach",
            TmuxExecKind::LocalSwitch => "switch-client",
            TmuxExecKind::RemoteAttach => {
                return Err(protocol_error("remote kind reached local tmux planner"));
            }
        };
        let mut argv = vec![
            "tmux".to_string(),
            "-L".to_string(),
            target.binding.endpoint.clone(),
        ];
        append_exact_tmux_child_focus(&mut argv, target)?;
        argv.extend([
            verb.to_string(),
            "-t".to_string(),
            target.binding.native_token.clone(),
        ]);
        OwnerExecPlan::local(target.clone(), kind, argv)
    }

    fn prepare_remote_tmux(
        &mut self,
        target: &FrozenConnectTarget,
    ) -> Result<OwnerExecPlan, TypedError> {
        let (plan, route) = self.remote_attach_plan(target)?;
        let destination = match &route.username {
            Some(username) => format!("{username}@{}", route.endpoint),
            None => route.endpoint.clone(),
        };
        let argv = vec![
            "ssh".to_string(),
            "-oBatchMode=yes".to_string(),
            "-oConnectTimeout=10".to_string(),
            "-tt".to_string(),
            destination.clone(),
            self.remote_bin.clone(),
            "_attach".to_string(),
            "--token".to_string(),
            plan.token.clone(),
        ];
        let witness = RemoteAttachWitness::new(
            plan.request_uid,
            plan.host_uid,
            plan.space_uid,
            target.backend_instance_uid,
            plan.server_epoch,
            route.route_id,
            plan.route,
            destination,
            plan.expires_at,
            plan.token,
        );
        OwnerExecPlan::remote(target.clone(), argv, witness)
    }
}

/// Derive the invoking terminal's tmux status without trusting `TMUX` or
/// marker variables as authority.  `None` is returned only when no tmux
/// selector is present and no tmux dmux marker claims otherwise.
pub fn production_connect_client_context() -> Result<ConnectClientContext, TypedError> {
    let tmux = std::env::var_os("TMUX");
    let tmux_pane = std::env::var_os("TMUX_PANE");
    match (&tmux, &tmux_pane) {
        (None, None) => {
            if std::env::var("DMUX_BACKEND").ok().as_deref() == Some("tmux") {
                return Err(TypedError::new(
                    ErrorCode::InvalidRef,
                    "tmux dmux marker exists but TMUX/TMUX_PANE are absent",
                ));
            }
            return Ok(ConnectClientContext::default());
        }
        (Some(value), Some(pane)) if !value.is_empty() && !pane.is_empty() => {}
        _ => {
            return Err(TypedError::new(
                ErrorCode::InvalidRef,
                "TMUX and TMUX_PANE must both be present and nonempty",
            ));
        }
    }

    let pane = tmux_pane
        .and_then(|value| value.into_string().ok())
        .ok_or_else(|| TypedError::new(ErrorCode::InvalidRef, "TMUX_PANE is not UTF-8"))?;
    let pane_id = pane
        .strip_prefix('%')
        .filter(|digits| {
            !digits.is_empty()
                && (digits == &"0"
                    || (!digits.starts_with('0')
                        && digits.bytes().all(|byte| byte.is_ascii_digit())))
        })
        .and_then(|digits| digits.parse::<u64>().ok())
        .ok_or_else(|| {
            TypedError::new(
                ErrorCode::InvalidRef,
                "TMUX_PANE is not a canonical %<decimal> pane id",
            )
        })?;
    let supplied = marker_from_process_env()?;
    if supplied.backend != Backend::Tmux || supplied.domain.is_some() {
        return Err(TypedError::new(
            ErrorCode::BackendMismatch,
            "a tmux client requires a local tmux dmux marker with no GUI domain",
        ));
    }

    let env = crate::operations::OperationEnv::production()
        .map_err(|error| TypedError::new(ErrorCode::OperationFailed, error.to_string()))?;
    let registry = Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir))
        .map_err(|error| TypedError::new(error.error_code(), error.to_string()))?;
    let identity = registry
        .identity()
        .map_err(|error| TypedError::new(error.error_code(), error.to_string()))?;
    if supplied.host_uid != identity.host_uid {
        return Err(TypedError::new(
            ErrorCode::HostIdentityChanged,
            "ambient tmux marker is not owned by the local authority",
        ));
    }
    let space = registry
        .space(supplied.space_uid)
        .map_err(|error| TypedError::new(error.error_code(), error.to_string()))?;
    let info = registry
        .backend_instance_info(space.backend_instance)
        .map_err(|error| TypedError::new(error.error_code(), error.to_string()))?;
    if info.backend != Backend::Tmux || info.owner != identity.host_uid {
        return Err(TypedError::new(
            ErrorCode::WrongBackendInstance,
            "ambient pane Space is not on the local managed tmux instance",
        ));
    }
    let namespace = info.socket_path.ok_or_else(|| {
        TypedError::new(
            ErrorCode::ProviderUnavailable,
            "managed tmux instance has no recorded -L namespace",
        )
    })?;
    let server = registry
        .backend_server(space.backend_instance)
        .map_err(|error| TypedError::new(error.error_code(), error.to_string()))?;
    let epoch = server.server_epoch.ok_or_else(|| {
        TypedError::new(
            ErrorCode::ProviderUnavailable,
            "managed tmux instance has no published epoch",
        )
    })?;
    let expected_identity = crate::backend::tmux::TmuxServerIdentity {
        pid: server
            .server_pid
            .and_then(|pid| u32::try_from(pid).ok())
            .ok_or_else(|| {
                TypedError::new(
                    ErrorCode::WrongBackendInstance,
                    "managed tmux instance has no valid published pid",
                )
            })?,
        start_token: server.server_start_token.clone().ok_or_else(|| {
            TypedError::new(
                ErrorCode::WrongBackendInstance,
                "managed tmux instance has no published process start token",
            )
        })?,
    };
    drop(registry);

    let provider = crate::backend::tmux::TmuxProvider::new(namespace.clone());
    require_published_tmux_incarnation(&provider, &namespace, &server)?;
    provider
        .verify_epoch(&namespace, epoch, &expected_identity)
        .map_err(typed_context_provider)?;
    // `#{pid}` is tmux's "server PID" format (there is no `server_pid`;
    // tmux expands an unknown variable to the empty string, which refused
    // every real client here until ADR 012 WS-D.3's reader test ran one).
    let ambient = Command::new("tmux")
        .args([
            "display-message",
            "-p",
            "#{pid}|#{pane_id}|#{@dmux_server_epoch}",
        ])
        .output()
        .map_err(|error| {
            TypedError::new(
                ErrorCode::ProviderUnavailable,
                format!("probing invoking tmux client: {error}"),
            )
        })?;
    if !ambient.status.success() {
        return Err(TypedError::new(
            ErrorCode::WrongBackendInstance,
            format!(
                "TMUX does not select a live invoking client: {}",
                String::from_utf8_lossy(&ambient.stderr)
                    .lines()
                    .next()
                    .unwrap_or("tmux display-message failed")
            ),
        ));
    }
    let ambient = std::str::from_utf8(&ambient.stdout)
        .map_err(|_| protocol_error("invoking tmux identity output is not UTF-8"))?
        .trim_end();
    let fields: Vec<_> = ambient.split('|').collect();
    if fields.len() != 3
        || fields[0] != expected_identity.pid.to_string()
        || fields[1] != pane
        || fields[2] != epoch.0.to_string()
    {
        return Err(TypedError::new(
            ErrorCode::WrongBackendInstance,
            "TMUX selects a different server, pane, or epoch than the managed marker",
        ));
    }

    let scope = crate::backend::InventoryScope::managed(Backend::Tmux, namespace.clone(), epoch);
    let authoritative = crate::operations::context_read(
        &env,
        &provider,
        &scope,
        supplied.space_uid,
        &format!("%{pane_id}"),
    )
    .map_err(typed_context_operation)?;
    if authoritative != supplied {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "ambient tmux marker differs from the exact registry/live pane context",
        ));
    }
    provider
        .verify_epoch(&namespace, epoch, &expected_identity)
        .map_err(typed_context_provider)?;
    Ok(ConnectClientContext {
        tmux_client: Some(LocalTmuxClient {
            owner: identity.host_uid,
            backend_instance_uid: space.backend_instance,
            server_epoch: epoch,
        }),
    })
}

/// Complete production planner: stable history, exact client context, one
/// owner/live resolver, and exactly one presentation adapter.
pub fn plan_connect_production(request: &ConnectRequest) -> Result<ConnectOutcome, TypedError> {
    preflight_connect_request(request)?;
    let client = production_connect_client_context()?;
    let state_dir = History::default_dir().ok_or_else(|| {
        TypedError::new(
            ErrorCode::OperationFailed,
            "HOME/XDG_STATE_HOME is unavailable for stable Space history",
        )
    })?;
    let history = History::new(state_dir);
    let mut runtime = ProductionConnectAdapter::production()?;
    connect_with_runtime(request, &client, &history, &mut runtime)
}

/// Production form of [`commit_exec_history`].
pub fn commit_production_exec_history(plan: &OwnerExecPlan) -> Result<(), TypedError> {
    let state_dir = History::default_dir().ok_or_else(|| {
        TypedError::new(
            ErrorCode::OperationFailed,
            "HOME/XDG_STATE_HOME is unavailable for stable Space history",
        )
    })?;
    commit_exec_history(&History::new(state_dir), plan)
}

/// Ordered side effects at the last boundary before a validated tmux exec.
/// The injectable associated types keep the production GUI source and
/// controller reservation opaque while allowing deterministic race tests.
pub trait ExecHandoffRuntime {
    type Source;
    type Correlation;

    /// Capture the exact visible source before any correlation side effect.
    fn capture_gui_source(
        &mut self,
        plan: &OwnerExecPlan,
    ) -> Result<Option<Self::Source>, TypedError>;

    /// Return the already-correlated client UID only for an exact tmux
    /// source (LocalSwitch); a Wez source and terminal-only source return
    /// `None`.
    fn source_tmux_client_uid(&self, source: &Self::Source) -> Option<Uuid>;

    /// Reserve PID/start/TTY or the remote request UID without emitting OSC.
    fn reserve_controller_correlation(
        &mut self,
        plan: &OwnerExecPlan,
        existing_client_uid: Option<Uuid>,
    ) -> Result<Option<Self::Correlation>, TypedError>;

    fn correlation_uid(&self, correlation: &Self::Correlation) -> Uuid;

    /// Persist only a pending source/destination proof. This must not rotate
    /// global GUI history; the detached monitor finalizes after the live tmux
    /// hook publishes the exact attached destination marker.
    fn stage_gui_transition(
        &mut self,
        plan: &OwnerExecPlan,
        tmux_client_uid: Uuid,
        source: Option<&Self::Source>,
    ) -> Result<Option<Uuid>, TypedError>;

    fn start_gui_transition_finalizer(&mut self, pending_uid: Uuid) -> Result<(), TypedError>;

    fn commit_terminal_history(&mut self, plan: &OwnerExecPlan) -> Result<(), TypedError>;

    fn cancel_gui_transition(&mut self, pending_uid: Uuid) -> Result<(), TypedError>;

    fn cancel_controller_correlation(
        &mut self,
        correlation: &Self::Correlation,
    ) -> Result<(), TypedError>;
}

#[derive(Debug)]
pub struct PreparedExecHandoff<C> {
    correlation: Option<C>,
    pending_uid: Option<Uuid>,
}

impl<C> PreparedExecHandoff<C> {
    pub fn pending_uid(&self) -> Option<Uuid> {
        self.pending_uid
    }
}

fn cleanup_failed_handoff<R: ExecHandoffRuntime + ?Sized>(
    runtime: &mut R,
    correlation: Option<&R::Correlation>,
    pending_uid: Option<Uuid>,
    mut primary: TypedError,
) -> TypedError {
    let mut cleanup_failures = Vec::new();
    if let Some(pending_uid) = pending_uid
        && let Err(error) = runtime.cancel_gui_transition(pending_uid)
    {
        cleanup_failures.push(error.message);
    }
    if let Some(correlation) = correlation
        && let Err(error) = runtime.cancel_controller_correlation(correlation)
    {
        cleanup_failures.push(error.message);
    }
    if !cleanup_failures.is_empty() {
        primary.message = format!(
            "{}; pre-exec cleanup also failed: {}",
            primary.message,
            cleanup_failures.join("; ")
        );
    }
    primary
}

/// Testable source-capture/reservation/staging order shared by public
/// Connect and New. No destination marker is emitted here. A managed source
/// without an exact correlation or staged monitor fails closed; terminal-only
/// handoffs update only per-owner terminal history.
pub fn prepare_exec_handoff_with_runtime<R: ExecHandoffRuntime + ?Sized>(
    plan: &OwnerExecPlan,
    runtime: &mut R,
) -> Result<PreparedExecHandoff<R::Correlation>, TypedError> {
    let source = runtime.capture_gui_source(plan)?;
    let existing_client_uid = source
        .as_ref()
        .and_then(|source| runtime.source_tmux_client_uid(source));
    let correlation = runtime.reserve_controller_correlation(plan, existing_client_uid)?;
    if source.is_some() && correlation.is_none() {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "managed GUI source has no exact controller correlation",
        ));
    }

    let mut pending_uid = None;
    let prepared = (|| {
        if let Some(correlation) = correlation.as_ref() {
            let client_uid = runtime.correlation_uid(correlation);
            pending_uid = runtime.stage_gui_transition(plan, client_uid, source.as_ref())?;
            if let Some(staged_uid) = pending_uid
                && staged_uid != client_uid
            {
                return Err(TypedError::new(
                    ErrorCode::ProtocolMismatch,
                    "staged GUI transition UID differs from its controller correlation",
                ));
            }
            if source.is_some() && pending_uid.is_none() {
                return Err(TypedError::new(
                    ErrorCode::ProtocolMismatch,
                    "managed GUI transition was not staged",
                ));
            }
            if source.is_none() && pending_uid.is_some() {
                return Err(TypedError::new(
                    ErrorCode::ProtocolMismatch,
                    "terminal-only handoff unexpectedly staged global GUI history",
                ));
            }
            if let Some(pending_uid) = pending_uid {
                runtime.start_gui_transition_finalizer(pending_uid)?;
            }
        }
        runtime.commit_terminal_history(plan)
    })();
    if let Err(error) = prepared {
        return Err(cleanup_failed_handoff(
            runtime,
            correlation.as_ref(),
            pending_uid,
            error,
        ));
    }
    Ok(PreparedExecHandoff {
        correlation,
        pending_uid,
    })
}

struct ProductionExecHandoffRuntime;

impl ExecHandoffRuntime for ProductionExecHandoffRuntime {
    type Source = crate::gui_cli::GuiExecSourceWitness;
    type Correlation = crate::remote::attach::ControllerCorrelationReservation;

    fn capture_gui_source(
        &mut self,
        plan: &OwnerExecPlan,
    ) -> Result<Option<Self::Source>, TypedError> {
        crate::gui_cli::capture_exact_gui_exec_source_production(plan)
    }

    fn source_tmux_client_uid(&self, source: &Self::Source) -> Option<Uuid> {
        source.tmux_client_uid()
    }

    fn reserve_controller_correlation(
        &mut self,
        plan: &OwnerExecPlan,
        existing_client_uid: Option<Uuid>,
    ) -> Result<Option<Self::Correlation>, TypedError> {
        crate::remote::attach::reserve_controller_correlation(plan, existing_client_uid)
    }

    fn correlation_uid(&self, correlation: &Self::Correlation) -> Uuid {
        correlation.client_uid()
    }

    fn stage_gui_transition(
        &mut self,
        plan: &OwnerExecPlan,
        tmux_client_uid: Uuid,
        source: Option<&Self::Source>,
    ) -> Result<Option<Uuid>, TypedError> {
        match crate::gui_cli::stage_correlated_gui_exec_transition_production(
            plan,
            tmux_client_uid,
            source,
        )? {
            crate::gui_cli::GuiExecTransitionOutcome::TerminalOnly => Ok(None),
            crate::gui_cli::GuiExecTransitionOutcome::Staged { pending_uid } => {
                Ok(Some(pending_uid))
            }
        }
    }

    fn start_gui_transition_finalizer(&mut self, pending_uid: Uuid) -> Result<(), TypedError> {
        start_gui_exec_transition_finalizer(pending_uid)
    }

    fn commit_terminal_history(&mut self, plan: &OwnerExecPlan) -> Result<(), TypedError> {
        commit_production_exec_history(plan)
    }

    fn cancel_gui_transition(&mut self, pending_uid: Uuid) -> Result<(), TypedError> {
        crate::gui_cli::cancel_correlated_gui_exec_transition_production(pending_uid).map(|_| ())
    }

    fn cancel_controller_correlation(
        &mut self,
        correlation: &Self::Correlation,
    ) -> Result<(), TypedError> {
        crate::remote::attach::cancel_controller_correlation_reservation(correlation).map(|_| ())
    }
}

fn start_gui_exec_transition_finalizer(pending_uid: Uuid) -> Result<(), TypedError> {
    let current_exe = std::env::current_exe().map_err(|error| {
        TypedError::new(
            ErrorCode::OperationFailed,
            format!("locating dmux GUI transition finalizer: {error}"),
        )
    })?;
    let mut child = Command::new(current_exe)
        .args([
            "_gui-exec-finalize",
            "--pending-uid",
            &pending_uid.to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            TypedError::new(
                ErrorCode::OperationFailed,
                format!("starting dmux GUI transition finalizer: {error}"),
            )
        })?;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(TypedError::new(
                    ErrorCode::OperationFailed,
                    format!("dmux GUI transition finalizer bootstrap exited {status}"),
                ));
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(TypedError::new(
                    ErrorCode::OperationFailed,
                    "dmux GUI transition finalizer bootstrap timed out",
                ));
            }
            Err(error) => {
                return Err(TypedError::new(
                    ErrorCode::OperationFailed,
                    format!("waiting for dmux GUI transition finalizer bootstrap: {error}"),
                ));
            }
        }
    }
}

/// Armed pre-exec guard. A successful `exec` replaces the process and never
/// drops it. If exec returns, or dispatch exits early, Drop cancels the
/// pending transition and removes only a fresh unstarted local record.
#[must_use = "the handoff guard must remain live until exec replaces the process"]
pub struct ProductionExecHandoffGuard {
    correlation: Option<crate::remote::attach::ControllerCorrelationReservation>,
    pending_uid: Option<Uuid>,
}

impl Drop for ProductionExecHandoffGuard {
    fn drop(&mut self) {
        if let Some(pending_uid) = self.pending_uid {
            let _ = crate::gui_cli::cancel_correlated_gui_exec_transition_production(pending_uid);
        }
        if let Some(correlation) = self.correlation.as_ref() {
            let _ = crate::remote::attach::cancel_controller_correlation_reservation(correlation);
        }
    }
}

/// Capture the exact GUI source, reserve correlation without OSC, stage and
/// start the post-attach finalizer, then commit terminal history. The caller
/// keeps the returned guard alive while consuming the plan with `exec`.
pub fn prepare_production_exec_handoff(
    plan: &OwnerExecPlan,
) -> Result<ProductionExecHandoffGuard, TypedError> {
    let prepared = prepare_exec_handoff_with_runtime(plan, &mut ProductionExecHandoffRuntime)?;
    Ok(ProductionExecHandoffGuard {
        correlation: prepared.correlation,
        pending_uid: prepared.pending_uid,
    })
}

fn marker_from_process_env() -> Result<crate::bootstrap::MarkerContext, TypedError> {
    fn required(name: &str) -> Result<String, TypedError> {
        std::env::var(name).map_err(|_| {
            TypedError::new(
                ErrorCode::InvalidRef,
                format!("managed tmux client requires exact {name}"),
            )
        })
    }
    fn canonical_uuid(name: &str) -> Result<Uuid, TypedError> {
        let raw = required(name)?;
        let value = Uuid::parse_str(&raw).map_err(|error| {
            TypedError::new(
                ErrorCode::InvalidRef,
                format!("tmux marker {name} is not a UUID: {error}"),
            )
        })?;
        if raw != value.to_string() {
            return Err(TypedError::new(
                ErrorCode::InvalidRef,
                format!("tmux marker {name} is not canonical lowercase UUID text"),
            ));
        }
        Ok(value)
    }
    if required("DMUX_CONTEXT_VERSION")? != "1" {
        return Err(TypedError::new(
            ErrorCode::InvalidRef,
            "tmux marker DMUX_CONTEXT_VERSION must be 1",
        ));
    }
    let space_no_raw = required("DMUX_SPACE_NO")?;
    let space_no_value = space_no_raw.parse::<u64>().map_err(|error| {
        TypedError::new(
            ErrorCode::InvalidRef,
            format!("tmux marker DMUX_SPACE_NO is malformed: {error}"),
        )
    })?;
    let space_no = NonZeroU64::new(space_no_value).ok_or_else(|| {
        TypedError::new(
            ErrorCode::InvalidRef,
            "tmux marker DMUX_SPACE_NO must be nonzero",
        )
    })?;
    if space_no_raw != space_no.to_string() {
        return Err(TypedError::new(
            ErrorCode::InvalidRef,
            "tmux marker DMUX_SPACE_NO is not canonical decimal",
        ));
    }
    let backend = match required("DMUX_BACKEND")?.as_str() {
        "tmux" => Backend::Tmux,
        "wez" => Backend::Wez,
        _ => {
            return Err(TypedError::new(
                ErrorCode::InvalidRef,
                "tmux marker DMUX_BACKEND must be wez or tmux",
            ));
        }
    };
    let domain = match required("DMUX_DOMAIN")? {
        value if value.is_empty() => None,
        value => Some(value),
    };
    Ok(crate::bootstrap::MarkerContext {
        host_uid: HostUid(canonical_uuid("DMUX_HOST_UID")?),
        space_uid: SpaceUid(canonical_uuid("DMUX_SPACE_UID")?),
        space_no: SpaceNo(space_no),
        backend,
        domain,
        server_epoch: ServerEpoch(canonical_uuid("DMUX_SERVER_EPOCH")?),
        group_ref: required("DMUX_GROUP_REF")?,
        split_ref: required("DMUX_SPLIT_REF")?,
    })
}

/// ADR 012 WS-A.9 at this reader (O's close handed the five readers outside
/// the operations layer to their file owners): when the registry row carries
/// the socket witnesses `tmux_bootstrap` published, a fresh probe of the
/// namespace and a fresh `stat` of its socket must agree with the row on pid,
/// start token, dev and ino before the server's self-reported epoch is
/// consulted. A replaced server on the same socket path that merely presents
/// the old `@dmux_server_epoch` is a stale incarnation (ADR 012 §3.1 state F)
/// and is refused `backend_epoch_changed` — the code every operations-layer
/// tmux verification answers with (`operations::verify_published_incarnation`)
/// — rather than the `wrong_backend_instance` a pid mismatch alone would draw
/// from `verify_epoch`, so the operator hears one name for one fault. That is
/// why this runs *before* `verify_epoch`. A row published before WS-A.9
/// carries no witnesses and is verified by identity and epoch alone.
pub fn require_published_tmux_incarnation(
    provider: &crate::backend::tmux::TmuxProvider<crate::backend::tmux::SystemRunner>,
    namespace: &str,
    published: &crate::registry::BackendServerRecord,
) -> Result<(), TypedError> {
    if published.socket_dev.is_none() && published.socket_ino.is_none() {
        return Ok(());
    }
    let live = provider.server_incarnation(namespace).map_err(|error| {
        TypedError::new(
            ErrorCode::ProviderUnavailable,
            format!("tmux incarnation probe on namespace {namespace:?}: {error:?}"),
        )
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
        return Err(TypedError::new(
            ErrorCode::BackendEpochChanged,
            format!(
                "tmux server on namespace {namespace:?} is a stale incarnation, not the \
                 registry-published one: registry pid {:?} start {:?} socket dev/ino {:?}/{:?}; \
                 live pid {} start {:?} socket {:?} dev/ino {}/{} (ADR 012 §3.1 state F); \
                 retire the published incarnation with `dmux repair retire-incarnation` and \
                 re-bootstrap the live server",
                published.server_pid,
                published.server_start_token,
                published.socket_dev,
                published.socket_ino,
                live.identity.pid,
                live.identity.start_token,
                live.socket_path,
                live.socket_dev,
                live.socket_ino
            ),
        ));
    }
    Ok(())
}

fn typed_context_provider(error: crate::backend::ProviderError) -> TypedError {
    use crate::backend::ProviderError;
    let code = match error {
        ProviderError::EpochChanged { .. } => ErrorCode::BackendEpochChanged,
        ProviderError::WrongInstance { .. } => ErrorCode::WrongBackendInstance,
        ProviderError::NotFound { .. } => ErrorCode::NotFound,
        ProviderError::MultiWindow { .. } => ErrorCode::RepairRequired,
        ProviderError::Timeout { .. } => ErrorCode::ProviderUnavailable,
        ProviderError::NativeFailure { .. } | ProviderError::PostconditionFailed { .. } => {
            ErrorCode::OperationFailed
        }
    };
    TypedError::new(code, format!("tmux client identity: {error:?}"))
}

fn typed_context_operation(error: crate::operations::OpError) -> TypedError {
    use crate::operations::OpError;
    let code = match &error {
        OpError::StaleRef(_) => ErrorCode::BackendEpochChanged,
        OpError::NotFound(_) => ErrorCode::NotFound,
        OpError::Indeterminate(_) => ErrorCode::ProviderUnavailable,
        OpError::Refused(_) | OpError::NameConflict(_) => ErrorCode::IdentityConflict,
        OpError::Registry(detail) if detail.contains("registry busy") => ErrorCode::RegistryBusy,
        OpError::Bootstrap(_) | OpError::Lock(_) | OpError::Provider(_) | OpError::Registry(_) => {
            ErrorCode::OperationFailed
        }
    };
    TypedError::new(code, format!("tmux client context: {error}"))
}

/// Resolve, fence, and present exactly one existing Space.  No error path
/// calls a second backend and this module has no create capability.
pub fn connect(
    request: &ConnectRequest,
    client: &ConnectClientContext,
    history: &dyn ConnectHistory,
    authority: &mut dyn ConnectAuthority,
    presenter: &mut dyn ConnectPresenter,
) -> Result<ConnectOutcome, TypedError> {
    let mut runtime = SplitConnectRuntime {
        authority,
        presenter,
    };
    connect_with_runtime(request, client, history, &mut runtime)
}

/// One-adapter form used by the production CLI and by `new` after it has
/// selected/created an exact identity.  Sequential calls on one mutable
/// runtime avoid duplicate state and the Rust double-borrow trap.
pub fn connect_with_runtime<R: ConnectRuntime + ?Sized>(
    request: &ConnectRequest,
    client: &ConnectClientContext,
    history: &dyn ConnectHistory,
    runtime: &mut R,
) -> Result<ConnectOutcome, TypedError> {
    preflight_connect_request(request)?;

    let local_host = runtime.local_host_uid()?;
    let (owner, locator, requested_child) =
        resolve_request_scope(request, local_host, history, runtime)?;
    let backend_filter = matches!(&locator, OwnerLocator::Name(_))
        .then_some(request.backend_constraint)
        .flatten();
    let query = OwnerConnectQuery {
        owner,
        locator,
        backend_filter,
        child: requested_child,
    };

    // The authority returns determinate scan failures directly.  There is no
    // catch-and-try-the-other-backend branch here.
    let first = runtime.resolve_live(&query)?;
    validate_resolved_target(&query, request.backend_constraint, &first)?;
    if request.launch_gui && first.backend == Backend::Tmux {
        return Err(TypedError::new(
            ErrorCode::Usage,
            "--launch-gui cannot present a tmux Space",
        ));
    }

    let target = runtime.revalidate_live(&first)?;
    validate_resolved_target(&query, request.backend_constraint, &target)?;
    require_same_frozen_target(&first, &target)?;

    match target.backend {
        Backend::Wez => {
            let expected = if request.launch_gui {
                PresentationMode::WezCold
            } else {
                PresentationMode::WezAmbient
            };
            let receipt = if request.launch_gui {
                runtime.present_wez_cold(&target)?
            } else {
                runtime.present_wez_ambient(&target)?
            };
            validate_receipt(&target, expected, &receipt)?;
            Ok(ConnectOutcome::Completed(receipt))
        }
        Backend::Tmux => {
            if target.owner == local_host {
                let kind = match client.tmux_client {
                    None => TmuxExecKind::LocalAttach,
                    Some(current)
                        if current.owner == target.owner
                            && current.backend_instance_uid == target.backend_instance_uid
                            && current.server_epoch == target.server_epoch =>
                    {
                        TmuxExecKind::LocalSwitch
                    }
                    Some(current) => {
                        return Err(TypedError::new(
                            ErrorCode::WrongBackendInstance,
                            format!(
                                "current tmux client is bound to owner {}/instance {}/epoch {}; \
                                 target is {}/{}/{} and cannot be nested",
                                current.owner.0,
                                current.backend_instance_uid.0,
                                current.server_epoch.0,
                                target.owner.0,
                                target.backend_instance_uid.0,
                                target.server_epoch.0
                            ),
                        ));
                    }
                };
                let plan = runtime.prepare_local_tmux(&target, kind)?;
                plan.validate_against(&target, kind)?;
                Ok(ConnectOutcome::Exec(plan))
            } else {
                let plan = runtime.prepare_remote_tmux(&target)?;
                plan.validate_against(&target, TmuxExecKind::RemoteAttach)?;
                Ok(ConnectOutcome::Exec(plan))
            }
        }
    }
}

/// Apply a terminal handoff's stable history intent immediately before
/// replacing the current process with its validated argv.
pub fn commit_exec_history(history: &History, plan: &OwnerExecPlan) -> Result<(), TypedError> {
    let intent = plan.history_intent();
    history
        .record_attach(intent.host_uid, intent.space_uid)
        .map_err(|error| {
            TypedError::new(
                ErrorCode::OperationFailed,
                format!("recording stable attach history: {error}"),
            )
        })
}

/// The owner and exact locator a request names, scoped by the §6.2
/// resolver (`resolve::scope_space_ref`, ADR 012 WS-D.3): an encoded owner
/// wins, then `--host`, then the local authority; `--name` is the literal
/// escape on the explicit-or-local owner; `dmux -` is the stable history
/// of that same default owner. Host tokens resolve through the authority.
fn resolve_request_scope<A: ConnectAuthority + ?Sized>(
    request: &ConnectRequest,
    local_host: HostUid,
    history: &dyn ConnectHistory,
    authority: &mut A,
) -> Result<(HostUid, OwnerLocator, Option<RequestedChild>), TypedError> {
    let explicit = request
        .explicit_host
        .as_ref()
        .map(|selector| authority.resolve_host(selector))
        .transpose()?;
    let context = HostContext {
        local: local_host,
        explicit,
    };

    let (scoped, embedded_child) = match &request.selector {
        ConnectSelector::ExactName(name) => (
            scope_space_ref(SpaceSelector::ExactName(name), context, |token| {
                authority.resolve_host(&HostSelector::from(token))
            })?,
            None,
        ),
        ConnectSelector::Previous => {
            let owner = context.default_owner();
            let previous = history.previous(owner).ok_or_else(|| {
                TypedError::new(
                    ErrorCode::NotFound,
                    format!("no previous Space is recorded for owner {}", owner.0),
                )
            })?;
            return Ok((owner, OwnerLocator::Uid(previous), request.child.clone()));
        }
        ConnectSelector::Ref(reference) => {
            let parsed = parse_connect_ref(reference)?;
            let scoped = scope_space_ref(SpaceSelector::Shape(&parsed.space), context, |token| {
                authority.resolve_host(&HostSelector::from(token))
            })?;
            (scoped, parsed.child.map(RequestedChild::from))
        }
    };
    let child = merge_requested_child(embedded_child, request.child.clone())?;
    Ok((scoped.owner, scoped.locator, child))
}

fn parse_connect_ref(reference: &str) -> Result<crate::refs::ParsedRef, TypedError> {
    parse_ref(reference).map_err(|error| {
        TypedError::new(
            ErrorCode::InvalidRef,
            format!("invalid Space ref {reference:?}: {error:?}"),
        )
    })
}

fn merge_requested_child(
    embedded: Option<RequestedChild>,
    explicit: Option<RequestedChild>,
) -> Result<Option<RequestedChild>, TypedError> {
    match (embedded, explicit) {
        (Some(left), Some(right)) if left != right => Err(TypedError::new(
            ErrorCode::InvalidRef,
            "reference child suffix contradicts the explicit child target",
        )),
        (Some(left), _) => Ok(Some(left)),
        (_, Some(right)) => Ok(Some(right)),
        (None, None) => Ok(None),
    }
}

fn validate_resolved_target(
    query: &OwnerConnectQuery,
    backend_constraint: Option<Backend>,
    target: &FrozenConnectTarget,
) -> Result<(), TypedError> {
    validate_target_shape(target)?;
    if target.owner != query.owner {
        return Err(TypedError::new(
            ErrorCode::HostIdentityChanged,
            format!(
                "owner returned HostUid {} for query scoped to {}",
                target.owner.0, query.owner.0
            ),
        ));
    }
    let locator_matches = match &query.locator {
        OwnerLocator::Uid(uid) => target.space_uid == *uid,
        OwnerLocator::Number(no) => target.space_no == *no,
        OwnerLocator::Name(name) => target.logical_name == *name,
    };
    if !locator_matches {
        return Err(protocol_error(
            "owner returned a Space that does not match the exact locator",
        ));
    }
    if let Some(expected) = backend_constraint
        && target.backend != expected
    {
        return Err(TypedError::new(
            ErrorCode::BackendMismatch,
            format!(
                "resolved stable Space is {}; --backend requested {expected}",
                target.backend
            ),
        ));
    }
    if target.child.as_ref().map(verified_as_requested) != query.child.clone() {
        return Err(protocol_error(
            "owner returned a child other than the exact requested child",
        ));
    }
    Ok(())
}

fn verified_as_requested(child: &VerifiedConnectChild) -> RequestedChild {
    RequestedChild {
        kind: child.kind(),
        epoch: child.epoch(),
        handle: child.requested_handle().clone(),
    }
}

fn protocol_child_as_verified(child: &AttachPlanChild) -> VerifiedConnectChild {
    match child {
        AttachPlanChild::Group { epoch, handle } => VerifiedConnectChild::Group {
            epoch: *epoch,
            handle: handle.clone(),
        },
        AttachPlanChild::Split {
            epoch,
            group,
            split,
        } => VerifiedConnectChild::Split {
            epoch: *epoch,
            group: group.clone(),
            split: split.clone(),
        },
    }
}

fn validate_target_shape(target: &FrozenConnectTarget) -> Result<(), TypedError> {
    if target.binding.native_token.is_empty()
        || target.binding.native_token.contains('\0')
        || target.binding.endpoint.is_empty()
        || target.binding.endpoint.contains('\0')
    {
        return Err(protocol_error(
            "owner target has an empty or invalid binding/endpoint",
        ));
    }
    if let Some(child) = &target.child {
        if child.epoch() != target.server_epoch {
            return Err(TypedError::new(
                ErrorCode::BackendEpochChanged,
                "child epoch differs from the frozen backend server epoch",
            ));
        }
        validate_handle_backend(target.backend, child.requested_handle())?;
        if let VerifiedConnectChild::Split { group, .. } = child {
            validate_handle_backend(target.backend, group)?;
        }
    }
    Ok(())
}

fn validate_handle_backend(backend: Backend, handle: &ProviderHandle) -> Result<(), TypedError> {
    let matches = matches!(
        (backend, handle),
        (Backend::Wez, ProviderHandle::Wz(_))
            | (Backend::Tmux, ProviderHandle::Tx(_))
            | (_, ProviderHandle::Opaque(_))
    );
    if matches {
        Ok(())
    } else {
        Err(protocol_error(format!(
            "{handle} is not a {backend} child handle"
        )))
    }
}

fn require_same_frozen_target(
    before: &FrozenConnectTarget,
    after: &FrozenConnectTarget,
) -> Result<(), TypedError> {
    if before.server_epoch != after.server_epoch {
        return Err(TypedError::new(
            ErrorCode::BackendEpochChanged,
            format!(
                "backend epoch changed from {} to {} during presentation preflight",
                before.server_epoch.0, after.server_epoch.0
            ),
        ));
    }
    if before.backend_instance_uid != after.backend_instance_uid {
        return Err(TypedError::new(
            ErrorCode::WrongBackendInstance,
            "backend instance changed during presentation preflight",
        ));
    }
    if before != after {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "Space binding, identity, name, or child changed during presentation preflight",
        ));
    }
    Ok(())
}

fn validate_receipt(
    target: &FrozenConnectTarget,
    expected: PresentationMode,
    receipt: &PresentationReceipt,
) -> Result<(), TypedError> {
    if receipt.target != *target || receipt.mode != expected {
        return Err(protocol_error(
            "GUI acknowledgement target or presentation mode differs from the frozen request",
        ));
    }
    if receipt.acknowledgement.trim().is_empty() || receipt.acknowledgement.contains('\0') {
        return Err(protocol_error("GUI acknowledgement identifier is invalid"));
    }
    Ok(())
}

fn validate_argv_common(argv: &[String]) -> Result<(), TypedError> {
    if argv.is_empty() || argv[0].is_empty() || argv.iter().any(|arg| arg.contains('\0')) {
        return Err(protocol_error(
            "exec argv is empty or contains an invalid atom",
        ));
    }
    Ok(())
}

fn executable_basename_is(program: &str, expected: &str) -> bool {
    Path::new(program)
        .file_name()
        .is_some_and(|name| name == expected)
}

fn contains_pair(argv: &[String], left: &str, right: &str) -> bool {
    argv.windows(2)
        .any(|window| window[0] == left && window[1] == right)
}

fn validate_local_tmux_argv(
    target: &FrozenConnectTarget,
    kind: TmuxExecKind,
    argv: &[String],
) -> Result<(), TypedError> {
    if target.backend != Backend::Tmux {
        return Err(protocol_error("tmux exec plan targets a non-tmux Space"));
    }
    validate_argv_common(argv)?;
    if !executable_basename_is(&argv[0], "tmux") {
        return Err(protocol_error(
            "local tmux plan does not exec tmux directly",
        ));
    }
    if !contains_pair(argv, "-L", &target.binding.endpoint)
        || !contains_pair(argv, "-t", &target.binding.native_token)
    {
        return Err(protocol_error(
            "local tmux plan does not carry the frozen namespace/session target",
        ));
    }
    let verb_ok = match kind {
        TmuxExecKind::LocalAttach => argv
            .iter()
            .any(|arg| arg == "attach" || arg == "attach-session"),
        TmuxExecKind::LocalSwitch => argv.iter().any(|arg| arg == "switch-client"),
        TmuxExecKind::RemoteAttach => false,
    };
    if !verb_ok {
        return Err(protocol_error(format!(
            "local tmux argv does not implement {kind:?}"
        )));
    }
    let expected_verb = match kind {
        TmuxExecKind::LocalAttach => ["attach", "attach-session"].as_slice(),
        TmuxExecKind::LocalSwitch => ["switch-client", "switch-client"].as_slice(),
        TmuxExecKind::RemoteAttach => unreachable!(),
    };
    let tail_ok = argv.len() >= 3
        && expected_verb
            .iter()
            .any(|verb| argv[argv.len() - 3] == *verb)
        && argv[argv.len() - 2] == "-t"
        && argv[argv.len() - 1] == target.binding.native_token;
    if !tail_ok {
        return Err(protocol_error(
            "local tmux argv does not finish with the exact attach/switch session target",
        ));
    }
    validate_exact_tmux_child_focus(target, argv)?;
    Ok(())
}

fn append_exact_tmux_child_focus(
    argv: &mut Vec<String>,
    target: &FrozenConnectTarget,
) -> Result<(), TypedError> {
    match target.child.as_ref() {
        None => Ok(()),
        Some(VerifiedConnectChild::Group {
            handle: ProviderHandle::Tx(window),
            ..
        }) => {
            argv.extend([
                "select-window".to_string(),
                "-t".to_string(),
                format!("{}:@{window}", target.binding.native_token),
                ";".to_string(),
            ]);
            Ok(())
        }
        Some(VerifiedConnectChild::Split {
            group: ProviderHandle::Tx(window),
            split: ProviderHandle::Tx(pane),
            ..
        }) => {
            argv.extend([
                "select-window".to_string(),
                "-t".to_string(),
                format!("{}:@{window}", target.binding.native_token),
                ";".to_string(),
                "select-pane".to_string(),
                "-t".to_string(),
                format!("%{pane}"),
                ";".to_string(),
            ]);
            Ok(())
        }
        Some(_) => Err(protocol_error(
            "owner-correlated tmux child has a non-tmux native handle",
        )),
    }
}

fn validate_exact_tmux_child_focus(
    target: &FrozenConnectTarget,
    argv: &[String],
) -> Result<(), TypedError> {
    let select_window_count = argv.iter().filter(|arg| *arg == "select-window").count();
    let select_pane_count = argv.iter().filter(|arg| *arg == "select-pane").count();
    let expected_tail = match target.child.as_ref() {
        None => {
            if select_window_count != 0 || select_pane_count != 0 {
                return Err(protocol_error(
                    "Space-only tmux plan contains an unrequested child focus",
                ));
            }
            return Ok(());
        }
        Some(VerifiedConnectChild::Group {
            handle: ProviderHandle::Tx(window),
            ..
        }) => {
            if select_window_count != 1 || select_pane_count != 0 {
                return Err(protocol_error(
                    "Group tmux plan does not select exactly one window",
                ));
            }
            vec![format!("{}:@{window}", target.binding.native_token)]
        }
        Some(VerifiedConnectChild::Split {
            group: ProviderHandle::Tx(window),
            split: ProviderHandle::Tx(pane),
            ..
        }) => {
            if select_window_count != 1 || select_pane_count != 1 {
                return Err(protocol_error(
                    "Split tmux plan does not select exactly one window and pane",
                ));
            }
            vec![
                format!("{}:@{window}", target.binding.native_token),
                format!("%{pane}"),
            ]
        }
        Some(_) => {
            return Err(protocol_error(
                "tmux child focus contains a non-tmux native handle",
            ));
        }
    };
    if !contains_pair(argv, "select-window", "-t") {
        return Err(protocol_error("tmux child plan omitted select-window -t"));
    }
    let window_ok = argv.windows(3).any(|window| {
        window[0] == "select-window" && window[1] == "-t" && window[2] == expected_tail[0]
    });
    if !window_ok {
        return Err(protocol_error(
            "tmux child plan selected a different exact window",
        ));
    }
    if expected_tail.len() == 2
        && !argv.windows(3).any(|window| {
            window[0] == "select-pane" && window[1] == "-t" && window[2] == expected_tail[1]
        })
    {
        return Err(protocol_error(
            "tmux child plan selected a different exact pane",
        ));
    }
    // Focus runs before the final attach/switch, so the terminal handoff
    // opens on the requested child and the final native atom remains the
    // bound session token (the same rebind proof `_attach` uses remotely).
    if argv.last() != Some(&target.binding.native_token) {
        return Err(protocol_error(
            "tmux child plan does not finish at the frozen session binding",
        ));
    }
    Ok(())
}

fn validate_remote_tmux_argv(
    target: &FrozenConnectTarget,
    argv: &[String],
    remote: &RemoteAttachWitness,
) -> Result<(), TypedError> {
    if target.backend != Backend::Tmux {
        return Err(protocol_error(
            "remote attach plan targets a non-tmux Space",
        ));
    }
    validate_argv_common(argv)?;
    if remote.host_uid != target.owner
        || remote.space_uid != target.space_uid
        || remote.backend_instance_uid != target.backend_instance_uid
        || remote.server_epoch != target.server_epoch
    {
        return Err(protocol_error(
            "remote attach token witness differs from the frozen target",
        ));
    }
    if remote.route_id <= 0
        || remote.route.trim().is_empty()
        || remote.destination.trim().is_empty()
        || remote.expires_at.trim().is_empty()
        || remote.token.len() < 32
        || remote.token.chars().any(char::is_whitespace)
        || remote.token.contains('\0')
    {
        return Err(protocol_error(
            "remote attach plan has an invalid winning route, expiry, or token",
        ));
    }
    if !executable_basename_is(&argv[0], "ssh")
        || !argv.iter().any(|arg| arg == "-t" || arg == "-tt")
        || !argv.iter().any(|arg| arg == &remote.destination)
        || !contains_pair(argv, "--token", &remote.token)
        || !argv.iter().any(|arg| arg == "_attach")
    {
        return Err(protocol_error(
            "remote tmux argv is not the verified ssh PTY `_attach --token` channel",
        ));
    }
    // The bounded RPC returns only a token.  A native target appearing in
    // the streaming argv would reintroduce the forbidden client-built path.
    if argv.iter().any(|arg| arg == &target.binding.native_token) {
        return Err(protocol_error(
            "remote tmux argv leaks/accepts a native session target",
        ));
    }
    Ok(())
}

fn protocol_error(message: impl Into<String>) -> TypedError {
    TypedError::new(ErrorCode::ProtocolMismatch, message)
}
