//! v1 -> v2 migration losslessness (ADR 009 §3): a populated v1 database
//! opened by v2 code migrates in place, keeps every row, keeps a
//! recomputable authority chain, and gains the v2 tables.

use std::time::Duration;

use dmux::model::HostUid;
use dmux::registry::sha256::sha256_hex;
use dmux::registry::{
    AttachTokenSpec, Registry, chain_head_hash, genesis_head_hash, now_rfc3339, rfc3339_utc, schema,
};
use rusqlite::{Connection, params};
use uuid::Uuid;

use crate::util::scratch;

struct SeededV1 {
    host_uid: Uuid,
    registry_uid: Uuid,
    peer_uid: Uuid,
    space1: Uuid,
    space2: Uuid,
    head2: String,
}

/// Build the database exactly as the v1 binary left it: schema at
/// user_version 1, populated with identity, a peer with alias/label, a
/// backend instance, live+deleted spaces, a binding, a completed journal
/// row, a route, a cached peer snapshot, and a valid two-link hash chain.
fn seed_v1(db_path: &std::path::Path) -> SeededV1 {
    let mut conn = Connection::open(db_path).unwrap();
    schema::apply_connection_settings(&conn, Duration::from_millis(500)).unwrap();
    schema::migrate_to(&mut conn, 1).unwrap();
    assert_eq!(schema::user_version(&conn).unwrap(), 1);

    let host_uid = Uuid::new_v4();
    let registry_uid = Uuid::new_v4();
    let peer_uid = Uuid::new_v4();
    let instance = Uuid::new_v4();
    let space1 = Uuid::now_v7();
    let space2 = Uuid::now_v7();
    let now = now_rfc3339();

    let genesis = genesis_head_hash(dmux::model::RegistryUid(registry_uid));
    let txn1 = Uuid::new_v4();
    let txn2 = Uuid::new_v4();
    let head1 = chain_head_hash(&genesis, 1, &txn1);
    let head2 = chain_head_hash(&head1, 2, &txn2);

    conn.execute(
        "INSERT INTO meta (id, schema_version, host_uid, registry_uid, authority_revision, \
         authority_head_hash, space_no_counter, created_at) VALUES (1, 1, ?1, ?2, 2, ?3, 3, ?4)",
        params![host_uid.to_string(), registry_uid.to_string(), head2, now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO authority_revisions (revision, parent_head_hash, head_hash, txn_uid, \
         committed_at) VALUES (1, ?1, ?2, ?3, ?4)",
        params![genesis, head1, txn1.to_string(), now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO authority_revisions (revision, parent_head_hash, head_hash, txn_uid, \
         committed_at) VALUES (2, ?1, ?2, ?3, ?4)",
        params![head1, head2, txn2.to_string(), now],
    )
    .unwrap();
    for (uid, alias) in [(host_uid, "a"), (peer_uid, "b")] {
        conn.execute(
            "INSERT INTO hosts (host_uid, lifecycle, enrolled_at) VALUES (?1, 'enrolled', ?2)",
            params![uid.to_string(), now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO host_refs (ref_kind, spelling, host_uid, state, created_at, changed_at) \
             VALUES ('alias', ?1, ?2, 'current', ?3, ?3)",
            params![alias, uid.to_string(), now],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO host_refs (ref_kind, spelling, host_uid, state, created_at, changed_at) \
         VALUES ('label', 'archie', ?1, 'current', ?2, ?2)",
        params![peer_uid.to_string(), now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO backend_instances (backend_instance_uid, owner_host_uid, backend, \
         socket_path, service_label, created_at) VALUES (?1, ?2, 'tmux', 'dmux', NULL, ?3)",
        params![instance.to_string(), host_uid.to_string(), now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO spaces (space_uid, owner_host_uid, space_no, backend_instance_id, \
         logical_name, lifecycle, health, created_at, updated_at) \
         VALUES (?1, ?2, 1, ?3, 'proj', 'active', 'healthy', ?4, ?4)",
        params![
            space1.to_string(),
            host_uid.to_string(),
            instance.to_string(),
            now
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO spaces (space_uid, owner_host_uid, space_no, backend_instance_id, \
         logical_name, lifecycle, health, created_at, updated_at, deleted_at) \
         VALUES (?1, ?2, 2, ?3, 'old', 'deleted', 'unknown', ?4, ?4, ?4)",
        params![
            space2.to_string(),
            host_uid.to_string(),
            instance.to_string(),
            now
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO native_bindings (space_uid, backend_instance_id, native_token, \
         native_kind, binding_state, observation, bound_at) \
         VALUES (?1, ?2, '$1', 'tmux_session_id', 'current', 'live', ?3)",
        params![space1.to_string(), instance.to_string(), now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO operations (operation_uid, space_uid, kind, operation_state, request_uid, \
         payload_json, started_at, updated_at, finished_at) \
         VALUES (?1, ?2, 'create', 'completed', ?3, '{}', ?4, ?4, ?4)",
        params![
            Uuid::new_v4().to_string(),
            space1.to_string(),
            Uuid::new_v4().to_string(),
            now
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO routes (host_uid, transport, endpoint, username, network_class, priority, \
         enabled) VALUES (?1, 'openssh', 'archie', 'fredrir', 'usb', 10, 1)",
        [peer_uid.to_string()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO remote_cache (host_uid, registry_uid, authority_revision, \
         authority_head_hash, snapshot_json, fetched_at) \
         VALUES (?1, ?2, 5, 'sha256:peerhead', '{\"spaces\":[]}', ?3)",
        params![peer_uid.to_string(), Uuid::new_v4().to_string(), now],
    )
    .unwrap();

    SeededV1 {
        host_uid,
        registry_uid,
        peer_uid,
        space1,
        space2,
        head2,
    }
}

#[test]
fn a_populated_v1_database_migrates_in_place_losslessly() {
    let s = scratch();
    let seeded = seed_v1(&s.config.db_path);

    // Open with the v2 code: migrates under the maintenance gate.
    let mut reg = Registry::open(s.config.clone()).unwrap();

    // Schema bookkeeping advanced and mirrored.
    assert_eq!(
        schema::user_version(reg.raw_connection()).unwrap(),
        schema::SCHEMA_VERSION
    );
    let id = reg.identity().unwrap();
    assert_eq!(id.schema_version, schema::SCHEMA_VERSION);
    // Identity survived byte-for-byte.
    assert_eq!(id.host_uid.0, seeded.host_uid);
    assert_eq!(id.registry_uid.0, seeded.registry_uid);

    // The authority chain is intact, recomputable, and unmoved.
    let head = reg.authority_head().unwrap();
    assert_eq!(head.revision, 2);
    assert_eq!(head.head_hash, seeded.head2);
    let chain = reg.revision_chain().unwrap();
    assert_eq!(chain.len(), 2);
    let mut parent = genesis_head_hash(id.registry_uid);
    for record in &chain {
        assert_eq!(record.parent_head_hash, parent);
        assert_eq!(
            record.head_hash,
            chain_head_hash(&parent, record.revision, &record.txn_uid)
        );
        parent = record.head_hash.clone();
    }

    // Spaces, bindings, journal: intact through the typed API.
    let spaces = reg.spaces().unwrap();
    assert_eq!(spaces.len(), 2);
    assert_eq!(spaces[0].space_uid.0, seeded.space1);
    assert_eq!(spaces[0].logical_name, "proj");
    assert_eq!(spaces[1].space_uid.0, seeded.space2);
    let binding = reg
        .current_binding(dmux::model::SpaceUid(seeded.space1))
        .unwrap()
        .unwrap();
    assert_eq!(binding.native_token, "$1");

    // Hosts, refs, routes, cache: intact through the new typed APIs.
    let hosts = reg.hosts().unwrap();
    assert_eq!(hosts.len(), 2);
    let peer = reg.host_by_alias("b").unwrap().unwrap();
    assert_eq!(peer.host_uid.0, seeded.peer_uid);
    assert_eq!(peer.label.as_deref(), Some("archie"));
    let routes = reg.routes_for(HostUid(seeded.peer_uid)).unwrap();
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].endpoint, "archie");
    assert!(routes[0].enabled);
    let cache = reg.peer_cache(HostUid(seeded.peer_uid)).unwrap().unwrap();
    assert_eq!(cache.authority_revision, 5);
    assert_eq!(cache.snapshot_json, serde_json::json!({ "spaces": [] }));

    // The v2 surface is live on the migrated database.
    let token = "attach-token";
    reg.issue_attach_token(&AttachTokenSpec {
        token_hash: sha256_hex(token.as_bytes()),
        request_uid: Uuid::new_v4(),
        host_uid: id.host_uid,
        space_uid: dmux::model::SpaceUid(seeded.space1),
        server_epoch: dmux::model::ServerEpoch(Uuid::new_v4()),
        route: "archie-usb".into(),
        attach_argv: vec!["tmux".into(), "attach".into()],
        issued_at: now_rfc3339(),
        expires_at: rfc3339_utc(std::time::SystemTime::now() + Duration::from_secs(60)),
    })
    .unwrap();

    // Reopening is idempotent: no further migration work, data unchanged.
    drop(reg);
    let reg = Registry::open(s.config.clone()).unwrap();
    assert_eq!(reg.spaces().unwrap().len(), 2);
    assert_eq!(reg.authority_head().unwrap().head_hash, seeded.head2);
}
