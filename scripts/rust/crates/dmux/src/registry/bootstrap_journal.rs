//! Bootstrap journal: the `bootstrap_requests` persistence behind the frozen
//! [`BootstrapJournal`] seam (plan §11.1, ADR 004; `registry-v1.sql`).
//!
//! State machine (VALIDATED here, mirroring the operations journal style):
//!
//! ```text
//! issued -> spawned -> correlated -> acked -> completed
//! any non-terminal (issued/spawned/correlated/acked)
//!     -> timeout | orphaned | conflict | aborted
//! terminal states (completed/timeout/orphaned/conflict/aborted): immutable
//! ```
//!
//! No skipping forward, no moving backward, and a self-loop is not a
//! transition. Anything else is
//! [`RegistryError::InvalidBootstrapTransition`], surfaced through the seam
//! as a `TypedError` with code `operation_failed`.
//!
//! Revision-chain policy (module docs in [`super`]): bootstrap rows are
//! journal state bookkeeping — like operation-state transitions they never
//! advance the authority revision chain. The identity mutations that
//! surround a bootstrap (space reservation, binding, epoch publication)
//! advance it through their own entry points.
//!
//! Reissuing an already-journaled `request_uid` (two brokers claiming one
//! request identity) is the `bootstrap_requests` primary key and surfaces
//! as [`RegistryError::BootstrapRequestExists`] (`identity_conflict`).

use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use crate::bootstrap::{BootstrapJournal, BootstrapState, IssuedRequest};
use crate::error::TypedError;
use crate::model::{BackendInstanceUid, ServerEpoch, SpaceUid};

use super::{Registry, RegistryError, Result, now_rfc3339, parse_uuid};

// ---------------------------------------------------------------------------
// State machine

/// `completed`/`timeout`/`orphaned`/`conflict`/`aborted` never change again.
pub fn bootstrap_is_terminal(state: BootstrapState) -> bool {
    use BootstrapState::*;
    matches!(state, Completed | Timeout | Orphaned | Conflict | Aborted)
}

/// The legal transition matrix (see the module docs).
pub fn bootstrap_can_transition(from: BootstrapState, to: BootstrapState) -> bool {
    use BootstrapState::*;
    if bootstrap_is_terminal(from) {
        return false;
    }
    matches!(
        (from, to),
        (Issued, Spawned)
            | (Spawned, Correlated)
            | (Correlated, Acked)
            | (Acked, Completed)
            | (_, Timeout | Orphaned | Conflict | Aborted)
    )
}

/// Inverse of [`BootstrapState::as_str`] (the frozen seam offers no parse).
pub fn parse_bootstrap_state(token: &str) -> Option<BootstrapState> {
    use BootstrapState::*;
    Some(match token {
        "issued" => Issued,
        "spawned" => Spawned,
        "correlated" => Correlated,
        "acked" => Acked,
        "completed" => Completed,
        "timeout" => Timeout,
        "orphaned" => Orphaned,
        "conflict" => Conflict,
        "aborted" => Aborted,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Row type

/// One `bootstrap_requests` row, every column — root takeover logic reads
/// the full record (plan §11.1: the journal's opaque Space/parent and
/// manifest-node path locate conforming orphans).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapRequestRow {
    pub request_uid: Uuid,
    pub operation_uid: Option<Uuid>,
    pub space_uid: Option<SpaceUid>,
    pub backend_instance: BackendInstanceUid,
    pub server_epoch: ServerEpoch,
    pub intended_parent: Option<String>,
    pub recovery_generation: Option<String>,
    pub manifest_node_path: Option<String>,
    /// JSON: raw spawn-return values (ADR 004 formats).
    pub returned_native_ids: Option<String>,
    pub final_group_ref: Option<String>,
    pub final_split_ref: Option<String>,
    pub state: BootstrapState,
    pub created_at: String,
    pub updated_at: String,
}

const BOOTSTRAP_COLUMNS: &str = "request_uid, operation_uid, space_uid, backend_instance_id, \
                                 server_epoch, intended_parent, recovery_generation, \
                                 manifest_node_path, returned_native_ids, final_group_ref, \
                                 final_split_ref, state, created_at, updated_at";

type RawBootstrapRow = (
    String,
    Option<String>,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    String,
    String,
);

fn map_bootstrap_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawBootstrapRow> {
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
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
        row.get(13)?,
    ))
}

fn finish_bootstrap_row(raw: RawBootstrapRow) -> Result<BootstrapRequestRow> {
    let (
        request,
        operation,
        space,
        instance,
        epoch,
        parent,
        generation,
        node_path,
        returned,
        group_ref,
        split_ref,
        state,
        created,
        updated,
    ) = raw;
    Ok(BootstrapRequestRow {
        request_uid: parse_uuid(&request)?,
        operation_uid: operation.as_deref().map(parse_uuid).transpose()?,
        space_uid: space
            .as_deref()
            .map(|s| parse_uuid(s).map(SpaceUid))
            .transpose()?,
        backend_instance: BackendInstanceUid(parse_uuid(&instance)?),
        server_epoch: ServerEpoch(parse_uuid(&epoch)?),
        intended_parent: parent,
        recovery_generation: generation,
        manifest_node_path: node_path,
        returned_native_ids: returned,
        final_group_ref: group_ref,
        final_split_ref: split_ref,
        state: parse_bootstrap_state(&state)
            .ok_or_else(|| RegistryError::Corrupt(format!("bootstrap state {state:?}")))?,
        created_at: created,
        updated_at: updated,
    })
}

// ---------------------------------------------------------------------------
// Internal helpers

fn bootstrap_state_of(tx: &Connection, uid: Uuid) -> Result<BootstrapState> {
    let token: Option<String> = tx
        .query_row(
            "SELECT state FROM bootstrap_requests WHERE request_uid = ?1",
            [uid.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    let token = token.ok_or_else(|| RegistryError::NotFound {
        what: format!("bootstrap request {uid}"),
    })?;
    parse_bootstrap_state(&token)
        .ok_or_else(|| RegistryError::Corrupt(format!("bootstrap state {token:?}")))
}

fn require_bootstrap_transition(tx: &Connection, uid: Uuid, to: BootstrapState) -> Result<()> {
    let from = bootstrap_state_of(tx, uid)?;
    if !bootstrap_can_transition(from, to) {
        return Err(RegistryError::InvalidBootstrapTransition { from, to });
    }
    Ok(())
}

fn is_request_uid_conflict(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(f, Some(msg))
            if f.code == rusqlite::ErrorCode::ConstraintViolation
                && msg.contains("bootstrap_requests.request_uid")
    )
}

// ---------------------------------------------------------------------------
// Query API (root takeover reads)

impl Registry {
    /// The complete journal row for `uid`, or `None` when never issued.
    pub fn bootstrap_request(&self, uid: Uuid) -> Result<Option<BootstrapRequestRow>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {BOOTSTRAP_COLUMNS} FROM bootstrap_requests WHERE request_uid = ?1"
                ),
                [uid.to_string()],
                map_bootstrap_row,
            )
            .optional()?
            .map(finish_bootstrap_row)
            .transpose()
    }
}

// ---------------------------------------------------------------------------
// The frozen seam

impl BootstrapJournal for Registry {
    /// Journal the request as `issued` BEFORE any native spawn, persisting
    /// every [`IssuedRequest`] field.
    fn bootstrap_issue(&mut self, request: &IssuedRequest) -> std::result::Result<(), TypedError> {
        self.immediate(|tx| {
            let now = now_rfc3339();
            tx.execute(
                "INSERT INTO bootstrap_requests \
                 (request_uid, operation_uid, space_uid, backend_instance_id, server_epoch, \
                  intended_parent, recovery_generation, manifest_node_path, state, \
                  created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'issued', ?9, ?9)",
                params![
                    request.request_uid.to_string(),
                    request.operation_uid.map(|u| u.to_string()),
                    request.space_uid.map(|s| s.0.to_string()),
                    request.backend_instance.0.to_string(),
                    request.server_epoch.0.to_string(),
                    request.intended_parent,
                    request.recovery_generation,
                    request.manifest_node_path,
                    now
                ],
            )
            .map_err(|e| {
                if is_request_uid_conflict(&e) {
                    RegistryError::BootstrapRequestExists {
                        request_uid: request.request_uid,
                    }
                } else {
                    RegistryError::from(e)
                }
            })?;
            Ok(())
        })
        .map_err(TypedError::from)
    }

    /// Record the raw spawn-return values; `issued -> spawned`.
    fn bootstrap_spawned(
        &mut self,
        uid: Uuid,
        returned_native_ids: &str,
    ) -> std::result::Result<(), TypedError> {
        self.immediate(|tx| {
            let now = now_rfc3339();
            require_bootstrap_transition(tx, uid, BootstrapState::Spawned)?;
            tx.execute(
                "UPDATE bootstrap_requests SET returned_native_ids = ?2, state = 'spawned', \
                 updated_at = ?3 WHERE request_uid = ?1",
                params![uid.to_string(), returned_native_ids, now],
            )?;
            Ok(())
        })
        .map_err(TypedError::from)
    }

    /// Record the derived exact refs; `spawned -> correlated`.
    fn bootstrap_correlated(
        &mut self,
        uid: Uuid,
        group_ref: &str,
        split_ref: &str,
    ) -> std::result::Result<(), TypedError> {
        self.immediate(|tx| {
            let now = now_rfc3339();
            require_bootstrap_transition(tx, uid, BootstrapState::Correlated)?;
            tx.execute(
                "UPDATE bootstrap_requests SET final_group_ref = ?2, final_split_ref = ?3, \
                 state = 'correlated', updated_at = ?4 WHERE request_uid = ?1",
                params![uid.to_string(), group_ref, split_ref, now],
            )?;
            Ok(())
        })
        .map_err(TypedError::from)
    }

    /// Any other VALIDATED transition (ack/complete and the failure exits).
    /// This generic entry point records no payload; spawn-return values and
    /// final refs go through their dedicated methods.
    fn bootstrap_state(
        &mut self,
        uid: Uuid,
        state: BootstrapState,
    ) -> std::result::Result<(), TypedError> {
        self.immediate(|tx| {
            let now = now_rfc3339();
            require_bootstrap_transition(tx, uid, state)?;
            tx.execute(
                "UPDATE bootstrap_requests SET state = ?2, updated_at = ?3 WHERE request_uid = ?1",
                params![uid.to_string(), state.as_str(), now],
            )?;
            Ok(())
        })
        .map_err(TypedError::from)
    }
}
