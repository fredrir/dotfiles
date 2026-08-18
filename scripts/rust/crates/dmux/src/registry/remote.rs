//! Routes, single-use attach tokens, and peer snapshot cache — the P7
//! remote surface frozen in ADR 009 §3, over the v1 `routes`/`remote_cache`
//! tables and the v2 `attach_tokens` table.
//!
//! Revision policy (module docs in [`super`]): route topology and trust
//! material (`upsert_route`, `set_route_enabled`) are authority state and
//! advance the hash chain; route outcome records, attach-token issue/redeem,
//! and peer-cache snapshots are diagnostics/cache/ephemeral authorization
//! and do NOT advance it.
//!
//! Lineage *policy* (plan §12.1 conflict/stale/rollback rules) lives in
//! `remote/**`; this module only stores and returns the checkpoint —
//! [`Registry::classify_lineage`] remains available to it.

use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

use crate::model::{HostUid, RegistryUid, ServerEpoch, SpaceUid};

use super::{Registry, RegistryError, Result, advance_revision, now_rfc3339, parse_uuid};

// ---------------------------------------------------------------------------
// Routes (plan §12.3)

/// `routes.transport` (registry-v1.sql CHECK set).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Transport {
    Local,
    Openssh,
    WezSsh,
}

impl Transport {
    pub fn as_str(self) -> &'static str {
        match self {
            Transport::Local => "local",
            Transport::Openssh => "openssh",
            Transport::WezSsh => "wez-ssh",
        }
    }

    pub fn parse(token: &str) -> Option<Self> {
        Some(match token {
            "local" => Transport::Local,
            "openssh" => Transport::Openssh,
            "wez-ssh" => Transport::WezSsh,
            _ => return None,
        })
    }
}

/// `routes.network_class` (registry-v1.sql CHECK set).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NetworkClass {
    Usb,
    Tailscale,
    Lan,
    Other,
}

impl NetworkClass {
    pub fn as_str(self) -> &'static str {
        match self {
            NetworkClass::Usb => "usb",
            NetworkClass::Tailscale => "tailscale",
            NetworkClass::Lan => "lan",
            NetworkClass::Other => "other",
        }
    }

    pub fn parse(token: &str) -> Option<Self> {
        Some(match token {
            "usb" => NetworkClass::Usb,
            "tailscale" => NetworkClass::Tailscale,
            "lan" => NetworkClass::Lan,
            "other" => NetworkClass::Other,
            _ => return None,
        })
    }
}

/// Input to [`Registry::upsert_route`], keyed on
/// `(host_uid, transport, endpoint)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteSpec {
    pub host_uid: HostUid,
    pub transport: Transport,
    pub endpoint: String,
    pub username: Option<String>,
    pub wez_domain: Option<String>,
    pub network_class: NetworkClass,
    /// Lower tries first within eligibility.
    pub priority: i64,
    pub required_capability: Option<String>,
    pub trust_fingerprint: Option<String>,
    pub enabled: bool,
}

/// One `routes` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteRow {
    pub route_id: i64,
    pub host_uid: HostUid,
    pub transport: Transport,
    pub endpoint: String,
    pub username: Option<String>,
    pub wez_domain: Option<String>,
    pub network_class: NetworkClass,
    pub priority: i64,
    pub required_capability: Option<String>,
    pub trust_fingerprint: Option<String>,
    pub enabled: bool,
    /// Typed outcome token, diagnostics only.
    pub last_outcome: Option<String>,
    pub last_outcome_at: Option<String>,
}

const ROUTE_COLUMNS: &str = "route_id, host_uid, transport, endpoint, username, wez_domain, \
                             network_class, priority, required_capability, trust_fingerprint, \
                             enabled, last_outcome, last_outcome_at";

type RawRouteRow = (
    i64,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    i64,
    Option<String>,
    Option<String>,
    i64,
    Option<String>,
    Option<String>,
);

fn map_route_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawRouteRow> {
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
    ))
}

fn finish_route_row(raw: RawRouteRow) -> Result<RouteRow> {
    let (
        route_id,
        host,
        transport,
        endpoint,
        username,
        wez_domain,
        class,
        priority,
        capability,
        fingerprint,
        enabled,
        outcome,
        outcome_at,
    ) = raw;
    Ok(RouteRow {
        route_id,
        host_uid: HostUid(parse_uuid(&host)?),
        transport: Transport::parse(&transport)
            .ok_or_else(|| RegistryError::Corrupt(format!("transport {transport:?}")))?,
        endpoint,
        username,
        wez_domain,
        network_class: NetworkClass::parse(&class)
            .ok_or_else(|| RegistryError::Corrupt(format!("network_class {class:?}")))?,
        priority,
        required_capability: capability,
        trust_fingerprint: fingerprint,
        enabled: enabled != 0,
        last_outcome: outcome,
        last_outcome_at: outcome_at,
    })
}

// ---------------------------------------------------------------------------
// Attach tokens (plan §12.1, ADR 009 §3)

/// Input to [`Registry::issue_attach_token`]. The opaque token itself is
/// NEVER stored — the caller passes only its sha256 lowercase hex
/// ([`crate::registry::sha256::sha256_hex`]). Timestamps are RFC 3339 UTC
/// seconds precision ([`super::now_rfc3339`] format); expiry compares them
/// lexicographically, which for this fixed format is chronological order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachTokenSpec {
    /// sha256 lowercase hex of the single-use token.
    pub token_hash: String,
    pub request_uid: Uuid,
    pub host_uid: HostUid,
    pub space_uid: SpaceUid,
    pub server_epoch: ServerEpoch,
    /// Route the token is bound to; `_attach` refuses other routes.
    pub route: String,
    /// The exact owner-generated attach command, stored as JSON argv.
    pub attach_argv: Vec<String>,
    pub issued_at: String,
    pub expires_at: String,
}

/// The redeemed token's plan — everything `_attach` verifies and `exec`s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedeemedAttach {
    pub request_uid: Uuid,
    pub host_uid: HostUid,
    pub space_uid: SpaceUid,
    pub server_epoch: ServerEpoch,
    pub route: String,
    pub attach_argv: Vec<String>,
    pub issued_at: String,
    pub expires_at: String,
    pub redeemed_at: String,
}

/// Outcome of [`Registry::redeem_attach_token`]. Exactly one concurrent
/// redeemer of an issued, unexpired token gets [`AttachRedemption::Redeemed`];
/// every other case is typed and never deletes the row (audit journal).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachRedemption {
    Redeemed(RedeemedAttach),
    /// Already redeemed once — replay.
    Replayed,
    /// Past `expires_at` (the row is marked `expired`, kept for audit).
    Expired,
    /// Administratively revoked.
    Revoked,
    /// No such token hash was ever issued.
    Unknown,
}

// ---------------------------------------------------------------------------
// Peer snapshot cache (plan §12.1)

/// One `remote_cache` checkpoint: a read-only snapshot of a remote
/// authority plus the lineage head it was fetched at. Stored and returned
/// verbatim; conflict/stale/rollback policy stays in `remote/**`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerCache {
    pub registry_uid: RegistryUid,
    pub authority_revision: u64,
    pub authority_head_hash: String,
    pub snapshot_json: serde_json::Value,
    pub fetched_at: String,
}

impl Registry {
    // -- routes -------------------------------------------------------------

    /// Insert or update the route keyed on `(host_uid, transport,
    /// endpoint)`, returning its `route_id`. An update replaces every
    /// mutable field from the spec but preserves the diagnostic
    /// `last_outcome`/`last_outcome_at`. The host must be enrolled.
    /// Advances the authority revision when anything changed (route
    /// topology and trust material are authority state).
    pub fn upsert_route(&mut self, spec: &RouteSpec) -> Result<i64> {
        self.immediate(|tx| {
            let now = now_rfc3339();
            let uid = spec.host_uid.0.to_string();
            let enrolled: Option<i64> = tx
                .query_row(
                    "SELECT 1 FROM hosts WHERE host_uid = ?1 AND lifecycle = 'enrolled'",
                    [&uid],
                    |row| row.get(0),
                )
                .optional()?;
            if enrolled.is_none() {
                return Err(RegistryError::NotFound {
                    what: format!("enrolled host {}", spec.host_uid.0),
                });
            }
            let existing: Option<i64> = tx
                .query_row(
                    "SELECT route_id FROM routes \
                     WHERE host_uid = ?1 AND transport = ?2 AND endpoint = ?3",
                    params![uid, spec.transport.as_str(), spec.endpoint],
                    |row| row.get(0),
                )
                .optional()?;
            let route_id = match existing {
                Some(route_id) => {
                    let changed = tx.execute(
                        "UPDATE routes SET username = ?2, wez_domain = ?3, network_class = ?4, \
                         priority = ?5, required_capability = ?6, trust_fingerprint = ?7, \
                         enabled = ?8 \
                         WHERE route_id = ?1 \
                           AND (username IS NOT ?2 OR wez_domain IS NOT ?3 \
                             OR network_class IS NOT ?4 OR priority IS NOT ?5 \
                             OR required_capability IS NOT ?6 OR trust_fingerprint IS NOT ?7 \
                             OR enabled IS NOT ?8)",
                        params![
                            route_id,
                            spec.username,
                            spec.wez_domain,
                            spec.network_class.as_str(),
                            spec.priority,
                            spec.required_capability,
                            spec.trust_fingerprint,
                            spec.enabled as i64
                        ],
                    )?;
                    if changed == 0 {
                        return Ok(route_id); // identical spec: pure no-op
                    }
                    route_id
                }
                None => {
                    tx.execute(
                        "INSERT INTO routes (host_uid, transport, endpoint, username, \
                         wez_domain, network_class, priority, required_capability, \
                         trust_fingerprint, enabled) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                        params![
                            uid,
                            spec.transport.as_str(),
                            spec.endpoint,
                            spec.username,
                            spec.wez_domain,
                            spec.network_class.as_str(),
                            spec.priority,
                            spec.required_capability,
                            spec.trust_fingerprint,
                            spec.enabled as i64
                        ],
                    )?;
                    tx.last_insert_rowid()
                }
            };
            advance_revision(tx, &now)?;
            Ok(route_id)
        })
    }

    /// Every route to a host, priority-ordered (lower first, then
    /// `route_id` for determinism). Disabled routes are included with
    /// `enabled = false`; eligibility filtering stays caller-side.
    pub fn routes_for(&self, host_uid: HostUid) -> Result<Vec<RouteRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ROUTE_COLUMNS} FROM routes WHERE host_uid = ?1 \
             ORDER BY priority, route_id"
        ))?;
        let rows = stmt.query_map([host_uid.0.to_string()], map_route_row)?;
        let mut routes = Vec::new();
        for row in rows {
            routes.push(finish_route_row(row?)?);
        }
        Ok(routes)
    }

    /// Enable or disable one route. Advances the authority revision when
    /// the flag actually flips; setting the current value is a no-op.
    pub fn set_route_enabled(&mut self, route_id: i64, enabled: bool) -> Result<()> {
        self.immediate(|tx| {
            let now = now_rfc3339();
            let changed = tx.execute(
                "UPDATE routes SET enabled = ?2 WHERE route_id = ?1 AND enabled <> ?2",
                params![route_id, enabled as i64],
            )?;
            if changed == 0 {
                let exists: Option<i64> = tx
                    .query_row(
                        "SELECT 1 FROM routes WHERE route_id = ?1",
                        [route_id],
                        |row| row.get(0),
                    )
                    .optional()?;
                return match exists {
                    Some(_) => Ok(()), // already in the requested state
                    None => Err(RegistryError::NotFound {
                        what: format!("route {route_id}"),
                    }),
                };
            }
            advance_revision(tx, &now)?;
            Ok(())
        })
    }

    /// Record the last typed outcome token for a route (diagnostics only —
    /// never advances the authority revision).
    pub fn record_route_outcome(&mut self, route_id: i64, outcome_token: &str) -> Result<()> {
        let outcome = outcome_token.to_string();
        self.immediate(|tx| {
            let changed = tx.execute(
                "UPDATE routes SET last_outcome = ?2, last_outcome_at = ?3 \
                 WHERE route_id = ?1",
                params![route_id, outcome, now_rfc3339()],
            )?;
            if changed != 1 {
                return Err(RegistryError::NotFound {
                    what: format!("route {route_id}"),
                });
            }
            Ok(())
        })
    }

    // -- attach tokens ------------------------------------------------------

    /// Record an issued single-use attach token (hash only). Reusing a
    /// token hash or request UID is the typed
    /// [`RegistryError::AttachTokenExists`] — the RPC idempotency ledger,
    /// not re-issue, handles request replay. Ephemeral authorization state:
    /// does not advance the authority revision.
    pub fn issue_attach_token(&mut self, spec: &AttachTokenSpec) -> Result<()> {
        let argv = serde_json::to_string(&spec.attach_argv)
            .map_err(|e| RegistryError::Corrupt(format!("attach argv: {e}")))?;
        self.immediate(|tx| {
            tx.execute(
                "INSERT INTO attach_tokens (token_hash, request_uid, host_uid, space_uid, \
                 server_epoch, route, attach_argv, issued_at, expires_at, state) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'issued')",
                params![
                    spec.token_hash,
                    spec.request_uid.to_string(),
                    spec.host_uid.0.to_string(),
                    spec.space_uid.0.to_string(),
                    spec.server_epoch.0.to_string(),
                    spec.route,
                    argv,
                    spec.issued_at,
                    spec.expires_at
                ],
            )
            .map_err(|e| match e {
                rusqlite::Error::SqliteFailure(f, Some(ref message))
                    if f.code == rusqlite::ErrorCode::ConstraintViolation
                        && message.contains("attach_tokens") =>
                {
                    RegistryError::AttachTokenExists {
                        request_uid: spec.request_uid,
                    }
                }
                other => other.into(),
            })?;
            Ok(())
        })
    }

    /// Atomically redeem a token by hash: a guarded single-row UPDATE
    /// (`state = 'issued' AND expires_at > now`) then read-back, so two
    /// concurrent redeemers produce exactly one
    /// [`AttachRedemption::Redeemed`]. `now` is an RFC 3339 UTC
    /// seconds-precision timestamp ([`super::now_rfc3339`]). Expiry and
    /// replay never delete the row; an expired-but-issued row is marked
    /// `expired` for the audit journal. Does not advance the authority
    /// revision.
    pub fn redeem_attach_token(&mut self, token_hash: &str, now: &str) -> Result<AttachRedemption> {
        let token_hash = token_hash.to_string();
        let now = now.to_string();
        self.immediate(|tx| {
            let changed = tx.execute(
                "UPDATE attach_tokens SET state = 'redeemed', redeemed_at = ?2 \
                 WHERE token_hash = ?1 AND state = 'issued' AND expires_at > ?2",
                params![token_hash, now],
            )?;
            if changed == 1 {
                let raw = tx.query_row(
                    "SELECT request_uid, host_uid, space_uid, server_epoch, route, \
                     attach_argv, issued_at, expires_at, redeemed_at \
                     FROM attach_tokens WHERE token_hash = ?1",
                    [&token_hash],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, String>(7)?,
                            row.get::<_, String>(8)?,
                        ))
                    },
                )?;
                let (request, host, space, epoch, route, argv, issued, expires, redeemed) = raw;
                let attach_argv: Vec<String> = serde_json::from_str(&argv)
                    .map_err(|e| RegistryError::Corrupt(format!("attach argv: {e}")))?;
                return Ok(AttachRedemption::Redeemed(RedeemedAttach {
                    request_uid: parse_uuid(&request)?,
                    host_uid: HostUid(parse_uuid(&host)?),
                    space_uid: SpaceUid(parse_uuid(&space)?),
                    server_epoch: ServerEpoch(parse_uuid(&epoch)?),
                    route,
                    attach_argv,
                    issued_at: issued,
                    expires_at: expires,
                    redeemed_at: redeemed,
                }));
            }
            let state: Option<(String, String)> = tx
                .query_row(
                    "SELECT state, expires_at FROM attach_tokens WHERE token_hash = ?1",
                    [&token_hash],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            match state {
                None => Ok(AttachRedemption::Unknown),
                Some((state, expires_at)) => match state.as_str() {
                    "issued" if expires_at.as_str() <= now.as_str() => {
                        // Mark expired for the audit journal; keep the row.
                        tx.execute(
                            "UPDATE attach_tokens SET state = 'expired' \
                             WHERE token_hash = ?1 AND state = 'issued'",
                            [&token_hash],
                        )?;
                        Ok(AttachRedemption::Expired)
                    }
                    "issued" => Err(RegistryError::Corrupt(format!(
                        "unexpired issued token {token_hash} failed its guarded redeem"
                    ))),
                    "redeemed" => Ok(AttachRedemption::Replayed),
                    "expired" => Ok(AttachRedemption::Expired),
                    "revoked" => Ok(AttachRedemption::Revoked),
                    other => Err(RegistryError::Corrupt(format!("token state {other:?}"))),
                },
            }
        })
    }

    // -- peer snapshot cache ------------------------------------------------

    /// The cached checkpoint for a peer, if any.
    pub fn peer_cache(&self, host_uid: HostUid) -> Result<Option<PeerCache>> {
        self.conn
            .query_row(
                "SELECT registry_uid, authority_revision, authority_head_hash, \
                 snapshot_json, fetched_at FROM remote_cache WHERE host_uid = ?1",
                [host_uid.0.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()?
            .map(|(registry, revision, head, snapshot, fetched)| {
                Ok(PeerCache {
                    registry_uid: RegistryUid(parse_uuid(&registry)?),
                    // A negative revision is not a small revision: it is a
                    // row no in-range write could have produced. Reading it
                    // back through `as u64` would resurrect an enormous
                    // checkpoint and quarantine the host forever with no
                    // diagnostic; surfacing it names the exact bad row
                    // instead. Never treat it as "no checkpoint" — that
                    // would silently drop this peer's anti-rollback anchor.
                    authority_revision: u64::try_from(revision).map_err(|_| {
                        RegistryError::Corrupt(format!(
                            "cached peer authority_revision {revision} is negative"
                        ))
                    })?,
                    authority_head_hash: head,
                    snapshot_json: serde_json::from_str(&snapshot)
                        .map_err(|e| RegistryError::Corrupt(format!("cached snapshot: {e}")))?,
                    fetched_at: fetched,
                })
            })
            .transpose()
    }

    /// Store (replace) the checkpoint for a peer, verbatim. Whether the
    /// checkpoint SHOULD replace the cached one — conflict, stale, rollback
    /// (plan §12.1) — is the caller's policy, decided before this call.
    /// Cache state: does not advance the authority revision. A revision
    /// outside the storable range is refused, never narrowed.
    pub fn store_peer_cache(&mut self, host_uid: HostUid, cache: &PeerCache) -> Result<()> {
        // Total, not narrowing: the checkpoint is the durable anti-rollback
        // anchor, and `as i64` would silently store a peer-supplied
        // revision >= 2^63 as a negative one. The wire bound
        // (`remote::protocol::MAX_JSON_INTEGER`) keeps such a value off the
        // peer surface; this refuses it for every other caller too.
        let revision = i64::try_from(cache.authority_revision).map_err(|_| {
            RegistryError::Corrupt(format!(
                "peer authority_revision {} exceeds the storable range",
                cache.authority_revision
            ))
        })?;
        let snapshot = cache.snapshot_json.to_string();
        let cache = cache.clone();
        self.immediate(|tx| {
            tx.execute(
                "INSERT INTO remote_cache (host_uid, registry_uid, authority_revision, \
                 authority_head_hash, snapshot_json, fetched_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
                 ON CONFLICT(host_uid) DO UPDATE SET registry_uid = excluded.registry_uid, \
                   authority_revision = excluded.authority_revision, \
                   authority_head_hash = excluded.authority_head_hash, \
                   snapshot_json = excluded.snapshot_json, \
                   fetched_at = excluded.fetched_at",
                params![
                    host_uid.0.to_string(),
                    cache.registry_uid.0.to_string(),
                    revision,
                    cache.authority_head_hash,
                    snapshot,
                    cache.fetched_at
                ],
            )?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::RegistryConfig;

    use crate::remote::protocol::POISON_REVISION;

    fn scratch_registry() -> (tempfile::TempDir, Registry) {
        let dir = tempfile::tempdir().unwrap();
        let registry = Registry::open(RegistryConfig::new(
            dir.path().join("registry.sqlite3"),
            dir.path().join("locks"),
        ))
        .unwrap();
        (dir, registry)
    }

    fn checkpoint(revision: u64) -> PeerCache {
        PeerCache {
            registry_uid: RegistryUid(Uuid::nil()),
            authority_revision: revision,
            authority_head_hash: "sha256:abc".into(),
            snapshot_json: serde_json::json!({ "spaces": [] }),
            fetched_at: now_rfc3339(),
        }
    }

    #[test]
    fn store_peer_cache_refuses_a_revision_outside_the_storable_range() {
        let (_dir, mut registry) = scratch_registry();
        let host = HostUid(Uuid::new_v4());
        registry.enroll_host(host, None).unwrap();
        for absurd in [POISON_REVISION, u64::MAX] {
            let error = registry
                .store_peer_cache(host, &checkpoint(absurd))
                .expect_err("an unstorable revision must not be narrowed and committed");
            assert!(matches!(error, RegistryError::Corrupt(_)), "{error}");
            assert!(error.to_string().contains(&absurd.to_string()), "{error}");
        }
        // Nothing was written, so no poisoned anti-rollback anchor exists
        // for a later honest handshake to be measured against.
        assert_eq!(registry.peer_cache(host).unwrap(), None);
    }

    #[test]
    fn store_peer_cache_round_trips_a_large_but_legal_revision() {
        let (_dir, mut registry) = scratch_registry();
        let host = HostUid(Uuid::new_v4());
        registry.enroll_host(host, None).unwrap();
        // The largest revision the wire admits (protocol::MAX_JSON_INTEGER)
        // is stored and returned verbatim, unchanged by the fix.
        let large = checkpoint(crate::remote::protocol::MAX_JSON_INTEGER);
        registry.store_peer_cache(host, &large).unwrap();
        assert_eq!(registry.peer_cache(host).unwrap(), Some(large));
        let ordinary = checkpoint(7);
        registry.store_peer_cache(host, &ordinary).unwrap();
        assert_eq!(registry.peer_cache(host).unwrap(), Some(ordinary));
    }

    #[test]
    fn peer_cache_read_refuses_a_negative_revision_already_on_disk() {
        let (_dir, mut registry) = scratch_registry();
        let host = HostUid(Uuid::new_v4());
        registry.enroll_host(host, None).unwrap();
        registry.store_peer_cache(host, &checkpoint(9)).unwrap();
        // Exactly the row a registry poisoned before this fix holds. Schema
        // v4 added `CHECK (authority_revision >= 0)`, so writing one now
        // takes suspending check enforcement for this connection — which is
        // the point of the read-path defence this test covers: rows written
        // before the constraint existed still have to be refused on read.
        registry
            .conn
            .pragma_update(None, "ignore_check_constraints", "ON")
            .unwrap();
        registry
            .conn
            .execute(
                "UPDATE remote_cache SET authority_revision = ?1 WHERE host_uid = ?2",
                params![i64::MIN, host.0.to_string()],
            )
            .unwrap();
        registry
            .conn
            .pragma_update(None, "ignore_check_constraints", "OFF")
            .unwrap();
        let error = registry
            .peer_cache(host)
            .expect_err("a negative checkpoint must not read back as an enormous revision");
        assert!(matches!(error, RegistryError::Corrupt(_)), "{error}");
    }

    #[test]
    fn transport_and_network_class_tokens_match_the_contract() {
        for t in [Transport::Local, Transport::Openssh, Transport::WezSsh] {
            assert_eq!(Transport::parse(t.as_str()), Some(t));
        }
        assert_eq!(Transport::WezSsh.as_str(), "wez-ssh");
        assert_eq!(Transport::parse("ssh"), None);
        for c in [
            NetworkClass::Usb,
            NetworkClass::Tailscale,
            NetworkClass::Lan,
            NetworkClass::Other,
        ] {
            assert_eq!(NetworkClass::parse(c.as_str()), Some(c));
        }
        assert_eq!(NetworkClass::parse("wifi"), None);
    }
}
