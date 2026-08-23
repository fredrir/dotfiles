//! v4 -> v5 adoption-journal migration (ADR 012 WS-D.2; plan §10.3): the
//! `operations` journal gains `source_native_token` without losing a row, a
//! journal state, the one-unfinished-per-Space index, or authority history.
//! A row journaled before v5 reads back with no source and reconciles
//! exactly as it did; a row journaled after v5 carries the source.

use std::time::Duration;

use dmux::model::{Backend, OperationKind, OperationState, RegistryUid};
use dmux::operations::{ReconcileOutcome, reconcile_apply, reconcile_scan};
use dmux::registry::{Registry, genesis_head_hash, now_rfc3339, schema};
use rusqlite::{Connection, params};
use uuid::Uuid;

use crate::util::scratch;

struct SeededV4 {
    space_uid: Uuid,
    operation_uid: Uuid,
    authority_head: String,
}

/// A v4 registry in the state a crashed `dmux adopt --name other` leaves:
/// one `reserved` Space named `other` and its `adopt/prepared` row, whose
/// payload records only `{name, backend_instance}` — there was nowhere to
/// put the source workspace name.
fn seed_stranded_v4(db_path: &std::path::Path) -> SeededV4 {
    let mut conn = Connection::open(db_path).unwrap();
    schema::apply_connection_settings(&conn, Duration::from_millis(500)).unwrap();
    schema::migrate_to(&mut conn, 4).unwrap();
    assert_eq!(schema::user_version(&conn).unwrap(), 4);
    let columns: Vec<String> = {
        let mut stmt = conn.prepare("PRAGMA table_info(operations)").unwrap();
        stmt.query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    assert!(
        !columns.iter().any(|c| c == "source_native_token"),
        "the fixture is only meaningful without the v5 column: {columns:?}"
    );

    let host_uid = Uuid::new_v4();
    let registry_uid = RegistryUid(Uuid::new_v4());
    let instance = Uuid::new_v4();
    let space_uid = Uuid::now_v7();
    let operation_uid = Uuid::new_v4();
    let authority_head = genesis_head_hash(registry_uid);
    let now = now_rfc3339();
    conn.execute(
        "INSERT INTO meta (id, schema_version, host_uid, registry_uid, authority_revision, \
         authority_head_hash, space_no_counter, created_at) \
         VALUES (1, 4, ?1, ?2, 0, ?3, 2, ?4)",
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
         socket_path, created_at) VALUES (?1, ?2, 'tmux', 'dmux-migrate-v5', ?3)",
        params![instance.to_string(), host_uid.to_string(), now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO spaces (space_uid, owner_host_uid, space_no, backend_instance_id, \
         logical_name, lifecycle, health, created_at, updated_at) \
         VALUES (?1, ?2, 1, ?3, 'other', 'reserved', 'unknown', ?4, ?4)",
        params![
            space_uid.to_string(),
            host_uid.to_string(),
            instance.to_string(),
            now
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO operations (operation_uid, space_uid, kind, operation_state, \
         request_uid, payload_json, started_at, updated_at) \
         VALUES (?1, ?2, 'adopt', 'prepared', ?3, ?4, ?5, ?5)",
        params![
            operation_uid.to_string(),
            space_uid.to_string(),
            Uuid::new_v4().to_string(),
            serde_json::json!({
                "name": "other",
                "backend_instance": instance.to_string(),
            })
            .to_string(),
            now
        ],
    )
    .unwrap();
    drop(conn);
    SeededV4 {
        space_uid,
        operation_uid,
        authority_head,
    }
}

#[test]
fn a_v4_journal_migrates_losslessly_and_pre_v5_rows_carry_no_source() {
    let s = scratch();
    let seeded = seed_stranded_v4(&s.config.db_path);

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
    assert_eq!(head.revision, 0, "a migration is not a mutation");
    assert_eq!(head.head_hash, seeded.authority_head);

    // The row survived byte for byte where it had bytes, and reads back
    // with no source: that is what "journaled before v5" means.
    let row = reg.operation(seeded.operation_uid).unwrap();
    assert_eq!(row.space_uid.0, seeded.space_uid);
    assert_eq!(row.kind, OperationKind::Adopt);
    assert_eq!(row.state, OperationState::Prepared);
    assert_eq!(row.source_native_token, None);
    let payload: serde_json::Value = serde_json::from_str(&row.payload_json).unwrap();
    assert_eq!(payload["name"], "other");
    assert!(row.finished_at.is_none());

    // Every index and the one-unfinished-per-Space rule are intact.
    let indexes: Vec<String> = {
        let mut stmt = reg
            .raw_connection()
            .prepare("PRAGMA index_list(operations)")
            .unwrap();
        let mut names = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        names.sort();
        names
    };
    assert!(
        indexes.iter().any(|i| i == "operations_one_unfinished_uq"),
        "{indexes:?}"
    );
    let unfinished = reg.unfinished_operations().unwrap();
    assert_eq!(unfinished.len(), 1);
    assert_eq!(unfinished[0].operation_uid, seeded.operation_uid);
    let err = reg
        .begin_operation(
            dmux::model::SpaceUid(seeded.space_uid),
            OperationKind::Stamp,
            Uuid::new_v4(),
            &serde_json::json!({}),
        )
        .unwrap_err();
    assert!(
        matches!(
            err,
            dmux::registry::RegistryError::OperationInProgress { .. }
        ),
        "{err}"
    );

    // A row journaled after the migration carries its source; a create
    // still carries none.
    let instance = reg
        .register_backend_instance(Backend::Tmux, Some("dmux-migrate-v5"), None)
        .unwrap();
    let adopted = reg
        .reserve_adoption(
            "legacy",
            instance,
            Uuid::new_v4(),
            OperationKind::Adopt,
            "$7",
        )
        .unwrap();
    assert_eq!(
        reg.operation(adopted.operation_uid)
            .unwrap()
            .source_native_token
            .as_deref(),
        Some("$7")
    );
    let created = reg
        .reserve_space("fresh", instance, Uuid::new_v4())
        .unwrap();
    assert_eq!(
        reg.operation(created.operation_uid)
            .unwrap()
            .source_native_token,
        None
    );
    assert_eq!(created.space_no.get(), 3, "the counter carried over");
}

/// The pre-v5 row reconciles exactly as it did: the tmux adoption's
/// reservation is released and the name freed, the decision made with no
/// source to read. (The Wez half — a reverse CAS aimed at the logical name
/// when no source is journaled — is pinned in `reconcile.rs`.)
#[test]
fn a_pre_v5_stranded_adoption_reconciles_unchanged() {
    let s = scratch();
    let seeded = seed_stranded_v4(&s.config.db_path);
    let env = dmux::operations::OperationEnv {
        db_path: s.config.db_path.clone(),
        lock_dir: s.config.lock_dir.clone(),
    };
    {
        // Reconciliation refuses an instance the registry cannot vouch for,
        // so the migrated instance publishes an epoch first — exactly what
        // a live host's registry holds.
        let mut reg = Registry::open(s.config.clone()).unwrap();
        let instance = reg
            .register_backend_instance(Backend::Tmux, Some("dmux-migrate-v5"), None)
            .unwrap();
        reg.publish_backend_server(
            instance,
            dmux::model::ServerEpoch(Uuid::new_v4()),
            Some(4242),
            Some("start"),
            None,
            None,
        )
        .unwrap();
    }
    let targets = reconcile_scan(&env).unwrap();
    assert_eq!(targets.len(), 1, "{targets:?}");
    assert_eq!(targets[0].operation_uid, seeded.operation_uid);
    assert_eq!(targets[0].duty, "adoption_reconcile");
    let result = reconcile_apply(&env, &targets[0], None);
    assert_eq!(
        result.outcome,
        ReconcileOutcome::ReservationReleased,
        "{result:?}"
    );
    let reg = Registry::open(s.config.clone()).unwrap();
    assert_eq!(
        reg.space(dmux::model::SpaceUid(seeded.space_uid))
            .unwrap()
            .lifecycle,
        dmux::model::Lifecycle::Aborted
    );
    assert_eq!(
        reg.operation(seeded.operation_uid).unwrap().state,
        OperationState::Aborted
    );
}
