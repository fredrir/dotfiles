//! P10 cold-recovery registry surface: intentional-empty floors, atomic
//! generation creation/replay, fenced compare-and-set transitions, and
//! unfinished-generation lookup. Every test uses an injected scratch
//! registry and lock directory.

use std::time::Duration;

use dmux::bootstrap::{BootstrapJournal, IssuedRequest};
use dmux::locks::{self, HeldLock, LockMode, LockScope};
use dmux::model::{Backend, BackendInstanceUid, Lifecycle, OperationState, ServerEpoch, SpaceUid};
use dmux::registry::recovery::{
    BeginRecovery, RECOVERY_GENERATION_PATH, RecoveryGenerationSpec, RecoveryNodeSpec,
    RecoveryNodeState,
};
use dmux::registry::{
    Lease, LeaseHolder, LeaseScope, NativeBindingSpec, NativeKind, Registry, RegistryError,
};
use uuid::Uuid;

use crate::util::{Scratch, open, reserve, scratch, tmux_instance};

const TTL: Duration = Duration::from_secs(30);

#[test]
fn backend_lookup_is_read_only_and_never_registers_a_missing_instance() {
    let s = scratch();
    let mut reg = open(&s.config);
    let initial = reg.authority_head().unwrap();
    assert_eq!(
        reg.backend_instance_for_backend(Backend::Wez).unwrap(),
        None
    );
    assert_eq!(reg.authority_head().unwrap(), initial);

    let instance = reg
        .register_backend_instance(Backend::Wez, Some("/tmp/lookup.sock"), Some("lookup"))
        .unwrap();
    let after_registration = reg.authority_head().unwrap();
    assert_eq!(
        reg.backend_instance_for_backend(Backend::Wez).unwrap(),
        Some(instance)
    );
    assert_eq!(
        reg.backend_instance_for_backend(Backend::Tmux).unwrap(),
        None
    );
    assert_eq!(reg.authority_head().unwrap(), after_registration);

    reg.raw_connection()
        .execute(
            "UPDATE backend_instances SET backend_instance_uid = 'not-a-uuid' \
             WHERE backend_instance_uid = ?1",
            [instance.0.to_string()],
        )
        .unwrap();
    assert!(matches!(
        reg.backend_instance_for_backend(Backend::Wez),
        Err(RegistryError::Corrupt(_))
    ));
    assert_eq!(reg.authority_head().unwrap(), after_registration);
}

fn published_instance(reg: &mut Registry) -> (BackendInstanceUid, ServerEpoch) {
    let instance = tmux_instance(reg);
    let epoch = ServerEpoch(Uuid::new_v4());
    reg.publish_backend_server(instance, epoch, Some(123), Some("start"), None, None)
        .unwrap();
    (instance, epoch)
}

fn recovery_setup() -> (
    Scratch,
    Registry,
    BackendInstanceUid,
    ServerEpoch,
    HeldLock,
    Lease,
) {
    let scratch = scratch();
    let mut reg = open(&scratch.config);
    let (instance, epoch) = published_instance(&mut reg);
    let kernel = locks::acquire(
        &scratch.config.lock_dir,
        LockScope::BackendInstance(instance),
        LockMode::Exclusive,
    )
    .unwrap();
    let holder = LeaseHolder::current(Uuid::new_v4());
    let lease = reg
        .acquire_lease(&LeaseScope::Recovery(instance), &holder, TTL, &kernel, None)
        .unwrap();
    (scratch, reg, instance, epoch, kernel, lease)
}

fn generation(instance: BackendInstanceUid, epoch: ServerEpoch) -> RecoveryGenerationSpec {
    RecoveryGenerationSpec {
        generation_uid: Uuid::new_v4(),
        backend_instance: instance,
        server_epoch: epoch,
        manifest_id: "sha256:manifest-a".into(),
    }
}

fn node(path: &str, space_uid: Option<SpaceUid>) -> RecoveryNodeSpec {
    RecoveryNodeSpec {
        space_uid,
        manifest_node_path: path.into(),
    }
}

fn issue_bootstrap(
    reg: &mut Registry,
    spec: &RecoveryGenerationSpec,
    path: &str,
    space_uid: Option<SpaceUid>,
) -> Uuid {
    let uid = Uuid::new_v4();
    reg.bootstrap_issue(&IssuedRequest {
        request_uid: uid,
        operation_uid: None,
        space_uid,
        backend_instance: spec.backend_instance,
        server_epoch: spec.server_epoch,
        intended_parent: None,
        recovery_generation: Some(spec.generation_uid.to_string()),
        manifest_node_path: Some(path.into()),
    })
    .unwrap();
    uid
}

/// The intentional-empty floor's guards, asserted on the production entry
/// point that raises it (`registry::recovery` module docs, ADR 012 WS-E.3
/// row 8): the exact exclusive backend-instance kernel lock, a published
/// epoch, the floor captured from the current head inside the abort's own
/// transaction, never lowered, and no authority revision advanced.
#[test]
fn the_empty_floor_is_raised_only_under_the_exact_exclusive_lock_and_published_epoch() {
    let (s, mut reg, instance, epoch, kernel, lease) = recovery_setup();
    assert_eq!(reg.intentional_empty_revision(instance).unwrap(), None);
    let spec = generation(instance, epoch);
    reg.begin_recovery(&spec, &[], &lease).unwrap();
    // Registering an instance is an identity mutation that advances the
    // chain, so the head is captured after it.
    let wrong_instance = reg
        .register_backend_instance(Backend::Wez, Some("/tmp/w8.sock"), Some("test"))
        .unwrap();
    let head = reg.authority_head().unwrap();

    // Another instance's exclusive lock, or a shared lock on this one, is
    // refused before any write; so is an epoch the instance does not publish.
    let wrong_kernel = locks::acquire(
        &s.config.lock_dir,
        LockScope::BackendInstance(wrong_instance),
        LockMode::Exclusive,
    )
    .unwrap();
    assert!(matches!(
        reg.abort_recovery_generation_and_record_current_empty(
            spec.generation_uid,
            RecoveryNodeState::Pending,
            epoch,
            &wrong_kernel,
            &lease,
        ),
        Err(RegistryError::KernelLockMismatch { .. })
    ));
    drop(wrong_kernel);
    drop(kernel);
    let shared = locks::acquire(
        &s.config.lock_dir,
        LockScope::BackendInstance(instance),
        LockMode::Shared,
    )
    .unwrap();
    assert!(matches!(
        reg.abort_recovery_generation_and_record_current_empty(
            spec.generation_uid,
            RecoveryNodeState::Pending,
            epoch,
            &shared,
            &lease,
        ),
        Err(RegistryError::KernelLockMismatch { .. })
    ));
    drop(shared);
    let kernel = locks::acquire(
        &s.config.lock_dir,
        LockScope::BackendInstance(instance),
        LockMode::Exclusive,
    )
    .unwrap();
    assert!(
        reg.abort_recovery_generation_and_record_current_empty(
            spec.generation_uid,
            RecoveryNodeState::Pending,
            ServerEpoch(Uuid::new_v4()),
            &kernel,
            &lease,
        )
        .is_err()
    );
    assert_eq!(reg.intentional_empty_revision(instance).unwrap(), None);
    assert_eq!(
        reg.recovery_rows(spec.generation_uid).unwrap()[0].node_state,
        RecoveryNodeState::Pending
    );
    assert_eq!(reg.authority_head().unwrap(), head);

    // The floor is the head captured inside the abort's own transaction,
    // and the abort advances no revision.
    let (floor, rows) = reg
        .abort_recovery_generation_and_record_current_empty(
            spec.generation_uid,
            RecoveryNodeState::Pending,
            epoch,
            &kernel,
            &lease,
        )
        .unwrap();
    assert_eq!(floor, head.revision);
    assert_eq!(
        reg.intentional_empty_revision(instance).unwrap(),
        Some(floor)
    );
    assert!(
        rows.iter()
            .all(|row| row.node_state == RecoveryNodeState::Aborted)
    );
    assert_eq!(reg.authority_head().unwrap(), head);

    // A later real authority mutation permits a strictly higher floor; the
    // next abort raises it, a replay of the first cannot lower it, and
    // neither adds a revision.
    reserve(&mut reg, "advance-head", instance);
    let later = reg.authority_head().unwrap();
    assert!(later.revision > head.revision);
    let next = generation(instance, epoch);
    reg.begin_recovery(&next, &[], &lease).unwrap();
    let (raised, _) = reg
        .abort_recovery_generation_and_record_current_empty(
            next.generation_uid,
            RecoveryNodeState::Pending,
            epoch,
            &kernel,
            &lease,
        )
        .unwrap();
    assert_eq!(raised, later.revision);
    assert_eq!(
        reg.intentional_empty_revision(instance).unwrap(),
        Some(later.revision)
    );
    let (replayed, _) = reg
        .abort_recovery_generation_and_record_current_empty(
            spec.generation_uid,
            RecoveryNodeState::Pending,
            epoch,
            &kernel,
            &lease,
        )
        .unwrap();
    assert_eq!(replayed, later.revision, "a replay never lowers the floor");
    assert_eq!(reg.authority_head().unwrap(), later);
}

#[test]
fn final_wez_tombstone_and_empty_floor_are_one_rollback_safe_transition() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = reg
        .register_backend_instance(Backend::Wez, Some("/tmp/final-empty.sock"), Some("test"))
        .unwrap();
    let epoch = ServerEpoch(Uuid::new_v4());
    reg.publish_backend_server(instance, epoch, Some(123), Some("start"), None, None)
        .unwrap();
    let aborted = reserve(&mut reg, "old-failed-reservation", instance);
    reg.abort_create(aborted.space_uid, aborted.operation_uid)
        .unwrap();
    assert_eq!(
        reg.space(aborted.space_uid).unwrap().lifecycle,
        Lifecycle::Aborted
    );
    let reservation = reserve(&mut reg, "last-wez-space", instance);
    reg.finalize_create(
        reservation.space_uid,
        reservation.operation_uid,
        &NativeBindingSpec {
            native_token: "dmux:last-wez-space".into(),
            native_kind: NativeKind::WezWorkspaceKey,
            server_epoch: Some(epoch),
        },
    )
    .unwrap();
    let operation_uid = reg
        .begin_remove(reservation.space_uid, Uuid::new_v4())
        .unwrap();
    let kernel = locks::acquire(
        &s.config.lock_dir,
        LockScope::BackendInstance(instance),
        LockMode::Exclusive,
    )
    .unwrap();
    let head_before = reg.authority_head().unwrap();

    // Fail after the tombstone/binding/journal/revision statements have run
    // but before the floor can be stored. SQLite must roll the whole
    // immediate transaction back; no terminal half-state is recoverable.
    reg.raw_connection()
        .execute_batch(
            "CREATE TEMP TRIGGER inject_final_empty_floor_failure \
             BEFORE UPDATE OF intentional_empty_revision ON backend_instances \
             BEGIN SELECT RAISE(ABORT, 'injected final-empty floor failure'); END;",
        )
        .unwrap();
    assert!(
        reg.complete_remove_intentionally_empty(
            reservation.space_uid,
            operation_uid,
            instance,
            epoch,
            &kernel,
        )
        .is_err()
    );
    assert_eq!(
        reg.space(reservation.space_uid).unwrap().lifecycle,
        Lifecycle::Deleting
    );
    assert_eq!(
        reg.operation(operation_uid).unwrap().state,
        OperationState::Prepared
    );
    assert!(
        reg.current_binding(reservation.space_uid)
            .unwrap()
            .is_some()
    );
    assert_eq!(reg.intentional_empty_revision(instance).unwrap(), None);
    assert_eq!(reg.authority_head().unwrap(), head_before);

    reg.raw_connection()
        .execute_batch("DROP TRIGGER inject_final_empty_floor_failure")
        .unwrap();
    let floor = reg
        .complete_remove_intentionally_empty(
            reservation.space_uid,
            operation_uid,
            instance,
            epoch,
            &kernel,
        )
        .unwrap();
    assert_eq!(
        reg.space(reservation.space_uid).unwrap().lifecycle,
        Lifecycle::Deleted
    );
    assert_eq!(
        reg.operation(operation_uid).unwrap().state,
        OperationState::Completed
    );
    assert!(
        reg.current_binding(reservation.space_uid)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        reg.intentional_empty_revision(instance).unwrap(),
        Some(floor)
    );
    assert_eq!(reg.authority_head().unwrap().revision, floor);
    assert_eq!(floor, head_before.revision + 1);
    assert_eq!(
        reg.space(aborted.space_uid).unwrap().lifecycle,
        Lifecycle::Aborted,
        "terminal identity history must not prevent an intentional-empty floor"
    );
}

#[test]
fn intentional_empty_completion_refuses_when_another_space_remains() {
    for (other_state, expected) in [
        ("reserved", Lifecycle::Reserved),
        ("active", Lifecycle::Active),
        ("deleting", Lifecycle::Deleting),
        ("conflict", Lifecycle::Conflict),
    ] {
        let s = scratch();
        let mut reg = open(&s.config);
        let instance = reg
            .register_backend_instance(Backend::Wez, Some("/tmp/not-final.sock"), Some("test"))
            .unwrap();
        let epoch = ServerEpoch(Uuid::new_v4());
        reg.publish_backend_server(instance, epoch, Some(123), Some("start"), None, None)
            .unwrap();
        let first = reserve(&mut reg, "first", instance);
        reg.finalize_create(
            first.space_uid,
            first.operation_uid,
            &NativeBindingSpec {
                native_token: "dmux:first".into(),
                native_kind: NativeKind::WezWorkspaceKey,
                server_epoch: Some(epoch),
            },
        )
        .unwrap();
        let second = reserve(&mut reg, "second", instance);
        match other_state {
            "reserved" => {}
            "active" | "deleting" => {
                reg.finalize_create(
                    second.space_uid,
                    second.operation_uid,
                    &NativeBindingSpec {
                        native_token: "dmux:second".into(),
                        native_kind: NativeKind::WezWorkspaceKey,
                        server_epoch: Some(epoch),
                    },
                )
                .unwrap();
                if other_state == "deleting" {
                    reg.begin_remove(second.space_uid, Uuid::new_v4()).unwrap();
                }
            }
            "conflict" => {
                reg.raw_connection()
                    .execute(
                        "UPDATE spaces SET lifecycle = 'conflict' WHERE space_uid = ?1",
                        [second.space_uid.0.to_string()],
                    )
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let operation_uid = reg.begin_remove(first.space_uid, Uuid::new_v4()).unwrap();
        let kernel = locks::acquire(
            &s.config.lock_dir,
            LockScope::BackendInstance(instance),
            LockMode::Exclusive,
        )
        .unwrap();
        let head = reg.authority_head().unwrap();

        let error = reg
            .complete_remove_intentionally_empty(
                first.space_uid,
                operation_uid,
                instance,
                epoch,
                &kernel,
            )
            .unwrap_err();
        assert!(
            error.to_string().contains("other live Spaces"),
            "{other_state}: {error}"
        );
        assert_eq!(
            reg.space(first.space_uid).unwrap().lifecycle,
            Lifecycle::Deleting,
            "{other_state}"
        );
        assert_eq!(
            reg.operation(operation_uid).unwrap().state,
            OperationState::Prepared,
            "{other_state}"
        );
        assert_eq!(
            reg.space(second.space_uid).unwrap().lifecycle,
            expected,
            "{other_state}"
        );
        assert_eq!(
            reg.intentional_empty_revision(instance).unwrap(),
            None,
            "{other_state}"
        );
        assert_eq!(reg.authority_head().unwrap(), head, "{other_state}");
    }
}

#[test]
fn begin_is_atomic_and_only_exact_replay_is_accepted() {
    let (_s, mut reg, instance, epoch, _kernel, lease) = recovery_setup();
    let first = reserve(&mut reg, "one", instance).space_uid;
    let second = reserve(&mut reg, "two", instance).space_uid;
    let spec = generation(instance, epoch);
    let nodes = vec![
        node("spaces/2/groups/0", Some(second)),
        node("spaces/1", Some(first)),
    ];

    let created = reg.begin_recovery(&spec, &nodes, &lease).unwrap();
    let BeginRecovery::Created(created_rows) = created else {
        panic!("first begin must create")
    };
    assert_eq!(created_rows.len(), 3);
    assert_eq!(created_rows[0].manifest_node_path, RECOVERY_GENERATION_PATH);
    assert_eq!(created_rows[0].space_uid, None);
    assert!(created_rows.iter().all(|row| {
        row.generation_uid == spec.generation_uid
            && row.backend_instance == instance
            && row.server_epoch == epoch
            && row.manifest_id == spec.manifest_id
            && row.node_state == RecoveryNodeState::Pending
            && row.bootstrap_request_uid.is_none()
            && !row.updated_at.is_empty()
    }));

    // Order at the call site is not identity; exact paths and SpaceUids are.
    let reversed = vec![nodes[1].clone(), nodes[0].clone()];
    let replay = reg.begin_recovery(&spec, &reversed, &lease).unwrap();
    let BeginRecovery::Replay(replay_rows) = replay else {
        panic!("same identity must replay")
    };
    assert_eq!(replay_rows, created_rows);

    let mut changed_manifest = spec.clone();
    changed_manifest.manifest_id = "sha256:different".into();
    assert!(
        reg.begin_recovery(&changed_manifest, &nodes, &lease)
            .is_err()
    );
    let changed_nodes = vec![node("spaces/1", Some(second))];
    assert!(reg.begin_recovery(&spec, &changed_nodes, &lease).is_err());

    let different = generation(instance, epoch);
    assert!(reg.begin_recovery(&different, &[], &lease).is_err());
    assert!(
        reg.recovery_rows(different.generation_uid)
            .unwrap()
            .is_empty(),
        "refused generation must be all-or-nothing"
    );
}

#[test]
fn begin_rejects_empty_duplicate_reserved_paths_and_epoch_mismatch_atomically() {
    let (_s, mut reg, instance, epoch, _kernel, lease) = recovery_setup();
    for nodes in [
        vec![node("", None)],
        vec![node("same", None), node("same", None)],
        vec![node(RECOVERY_GENERATION_PATH, None)],
    ] {
        let spec = generation(instance, epoch);
        assert!(reg.begin_recovery(&spec, &nodes, &lease).is_err());
        assert!(reg.recovery_rows(spec.generation_uid).unwrap().is_empty());
    }

    let wrong_epoch = generation(instance, ServerEpoch(Uuid::new_v4()));
    assert!(reg.begin_recovery(&wrong_epoch, &[], &lease).is_err());
    assert!(
        reg.recovery_rows(wrong_epoch.generation_uid)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn stale_or_wrong_scope_lease_is_rejected_by_every_fenced_write() {
    let (_s, mut reg, instance, epoch, kernel, lease) = recovery_setup();
    let spec = generation(instance, epoch);
    reg.assert_lease_fence(&LeaseScope::Recovery(instance), &lease)
        .unwrap();
    assert!(
        reg.assert_lease_fence(&LeaseScope::Backend(instance), &lease)
            .is_err()
    );

    let backend_holder = LeaseHolder::current(Uuid::new_v4());
    let wrong_scope_lease = reg
        .acquire_lease(
            &LeaseScope::Backend(instance),
            &backend_holder,
            TTL,
            &kernel,
            None,
        )
        .unwrap();
    assert!(reg.begin_recovery(&spec, &[], &wrong_scope_lease).is_err());
    assert!(reg.recovery_rows(spec.generation_uid).unwrap().is_empty());

    reg.begin_recovery(&spec, &[], &lease).unwrap();

    reg.release_lease(&LeaseScope::Recovery(instance), lease.holder_request_uid)
        .unwrap();
    assert!(
        reg.assert_lease_fence(&LeaseScope::Recovery(instance), &lease)
            .is_err()
    );
    assert!(reg.begin_recovery(&spec, &[], &lease).is_err());
    assert!(
        reg.transition_recovery_node(
            spec.generation_uid,
            RECOVERY_GENERATION_PATH,
            RecoveryNodeState::Pending,
            RecoveryNodeState::Preparing,
            None,
            &lease,
        )
        .is_err()
    );
    assert_eq!(
        reg.recovery_rows(spec.generation_uid).unwrap()[0].node_state,
        RecoveryNodeState::Pending,
        "stale fence must not mutate the row"
    );
}

#[test]
fn node_and_generation_transition_matrices_are_fenced_and_idempotent() {
    let (_s, mut reg, instance, epoch, _kernel, lease) = recovery_setup();
    let space = reserve(&mut reg, "space", instance).space_uid;
    let spec = generation(instance, epoch);
    let path = "spaces/1/groups/0/splits/0";
    let skipped = "spaces/deleted";
    reg.begin_recovery(
        &spec,
        &[node(path, Some(space)), node(skipped, None)],
        &lease,
    )
    .unwrap();

    let request = issue_bootstrap(&mut reg, &spec, path, Some(space));
    let preparing = reg
        .transition_recovery_node(
            spec.generation_uid,
            path,
            RecoveryNodeState::Pending,
            RecoveryNodeState::Preparing,
            Some(request),
            &lease,
        )
        .unwrap();
    assert_eq!(preparing.bootstrap_request_uid, Some(request));
    assert!(
        reg.transition_recovery_node(
            spec.generation_uid,
            path,
            RecoveryNodeState::Pending,
            RecoveryNodeState::Restoring,
            Some(request),
            &lease,
        )
        .is_err(),
        "compare-and-set refuses a stale expected state"
    );

    // Lost acknowledgement: already-at-target is accepted only with the
    // exact linked request identity.
    assert_eq!(
        reg.transition_recovery_node(
            spec.generation_uid,
            path,
            RecoveryNodeState::Pending,
            RecoveryNodeState::Preparing,
            Some(request),
            &lease,
        )
        .unwrap(),
        preparing
    );
    assert!(
        reg.transition_recovery_node(
            spec.generation_uid,
            path,
            RecoveryNodeState::Pending,
            RecoveryNodeState::Preparing,
            None,
            &lease,
        )
        .is_err()
    );

    // None preserves the request on a real state-only transition.
    let restoring = reg
        .transition_recovery_node(
            spec.generation_uid,
            path,
            RecoveryNodeState::Preparing,
            RecoveryNodeState::Restoring,
            None,
            &lease,
        )
        .unwrap();
    assert_eq!(restoring.bootstrap_request_uid, Some(request));
    assert!(
        reg.transition_recovery_node(
            spec.generation_uid,
            path,
            RecoveryNodeState::Restoring,
            RecoveryNodeState::Skipped,
            None,
            &lease,
        )
        .is_err()
    );

    // A retry may replace the linked request while moving back to preparing.
    let retry_request = issue_bootstrap(&mut reg, &spec, path, Some(space));
    let retrying = reg
        .transition_recovery_node(
            spec.generation_uid,
            path,
            RecoveryNodeState::Restoring,
            RecoveryNodeState::Preparing,
            Some(retry_request),
            &lease,
        )
        .unwrap();
    assert_eq!(retrying.bootstrap_request_uid, Some(retry_request));
    reg.transition_recovery_node(
        spec.generation_uid,
        path,
        RecoveryNodeState::Preparing,
        RecoveryNodeState::Failed,
        None,
        &lease,
    )
    .unwrap();
    reg.transition_recovery_node(
        spec.generation_uid,
        path,
        RecoveryNodeState::Failed,
        RecoveryNodeState::Preparing,
        None,
        &lease,
    )
    .unwrap();
    reg.transition_recovery_node(
        spec.generation_uid,
        path,
        RecoveryNodeState::Preparing,
        RecoveryNodeState::Restoring,
        None,
        &lease,
    )
    .unwrap();
    reg.transition_recovery_node(
        spec.generation_uid,
        path,
        RecoveryNodeState::Restoring,
        RecoveryNodeState::Completed,
        None,
        &lease,
    )
    .unwrap();
    assert!(
        reg.transition_recovery_node(
            spec.generation_uid,
            path,
            RecoveryNodeState::Completed,
            RecoveryNodeState::Preparing,
            None,
            &lease,
        )
        .is_err(),
        "completed is terminal"
    );

    reg.transition_recovery_node(
        spec.generation_uid,
        skipped,
        RecoveryNodeState::Pending,
        RecoveryNodeState::Skipped,
        None,
        &lease,
    )
    .unwrap();
    assert!(
        reg.transition_recovery_node(
            spec.generation_uid,
            skipped,
            RecoveryNodeState::Skipped,
            RecoveryNodeState::Preparing,
            None,
            &lease,
        )
        .is_err(),
        "skipped is terminal"
    );

    assert!(
        reg.transition_recovery_node(
            spec.generation_uid,
            RECOVERY_GENERATION_PATH,
            RecoveryNodeState::Pending,
            RecoveryNodeState::Skipped,
            None,
            &lease,
        )
        .is_err(),
        "the generation root has its narrower matrix"
    );
    for (from, to) in [
        (RecoveryNodeState::Pending, RecoveryNodeState::Preparing),
        (RecoveryNodeState::Preparing, RecoveryNodeState::Restoring),
        (RecoveryNodeState::Restoring, RecoveryNodeState::Failed),
        (RecoveryNodeState::Failed, RecoveryNodeState::Preparing),
        (RecoveryNodeState::Preparing, RecoveryNodeState::Restoring),
        (RecoveryNodeState::Restoring, RecoveryNodeState::Completed),
    ] {
        reg.transition_recovery_node(
            spec.generation_uid,
            RECOVERY_GENERATION_PATH,
            from,
            to,
            None,
            &lease,
        )
        .unwrap();
    }
}

#[test]
fn unfinished_lookup_tracks_nonterminal_root_and_rows_are_path_ordered() {
    let (_s, mut reg, instance, epoch, _kernel, lease) = recovery_setup();
    let spec = generation(instance, epoch);
    reg.begin_recovery(
        &spec,
        &[node("z", None), node("a", None), node("m", None)],
        &lease,
    )
    .unwrap();

    let (found, found_rows) = reg
        .unfinished_recovery_for_instance(instance)
        .unwrap()
        .expect("pending root is unfinished");
    assert_eq!(found, spec);
    assert_eq!(
        found.server_epoch, epoch,
        "the root carries the epoch it was journaled under; the caller compares it"
    );
    assert_eq!(
        found_rows
            .iter()
            .map(|row| row.manifest_node_path.as_str())
            .collect::<Vec<_>>(),
        vec![RECOVERY_GENERATION_PATH, "a", "m", "z"]
    );
    reg.transition_recovery_node(
        spec.generation_uid,
        RECOVERY_GENERATION_PATH,
        RecoveryNodeState::Pending,
        RecoveryNodeState::Preparing,
        None,
        &lease,
    )
    .unwrap();
    reg.transition_recovery_node(
        spec.generation_uid,
        RECOVERY_GENERATION_PATH,
        RecoveryNodeState::Preparing,
        RecoveryNodeState::Restoring,
        None,
        &lease,
    )
    .unwrap();
    reg.transition_recovery_node(
        spec.generation_uid,
        RECOVERY_GENERATION_PATH,
        RecoveryNodeState::Restoring,
        RecoveryNodeState::Failed,
        None,
        &lease,
    )
    .unwrap();
    assert!(
        reg.unfinished_recovery_for_instance(instance)
            .unwrap()
            .is_some(),
        "failed remains resumable"
    );
    reg.transition_recovery_node(
        spec.generation_uid,
        RECOVERY_GENERATION_PATH,
        RecoveryNodeState::Failed,
        RecoveryNodeState::Preparing,
        None,
        &lease,
    )
    .unwrap();
    reg.transition_recovery_node(
        spec.generation_uid,
        RECOVERY_GENERATION_PATH,
        RecoveryNodeState::Preparing,
        RecoveryNodeState::Restoring,
        None,
        &lease,
    )
    .unwrap();
    reg.transition_recovery_node(
        spec.generation_uid,
        RECOVERY_GENERATION_PATH,
        RecoveryNodeState::Restoring,
        RecoveryNodeState::Completed,
        None,
        &lease,
    )
    .unwrap();
    assert!(
        reg.unfinished_recovery_for_instance(instance)
            .unwrap()
            .is_none()
    );

    // A completed root no longer blocks a fresh generation on the instance.
    let next = generation(instance, epoch);
    assert!(matches!(
        reg.begin_recovery(&next, &[], &lease).unwrap(),
        BeginRecovery::Created(_)
    ));
}

#[test]
fn child_abort_is_explicit_fenced_and_terminal_from_every_nonterminal_state() {
    let (_s, mut reg, instance, epoch, _kernel, lease) = recovery_setup();
    let spec = generation(instance, epoch);
    let pending = "nodes/pending";
    let preparing = "nodes/preparing";
    let restoring = "nodes/restoring";
    let failed = "nodes/failed";
    let completed = "nodes/completed";
    let skipped = "nodes/skipped";
    reg.begin_recovery(
        &spec,
        &[
            node(pending, None),
            node(preparing, None),
            node(restoring, None),
            node(failed, None),
            node(completed, None),
            node(skipped, None),
        ],
        &lease,
    )
    .unwrap();

    for path in [preparing, restoring, failed, completed] {
        reg.transition_recovery_node(
            spec.generation_uid,
            path,
            RecoveryNodeState::Pending,
            RecoveryNodeState::Preparing,
            None,
            &lease,
        )
        .unwrap();
    }
    for path in [restoring, completed] {
        reg.transition_recovery_node(
            spec.generation_uid,
            path,
            RecoveryNodeState::Preparing,
            RecoveryNodeState::Restoring,
            None,
            &lease,
        )
        .unwrap();
    }
    reg.transition_recovery_node(
        spec.generation_uid,
        failed,
        RecoveryNodeState::Preparing,
        RecoveryNodeState::Failed,
        None,
        &lease,
    )
    .unwrap();
    reg.transition_recovery_node(
        spec.generation_uid,
        completed,
        RecoveryNodeState::Restoring,
        RecoveryNodeState::Completed,
        None,
        &lease,
    )
    .unwrap();
    reg.transition_recovery_node(
        spec.generation_uid,
        skipped,
        RecoveryNodeState::Pending,
        RecoveryNodeState::Skipped,
        None,
        &lease,
    )
    .unwrap();

    let head = reg.authority_head().unwrap();
    for (path, expected) in [
        (pending, RecoveryNodeState::Pending),
        (preparing, RecoveryNodeState::Preparing),
        (restoring, RecoveryNodeState::Restoring),
        (failed, RecoveryNodeState::Failed),
    ] {
        let aborted = reg
            .transition_recovery_node(
                spec.generation_uid,
                path,
                expected,
                RecoveryNodeState::Aborted,
                None,
                &lease,
            )
            .unwrap();
        assert_eq!(aborted.node_state, RecoveryNodeState::Aborted);
        assert!(aborted.node_state.is_terminal());

        // Lost acknowledgement is an exact replay, not a second mutation.
        assert_eq!(
            reg.transition_recovery_node(
                spec.generation_uid,
                path,
                expected,
                RecoveryNodeState::Aborted,
                None,
                &lease,
            )
            .unwrap(),
            aborted
        );
        assert!(
            reg.transition_recovery_node(
                spec.generation_uid,
                path,
                RecoveryNodeState::Aborted,
                RecoveryNodeState::Preparing,
                None,
                &lease,
            )
            .is_err(),
            "aborted node {path} must be immutable"
        );
    }
    for (path, terminal) in [
        (completed, RecoveryNodeState::Completed),
        (skipped, RecoveryNodeState::Skipped),
    ] {
        assert!(terminal.is_terminal());
        assert!(
            reg.transition_recovery_node(
                spec.generation_uid,
                path,
                terminal,
                RecoveryNodeState::Aborted,
                None,
                &lease,
            )
            .is_err(),
            "terminal proof row {path} must not be rewritten as aborted"
        );
    }
    assert_eq!(reg.authority_head().unwrap(), head);

    assert!(
        reg.transition_recovery_node(
            spec.generation_uid,
            RECOVERY_GENERATION_PATH,
            RecoveryNodeState::Pending,
            RecoveryNodeState::Aborted,
            None,
            &lease,
        )
        .is_err(),
        "the root must use the atomic generation-abort API"
    );
    assert!(
        reg.unfinished_recovery_for_instance(instance)
            .unwrap()
            .is_some()
    );
}

#[test]
fn atomic_generation_abort_with_floor_is_fenced_preserves_proof_rows_and_unblocks_the_next_generation()
 {
    let (_s, mut reg, instance, epoch, kernel, lease) = recovery_setup();
    let spec = generation(instance, epoch);
    let pending = "nodes/pending";
    let preparing = "nodes/preparing";
    let restoring = "nodes/restoring";
    let failed = "nodes/failed";
    let completed = "nodes/completed";
    let skipped = "nodes/skipped";
    reg.begin_recovery(
        &spec,
        &[
            node(pending, None),
            node(preparing, None),
            node(restoring, None),
            node(failed, None),
            node(completed, None),
            node(skipped, None),
        ],
        &lease,
    )
    .unwrap();

    for path in [preparing, restoring, failed, completed] {
        reg.transition_recovery_node(
            spec.generation_uid,
            path,
            RecoveryNodeState::Pending,
            RecoveryNodeState::Preparing,
            None,
            &lease,
        )
        .unwrap();
    }
    for path in [restoring, completed] {
        reg.transition_recovery_node(
            spec.generation_uid,
            path,
            RecoveryNodeState::Preparing,
            RecoveryNodeState::Restoring,
            None,
            &lease,
        )
        .unwrap();
    }
    reg.transition_recovery_node(
        spec.generation_uid,
        failed,
        RecoveryNodeState::Preparing,
        RecoveryNodeState::Failed,
        None,
        &lease,
    )
    .unwrap();
    reg.transition_recovery_node(
        spec.generation_uid,
        completed,
        RecoveryNodeState::Restoring,
        RecoveryNodeState::Completed,
        None,
        &lease,
    )
    .unwrap();
    reg.transition_recovery_node(
        spec.generation_uid,
        skipped,
        RecoveryNodeState::Pending,
        RecoveryNodeState::Skipped,
        None,
        &lease,
    )
    .unwrap();
    for (from, to) in [
        (RecoveryNodeState::Pending, RecoveryNodeState::Preparing),
        (RecoveryNodeState::Preparing, RecoveryNodeState::Restoring),
        (RecoveryNodeState::Restoring, RecoveryNodeState::Failed),
    ] {
        reg.transition_recovery_node(
            spec.generation_uid,
            RECOVERY_GENERATION_PATH,
            from,
            to,
            None,
            &lease,
        )
        .unwrap();
    }

    let before = reg.recovery_rows(spec.generation_uid).unwrap();
    let head = reg.authority_head().unwrap();
    assert!(
        reg.abort_recovery_generation_and_record_current_empty(
            spec.generation_uid,
            RecoveryNodeState::Restoring,
            epoch,
            &kernel,
            &lease,
        )
        .is_err(),
        "a stale root expectation must lose its compare-and-set"
    );
    assert_eq!(reg.intentional_empty_revision(instance).unwrap(), None);
    assert_eq!(reg.recovery_rows(spec.generation_uid).unwrap(), before);

    reg.release_lease(&LeaseScope::Recovery(instance), lease.holder_request_uid)
        .unwrap();
    assert!(
        reg.abort_recovery_generation_and_record_current_empty(
            spec.generation_uid,
            RecoveryNodeState::Failed,
            epoch,
            &kernel,
            &lease,
        )
        .is_err(),
        "a stale lease must not abort any row"
    );
    assert_eq!(reg.intentional_empty_revision(instance).unwrap(), None);
    assert_eq!(reg.recovery_rows(spec.generation_uid).unwrap(), before);

    let replacement = reg
        .acquire_lease(
            &LeaseScope::Recovery(instance),
            &LeaseHolder::current(Uuid::new_v4()),
            TTL,
            &kernel,
            None,
        )
        .unwrap();
    let (floor, aborted) = reg
        .abort_recovery_generation_and_record_current_empty(
            spec.generation_uid,
            RecoveryNodeState::Failed,
            epoch,
            &kernel,
            &replacement,
        )
        .unwrap();
    assert_eq!(floor, head.revision, "the floor is the head at abort time");
    assert_eq!(
        reg.intentional_empty_revision(instance).unwrap(),
        Some(floor)
    );
    let state = |path: &str| {
        aborted
            .iter()
            .find(|row| row.manifest_node_path == path)
            .unwrap()
            .node_state
    };
    assert_eq!(state(RECOVERY_GENERATION_PATH), RecoveryNodeState::Aborted);
    for path in [pending, preparing, restoring, failed] {
        assert_eq!(state(path), RecoveryNodeState::Aborted);
    }
    assert_eq!(state(completed), RecoveryNodeState::Completed);
    assert_eq!(state(skipped), RecoveryNodeState::Skipped);
    assert!(aborted.iter().all(|row| row.node_state.is_terminal()));
    assert_eq!(reg.authority_head().unwrap(), head);
    assert!(
        reg.unfinished_recovery_for_instance(instance)
            .unwrap()
            .is_none()
    );

    // A lost acknowledgement is exactly replayable with the old expected
    // root, and no journal timestamp or authority revision changes.
    assert_eq!(
        reg.abort_recovery_generation_and_record_current_empty(
            spec.generation_uid,
            RecoveryNodeState::Failed,
            epoch,
            &kernel,
            &replacement,
        )
        .unwrap(),
        (floor, aborted.clone())
    );
    assert_eq!(reg.authority_head().unwrap(), head);

    let next = generation(instance, epoch);
    assert!(matches!(
        reg.begin_recovery(&next, &[], &replacement).unwrap(),
        BeginRecovery::Created(_)
    ));
    assert_eq!(reg.recovery_rows(spec.generation_uid).unwrap(), aborted);
}

#[test]
fn atomic_generation_abort_with_floor_accepts_each_nonterminal_root_state_only() {
    for target in [
        RecoveryNodeState::Pending,
        RecoveryNodeState::Preparing,
        RecoveryNodeState::Restoring,
        RecoveryNodeState::Failed,
    ] {
        let (_s, mut reg, instance, epoch, kernel, lease) = recovery_setup();
        let spec = generation(instance, epoch);
        reg.begin_recovery(&spec, &[], &lease).unwrap();
        if target != RecoveryNodeState::Pending {
            reg.transition_recovery_node(
                spec.generation_uid,
                RECOVERY_GENERATION_PATH,
                RecoveryNodeState::Pending,
                RecoveryNodeState::Preparing,
                None,
                &lease,
            )
            .unwrap();
        }
        if matches!(
            target,
            RecoveryNodeState::Restoring | RecoveryNodeState::Failed
        ) {
            reg.transition_recovery_node(
                spec.generation_uid,
                RECOVERY_GENERATION_PATH,
                RecoveryNodeState::Preparing,
                RecoveryNodeState::Restoring,
                None,
                &lease,
            )
            .unwrap();
        }
        if target == RecoveryNodeState::Failed {
            reg.transition_recovery_node(
                spec.generation_uid,
                RECOVERY_GENERATION_PATH,
                RecoveryNodeState::Restoring,
                RecoveryNodeState::Failed,
                None,
                &lease,
            )
            .unwrap();
        }
        let (_, rows) = reg
            .abort_recovery_generation_and_record_current_empty(
                spec.generation_uid,
                target,
                epoch,
                &kernel,
                &lease,
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].node_state, RecoveryNodeState::Aborted);

        for terminal in [
            RecoveryNodeState::Completed,
            RecoveryNodeState::Skipped,
            RecoveryNodeState::Aborted,
        ] {
            assert!(
                reg.abort_recovery_generation_and_record_current_empty(
                    spec.generation_uid,
                    terminal,
                    epoch,
                    &kernel,
                    &lease,
                )
                .is_err(),
                "terminal expected state {} must not initiate abort",
                terminal.as_str()
            );
        }
    }
}

#[test]
fn intentional_empty_floor_and_generation_abort_commit_or_roll_back_together() {
    let (_s, mut reg, instance, epoch, kernel, lease) = recovery_setup();
    let spec = generation(instance, epoch);
    reg.begin_recovery(&spec, &[node("nodes/a", None)], &lease)
        .unwrap();
    for (from, to) in [
        (RecoveryNodeState::Pending, RecoveryNodeState::Preparing),
        (RecoveryNodeState::Preparing, RecoveryNodeState::Restoring),
        (RecoveryNodeState::Restoring, RecoveryNodeState::Failed),
    ] {
        reg.transition_recovery_node(
            spec.generation_uid,
            RECOVERY_GENERATION_PATH,
            from,
            to,
            None,
            &lease,
        )
        .unwrap();
    }
    let before = reg.recovery_rows(spec.generation_uid).unwrap();
    let head = reg.authority_head().unwrap();
    reg.raw_connection()
        .execute_batch(&format!(
            "CREATE TEMP TRIGGER inject_abort_failure \
             BEFORE UPDATE OF node_state ON recovery_journal \
             WHEN OLD.generation_uid = '{}' \
               AND OLD.manifest_node_path = '{}' \
               AND NEW.node_state = 'aborted' \
             BEGIN SELECT RAISE(ABORT, 'injected abort failure'); END;",
            spec.generation_uid, RECOVERY_GENERATION_PATH
        ))
        .unwrap();

    assert!(
        reg.abort_recovery_generation_and_record_current_empty(
            spec.generation_uid,
            RecoveryNodeState::Failed,
            epoch,
            &kernel,
            &lease,
        )
        .is_err()
    );
    assert_eq!(reg.intentional_empty_revision(instance).unwrap(), None);
    assert_eq!(reg.recovery_rows(spec.generation_uid).unwrap(), before);
    assert_eq!(reg.authority_head().unwrap(), head);

    reg.raw_connection()
        .execute_batch("DROP TRIGGER inject_abort_failure")
        .unwrap();
    let (floor, aborted) = reg
        .abort_recovery_generation_and_record_current_empty(
            spec.generation_uid,
            RecoveryNodeState::Failed,
            epoch,
            &kernel,
            &lease,
        )
        .unwrap();
    assert_eq!(floor, head.revision);
    assert_eq!(
        reg.intentional_empty_revision(instance).unwrap(),
        Some(floor)
    );
    assert!(aborted.iter().all(|row| row.node_state.is_terminal()));
    assert_eq!(reg.authority_head().unwrap(), head);

    // A lost acknowledgement replays both durable facts without moving the
    // authority head or rewriting journal timestamps.
    assert_eq!(
        reg.abort_recovery_generation_and_record_current_empty(
            spec.generation_uid,
            RecoveryNodeState::Failed,
            epoch,
            &kernel,
            &lease,
        )
        .unwrap(),
        (floor, aborted)
    );
    assert_eq!(reg.authority_head().unwrap(), head);
}

#[test]
fn stale_epoch_generation_is_visible_and_can_be_retired_under_the_current_fence() {
    let (_s, mut reg, instance, old_epoch, kernel, old_lease) = recovery_setup();
    let old = generation(instance, old_epoch);
    reg.begin_recovery(&old, &[node("nodes/a", None)], &old_lease)
        .unwrap();
    reg.release_lease(
        &LeaseScope::Recovery(instance),
        old_lease.holder_request_uid,
    )
    .unwrap();

    let new_epoch = ServerEpoch(Uuid::new_v4());
    reg.publish_backend_server(instance, new_epoch, Some(456), Some("restart"), None, None)
        .unwrap();
    let current_lease = reg
        .acquire_lease(
            &LeaseScope::Recovery(instance),
            &LeaseHolder::current(Uuid::new_v4()),
            TTL,
            &kernel,
            None,
        )
        .unwrap();

    let (found, _) = reg
        .unfinished_recovery_for_instance(instance)
        .unwrap()
        .expect("old epoch root remains a durable instance owner");
    assert_eq!(found, old);
    let retired = reg
        .abort_stale_recovery_generation(
            old.generation_uid,
            RecoveryNodeState::Pending,
            new_epoch,
            &current_lease,
        )
        .unwrap();
    assert!(retired.iter().all(|row| row.node_state.is_terminal()));
    assert!(
        reg.unfinished_recovery_for_instance(instance)
            .unwrap()
            .is_none()
    );
    assert_eq!(reg.intentional_empty_revision(instance).unwrap(), None);

    let current = generation(instance, new_epoch);
    assert!(matches!(
        reg.begin_recovery(&current, &[], &current_lease).unwrap(),
        BeginRecovery::Created(_)
    ));
}
