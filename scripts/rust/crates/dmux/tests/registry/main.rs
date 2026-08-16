//! P2 registry gate tests (plan §18 P2 row, §20.1): concurrent first-run,
//! allocation uniqueness/monotonicity, tombstone non-reuse, journal crash
//! points, idempotency, SQLITE_BUSY typing, WAL-safe online backup, the
//! authority revision chain, and fenced leases.

mod util;

mod alloc;
mod backup;
mod busy;
mod idempotency;
mod init;
mod journal;
mod leases;
mod lineage;
