//! The scope a provider call is made under: which backend, which exact
//! endpoint, and the epoch the caller expects that endpoint to serve.
//!
//! Lives in its own module so that the adapters (`backend::wez`,
//! `backend::tmux`) are not module descendants of the type: a private field
//! here is private to them too. Report 05 measured that with the field
//! private inside `backend/mod.rs` the adapters kept full access, which is
//! exactly the hatch this boundary exists to close (ADR 012 WS-A.1).

use crate::model::{Backend, BackendInstanceUid, ServerEpoch};
use crate::registry::{Registry, Result as RegistryResult};

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

/// What the registry says about one backend's managed instance, resolved
/// before anything is probed. Promoted from `ls_cli`'s `ScanTarget`
/// (ADR 012 WS-A.4, review report 05): this is the one place where "a
/// managed instance's epoch is NULL" is first observable, so it is the one
/// place that decides. The enum carries no `Option<ServerEpoch>` — in the
/// `Unpublished` arm there is no epoch value in scope to hand to
/// [`InventoryScope::managed`], so a caller that wants to proceed anyway has
/// to write the other branch out loud, in a function that resolved a
/// registry instance three lines earlier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedTarget {
    /// A registered, addressable instance with a published epoch: probe
    /// exactly that endpoint, pinned to exactly that epoch.
    Managed {
        instance: BackendInstanceUid,
        scope: InventoryScope,
    },
    /// Registered and addressable, but the server incarnation was never
    /// published (`server_epoch` is NULL). The row exists before the mux
    /// coordinates (`dmux-mux-start.sh` registers first and publishes later)
    /// and stays this way if coordination never completes. Nothing about the
    /// live server can be verified: reads are indeterminate, mutations
    /// refuse, and no durable row may be minted from what a scan would say.
    Unpublished(BackendInstanceUid),
    /// Registered without a recorded endpoint: the registry claims an
    /// instance this process cannot address.
    Unaddressable(BackendInstanceUid),
    /// No instance is registered for this backend. Whether to discover
    /// natives on a first-contact endpoint is the caller's decision, made
    /// with [`InventoryScope::unmanaged_endpoint`] and named in the audit
    /// allowlist.
    Unregistered,
}

impl ManagedTarget {
    /// The instance the registry knows about, whichever state it is in.
    pub fn instance(&self) -> Option<BackendInstanceUid> {
        match self {
            ManagedTarget::Managed { instance, .. }
            | ManagedTarget::Unpublished(instance)
            | ManagedTarget::Unaddressable(instance) => Some(*instance),
            ManagedTarget::Unregistered => None,
        }
    }

    /// The pinned scope, only for a managed instance.
    pub fn scope(&self) -> Option<&InventoryScope> {
        match self {
            ManagedTarget::Managed { scope, .. } => Some(scope),
            _ => None,
        }
    }

    /// The refusal every verb reports for an unpublished instance. One text,
    /// so the nine readers and writers that used to launder this case say the
    /// same thing; the code that goes with it is `BackendEpochChanged`, the
    /// mapping `ls` already made (an unpublished epoch is an epoch fault, not
    /// an unreachable endpoint).
    pub fn unpublished_detail(backend: Backend, instance: BackendInstanceUid) -> String {
        format!(
            "managed {backend} backend instance {} has published no server epoch, so nothing \
             about its live server can be verified",
            instance.0
        )
    }

    /// The refusal for an instance with no recorded endpoint.
    pub fn unaddressable_detail(backend: Backend, instance: BackendInstanceUid) -> String {
        format!(
            "managed {backend} backend instance {} has no recorded endpoint",
            instance.0
        )
    }
}

/// Resolve one backend's managed instance from the registry. This is the
/// only sanctioned way to turn a registry instance into a scope; every
/// `InventoryScope::managed` built from registry rows goes through here or
/// through [`resolve_managed_instance`].
pub fn resolve_managed(registry: &Registry, backend: Backend) -> RegistryResult<ManagedTarget> {
    let Some(instance) = registry.backend_instance_for_backend(backend)? else {
        return Ok(ManagedTarget::Unregistered);
    };
    resolve_managed_instance(registry, instance)
}

/// The same resolution for a caller that already holds the instance — a
/// Space row's `backend_instance`, typically. Never `Unregistered`: the
/// instance is a foreign key the registry vouches for.
pub fn resolve_managed_instance(
    registry: &Registry,
    instance: BackendInstanceUid,
) -> RegistryResult<ManagedTarget> {
    let info = registry.backend_instance_info(instance)?;
    let Some(endpoint) = info.socket_path else {
        return Ok(ManagedTarget::Unaddressable(instance));
    };
    let Some(epoch) = registry.backend_server(instance)?.server_epoch else {
        return Ok(ManagedTarget::Unpublished(instance));
    };
    Ok(ManagedTarget::Managed {
        instance,
        scope: InventoryScope::managed(info.backend, endpoint, epoch),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::RegistryConfig;
    use uuid::Uuid;

    fn scratch() -> (tempfile::TempDir, Registry) {
        let dir = tempfile::tempdir().expect("scratch dir");
        let registry = Registry::open(RegistryConfig::new(
            dir.path().join("registry.sqlite3"),
            dir.path().join("locks"),
        ))
        .expect("scratch registry");
        (dir, registry)
    }

    #[test]
    fn an_unregistered_backend_resolves_to_unregistered() {
        let (_dir, registry) = scratch();
        let target = resolve_managed(&registry, Backend::Wez).unwrap();
        assert_eq!(target, ManagedTarget::Unregistered);
        assert_eq!(target.instance(), None);
        assert!(target.scope().is_none());
    }

    #[test]
    fn an_instance_without_an_endpoint_is_unaddressable() {
        let (_dir, mut registry) = scratch();
        let instance = registry
            .register_backend_instance(Backend::Wez, None, None)
            .unwrap();
        let target = resolve_managed(&registry, Backend::Wez).unwrap();
        assert_eq!(target, ManagedTarget::Unaddressable(instance));
        assert_eq!(target.instance(), Some(instance));
        assert!(target.scope().is_none());
    }

    #[test]
    fn a_registered_instance_with_no_published_epoch_is_unpublished_not_a_scope() {
        let (_dir, mut registry) = scratch();
        let instance = registry
            .register_backend_instance(Backend::Tmux, Some("dmux-scratch"), None)
            .unwrap();
        let target = resolve_managed(&registry, Backend::Tmux).unwrap();
        assert_eq!(target, ManagedTarget::Unpublished(instance));
        assert_eq!(target.instance(), Some(instance));
        // The whole point: there is no scope to hand to a provider here, and
        // no epoch value in reach to build one with.
        assert!(target.scope().is_none());
        let detail = ManagedTarget::unpublished_detail(Backend::Tmux, instance);
        assert!(detail.contains("has published no server epoch"), "{detail}");
        assert!(detail.contains(&instance.0.to_string()), "{detail}");
    }

    #[test]
    fn a_published_instance_resolves_to_a_scope_pinned_to_its_epoch() {
        let (_dir, mut registry) = scratch();
        let instance = registry
            .register_backend_instance(Backend::Wez, Some("/tmp/scratch.sock"), None)
            .unwrap();
        let epoch = ServerEpoch(Uuid::new_v4());
        registry
            .publish_backend_server(instance, epoch, Some(4242), Some("tok"), None, None)
            .unwrap();
        let target = resolve_managed(&registry, Backend::Wez).unwrap();
        let ManagedTarget::Managed {
            instance: resolved,
            scope,
        } = &target
        else {
            panic!("expected Managed, got {target:?}");
        };
        assert_eq!(*resolved, instance);
        assert_eq!(scope.backend, Backend::Wez);
        assert_eq!(scope.endpoint, "/tmp/scratch.sock");
        assert_eq!(scope.expected_epoch(), Some(epoch));
        assert_eq!(target.scope(), Some(scope));
    }

    #[test]
    fn resolving_by_instance_agrees_with_resolving_by_backend() {
        let (_dir, mut registry) = scratch();
        let instance = registry
            .register_backend_instance(Backend::Tmux, Some("dmux-scratch"), None)
            .unwrap();
        assert_eq!(
            resolve_managed_instance(&registry, instance).unwrap(),
            resolve_managed(&registry, Backend::Tmux).unwrap()
        );
        let epoch = ServerEpoch(Uuid::new_v4());
        registry
            .publish_backend_server(instance, epoch, None, None, None, None)
            .unwrap();
        assert_eq!(
            resolve_managed_instance(&registry, instance).unwrap(),
            resolve_managed(&registry, Backend::Tmux).unwrap()
        );
    }
}
