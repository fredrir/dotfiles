//! First-run identity: exactly one (HostUid, RegistryUid) even under
//! concurrent first-run; contract connection settings and file modes.

use std::os::unix::fs::MetadataExt;

use dmux::registry::{Registry, genesis_head_hash};
use uuid::Version;

use crate::util::{open, scratch};

#[test]
fn first_run_initializes_identity_counters_and_genesis_exactly_once() {
    let s = scratch();
    let reg = open(&s.config);
    let id = reg.identity().unwrap();
    assert_eq!(id.schema_version, 1);
    assert_eq!(id.host_uid.0.get_version(), Some(Version::Random));
    assert_eq!(id.registry_uid.0.get_version(), Some(Version::Random));
    assert!(!id.created_at.is_empty());

    let head = reg.authority_head().unwrap();
    assert_eq!(head.revision, 0);
    assert_eq!(head.head_hash, genesis_head_hash(id.registry_uid));
    assert!(reg.revision_chain().unwrap().is_empty());

    // Counter starts at 1; self host enrolled; alias `a` seeded current.
    let counter: i64 = reg
        .raw_connection()
        .query_row("SELECT space_no_counter FROM meta", [], |r| r.get(0))
        .unwrap();
    assert_eq!(counter, 1);
    let (host, lifecycle): (String, String) = reg
        .raw_connection()
        .query_row("SELECT host_uid, lifecycle FROM hosts", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(host, id.host_uid.0.to_string());
    assert_eq!(lifecycle, "enrolled");
    let alias_host: String = reg
        .raw_connection()
        .query_row(
            "SELECT host_uid FROM host_refs \
             WHERE ref_kind='alias' AND spelling='a' AND state='current'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(alias_host, id.host_uid.0.to_string());

    // Reopen: the identity is permanent.
    drop(reg);
    let reg = open(&s.config);
    assert_eq!(reg.identity().unwrap(), id);
}

#[test]
fn concurrent_first_run_yields_exactly_one_identity() {
    let s = scratch();
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let config = s.config.clone();
            std::thread::spawn(move || {
                let reg = Registry::open(config).unwrap();
                reg.identity().unwrap()
            })
        })
        .collect();
    let identities: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for id in &identities {
        assert_eq!(id, &identities[0], "every opener must see one identity");
    }

    let reg = open(&s.config);
    let meta_rows: i64 = reg
        .raw_connection()
        .query_row("SELECT count(*) FROM meta", [], |r| r.get(0))
        .unwrap();
    assert_eq!(meta_rows, 1);
    let host_rows: i64 = reg
        .raw_connection()
        .query_row("SELECT count(*) FROM hosts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(host_rows, 1);
}

#[test]
fn contract_connection_settings_and_file_modes() {
    let s = scratch();
    let reg = open(&s.config);
    let conn = reg.raw_connection();
    let journal: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(journal.to_ascii_lowercase(), "wal");
    let fk: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
        .unwrap();
    assert_eq!(fk, 1);
    let sync: i64 = conn
        .query_row("PRAGMA synchronous", [], |r| r.get(0))
        .unwrap();
    assert_eq!(sync, 2, "synchronous=FULL");
    let trusted: i64 = conn
        .query_row("PRAGMA trusted_schema", [], |r| r.get(0))
        .unwrap();
    assert_eq!(trusted, 0);

    // db 0600, lock dir 0700 (created by open with an explicit path).
    let db_mode = std::fs::metadata(&s.config.db_path).unwrap().mode() & 0o777;
    assert_eq!(db_mode, 0o600);
    let lock_mode = std::fs::metadata(&s.config.lock_dir).unwrap().mode() & 0o777;
    assert_eq!(lock_mode, 0o700);
}

#[test]
fn db_parent_directory_is_created_0700() {
    let s = scratch();
    let mut config = s.config.clone();
    config.db_path = s.dir.path().join("data/dmux/registry.sqlite3");
    let reg = Registry::open(config.clone()).unwrap();
    drop(reg);
    let parent_mode = std::fs::metadata(config.db_path.parent().unwrap())
        .unwrap()
        .mode()
        & 0o777;
    assert_eq!(parent_mode, 0o700);
}
