//! Normative kernel-lock ordering (plan §10.1): authority gate → decision
//! locks in exact-byte lexical order → backend-instance locks by
//! BackendInstanceUid → Space lock; release in reverse; no decision lock
//! after backend/Space. Out-of-order acquisition panics in debug builds.

use std::panic::AssertUnwindSafe;

use dmux::locks::{LockMode, LockScope, OrderedLocks};
use dmux::model::{BackendInstanceUid, HostUid, SpaceUid};
use uuid::Uuid;

fn dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn owner() -> HostUid {
    HostUid(Uuid::new_v4())
}

fn expect_order_panic(f: impl FnOnce()) {
    let err = std::panic::catch_unwind(AssertUnwindSafe(f))
        .expect_err("out-of-order acquisition must be rejected");
    let message = err
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default();
    assert!(
        message.contains("lock order violation"),
        "unexpected panic: {message}"
    );
}

#[test]
fn full_order_round_trip_acquires_and_releases_in_reverse() {
    let dir = dir();
    let owner = owner();
    let backend = BackendInstanceUid(Uuid::new_v4());
    let space = SpaceUid(Uuid::now_v7());

    let mut locks = OrderedLocks::new(dir.path());
    locks
        .acquire(LockScope::AuthorityGate, LockMode::Shared)
        .unwrap();
    // Argument order is deliberately unsorted; the builder acquires in
    // exact-byte lexical key order.
    locks
        .acquire_decisions(owner, &["zeta", "alpha"], LockMode::Exclusive)
        .unwrap();
    locks
        .acquire(LockScope::BackendInstance(backend), LockMode::Exclusive)
        .unwrap();
    locks
        .acquire(LockScope::Space(space), LockMode::Exclusive)
        .unwrap();

    let scopes = locks.held_scopes();
    assert_eq!(scopes.len(), 5);
    // Ranks are nondecreasing and same-rank keys strictly increase.
    for pair in scopes.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        assert!(
            b.rank() > a.rank() || (b.rank() == a.rank() && b.key() > a.key()),
            "{} then {}",
            a.key(),
            b.key()
        );
    }

    // Reverse release: last acquired comes back first.
    assert_eq!(locks.release_last(), Some(LockScope::Space(space)));
    assert_eq!(
        locks.release_last(),
        Some(LockScope::BackendInstance(backend))
    );
    locks.release_all();
    assert!(locks.held_scopes().is_empty());

    // Everything is reacquirable after release.
    locks
        .acquire(LockScope::AuthorityGate, LockMode::Exclusive)
        .unwrap();
    locks.release_all();
}

#[test]
fn first_acquisition_must_be_the_authority_gate() {
    let dir = dir();
    expect_order_panic(|| {
        let mut locks = OrderedLocks::new(dir.path());
        let _ = locks.acquire(
            LockScope::BackendInstance(BackendInstanceUid(Uuid::new_v4())),
            LockMode::Exclusive,
        );
    });
}

#[test]
fn no_decision_lock_after_backend_or_space() {
    let dir = dir();
    let owner = owner();
    let backend = BackendInstanceUid(Uuid::new_v4());
    expect_order_panic(|| {
        let mut locks = OrderedLocks::new(dir.path());
        locks
            .acquire(LockScope::AuthorityGate, LockMode::Shared)
            .unwrap();
        locks
            .acquire(LockScope::BackendInstance(backend), LockMode::Exclusive)
            .unwrap();
        let _ = locks.acquire(LockScope::decision(owner, "late"), LockMode::Exclusive);
    });
    expect_order_panic(|| {
        let mut locks = OrderedLocks::new(dir.path());
        locks
            .acquire(LockScope::AuthorityGate, LockMode::Shared)
            .unwrap();
        locks
            .acquire(
                LockScope::Space(SpaceUid(Uuid::now_v7())),
                LockMode::Exclusive,
            )
            .unwrap();
        let _ = locks.acquire(LockScope::decision(owner, "late"), LockMode::Exclusive);
    });
}

#[test]
fn same_rank_requires_strict_lexical_order_and_no_duplicates() {
    let dir = dir();
    let owner = owner();
    // Find the lexical order of two decision scopes by key.
    let (first, second) = {
        let a = LockScope::decision(owner, "one");
        let b = LockScope::decision(owner, "two");
        if a.key() < b.key() { (a, b) } else { (b, a) }
    };

    // Correct order works.
    let mut locks = OrderedLocks::new(dir.path());
    locks
        .acquire(LockScope::AuthorityGate, LockMode::Shared)
        .unwrap();
    locks.acquire(first.clone(), LockMode::Exclusive).unwrap();
    locks.acquire(second.clone(), LockMode::Exclusive).unwrap();
    drop(locks);

    // Reversed order is rejected.
    expect_order_panic(|| {
        let mut locks = OrderedLocks::new(dir.path());
        locks
            .acquire(LockScope::AuthorityGate, LockMode::Shared)
            .unwrap();
        locks.acquire(second.clone(), LockMode::Exclusive).unwrap();
        let _ = locks.acquire(first.clone(), LockMode::Exclusive);
    });

    // Duplicates are rejected (equal key is not strictly greater).
    expect_order_panic(|| {
        let mut locks = OrderedLocks::new(dir.path());
        locks
            .acquire(LockScope::AuthorityGate, LockMode::Shared)
            .unwrap();
        locks
            .acquire(LockScope::decision(owner, "dup"), LockMode::Exclusive)
            .unwrap();
        let _ = locks.acquire(LockScope::decision(owner, "dup"), LockMode::Exclusive);
    });
}

#[test]
fn backend_instances_acquire_in_uid_order() {
    let dir = dir();
    let low = BackendInstanceUid(Uuid::from_u128(1));
    let high = BackendInstanceUid(Uuid::from_u128(u128::MAX - 1));

    let mut locks = OrderedLocks::new(dir.path());
    locks
        .acquire(LockScope::AuthorityGate, LockMode::Shared)
        .unwrap();
    locks
        .acquire(LockScope::BackendInstance(low), LockMode::Shared)
        .unwrap();
    locks
        .acquire(LockScope::BackendInstance(high), LockMode::Shared)
        .unwrap();
    drop(locks);

    expect_order_panic(|| {
        let mut locks = OrderedLocks::new(dir.path());
        locks
            .acquire(LockScope::AuthorityGate, LockMode::Shared)
            .unwrap();
        locks
            .acquire(LockScope::BackendInstance(high), LockMode::Shared)
            .unwrap();
        let _ = locks.acquire(LockScope::BackendInstance(low), LockMode::Shared);
    });
}

#[test]
fn shared_gate_holders_coexist_across_builders() {
    let dir = dir();
    let mut a = OrderedLocks::new(dir.path());
    let mut b = OrderedLocks::new(dir.path());
    a.acquire(LockScope::AuthorityGate, LockMode::Shared)
        .unwrap();
    // A second shared holder proceeds without blocking.
    assert!(
        b.try_acquire(LockScope::AuthorityGate, LockMode::Shared)
            .unwrap()
    );
    // Maintenance (exclusive gate) overlaps nothing.
    let mut m = OrderedLocks::new(dir.path());
    assert!(
        !m.try_acquire(LockScope::AuthorityGate, LockMode::Exclusive)
            .unwrap()
    );
    drop(a);
    drop(b);
    assert!(
        m.try_acquire(LockScope::AuthorityGate, LockMode::Exclusive)
            .unwrap()
    );
}

#[test]
fn contended_decision_pairs_in_enforced_order_do_not_deadlock() {
    let dir = dir();
    let owner = owner();
    let path = dir.path().to_path_buf();

    // Two threads take the SAME two decision locks, passing the names in
    // opposite argument orders. The builder's enforced exact-byte order
    // makes both acquire in the same sequence, so the classic ABBA deadlock
    // cannot form; the test completing is the assertion.
    let spawn = |names: [&'static str; 2]| {
        let path = path.clone();
        std::thread::spawn(move || {
            for _ in 0..25 {
                let mut locks = OrderedLocks::new(&path);
                locks
                    .acquire(LockScope::AuthorityGate, LockMode::Shared)
                    .unwrap();
                locks
                    .acquire_decisions(owner, &names, LockMode::Exclusive)
                    .unwrap();
                locks.release_all();
            }
        })
    };
    let t1 = spawn(["projA", "projB"]);
    let t2 = spawn(["projB", "projA"]);
    t1.join().unwrap();
    t2.join().unwrap();
}
