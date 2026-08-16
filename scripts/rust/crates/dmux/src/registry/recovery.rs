//! Fenced, crash-resumable cold-recovery journal (plan §15.3).
//!
//! Recovery bookkeeping never advances the authority revision. Every write
//! verifies the currently-held recovery lease in the same `BEGIN IMMEDIATE`
//! transaction as the journal mutation. The recovery and snapshot database
//! scopes share the backend-instance kernel lock; callers acquire that lock
//! before obtaining the lease through the normal registry API.

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::locks::{HeldLock, LockMode, LockScope};
use crate::model::{BackendInstanceUid, ServerEpoch, SpaceUid};

use super::{Lease, LeaseScope, Registry, RegistryError, Result, now_rfc3339, parse_uuid};

/// The reserved row which records generation-wide progress.
pub const RECOVERY_GENERATION_PATH: &str = "@generation";

/// One recovery generation or manifest-node state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryNodeState {
    Pending,
    Preparing,
    Restoring,
    Completed,
    Failed,
    Skipped,
    Aborted,
}

impl RecoveryNodeState {
    pub fn as_str(self) -> &'static str {
        match self {
            RecoveryNodeState::Pending => "pending",
            RecoveryNodeState::Preparing => "preparing",
            RecoveryNodeState::Restoring => "restoring",
            RecoveryNodeState::Completed => "completed",
            RecoveryNodeState::Failed => "failed",
            RecoveryNodeState::Skipped => "skipped",
            RecoveryNodeState::Aborted => "aborted",
        }
    }

    pub fn parse(token: &str) -> Option<Self> {
        Some(match token {
            "pending" => RecoveryNodeState::Pending,
            "preparing" => RecoveryNodeState::Preparing,
            "restoring" => RecoveryNodeState::Restoring,
            "completed" => RecoveryNodeState::Completed,
            "failed" => RecoveryNodeState::Failed,
            "skipped" => RecoveryNodeState::Skipped,
            "aborted" => RecoveryNodeState::Aborted,
            _ => return None,
        })
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            RecoveryNodeState::Completed | RecoveryNodeState::Skipped | RecoveryNodeState::Aborted
        )
    }
}

/// Immutable identity of one manifest node in a generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryNodeSpec {
    pub space_uid: Option<SpaceUid>,
    pub manifest_node_path: String,
}

/// Immutable identity shared by every row in a generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryGenerationSpec {
    pub generation_uid: Uuid,
    pub backend_instance: BackendInstanceUid,
    pub server_epoch: ServerEpoch,
    pub manifest_id: String,
}

/// One complete `recovery_journal` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryJournalRow {
    pub generation_uid: Uuid,
    pub backend_instance: BackendInstanceUid,
    pub server_epoch: ServerEpoch,
    pub manifest_id: String,
    pub space_uid: Option<SpaceUid>,
    pub manifest_node_path: String,
    pub node_state: RecoveryNodeState,
    pub bootstrap_request_uid: Option<Uuid>,
    pub updated_at: String,
}

/// Result of atomically creating a generation or exactly replaying it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BeginRecovery {
    Created(Vec<RecoveryJournalRow>),
    Replay(Vec<RecoveryJournalRow>),
}

const RECOVERY_COLUMNS: &str = "generation_uid, backend_instance_id, server_epoch, manifest_id, \
                               space_uid, manifest_node_path, node_state, \
                               bootstrap_request_uid, updated_at";

type RawRecoveryRow = (
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    String,
);

fn map_recovery_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawRecoveryRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
    ))
}

fn finish_recovery_row(raw: RawRecoveryRow) -> Result<RecoveryJournalRow> {
    let (generation, instance, epoch, manifest, space, path, state, bootstrap, updated) = raw;
    Ok(RecoveryJournalRow {
        generation_uid: parse_uuid(&generation)?,
        backend_instance: BackendInstanceUid(parse_uuid(&instance)?),
        server_epoch: ServerEpoch(parse_uuid(&epoch)?),
        manifest_id: manifest,
        space_uid: space
            .as_deref()
            .map(|uid| parse_uuid(uid).map(SpaceUid))
            .transpose()?,
        manifest_node_path: path,
        node_state: RecoveryNodeState::parse(&state)
            .ok_or_else(|| RegistryError::Corrupt(format!("recovery node state {state:?}")))?,
        bootstrap_request_uid: bootstrap.as_deref().map(parse_uuid).transpose()?,
        updated_at: updated,
    })
}

fn recovery_rows_on(conn: &Connection, generation_uid: Uuid) -> Result<Vec<RecoveryJournalRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {RECOVERY_COLUMNS} FROM recovery_journal \
         WHERE generation_uid = ?1 ORDER BY manifest_node_path COLLATE BINARY"
    ))?;
    let mapped = stmt.query_map([generation_uid.to_string()], map_recovery_row)?;
    let mut rows = Vec::new();
    for row in mapped {
        rows.push(finish_recovery_row(row?)?);
    }
    Ok(rows)
}

fn unfinished_recovery_on(
    conn: &Connection,
    instance: BackendInstanceUid,
    epoch: Option<ServerEpoch>,
) -> Result<Option<(RecoveryGenerationSpec, Vec<RecoveryJournalRow>)>> {
    let mut sql = String::from(
        "SELECT generation_uid, server_epoch, manifest_id FROM recovery_journal \
         WHERE backend_instance_id = ?1 ",
    );
    if epoch.is_some() {
        sql.push_str("AND server_epoch = ?2 ");
    }
    sql.push_str(
        "AND manifest_node_path = ?3 \
         AND node_state IN ('pending', 'preparing', 'restoring', 'failed') \
         ORDER BY generation_uid",
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut roots = if let Some(epoch) = epoch {
        stmt.query_map(
            params![
                instance.0.to_string(),
                epoch.0.to_string(),
                RECOVERY_GENERATION_PATH
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        // Numbered parameters deliberately retain ?3 so the exact-epoch and
        // all-epoch forms share one query shape.
        stmt.query_map(
            params![
                instance.0.to_string(),
                rusqlite::types::Null,
                RECOVERY_GENERATION_PATH
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    match roots.len() {
        0 => Ok(None),
        1 => {
            let (generation, server_epoch, manifest_id) = roots.pop().expect("one root");
            let generation_uid = parse_uuid(&generation)?;
            let rows = recovery_rows_on(conn, generation_uid)?;
            Ok(Some((
                RecoveryGenerationSpec {
                    generation_uid,
                    backend_instance: instance,
                    server_epoch: ServerEpoch(parse_uuid(&server_epoch)?),
                    manifest_id,
                },
                rows,
            )))
        }
        count => Err(recovery_error(format!(
            "{count} unfinished roots for backend instance {}{}",
            instance.0,
            epoch
                .map(|value| format!(" epoch {}", value.0))
                .unwrap_or_default()
        ))),
    }
}

fn recovery_error(message: impl Into<String>) -> RegistryError {
    RegistryError::Corrupt(format!("recovery journal: {}", message.into()))
}

fn require_published_epoch(
    conn: &Connection,
    instance: BackendInstanceUid,
    epoch: ServerEpoch,
) -> Result<()> {
    let published: Option<Option<String>> = conn
        .query_row(
            "SELECT server_epoch FROM backend_instances WHERE backend_instance_uid = ?1",
            [instance.0.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    let Some(published) = published else {
        return Err(RegistryError::NotFound {
            what: format!("backend instance {}", instance.0),
        });
    };
    let Some(published) = published else {
        return Err(recovery_error(format!(
            "backend instance {} has no published server epoch",
            instance.0
        )));
    };
    if parse_uuid(&published)? != epoch.0 {
        return Err(recovery_error(format!(
            "backend instance {} epoch mismatch",
            instance.0
        )));
    }
    Ok(())
}

fn assert_lease_fence_on(conn: &Connection, scope: &LeaseScope, lease: &Lease) -> Result<()> {
    let expected_scope = scope.as_scope_string();
    if lease.scope != expected_scope {
        return Err(recovery_error(format!(
            "lease scope {:?} does not match {:?}",
            lease.scope, expected_scope
        )));
    }
    let current: i64 = conn.query_row(
        "SELECT count(*) FROM leases AS l \
         JOIN lease_scopes AS s ON s.scope = l.scope \
         WHERE l.lease_id = ?1 AND l.scope = ?2 AND l.holder_request_uid = ?3 \
           AND l.fencing_token = ?4 AND l.state = 'held' \
           AND s.last_fencing_token = l.fencing_token",
        params![
            lease.lease_id,
            expected_scope,
            lease.holder_request_uid.to_string(),
            lease.fencing_token
        ],
        |row| row.get(0),
    )?;
    if current != 1 {
        return Err(recovery_error(format!(
            "stale lease fence {} for {:?}",
            lease.fencing_token, lease.scope
        )));
    }
    Ok(())
}

fn node_can_transition(from: RecoveryNodeState, to: RecoveryNodeState) -> bool {
    use RecoveryNodeState::*;
    matches!(
        (from, to),
        (Pending, Preparing | Skipped | Aborted)
            | (Preparing, Restoring | Failed | Aborted)
            | (Restoring, Completed | Failed | Preparing | Aborted)
            | (Failed, Preparing | Aborted)
    )
}

fn generation_can_transition(from: RecoveryNodeState, to: RecoveryNodeState) -> bool {
    use RecoveryNodeState::*;
    matches!(
        (from, to),
        (Pending, Preparing)
            | (Preparing, Restoring)
            | (Restoring, Completed | Failed)
            | (Failed, Preparing)
    )
}

fn is_abortable(state: RecoveryNodeState) -> bool {
    matches!(
        state,
        RecoveryNodeState::Pending
            | RecoveryNodeState::Preparing
            | RecoveryNodeState::Restoring
            | RecoveryNodeState::Failed
    )
}

fn checked_generation_root(
    generation_uid: Uuid,
    rows: &[RecoveryJournalRow],
) -> Result<RecoveryJournalRow> {
    let roots = rows
        .iter()
        .filter(|row| row.manifest_node_path == RECOVERY_GENERATION_PATH)
        .collect::<Vec<_>>();
    if roots.len() != 1 {
        return Err(recovery_error(format!(
            "generation {generation_uid} has {} root rows",
            roots.len()
        )));
    }
    let root = roots[0];
    if rows.iter().any(|row| {
        row.backend_instance != root.backend_instance
            || row.server_epoch != root.server_epoch
            || row.manifest_id != root.manifest_id
    }) {
        return Err(recovery_error(format!(
            "generation {generation_uid} has inconsistent row identity"
        )));
    }
    Ok(root.clone())
}

fn abort_generation_rows_on(
    conn: &Connection,
    generation_uid: Uuid,
    expected_root: RecoveryNodeState,
    rows: &[RecoveryJournalRow],
) -> Result<Vec<RecoveryJournalRow>> {
    let root = checked_generation_root(generation_uid, rows)?;
    if root.node_state == RecoveryNodeState::Aborted {
        if rows.iter().any(|row| !row.node_state.is_terminal()) {
            return Err(recovery_error(format!(
                "aborted generation {generation_uid} has nonterminal rows"
            )));
        }
        return Ok(rows.to_vec());
    }
    if root.node_state != expected_root {
        return Err(recovery_error(format!(
            "generation {generation_uid} is {}, expected {}",
            root.node_state.as_str(),
            expected_root.as_str()
        )));
    }

    let now = now_rfc3339();
    let changed = conn.execute(
        "UPDATE recovery_journal SET node_state = 'aborted', updated_at = ?3 \
         WHERE generation_uid = ?1 AND manifest_node_path = ?2 AND node_state = ?4",
        params![
            generation_uid.to_string(),
            RECOVERY_GENERATION_PATH,
            now,
            expected_root.as_str()
        ],
    )?;
    if changed != 1 {
        return Err(recovery_error(format!(
            "compare-and-set lost for recovery generation {generation_uid}"
        )));
    }
    conn.execute(
        "UPDATE recovery_journal SET node_state = 'aborted', updated_at = ?2 \
         WHERE generation_uid = ?1 AND manifest_node_path <> ?3 \
           AND node_state IN ('pending', 'preparing', 'restoring', 'failed')",
        params![generation_uid.to_string(), now, RECOVERY_GENERATION_PATH],
    )?;

    let rows = recovery_rows_on(conn, generation_uid)?;
    if rows.iter().any(|row| !row.node_state.is_terminal()) {
        return Err(recovery_error(format!(
            "generation {generation_uid} abort left nonterminal rows"
        )));
    }
    Ok(rows)
}

fn validate_nodes(nodes: &[RecoveryNodeSpec]) -> Result<()> {
    let mut paths = HashSet::with_capacity(nodes.len());
    for node in nodes {
        if node.manifest_node_path.is_empty() {
            return Err(recovery_error("manifest node path is empty"));
        }
        if node.manifest_node_path == RECOVERY_GENERATION_PATH {
            return Err(recovery_error(format!(
                "manifest node path {RECOVERY_GENERATION_PATH:?} is reserved"
            )));
        }
        if !paths.insert(node.manifest_node_path.as_str()) {
            return Err(recovery_error(format!(
                "duplicate manifest node path {:?}",
                node.manifest_node_path
            )));
        }
    }
    Ok(())
}

fn replay_is_exact(
    rows: &[RecoveryJournalRow],
    spec: &RecoveryGenerationSpec,
    nodes: &[RecoveryNodeSpec],
) -> bool {
    if rows.len() != nodes.len() + 1 {
        return false;
    }
    let expected: HashMap<&str, Option<SpaceUid>> = nodes
        .iter()
        .map(|node| (node.manifest_node_path.as_str(), node.space_uid))
        .collect();
    let mut saw_root = false;
    for row in rows {
        if row.generation_uid != spec.generation_uid
            || row.backend_instance != spec.backend_instance
            || row.server_epoch != spec.server_epoch
            || row.manifest_id != spec.manifest_id
        {
            return false;
        }
        if row.manifest_node_path == RECOVERY_GENERATION_PATH {
            if saw_root || row.space_uid.is_some() {
                return false;
            }
            saw_root = true;
        } else if expected.get(row.manifest_node_path.as_str()) != Some(&row.space_uid) {
            return false;
        }
    }
    saw_root
}

impl Registry {
    /// The per-instance recovery eligibility floor, or `None` if an
    /// intentional empty has never been recorded.
    pub fn intentional_empty_revision(&self, instance: BackendInstanceUid) -> Result<Option<u64>> {
        let stored: Option<Option<i64>> = self
            .conn
            .query_row(
                "SELECT intentional_empty_revision FROM backend_instances \
                 WHERE backend_instance_uid = ?1",
                [instance.0.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        let Some(stored) = stored else {
            return Err(RegistryError::NotFound {
                what: format!("backend instance {}", instance.0),
            });
        };
        stored
            .map(|value| {
                u64::try_from(value).map_err(|_| {
                    recovery_error(format!("negative intentional-empty revision {value}"))
                })
            })
            .transpose()
    }

    /// Record the authority revision whose complete same-epoch scan proved
    /// the instance intentionally empty. This is journal metadata: it does
    /// not advance authority and it never lowers an existing floor.
    pub fn record_intentional_empty_revision(
        &mut self,
        instance: BackendInstanceUid,
        server_epoch: ServerEpoch,
        revision: u64,
        kernel: &HeldLock,
    ) -> Result<u64> {
        if kernel.mode() != LockMode::Exclusive
            || kernel.scope() != &LockScope::BackendInstance(instance)
        {
            return Err(RegistryError::KernelLockMismatch {
                scope: LeaseScope::Backend(instance).as_scope_string(),
            });
        }
        let revision_i64 = i64::try_from(revision)
            .map_err(|_| recovery_error(format!("authority revision {revision} exceeds i64")))?;
        self.immediate(|tx| {
            require_published_epoch(tx, instance, server_epoch)?;
            let head: i64 = tx.query_row(
                "SELECT authority_revision FROM meta WHERE id = 1",
                [],
                |row| row.get(0),
            )?;
            if head != revision_i64 {
                return Err(recovery_error(format!(
                    "intentional-empty revision {revision} is not current head {head}"
                )));
            }
            tx.execute(
                "UPDATE backend_instances \
                 SET intentional_empty_revision = \
                   CASE WHEN intentional_empty_revision IS NULL \
                          OR intentional_empty_revision < ?2 \
                        THEN ?2 ELSE intentional_empty_revision END \
                 WHERE backend_instance_uid = ?1",
                params![instance.0.to_string(), revision_i64],
            )?;
            let stored: i64 = tx.query_row(
                "SELECT intentional_empty_revision FROM backend_instances \
                 WHERE backend_instance_uid = ?1",
                [instance.0.to_string()],
                |row| row.get(0),
            )?;
            u64::try_from(stored).map_err(|_| {
                recovery_error(format!("negative intentional-empty revision {stored}"))
            })
        })
    }

    /// Atomically capture the current authority head as this instance's
    /// intentional-empty floor.  This is the production remove-path helper:
    /// unrelated authority operations may advance the global head while the
    /// caller holds the backend lock, so selecting the head and storing the
    /// floor must happen in one SQLite transaction.
    pub fn record_current_intentional_empty_revision(
        &mut self,
        instance: BackendInstanceUid,
        server_epoch: ServerEpoch,
        kernel: &HeldLock,
    ) -> Result<u64> {
        if kernel.mode() != LockMode::Exclusive
            || kernel.scope() != &LockScope::BackendInstance(instance)
        {
            return Err(RegistryError::KernelLockMismatch {
                scope: LeaseScope::Backend(instance).as_scope_string(),
            });
        }
        self.immediate(|tx| {
            require_published_epoch(tx, instance, server_epoch)?;
            let revision: i64 = tx.query_row(
                "SELECT authority_revision FROM meta WHERE id = 1",
                [],
                |row| row.get(0),
            )?;
            tx.execute(
                "UPDATE backend_instances \
                 SET intentional_empty_revision = \
                   CASE WHEN intentional_empty_revision IS NULL \
                          OR intentional_empty_revision < ?2 \
                        THEN ?2 ELSE intentional_empty_revision END \
                 WHERE backend_instance_uid = ?1",
                params![instance.0.to_string(), revision],
            )?;
            let stored: i64 = tx.query_row(
                "SELECT intentional_empty_revision FROM backend_instances \
                 WHERE backend_instance_uid = ?1",
                [instance.0.to_string()],
                |row| row.get(0),
            )?;
            u64::try_from(stored).map_err(|_| {
                recovery_error(format!("negative intentional-empty revision {stored}"))
            })
        })
    }

    /// Verify that `lease` is the current held fence for `scope`.
    pub fn assert_lease_fence(&self, scope: &LeaseScope, lease: &Lease) -> Result<()> {
        assert_lease_fence_on(&self.conn, scope, lease)
    }

    /// Create a generation and all of its pending nodes atomically, or
    /// return the existing rows only when the replay identity is exact.
    pub fn begin_recovery(
        &mut self,
        spec: &RecoveryGenerationSpec,
        nodes: &[RecoveryNodeSpec],
        lease: &Lease,
    ) -> Result<BeginRecovery> {
        validate_nodes(nodes)?;
        self.immediate(|tx| {
            let scope = LeaseScope::Recovery(spec.backend_instance);
            assert_lease_fence_on(tx, &scope, lease)?;
            require_published_epoch(tx, spec.backend_instance, spec.server_epoch)?;

            let existing = recovery_rows_on(tx, spec.generation_uid)?;
            if !existing.is_empty() {
                if !replay_is_exact(&existing, spec, nodes) {
                    return Err(recovery_error(format!(
                        "generation {} replay does not exactly match its journal",
                        spec.generation_uid
                    )));
                }
                return Ok(BeginRecovery::Replay(existing));
            }

            let unfinished: Option<String> = tx
                .query_row(
                    "SELECT generation_uid FROM recovery_journal \
                     WHERE backend_instance_id = ?1 \
                       AND manifest_node_path = ?2 \
                       AND node_state IN ('pending', 'preparing', 'restoring', 'failed') \
                     ORDER BY generation_uid LIMIT 1",
                    params![
                        spec.backend_instance.0.to_string(),
                        RECOVERY_GENERATION_PATH
                    ],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(generation) = unfinished {
                return Err(recovery_error(format!(
                    "unfinished generation {generation} already owns backend instance {}",
                    spec.backend_instance.0
                )));
            }

            let now = now_rfc3339();
            tx.execute(
                "INSERT INTO recovery_journal \
                 (generation_uid, backend_instance_id, server_epoch, manifest_id, space_uid, \
                  manifest_node_path, node_state, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, NULL, ?5, 'pending', ?6)",
                params![
                    spec.generation_uid.to_string(),
                    spec.backend_instance.0.to_string(),
                    spec.server_epoch.0.to_string(),
                    spec.manifest_id,
                    RECOVERY_GENERATION_PATH,
                    now
                ],
            )?;
            for node in nodes {
                tx.execute(
                    "INSERT INTO recovery_journal \
                     (generation_uid, backend_instance_id, server_epoch, manifest_id, space_uid, \
                      manifest_node_path, node_state, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7)",
                    params![
                        spec.generation_uid.to_string(),
                        spec.backend_instance.0.to_string(),
                        spec.server_epoch.0.to_string(),
                        spec.manifest_id,
                        node.space_uid.map(|uid| uid.0.to_string()),
                        node.manifest_node_path,
                        now
                    ],
                )?;
            }
            Ok(BeginRecovery::Created(recovery_rows_on(
                tx,
                spec.generation_uid,
            )?))
        })
    }

    /// All rows for one generation, in exact-byte manifest path order.
    pub fn recovery_rows(&self, generation_uid: Uuid) -> Result<Vec<RecoveryJournalRow>> {
        recovery_rows_on(&self.conn, generation_uid)
    }

    /// The unfinished generation for this exact backend epoch. The reserved
    /// root is authoritative; completed generations are not returned.
    pub fn unfinished_recovery(
        &self,
        instance: BackendInstanceUid,
        server_epoch: ServerEpoch,
    ) -> Result<Option<(RecoveryGenerationSpec, Vec<RecoveryJournalRow>)>> {
        unfinished_recovery_on(&self.conn, instance, Some(server_epoch))
    }

    /// The sole unfinished generation for an instance, including one left
    /// behind by an older server epoch.  Callers use this before admitting a
    /// new generation so a server restart cannot hide a durable owner.
    pub fn unfinished_recovery_for_instance(
        &self,
        instance: BackendInstanceUid,
    ) -> Result<Option<(RecoveryGenerationSpec, Vec<RecoveryJournalRow>)>> {
        unfinished_recovery_on(&self.conn, instance, None)
    }

    /// The completed generation for an exact server incarnation.  It is
    /// retained so a coordinator killed after committing the terminal root
    /// but before publishing `ready` can verify the native tree and finish
    /// the sidecar handoff without restoring a second time.
    pub fn completed_recovery(
        &self,
        instance: BackendInstanceUid,
        server_epoch: ServerEpoch,
    ) -> Result<Option<(RecoveryGenerationSpec, Vec<RecoveryJournalRow>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT generation_uid, manifest_id FROM recovery_journal \
             WHERE backend_instance_id = ?1 AND server_epoch = ?2 \
               AND manifest_node_path = ?3 AND node_state = 'completed' \
             ORDER BY generation_uid",
        )?;
        let roots = stmt.query_map(
            params![
                instance.0.to_string(),
                server_epoch.0.to_string(),
                RECOVERY_GENERATION_PATH
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )?;
        let mut roots = roots.collect::<rusqlite::Result<Vec<_>>>()?;
        match roots.len() {
            0 => Ok(None),
            1 => {
                let (generation, manifest_id) = roots.pop().expect("one root");
                let generation_uid = parse_uuid(&generation)?;
                Ok(Some((
                    RecoveryGenerationSpec {
                        generation_uid,
                        backend_instance: instance,
                        server_epoch,
                        manifest_id,
                    },
                    recovery_rows_on(&self.conn, generation_uid)?,
                )))
            }
            count => Err(recovery_error(format!(
                "{count} completed roots for backend instance {} epoch {}",
                instance.0, server_epoch.0
            ))),
        }
    }

    /// Compare-and-set one node under the current recovery fence. A lost-ack
    /// replay is accepted only when the row is already in `to` and its
    /// bootstrap request UID exactly matches the supplied UID. On a real
    /// transition, `None` preserves the currently linked bootstrap request.
    pub fn transition_recovery_node(
        &mut self,
        generation_uid: Uuid,
        manifest_node_path: &str,
        expected: RecoveryNodeState,
        to: RecoveryNodeState,
        bootstrap_request_uid: Option<Uuid>,
        lease: &Lease,
    ) -> Result<RecoveryJournalRow> {
        if manifest_node_path.is_empty() {
            return Err(recovery_error("manifest node path is empty"));
        }
        let path = manifest_node_path.to_string();
        self.immediate(|tx| {
            let row = tx
                .query_row(
                    &format!(
                        "SELECT {RECOVERY_COLUMNS} FROM recovery_journal \
                         WHERE generation_uid = ?1 AND manifest_node_path = ?2"
                    ),
                    params![generation_uid.to_string(), path],
                    map_recovery_row,
                )
                .optional()?
                .ok_or_else(|| RegistryError::NotFound {
                    what: format!("recovery node {generation_uid}/{path}"),
                })
                .and_then(finish_recovery_row)?;

            let scope = LeaseScope::Recovery(row.backend_instance);
            assert_lease_fence_on(tx, &scope, lease)?;
            require_published_epoch(tx, row.backend_instance, row.server_epoch)?;

            if row.node_state == to {
                if row.bootstrap_request_uid != bootstrap_request_uid {
                    return Err(recovery_error(format!(
                        "idempotent transition for {generation_uid}/{path} has a different bootstrap request"
                    )));
                }
                return Ok(row);
            }
            if row.node_state != expected {
                return Err(recovery_error(format!(
                    "recovery node {generation_uid}/{path} is {}, expected {}",
                    row.node_state.as_str(),
                    expected.as_str()
                )));
            }
            let allowed = if path == RECOVERY_GENERATION_PATH {
                if to == RecoveryNodeState::Aborted {
                    return Err(recovery_error(format!(
                        "generation {generation_uid} abort must use the atomic generation API"
                    )));
                }
                generation_can_transition(expected, to)
            } else {
                node_can_transition(expected, to)
            };
            if !allowed {
                return Err(recovery_error(format!(
                    "illegal {} transition {} -> {}",
                    if path == RECOVERY_GENERATION_PATH {
                        "generation"
                    } else {
                        "node"
                    },
                    expected.as_str(),
                    to.as_str()
                )));
            }

            let now = now_rfc3339();
            let changed = tx.execute(
                "UPDATE recovery_journal SET node_state = ?3, \
                   bootstrap_request_uid = COALESCE(?4, bootstrap_request_uid), updated_at = ?5 \
                 WHERE generation_uid = ?1 AND manifest_node_path = ?2 AND node_state = ?6",
                params![
                    generation_uid.to_string(),
                    path,
                    to.as_str(),
                    bootstrap_request_uid.map(|uid| uid.to_string()),
                    now,
                    expected.as_str()
                ],
            )?;
            if changed != 1 {
                return Err(recovery_error(format!(
                    "compare-and-set lost for recovery node {generation_uid}/{path}"
                )));
            }
            tx.query_row(
                &format!(
                    "SELECT {RECOVERY_COLUMNS} FROM recovery_journal \
                     WHERE generation_uid = ?1 AND manifest_node_path = ?2"
                ),
                params![generation_uid.to_string(), path],
                map_recovery_row,
            )
            .map_err(RegistryError::from)
            .and_then(finish_recovery_row)
        })
    }

    /// Atomically terminate one recovery generation after its caller has
    /// proved every native resource removed or absent. The root is a strict
    /// compare-and-set from `expected_root`; all nonterminal children become
    /// `aborted` in the same transaction, while completed/skipped children
    /// remain immutable proof history. An exact lost-ack replay of an already
    /// aborted, wholly-terminal generation returns its durable rows.
    ///
    /// Recovery abort is journal bookkeeping and never advances authority.
    pub fn abort_recovery_generation(
        &mut self,
        generation_uid: Uuid,
        expected_root: RecoveryNodeState,
        lease: &Lease,
    ) -> Result<Vec<RecoveryJournalRow>> {
        if !is_abortable(expected_root) {
            return Err(recovery_error(format!(
                "generation abort expected state {} is not abortable",
                expected_root.as_str()
            )));
        }

        self.immediate(|tx| {
            let rows = recovery_rows_on(tx, generation_uid)?;
            if rows.is_empty() {
                return Err(RegistryError::NotFound {
                    what: format!("recovery generation {generation_uid}"),
                });
            }
            let root = checked_generation_root(generation_uid, &rows)?;

            let scope = LeaseScope::Recovery(root.backend_instance);
            assert_lease_fence_on(tx, &scope, lease)?;
            require_published_epoch(tx, root.backend_instance, root.server_epoch)?;
            abort_generation_rows_on(tx, generation_uid, expected_root, &rows)
        })
    }

    /// Atomically record the current authority head as an intentional-empty
    /// floor and terminally abort the exact generation.  The current server
    /// epoch may differ from the journal epoch (for an explicit abort after a
    /// server restart), but the recovery fence and exclusive instance kernel
    /// lock must both name the generation's backend instance.  No authority
    /// revision is advanced.
    pub fn abort_recovery_generation_and_record_current_empty(
        &mut self,
        generation_uid: Uuid,
        expected_root: RecoveryNodeState,
        current_server_epoch: ServerEpoch,
        kernel: &HeldLock,
        lease: &Lease,
    ) -> Result<(u64, Vec<RecoveryJournalRow>)> {
        if !is_abortable(expected_root) {
            return Err(recovery_error(format!(
                "generation abort expected state {} is not abortable",
                expected_root.as_str()
            )));
        }
        if kernel.mode() != LockMode::Exclusive {
            return Err(RegistryError::KernelLockMismatch {
                scope: lease.scope.clone(),
            });
        }

        self.immediate(|tx| {
            let rows = recovery_rows_on(tx, generation_uid)?;
            if rows.is_empty() {
                return Err(RegistryError::NotFound {
                    what: format!("recovery generation {generation_uid}"),
                });
            }
            let root = checked_generation_root(generation_uid, &rows)?;
            if kernel.scope() != &LockScope::BackendInstance(root.backend_instance) {
                return Err(RegistryError::KernelLockMismatch {
                    scope: LeaseScope::Recovery(root.backend_instance).as_scope_string(),
                });
            }
            let scope = LeaseScope::Recovery(root.backend_instance);
            assert_lease_fence_on(tx, &scope, lease)?;
            require_published_epoch(tx, root.backend_instance, current_server_epoch)?;

            if root.node_state == RecoveryNodeState::Aborted {
                let floor: Option<i64> = tx.query_row(
                    "SELECT intentional_empty_revision FROM backend_instances \
                     WHERE backend_instance_uid = ?1",
                    [root.backend_instance.0.to_string()],
                    |row| row.get(0),
                )?;
                let floor = floor.ok_or_else(|| {
                    recovery_error(format!(
                        "aborted generation {generation_uid} has no intentional-empty floor"
                    ))
                })?;
                let floor = u64::try_from(floor).map_err(|_| {
                    recovery_error(format!("negative intentional-empty revision {floor}"))
                })?;
                return Ok((
                    floor,
                    abort_generation_rows_on(tx, generation_uid, expected_root, &rows)?,
                ));
            }

            let revision: i64 = tx.query_row(
                "SELECT authority_revision FROM meta WHERE id = 1",
                [],
                |row| row.get(0),
            )?;
            tx.execute(
                "UPDATE backend_instances \
                 SET intentional_empty_revision = \
                   CASE WHEN intentional_empty_revision IS NULL \
                          OR intentional_empty_revision < ?2 \
                        THEN ?2 ELSE intentional_empty_revision END \
                 WHERE backend_instance_uid = ?1",
                params![root.backend_instance.0.to_string(), revision],
            )?;
            let aborted = abort_generation_rows_on(tx, generation_uid, expected_root, &rows)?;
            let stored: i64 = tx.query_row(
                "SELECT intentional_empty_revision FROM backend_instances \
                 WHERE backend_instance_uid = ?1",
                [root.backend_instance.0.to_string()],
                |row| row.get(0),
            )?;
            let stored = u64::try_from(stored).map_err(|_| {
                recovery_error(format!("negative intentional-empty revision {stored}"))
            })?;
            Ok((stored, aborted))
        })
    }

    /// Terminally retire a generation from an older server epoch after the
    /// current owner has proved in-process that none of its native resources
    /// survived.  This intentionally does not move the empty floor: normal
    /// cold recovery may immediately start a fresh current-epoch generation
    /// from the same eligible manifest.
    pub fn abort_stale_recovery_generation(
        &mut self,
        generation_uid: Uuid,
        expected_root: RecoveryNodeState,
        current_server_epoch: ServerEpoch,
        lease: &Lease,
    ) -> Result<Vec<RecoveryJournalRow>> {
        if !is_abortable(expected_root) {
            return Err(recovery_error(format!(
                "generation abort expected state {} is not abortable",
                expected_root.as_str()
            )));
        }
        self.immediate(|tx| {
            let rows = recovery_rows_on(tx, generation_uid)?;
            if rows.is_empty() {
                return Err(RegistryError::NotFound {
                    what: format!("recovery generation {generation_uid}"),
                });
            }
            let root = checked_generation_root(generation_uid, &rows)?;
            let scope = LeaseScope::Recovery(root.backend_instance);
            assert_lease_fence_on(tx, &scope, lease)?;
            require_published_epoch(tx, root.backend_instance, current_server_epoch)?;
            if root.server_epoch == current_server_epoch {
                return Err(recovery_error(format!(
                    "generation {generation_uid} belongs to the current server epoch"
                )));
            }
            abort_generation_rows_on(tx, generation_uid, expected_root, &rows)
        })
    }
}
