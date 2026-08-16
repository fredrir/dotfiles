//! Stable previous/current Space references for `dmux -` (plan §9.2).
//! Implemented in P2 by the identity/registry agent, which owns this file
//! from the recorded W2 handoff onward.
//!
//! Contract highlights: history is keyed by SpaceUid (stable identity),
//! never by mutable names or row positions; client-side history lives under
//! `$XDG_STATE_HOME/dmux`; legacy name-based history converts to SpaceUid
//! only when unambiguous, warning and dropping the rest (plan §17 step 11).
