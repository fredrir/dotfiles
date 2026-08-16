//! SQLITE_BUSY handling: a held write transaction from another connection
//! yields bounded retries, then typed `registry_busy` with no side effects;
//! WAL keeps reads working meanwhile.

use std::time::Duration;

use dmux::error::ErrorCode;
use dmux::registry::{BusyPolicy, Registry, RegistryError};
use uuid::Uuid;

use crate::util::{open, scratch, tmux_instance};

#[test]
fn bounded_retry_then_typed_registry_busy_with_no_side_effects() {
    let s = scratch();
    let mut config = s.config.clone();
    config.busy = BusyPolicy {
        busy_timeout: Duration::from_millis(20),
        attempts: 3,
        retry_base: Duration::from_millis(2),
    };
    // Initialize and register the instance before the blocker appears.
    let mut reg = Registry::open(config.clone()).unwrap();
    let instance = tmux_instance(&mut reg);
    let head_before = reg.authority_head().unwrap();

    // Another connection holds a write transaction open.
    let blocker = rusqlite::Connection::open(&config.db_path).unwrap();
    blocker.busy_timeout(Duration::from_millis(10)).unwrap();
    blocker
        .execute_batch(
            "BEGIN IMMEDIATE; \
             INSERT INTO lease_scopes (scope, last_fencing_token) VALUES ('blocker', 0);",
        )
        .unwrap();

    let started = std::time::Instant::now();
    let err = reg
        .reserve_space("held", instance, Uuid::new_v4())
        .unwrap_err();
    let elapsed = started.elapsed();
    assert!(matches!(err, RegistryError::Busy));
    assert_eq!(err.error_code(), ErrorCode::RegistryBusy);
    // Bounded: it waited through the retries, then stopped. No unbounded
    // spinning (generous ceiling to keep the assertion robust under load).
    assert!(elapsed >= Duration::from_millis(20), "no retry happened");
    assert!(elapsed < Duration::from_secs(5), "retry was not bounded");

    blocker.execute_batch("ROLLBACK").unwrap();

    // No side effects: no space rows, counter untouched, chain untouched.
    assert!(reg.spaces().unwrap().is_empty());
    let counter: i64 = reg
        .raw_connection()
        .query_row("SELECT space_no_counter FROM meta", [], |r| r.get(0))
        .unwrap();
    assert_eq!(counter, 1);
    assert_eq!(reg.authority_head().unwrap(), head_before);
    let ops: i64 = reg
        .raw_connection()
        .query_row("SELECT count(*) FROM operations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ops, 0);

    // After the blocker is gone the same call succeeds.
    let r = reg.reserve_space("held", instance, Uuid::new_v4()).unwrap();
    assert_eq!(r.space_no.get(), 1);
}

#[test]
fn reads_proceed_under_wal_while_a_writer_holds_the_database() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);
    let r = reg.reserve_space("proj", instance, Uuid::new_v4()).unwrap();

    let blocker = rusqlite::Connection::open(&s.config.db_path).unwrap();
    blocker
        .execute_batch(
            "BEGIN IMMEDIATE; \
             INSERT INTO lease_scopes (scope, last_fencing_token) VALUES ('blocker', 0);",
        )
        .unwrap();

    // Reads are unaffected by the write lock in WAL mode.
    assert_eq!(reg.identity().unwrap().schema_version, 1);
    assert_eq!(reg.spaces().unwrap().len(), 1);
    assert_eq!(reg.space(r.space_uid).unwrap().logical_name, "proj");

    blocker.execute_batch("ROLLBACK").unwrap();
}
