//! Identity allocation: uniqueness + monotonicity under concurrency,
//! intentional aborted gaps, tombstone non-reuse, typed name conflicts.

use std::collections::BTreeSet;

use dmux::error::ErrorCode;
use dmux::model::{Backend, Lifecycle};
use dmux::registry::{BindingState, Registry, RegistryError};
use uuid::Uuid;

use crate::util::{finalize, open, reserve, scratch, tmux_instance};

#[test]
fn concurrent_reserves_allocate_unique_monotonic_numbers_and_uids() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);
    drop(reg);

    const THREADS: u64 = 6;
    const PER_THREAD: u64 = 5;
    let handles: Vec<_> = (0..THREADS)
        .map(|t| {
            let config = s.config.clone();
            std::thread::spawn(move || {
                let mut reg = Registry::open(config).unwrap();
                let mut mine = Vec::new();
                for i in 0..PER_THREAD {
                    let r = reg
                        .reserve_space(&format!("t{t}-{i}"), instance, Uuid::new_v4())
                        .unwrap();
                    mine.push(r);
                }
                mine
            })
        })
        .collect();

    let mut all = Vec::new();
    for handle in handles {
        let mine = handle.join().unwrap();
        // Per-thread monotonicity: later reserves get larger numbers.
        for pair in mine.windows(2) {
            assert!(pair[1].space_no > pair[0].space_no);
        }
        all.extend(mine);
    }

    let total = (THREADS * PER_THREAD) as usize;
    let nos: BTreeSet<u64> = all.iter().map(|r| r.space_no.get()).collect();
    let uids: BTreeSet<Uuid> = all.iter().map(|r| r.space_uid.0).collect();
    let ops: BTreeSet<Uuid> = all.iter().map(|r| r.operation_uid).collect();
    assert_eq!(nos.len(), total, "duplicate SpaceNo allocated");
    assert_eq!(uids.len(), total, "duplicate SpaceUid allocated");
    assert_eq!(ops.len(), total);
    // No gaps unless aborted: exactly 1..=total.
    assert_eq!(nos, (1..=THREADS * PER_THREAD).collect::<BTreeSet<_>>());

    // Every reservation left a journal row.
    let reg = open(&s.config);
    let journal_rows: i64 = reg
        .raw_connection()
        .query_row(
            "SELECT count(*) FROM operations WHERE kind='create'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(journal_rows as usize, total);
}

#[test]
fn aborted_reservation_consumes_its_number_and_frees_the_name() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);

    let first = reserve(&mut reg, "proj", instance);
    assert_eq!(first.space_no.get(), 1);
    reg.abort_create(first.space_uid, first.operation_uid)
        .unwrap();

    // The aborted row is intact and terminal; its number/uid are consumed.
    let aborted = reg.space(first.space_uid).unwrap();
    assert_eq!(aborted.lifecycle, Lifecycle::Aborted);
    assert_eq!(aborted.space_no, first.space_no);

    // The name is free again (aborted rows do not occupy names)...
    let second = reserve(&mut reg, "proj", instance);
    // ...but the gap is preserved: numbers/UIDs are never reused.
    assert_eq!(second.space_no.get(), 2);
    assert_ne!(second.space_uid, first.space_uid);

    let third = reserve(&mut reg, "other", instance);
    assert_eq!(third.space_no.get(), 3);
}

#[test]
fn tombstone_non_reuse_remove_then_recreate_same_name() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);

    let original = reserve(&mut reg, "proj", instance);
    finalize(&mut reg, &original, "$1");
    let op = reg
        .begin_remove(original.space_uid, Uuid::new_v4())
        .unwrap();
    // The caller verified native absence; only then does deleted commit.
    reg.complete_remove(original.space_uid, op).unwrap();

    let tombstone = reg.space(original.space_uid).unwrap();
    assert_eq!(tombstone.lifecycle, Lifecycle::Deleted);
    assert!(tombstone.deleted_at.is_some());
    assert_eq!(tombstone.logical_name, "proj");
    // Its binding is severed, not deleted.
    assert!(reg.current_binding(original.space_uid).unwrap().is_none());
    let severed: String = reg
        .raw_connection()
        .query_row(
            "SELECT binding_state FROM native_bindings WHERE space_uid=?1",
            [original.space_uid.0.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(severed, BindingState::Severed.as_str());

    // Recreate the same name: new UID, larger SpaceNo, old row untouched.
    let recreated = reserve(&mut reg, "proj", instance);
    assert_ne!(recreated.space_uid, original.space_uid);
    assert!(recreated.space_no > original.space_no);
    finalize(&mut reg, &recreated, "$2");

    let old = reg.space(original.space_uid).unwrap();
    assert_eq!(old.lifecycle, Lifecycle::Deleted);
    assert_eq!(old.space_no, original.space_no);

    // Rows are never deleted: both exist.
    let rows: i64 = reg
        .raw_connection()
        .query_row(
            "SELECT count(*) FROM spaces WHERE logical_name='proj'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 2);
}

#[test]
fn live_name_conflict_is_typed_and_leaves_no_side_effects() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);

    let first = reserve(&mut reg, "dup", instance);
    // A reserved row already occupies the name.
    let err = reg
        .reserve_space("dup", instance, Uuid::new_v4())
        .unwrap_err();
    assert!(matches!(&err, RegistryError::NameConflict { name } if name == "dup"));
    assert_eq!(err.error_code(), ErrorCode::NameConflict);

    finalize(&mut reg, &first, "$1");
    let err = reg
        .reserve_space("dup", instance, Uuid::new_v4())
        .unwrap_err();
    assert!(matches!(err, RegistryError::NameConflict { .. }));

    // The failed attempts consumed no visible identity: next number is 2.
    let next = reserve(&mut reg, "dup2", instance);
    assert_eq!(next.space_no.get(), 2);
}

#[test]
fn cross_backend_duplicate_names_are_allowed_at_the_registry_layer() {
    // Plan §10.1: unique active logical name WITHIN one backend instance;
    // cross-backend duplicates exist and are ambiguous without a filter.
    let s = scratch();
    let mut reg = open(&s.config);
    let tmux = tmux_instance(&mut reg);
    let wez = reg
        .register_backend_instance(Backend::Wez, Some("/tmp/sock"), None)
        .unwrap();
    assert_ne!(tmux, wez);

    let a = reserve(&mut reg, "same", tmux);
    let b = reserve(&mut reg, "same", wez);
    assert_ne!(a.space_uid, b.space_uid);
    assert_eq!(
        reg.live_space_by_name(tmux, "same")
            .unwrap()
            .unwrap()
            .space_uid,
        a.space_uid
    );
    assert_eq!(
        reg.live_space_by_name(wez, "same")
            .unwrap()
            .unwrap()
            .space_uid,
        b.space_uid
    );

    // Instance registration is get-or-create: same instance comes back.
    assert_eq!(
        reg.register_backend_instance(Backend::Tmux, None, None)
            .unwrap(),
        tmux
    );
}
