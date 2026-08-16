//! Host enrollment, aliases, and labels over the v1 `hosts`/`host_refs`
//! tables — the P7 identity surface frozen in ADR 009 §3.
//!
//! Invariants carried here:
//! - `a` always means the local authority; it is minted at registry open and
//!   can never be forgotten.
//! - Aliases are bijective base-26 in enrollment order and are NEVER reused,
//!   even after tombstoning: allocation continues past the highest index ever
//!   allocated.
//! - A spelling (alias or label), once used, never binds to a different
//!   HostUid. `host_refs` rows are only ever transitioned
//!   current -> historical/tombstoned (and, for the same HostUid only,
//!   back to current on re-enrollment/relabel); they are never deleted and
//!   never repointed.
//! - Enrollment, forgetting, and labeling are authority mutations and
//!   advance the hash-chained authority revision; pure no-ops do not.

use rusqlite::{OptionalExtension, params};

use crate::model::HostUid;
use crate::refs::{alias_to_index, index_to_alias, is_valid_host_label};

use super::{Registry, RegistryError, Result, advance_revision, now_rfc3339, parse_uuid};

/// `hosts.lifecycle` (registry-v1.sql CHECK set).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostLifecycle {
    Enrolled,
    Tombstoned,
}

impl HostLifecycle {
    pub fn as_str(self) -> &'static str {
        match self {
            HostLifecycle::Enrolled => "enrolled",
            HostLifecycle::Tombstoned => "tombstoned",
        }
    }

    pub fn parse(token: &str) -> Option<Self> {
        Some(match token {
            "enrolled" => HostLifecycle::Enrolled,
            "tombstoned" => HostLifecycle::Tombstoned,
            _ => return None,
        })
    }
}

/// Result of [`Registry::enroll_host`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrolledHost {
    pub host_uid: HostUid,
    /// The current alias — freshly allocated for a new HostUid, otherwise
    /// the permanent alias this HostUid has always had.
    pub alias: String,
    /// The current label after this call, if any.
    pub label: Option<String>,
    /// True only on first-ever enrollment of this HostUid.
    pub newly_enrolled: bool,
    /// True when a tombstoned HostUid was reactivated (plan §12.2:
    /// re-enrollment is the only normal way back).
    pub reactivated: bool,
}

/// One `hosts` row joined with its current alias/label refs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRow {
    pub host_uid: HostUid,
    /// Current alias; `None` while tombstoned (the spelling stays reserved
    /// for this HostUid forever and returns on re-enrollment).
    pub alias: Option<String>,
    pub label: Option<String>,
    pub lifecycle: HostLifecycle,
    pub enrolled_at: String,
    pub tombstoned_at: Option<String>,
}

const HOST_COLUMNS: &str = "h.host_uid, h.lifecycle, h.enrolled_at, h.tombstoned_at, \
                            a.spelling, l.spelling";
const HOST_JOINS: &str = "FROM hosts h \
     LEFT JOIN host_refs a ON a.host_uid = h.host_uid \
       AND a.ref_kind = 'alias' AND a.state = 'current' \
     LEFT JOIN host_refs l ON l.host_uid = h.host_uid \
       AND l.ref_kind = 'label' AND l.state = 'current'";

type RawHostRow = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn map_host_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawHostRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
    ))
}

fn finish_host_row(raw: RawHostRow) -> Result<HostRow> {
    let (host, lifecycle, enrolled, tombstoned, alias, label) = raw;
    Ok(HostRow {
        host_uid: HostUid(parse_uuid(&host)?),
        alias,
        label,
        lifecycle: HostLifecycle::parse(&lifecycle)
            .ok_or_else(|| RegistryError::Corrupt(format!("host lifecycle {lifecycle:?}")))?,
        enrolled_at: enrolled,
        tombstoned_at: tombstoned,
    })
}

impl Registry {
    /// Enroll a peer authority (or re-affirm one), idempotent by HostUid
    /// (plan §12.2 steps 3–4).
    ///
    /// - A NEW HostUid gets a `hosts` row and the next bijective base-26
    ///   alias in sequence (continuing past the highest index ever
    ///   allocated; spellings already bound to another HostUid — e.g. as a
    ///   label — are skipped, never rebound).
    /// - A tombstoned HostUid is reactivated: lifecycle back to `enrolled`,
    ///   `enrolled_at` refreshed, and its permanent alias transitioned back
    ///   to `current`. Its routes stay disabled and its labels stay
    ///   tombstoned; the caller re-records what it wants.
    /// - An already-enrolled HostUid with no label change is a pure no-op
    ///   and does not advance the authority revision.
    ///
    /// `label`, when given, is bound exactly like
    /// [`Registry::set_host_label`].
    pub fn enroll_host(&mut self, host_uid: HostUid, label: Option<&str>) -> Result<EnrolledHost> {
        if let Some(label) = label
            && !is_valid_host_label(label)
        {
            return Err(RegistryError::InvalidLabel {
                label: label.to_string(),
            });
        }
        self.immediate(|tx| {
            let now = now_rfc3339();
            let uid = host_uid.0.to_string();
            let lifecycle: Option<String> = tx
                .query_row(
                    "SELECT lifecycle FROM hosts WHERE host_uid = ?1",
                    [&uid],
                    |row| row.get(0),
                )
                .optional()?;
            let mut mutated = false;
            let mut newly_enrolled = false;
            let mut reactivated = false;
            let alias = match lifecycle.as_deref() {
                None => {
                    let alias = next_free_alias(tx, host_uid)?;
                    tx.execute(
                        "INSERT INTO hosts (host_uid, lifecycle, enrolled_at) \
                         VALUES (?1, 'enrolled', ?2)",
                        params![uid, now],
                    )?;
                    tx.execute(
                        "INSERT INTO host_refs (ref_kind, spelling, host_uid, state, \
                         created_at, changed_at) VALUES ('alias', ?1, ?2, 'current', ?3, ?3)",
                        params![alias, uid, now],
                    )?;
                    mutated = true;
                    newly_enrolled = true;
                    alias
                }
                Some("tombstoned") => {
                    tx.execute(
                        "UPDATE hosts SET lifecycle = 'enrolled', enrolled_at = ?2, \
                         tombstoned_at = NULL WHERE host_uid = ?1",
                        params![uid, now],
                    )?;
                    // The permanent alias returns to `current` — same
                    // HostUid, so the never-rebound guarantee holds.
                    tx.execute(
                        "UPDATE host_refs SET state = 'current', changed_at = ?2 \
                         WHERE host_uid = ?1 AND ref_kind = 'alias' AND state = 'tombstoned'",
                        params![uid, now],
                    )?;
                    mutated = true;
                    reactivated = true;
                    current_alias(tx, host_uid)?.ok_or_else(|| {
                        RegistryError::Corrupt(format!("host {} has no alias ref", host_uid.0))
                    })?
                }
                Some("enrolled") => current_alias(tx, host_uid)?.ok_or_else(|| {
                    RegistryError::Corrupt(format!("host {} has no current alias", host_uid.0))
                })?,
                Some(other) => {
                    return Err(RegistryError::Corrupt(format!("host lifecycle {other:?}")));
                }
            };
            if let Some(label) = label {
                mutated |= bind_label(tx, host_uid, label, &now)?;
            }
            if mutated {
                advance_revision(tx, &now)?;
            }
            Ok(EnrolledHost {
                host_uid,
                alias,
                label: current_label(tx, host_uid)?,
                newly_enrolled,
                reactivated,
            })
        })
    }

    /// Every host (self and peers, enrolled and tombstoned) with its
    /// current alias/label, in enrollment order.
    pub fn hosts(&self) -> Result<Vec<HostRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {HOST_COLUMNS} {HOST_JOINS} ORDER BY h.enrolled_at, h.host_uid"
        ))?;
        let rows = stmt.query_map([], map_host_row)?;
        let mut hosts = Vec::new();
        for row in rows {
            hosts.push(finish_host_row(row?)?);
        }
        Ok(hosts)
    }

    /// Resolve a CURRENT alias spelling. Tombstoned hosts have no current
    /// alias, so a forgotten host does not resolve.
    pub fn host_by_alias(&self, alias: &str) -> Result<Option<HostRow>> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {HOST_COLUMNS} {HOST_JOINS} \
                     WHERE a.spelling = ?1"
                ),
                [alias],
                map_host_row,
            )
            .optional()?
            .map(finish_host_row)
            .transpose()
    }

    /// Bind a friendly label to an enrolled host. A spelling once used
    /// (as label OR alias) is permanently bound to its first HostUid;
    /// rebinding it to a different host is the typed
    /// [`RegistryError::SpellingBound`] (`identity_conflict`). The host's
    /// previous current label transitions to `historical` (never deleted);
    /// relabeling back to one of this host's own historical/tombstoned
    /// spellings reactivates that row.
    pub fn set_host_label(&mut self, host_uid: HostUid, label: &str) -> Result<()> {
        if !is_valid_host_label(label) {
            return Err(RegistryError::InvalidLabel {
                label: label.to_string(),
            });
        }
        let label = label.to_string();
        self.immediate(|tx| {
            let now = now_rfc3339();
            let enrolled: Option<String> = tx
                .query_row(
                    "SELECT lifecycle FROM hosts WHERE host_uid = ?1 AND lifecycle = 'enrolled'",
                    [host_uid.0.to_string()],
                    |row| row.get(0),
                )
                .optional()?;
            if enrolled.is_none() {
                return Err(RegistryError::NotFound {
                    what: format!("enrolled host {}", host_uid.0),
                });
            }
            if bind_label(tx, host_uid, &label, &now)? {
                advance_revision(tx, &now)?;
            }
            Ok(())
        })
    }

    /// Forget a peer authority (plan §12.2): refuses the local host
    /// (`a` can never be forgotten), tombstones the `hosts` row and its
    /// current refs, disables all its routes, and retains everything else —
    /// route rows, cached snapshots, Space/tombstone history. Idempotent:
    /// forgetting an already-tombstoned host is a no-op that does not
    /// advance the authority revision.
    pub fn forget_host(&mut self, host_uid: HostUid) -> Result<()> {
        self.immediate(|tx| {
            let now = now_rfc3339();
            let local: String =
                tx.query_row("SELECT host_uid FROM meta WHERE id = 1", [], |row| {
                    row.get(0)
                })?;
            if parse_uuid(&local)? == host_uid.0 {
                return Err(RegistryError::LocalHostImmutable { host_uid });
            }
            let lifecycle: Option<String> = tx
                .query_row(
                    "SELECT lifecycle FROM hosts WHERE host_uid = ?1",
                    [host_uid.0.to_string()],
                    |row| row.get(0),
                )
                .optional()?;
            match lifecycle.as_deref() {
                None => {
                    return Err(RegistryError::NotFound {
                        what: format!("host {}", host_uid.0),
                    });
                }
                Some("tombstoned") => return Ok(()), // idempotent no-op
                Some(_) => {}
            }
            tx.execute(
                "UPDATE hosts SET lifecycle = 'tombstoned', tombstoned_at = ?2 \
                 WHERE host_uid = ?1",
                params![host_uid.0.to_string(), now],
            )?;
            tx.execute(
                "UPDATE host_refs SET state = 'tombstoned', changed_at = ?2 \
                 WHERE host_uid = ?1 AND state = 'current'",
                params![host_uid.0.to_string(), now],
            )?;
            tx.execute(
                "UPDATE routes SET enabled = 0 WHERE host_uid = ?1",
                [host_uid.0.to_string()],
            )?;
            advance_revision(tx, &now)?;
            Ok(())
        })
    }
}

/// The next never-used alias: one past the highest alias index ever
/// allocated (never reused, even after tombstoning), skipping any spelling
/// already bound to a different HostUid as a label.
fn next_free_alias(tx: &rusqlite::Connection, host_uid: HostUid) -> Result<String> {
    let mut stmt = tx.prepare("SELECT spelling FROM host_refs WHERE ref_kind = 'alias'")?;
    let mut max_index = 0u64;
    for spelling in stmt.query_map([], |row| row.get::<_, String>(0))? {
        let spelling = spelling?;
        let index = alias_to_index(&spelling)
            .ok_or_else(|| RegistryError::Corrupt(format!("alias spelling {spelling:?}")))?;
        max_index = max_index.max(index);
    }
    let mut index = max_index + 1;
    loop {
        let alias =
            index_to_alias(index).ok_or_else(|| RegistryError::Corrupt("alias index 0".into()))?;
        match spelling_owner(tx, &alias)? {
            Some(owner) if owner != host_uid => index += 1, // bound elsewhere: skip forever
            _ => return Ok(alias),
        }
    }
}

/// The HostUid a spelling is permanently bound to (any ref kind, any
/// state), if it was ever used.
fn spelling_owner(tx: &rusqlite::Connection, spelling: &str) -> Result<Option<HostUid>> {
    let owner: Option<String> = tx
        .query_row(
            "SELECT host_uid FROM host_refs WHERE spelling = ?1 LIMIT 1",
            [spelling],
            |row| row.get(0),
        )
        .optional()?;
    owner.map(|uid| Ok(HostUid(parse_uuid(&uid)?))).transpose()
}

fn current_alias(tx: &rusqlite::Connection, host_uid: HostUid) -> Result<Option<String>> {
    Ok(tx
        .query_row(
            "SELECT spelling FROM host_refs \
             WHERE host_uid = ?1 AND ref_kind = 'alias' AND state = 'current'",
            [host_uid.0.to_string()],
            |row| row.get(0),
        )
        .optional()?)
}

fn current_label(tx: &rusqlite::Connection, host_uid: HostUid) -> Result<Option<String>> {
    Ok(tx
        .query_row(
            "SELECT spelling FROM host_refs \
             WHERE host_uid = ?1 AND ref_kind = 'label' AND state = 'current'",
            [host_uid.0.to_string()],
            |row| row.get(0),
        )
        .optional()?)
}

/// Bind `label` as the current label of `host_uid` inside an open
/// transaction. Returns true when anything changed (false: the label was
/// already current — a pure no-op).
fn bind_label(
    tx: &rusqlite::Connection,
    host_uid: HostUid,
    label: &str,
    now: &str,
) -> Result<bool> {
    let existing: Option<(String, String, String)> = tx
        .query_row(
            "SELECT host_uid, ref_kind, state FROM host_refs \
             WHERE ref_kind = 'label' AND spelling = ?1",
            [label],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    // The spelling may also exist as an ALIAS ref; a spelling never binds
    // to a different HostUid across kinds either.
    if let Some(owner) = spelling_owner(tx, label)?
        && owner != host_uid
    {
        return Err(RegistryError::SpellingBound {
            spelling: label.to_string(),
            bound_to: owner,
        });
    }
    match existing {
        Some((_, _, state)) if state == "current" => Ok(false),
        Some(_) => {
            // This host's own historical/tombstoned spelling returns.
            demote_current_label(tx, host_uid, now)?;
            tx.execute(
                "UPDATE host_refs SET state = 'current', changed_at = ?3 \
                 WHERE ref_kind = 'label' AND spelling = ?1 AND host_uid = ?2",
                params![label, host_uid.0.to_string(), now],
            )?;
            Ok(true)
        }
        None => {
            demote_current_label(tx, host_uid, now)?;
            tx.execute(
                "INSERT INTO host_refs (ref_kind, spelling, host_uid, state, \
                 created_at, changed_at) VALUES ('label', ?1, ?2, 'current', ?3, ?3)",
                params![label, host_uid.0.to_string(), now],
            )?;
            Ok(true)
        }
    }
}

fn demote_current_label(tx: &rusqlite::Connection, host_uid: HostUid, now: &str) -> Result<()> {
    tx.execute(
        "UPDATE host_refs SET state = 'historical', changed_at = ?2 \
         WHERE host_uid = ?1 AND ref_kind = 'label' AND state = 'current'",
        params![host_uid.0.to_string(), now],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_lifecycle_tokens_round_trip() {
        for l in [HostLifecycle::Enrolled, HostLifecycle::Tombstoned] {
            assert_eq!(HostLifecycle::parse(l.as_str()), Some(l));
        }
        assert_eq!(HostLifecycle::parse("gone"), None);
    }
}
