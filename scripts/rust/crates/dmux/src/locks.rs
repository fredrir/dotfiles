//! POSIX scoped kernel locks and the normative acquisition ordering
//! (plan §10.1). Implemented in P2 by the identity/registry agent, which
//! owns this file from the recorded W2 handoff onward.
//!
//! Contract highlights the implementation must satisfy: authority gate
//! (shared; maintenance exclusive) → decision locks in exact-byte lexical
//! order → backend-instance lock(s) by BackendInstanceUid → Space lock;
//! release in reverse; no decision lock after backend/Space. `fcntl` locks
//! provide non-stealable exclusion; SQLite lease rows record ownership and
//! fencing tokens; clock expiry alone never authorizes takeover. Lock files
//! live under `crate::runtime::dmux_runtime_dir()`.
