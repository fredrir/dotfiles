//! Deterministic public Connect/New pre-exec handoff ordering contract.
//!
//! Production owns heartbeat/process/provider checks. This fake proves that
//! the shared dispatcher captures the source before reserving correlation,
//! stages only pending history, starts its post-attach monitor, and cancels
//! every staged artifact when a raced proof or later pre-exec step fails.

use std::num::NonZeroU64;

use dmux::connect_cli::{
    ExecHandoffRuntime, FrozenBinding, FrozenConnectTarget, OwnerExecPlan, TmuxExecKind,
    prepare_exec_handoff_with_runtime,
};
use dmux::error::{ErrorCode, TypedError};
use dmux::model::{Backend, BackendInstanceUid, HostUid, ServerEpoch, SpaceNo, SpaceUid};
use uuid::Uuid;

fn uid(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

fn plan() -> OwnerExecPlan {
    let target = FrozenConnectTarget {
        owner: HostUid(uid(1)),
        space_uid: SpaceUid(uid(2)),
        space_no: SpaceNo(NonZeroU64::new(3).unwrap()),
        logical_name: "destination".into(),
        backend: Backend::Tmux,
        backend_instance_uid: BackendInstanceUid(uid(4)),
        server_epoch: ServerEpoch(uid(5)),
        binding: FrozenBinding {
            native_token: "$7".into(),
            endpoint: "dmux-managed".into(),
        },
        child: None,
    };
    OwnerExecPlan::local(
        target,
        TmuxExecKind::LocalAttach,
        ["tmux", "-L", "dmux-managed", "attach", "-t", "$7"]
            .map(str::to_string)
            .to_vec(),
    )
    .unwrap()
}

#[derive(Debug, Clone, Copy)]
struct Source {
    existing_client_uid: Option<Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    Capture,
    Reserve(Option<Uuid>),
    Stage { client_uid: Uuid, has_source: bool },
    Start(Uuid),
    Terminal,
    CancelPending(Uuid),
    CancelCorrelation(Uuid),
}

struct FakeHandoff {
    source: Result<Option<Source>, TypedError>,
    correlation: Result<Option<Uuid>, TypedError>,
    pending: Result<Option<Uuid>, TypedError>,
    start: Result<(), TypedError>,
    terminal: Result<(), TypedError>,
    calls: Vec<Call>,
}

impl FakeHandoff {
    fn managed(client_uid: Uuid, pending_uid: Uuid) -> Self {
        FakeHandoff {
            source: Ok(Some(Source {
                existing_client_uid: None,
            })),
            correlation: Ok(Some(client_uid)),
            pending: Ok(Some(pending_uid)),
            start: Ok(()),
            terminal: Ok(()),
            calls: Vec::new(),
        }
    }

    fn terminal_only() -> Self {
        FakeHandoff {
            source: Ok(None),
            correlation: Ok(None),
            pending: Ok(None),
            start: Ok(()),
            terminal: Ok(()),
            calls: Vec::new(),
        }
    }
}

impl ExecHandoffRuntime for FakeHandoff {
    type Source = Source;
    type Correlation = Uuid;

    fn capture_gui_source(
        &mut self,
        _plan: &OwnerExecPlan,
    ) -> Result<Option<Self::Source>, TypedError> {
        self.calls.push(Call::Capture);
        self.source.clone()
    }

    fn source_tmux_client_uid(&self, source: &Self::Source) -> Option<Uuid> {
        source.existing_client_uid
    }

    fn reserve_controller_correlation(
        &mut self,
        _plan: &OwnerExecPlan,
        existing_client_uid: Option<Uuid>,
    ) -> Result<Option<Self::Correlation>, TypedError> {
        self.calls.push(Call::Reserve(existing_client_uid));
        self.correlation.clone()
    }

    fn correlation_uid(&self, correlation: &Self::Correlation) -> Uuid {
        *correlation
    }

    fn stage_gui_transition(
        &mut self,
        _plan: &OwnerExecPlan,
        client_uid: Uuid,
        source: Option<&Self::Source>,
    ) -> Result<Option<Uuid>, TypedError> {
        self.calls.push(Call::Stage {
            client_uid,
            has_source: source.is_some(),
        });
        self.pending.clone()
    }

    fn start_gui_transition_finalizer(&mut self, pending_uid: Uuid) -> Result<(), TypedError> {
        self.calls.push(Call::Start(pending_uid));
        self.start.clone()
    }

    fn commit_terminal_history(&mut self, _plan: &OwnerExecPlan) -> Result<(), TypedError> {
        self.calls.push(Call::Terminal);
        self.terminal.clone()
    }

    fn cancel_gui_transition(&mut self, pending_uid: Uuid) -> Result<(), TypedError> {
        self.calls.push(Call::CancelPending(pending_uid));
        Ok(())
    }

    fn cancel_controller_correlation(
        &mut self,
        correlation: &Self::Correlation,
    ) -> Result<(), TypedError> {
        self.calls.push(Call::CancelCorrelation(*correlation));
        Ok(())
    }
}

fn failure(code: ErrorCode, message: &str) -> TypedError {
    TypedError::new(code, message)
}

#[test]
fn source_is_captured_before_reservation_and_only_pending_history_precedes_exec() {
    let client_uid = uid(10);
    let pending_uid = client_uid;
    let mut runtime = FakeHandoff::managed(client_uid, pending_uid);

    let prepared = prepare_exec_handoff_with_runtime(&plan(), &mut runtime).unwrap();

    assert_eq!(prepared.pending_uid(), Some(pending_uid));
    assert_eq!(
        runtime.calls,
        [
            Call::Capture,
            Call::Reserve(None),
            Call::Stage {
                client_uid,
                has_source: true,
            },
            Call::Start(pending_uid),
            Call::Terminal,
        ]
    );
}

#[test]
fn exact_tmux_source_uid_is_reused_for_local_switch_correlation() {
    let client_uid = uid(20);
    let pending_uid = client_uid;
    let mut runtime = FakeHandoff::managed(client_uid, pending_uid);
    runtime.source = Ok(Some(Source {
        existing_client_uid: Some(client_uid),
    }));

    prepare_exec_handoff_with_runtime(&plan(), &mut runtime).unwrap();

    assert_eq!(runtime.calls[0], Call::Capture);
    assert_eq!(runtime.calls[1], Call::Reserve(Some(client_uid)));
}

#[test]
fn headless_and_unmanaged_handoffs_never_stage_global_gui_history() {
    let mut headless = FakeHandoff::terminal_only();
    prepare_exec_handoff_with_runtime(&plan(), &mut headless).unwrap();
    assert_eq!(
        headless.calls,
        [Call::Capture, Call::Reserve(None), Call::Terminal]
    );

    let client_uid = uid(30);
    let mut unmanaged = FakeHandoff::terminal_only();
    unmanaged.correlation = Ok(Some(client_uid));
    prepare_exec_handoff_with_runtime(&plan(), &mut unmanaged).unwrap();
    assert_eq!(
        unmanaged.calls,
        [
            Call::Capture,
            Call::Reserve(None),
            Call::Stage {
                client_uid,
                has_source: false,
            },
            Call::Terminal,
        ]
    );
}

#[test]
fn source_race_during_staging_cancels_reservation_before_terminal_history() {
    let client_uid = uid(40);
    let mut runtime = FakeHandoff::managed(client_uid, uid(41));
    runtime.pending = Err(failure(
        ErrorCode::IdentityConflict,
        "captured source heartbeat changed",
    ));

    let error = prepare_exec_handoff_with_runtime(&plan(), &mut runtime).unwrap_err();

    assert_eq!(error.code, ErrorCode::IdentityConflict);
    assert_eq!(
        runtime.calls,
        [
            Call::Capture,
            Call::Reserve(None),
            Call::Stage {
                client_uid,
                has_source: true,
            },
            Call::CancelCorrelation(client_uid),
        ]
    );
}

#[test]
fn monitor_or_terminal_failure_cancels_pending_then_correlation() {
    let client_uid = uid(50);
    let pending_uid = client_uid;
    let mut monitor_failure = FakeHandoff::managed(client_uid, pending_uid);
    monitor_failure.start = Err(failure(
        ErrorCode::OperationFailed,
        "monitor bootstrap failed",
    ));
    let error = prepare_exec_handoff_with_runtime(&plan(), &mut monitor_failure).unwrap_err();
    assert_eq!(error.code, ErrorCode::OperationFailed);
    assert_eq!(
        &monitor_failure.calls[4..],
        [
            Call::CancelPending(pending_uid),
            Call::CancelCorrelation(client_uid),
        ]
    );

    let mut terminal_failure = FakeHandoff::managed(client_uid, pending_uid);
    terminal_failure.terminal = Err(failure(
        ErrorCode::OperationFailed,
        "terminal history unavailable",
    ));
    let error = prepare_exec_handoff_with_runtime(&plan(), &mut terminal_failure).unwrap_err();
    assert_eq!(error.code, ErrorCode::OperationFailed);
    assert_eq!(
        &terminal_failure.calls[5..],
        [
            Call::CancelPending(pending_uid),
            Call::CancelCorrelation(client_uid),
        ]
    );
}

#[test]
fn pending_uid_and_terminal_only_contract_drift_fail_closed() {
    let client_uid = uid(60);
    let wrong_pending_uid = uid(61);
    let mut wrong_uid = FakeHandoff::managed(client_uid, wrong_pending_uid);

    let error = prepare_exec_handoff_with_runtime(&plan(), &mut wrong_uid).unwrap_err();

    assert_eq!(error.code, ErrorCode::ProtocolMismatch);
    assert_eq!(
        &wrong_uid.calls[3..],
        [
            Call::CancelPending(wrong_pending_uid),
            Call::CancelCorrelation(client_uid),
        ]
    );

    let mut terminal_only = FakeHandoff::terminal_only();
    terminal_only.correlation = Ok(Some(client_uid));
    terminal_only.pending = Ok(Some(client_uid));

    let error = prepare_exec_handoff_with_runtime(&plan(), &mut terminal_only).unwrap_err();

    assert_eq!(error.code, ErrorCode::ProtocolMismatch);
    assert_eq!(
        &terminal_only.calls[3..],
        [
            Call::CancelPending(client_uid),
            Call::CancelCorrelation(client_uid),
        ]
    );
}
