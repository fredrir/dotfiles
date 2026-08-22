//! v2 -> v3 recovery-journal migration: the CHECK constraint gains the
//! durable `aborted` terminal state without losing journal rows, foreign
//! keys, the composite primary key, or authority history.

use std::time::Duration;

use dmux::locks::{self, LockMode, LockScope};
use dmux::model::{BackendInstanceUid, RegistryUid, ServerEpoch};
use dmux::registry::recovery::{RECOVERY_GENERATION_PATH, RecoveryNodeState};
use dmux::registry::{LeaseHolder, LeaseScope, Registry, genesis_head_hash, now_rfc3339, schema};
use rusqlite::{Connection, params};
use uuid::Uuid;

use crate::util::scratch;

struct SeededV2 {
    instance: BackendInstanceUid,
    epoch: ServerEpoch,
    generation: Uuid,
    authority_head: String,
}

fn seed_failed_v2(db_path: &std::path::Path) -> SeededV2 {
    let mut conn = Connection::open(db_path).unwrap();
    schema::apply_connection_settings(&conn, Duration::from_millis(500)).unwrap();
    schema::migrate_to(&mut conn, 2).unwrap();
    assert_eq!(schema::user_version(&conn).unwrap(), 2);

    let host_uid = Uuid::new_v4();
    let registry_uid = RegistryUid(Uuid::new_v4());
    let instance = BackendInstanceUid(Uuid::new_v4());
    let epoch = ServerEpoch(Uuid::new_v4());
    let generation = Uuid::new_v4();
    let authority_head = genesis_head_hash(registry_uid);
    let now = now_rfc3339();
    conn.execute(
        "INSERT INTO meta (id, schema_version, host_uid, registry_uid, authority_revision, \
         authority_head_hash, space_no_counter, created_at) \
         VALUES (1, 2, ?1, ?2, 0, ?3, 1, ?4)",
        params![
            host_uid.to_string(),
            registry_uid.0.to_string(),
            authority_head,
            now
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO hosts (host_uid, lifecycle, enrolled_at) VALUES (?1, 'enrolled', ?2)",
        params![host_uid.to_string(), now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO host_refs (ref_kind, spelling, host_uid, state, created_at, changed_at) \
         VALUES ('alias', 'a', ?1, 'current', ?2, ?2)",
        params![host_uid.to_string(), now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO backend_instances (backend_instance_uid, owner_host_uid, backend, \
         server_epoch, created_at) VALUES (?1, ?2, 'wez', ?3, ?4)",
        params![
            instance.0.to_string(),
            host_uid.to_string(),
            epoch.0.to_string(),
            now
        ],
    )
    .unwrap();
    for (path, state) in [
        (RECOVERY_GENERATION_PATH, "failed"),
        ("spaces/1", "completed"),
    ] {
        conn.execute(
            "INSERT INTO recovery_journal \
             (generation_uid, backend_instance_id, server_epoch, manifest_id, space_uid, \
              manifest_node_path, node_state, bootstrap_request_uid, updated_at) \
             VALUES (?1, ?2, ?3, 'sha256:old-manifest', NULL, ?4, ?5, NULL, ?6)",
            params![
                generation.to_string(),
                instance.0.to_string(),
                epoch.0.to_string(),
                path,
                state,
                now
            ],
        )
        .unwrap();
    }
    drop(conn);

    SeededV2 {
        instance,
        epoch,
        generation,
        authority_head,
    }
}

#[test]
fn failed_v2_generation_migrates_losslessly_and_can_be_atomically_aborted() {
    let s = scratch();
    let seeded = seed_failed_v2(&s.config.db_path);

    let mut reg = Registry::open(s.config.clone()).unwrap();
    assert_eq!(
        schema::user_version(reg.raw_connection()).unwrap(),
        schema::SCHEMA_VERSION
    );
    assert_eq!(
        reg.identity().unwrap().schema_version,
        schema::SCHEMA_VERSION
    );
    let head = reg.authority_head().unwrap();
    assert_eq!(head.revision, 0);
    assert_eq!(head.head_hash, seeded.authority_head);

    let before = reg.recovery_rows(seeded.generation).unwrap();
    assert_eq!(before.len(), 2);
    assert_eq!(before[0].manifest_node_path, RECOVERY_GENERATION_PATH);
    assert_eq!(before[0].node_state, RecoveryNodeState::Failed);
    assert_eq!(before[1].manifest_node_path, "spaces/1");
    assert_eq!(before[1].node_state, RecoveryNodeState::Completed);

    let foreign_targets = {
        let mut stmt = reg
            .raw_connection()
            .prepare("PRAGMA foreign_key_list(recovery_journal)")
            .unwrap();
        let rows = stmt.query_map([], |row| row.get::<_, String>(2)).unwrap();
        let mut targets = rows.collect::<rusqlite::Result<Vec<_>>>().unwrap();
        targets.sort();
        targets
    };
    assert_eq!(
        foreign_targets,
        vec!["backend_instances", "bootstrap_requests", "spaces"]
    );
    let primary_key: Vec<(String, i64)> = {
        let mut stmt = reg
            .raw_connection()
            .prepare("PRAGMA table_info(recovery_journal)")
            .unwrap();
        let rows = stmt
            .query_map([], |row| Ok((row.get(1)?, row.get(5)?)))
            .unwrap();
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .into_iter()
            .filter(|(_, position)| *position > 0)
            .collect()
    };
    assert_eq!(
        primary_key,
        vec![
            ("generation_uid".into(), 1),
            ("manifest_node_path".into(), 2)
        ]
    );

    let kernel = locks::acquire(
        &s.config.lock_dir,
        LockScope::BackendInstance(seeded.instance),
        LockMode::Exclusive,
    )
    .unwrap();
    let lease = reg
        .acquire_lease(
            &LeaseScope::Recovery(seeded.instance),
            &LeaseHolder::current(Uuid::new_v4()),
            Duration::from_secs(30),
            &kernel,
            None,
        )
        .unwrap();
    let (floor, aborted) = reg
        .abort_recovery_generation_and_record_current_empty(
            seeded.generation,
            RecoveryNodeState::Failed,
            seeded.epoch,
            &kernel,
            &lease,
        )
        .unwrap();
    assert_eq!(aborted[0].node_state, RecoveryNodeState::Aborted);
    assert_eq!(aborted[1].node_state, RecoveryNodeState::Completed);
    assert_eq!(floor, head.revision);
    assert!(
        reg.unfinished_recovery_for_instance(seeded.instance)
            .unwrap()
            .is_none()
    );
    assert_eq!(reg.authority_head().unwrap(), head);
}
