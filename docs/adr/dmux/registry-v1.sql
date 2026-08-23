-- dmux registry v1 — normative SQL contract (plan §10.1)
--
-- Status: frozen at P1. This file is a CONTRACT ARTIFACT, not executable
-- migration code. P2's identity agent implements it in
-- scripts/rust/crates/dmux/src/registry/schema.rs. Equivalent index names are
-- allowed; weaker semantics are not. Every `-- REQUIRED` block is verbatim
-- from the plan and must survive with identical semantics.
--
-- Runtime connection settings (not DDL, normative all the same):
--   foreign_keys=ON, journal_mode=WAL, synchronous=FULL, trusted_schema=OFF,
--   busy_timeout=5000. SQLITE_BUSY: bounded jittered retries for reads and
--   short transitions, then typed `registry_busy` with no native action
--   started. Backups: online backup API after a checked WAL checkpoint only.
--   Location: $XDG_DATA_HOME/dmux/registry.sqlite3 (dir 0700, db 0600),
--   never inside synced dotfiles.
--
-- Conventions: UUIDs are lowercase hyphenated TEXT. Timestamps are UTC
-- RFC 3339 TEXT. All state strings are the plan's exact tokens.

-- One row: this installation's authority identity and counters.
CREATE TABLE meta (
  id                  INTEGER PRIMARY KEY CHECK (id = 1),
  schema_version      INTEGER NOT NULL,
  host_uid            TEXT    NOT NULL,  -- UUIDv4; permanent unless explicit rekey
  registry_uid        TEXT    NOT NULL,  -- UUIDv4; clone/rollback detection root
  authority_revision  INTEGER NOT NULL,  -- current head revision (mirror of authority_revisions)
  authority_head_hash TEXT    NOT NULL,
  space_no_counter    INTEGER NOT NULL CHECK (space_no_counter >= 1),
  -- next SpaceNo to hand out; monotone, never decremented, gaps intentional
  created_at          TEXT    NOT NULL
);

-- Append-only hash chain: every committed authority mutation advances one row.
CREATE TABLE authority_revisions (
  revision         INTEGER PRIMARY KEY,       -- strictly increasing, no reuse
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

-- Compact aliases and labels. A spelling, once used, is never rebound to a
-- different HostUid: the (ref_kind, spelling) primary key plus the rule that
-- rows are only ever transitioned current -> historical/tombstoned (never
-- deleted, never repointed) carries that guarantee.
CREATE TABLE host_refs (
  ref_kind   TEXT NOT NULL CHECK (ref_kind IN ('alias', 'label')),
  spelling   TEXT NOT NULL,  -- alias: bijective base-26 [a-z]+; label: [a-z][a-z0-9-]{0,31}
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

-- Transport paths to a HostUid (plan §12.3). Mutable and replaceable;
-- identity lives in hosts, never here.
CREATE TABLE routes (
  route_id            INTEGER PRIMARY KEY,
  host_uid            TEXT    NOT NULL REFERENCES hosts(host_uid),
  transport           TEXT    NOT NULL CHECK (transport IN ('local', 'openssh', 'wez-ssh')),
  endpoint            TEXT    NOT NULL,
  username            TEXT,
  wez_domain          TEXT,             -- generated stable native domain name
  network_class       TEXT    NOT NULL CHECK (network_class IN ('usb', 'tailscale', 'lan', 'other')),
  priority            INTEGER NOT NULL, -- lower tries first within eligibility
  required_capability TEXT,
  trust_fingerprint   TEXT,
  enabled             INTEGER NOT NULL CHECK (enabled IN (0, 1)),
  last_outcome        TEXT,             -- typed outcome token, diagnostics only
  last_outcome_at     TEXT
);

-- v1: exactly one managed unix-Wez instance and one default tmux namespace
-- per owner (plan §2.15) — enforced by the unique index below.
CREATE TABLE backend_instances (
  backend_instance_uid       TEXT PRIMARY KEY,
  owner_host_uid             TEXT NOT NULL REFERENCES hosts(host_uid),
  backend                    TEXT NOT NULL CHECK (backend IN ('wez', 'tmux')),
  socket_path                TEXT,     -- wez: exact service socket; tmux: -L namespace
  service_label              TEXT,     -- systemd unit / launchd label
  server_epoch               TEXT,     -- current epoch UUID, NULL when stopped/unknown
  server_pid                 INTEGER,
  server_start_token         TEXT,
  socket_dev                 INTEGER,  -- ADR 001/002: replacement detection
  socket_ino                 INTEGER,
  intentional_empty_revision INTEGER,  -- plan §15.3: recovery eligibility floor
  created_at                 TEXT NOT NULL
);
CREATE UNIQUE INDEX backend_instances_one_per_owner_uq
  ON backend_instances(owner_host_uid, backend);

CREATE TABLE spaces (
  space_uid           TEXT    NOT NULL,  -- UUIDv7
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
-- Deleted/aborted rows are never deleted; their UIDs and numbers are never
-- reused (no DELETE on this table, ever).

-- Diagnostic rename history; old names are hints, never permanent aliases.
CREATE TABLE space_name_history (
  space_uid     TEXT NOT NULL REFERENCES spaces(space_uid),
  old_name      TEXT NOT NULL,
  new_name      TEXT NOT NULL,
  operation_uid TEXT,
  changed_at    TEXT NOT NULL
);

-- Current native token/key per Space plus observation metadata.
-- wez: the opaque workspace key `dmux:<host-uid>:<space-uid>`;
-- tmux: the immutable session id `$N` (name is mutable and lives in spaces).
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
  request_uid     TEXT NOT NULL,  -- client idempotency key (joins rpc_requests)
  payload_json    TEXT NOT NULL,  -- exact intent: names, expected versions, native refs
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
  intended_parent     TEXT,           -- exact native parent locator (window/pane id) or NULL for new Space
  recovery_generation TEXT,           -- set only for recovery-created panes
  manifest_node_path  TEXT,
  returned_native_ids TEXT,           -- JSON: raw spawn-return values (ADR 004 formats)
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
  scope              TEXT PRIMARY KEY,  -- 'backend:<uid>' | 'recovery:<uid>' | 'snapshot:<uid>' | 'decision:<owner>:<sha256>' | 'maintenance'
  last_fencing_token INTEGER NOT NULL CHECK (last_fencing_token >= 0)
);
CREATE TABLE leases (
  lease_id           INTEGER PRIMARY KEY,
  scope              TEXT    NOT NULL REFERENCES lease_scopes(scope),
  holder_request_uid TEXT    NOT NULL,
  fencing_token      INTEGER NOT NULL,  -- assigned from lease_scopes.last_fencing_token + 1, atomically
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
  rows_json           TEXT,   -- present only for 'complete'
  scanned_at          TEXT NOT NULL
);

-- Owner-side idempotency ledger for the remote agent (plan §12.1).
CREATE TABLE rpc_requests (
  request_uid    TEXT PRIMARY KEY,
  method         TEXT NOT NULL,
  payload_sha256 TEXT NOT NULL,  -- reuse of request_uid with different digest is rejected
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

-- ===========================================================================
-- v2 appendix (W5 / ADR 009 §3): applied by migration when user_version < 2.
-- Implemented in registry/schema.rs (SCHEMA_VERSION = 2); mirrored here
-- verbatim from the identity agent's W5 handoff.

-- Single-use remote tmux attach tokens (plan §12.1). Only the sha256 of a
-- token is ever stored; rows are retained (never deleted) for audit.
CREATE TABLE attach_tokens (
  token_hash   TEXT PRIMARY KEY,  -- sha256 lowercase hex of the opaque token
  request_uid  TEXT NOT NULL UNIQUE,
  host_uid     TEXT NOT NULL REFERENCES hosts(host_uid),
  space_uid    TEXT NOT NULL,
  server_epoch TEXT NOT NULL,
  route        TEXT NOT NULL,
  attach_argv  TEXT NOT NULL,     -- JSON argv of the exact owner-generated attach command
  issued_at    TEXT NOT NULL,
  expires_at   TEXT NOT NULL,
  state        TEXT NOT NULL CHECK (state IN ('issued', 'redeemed', 'expired', 'revoked')),
  redeemed_at  TEXT
);

-- Per-pane marker acknowledgements for adopted Spaces (plan §10.3): health
-- becomes healthy only when every live pane has one current-epoch stamp.
CREATE TABLE pane_stamps (
  space_uid    TEXT NOT NULL REFERENCES spaces(space_uid),
  server_epoch TEXT NOT NULL,
  pane_handle  TEXT NOT NULL,
  stamped_at   TEXT NOT NULL,
  PRIMARY KEY (space_uid, server_epoch, pane_handle)
);

-- Enforces the frozen upsert key for routes (safe retrofit: no v1 code
-- ever wrote the routes table).
CREATE UNIQUE INDEX routes_host_transport_endpoint_uq
  ON routes(host_uid, transport, endpoint);

-- ===========================================================================
-- v5 appendix (ADR 012 WS-D.2; plan §10.3): applied by migration when
-- user_version < 5. Implemented in registry/schema.rs (SCHEMA_VERSION = 5).
-- The adopt/rebind journal row records the exact native token it was opened
-- for (tmux session id; Wez workspace name before its CAS rename to the
-- opaque key). NULL on create rows and on rows journaled before v5, which
-- reconciliation reverses to the logical name as before.
ALTER TABLE operations ADD COLUMN source_native_token TEXT;

