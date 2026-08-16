//! Typed create-or-connect orchestration for public `dmux new`.
//!
//! The owner remains the only authority that may scan or mutate native
//! backends.  This layer freezes host/backend policy, orders presentation
//! preflight before reservation, invokes one owner create method, and then
//! reuses the non-creating `connect_cli` handoff.  There is deliberately no
//! "try the other backend" branch.

use std::num::NonZeroU64;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::backend::{InventoryScope, Provider};
use crate::connect_cli::{
    ConnectAuthority, ConnectClientContext, ConnectHistory, ConnectOutcome, ConnectPresenter,
    ConnectRequest, ConnectSelector, FrozenConnectTarget, HostSelector, OwnerConnectQuery,
    OwnerExecPlan, OwnerLocator, PresentationReceipt, ProductionConnectAdapter,
};
use crate::error::{ErrorCode, TypedError};
use crate::history::History;
use crate::model::{Backend, BackendInstanceUid, HostUid, ServerEpoch, SpaceNo, SpaceUid};
use crate::operations::{
    CreateRequest, CreatedSpace, OpError, OperationEnv, OwnerCreateTarget,
    create_space_owner_fenced, lookup_new_owner_fenced,
};
use crate::policy::{CreationContext, LocalEnv, NewPlan, plan_new};
use crate::refs::canonical_uri;
use crate::registry::{Registry, RegistryConfig};
use crate::remote::client::{
    AgentInvocation, DEFAULT_DEADLINE, PeerExpectation, RouteInvoker, SshInvoker, call_over_routes,
    request_envelope,
};
use crate::remote::protocol::{
    self, Envelope, NewLookupBlockReason, NewLookupClass, NewLookupPayload, NewLookupResult,
    NewPayload,
};
use crate::resolve::{ClassSummary, NewLookup, lookup_for_new};

/// Public `new` request after clap-level parsing. `name` is always literal
/// exact bytes during lookup; managed-name grammar applies only if policy
/// reaches creation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explicit_host: Option<HostSelector>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_constraint: Option<Backend>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default)]
    pub no_connect: bool,
    #[serde(default)]
    pub allow_name_collision: bool,
    #[serde(default)]
    pub launch_gui: bool,
    /// User program argv; empty means the owner's login shell.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub program: Vec<String>,
}

/// Both owner-side exact-name partitions from one decision-fenced lookup.
/// The authority must obtain both inventories (or their typed indeterminate
/// outcomes) and join them to durable registry state; discovery never adopts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewLookupSnapshot {
    pub wez: ClassSummary,
    pub tmux: ClassSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NewPresentationMode {
    Ambient,
    Cold,
}

/// Capability frozen before identity reservation for a connecting Wez
/// create.  The GUI adapter proves the exact owner backend incarnation,
/// route/domain, build compatibility and uniquely bound GUI.  The owner
/// create adapter must reject a witness whose instance/epoch changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WezPresentationPreflight {
    pub owner: HostUid,
    pub backend_instance_uid: BackendInstanceUid,
    pub server_epoch: ServerEpoch,
    pub gui_instance: String,
    pub domain: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternate_domains: Vec<String>,
    pub mode: NewPresentationMode,
}

/// Mutation payload sent to exactly one owner. Native endpoints, helper
/// paths, server epochs and provider handles are intentionally absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerNewRequest {
    pub request_uid: Uuid,
    pub owner: HostUid,
    pub backend: Backend,
    pub name: String,
    pub cwd: Option<String>,
    pub program: Vec<String>,
    /// Valid only with an explicit backend. The owner may waive one
    /// opposite *selectable managed* same-name row; unmanaged, blocking or
    /// indeterminate opposite state remains a refusal.
    pub allow_name_collision: bool,
    pub presentation: Option<WezPresentationPreflight>,
}

/// Stable result retained even when post-create presentation fails.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewReceipt {
    pub created: bool,
    pub connected: bool,
    pub stable_ref: String,
    pub owner: HostUid,
    pub space_uid: SpaceUid,
    pub space_no: SpaceNo,
    pub backend: Backend,
    #[serde(default)]
    pub replayed: bool,
}

impl NewReceipt {
    fn selected(target: &FrozenConnectTarget, connected: bool) -> Self {
        NewReceipt {
            created: false,
            connected,
            stable_ref: canonical_uri(target.owner, target.space_uid),
            owner: target.owner,
            space_uid: target.space_uid,
            space_no: target.space_no,
            backend: target.backend,
            replayed: false,
        }
    }

    fn created(owner: HostUid, created: &CreatedSpace, connected: bool) -> Self {
        NewReceipt {
            created: true,
            connected,
            stable_ref: canonical_uri(owner, created.space_uid),
            owner,
            space_uid: created.space_uid,
            space_no: created.space_no,
            backend: created.backend,
            replayed: created.replayed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewOutcome {
    /// Bounded result (`--no-connect`) or acknowledged Wez presentation.
    Completed {
        result: NewReceipt,
        presentation: Option<PresentationReceipt>,
    },
    /// Validated tmux terminal handoff. The caller commits its stable history
    /// intent and execs exactly this argv; it must not render a success first.
    Exec {
        result: NewReceipt,
        plan: Box<OwnerExecPlan>,
    },
}

/// An error may retain a result only after durable creation. Such a failure
/// is always rewritten to `partial_result` (exit 7) and never triggers
/// tombstoning, retry on another backend, or another native create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewFailure {
    pub error: TypedError,
    pub result: Option<Box<NewReceipt>>,
}

impl From<TypedError> for NewFailure {
    fn from(error: TypedError) -> Self {
        NewFailure {
            error,
            result: None,
        }
    }
}

/// Owner/routing seams specific to `new`. The lookup implementation owns the
/// per-logical-name decision lease. `create_owner` executes locally through
/// `operations::create_space_owner_fenced` or remotely through protocol NEW,
/// always using `OwnerNewRequest.request_uid` end to end.
pub trait NewAuthority: ConnectAuthority {
    fn lookup_exact(&mut self, owner: HostUid, name: &str)
    -> Result<NewLookupSnapshot, TypedError>;

    /// Return freshly verified §8.3 facts. Implementations must set
    /// `explicit_backend` to the supplied constraint exactly.
    fn creation_context(
        &mut self,
        owner: HostUid,
        explicit_backend: Option<Backend>,
        launch_gui: bool,
    ) -> Result<CreationContext, TypedError>;

    /// Prove the selected Wez presentation path before reservation. Ambient
    /// mode requires a trusted live bridge; cold mode is used only after an
    /// explicit `--launch-gui` request.
    fn preflight_wez_presentation(
        &mut self,
        owner: HostUid,
        mode: NewPresentationMode,
    ) -> Result<WezPresentationPreflight, TypedError>;

    /// Invoke exactly one owner. A successful replay returns the original
    /// `CreatedSpace`; an uncertain remote acknowledgement is retried only by
    /// the transport layer with this same request UID.
    fn create_owner(&mut self, request: &OwnerNewRequest) -> Result<CreatedSpace, TypedError>;
}

/// A single runtime implements both connect traits, avoiding aliasing one
/// mutable adapter into separate authority/presenter references. The
/// orchestrator passes it directly to `connect_cli::connect_with_runtime`.
pub trait NewRuntime: NewAuthority + ConnectPresenter {}

impl<T: NewAuthority + ConnectPresenter + ?Sized> NewRuntime for T {}

struct OwnedOwnerTarget {
    backend: Backend,
    instance: BackendInstanceUid,
    provider: Box<dyn Provider>,
    scope: InventoryScope,
}

impl OwnedOwnerTarget {
    fn borrowed(&self) -> OwnerCreateTarget<'_> {
        OwnerCreateTarget {
            backend: self.backend,
            instance: self.instance,
            provider: self.provider.as_ref(),
            scope: &self.scope,
        }
    }
}

/// Production runtime shared by `new` and its exact connect handoff.
pub struct ProductionNewRuntime<I = SshInvoker> {
    connect: ProductionConnectAdapter<I>,
    env: OperationEnv,
    invoker: I,
    remote_bin: String,
    helper_bin: String,
    wezterm_bin: String,
    wezterm_config: String,
}

impl ProductionNewRuntime<SshInvoker> {
    pub fn production() -> Result<Self, TypedError> {
        let env = OperationEnv::production()
            .map_err(|error| TypedError::new(ErrorCode::OperationFailed, error.to_string()))?;
        let invoker = SshInvoker::default();
        let connect = ProductionConnectAdapter::with_invoker(
            OperationEnv {
                db_path: env.db_path.clone(),
                lock_dir: env.lock_dir.clone(),
            },
            invoker.clone(),
            "dmux",
        );
        let helper_bin = std::env::current_exe()
            .ok()
            .map(|path| path.with_file_name("pane-bootstrap"))
            .filter(|path| path.exists())
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "pane-bootstrap".into());
        let (wezterm_bin, wezterm_config) = crate::runtime::production_wez_paths();
        Ok(ProductionNewRuntime {
            connect,
            env,
            invoker,
            remote_bin: "dmux".into(),
            helper_bin,
            wezterm_bin,
            wezterm_config,
        })
    }
}

impl<I: RouteInvoker + Clone> ProductionNewRuntime<I> {
    #[allow(clippy::too_many_arguments)]
    pub fn with_dependencies(
        env: OperationEnv,
        _runtime_dir: PathBuf,
        invoker: I,
        remote_bin: impl Into<String>,
        helper_bin: impl Into<String>,
        wezterm_bin: impl Into<String>,
        wezterm_config: impl Into<String>,
    ) -> Self {
        let remote_bin = remote_bin.into();
        let connect = ProductionConnectAdapter::with_invoker(
            OperationEnv {
                db_path: env.db_path.clone(),
                lock_dir: env.lock_dir.clone(),
            },
            invoker.clone(),
            remote_bin.clone(),
        );
        ProductionNewRuntime {
            connect,
            env,
            invoker,
            remote_bin,
            helper_bin: helper_bin.into(),
            wezterm_bin: wezterm_bin.into(),
            wezterm_config: wezterm_config.into(),
        }
    }

    fn registry(&self) -> Result<Registry, TypedError> {
        Registry::open(RegistryConfig::new(&self.env.db_path, &self.env.lock_dir))
            .map_err(|error| TypedError::new(error.error_code(), error.to_string()))
    }

    fn is_local_owner(&self, owner: HostUid) -> Result<bool, TypedError> {
        Ok(self
            .registry()?
            .identity()
            .map_err(|error| TypedError::new(error.error_code(), error.to_string()))?
            .host_uid
            == owner)
    }

    fn local_target(&self, backend: Backend) -> Result<Option<OwnedOwnerTarget>, TypedError> {
        let registry = self.registry()?;
        let Some(instance) = registry
            .backend_instance_for_backend(backend)
            .map_err(|error| TypedError::new(error.error_code(), error.to_string()))?
        else {
            return Ok(None);
        };
        let info = registry
            .backend_instance_info(instance)
            .map_err(|error| TypedError::new(error.error_code(), error.to_string()))?;
        let endpoint = info.socket_path.ok_or_else(|| {
            TypedError::new(
                ErrorCode::ProviderUnavailable,
                format!("managed {backend} instance has no recorded endpoint"),
            )
        })?;
        let expected_epoch = registry
            .backend_server(instance)
            .map_err(|error| TypedError::new(error.error_code(), error.to_string()))?
            .server_epoch;
        let provider: Box<dyn Provider> = match backend {
            Backend::Tmux => Box::new(crate::backend::tmux::TmuxProvider::new(endpoint.clone())),
            Backend::Wez => Box::new(crate::backend::wez::WezProvider::new(
                &self.wezterm_bin,
                self.wezterm_config.clone(),
            )),
        };
        Ok(Some(OwnedOwnerTarget {
            backend,
            instance,
            provider,
            scope: InventoryScope {
                backend,
                endpoint,
                expected_epoch,
            },
        }))
    }

    fn remote_call<T: for<'de> Deserialize<'de>>(
        &self,
        owner: HostUid,
        method: &str,
        request_uid: Uuid,
        payload: serde_json::Value,
        claimed: Option<(BackendInstanceUid, ServerEpoch)>,
    ) -> Result<(T, Envelope), TypedError> {
        let mut registry = self.registry()?;
        let identity = registry
            .identity()
            .map_err(|error| TypedError::new(error.error_code(), error.to_string()))?;
        let head = registry
            .authority_head()
            .map_err(|error| TypedError::new(error.error_code(), error.to_string()))?;
        let mut request = request_envelope(&identity, &head, method, request_uid, payload);
        if let Some((instance, epoch)) = claimed {
            request.backend_instance_uid = Some(instance);
            request.server_epoch = Some(epoch);
        }
        let mut invocation = AgentInvocation::new(method);
        invocation.remote_bin = self.remote_bin.clone();
        let outcome = call_over_routes(
            &mut registry,
            &PeerExpectation {
                host_uid: owner,
                need_capability: match method {
                    protocol::methods::NEW_LOOKUP => Some(protocol::CAP_NEW_LOOKUP.to_string()),
                    protocol::methods::NEW => Some(protocol::CAP_NEW_FENCED_COLLISION.to_string()),
                    _ => None,
                },
                claimed_current: false,
            },
            &request,
            &self.invoker,
            &invocation,
            DEFAULT_DEADLINE,
        )?;
        if outcome.envelope.method != method || outcome.envelope.request_uid != request_uid {
            return Err(TypedError::new(
                ErrorCode::ProtocolMismatch,
                "owner response changed method/request UID",
            ));
        }
        let required_response_capability = match method {
            protocol::methods::NEW_LOOKUP => Some(protocol::CAP_NEW_LOOKUP),
            protocol::methods::NEW => Some(protocol::CAP_NEW_FENCED_COLLISION),
            _ => None,
        };
        if let Some(required) = required_response_capability
            && !outcome
                .envelope
                .capabilities
                .iter()
                .any(|capability| capability == required)
        {
            return Err(TypedError::new(
                ErrorCode::VersionMismatch,
                format!("owner response lacks required capability {required}"),
            ));
        }
        if let Some((instance, epoch)) = claimed
            && (outcome.envelope.backend_instance_uid != Some(instance)
                || outcome.envelope.server_epoch != Some(epoch))
        {
            return Err(TypedError::new(
                ErrorCode::BackendEpochChanged,
                "owner response changed the claimed backend instance/epoch",
            ));
        }
        let parsed = serde_json::from_value(outcome.envelope.payload.clone().ok_or_else(|| {
            TypedError::new(
                ErrorCode::ProtocolMismatch,
                "successful owner response omitted payload",
            )
        })?)
        .map_err(|error| {
            TypedError::new(
                ErrorCode::ProtocolMismatch,
                format!("owner {method} payload: {error}"),
            )
        })?;
        Ok((parsed, outcome.envelope))
    }
}

impl<I: RouteInvoker + Clone> ConnectAuthority for ProductionNewRuntime<I> {
    fn local_host_uid(&mut self) -> Result<HostUid, TypedError> {
        ConnectAuthority::local_host_uid(&mut self.connect)
    }

    fn resolve_host(&mut self, selector: &HostSelector) -> Result<HostUid, TypedError> {
        ConnectAuthority::resolve_host(&mut self.connect, selector)
    }

    fn resolve_live(
        &mut self,
        query: &OwnerConnectQuery,
    ) -> Result<FrozenConnectTarget, TypedError> {
        ConnectAuthority::resolve_live(&mut self.connect, query)
    }

    fn revalidate_live(
        &mut self,
        target: &FrozenConnectTarget,
    ) -> Result<FrozenConnectTarget, TypedError> {
        ConnectAuthority::revalidate_live(&mut self.connect, target)
    }
}

impl<I: RouteInvoker + Clone> ConnectPresenter for ProductionNewRuntime<I> {
    fn present_wez_ambient(
        &mut self,
        target: &FrozenConnectTarget,
    ) -> Result<PresentationReceipt, TypedError> {
        ConnectPresenter::present_wez_ambient(&mut self.connect, target)
    }

    fn present_wez_cold(
        &mut self,
        target: &FrozenConnectTarget,
    ) -> Result<PresentationReceipt, TypedError> {
        ConnectPresenter::present_wez_cold(&mut self.connect, target)
    }

    fn prepare_local_tmux(
        &mut self,
        target: &FrozenConnectTarget,
        kind: crate::connect_cli::TmuxExecKind,
    ) -> Result<OwnerExecPlan, TypedError> {
        ConnectPresenter::prepare_local_tmux(&mut self.connect, target, kind)
    }

    fn prepare_remote_tmux(
        &mut self,
        target: &FrozenConnectTarget,
    ) -> Result<OwnerExecPlan, TypedError> {
        ConnectPresenter::prepare_remote_tmux(&mut self.connect, target)
    }
}

fn typed_operation(error: OpError) -> TypedError {
    let code = match &error {
        OpError::NameConflict(_) => ErrorCode::NameConflict,
        OpError::Indeterminate(_) => ErrorCode::ProviderUnavailable,
        OpError::NotFound(_) => ErrorCode::NotFound,
        OpError::Refused(_) => ErrorCode::OperationInProgress,
        OpError::StaleRef(_) => ErrorCode::BackendEpochChanged,
        OpError::Registry(detail) if detail.contains("registry busy") => ErrorCode::RegistryBusy,
        OpError::Bootstrap(_) | OpError::Lock(_) | OpError::Provider(_) | OpError::Registry(_) => {
            ErrorCode::OperationFailed
        }
    };
    TypedError::new(code, error.to_string())
}

fn block_from_wire(reason: NewLookupBlockReason) -> crate::resolve::BlockReason {
    use crate::resolve::BlockReason as B;
    match reason {
        NewLookupBlockReason::LifecycleReserved => B::LifecycleReserved,
        NewLookupBlockReason::LifecycleDeleting => B::LifecycleDeleting,
        NewLookupBlockReason::LifecycleConflict => B::LifecycleConflict,
        NewLookupBlockReason::OperationInProgress => B::OperationInProgress,
        NewLookupBlockReason::Unhealthy(health) => B::Unhealthy(health),
        NewLookupBlockReason::NoBinding => B::NoBinding,
        NewLookupBlockReason::ActiveAbsent => B::ActiveAbsent,
        NewLookupBlockReason::ServerStopped => B::ServerStopped,
        NewLookupBlockReason::IndeterminateObservation => B::IndeterminateObservation,
        NewLookupBlockReason::UnmanagedSameName => B::UnmanagedSameName,
        NewLookupBlockReason::MultiWindow => B::MultiWindow,
    }
}

fn class_from_wire(class: NewLookupClass) -> Result<ClassSummary, TypedError> {
    Ok(match class {
        NewLookupClass::NoMatch => ClassSummary::NoMatch,
        NewLookupClass::Indeterminate => ClassSummary::Indeterminate,
        NewLookupClass::Blocking {
            reason, space_uid, ..
        } => ClassSummary::Blocking {
            reason: block_from_wire(reason),
            space: space_uid,
        },
        NewLookupClass::Selectable {
            space_uid,
            space_no,
        } => ClassSummary::Selectable {
            space: space_uid,
            no: SpaceNo(NonZeroU64::new(space_no).ok_or_else(|| {
                TypedError::new(ErrorCode::ProtocolMismatch, "owner returned SpaceNo zero")
            })?),
        },
    })
}

impl<I: RouteInvoker + Clone> NewAuthority for ProductionNewRuntime<I> {
    fn lookup_exact(
        &mut self,
        owner: HostUid,
        name: &str,
    ) -> Result<NewLookupSnapshot, TypedError> {
        if self.is_local_owner(owner)? {
            let wez = self.local_target(Backend::Wez)?;
            let tmux = self.local_target(Backend::Tmux)?;
            let lookup = lookup_new_owner_fenced(
                &self.env,
                wez.as_ref().map(OwnedOwnerTarget::borrowed),
                tmux.as_ref().map(OwnedOwnerTarget::borrowed),
                name,
            )
            .map_err(typed_operation)?;
            return Ok(NewLookupSnapshot {
                wez: lookup.wez,
                tmux: lookup.tmux,
            });
        }
        let (lookup, _): (NewLookupResult, _) = self.remote_call(
            owner,
            protocol::methods::NEW_LOOKUP,
            Uuid::new_v4(),
            serde_json::to_value(NewLookupPayload { name: name.into() })
                .map_err(|error| TypedError::new(ErrorCode::OperationFailed, error.to_string()))?,
            None,
        )?;
        Ok(NewLookupSnapshot {
            wez: class_from_wire(lookup.wez)?,
            tmux: class_from_wire(lookup.tmux)?,
        })
    }

    fn creation_context(
        &mut self,
        owner: HostUid,
        explicit_backend: Option<Backend>,
        launch_gui: bool,
    ) -> Result<CreationContext, TypedError> {
        crate::gui_cli::new_creation_context_production(owner, explicit_backend, launch_gui)
    }

    fn preflight_wez_presentation(
        &mut self,
        owner: HostUid,
        mode: NewPresentationMode,
    ) -> Result<WezPresentationPreflight, TypedError> {
        crate::gui_cli::preflight_new_wez_presentation_production(owner, mode)
    }

    fn create_owner(&mut self, request: &OwnerNewRequest) -> Result<CreatedSpace, TypedError> {
        if self.is_local_owner(request.owner)? {
            let selected = self.local_target(request.backend)?.ok_or_else(|| {
                TypedError::new(
                    ErrorCode::ProviderUnavailable,
                    format!("owner has no managed {} instance", request.backend),
                )
            })?;
            if let Some(witness) = &request.presentation
                && (request.backend != Backend::Wez
                    || witness.owner != request.owner
                    || witness.backend_instance_uid != selected.instance
                    || Some(witness.server_epoch) != selected.scope.expected_epoch)
            {
                return Err(TypedError::new(
                    ErrorCode::BackendEpochChanged,
                    "presentation preflight does not match the selected local Wez incarnation",
                ));
            }
            let opposite_backend = match request.backend {
                Backend::Wez => Backend::Tmux,
                Backend::Tmux => Backend::Wez,
            };
            let opposite = self.local_target(opposite_backend)?;
            let cwd = request
                .cwd
                .as_deref()
                .map(|cwd| {
                    let path = std::fs::canonicalize(cwd).map_err(|error| {
                        TypedError::new(ErrorCode::NotFound, format!("--dir {cwd:?}: {error}"))
                    })?;
                    if !path.is_dir() {
                        return Err(TypedError::new(
                            ErrorCode::NotFound,
                            format!("--dir {cwd:?} is not a directory"),
                        ));
                    }
                    Ok(path.display().to_string())
                })
                .transpose()?;
            return create_space_owner_fenced(
                &self.env,
                selected.borrowed(),
                opposite.as_ref().map(OwnedOwnerTarget::borrowed),
                request.allow_name_collision,
                &CreateRequest {
                    request_uid: request.request_uid,
                    name: request.name.clone(),
                    cwd,
                    program: request.program.clone(),
                    helper_bin: self.helper_bin.clone(),
                },
            )
            .map_err(typed_operation);
        }
        let claimed = request
            .presentation
            .as_ref()
            .map(|witness| (witness.backend_instance_uid, witness.server_epoch));
        let (created, envelope): (CreatedSpace, _) = self.remote_call(
            request.owner,
            protocol::methods::NEW,
            request.request_uid,
            serde_json::to_value(NewPayload {
                name: request.name.clone(),
                backend: request.backend,
                cwd: request.cwd.clone(),
                program: request.program.clone(),
                allow_name_collision: request.allow_name_collision,
            })
            .map_err(|error| TypedError::new(ErrorCode::OperationFailed, error.to_string()))?,
            claimed,
        )?;
        if created.backend != request.backend {
            return Err(TypedError::new(
                ErrorCode::ProtocolMismatch,
                "remote owner NEW changed the selected backend",
            ));
        }
        if envelope.host_uid != request.owner {
            return Err(TypedError::new(
                ErrorCode::HostIdentityChanged,
                "remote owner NEW response changed owner identity",
            ));
        }
        Ok(created)
    }
}

/// Execute the §8.2 create-or-connect state machine.
pub fn create_or_connect(
    request: &NewRequest,
    client: &ConnectClientContext,
    history: &dyn ConnectHistory,
    runtime: &mut dyn NewRuntime,
) -> Result<NewOutcome, NewFailure> {
    validate_static_options(request)?;

    let local_host = runtime.local_host_uid()?;
    let owner = request
        .explicit_host
        .as_ref()
        .map(|selector| runtime.resolve_host(selector))
        .transpose()?
        .unwrap_or(local_host);

    let snapshot = runtime.lookup_exact(owner, &request.name)?;
    let lookup = lookup_for_new(
        request.backend_constraint,
        request.allow_name_collision,
        snapshot.wez,
        snapshot.tmux,
    );
    let explained = plan_lookup(request, runtime, owner, lookup)?;

    match explained.value {
        NewPlan::Fail(error) => Err(error.into()),
        NewPlan::Connect { backend, space } => {
            finish_existing(request, client, history, runtime, owner, backend, space)
        }
        NewPlan::Create { backend } => {
            if request.launch_gui && backend != Backend::Wez {
                return Err(TypedError::new(
                    ErrorCode::Usage,
                    "--launch-gui cannot create or present a tmux Space",
                )
                .into());
            }
            let presentation = if !request.no_connect && backend == Backend::Wez {
                let mode = if request.launch_gui {
                    NewPresentationMode::Cold
                } else {
                    NewPresentationMode::Ambient
                };
                let witness = runtime.preflight_wez_presentation(owner, mode)?;
                validate_preflight(owner, mode, &witness)?;
                Some(witness)
            } else {
                None
            };
            let owner_request = OwnerNewRequest {
                request_uid: Uuid::new_v4(),
                owner,
                backend,
                name: request.name.clone(),
                cwd: request.cwd.clone(),
                program: request.program.clone(),
                allow_name_collision: request.allow_name_collision,
                presentation,
            };
            match runtime.create_owner(&owner_request) {
                Ok(created) => {
                    finish_created(request, client, history, runtime, owner, backend, created)
                }
                Err(error) if error.code == ErrorCode::NameConflict => {
                    // A same-name decision waiter may have completed while
                    // this client was queued. Re-run the full owner lookup;
                    // only a now-selectable exact match can turn this into
                    // idempotent connect. Unmanaged/external races remain
                    // blocking/conflict and never cause another create.
                    finish_concurrent_winner(
                        request, client, history, runtime, owner, backend, error,
                    )
                }
                Err(error) => Err(error.into()),
            }
        }
    }
}

/// Complete production planner for the feature-on public command. A tmux
/// `Exec` outcome is still only a plan; main must call
/// `connect_cli::commit_production_exec_history` immediately before exec.
pub fn plan_new_production(request: &NewRequest) -> Result<NewOutcome, NewFailure> {
    let client = validated_client_context(request, || {
        crate::connect_cli::production_connect_client_context()
    })?;
    let state_dir = History::default_dir().ok_or_else(|| {
        NewFailure::from(TypedError::new(
            ErrorCode::OperationFailed,
            "HOME/XDG_STATE_HOME is unavailable for stable Space history",
        ))
    })?;
    let history = History::new(state_dir);
    let mut runtime = ProductionNewRuntime::production()?;
    create_or_connect(request, &client, &history, &mut runtime)
}

fn validated_client_context(
    request: &NewRequest,
    load_ambient: impl FnOnce() -> Result<ConnectClientContext, TypedError>,
) -> Result<ConnectClientContext, NewFailure> {
    // Static usage failures and bounded no-connect automation must not read
    // or validate ambient TMUX/TMUX_PANE/marker state.
    validate_static_options(request)?;
    if request.no_connect {
        Ok(ConnectClientContext::default())
    } else {
        load_ambient().map_err(NewFailure::from)
    }
}

fn validate_static_options(request: &NewRequest) -> Result<(), NewFailure> {
    if request.allow_name_collision && request.backend_constraint.is_none() {
        return Err(TypedError::new(
            ErrorCode::Usage,
            "--allow-name-collision requires explicit --backend wez|tmux",
        )
        .into());
    }
    if request.launch_gui && request.no_connect {
        return Err(
            TypedError::new(ErrorCode::Usage, "--launch-gui conflicts with --no-connect").into(),
        );
    }
    if request.launch_gui && request.backend_constraint == Some(Backend::Tmux) {
        return Err(TypedError::new(
            ErrorCode::Usage,
            "--launch-gui is valid only with the Wez backend",
        )
        .into());
    }
    Ok(())
}

fn validate_preflight(
    owner: HostUid,
    mode: NewPresentationMode,
    witness: &WezPresentationPreflight,
) -> Result<(), NewFailure> {
    if witness.owner != owner
        || witness.mode != mode
        || witness.gui_instance.is_empty()
        || witness.domain.is_empty()
        || witness.alternate_domains.iter().any(String::is_empty)
        || witness
            .alternate_domains
            .iter()
            .any(|alternate| alternate == &witness.domain)
    {
        return Err(protocol_failure(
            "Wez presentation preflight returned an invalid or differently scoped witness",
        ));
    }
    Ok(())
}

fn finish_existing(
    request: &NewRequest,
    client: &ConnectClientContext,
    history: &dyn ConnectHistory,
    runtime: &mut dyn NewRuntime,
    owner: HostUid,
    backend: Backend,
    space: SpaceUid,
) -> Result<NewOutcome, NewFailure> {
    if request.no_connect {
        let target = runtime.resolve_live(&OwnerConnectQuery {
            owner,
            locator: OwnerLocator::Uid(space),
            backend_filter: None,
            child: None,
        })?;
        require_selected(owner, &request.name, backend, space, &target)?;
        return Ok(NewOutcome::Completed {
            result: NewReceipt::selected(&target, false),
            presentation: None,
        });
    }
    let (outcome, target) = connect_selected(request, client, history, runtime, owner, space)?;
    require_selected(owner, &request.name, backend, space, &target)?;
    Ok(map_connect(NewReceipt::selected(&target, true), outcome))
}

fn finish_created(
    request: &NewRequest,
    client: &ConnectClientContext,
    history: &dyn ConnectHistory,
    runtime: &mut dyn NewRuntime,
    owner: HostUid,
    chosen_backend: Backend,
    created: CreatedSpace,
) -> Result<NewOutcome, NewFailure> {
    if created.backend != chosen_backend {
        return Err(protocol_failure(
            "owner create response changed the frozen backend",
        ));
    }
    let base = NewReceipt::created(owner, &created, false);
    if request.no_connect {
        return Ok(NewOutcome::Completed {
            result: base,
            presentation: None,
        });
    }
    match connect_selected(request, client, history, runtime, owner, created.space_uid) {
        Ok((outcome, target)) => {
            if target.owner != owner
                || target.space_uid != created.space_uid
                || target.space_no != created.space_no
                || target.backend != created.backend
            {
                return Err(partial_failure(
                    base,
                    TypedError::new(
                        ErrorCode::IdentityConflict,
                        "created identity differs from the post-create presentation target",
                    ),
                ));
            }
            Ok(map_connect(
                NewReceipt::created(owner, &created, true),
                outcome,
            ))
        }
        Err(failure) => Err(partial_failure(base, failure.error)),
    }
}

fn finish_concurrent_winner(
    request: &NewRequest,
    client: &ConnectClientContext,
    history: &dyn ConnectHistory,
    runtime: &mut dyn NewRuntime,
    owner: HostUid,
    attempted_backend: Backend,
    original: TypedError,
) -> Result<NewOutcome, NewFailure> {
    let snapshot = runtime.lookup_exact(owner, &request.name)?;
    let lookup = lookup_for_new(
        request.backend_constraint,
        request.allow_name_collision,
        snapshot.wez,
        snapshot.tmux,
    );
    match plan_lookup(request, runtime, owner, lookup)?.value {
        NewPlan::Connect { backend, space } if backend == attempted_backend => {
            finish_existing(request, client, history, runtime, owner, backend, space)
        }
        NewPlan::Fail(error) => Err(error.into()),
        _ => Err(original.into()),
    }
}

/// Existing/blocked/ambiguous resolution must not perform GUI/USB creation
/// eligibility probes. `plan_new` consults the context only on
/// `ProceedCreate`, so a deliberately inert context preserves its single
/// typed translation while keeping the probe boundary exact.
fn plan_lookup(
    request: &NewRequest,
    runtime: &mut dyn NewRuntime,
    owner: HostUid,
    lookup: NewLookup,
) -> Result<crate::policy::Explained<NewPlan>, NewFailure> {
    let context = if matches!(lookup, NewLookup::ProceedCreate { .. }) {
        let context =
            runtime.creation_context(owner, request.backend_constraint, request.launch_gui)?;
        if context.explicit_backend != request.backend_constraint {
            return Err(protocol_failure(
                "creation-policy context changed the caller's backend constraint",
            ));
        }
        context
    } else {
        CreationContext {
            explicit_backend: request.backend_constraint,
            local: LocalEnv {
                trusted_gui_bridge: false,
                wez_service_compatible: false,
            },
            remote: None,
        }
    };
    Ok(plan_new(&request.name, lookup, &context))
}

fn connect_selected(
    request: &NewRequest,
    client: &ConnectClientContext,
    history: &dyn ConnectHistory,
    runtime: &mut dyn NewRuntime,
    owner: HostUid,
    space: SpaceUid,
) -> Result<(ConnectOutcome, FrozenConnectTarget), NewFailure> {
    let connect_request = ConnectRequest {
        selector: ConnectSelector::Ref(canonical_uri(owner, space)),
        explicit_host: None,
        backend_constraint: request.backend_constraint,
        child: None,
        launch_gui: request.launch_gui,
    };
    let outcome =
        crate::connect_cli::connect_with_runtime(&connect_request, client, history, runtime)?;
    let target = match &outcome {
        ConnectOutcome::Completed(receipt) => receipt.target.clone(),
        ConnectOutcome::Exec(plan) => plan.target().clone(),
    };
    Ok((outcome, target))
}

fn map_connect(result: NewReceipt, outcome: ConnectOutcome) -> NewOutcome {
    match outcome {
        ConnectOutcome::Completed(presentation) => NewOutcome::Completed {
            result,
            presentation: Some(presentation),
        },
        ConnectOutcome::Exec(plan) => NewOutcome::Exec {
            result,
            plan: Box::new(plan),
        },
    }
}

fn require_selected(
    owner: HostUid,
    name: &str,
    backend: Backend,
    space: SpaceUid,
    target: &FrozenConnectTarget,
) -> Result<(), NewFailure> {
    if target.owner != owner
        || target.space_uid != space
        || target.backend != backend
        || target.logical_name != name
    {
        return Err(protocol_failure(
            "owner resolution changed the exact selected Space identity/name/backend",
        ));
    }
    Ok(())
}

fn protocol_failure(message: impl Into<String>) -> NewFailure {
    TypedError::new(ErrorCode::ProtocolMismatch, message).into()
}

fn partial_failure(result: NewReceipt, cause: TypedError) -> NewFailure {
    let mut error = TypedError::new(
        ErrorCode::PartialResult,
        format!(
            "Space creation completed, but presentation failed ({}): {}",
            cause.code, cause.message
        ),
    );
    error.target = Some(result.stable_ref.clone());
    NewFailure {
        error,
        result: Some(Box::new(result)),
    }
}

#[cfg(test)]
mod production_order_tests {
    use std::cell::Cell;

    use super::*;

    fn request() -> NewRequest {
        NewRequest {
            name: "project".into(),
            explicit_host: None,
            backend_constraint: None,
            cwd: None,
            no_connect: false,
            allow_name_collision: false,
            launch_gui: false,
            program: Vec::new(),
        }
    }

    #[test]
    fn bounded_no_connect_never_loads_bogus_ambient_tmux_context() {
        let mut request = request();
        request.no_connect = true;
        let probed = Cell::new(false);
        let context = validated_client_context(&request, || {
            probed.set(true);
            Err(TypedError::new(
                ErrorCode::WrongBackendInstance,
                "bogus TMUX/TMUX_PANE",
            ))
        })
        .unwrap();
        assert!(!probed.get());
        assert_eq!(context, ConnectClientContext::default());
    }

    #[test]
    fn static_usage_failure_precedes_bogus_ambient_tmux_context() {
        let mut request = request();
        request.allow_name_collision = true;
        let probed = Cell::new(false);
        let failure = validated_client_context(&request, || {
            probed.set(true);
            Err(TypedError::new(
                ErrorCode::WrongBackendInstance,
                "bogus TMUX/TMUX_PANE",
            ))
        })
        .unwrap_err();
        assert_eq!(failure.error.code, ErrorCode::Usage);
        assert!(!probed.get());
    }
}
