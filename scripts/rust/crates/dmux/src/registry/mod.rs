//! Registry: transactions and the public registry API (plan §10).
//! Implemented in P2 by the identity/registry agent, which owns
//! `src/registry/**` from the recorded W2 handoff onward.
//!
//! The storage contract is `docs/adr/dmux/registry-v1.sql` — equivalent
//! index names are allowed, weaker semantics are not. Submodules per plan
//! §9.2: `schema` (versioned SQLite migrations), `reconcile` (adoption and
//! scan reconciliation).
