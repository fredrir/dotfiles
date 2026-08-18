//! Focused public `new` create-or-connect contract tests. Native creation and
//! presentation are injectable; the assertions prove ordering and the lack
//! of any backend-fallback call.

use std::collections::{BTreeMap, VecDeque};
use std::num::NonZeroU64;

use dmux::connect_cli::{
    ConnectAuthority, ConnectClientContext, ConnectHistory, ConnectPresenter, FrozenBinding,
    FrozenConnectTarget, HostSelector, OwnerConnectQuery, OwnerExecPlan, PresentationMode,
    PresentationReceipt, TmuxExecKind,
};
use dmux::error::{ErrorCode, TypedError};
use dmux::model::{Backend, BackendInstanceUid, HostUid, ServerEpoch, SpaceNo, SpaceUid};
use dmux::new_cli::{
    NewAuthority, NewLookupSnapshot, NewOutcome, NewPresentationMode, NewRequest, OwnerNewRequest,
    WezPresentationPreflight, create_or_connect,
};
use dmux::operations::CreatedSpace;
use dmux::policy::{CreationContext, LocalEnv};
use dmux::resolve::{BlockReason, ClassSummary};
use uuid::Uuid;

fn host(n: u128) -> HostUid {
    HostUid(Uuid::from_u128(n))
}

fn uid(n: u128) -> SpaceUid {
    SpaceUid(Uuid::from_u128(n))
}

fn instance(n: u128) -> BackendInstanceUid {
    BackendInstanceUid(Uuid::from_u128(n))
}

fn epoch(n: u128) -> ServerEpoch {
    ServerEpoch(Uuid::from_u128(n))
}

fn no(n: u64) -> SpaceNo {
    SpaceNo(NonZeroU64::new(n).unwrap())
}

fn selectable(backend: Backend, target: &FrozenConnectTarget) -> NewLookupSnapshot {
    let yes = ClassSummary::Selectable {
        space: target.space_uid,
        no: target.space_no,
    };
    match backend {
        Backend::Wez => NewLookupSnapshot {
            wez: yes,
            tmux: ClassSummary::NoMatch,
        },
        Backend::Tmux => NewLookupSnapshot {
            wez: ClassSummary::NoMatch,
            tmux: yes,
        },
    }
}

fn empty() -> NewLookupSnapshot {
    NewLookupSnapshot {
        wez: ClassSummary::NoMatch,
        tmux: ClassSummary::NoMatch,
    }
}

fn blocked_wez(reason: BlockReason) -> NewLookupSnapshot {
    NewLookupSnapshot {
        wez: ClassSummary::Blocking {
            reason,
            space: Some(uid(20)),
        },
        tmux: ClassSummary::NoMatch,
    }
}

fn target(
    owner: HostUid,
    backend: Backend,
    name: &str,
    space_uid: SpaceUid,
) -> FrozenConnectTarget {
    FrozenConnectTarget {
        owner,
        space_uid,
        space_no: no(7),
        logical_name: name.into(),
        backend,
        backend_instance_uid: instance(if backend == Backend::Wez { 30 } else { 31 }),
        server_epoch: epoch(if backend == Backend::Wez { 40 } else { 41 }),
        binding: FrozenBinding {
            native_token: if backend == Backend::Wez {
                format!("dmux:{}:{}", owner.0, space_uid.0)
            } else {
                "$7".into()
            },
            endpoint: if backend == Backend::Wez {
                "/tmp/dmux-wez.sock".into()
            } else {
                "dmux-managed".into()
            },
        },
        child: None,
    }
}

#[derive(Default)]
struct NoHistory;

impl ConnectHistory for NoHistory {
    fn previous(&self, _host: HostUid) -> Option<SpaceUid> {
        None
    }
}

struct FakeRuntime {
    local: HostUid,
    hosts: BTreeMap<String, HostUid>,
    lookups: VecDeque<NewLookupSnapshot>,
    context: Result<CreationContext, TypedError>,
    target: FrozenConnectTarget,
    create_result: Result<CreatedSpace, TypedError>,
    connect_error: Option<TypedError>,
    preflight_error: Option<TypedError>,
    recover_error: Option<TypedError>,
    recover_calls: usize,
    events: Vec<&'static str>,
    create_requests: Vec<OwnerNewRequest>,
    context_calls: usize,
    context_launches: Vec<bool>,
}

impl FakeRuntime {
    fn new(target: FrozenConnectTarget, snapshot: NewLookupSnapshot) -> Self {
        let backend = target.backend;
        FakeRuntime {
            local: host(1),
            hosts: BTreeMap::new(),
            lookups: VecDeque::from([snapshot]),
            context: Ok(CreationContext {
                explicit_backend: None,
                local: LocalEnv {
                    trusted_gui_bridge: backend == Backend::Wez,
                    wez_service_compatible: backend == Backend::Wez,
                },
                remote: None,
            }),
            create_result: Ok(CreatedSpace {
                space_uid: target.space_uid,
                space_no: target.space_no,
                backend,
                native_token: target.binding.native_token.clone(),
                group_ref: "g00000000-0000-4000-8000-000000000040.wz-1".into(),
                split_ref: "p00000000-0000-4000-8000-000000000040.wz-1".into(),
                replayed: false,
            }),
            target,
            connect_error: None,
            preflight_error: None,
            recover_error: None,
            recover_calls: 0,
            events: Vec::new(),
            create_requests: Vec::new(),
            context_calls: 0,
            context_launches: Vec::new(),
        }
    }
}

impl ConnectAuthority for FakeRuntime {
    fn local_host_uid(&mut self) -> Result<HostUid, TypedError> {
        Ok(self.local)
    }

    fn resolve_host(&mut self, selector: &HostSelector) -> Result<HostUid, TypedError> {
        match selector {
            HostSelector::Uid(uid)
                if *uid == self.local || self.hosts.values().any(|h| h == uid) =>
            {
                Ok(*uid)
            }
            HostSelector::AliasOrLabel(name) => self
                .hosts
                .get(name)
                .copied()
                .ok_or_else(|| TypedError::new(ErrorCode::NotFound, "unknown host")),
            _ => Err(TypedError::new(ErrorCode::NotFound, "unknown host")),
        }
    }

    fn resolve_live(
        &mut self,
        _query: &OwnerConnectQuery,
    ) -> Result<FrozenConnectTarget, TypedError> {
        self.events.push("resolve");
        Ok(self.target.clone())
    }

    fn revalidate_live(
        &mut self,
        _target: &FrozenConnectTarget,
    ) -> Result<FrozenConnectTarget, TypedError> {
        self.events.push("revalidate");
        Ok(self.target.clone())
    }
}

impl ConnectPresenter for FakeRuntime {
    fn present_wez_ambient(
        &mut self,
        target: &FrozenConnectTarget,
    ) -> Result<PresentationReceipt, TypedError> {
        self.events.push("present");
        if let Some(error) = self.connect_error.clone() {
            return Err(error);
        }
        PresentationReceipt::acknowledged(
            target.clone(),
            PresentationMode::WezAmbient,
            "bridge-ack",
        )
    }

    fn present_wez_cold(
        &mut self,
        target: &FrozenConnectTarget,
    ) -> Result<PresentationReceipt, TypedError> {
        self.events.push("present");
        if let Some(error) = self.connect_error.clone() {
            return Err(error);
        }
        PresentationReceipt::acknowledged(target.clone(), PresentationMode::WezCold, "bridge-ack")
    }

    fn prepare_local_tmux(
        &mut self,
        target: &FrozenConnectTarget,
        kind: TmuxExecKind,
    ) -> Result<OwnerExecPlan, TypedError> {
        self.events.push("handoff");
        if let Some(error) = self.connect_error.clone() {
            return Err(error);
        }
        let verb = match kind {
            TmuxExecKind::LocalAttach => "attach",
            TmuxExecKind::LocalSwitch => "switch-client",
            TmuxExecKind::RemoteAttach => unreachable!(),
        };
        OwnerExecPlan::local(
            target.clone(),
            kind,
            vec![
                "tmux".into(),
                "-L".into(),
                target.binding.endpoint.clone(),
                verb.into(),
                "-t".into(),
                target.binding.native_token.clone(),
            ],
        )
    }

    fn prepare_remote_tmux(
        &mut self,
        _target: &FrozenConnectTarget,
    ) -> Result<OwnerExecPlan, TypedError> {
        unreachable!("connect_existing owns the fake handoff")
    }
}

impl NewAuthority for FakeRuntime {
    fn lookup_exact(
        &mut self,
        _owner: HostUid,
        _name: &str,
    ) -> Result<NewLookupSnapshot, TypedError> {
        self.events.push("lookup");
        self.lookups
            .pop_front()
            .ok_or_else(|| TypedError::new(ErrorCode::OperationFailed, "unexpected extra lookup"))
    }

    fn creation_context(
        &mut self,
        _owner: HostUid,
        explicit_backend: Option<Backend>,
        launch_gui: bool,
    ) -> Result<CreationContext, TypedError> {
        self.events.push("policy");
        self.context_calls += 1;
        self.context_launches.push(launch_gui);
        let mut context = self.context.clone()?;
        context.explicit_backend = explicit_backend;
        Ok(context)
    }

    fn recover_wez_service(&mut self, _owner: HostUid) -> Result<(), TypedError> {
        self.events.push("recover");
        self.recover_calls += 1;
        match self.recover_error.clone() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn preflight_wez_presentation(
        &mut self,
        owner: HostUid,
        mode: NewPresentationMode,
    ) -> Result<WezPresentationPreflight, TypedError> {
        self.events.push("preflight");
        if let Some(error) = self.preflight_error.clone() {
            return Err(error);
        }
        Ok(WezPresentationPreflight {
            owner,
            backend_instance_uid: self.target.backend_instance_uid,
            server_epoch: self.target.server_epoch,
            gui_instance: "gui-1".into(),
            domain: "dmux".into(),
            alternate_domains: Vec::new(),
            mode,
        })
    }

    fn create_owner(&mut self, request: &OwnerNewRequest) -> Result<CreatedSpace, TypedError> {
        self.events.push("create");
        self.create_requests.push(request.clone());
        self.create_result.clone()
    }
}

fn request(name: &str) -> NewRequest {
    NewRequest {
        name: name.into(),
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
fn existing_literal_match_connects_without_name_validation_or_creation_policy_probe() {
    let target = target(host(1), Backend::Tmux, "not valid!", uid(20));
    let mut runtime = FakeRuntime::new(target.clone(), selectable(Backend::Tmux, &target));
    runtime.context = Err(TypedError::new(
        ErrorCode::RouteUnavailable,
        "creation policy must not run",
    ));
    let outcome = create_or_connect(
        &request("not valid!"),
        &ConnectClientContext::default(),
        &NoHistory,
        &mut runtime,
    )
    .unwrap();
    assert!(matches!(outcome, NewOutcome::Exec { .. }));
    assert_eq!(runtime.context_calls, 0);
    assert!(runtime.create_requests.is_empty());
    assert_eq!(
        runtime.events,
        ["lookup", "resolve", "revalidate", "handoff"]
    );
}

#[test]
fn indeterminate_or_blocking_inventory_never_preflights_or_creates() {
    for snapshot in [
        NewLookupSnapshot {
            wez: ClassSummary::Indeterminate,
            tmux: ClassSummary::NoMatch,
        },
        NewLookupSnapshot {
            wez: ClassSummary::Blocking {
                reason: BlockReason::UnmanagedSameName,
                space: None,
            },
            tmux: ClassSummary::NoMatch,
        },
        // An indeterminate observation is not owner proof that the server
        // stopped, so it earns no recovery attempt either.
        blocked_wez(BlockReason::IndeterminateObservation),
    ] {
        let target = target(host(1), Backend::Wez, "project", uid(20));
        let mut runtime = FakeRuntime::new(target, snapshot);
        let failure = create_or_connect(
            &request("project"),
            &ConnectClientContext::default(),
            &NoHistory,
            &mut runtime,
        )
        .unwrap_err();
        assert!(matches!(
            failure.error.code,
            ErrorCode::ProviderUnavailable | ErrorCode::RepairRequired
        ));
        assert_eq!(runtime.events, ["lookup"]);
    }
}

#[test]
fn stopped_wez_service_recovery_precedes_the_space_absent_refusal() {
    let target = target(host(1), Backend::Wez, "project", uid(20));
    let mut runtime = FakeRuntime::new(target, blocked_wez(BlockReason::ServerStopped));
    runtime
        .lookups
        .push_back(blocked_wez(BlockReason::ActiveAbsent));
    let failure = create_or_connect(
        &request("project"),
        &ConnectClientContext::default(),
        &NoHistory,
        &mut runtime,
    )
    .unwrap_err();
    assert_eq!(failure.error.code, ErrorCode::SpaceAbsent);
    assert_eq!(failure.error.code.exit_status().code(), 3);
    assert_eq!(runtime.recover_calls, 1);
    assert!(runtime.create_requests.is_empty());
    assert_eq!(runtime.events, ["lookup", "recover", "lookup"]);
}

#[test]
fn healthy_wez_lookup_never_asks_the_service_manager_to_start_anything() {
    let target = target(host(1), Backend::Wez, "project", uid(20));
    for snapshot in [selectable(Backend::Wez, &target), empty()] {
        let mut runtime = FakeRuntime::new(target.clone(), snapshot);
        create_or_connect(
            &request("project"),
            &ConnectClientContext::default(),
            &NoHistory,
            &mut runtime,
        )
        .unwrap();
        assert_eq!(runtime.recover_calls, 0);
        assert!(!runtime.events.contains(&"recover"));
    }
}

#[test]
fn failed_service_recovery_still_returns_the_original_stopped_refusal() {
    let target = target(host(1), Backend::Wez, "project", uid(20));
    let mut runtime = FakeRuntime::new(target, blocked_wez(BlockReason::ServerStopped));
    runtime.recover_error = Some(TypedError::new(
        ErrorCode::ProviderUnavailable,
        "fixed Wez service start failed with exit 1",
    ));
    let failure = create_or_connect(
        &request("project"),
        &ConnectClientContext::default(),
        &NoHistory,
        &mut runtime,
    )
    .unwrap_err();
    assert_eq!(failure.error.code, ErrorCode::ProviderUnavailable);
    assert!(failure.error.message.contains("ServerStopped"));
    assert!(!failure.error.message.contains("start failed"));
    assert_eq!(runtime.recover_calls, 1);
    assert_eq!(runtime.events, ["lookup", "recover"]);
}

#[test]
fn the_repartition_after_recovery_decides_the_outcome() {
    let target = target(host(1), Backend::Wez, "project", uid(20));
    let mut runtime = FakeRuntime::new(target.clone(), blocked_wez(BlockReason::ServerStopped));
    runtime.lookups.push_back(selectable(Backend::Wez, &target));
    let outcome = create_or_connect(
        &request("project"),
        &ConnectClientContext::default(),
        &NoHistory,
        &mut runtime,
    )
    .unwrap();
    let NewOutcome::Completed { result, .. } = outcome else {
        panic!("Wez presentation completes in process")
    };
    assert!(!result.created);
    assert!(result.connected);
    assert_eq!(result.space_uid, target.space_uid);
    assert_eq!(runtime.recover_calls, 1);
    assert!(runtime.create_requests.is_empty());
    assert_eq!(
        runtime.events,
        [
            "lookup",
            "recover",
            "lookup",
            "resolve",
            "revalidate",
            "present"
        ]
    );
}

#[test]
fn a_still_stopped_repartition_refuses_without_a_second_recovery() {
    let target = target(host(1), Backend::Wez, "project", uid(20));
    let mut runtime = FakeRuntime::new(target, blocked_wez(BlockReason::ServerStopped));
    runtime
        .lookups
        .push_back(blocked_wez(BlockReason::ServerStopped));
    let failure = create_or_connect(
        &request("project"),
        &ConnectClientContext::default(),
        &NoHistory,
        &mut runtime,
    )
    .unwrap_err();
    assert_eq!(failure.error.code, ErrorCode::ProviderUnavailable);
    assert_eq!(runtime.recover_calls, 1);
    assert_eq!(runtime.events, ["lookup", "recover", "lookup"]);
}

#[test]
fn a_stopped_wez_record_blocking_an_explicit_tmux_create_recovers_first() {
    let target = target(host(1), Backend::Tmux, "project", uid(20));
    let mut runtime = FakeRuntime::new(target, blocked_wez(BlockReason::ServerStopped));
    runtime.lookups.push_back(empty());
    let mut req = request("project");
    req.backend_constraint = Some(Backend::Tmux);
    req.no_connect = true;
    let outcome = create_or_connect(
        &req,
        &ConnectClientContext::default(),
        &NoHistory,
        &mut runtime,
    )
    .unwrap();
    let NewOutcome::Completed { result, .. } = outcome else {
        panic!("no-connect must be bounded")
    };
    assert!(result.created);
    assert_eq!(result.backend, Backend::Tmux);
    assert_eq!(runtime.recover_calls, 1);
    assert_eq!(
        runtime.events,
        ["lookup", "recover", "lookup", "policy", "create"]
    );
}

#[test]
fn allow_collision_requires_explicit_backend_and_is_forwarded_to_owner() {
    let target = target(host(1), Backend::Wez, "project", uid(20));
    let mut invalid_runtime = FakeRuntime::new(target.clone(), empty());
    let mut invalid = request("project");
    invalid.allow_name_collision = true;
    let error = create_or_connect(
        &invalid,
        &ConnectClientContext::default(),
        &NoHistory,
        &mut invalid_runtime,
    )
    .unwrap_err();
    assert_eq!(error.error.code, ErrorCode::Usage);
    assert!(invalid_runtime.events.is_empty());

    let opposite = NewLookupSnapshot {
        wez: ClassSummary::NoMatch,
        tmux: ClassSummary::Selectable {
            space: uid(99),
            no: no(9),
        },
    };
    let mut refused_runtime = FakeRuntime::new(target.clone(), opposite);
    let mut refused = request("project");
    refused.backend_constraint = Some(Backend::Wez);
    refused.no_connect = true;
    let failure = create_or_connect(
        &refused,
        &ConnectClientContext::default(),
        &NoHistory,
        &mut refused_runtime,
    )
    .unwrap_err();
    assert_eq!(failure.error.code, ErrorCode::NameConflict);
    assert_eq!(refused_runtime.events, ["lookup"]);

    let mut runtime = FakeRuntime::new(target, opposite);
    let mut allowed = request("project");
    allowed.backend_constraint = Some(Backend::Wez);
    allowed.allow_name_collision = true;
    allowed.no_connect = true;
    let outcome = create_or_connect(
        &allowed,
        &ConnectClientContext::default(),
        &NoHistory,
        &mut runtime,
    )
    .unwrap();
    let NewOutcome::Completed { result, .. } = outcome else {
        panic!("no-connect must be bounded")
    };
    assert!(result.created);
    assert!(runtime.create_requests[0].allow_name_collision);
    assert_eq!(runtime.create_requests[0].backend, Backend::Wez);
}

#[test]
fn wez_presentation_is_preflighted_before_owner_reservation() {
    let target = target(host(1), Backend::Wez, "project", uid(20));
    let mut runtime = FakeRuntime::new(target, empty());
    let outcome = create_or_connect(
        &request("project"),
        &ConnectClientContext::default(),
        &NoHistory,
        &mut runtime,
    )
    .unwrap();
    assert!(matches!(outcome, NewOutcome::Completed { .. }));
    assert_eq!(
        runtime.events,
        [
            "lookup",
            "policy",
            "preflight",
            "create",
            "resolve",
            "revalidate",
            "present"
        ]
    );
    assert_eq!(
        runtime.create_requests[0]
            .presentation
            .as_ref()
            .unwrap()
            .mode,
        NewPresentationMode::Ambient
    );
}

#[test]
fn failed_wez_preflight_consumes_no_identity_and_never_falls_back() {
    let target = target(host(1), Backend::Wez, "project", uid(20));
    let mut runtime = FakeRuntime::new(target, empty());
    runtime.preflight_error = Some(TypedError::new(
        ErrorCode::BridgeUnavailable,
        "no trusted GUI bridge",
    ));
    let failure = create_or_connect(
        &request("project"),
        &ConnectClientContext::default(),
        &NoHistory,
        &mut runtime,
    )
    .unwrap_err();
    assert_eq!(failure.error.code, ErrorCode::BridgeUnavailable);
    assert!(failure.result.is_none());
    assert!(runtime.create_requests.is_empty());
    assert_eq!(runtime.events, ["lookup", "policy", "preflight"]);
}

#[test]
fn no_connect_skips_preflight_and_presentation_for_create_and_existing() {
    let target = target(host(1), Backend::Wez, "project", uid(20));
    let mut create_runtime = FakeRuntime::new(target.clone(), empty());
    let mut req = request("project");
    req.no_connect = true;
    let created = create_or_connect(
        &req,
        &ConnectClientContext::default(),
        &NoHistory,
        &mut create_runtime,
    )
    .unwrap();
    assert!(matches!(created, NewOutcome::Completed { .. }));
    assert_eq!(create_runtime.events, ["lookup", "policy", "create"]);
    assert!(create_runtime.create_requests[0].presentation.is_none());

    let mut existing_runtime = FakeRuntime::new(target.clone(), selectable(Backend::Wez, &target));
    let existing = create_or_connect(
        &req,
        &ConnectClientContext::default(),
        &NoHistory,
        &mut existing_runtime,
    )
    .unwrap();
    let NewOutcome::Completed { result, .. } = existing else {
        panic!("bounded existing result")
    };
    assert!(!result.created);
    assert!(!result.connected);
    assert_eq!(existing_runtime.events, ["lookup", "resolve"]);
}

#[test]
fn launch_gui_that_selects_tmux_refuses_before_owner_create() {
    let target = target(host(1), Backend::Tmux, "project", uid(20));
    let mut runtime = FakeRuntime::new(target, empty());
    let mut req = request("project");
    req.launch_gui = true;
    let failure = create_or_connect(
        &req,
        &ConnectClientContext::default(),
        &NoHistory,
        &mut runtime,
    )
    .unwrap_err();
    assert_eq!(failure.error.code, ErrorCode::Usage);
    assert_eq!(runtime.events, ["lookup", "policy"]);
}

#[test]
fn explicit_wez_launch_uses_cold_policy_and_preflight_before_create() {
    let target = target(host(1), Backend::Wez, "project", uid(20));
    let mut runtime = FakeRuntime::new(target, empty());
    let mut req = request("project");
    req.backend_constraint = Some(Backend::Wez);
    req.launch_gui = true;
    let outcome = create_or_connect(
        &req,
        &ConnectClientContext::default(),
        &NoHistory,
        &mut runtime,
    )
    .unwrap();
    assert!(matches!(outcome, NewOutcome::Completed { .. }));
    assert_eq!(runtime.context_launches, [true]);
    assert_eq!(
        runtime.create_requests[0]
            .presentation
            .as_ref()
            .unwrap()
            .mode,
        NewPresentationMode::Cold
    );
    assert_eq!(
        runtime.events,
        [
            "lookup",
            "policy",
            "preflight",
            "create",
            "resolve",
            "revalidate",
            "present"
        ]
    );
}

#[test]
fn successful_create_then_presentation_failure_is_stable_partial_exit_seven() {
    let target = target(host(1), Backend::Wez, "project", uid(20));
    let mut runtime = FakeRuntime::new(target.clone(), empty());
    runtime.connect_error = Some(TypedError::new(
        ErrorCode::BridgeUnavailable,
        "bridge disappeared",
    ));
    let failure = create_or_connect(
        &request("project"),
        &ConnectClientContext::default(),
        &NoHistory,
        &mut runtime,
    )
    .unwrap_err();
    assert_eq!(failure.error.code, ErrorCode::PartialResult);
    assert_eq!(failure.error.code.exit_status().code(), 7);
    let result = failure.result.expect("durable partial identity");
    assert!(result.created);
    assert!(!result.connected);
    assert_eq!(result.space_uid, target.space_uid);
    assert_eq!(
        failure.error.target.as_deref(),
        Some(result.stable_ref.as_str())
    );
    assert_eq!(runtime.create_requests.len(), 1);
}

#[test]
fn concurrent_same_name_winner_is_reselected_without_second_create() {
    let target = target(host(1), Backend::Tmux, "project", uid(20));
    let mut runtime = FakeRuntime::new(target.clone(), empty());
    runtime
        .lookups
        .push_back(selectable(Backend::Tmux, &target));
    runtime.create_result = Err(TypedError::new(ErrorCode::NameConflict, "winner committed"));
    let outcome = create_or_connect(
        &request("project"),
        &ConnectClientContext::default(),
        &NoHistory,
        &mut runtime,
    )
    .unwrap();
    let NewOutcome::Exec { result, .. } = outcome else {
        panic!("tmux winner uses terminal handoff")
    };
    assert!(!result.created);
    assert_eq!(runtime.create_requests.len(), 1);
    assert_eq!(
        runtime.events,
        [
            "lookup",
            "policy",
            "create",
            "lookup",
            "resolve",
            "revalidate",
            "handoff"
        ]
    );
}

#[test]
fn remote_owner_and_exact_backend_are_frozen_with_no_fallback() {
    let remote = host(2);
    let target = target(remote, Backend::Tmux, "project", uid(20));
    let mut runtime = FakeRuntime::new(target, empty());
    runtime.hosts.insert("archie".into(), remote);
    runtime.create_result = Err(TypedError::new(
        ErrorCode::AuthFailed,
        "terminal auth failure",
    ));
    let mut req = request("project");
    req.explicit_host = Some(HostSelector::AliasOrLabel("archie".into()));
    req.backend_constraint = Some(Backend::Tmux);
    let failure = create_or_connect(
        &req,
        &ConnectClientContext::default(),
        &NoHistory,
        &mut runtime,
    )
    .unwrap_err();
    assert_eq!(failure.error.code, ErrorCode::AuthFailed);
    assert_eq!(runtime.create_requests.len(), 1);
    assert_eq!(runtime.create_requests[0].owner, remote);
    assert_eq!(runtime.create_requests[0].backend, Backend::Tmux);
    assert_eq!(runtime.events, ["lookup", "policy", "create"]);
}
