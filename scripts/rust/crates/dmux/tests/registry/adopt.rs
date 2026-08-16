//! P6 adoption registry surface (plan §10.3): kind-explicit reservation
//! (`adopt`/`rebind` alongside `create`), adoption finalization that lands
//! active+unstamped, and validated health transitions with the pinned
//! revision decision (health never advances the authority chain).

use dmux::error::ErrorCode;
use dmux::model::{Health, Lifecycle, Observation, OperationKind, OperationState};
use dmux::registry::{BindingState, NativeBindingSpec, NativeKind, RegistryError};
use uuid::Uuid;

use crate::util::{finalize, open, reserve, scratch, tmux_instance};

fn tmux_binding(token: &str) -> NativeBindingSpec {
    NativeBindingSpec {
        native_token: token.to_string(),
        native_kind: NativeKind::TmuxSessionId,
        server_epoch: None,
    }
}

#[test]
fn adopt_reservation_finalizes_active_unstamped_with_current_binding() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);

    let r = reg
        .reserve_space_kind("adopted", instance, Uuid::new_v4(), OperationKind::Adopt)
        .unwrap();

    // The reservation journals the adopt kind, prepared, identity reserved.
    let op = reg.operation(r.operation_uid).unwrap();
    assert_eq!(op.kind, OperationKind::Adopt);
    assert_eq!(op.state, OperationState::Prepared);
    let row = reg.space(r.space_uid).unwrap();
    assert_eq!(row.lifecycle, Lifecycle::Reserved);
    assert_eq!(row.health, Health::Unknown);

    let head_before = reg.authority_head().unwrap();
    reg.finalize_adopt(r.space_uid, r.operation_uid, &tmux_binding("$42"))
        .unwrap();

    // Plan §10.3: after stamping/binding the Space is active + unstamped
    // until every pane acknowledges — never healthy at finalize time.
    let row = reg.space(r.space_uid).unwrap();
    assert_eq!(row.lifecycle, Lifecycle::Active);
    assert_eq!(row.health, Health::Unstamped);

    let binding = reg.current_binding(r.space_uid).unwrap().unwrap();
    assert_eq!(binding.native_token, "$42");
    assert_eq!(binding.native_kind, NativeKind::TmuxSessionId);
    assert_eq!(binding.binding_state, BindingState::Current);
    assert_eq!(binding.observation, Observation::Live);

    let op = reg.operation(r.operation_uid).unwrap();
    assert_eq!(op.state, OperationState::Completed);
    assert!(reg.unfinished_operation(r.space_uid).unwrap().is_none());

    // Finalizing an adoption is a lifecycle+binding mutation: it advances
    // the authority chain (contrast with set_space_health below).
    let head_after = reg.authority_head().unwrap();
    assert_eq!(head_after.revision, head_before.revision + 1);
    assert_ne!(head_after.head_hash, head_before.head_hash);
}

#[test]
fn rebind_reservation_finalizes_like_adopt() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);

    let r = reg
        .reserve_space_kind("rebound", instance, Uuid::new_v4(), OperationKind::Rebind)
        .unwrap();
    assert_eq!(
        reg.operation(r.operation_uid).unwrap().kind,
        OperationKind::Rebind
    );
    reg.finalize_adopt(r.space_uid, r.operation_uid, &tmux_binding("$7"))
        .unwrap();
    let row = reg.space(r.space_uid).unwrap();
    assert_eq!(row.lifecycle, Lifecycle::Active);
    // Plan §10.3: repair rebind also finishes unstamped until all panes
    // acknowledge.
    assert_eq!(row.health, Health::Unstamped);
}

#[test]
fn reserve_space_kind_rejects_existing_space_kinds_typed_with_no_side_effects() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);

    for kind in [
        OperationKind::Rename,
        OperationKind::Remove,
        OperationKind::Normalize,
        OperationKind::Stamp,
    ] {
        let err = reg
            .reserve_space_kind("fresh", instance, Uuid::new_v4(), kind)
            .unwrap_err();
        assert!(
            matches!(&err, RegistryError::KindNotAllowed { kind: k, .. } if *k == kind),
            "{kind}: got {err}"
        );
        assert_eq!(err.error_code(), ErrorCode::Usage);
    }

    // No side effects: nothing reserved, no journal rows, no number burned.
    let spaces: i64 = reg
        .raw_connection()
        .query_row("SELECT count(*) FROM spaces", [], |r| r.get(0))
        .unwrap();
    assert_eq!(spaces, 0);
    let ops: i64 = reg
        .raw_connection()
        .query_row("SELECT count(*) FROM operations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ops, 0);
    let next = reserve(&mut reg, "fresh", instance);
    assert_eq!(next.space_no.get(), 1);
}

#[test]
fn finalize_create_and_finalize_adopt_enforce_their_journal_kinds() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);

    // A create reservation cannot be finalized as an adoption...
    let created = reserve(&mut reg, "created", instance);
    let err = reg
        .finalize_adopt(
            created.space_uid,
            created.operation_uid,
            &tmux_binding("$1"),
        )
        .unwrap_err();
    assert!(matches!(
        err,
        RegistryError::KindNotAllowed {
            kind: OperationKind::Create,
            ..
        }
    ));
    assert_eq!(err.error_code(), ErrorCode::Usage);
    // ...and the rejection left the reservation untouched and completable.
    assert_eq!(
        reg.space(created.space_uid).unwrap().lifecycle,
        Lifecycle::Reserved
    );
    finalize(&mut reg, &created, "$1");
    assert_eq!(
        reg.space(created.space_uid).unwrap().health,
        Health::Healthy
    );

    // ...and an adopt reservation cannot be finalized as a create.
    let adopted = reg
        .reserve_space_kind("adopted", instance, Uuid::new_v4(), OperationKind::Adopt)
        .unwrap();
    let err = reg
        .finalize_create(
            adopted.space_uid,
            adopted.operation_uid,
            &tmux_binding("$2"),
        )
        .unwrap_err();
    assert!(matches!(
        err,
        RegistryError::KindNotAllowed {
            kind: OperationKind::Adopt,
            ..
        }
    ));
    reg.finalize_adopt(
        adopted.space_uid,
        adopted.operation_uid,
        &tmux_binding("$2"),
    )
    .unwrap();
    assert_eq!(
        reg.space(adopted.space_uid).unwrap().health,
        Health::Unstamped
    );
}

#[test]
fn failed_adoption_aborts_and_consumes_its_number() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);

    let r = reg
        .reserve_space_kind("gone", instance, Uuid::new_v4(), OperationKind::Adopt)
        .unwrap();
    reg.abort_create(r.space_uid, r.operation_uid).unwrap();
    let row = reg.space(r.space_uid).unwrap();
    assert_eq!(row.lifecycle, Lifecycle::Aborted);
    assert_eq!(
        reg.operation(r.operation_uid).unwrap().state,
        OperationState::Aborted
    );
    // The gap is intentional; the name frees up, the number does not.
    let next = reserve(&mut reg, "gone", instance);
    assert!(next.space_no > r.space_no);
}

#[test]
fn set_space_health_transitions_and_advances_updated_at() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);

    let r = reserve(&mut reg, "proj", instance);
    finalize(&mut reg, &r, "$1");
    assert_eq!(reg.space(r.space_uid).unwrap().health, Health::Healthy);

    // healthy -> unstamped (e.g. a scan found an unacknowledged pane).
    reg.set_space_health(r.space_uid, Health::Unstamped)
        .unwrap();
    assert_eq!(reg.space(r.space_uid).unwrap().health, Health::Unstamped);

    // unstamped -> healthy (every live pane acknowledged, plan §10.3),
    // and updated_at advances past a backdated value.
    reg.raw_connection()
        .execute(
            "UPDATE spaces SET updated_at = '2000-01-01T00:00:00Z' WHERE space_uid = ?1",
            [r.space_uid.0.to_string()],
        )
        .unwrap();
    reg.set_space_health(r.space_uid, Health::Healthy).unwrap();
    let row = reg.space(r.space_uid).unwrap();
    assert_eq!(row.health, Health::Healthy);
    assert_ne!(row.updated_at, "2000-01-01T00:00:00Z");

    // Every Health variant round-trips through the contract's CHECK set.
    for health in [
        Health::MultiWindow,
        Health::NativeKeyCollision,
        Health::Unstamped,
        Health::Unknown,
        Health::Healthy,
    ] {
        reg.set_space_health(r.space_uid, health).unwrap();
        assert_eq!(reg.space(r.space_uid).unwrap().health, health);
    }

    // Non-terminal includes a still-reserved row.
    let pending = reserve(&mut reg, "pending", instance);
    reg.set_space_health(pending.space_uid, Health::Unstamped)
        .unwrap();
    assert_eq!(
        reg.space(pending.space_uid).unwrap().health,
        Health::Unstamped
    );
}

#[test]
fn set_space_health_rejects_terminal_lifecycles() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);

    // Aborted: immutable history.
    let aborted = reserve(&mut reg, "aborted", instance);
    reg.abort_create(aborted.space_uid, aborted.operation_uid)
        .unwrap();
    let err = reg
        .set_space_health(aborted.space_uid, Health::Healthy)
        .unwrap_err();
    assert!(matches!(err, RegistryError::NotFound { .. }));
    assert_eq!(err.error_code(), ErrorCode::NotFound);
    assert_eq!(
        reg.space(aborted.space_uid).unwrap().health,
        Health::Unknown
    );

    // Deleted tombstone: immutable history.
    let removed = reserve(&mut reg, "removed", instance);
    finalize(&mut reg, &removed, "$1");
    let op = reg.begin_remove(removed.space_uid, Uuid::new_v4()).unwrap();
    reg.complete_remove(removed.space_uid, op).unwrap();
    let before = reg.space(removed.space_uid).unwrap().health;
    let err = reg
        .set_space_health(removed.space_uid, Health::Unstamped)
        .unwrap_err();
    assert!(matches!(err, RegistryError::NotFound { .. }));
    assert_eq!(reg.space(removed.space_uid).unwrap().health, before);
}

#[test]
fn set_space_health_never_advances_the_revision_chain() {
    // Pinned revision decision: health is observation-derived state
    // (pane-stamp acknowledgements/scans), not identity — it does NOT
    // advance the authority chain. The identity-bearing adoption
    // transition is finalize_adopt, which does (asserted above).
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);

    let r = reg
        .reserve_space_kind("adopted", instance, Uuid::new_v4(), OperationKind::Adopt)
        .unwrap();
    reg.finalize_adopt(r.space_uid, r.operation_uid, &tmux_binding("$3"))
        .unwrap();

    let head_before = reg.authority_head().unwrap();
    let chain_before = reg.revision_chain().unwrap().len();
    reg.set_space_health(r.space_uid, Health::Healthy).unwrap();
    reg.set_space_health(r.space_uid, Health::Unstamped)
        .unwrap();
    let head_after = reg.authority_head().unwrap();
    assert_eq!(head_after, head_before);
    assert_eq!(reg.revision_chain().unwrap().len(), chain_before);
}
