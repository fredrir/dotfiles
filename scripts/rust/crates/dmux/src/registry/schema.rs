//! Versioned SQLite migrations implementing the frozen storage contract
//! `docs/adr/dmux/registry-v1.sql` (plan §10.1) plus the v2 extension frozen
//! in `docs/adr/dmux/009-w5-dispatch.md` §3 (attach tokens, pane stamps).
//!
//! The v1 DDL below is the contract file transcribed verbatim — identical
//! index names, identical semantics, including every `-- REQUIRED` partial
//! index. Migration bookkeeping uses `PRAGMA user_version`;
//! `meta.schema_version` mirrors it once the meta row exists. Migrations run
//! under the exclusive maintenance gate (`registry::Registry::open` acquires
//! it) and are lossless: a v1 database opened by v2 code migrates in place
//! with every row intact.

use std::time::Duration;

use rusqlite::Connection;

/// Current schema version. Each entry in [`MIGRATIONS`] moves
/// `user_version` from `n-1` to `n`.
pub const SCHEMA_VERSION: i64 = 2;

const MIGRATIONS: &[(i64, &str)] = &[(1, V1_DDL), (2, V2_DDL)];

/// registry-v1.sql, verbatim semantics (contract: equivalent index names
/// allowed, weaker semantics not — the names are kept identical anyway).
const V1_DDL: &str = r#"
-- One row: this installation's authority identity and counters.
CREATE TABLE meta (
  id                  INTEGER PRIMARY KEY CHECK (id = 1),
  schema_version      INTEGER NOT NULL,
  host_uid            TEXT    NOT NULL,
  registry_uid        TEXT    NOT NULL,
  authority_revision  INTEGER NOT NULL,
  authority_head_hash TEXT    NOT NULL,
  space_no_counter    INTEGER NOT NULL CHECK (space_no_counter >= 1),
  created_at          TEXT    NOT NULL
);

-- Append-only hash chain: every committed authority mutation advances one row.
CREATE TABLE authority_revisions (
  revision         INTEGER PRIMARY KEY,
  parent_head_hash TEXT    NOT NULL,
  head_hash        TEXT    NOT NULL UNIQUE,
  txn_uid          TEXT    NOT NULL UNIQUE,
  committed_at     TEXT    NOT NULL
);

-- Enrolled authorities (self + peers).
CREATE TABLE hosts (
  host_uid      TEXT PRIMARY KEY,
  lifecycle     TEXT NOT NULL CHECK (lifecycle IN ('enrolled', 'tombstoned')),
  enrolled_at   TEXT NOT NULL,
  tombstoned_at TEXT
);

-- Compact aliases and labels; a spelling is never rebound to a different
-- HostUid (rows only transition current -> historical/tombstoned).
CREATE TABLE host_refs (
  ref_kind   TEXT NOT NULL CHECK (ref_kind IN ('alias', 'label')),
  spelling   TEXT NOT NULL,
  host_uid   TEXT NOT NULL REFERENCES hosts(host_uid),
  state      TEXT NOT NULL CHECK (state IN ('current', 'historical', 'tombstoned')),
  created_at TEXT NOT NULL,
  changed_at TEXT NOT NULL,
  PRIMARY KEY (ref_kind, spelling)
);
CREATE UNIQUE INDEX host_refs_one_current_alias_uq
  ON host_refs(host_uid) WHERE ref_kind = 'alias' AND state = 'current';
CREATE UNIQUE INDEX host_refs_one_current_label_uq
  ON host_refs(host_uid) WHERE ref_kind = 'label' AND state = 'current';

-- Transport paths to a HostUid (plan §12.3); identity lives in hosts.
CREATE TABLE routes (
  route_id            INTEGER PRIMARY KEY,
  host_uid            TEXT    NOT NULL REFERENCES hosts(host_uid),
  transport           TEXT    NOT NULL CHECK (transport IN ('local', 'openssh', 'wez-ssh')),
  endpoint            TEXT    NOT NULL,
  username            TEXT,
  wez_domain          TEXT,
  network_class       TEXT    NOT NULL CHECK (network_class IN ('usb', 'tailscale', 'lan', 'other')),
  priority            INTEGER NOT NULL,
  required_capability TEXT,
  trust_fingerprint   TEXT,
  enabled             INTEGER NOT NULL CHECK (enabled IN (0, 1)),
  last_outcome        TEXT,
  last_outcome_at     TEXT
);

-- v1: exactly one managed unix-Wez instance and one default tmux namespace
-- per owner (plan §2.15).
CREATE TABLE backend_instances (
  backend_instance_uid       TEXT PRIMARY KEY,
  owner_host_uid             TEXT NOT NULL REFERENCES hosts(host_uid),
  backend                    TEXT NOT NULL CHECK (backend IN ('wez', 'tmux')),
  socket_path                TEXT,
  service_label              TEXT,
  server_epoch               TEXT,
  server_pid                 INTEGER,
  server_start_token         TEXT,
  socket_dev                 INTEGER,
  socket_ino                 INTEGER,
  intentional_empty_revision INTEGER,
  created_at                 TEXT NOT NULL
);
CREATE UNIQUE INDEX backend_instances_one_per_owner_uq
  ON backend_instances(owner_host_uid, backend);

CREATE TABLE spaces (
  space_uid           TEXT    NOT NULL,
  owner_host_uid      TEXT    NOT NULL REFERENCES hosts(host_uid),
  space_no            INTEGER NOT NULL CHECK (space_no >= 1),
  backend_instance_id TEXT    NOT NULL REFERENCES backend_instances(backend_instance_uid),
  logical_name        TEXT    NOT NULL,
  lifecycle           TEXT    NOT NULL CHECK
    (lifecycle IN ('reserved', 'active', 'deleting', 'deleted', 'conflict', 'aborted')),
  health              TEXT    NOT NULL CHECK
    (health IN ('healthy', 'multi_window', 'native_key_collision', 'unstamped', 'unknown')),
  default_cwd         TEXT,
  created_at          TEXT    NOT NULL,
  updated_at          TEXT    NOT NULL,
  deleted_at          TEXT
);
-- REQUIRED (plan §10.1, verbatim semantics):
CREATE UNIQUE INDEX spaces_owner_no_uq
  ON spaces(owner_host_uid, space_no);
CREATE UNIQUE INDEX spaces_uid_uq
  ON spaces(space_uid);
CREATE UNIQUE INDEX spaces_live_name_uq
  ON spaces(backend_instance_id, logical_name COLLATE BINARY)
  WHERE lifecycle IN ('reserved','active','deleting','conflict');
-- Deleted/aborted rows are never deleted; UIDs/numbers never reused.

-- Diagnostic rename history; old names are hints, never permanent aliases.
CREATE TABLE space_name_history (
  space_uid     TEXT NOT NULL REFERENCES spaces(space_uid),
  old_name      TEXT NOT NULL,
  new_name      TEXT NOT NULL,
  operation_uid TEXT,
  changed_at    TEXT NOT NULL
);

-- Current native token/key per Space plus observation metadata.
CREATE TABLE native_bindings (
  binding_id          INTEGER PRIMARY KEY,
  space_uid           TEXT NOT NULL REFERENCES spaces(space_uid),
  backend_instance_id TEXT NOT NULL REFERENCES backend_instances(backend_instance_uid),
  native_token        TEXT NOT NULL,
  native_kind         TEXT NOT NULL CHECK (native_kind IN ('wez_workspace_key', 'tmux_session_id')),
  binding_state       TEXT NOT NULL CHECK (binding_state IN ('current', 'superseded', 'severed')),
  server_epoch        TEXT,
  observation         TEXT NOT NULL CHECK
    (observation IN ('live', 'absent', 'stopped', 'unreachable', 'incompatible', 'unmanaged')),
  observed_at         TEXT,
  bound_at            TEXT NOT NULL
);
-- REQUIRED (plan §10.1, verbatim semantics):
CREATE UNIQUE INDEX bindings_current_native_uq
  ON native_bindings(backend_instance_id, native_token)
  WHERE binding_state = 'current';
CREATE UNIQUE INDEX bindings_one_current_per_space_uq
  ON native_bindings(space_uid) WHERE binding_state = 'current';

-- Create/rename/remove/adopt/rebind journal (plan §10.2).
CREATE TABLE operations (
  operation_uid   TEXT PRIMARY KEY,
  space_uid       TEXT NOT NULL REFERENCES spaces(space_uid),
  kind            TEXT NOT NULL CHECK
    (kind IN ('create', 'rename', 'remove', 'adopt', 'rebind', 'normalize', 'stamp')),
  operation_state TEXT NOT NULL CHECK
    (operation_state IN ('prepared', 'running', 'unknown', 'completed', 'failed', 'aborted', 'conflict')),
  request_uid     TEXT NOT NULL,
  payload_json    TEXT NOT NULL,
  fencing_token   INTEGER,
  started_at      TEXT NOT NULL,
  updated_at      TEXT NOT NULL,
  finished_at     TEXT
);
-- REQUIRED (plan §10.1, verbatim semantics):
CREATE UNIQUE INDEX operations_one_unfinished_uq
  ON operations(space_uid)
  WHERE operation_state IN ('prepared','running','unknown');

-- Provisional pane bootstrap state machine (plan §11.1, ADR 004).
CREATE TABLE bootstrap_requests (
  request_uid         TEXT PRIMARY KEY,
  operation_uid       TEXT REFERENCES operations(operation_uid),
  space_uid           TEXT REFERENCES spaces(space_uid),
  backend_instance_id TEXT NOT NULL REFERENCES backend_instances(backend_instance_uid),
  server_epoch        TEXT NOT NULL,
  intended_parent     TEXT,
  recovery_generation TEXT,
  manifest_node_path  TEXT,
  returned_native_ids TEXT,
  final_group_ref     TEXT,
  final_split_ref     TEXT,
  state               TEXT NOT NULL CHECK
    (state IN ('issued', 'spawned', 'correlated', 'acked', 'completed',
               'timeout', 'orphaned', 'conflict', 'aborted')),
  created_at          TEXT NOT NULL,
  updated_at          TEXT NOT NULL
);

-- Renewable fencing leases (plan §10.1). SQLite rows record ownership and
-- recovery; POSIX fcntl locks provide the non-stealable exclusion. Clock
-- expiry alone NEVER authorizes takeover.
CREATE TABLE lease_scopes (
  scope              TEXT PRIMARY KEY,
  last_fencing_token INTEGER NOT NULL CHECK (last_fencing_token >= 0)
);
CREATE TABLE leases (
  lease_id           INTEGER PRIMARY KEY,
  scope              TEXT    NOT NULL REFERENCES lease_scopes(scope),
  holder_request_uid TEXT    NOT NULL,
  fencing_token      INTEGER NOT NULL,
  holder_pid         INTEGER,
  holder_start_token TEXT,
  boot_id            TEXT,
  expires_at         TEXT    NOT NULL,
  renewed_at         TEXT    NOT NULL,
  state              TEXT    NOT NULL CHECK
    (state IN ('held', 'released', 'superseded', 'expired_observed'))
);
CREATE UNIQUE INDEX leases_scope_held_uq ON leases(scope) WHERE state = 'held';
CREATE UNIQUE INDEX leases_scope_token_uq ON leases(scope, fencing_token);

-- Complete/partial scan records (observation cache; never identity).
CREATE TABLE backend_scans (
  scan_id             INTEGER PRIMARY KEY,
  backend_instance_id TEXT NOT NULL REFERENCES backend_instances(backend_instance_uid),
  server_epoch        TEXT,
  outcome             TEXT NOT NULL CHECK
    (outcome IN ('complete', 'stopped', 'unreachable', 'auth_failed',
                 'host_key_identity_failed', 'command_missing', 'version_mismatch',
                 'protocol_mismatch', 'malformed', 'timeout', 'permission_failure')),
  rows_json           TEXT,
  scanned_at          TEXT NOT NULL
);

-- Owner-side idempotency ledger for the remote agent (plan §12.1).
CREATE TABLE rpc_requests (
  request_uid    TEXT PRIMARY KEY,
  method         TEXT NOT NULL,
  payload_sha256 TEXT NOT NULL,
  operation_uid  TEXT REFERENCES operations(operation_uid),
  result_state   TEXT NOT NULL CHECK (result_state IN ('final', 'unknown')),
  result_json    TEXT,
  received_at    TEXT NOT NULL,
  finished_at    TEXT
);

-- Read-only snapshots of remote authorities plus lineage checkpoints.
CREATE TABLE remote_cache (
  host_uid            TEXT PRIMARY KEY REFERENCES hosts(host_uid),
  registry_uid        TEXT NOT NULL,
  authority_revision  INTEGER NOT NULL,
  authority_head_hash TEXT NOT NULL,
  snapshot_json       TEXT NOT NULL,
  fetched_at          TEXT NOT NULL
);

-- Manifest-node restore progress for crash-resumable cold recovery (§15.3).
CREATE TABLE recovery_journal (
  generation_uid        TEXT NOT NULL,
  backend_instance_id   TEXT NOT NULL REFERENCES backend_instances(backend_instance_uid),
  server_epoch          TEXT NOT NULL,
  manifest_id           TEXT NOT NULL,
  space_uid             TEXT REFERENCES spaces(space_uid),
  manifest_node_path    TEXT NOT NULL,
  node_state            TEXT NOT NULL CHECK
    (node_state IN ('pending', 'preparing', 'restoring', 'completed', 'failed', 'skipped')),
  bootstrap_request_uid TEXT REFERENCES bootstrap_requests(request_uid),
  updated_at            TEXT NOT NULL,
  PRIMARY KEY (generation_uid, manifest_node_path)
);
"#;

/// Schema v2 (ADR 009 §3): single-use PTY attach tokens and per-pane stamp
/// acknowledgements, plus a hardening index for the route-upsert key. Purely
/// additive — no v1 row is touched.
const V2_DDL: &str = r#"
-- Single-use PTY attach tokens (plan §12.1, ADR 009 §3). Only the sha256
-- lowercase-hex of a token is ever stored; expiry/replay/revocation never
-- deletes a row — the journal is kept for audit.
CREATE TABLE attach_tokens (
  token_hash   TEXT PRIMARY KEY,  -- sha256 lowercase hex of the opaque token
  request_uid  TEXT NOT NULL UNIQUE,
  host_uid     TEXT NOT NULL REFERENCES hosts(host_uid),
  space_uid    TEXT NOT NULL,
  server_epoch TEXT NOT NULL,
  route        TEXT NOT NULL,     -- route the token is bound to
  attach_argv  TEXT NOT NULL,     -- JSON argv of the exact owner-generated attach command
  issued_at    TEXT NOT NULL,
  expires_at   TEXT NOT NULL,
  state        TEXT NOT NULL CHECK (state IN ('issued', 'redeemed', 'expired', 'revoked')),
  redeemed_at  TEXT
);

-- Per-pane stamp acknowledgements for adopted-Space completion (plan §10.3).
-- Epoch-scoped observation state; an upsert refreshes stamped_at.
CREATE TABLE pane_stamps (
  space_uid    TEXT NOT NULL REFERENCES spaces(space_uid),
  server_epoch TEXT NOT NULL,
  pane_handle  TEXT NOT NULL,     -- canonical provider handle string, e.g. 'tx-13' / 'wz-42'
  stamped_at   TEXT NOT NULL,
  PRIMARY KEY (space_uid, server_epoch, pane_handle)
);

-- The v2 route-upsert API is keyed on (host_uid, transport, endpoint); the
-- database enforces the same key.
CREATE UNIQUE INDEX routes_host_transport_endpoint_uq
  ON routes(host_uid, transport, endpoint);
"#;

/// Apply the normative per-connection settings from the contract header:
/// foreign_keys=ON, journal_mode=WAL, synchronous=FULL, trusted_schema=OFF,
/// busy_timeout (5000 ms in production). Returns the resulting journal mode
/// so the caller can verify `wal`.
pub fn apply_connection_settings(
    conn: &Connection,
    busy_timeout: Duration,
) -> rusqlite::Result<String> {
    conn.busy_timeout(busy_timeout)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "trusted_schema", "OFF")?;
    let mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
    conn.pragma_update(None, "synchronous", "FULL")?;
    Ok(mode)
}

pub fn user_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
}

/// Apply every pending migration, each in its own transaction, advancing
/// `user_version` and mirroring `meta.schema_version` where a meta row
/// exists. Caller must hold the exclusive maintenance gate.
pub fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    migrate_to(conn, SCHEMA_VERSION)
}

/// [`migrate`] stopping at `target` — a seam for migration tests and
/// tooling that need a database frozen at an older schema version.
pub fn migrate_to(conn: &mut Connection, target: i64) -> rusqlite::Result<()> {
    loop {
        let version = user_version(conn)?;
        if version >= target {
            return Ok(());
        }
        let Some((next, ddl)) = MIGRATIONS.iter().find(|(v, _)| *v == version + 1) else {
            return Ok(());
        };
        let tx = conn.transaction()?;
        tx.execute_batch(ddl)?;
        tx.pragma_update(None, "user_version", *next)?;
        // Mirror into meta when the row exists (no-op on first init, where
        // the meta row is inserted afterwards with the current version).
        tx.execute("UPDATE meta SET schema_version = ?1", [*next])?;
        tx.commit()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_a_fresh_database_to_current_version() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = Connection::open(dir.path().join("t.sqlite3")).unwrap();
        let mode = apply_connection_settings(&conn, Duration::from_millis(100)).unwrap();
        assert_eq!(mode.to_ascii_lowercase(), "wal");
        migrate(&mut conn).unwrap();
        assert_eq!(user_version(&conn).unwrap(), SCHEMA_VERSION);
        // Idempotent.
        migrate(&mut conn).unwrap();
        assert_eq!(user_version(&conn).unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn migrate_to_stops_at_the_target_and_resumes_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = Connection::open(dir.path().join("t.sqlite3")).unwrap();
        apply_connection_settings(&conn, Duration::from_millis(100)).unwrap();
        migrate_to(&mut conn, 1).unwrap();
        assert_eq!(user_version(&conn).unwrap(), 1);
        let has_tokens: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='attach_tokens'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(has_tokens, 0, "v1 database must not have v2 tables");
        migrate(&mut conn).unwrap();
        assert_eq!(user_version(&conn).unwrap(), SCHEMA_VERSION);
        for table in ["attach_tokens", "pane_stamps"] {
            let n: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(n, 1, "v2 table {table} missing");
        }
    }

    #[test]
    fn all_required_partial_indexes_exist_with_contract_semantics() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = Connection::open(dir.path().join("t.sqlite3")).unwrap();
        apply_connection_settings(&conn, Duration::from_millis(100)).unwrap();
        migrate(&mut conn).unwrap();
        // The five REQUIRED indexes, by name, must exist and be UNIQUE.
        for name in [
            "spaces_owner_no_uq",
            "spaces_uid_uq",
            "spaces_live_name_uq",
            "bindings_current_native_uq",
            "operations_one_unfinished_uq",
            "routes_host_transport_endpoint_uq",
        ] {
            let sql: String = conn
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE type='index' AND name=?1",
                    [name],
                    |row| row.get(0),
                )
                .unwrap_or_else(|_| panic!("required index {name} missing"));
            assert!(
                sql.to_ascii_uppercase().contains("UNIQUE"),
                "{name} must be UNIQUE: {sql}"
            );
        }
    }
}
