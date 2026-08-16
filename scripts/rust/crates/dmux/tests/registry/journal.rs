//! Operation journal (plan §10.2): one unfinished op per Space, the legal
//! transition matrix, and crash-point reconciliation for every persisted
//! intermediate state of create/rename/remove — with no silent choice when
//! both rename states exist.

use dmux::error::ErrorCode;
use dmux::model::{Lifecycle, OperationKind, OperationState};
use dmux::registry::reconcile::{
    CreateDecision, CreateScan, RenameDecision, ResumeDuty, decide_create, decide_rename,
    resume_duty,
};
use dmux::registry::{NativeBindingSpec, NativeKind, RegistryError};
use uuid::Uuid;

use crate::util::{finalize, open, reserve, scratch, tmux_instance};

#[test]
fn one_unfinished_operation_per_space_is_enforced_by_the_partial_index() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);

    // The create journal row from the reservation is still unfinished.
    let r = reserve(&mut reg, "proj", instance);
    let err = reg
        .begin_operation(
            r.space_uid,
            OperationKind::Stamp,
            Uuid::new_v4(),
            &serde_json::json!({}),
        )
        .unwrap_err();
    assert!(
        matches!(&err, RegistryError::OperationInProgress { space_uid } if *space_uid == r.space_uid)
    );
    assert_eq!(err.error_code(), ErrorCode::OperationInProgress);

    // Rename intent hits the same constraint through its own entry point.
    let err = reg
        .begin_rename(r.space_uid, "newname", Uuid::new_v4())
        .unwrap_err();
    assert!(matches!(err, RegistryError::OperationInProgress { .. }));

    // Finishing the create frees the slot.
    finalize(&mut reg, &r, "$1");
    assert!(reg.unfinished_operation(r.space_uid).unwrap().is_none());
    reg.begin_rename(r.space_uid, "newname", Uuid::new_v4())
        .unwrap();
}

#[test]
fn transition_matrix_is_enforced_in_the_database() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);
    let r = reserve(&mut reg, "proj", instance);

    reg.transition_operation(r.operation_uid, OperationState::Running)
        .unwrap();
    reg.transition_operation(r.operation_uid, OperationState::Unknown)
        .unwrap();
    reg.transition_operation(r.operation_uid, OperationState::Running)
        .unwrap();
    reg.transition_operation(r.operation_uid, OperationState::Failed)
        .unwrap();

    // Terminal states are final.
    let err = reg
        .transition_operation(r.operation_uid, OperationState::Running)
        .unwrap_err();
    assert!(matches!(
        err,
        RegistryError::InvalidTransition {
            from: OperationState::Failed,
            to: OperationState::Running
        }
    ));
    let row = reg.operation(r.operation_uid).unwrap();
    assert_eq!(row.state, OperationState::Failed);
    assert!(row.finished_at.is_some());

    // Unknown operation UIDs are typed not-found.
    let err = reg
        .transition_operation(Uuid::new_v4(), OperationState::Running)
        .unwrap_err();
    assert!(matches!(err, RegistryError::NotFound { .. }));
}

#[test]
fn create_crash_points_reconcile_from_every_persisted_state() {
    for intermediate in [
        OperationState::Prepared,
        OperationState::Running,
        OperationState::Unknown,
    ] {
        let s = scratch();
        let mut reg = open(&s.config);
        let instance = tmux_instance(&mut reg);
        let r = reserve(&mut reg, "proj", instance);
        if intermediate != OperationState::Prepared {
            reg.transition_operation(r.operation_uid, OperationState::Running)
                .unwrap();
            if intermediate == OperationState::Unknown {
                reg.transition_operation(r.operation_uid, OperationState::Unknown)
                    .unwrap();
            }
        }
        drop(reg); // crash

        let mut reg = open(&s.config);
        let row = reg
            .unfinished_operation(r.space_uid)
            .unwrap()
            .expect("crashed create must be discoverable");
        assert_eq!(row.operation_uid, r.operation_uid);
        assert_eq!(row.kind, OperationKind::Create);
        assert_eq!(row.state, intermediate);

        // The resuming holder must run the complete keyed lookup first —
        // a blind respawn is never permitted.
        assert_eq!(
            resume_duty(row.kind, row.state),
            ResumeDuty::CreateKeyedLookup
        );
        assert_eq!(
            decide_create(CreateScan::ZeroMatches),
            CreateDecision::RetryCreate
        );
        assert_eq!(
            decide_create(CreateScan::OneConforming),
            CreateDecision::RebindAndFinalize
        );
        assert_eq!(
            decide_create(CreateScan::Indeterminate),
            CreateDecision::FailClosed
        );
        assert_eq!(
            decide_create(CreateScan::MultipleOrConflicting),
            CreateDecision::FailClosed
        );

        // Simulate the OneConforming outcome: rebind/finalize completes it.
        reg.finalize_create(
            r.space_uid,
            r.operation_uid,
            &NativeBindingSpec {
                native_token: "$9".into(),
                native_kind: NativeKind::TmuxSessionId,
                server_epoch: None,
            },
        )
        .unwrap();
        assert!(reg.unfinished_operation(r.space_uid).unwrap().is_none());
        assert_eq!(reg.space(r.space_uid).unwrap().lifecycle, Lifecycle::Active);
    }
}

#[test]
fn rename_crash_reconciliation_commits_new_only_and_conflicts_on_both() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);
    let r = reserve(&mut reg, "old", instance);
    finalize(&mut reg, &r, "$1");
    let op = reg
        .begin_rename(r.space_uid, "new", Uuid::new_v4())
        .unwrap();
    drop(reg); // crash with rename intent persisted

    let mut reg = open(&s.config);
    let row = reg.unfinished_operation(r.space_uid).unwrap().unwrap();
    assert_eq!(row.operation_uid, op);
    assert_eq!(row.kind, OperationKind::Rename);
    assert_eq!(
        resume_duty(row.kind, row.state),
        ResumeDuty::RenameObserveStates
    );
    // The recorded intent carries exact old/new names.
    let payload: serde_json::Value = serde_json::from_str(&row.payload_json).unwrap();
    assert_eq!(payload["old"], "old");
    assert_eq!(payload["new"], "new");

    // The four observed-state outcomes are explicit; both-exist is never a
    // silent pick.
    assert_eq!(
        decide_rename(true, false),
        RenameDecision::RetryNativeRename
    );
    assert_eq!(
        decide_rename(false, true),
        RenameDecision::CommitRegistryRename
    );
    assert_eq!(decide_rename(true, true), RenameDecision::ConflictBothExist);
    assert_eq!(
        decide_rename(false, false),
        RenameDecision::ConflictNeitherExists
    );

    // New-only: commit the registry side. Identity never changes.
    reg.commit_rename(r.space_uid, op).unwrap();
    let renamed = reg.space(r.space_uid).unwrap();
    assert_eq!(renamed.logical_name, "new");
    assert_eq!(renamed.space_uid, r.space_uid);
    assert_eq!(renamed.space_no, r.space_no);
    let (old_name, new_name): (String, String) = reg
        .raw_connection()
        .query_row(
            "SELECT old_name, new_name FROM space_name_history WHERE space_uid=?1",
            [r.space_uid.0.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((old_name.as_str(), new_name.as_str()), ("old", "new"));
}

#[test]
fn rename_both_exist_becomes_explicit_conflict_state() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);
    let r = reserve(&mut reg, "old", instance);
    finalize(&mut reg, &r, "$1");
    let op = reg
        .begin_rename(r.space_uid, "new", Uuid::new_v4())
        .unwrap();
    drop(reg);

    let mut reg = open(&s.config);
    // Observation: both old and new exist natively → explicit conflict.
    assert_eq!(decide_rename(true, true), RenameDecision::ConflictBothExist);
    reg.transition_operation(op, OperationState::Conflict)
        .unwrap();
    // The journal slot frees (conflict is terminal) but nothing was renamed.
    assert!(reg.unfinished_operation(r.space_uid).unwrap().is_none());
    assert_eq!(reg.space(r.space_uid).unwrap().logical_name, "old");
    assert_eq!(reg.operation(op).unwrap().state, OperationState::Conflict);
}

#[test]
fn rename_to_an_occupied_live_name_is_rejected_up_front() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);
    let a = reserve(&mut reg, "a", instance);
    finalize(&mut reg, &a, "$1");
    let b = reserve(&mut reg, "b", instance);
    finalize(&mut reg, &b, "$2");

    let err = reg
        .begin_rename(a.space_uid, "b", Uuid::new_v4())
        .unwrap_err();
    assert!(matches!(&err, RegistryError::NameConflict { name } if name == "b"));
    // Nothing was journaled.
    assert!(reg.unfinished_operation(a.space_uid).unwrap().is_none());
}

#[test]
fn remove_crash_point_persists_deleting_intent_before_any_kill() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);
    let r = reserve(&mut reg, "proj", instance);
    finalize(&mut reg, &r, "$1");
    let op = reg.begin_remove(r.space_uid, Uuid::new_v4()).unwrap();
    drop(reg); // crash between intent and native removal

    let mut reg = open(&s.config);
    // The deleting intent is durable; absence was never assumed.
    assert_eq!(
        reg.space(r.space_uid).unwrap().lifecycle,
        Lifecycle::Deleting
    );
    let row = reg.unfinished_operation(r.space_uid).unwrap().unwrap();
    assert_eq!(row.kind, OperationKind::Remove);
    assert_eq!(
        resume_duty(row.kind, row.state),
        ResumeDuty::RemoveVerifyAbsence
    );

    // Only verified absence commits deleted.
    reg.complete_remove(r.space_uid, op).unwrap();
    let done = reg.space(r.space_uid).unwrap();
    assert_eq!(done.lifecycle, Lifecycle::Deleted);
    assert!(done.deleted_at.is_some());
}

#[test]
fn adoption_kinds_have_explicit_resume_duties() {
    // Pure decision coverage for the remaining journal kinds (plan §10.3).
    for kind in [
        OperationKind::Adopt,
        OperationKind::Rebind,
        OperationKind::Normalize,
        OperationKind::Stamp,
    ] {
        for state in [
            OperationState::Prepared,
            OperationState::Running,
            OperationState::Unknown,
        ] {
            assert_eq!(resume_duty(kind, state), ResumeDuty::AdoptionReconcile);
        }
        assert_eq!(
            resume_duty(kind, OperationState::Completed),
            ResumeDuty::Nothing
        );
    }
}
