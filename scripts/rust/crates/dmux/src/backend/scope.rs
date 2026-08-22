//! The scope a provider call is made under: which backend, which exact
//! endpoint, and the epoch the caller expects that endpoint to serve.
//!
//! Lives in its own module so that the adapters (`backend::wez`,
//! `backend::tmux`) are not module descendants of the type: a private field
//! here is private to them too. Report 05 measured that with the field
//! private inside `backend/mod.rs` the adapters kept full access, which is
//! exactly the hatch this boundary exists to close (ADR 012 WS-A.1).

use crate::model::{Backend, ServerEpoch};

/// Scope for one provider call: the backend, the exact endpoint, and — for a
/// managed instance — the epoch the registry has published for it.
///
/// The epoch is private. There are exactly two ways to build a scope, and
/// they are not interchangeable:
///
/// * [`InventoryScope::managed`] — the endpoint came from a registry instance
///   and the caller holds that instance's *published* epoch. The provider
///   verifies the live server against it, refuses with
///   `backend_epoch_changed` on mismatch, and discards native IDs from a
///   server that did not match.
/// * [`InventoryScope::unmanaged_endpoint`] — nothing in the registry vouches
///   for the endpoint: a first-contact tmux namespace reached only after
///   `backend_instance_for_backend` returned `None`, or the hidden `--socket`
///   test seam. A scan under it trusts whatever answers, so no mutation may
///   run under it and no durable registry row may be minted from what it
///   observes.
///
/// A registry instance whose published epoch is `NULL` is **not** unmanaged;
/// it is a managed instance nobody has verified yet (ADR 012 §4, review
/// report 05). Turning that `None` into `unmanaged_endpoint` is the defect
/// class this boundary exists to close, which is why every
/// `unmanaged_endpoint` call site in `src/` is held to an explicit allowlist
/// by the audit test in `tests/`, and why there is no constructor that takes
/// an `Option<ServerEpoch>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryScope {
    pub backend: Backend,
    /// Exact socket path (Wez service socket / tmux `-L` namespace socket).
    pub endpoint: String,
    expected_epoch: Option<ServerEpoch>,
}

impl InventoryScope {
    /// A scope for a registry-managed instance whose published epoch the
    /// caller holds. The adapters verify the live server against `epoch`
    /// before trusting anything it says.
    pub fn managed(backend: Backend, endpoint: impl Into<String>, epoch: ServerEpoch) -> Self {
        Self {
            backend,
            endpoint: endpoint.into(),
            expected_epoch: Some(epoch),
        }
    }

    /// A scope for an endpoint the registry does not vouch for. Discovery
    /// only: reads under it are unverified and mutations refuse. Every call
    /// site is named in the audit allowlist; add one only with a reason the
    /// allowlist can quote.
    pub fn unmanaged_endpoint(backend: Backend, endpoint: impl Into<String>) -> Self {
        Self {
            backend,
            endpoint: endpoint.into(),
            expected_epoch: None,
        }
    }

    /// The epoch a managed scope pins, or `None` for an unmanaged endpoint.
    pub fn expected_epoch(&self) -> Option<ServerEpoch> {
        self.expected_epoch
    }
}
