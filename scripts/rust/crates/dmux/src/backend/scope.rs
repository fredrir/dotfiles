//! The scope a provider call is made under: which backend, which exact
//! endpoint, and the epoch the caller expects that endpoint to serve.
//!
//! Lives in its own module so that the adapters (`backend::wez`,
//! `backend::tmux`) are not module descendants of the type: a private field
//! here is private to them too. Report 05 measured that with the field
//! private inside `backend/mod.rs` the adapters kept full access, which is
//! exactly the hatch this boundary exists to close (ADR 012 WS-A.1).

use crate::model::{Backend, ServerEpoch};

/// Scope for an inventory scan. v1 has one managed instance per backend per
/// owner; the scope carries the exact endpoint identity to verify against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryScope {
    pub backend: Backend,
    /// Exact socket path (Wez service socket / tmux `-L` namespace socket).
    pub endpoint: String,
    /// Expected epoch when the caller already holds one; a mismatch is
    /// `backend_epoch_changed`, and returned native IDs are discarded.
    pub expected_epoch: Option<ServerEpoch>,
}
