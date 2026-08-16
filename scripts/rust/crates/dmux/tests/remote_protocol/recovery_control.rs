//! P10 owner recovery control over the existing one-document agent
//! envelope. Targets and runtime paths are owner-resolved; resume is
//! idempotent both by envelope UID and by the create-new control sidecar.

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use dmux::locks::{self, LockMode, LockScope};
use dmux::model::{Backend, BackendInstanceUid, HostUid, ServerEpoch};
use dmux::recovery::{RecoveryControlAction, RecoveryControlRequest, RecoveryInspection};
use dmux::registry::recovery::{
    RECOVERY_GENERATION_PATH, RecoveryGenerationSpec, RecoveryNodeState,
};
use dmux::registry::{LeaseHolder, LeaseScope, NetworkClass, RouteSpec, Transport};
use dmux::remote::client::{
    AgentInvocation, DirectInvoker, RecoveryOwnerCommand, RecoveryOwnerContext, RecoveryOwnerReply,
    call_recovery_owner_with,
};
use dmux::remote::protocol;
use serde_json::json;
use uuid::Uuid;

use crate::util::{DMUX_BIN, Scratch, envelope, error_code};

fn failed_generation(
    scratch: &Scratch,
) -> (BackendInstanceUid, ServerEpoch, RecoveryGenerationSpec) {
    let socket = scratch.data.path().join("owner-wez.sock");
    let mut registry = scratch.registry();
    let instance = registry
        .register_backend_instance(Backend::Wez, Some(socket.to_str().unwrap()), None)
        .unwrap();
    let epoch = ServerEpoch(Uuid::new_v4());
    registry
        .publish_backend_server(
            instance,
            epoch,
            Some(4242),
            Some("recovery-test-owner"),
            None,
            None,
        )
        .unwrap();
    let kernel = locks::acquire(
        scratch.locks.path(),
        LockScope::BackendInstance(instance),
        LockMode::Exclusive,
    )
    .unwrap();
    let holder = LeaseHolder::current(Uuid::new_v4());
    let lease = registry
        .acquire_lease(
            &LeaseScope::Recovery(instance),
            &holder,
            Duration::from_secs(30),
            &kernel,
            None,
        )
        .unwrap();
    let generation = RecoveryGenerationSpec {
        generation_uid: Uuid::new_v4(),
        backend_instance: instance,
        server_epoch: epoch,
        manifest_id: "sha256:remote-recovery-test".into(),
    };
    registry.begin_recovery(&generation, &[], &lease).unwrap();
    registry
        .transition_recovery_node(
            generation.generation_uid,
            RECOVERY_GENERATION_PATH,
            RecoveryNodeState::Pending,
            RecoveryNodeState::Preparing,
            None,
            &lease,
        )
        .unwrap();
    registry
        .transition_recovery_node(
            generation.generation_uid,
            RECOVERY_GENERATION_PATH,
            RecoveryNodeState::Preparing,
            RecoveryNodeState::Restoring,
            None,
            &lease,
        )
        .unwrap();
    registry
        .transition_recovery_node(
            generation.generation_uid,
            RECOVERY_GENERATION_PATH,
            RecoveryNodeState::Restoring,
            RecoveryNodeState::Failed,
            None,
            &lease,
        )
        .unwrap();
    registry
        .release_lease(&LeaseScope::Recovery(instance), lease.holder_request_uid)
        .unwrap();
    drop(kernel);
    drop(registry);
    (instance, epoch, generation)
}

fn qualified(
    method: &str,
    request_uid: Uuid,
    instance: BackendInstanceUid,
    epoch: ServerEpoch,
) -> protocol::Envelope {
    let mut request = envelope(method, request_uid, json!({}));
    request.backend_instance_uid = Some(instance);
    request.server_epoch = Some(epoch);
    request
}

fn recovery_client(owner: &Scratch, tag: &str) -> (Scratch, HostUid) {
    let owner_uid = owner.registry().identity().unwrap().host_uid;
    let client = Scratch::new(tag);
    let mut registry = client.registry();
    registry.enroll_host(owner_uid, None).unwrap();
    registry
        .upsert_route(&RouteSpec {
            host_uid: owner_uid,
            transport: Transport::Openssh,
            endpoint: "owner-direct".into(),
            username: None,
            wez_domain: None,
            network_class: NetworkClass::Other,
            priority: 10,
            required_capability: None,
            trust_fingerprint: None,
            enabled: true,
        })
        .unwrap();
    drop(registry);
    (client, owner_uid)
}

#[test]
fn recovery_status_and_resume_are_owner_scoped_and_idempotent() {
    let scratch = Scratch::new("remote-recovery");
    let (instance, epoch, generation) = failed_generation(&scratch);

    let status = qualified(
        protocol::methods::RECOVERY_STATUS,
        Uuid::new_v4(),
        instance,
        epoch,
    );
    let (code, response) = scratch.agent(&status);
    assert_eq!(code, 0, "{response:?}");
    assert_eq!(response.backend_instance_uid, Some(instance));
    assert_eq!(response.server_epoch, Some(epoch));
    let inspection: RecoveryInspection = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert_eq!(inspection.backend_instance_uid, instance);
    assert_eq!(inspection.server_epoch, Some(epoch));
    assert_eq!(
        inspection.generation.as_ref().unwrap().generation_uid,
        generation.generation_uid
    );
    assert_eq!(inspection.journal.len(), 1);
    assert_eq!(inspection.journal[0].node_state, RecoveryNodeState::Failed);

    let resume = qualified(
        protocol::methods::RECOVERY_RESUME,
        Uuid::new_v4(),
        instance,
        epoch,
    );
    let (code, response) = scratch.agent(&resume);
    assert_eq!(code, 0, "{response:?}");
    let first: RecoveryControlRequest = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert_eq!(first.backend_instance_uid, instance);
    assert_eq!(first.server_epoch, epoch);

    // Same envelope UID returns its ledger-cached receipt.
    let (code, response) = scratch.agent(&resume);
    assert_eq!(code, 0, "{response:?}");
    let replay: RecoveryControlRequest = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert_eq!(replay, first);

    // A distinct envelope while the same exact control is pending is also
    // deduplicated by the owner's create-new sidecar, never replaced.
    let distinct = qualified(
        protocol::methods::RECOVERY_RESUME,
        Uuid::new_v4(),
        instance,
        epoch,
    );
    let (code, response) = scratch.agent(&distinct);
    assert_eq!(code, 0, "{response:?}");
    let deduped: RecoveryControlRequest =
        serde_json::from_value(response.payload.unwrap()).unwrap();
    assert_eq!(deduped, first);

    let control = scratch
        .locks
        .path()
        .join("recovery")
        .join(epoch.0.to_string())
        .join("control.json");
    let metadata = std::fs::metadata(&control).unwrap();
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
}

#[test]
fn recovery_abort_is_owner_scoped_replay_safe_and_conflicts_with_resume() {
    let owner = Scratch::new("remote-recovery-abort");
    let (instance, epoch, _) = failed_generation(&owner);
    let abort = qualified(
        protocol::methods::RECOVERY_ABORT,
        Uuid::new_v4(),
        instance,
        epoch,
    );
    let (code, response) = owner.agent(&abort);
    assert_eq!(code, 0, "{response:?}");
    let first: RecoveryControlRequest = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert_eq!(first.action, RecoveryControlAction::Abort);
    assert_eq!(first.backend_instance_uid, instance);
    assert_eq!(first.server_epoch, epoch);

    let (code, response) = owner.agent(&abort);
    assert_eq!(code, 0, "{response:?}");
    let replay: RecoveryControlRequest = serde_json::from_value(response.payload.unwrap()).unwrap();
    assert_eq!(replay, first);

    let distinct_abort = qualified(
        protocol::methods::RECOVERY_ABORT,
        Uuid::new_v4(),
        instance,
        epoch,
    );
    let (code, response) = owner.agent(&distinct_abort);
    assert_eq!(code, 0, "{response:?}");
    let deduped: RecoveryControlRequest =
        serde_json::from_value(response.payload.unwrap()).unwrap();
    assert_eq!(deduped, first);

    // An opposite action may not replace a pending abort control sidecar.
    let resume = qualified(
        protocol::methods::RECOVERY_RESUME,
        Uuid::new_v4(),
        instance,
        epoch,
    );
    let (code, response) = owner.agent(&resume);
    assert_ne!(code, 0, "{response:?}");
    assert_eq!(error_code(&response), "operation_failed");

    // The controller convenience seam reaches the same exact owner receipt
    // through enrolled-route identity and lineage validation.
    let (client, owner_uid) = recovery_client(&owner, "remote-recovery-abort-controller");
    let invocation = AgentInvocation {
        method: "ignored".into(),
        protocol: 0,
        remote_bin: DMUX_BIN.to_string(),
        data_dir: Some(owner.data.path().display().to_string()),
        lock_dir: Some(owner.locks.path().display().to_string()),
    };
    let mut registry = client.registry();
    let outcome = call_recovery_owner_with(
        &mut registry,
        RecoveryOwnerContext::qualified(owner_uid, instance, epoch),
        RecoveryOwnerCommand::Abort,
        &DirectInvoker,
        &invocation,
        Duration::from_secs(10),
    )
    .unwrap();
    let RecoveryOwnerReply::Control(receipt) = outcome.reply else {
        panic!("abort command returned an inspection")
    };
    assert_eq!(receipt, first);
}

#[test]
fn recovery_rpc_rejects_controller_paths_and_stale_owner_claims() {
    let scratch = Scratch::new("remote-recovery-refusal");
    let (instance, epoch, _) = failed_generation(&scratch);

    let with_path = envelope(
        protocol::methods::RECOVERY_STATUS,
        Uuid::new_v4(),
        json!({ "runtime_dir": "/controller/injected" }),
    );
    let (code, response) = scratch.agent(&with_path);
    assert_eq!(code, 2, "{response:?}");
    assert_eq!(error_code(&response), "usage");

    let mut stale = qualified(
        protocol::methods::RECOVERY_STATUS,
        Uuid::new_v4(),
        instance,
        epoch,
    );
    stale.server_epoch = Some(ServerEpoch(Uuid::new_v4()));
    let (code, response) = scratch.agent(&stale);
    assert_eq!(code, 1, "{response:?}");
    assert_eq!(error_code(&response), "backend_epoch_changed");

    let mut wrong = qualified(
        protocol::methods::RECOVERY_RESUME,
        Uuid::new_v4(),
        instance,
        epoch,
    );
    wrong.backend_instance_uid = Some(BackendInstanceUid(Uuid::new_v4()));
    let (code, response) = scratch.agent(&wrong);
    assert_eq!(code, 1, "{response:?}");
    assert_eq!(error_code(&response), "wrong_backend_instance");
}

#[test]
fn controller_helper_owns_route_lineage_and_typed_recovery_parsing() {
    let owner = Scratch::new("remote-recovery-owner-call");
    let (instance, epoch, generation) = failed_generation(&owner);
    let (client, owner_uid) = recovery_client(&owner, "remote-recovery-controller");
    let invocation = AgentInvocation {
        // The helper must replace these two values from the typed command.
        method: "intentionally-wrong".into(),
        protocol: u32::MAX,
        remote_bin: DMUX_BIN.to_string(),
        data_dir: Some(owner.data.path().display().to_string()),
        lock_dir: Some(owner.locks.path().display().to_string()),
    };
    let mut registry = client.registry();
    let status = call_recovery_owner_with(
        &mut registry,
        RecoveryOwnerContext::new(owner_uid),
        RecoveryOwnerCommand::Status,
        &DirectInvoker,
        &invocation,
        Duration::from_secs(10),
    )
    .unwrap();
    let RecoveryOwnerReply::Status(inspection) = status.reply else {
        panic!("status command returned a control receipt")
    };
    assert_eq!(inspection.backend_instance_uid, instance);
    assert_eq!(inspection.server_epoch, Some(epoch));
    assert_eq!(
        inspection.generation.unwrap().generation_uid,
        generation.generation_uid
    );
    assert!(registry.peer_cache(owner_uid).unwrap().is_some());

    let resumed = call_recovery_owner_with(
        &mut registry,
        RecoveryOwnerContext::qualified(owner_uid, instance, epoch),
        RecoveryOwnerCommand::Resume,
        &DirectInvoker,
        &invocation,
        Duration::from_secs(10),
    )
    .unwrap();
    let RecoveryOwnerReply::Control(receipt) = resumed.reply else {
        panic!("resume command returned an inspection")
    };
    assert_eq!(receipt.action, RecoveryControlAction::Resume);
    assert_eq!(receipt.backend_instance_uid, instance);
    assert_eq!(receipt.server_epoch, epoch);
}
