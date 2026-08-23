//! P2 registry gate tests (plan §18 P2 row, §20.1): concurrent first-run,
//! allocation uniqueness/monotonicity, tombstone non-reuse, journal crash
//! points, idempotency, SQLITE_BUSY typing, WAL-safe online backup, the
//! authority revision chain, and fenced leases. The P5 identity slice adds
//! the bootstrap journal and server-epoch publication in [`bootstrap`];
//! the P6 adoption surface (kind-explicit reservation, unstamped
//! finalization, health transitions) lives in [`adopt`]. The W5/P7 identity
//! surface (ADR 009 §3) adds the v1→v2 migration in [`migrate_v2`] (the
//! v4→v5 adoption-journal source token is [`migrate_v5`]), host
//! enrollment in [`hosts`], route records in [`routes`], single-use attach
//! tokens in [`attach`], the peer snapshot cache in [`peer_cache`], and
//! pane stamps in [`stamps`].

mod util;

mod adopt;
mod alloc;
mod attach;
mod backup;
mod bindings;
mod bootstrap;
mod busy;
mod hosts;
mod idempotency;
mod init;
mod journal;
mod leases;
mod lineage;
mod migrate_v2;
mod migrate_v3;
mod migrate_v5;
mod peer_cache;
mod reconcile;
mod recovery;
mod routes;
mod stamps;
