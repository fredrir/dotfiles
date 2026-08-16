//! Focused P9 public-connect orchestration contract.  Every fake seam is
//! read/present only; the test harness has no creation operation to call.

use std::collections::BTreeMap;
use std::num::NonZeroU64;

use dmux::connect_cli::{
    ConnectAuthority, ConnectClientContext, ConnectHistory, ConnectOutcome, ConnectPresenter,
    ConnectRequest, ConnectSelector, FrozenBinding, FrozenConnectTarget, HostSelector,
    LocalTmuxClient, OwnerConnectQuery, OwnerExecPlan, OwnerLocator, PresentationMode,
    PresentationReceipt, ProductionConnectAdapter, RemoteAttachWitness, RequestedChild,
    TmuxExecKind, VerifiedConnectChild, connect, parse_requested_child, preflight_connect_request,
};
use dmux::error::{ErrorCode, TypedError};
use dmux::model::{
    Backend, BackendInstanceUid, ChildKind, HostUid, ProviderHandle, ServerEpoch, SpaceNo, SpaceUid,
};
use dmux::operations::OperationEnv;
use dmux::remote::client::DirectInvoker;
use uuid::Uuid;

fn host(n: u128) -> HostUid {
    HostUid(Uuid::from_u128(n))
}

fn space(n: u128) -> SpaceUid {
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

#[test]
fn standalone_child_parser_uses_the_exact_epoch_qualified_grammar() {
    let group = parse_requested_child(&format!("g{}.tx-9", epoch(50).0), ChildKind::Group).unwrap();
    assert_eq!(group.kind, ChildKind::Group);
    assert_eq!(group.epoch, epoch(50));
    assert_eq!(group.handle, ProviderHandle::Tx(9));

    let split = parse_requested_child(&format!("p{}.wz-0", epoch(51).0), ChildKind::Split).unwrap();
    assert_eq!(split.kind, ChildKind::Split);
    assert_eq!(split.epoch, epoch(51));
    assert_eq!(split.handle, ProviderHandle::Wz(0));

    let wrong_kind =
        parse_requested_child(&format!("g{}.tx-9", epoch(50).0), ChildKind::Split).unwrap_err();
    assert_eq!(wrong_kind.code, ErrorCode::InvalidRef);

    for malformed in [
        "g.tx-9",
        "g00000000-0000-0000-0000-000000000032.tx-09",
        "tx-9",
    ] {
        assert_eq!(
            parse_requested_child(malformed, ChildKind::Group)
                .unwrap_err()
                .code,
            ErrorCode::InvalidRef,
            "{malformed}"
        );
    }
}

fn target(owner: HostUid, backend: Backend) -> FrozenConnectTarget {
    FrozenConnectTarget {
        owner,
        space_uid: space(20),
        space_no: no(2),
        logical_name: "project".into(),
        backend,
        backend_instance_uid: instance(match backend {
            Backend::Wez => 30,
            Backend::Tmux => 31,
        }),
        server_epoch: epoch(match backend {
            Backend::Wez => 40,
            Backend::Tmux => 41,
        }),
        binding: FrozenBinding {
            native_token: match backend {
                Backend::Wez => format!("dmux:{}:{}", owner.0, space(20).0),
                Backend::Tmux => "$7".into(),
            },
            endpoint: match backend {
                Backend::Wez => "/tmp/dmux-wez.sock".into(),
                Backend::Tmux => "dmux-managed".into(),
            },
        },
        child: None,
    }
}

#[derive(Default)]
struct FakeHistory {
    rows: BTreeMap<String, SpaceUid>,
}

impl FakeHistory {
    fn with(owner: HostUid, target: SpaceUid) -> Self {
        let mut rows = BTreeMap::new();
        rows.insert(owner.0.to_string(), target);
        FakeHistory { rows }
    }
}

impl ConnectHistory for FakeHistory {
    fn previous(&self, owner: HostUid) -> Option<SpaceUid> {
        self.rows.get(&owner.0.to_string()).copied()
    }
}

struct FakeAuthority {
    local: HostUid,
    hosts: BTreeMap<String, HostUid>,
    first: Result<FrozenConnectTarget, TypedError>,
    second: Option<Result<FrozenConnectTarget, TypedError>>,
    resolve_calls: usize,
    revalidate_calls: usize,
    query: Option<OwnerConnectQuery>,
}

impl FakeAuthority {
    fn new(local: HostUid, target: FrozenConnectTarget) -> Self {
        FakeAuthority {
            local,
            hosts: BTreeMap::new(),
            first: Ok(target),
            second: None,
            resolve_calls: 0,
            revalidate_calls: 0,
            query: None,
        }
    }

    fn alias(mut self, spelling: &str, owner: HostUid) -> Self {
        self.hosts.insert(spelling.into(), owner);
        self
    }
}

impl ConnectAuthority for FakeAuthority {
    fn local_host_uid(&mut self) -> Result<HostUid, TypedError> {
        Ok(self.local)
    }

    fn resolve_host(&mut self, selector: &HostSelector) -> Result<HostUid, TypedError> {
        match selector {
            HostSelector::Uid(uid)
                if *uid == self.local || self.hosts.values().any(|known| known == uid) =>
            {
                Ok(*uid)
            }
            HostSelector::AliasOrLabel(spelling) if spelling == "a" => Ok(self.local),
            HostSelector::AliasOrLabel(spelling) => {
                self.hosts.get(spelling).copied().ok_or_else(|| {
                    TypedError::new(ErrorCode::NotFound, format!("unknown host {spelling}"))
                })
            }
            HostSelector::Uid(uid) => Err(TypedError::new(
                ErrorCode::NotFound,
                format!("unknown host {}", uid.0),
            )),
        }
    }

    fn resolve_live(
        &mut self,
        query: &OwnerConnectQuery,
    ) -> Result<FrozenConnectTarget, TypedError> {
        self.resolve_calls += 1;
        self.query = Some(query.clone());
        self.first.clone()
    }

    fn revalidate_live(
        &mut self,
        target: &FrozenConnectTarget,
    ) -> Result<FrozenConnectTarget, TypedError> {
        self.revalidate_calls += 1;
        self.second.clone().unwrap_or_else(|| Ok(target.clone()))
    }
}

#[derive(Default)]
struct FakePresenter {
    calls: Vec<String>,
    ambient_error: Option<TypedError>,
    cold_error: Option<TypedError>,
    seen_child: Option<VerifiedConnectChild>,
}

impl ConnectPresenter for FakePresenter {
    fn present_wez_ambient(
        &mut self,
        target: &FrozenConnectTarget,
    ) -> Result<PresentationReceipt, TypedError> {
        self.calls.push("wez_ambient".into());
        self.seen_child = target.child.clone();
        if let Some(error) = &self.ambient_error {
            return Err(error.clone());
        }
        PresentationReceipt::acknowledged(
            target.clone(),
            PresentationMode::WezAmbient,
            "bridge-request-1",
        )
    }

    fn present_wez_cold(
        &mut self,
        target: &FrozenConnectTarget,
    ) -> Result<PresentationReceipt, TypedError> {
        self.calls.push("wez_cold".into());
        self.seen_child = target.child.clone();
        if let Some(error) = &self.cold_error {
            return Err(error.clone());
        }
        PresentationReceipt::acknowledged(
            target.clone(),
            PresentationMode::WezCold,
            "cold-request-1",
        )
    }

    fn prepare_local_tmux(
        &mut self,
        target: &FrozenConnectTarget,
        kind: TmuxExecKind,
    ) -> Result<OwnerExecPlan, TypedError> {
        self.calls.push(format!("tmux_{kind:?}"));
        self.seen_child = target.child.clone();
        let verb = match kind {
            TmuxExecKind::LocalAttach => "attach",
            TmuxExecKind::LocalSwitch => "switch-client",
            TmuxExecKind::RemoteAttach => unreachable!(),
        };
        OwnerExecPlan::local(
            target.clone(),
            kind,
            vec![
                "/opt/homebrew/bin/tmux".into(),
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
        target: &FrozenConnectTarget,
    ) -> Result<OwnerExecPlan, TypedError> {
        self.calls.push("tmux_remote".into());
        self.seen_child = target.child.clone();
        let token = "0123456789abcdef0123456789abcdef";
        let witness = RemoteAttachWitness::new(
            Uuid::from_u128(90),
            target.owner,
            target.space_uid,
            target.backend_instance_uid,
            target.server_epoch,
            17,
            "archie-usb",
            "fredrir@archie-usb",
            "2099-01-01T00:00:00Z",
            token,
        );
        OwnerExecPlan::remote(
            target.clone(),
            vec![
                "/usr/bin/ssh".into(),
                "-tt".into(),
                "fredrir@archie-usb".into(),
                "dmux".into(),
                "_attach".into(),
                "--token".into(),
                token.into(),
            ],
            witness,
        )
    }
}

fn request(selector: ConnectSelector) -> ConnectRequest {
    ConnectRequest {
        selector,
        explicit_host: None,
        backend_constraint: None,
        child: None,
        launch_gui: false,
    }
}

#[test]
fn static_preflight_rejects_syntax_and_decidable_contradictions_without_authority() {
    let mut req = request(ConnectSelector::ExactName(String::new()));
    assert_eq!(
        preflight_connect_request(&req).unwrap_err().code,
        ErrorCode::InvalidRef
    );

    req.selector = ConnectSelector::Ref("0".into());
    assert_eq!(
        preflight_connect_request(&req).unwrap_err().code,
        ErrorCode::InvalidRef
    );

    req.selector = ConnectSelector::Ref(format!("2/g{}.tx-1", epoch(50).0));
    req.child = Some(RequestedChild {
        kind: ChildKind::Group,
        epoch: epoch(50),
        handle: ProviderHandle::Tx(2),
    });
    assert_eq!(
        preflight_connect_request(&req).unwrap_err().code,
        ErrorCode::InvalidRef
    );

    req = request(ConnectSelector::Ref("2".into()));
    req.backend_constraint = Some(Backend::Tmux);
    req.launch_gui = true;
    assert_eq!(
        preflight_connect_request(&req).unwrap_err().code,
        ErrorCode::Usage
    );

    req = request(ConnectSelector::Ref(format!(
        "dmux://{}/spaces/{}",
        host(2).0,
        space(20).0
    )));
    req.explicit_host = Some(HostSelector::Uid(host(3)));
    assert_eq!(
        preflight_connect_request(&req).unwrap_err().code,
        ErrorCode::InvalidRef
    );
}

#[test]
fn contradictory_explicit_and_embedded_hosts_fail_before_owner_scan() {
    let local = host(1);
    let remote = host(2);
    let mut authority = FakeAuthority::new(local, target(remote, Backend::Wez))
        .alias("a", local)
        .alias("b", remote);
    let mut presenter = FakePresenter::default();
    let mut req = request(ConnectSelector::Ref("b:project".into()));
    req.explicit_host = Some(HostSelector::AliasOrLabel("a".into()));

    let error = connect(
        &req,
        &ConnectClientContext::default(),
        &FakeHistory::default(),
        &mut authority,
        &mut presenter,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidRef);
    assert_eq!(authority.resolve_calls, 0);
    assert!(presenter.calls.is_empty());
}

#[test]
fn stable_uid_backend_contradiction_is_not_reinterpreted_as_a_name() {
    let local = host(1);
    let tmux = target(local, Backend::Tmux);
    let mut authority = FakeAuthority::new(local, tmux.clone());
    let mut presenter = FakePresenter::default();
    let mut req = request(ConnectSelector::Ref(format!(
        "dmux://{}/spaces/{}",
        local.0, tmux.space_uid.0
    )));
    req.backend_constraint = Some(Backend::Wez);

    let error = connect(
        &req,
        &ConnectClientContext::default(),
        &FakeHistory::default(),
        &mut authority,
        &mut presenter,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BackendMismatch);
    assert_eq!(authority.resolve_calls, 1);
    assert_eq!(authority.revalidate_calls, 0);
    assert!(presenter.calls.is_empty());
    assert_eq!(authority.query.unwrap().backend_filter, None);
}

#[test]
fn missing_previous_is_not_found_and_has_no_owner_or_create_side_effect() {
    let local = host(1);
    let mut authority = FakeAuthority::new(local, target(local, Backend::Wez));
    let mut presenter = FakePresenter::default();
    let error = connect(
        &request(ConnectSelector::Previous),
        &ConnectClientContext::default(),
        &FakeHistory::default(),
        &mut authority,
        &mut presenter,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::NotFound);
    assert_eq!(authority.resolve_calls, 0);
    assert!(presenter.calls.is_empty());
}

#[test]
fn owner_scan_failure_propagates_without_cross_backend_fallback() {
    let local = host(1);
    let expected = TypedError::new(
        ErrorCode::RouteUnavailable,
        "the exact owner scan failed before authentication",
    );
    let mut authority = FakeAuthority::new(local, target(local, Backend::Wez));
    authority.first = Err(expected.clone());
    let mut presenter = FakePresenter::default();

    let error = connect(
        &request(ConnectSelector::ExactName("project".into())),
        &ConnectClientContext::default(),
        &FakeHistory::default(),
        &mut authority,
        &mut presenter,
    )
    .unwrap_err();
    assert_eq!(error, expected);
    assert_eq!(authority.resolve_calls, 1);
    assert_eq!(authority.revalidate_calls, 0);
    assert!(presenter.calls.is_empty());
}

#[test]
fn wez_bridge_failure_never_calls_a_tmux_path() {
    let local = host(1);
    let mut authority = FakeAuthority::new(local, target(local, Backend::Wez));
    let expected = TypedError::new(ErrorCode::BridgeUnavailable, "no trusted GUI heartbeat");
    let mut presenter = FakePresenter {
        ambient_error: Some(expected.clone()),
        ..FakePresenter::default()
    };

    let error = connect(
        &request(ConnectSelector::Ref("2".into())),
        &ConnectClientContext::default(),
        &FakeHistory::default(),
        &mut authority,
        &mut presenter,
    )
    .unwrap_err();
    assert_eq!(error, expected);
    assert_eq!(presenter.calls, ["wez_ambient"]);
}

#[test]
fn exact_split_child_is_owner_correlated_and_reaches_only_wez_adapter() {
    let local = host(1);
    let e = epoch(40);
    let mut wez = target(local, Backend::Wez);
    wez.child = Some(VerifiedConnectChild::Split {
        epoch: e,
        group: ProviderHandle::Wz(6),
        split: ProviderHandle::Wz(7),
    });
    let mut authority = FakeAuthority::new(local, wez);
    let mut presenter = FakePresenter::default();
    let req = request(ConnectSelector::Ref(format!("2/p{}.wz-7", e.0)));

    let outcome = connect(
        &req,
        &ConnectClientContext::default(),
        &FakeHistory::default(),
        &mut authority,
        &mut presenter,
    )
    .unwrap();
    assert!(matches!(outcome, ConnectOutcome::Completed(_)));
    assert_eq!(presenter.calls, ["wez_ambient"]);
    assert_eq!(
        authority.query.unwrap().child,
        Some(RequestedChild {
            kind: ChildKind::Split,
            epoch: e,
            handle: ProviderHandle::Wz(7),
        })
    );
    assert_eq!(
        presenter.seen_child,
        Some(VerifiedConnectChild::Split {
            epoch: e,
            group: ProviderHandle::Wz(6),
            split: ProviderHandle::Wz(7),
        })
    );
}

#[test]
fn owner_returning_a_different_child_is_protocol_failure() {
    let local = host(1);
    let e = epoch(40);
    let mut wez = target(local, Backend::Wez);
    wez.child = Some(VerifiedConnectChild::Group {
        epoch: e,
        handle: ProviderHandle::Wz(8),
    });
    let mut authority = FakeAuthority::new(local, wez);
    let mut presenter = FakePresenter::default();
    let req = request(ConnectSelector::Ref(format!("2/g{}.wz-7", e.0)));

    let error = connect(
        &req,
        &ConnectClientContext::default(),
        &FakeHistory::default(),
        &mut authority,
        &mut presenter,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::ProtocolMismatch);
    assert_eq!(authority.revalidate_calls, 0);
    assert!(presenter.calls.is_empty());
}

#[test]
fn epoch_race_fails_before_any_presentation() {
    let local = host(1);
    let first = target(local, Backend::Wez);
    let mut raced = first.clone();
    raced.server_epoch = epoch(99);
    let mut authority = FakeAuthority::new(local, first);
    authority.second = Some(Ok(raced));
    let mut presenter = FakePresenter::default();

    let error = connect(
        &request(ConnectSelector::Ref("2".into())),
        &ConnectClientContext::default(),
        &FakeHistory::default(),
        &mut authority,
        &mut presenter,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BackendEpochChanged);
    assert_eq!(authority.revalidate_calls, 1);
    assert!(presenter.calls.is_empty());
}

#[test]
fn local_tmux_detached_attaches_and_exact_same_server_switches() {
    let local = host(1);
    for (client, expected) in [
        (None, TmuxExecKind::LocalAttach),
        (
            Some(LocalTmuxClient {
                owner: local,
                backend_instance_uid: instance(31),
                server_epoch: epoch(41),
            }),
            TmuxExecKind::LocalSwitch,
        ),
    ] {
        let mut authority = FakeAuthority::new(local, target(local, Backend::Tmux));
        let mut presenter = FakePresenter::default();
        let outcome = connect(
            &request(ConnectSelector::Ref("2".into())),
            &ConnectClientContext {
                tmux_client: client,
            },
            &FakeHistory::default(),
            &mut authority,
            &mut presenter,
        )
        .unwrap();
        let ConnectOutcome::Exec(plan) = outcome else {
            panic!("tmux presentation must hand off")
        };
        assert_eq!(plan.kind(), expected);
        assert_eq!(plan.history_intent().space_uid, space(20));
        assert!(plan.argv().iter().any(|arg| match expected {
            TmuxExecKind::LocalAttach => arg == "attach",
            TmuxExecKind::LocalSwitch => arg == "switch-client",
            TmuxExecKind::RemoteAttach => false,
        }));
    }
}

#[test]
fn production_local_tmux_split_plan_focuses_exact_child_before_handoff() {
    let local = host(1);
    let mut tmux = target(local, Backend::Tmux);
    tmux.child = Some(VerifiedConnectChild::Split {
        epoch: tmux.server_epoch,
        group: ProviderHandle::Tx(6),
        split: ProviderHandle::Tx(7),
    });
    let scratch = tempfile::tempdir().unwrap();
    let mut adapter = ProductionConnectAdapter::with_invoker(
        OperationEnv {
            db_path: scratch.path().join("registry.sqlite3"),
            lock_dir: scratch.path().join("locks"),
        },
        DirectInvoker,
        "dmux",
    );
    let plan = adapter
        .prepare_local_tmux(&tmux, TmuxExecKind::LocalAttach)
        .unwrap();
    assert_eq!(
        plan.argv(),
        [
            "tmux",
            "-L",
            "dmux-managed",
            "select-window",
            "-t",
            "$7:@6",
            ";",
            "select-pane",
            "-t",
            "%7",
            ";",
            "attach",
            "-t",
            "$7",
        ]
    );
    assert_eq!(plan.argv().last().map(String::as_str), Some("$7"));
}

#[test]
fn local_tmux_child_plan_validator_rejects_attach_before_focus() {
    let local = host(1);
    let mut tmux = target(local, Backend::Tmux);
    tmux.child = Some(VerifiedConnectChild::Group {
        epoch: tmux.server_epoch,
        handle: ProviderHandle::Tx(6),
    });
    let error = OwnerExecPlan::local(
        tmux,
        TmuxExecKind::LocalAttach,
        vec![
            "tmux".into(),
            "-L".into(),
            "dmux-managed".into(),
            "attach".into(),
            "-t".into(),
            "$7".into(),
            ";".into(),
            "select-window".into(),
            "-t".into(),
            "$7:@6".into(),
        ],
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::ProtocolMismatch);
}

#[test]
fn another_local_tmux_server_is_not_misclassified_as_detached() {
    let local = host(1);
    let mut authority = FakeAuthority::new(local, target(local, Backend::Tmux));
    let mut presenter = FakePresenter::default();
    let context = ConnectClientContext {
        tmux_client: Some(LocalTmuxClient {
            owner: local,
            backend_instance_uid: instance(777),
            server_epoch: epoch(778),
        }),
    };
    let error = connect(
        &request(ConnectSelector::Ref("2".into())),
        &context,
        &FakeHistory::default(),
        &mut authority,
        &mut presenter,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::WrongBackendInstance);
    assert!(presenter.calls.is_empty());
}

#[test]
fn remote_tmux_handoff_carries_verified_winning_route_and_single_use_token() {
    let local = host(1);
    let remote = host(2);
    let mut authority = FakeAuthority::new(local, target(remote, Backend::Tmux)).alias("b", remote);
    let mut presenter = FakePresenter::default();
    let outcome = connect(
        &request(ConnectSelector::Ref("b2".into())),
        &ConnectClientContext::default(),
        &FakeHistory::default(),
        &mut authority,
        &mut presenter,
    )
    .unwrap();
    let ConnectOutcome::Exec(plan) = outcome else {
        panic!("remote tmux presentation must hand off")
    };
    assert_eq!(plan.kind(), TmuxExecKind::RemoteAttach);
    let witness = plan.remote_witness().unwrap();
    assert_eq!(witness.route_id, 17);
    assert_eq!(witness.route, "archie-usb");
    assert_eq!(witness.destination, "fredrir@archie-usb");
    assert_eq!(witness.space_uid, space(20));
    assert!(
        plan.argv()
            .windows(2)
            .any(|pair| pair == ["--token", witness.token()])
    );
    assert!(!format!("{witness:?}").contains(witness.token()));
    assert!(!plan.argv().iter().any(|arg| arg == "$7"));
}

#[test]
fn malformed_remote_attach_plan_cannot_embed_native_target_or_wrong_route() {
    let remote = host(2);
    let target = target(remote, Backend::Tmux);
    let token = "0123456789abcdef0123456789abcdef";
    let witness = RemoteAttachWitness::new(
        Uuid::from_u128(90),
        remote,
        target.space_uid,
        target.backend_instance_uid,
        target.server_epoch,
        17,
        "archie-usb",
        "fredrir@archie-usb",
        "2099-01-01T00:00:00Z",
        token,
    );
    let leaked = OwnerExecPlan::remote(
        target.clone(),
        vec![
            "ssh".into(),
            "-t".into(),
            "fredrir@archie-usb".into(),
            "dmux".into(),
            "_attach".into(),
            "--token".into(),
            token.into(),
            "$7".into(),
        ],
        witness.clone(),
    )
    .unwrap_err();
    assert_eq!(leaked.code, ErrorCode::ProtocolMismatch);

    let mut wrong_route = witness;
    wrong_route.destination = "fredrir@archie-ts".into();
    let error = OwnerExecPlan::remote(
        target,
        vec![
            "ssh".into(),
            "-t".into(),
            "fredrir@archie-usb".into(),
            "dmux".into(),
            "_attach".into(),
            "--token".into(),
            token.into(),
        ],
        wrong_route,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::ProtocolMismatch);
}

#[test]
fn tmux_launch_gui_is_usage_and_never_mints_an_attach_plan() {
    let local = host(1);
    let mut authority = FakeAuthority::new(local, target(local, Backend::Tmux));
    let mut presenter = FakePresenter::default();
    let mut req = request(ConnectSelector::Ref("2".into()));
    req.backend_constraint = Some(Backend::Tmux);
    req.launch_gui = true;
    let error = connect(
        &req,
        &ConnectClientContext::default(),
        &FakeHistory::default(),
        &mut authority,
        &mut presenter,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Usage);
    assert_eq!(authority.resolve_calls, 0);
    assert!(presenter.calls.is_empty());
}

#[test]
fn previous_uses_stable_space_uid_after_rename() {
    let local = host(1);
    let mut renamed = target(local, Backend::Wez);
    renamed.logical_name = "renamed-after-history".into();
    let mut authority = FakeAuthority::new(local, renamed);
    let mut presenter = FakePresenter::default();
    let outcome = connect(
        &request(ConnectSelector::Previous),
        &ConnectClientContext::default(),
        &FakeHistory::with(local, space(20)),
        &mut authority,
        &mut presenter,
    )
    .unwrap();
    assert!(matches!(outcome, ConnectOutcome::Completed(_)));
    assert_eq!(
        authority.query.unwrap().locator,
        OwnerLocator::Uid(space(20))
    );
}

#[test]
fn exact_name_backend_constraint_is_an_owner_filter_not_a_fallback() {
    let local = host(1);
    let mut authority = FakeAuthority::new(local, target(local, Backend::Wez));
    let mut presenter = FakePresenter::default();
    let mut req = request(ConnectSelector::ExactName("project".into()));
    req.backend_constraint = Some(Backend::Wez);
    connect(
        &req,
        &ConnectClientContext::default(),
        &FakeHistory::default(),
        &mut authority,
        &mut presenter,
    )
    .unwrap();
    assert_eq!(authority.query.unwrap().backend_filter, Some(Backend::Wez));
    assert_eq!(presenter.calls, ["wez_ambient"]);
}
