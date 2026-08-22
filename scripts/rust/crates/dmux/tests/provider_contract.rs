//! Provider conformance harness — root-owned (plan §19, W1).
//!
//! P3a/P3b register their real adapters here; provider agents own only their
//! adapter-specific fixtures (`tests/fixtures/{tmux,wez}/**`, `provider_*.rs`).
//! In P1 the harness runs against an in-memory fake, which pins down trait
//! object-safety and the invariants every `Complete` inventory must satisfy.
//!
//! The fake is also the contract's worked example of the scope boundary
//! (`backend::scope`, ADR 012 WS-A.13): under [`InventoryScope::managed`] a
//! provider verifies the live server against the caller's pin and refuses a
//! mismatch; under [`InventoryScope::unmanaged_endpoint`] it may only
//! *discover* a server nobody has published. A published server reached
//! without a pin is refused, not answered — the pin was dropped on the way,
//! and `None` never means "skip verification" (review report 02 on this
//! file, and the `_in` readers it used to mirror).

use std::collections::HashSet;

use dmux::backend::{
    Capabilities, CreateSpec, InventoryOutcome, InventoryScope, NativeBinding, NativeGroupRow,
    NativeInventory, NativeSpaceRow, NativeSplitRow, PresentationTarget, Provider, ProviderError,
    ProviderResult, SplitSpec,
};
use dmux::model::{Backend, ProviderHandle, ServerEpoch};
use uuid::Uuid;

/// Invariants every provider's `Complete` inventory must satisfy, regardless
/// of backend. Real adapters are run through this same assertion set.
fn assert_inventory_invariants(inv: &NativeInventory) {
    let mut space_tokens = HashSet::new();
    let mut group_handles = HashSet::new();
    let mut split_handles = HashSet::new();
    for row in &inv.rows {
        assert!(
            !row.native_token.is_empty(),
            "native token must be non-empty"
        );
        assert!(
            space_tokens.insert(row.native_token.clone()),
            "duplicate native token {}",
            row.native_token
        );
        assert!(
            !row.native_token.starts_with("dmux:system:"),
            "sentinel must be excluded from user inventory (ADR 002)"
        );
        for group in &row.groups {
            assert!(
                group_handles.insert(group.handle.clone()),
                "group handle {} not unique across the scan",
                group.handle
            );
            assert!(
                !group.splits.is_empty(),
                "a Group always has at least one Split"
            );
            for split in &group.splits {
                assert!(
                    split_handles.insert(split.handle.clone()),
                    "split handle {} not unique across the scan",
                    split.handle
                );
            }
        }
    }
}

const ENDPOINT: &str = "/tmp/fake/sock";

/// The refusal both adapters give a managed action without a pin
/// (`tmux.rs` `required_epoch`, `wez.rs` `required_action_epoch`).
const UNPINNED_DETAIL: &str = "managed action requires a managed scope carrying the published \
                               server epoch; an unmanaged endpoint may only be discovered";

struct FakeProvider {
    /// What the server itself answers for its incarnation: `Some` for a
    /// server whose epoch the registry has published, `None` for a
    /// first-contact server nothing vouches for (tmux's "unepoched").
    epoch: Option<ServerEpoch>,
}

impl FakeProvider {
    fn published(epoch: ServerEpoch) -> Self {
        Self { epoch: Some(epoch) }
    }

    fn unepoched() -> Self {
        Self { epoch: None }
    }

    /// The scope a caller that holds this server's published epoch builds.
    fn scope(&self) -> InventoryScope {
        InventoryScope::managed(
            Backend::Wez,
            ENDPOINT,
            self.epoch
                .expect("only a published server has an epoch to pin"),
        )
    }

    /// A managed action needs the caller's pin; nothing about the scope's
    /// endpoint says which server is meant without one.
    fn required_epoch(scope: &InventoryScope) -> ProviderResult<ServerEpoch> {
        scope
            .expected_epoch()
            .ok_or_else(|| ProviderError::WrongInstance {
                detail: UNPINNED_DETAIL.into(),
            })
    }
}

impl Provider for FakeProvider {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            backend: Backend::Wez,
            cas_rename: true,
            probed: vec![],
        }
    }

    fn inventory(&self, scope: &InventoryScope) -> InventoryOutcome {
        match (scope.expected_epoch(), self.epoch) {
            // A pinned read verifies the live server against the pin and
            // refuses a mismatch — an unepoched server included — as the
            // epoch fault `ls` maps to `backend_epoch_changed` (the tmux
            // adapter's inventory, the wez adapter's `ScanFail::EpochChanged`).
            (Some(expected), observed) if observed != Some(expected) => {
                return InventoryOutcome::Malformed {
                    detail: format!(
                        "backend_epoch_changed: expected {} observed {}",
                        expected.0,
                        observed.map_or("unepoched".to_string(), |e| e.0.to_string())
                    ),
                };
            }
            // A published server addressed without a pin: the caller reached
            // a registry-managed instance and dropped its epoch on the way.
            // That is the laundering class ADR 012 closes, so the read is
            // refused the way the wez adapter reports `WrongInstance` on a
            // scan — never answered, so nothing minted from it can look
            // verified.
            (None, Some(_)) => {
                return InventoryOutcome::Malformed {
                    detail: format!("wrong_backend_instance: {UNPINNED_DETAIL}"),
                };
            }
            // A pinned read of the incarnation it was pinned to is complete,
            // and a first-contact server under an unmanaged scope is
            // discoverable (complete, `server_epoch: None`, nothing written).
            (Some(_), _) | (None, None) => {}
        }
        InventoryOutcome::Complete(NativeInventory {
            server_epoch: self.epoch,
            rows: vec![NativeSpaceRow {
                native_token: "dmux:host:space".into(),
                native_name: "dmux:host:space".into(),
                multi_window: false,
                groups: vec![NativeGroupRow {
                    handle: ProviderHandle::Wz(1),
                    title: Some("editor".into()),
                    splits: vec![
                        NativeSplitRow {
                            handle: ProviderHandle::Wz(1),
                            title: None,
                            cwd: Some("/home/user".into()),
                        },
                        NativeSplitRow {
                            handle: ProviderHandle::Wz(2),
                            title: None,
                            cwd: None,
                        },
                    ],
                }],
            }],
        })
    }

    fn create(&self, scope: &InventoryScope, spec: &CreateSpec) -> ProviderResult<NativeBinding> {
        let epoch = Self::required_epoch(scope)?;
        if self.epoch != Some(epoch) {
            return Err(ProviderError::EpochChanged {
                expected: epoch,
                observed: self.epoch,
            });
        }
        assert!(
            !spec.bootstrap_argv.is_empty(),
            "provider spawns the helper, never raw commands"
        );
        Ok(NativeBinding {
            native_token: spec.native_token.clone(),
            server_epoch: epoch,
            root_group: ProviderHandle::Wz(9),
            root_split: ProviderHandle::Wz(9),
        })
    }

    fn prepare_presentation(
        &self,
        _: &InventoryScope,
        binding: &NativeBinding,
        _: Option<&ProviderHandle>,
    ) -> ProviderResult<PresentationTarget> {
        if Some(binding.server_epoch) != self.epoch {
            return Err(ProviderError::EpochChanged {
                expected: binding.server_epoch,
                observed: self.epoch,
            });
        }
        Ok(PresentationTarget::Wez {
            domain: "unix".into(),
            opaque_key: binding.native_token.clone(),
            child: None,
        })
    }

    fn rename(&self, _: &InventoryScope, _: &NativeBinding, _: &str) -> ProviderResult<()> {
        Ok(())
    }
    fn remove(&self, _: &InventoryScope, _: &NativeBinding) -> ProviderResult<()> {
        Ok(())
    }
    fn group_list(
        &self,
        _: &InventoryScope,
        _: &NativeBinding,
    ) -> ProviderResult<Vec<NativeGroupRow>> {
        Ok(vec![])
    }
    fn group_new(
        &self,
        _: &InventoryScope,
        _: &NativeBinding,
        _: &CreateSpec,
    ) -> ProviderResult<ProviderHandle> {
        Ok(ProviderHandle::Wz(10))
    }
    fn group_activate(&self, _: &InventoryScope, _: &ProviderHandle) -> ProviderResult<()> {
        Ok(())
    }
    fn group_rename(&self, _: &InventoryScope, _: &ProviderHandle, _: &str) -> ProviderResult<()> {
        Ok(())
    }
    fn group_remove(&self, _: &InventoryScope, _: &ProviderHandle) -> ProviderResult<()> {
        Ok(())
    }
    fn split_list(
        &self,
        _: &InventoryScope,
        _: &ProviderHandle,
    ) -> ProviderResult<Vec<NativeSplitRow>> {
        Ok(vec![])
    }
    fn split_new(
        &self,
        _: &InventoryScope,
        _: &ProviderHandle,
        _: &SplitSpec,
    ) -> ProviderResult<ProviderHandle> {
        Ok(ProviderHandle::Wz(11))
    }
    fn split_activate(&self, _: &InventoryScope, _: &ProviderHandle) -> ProviderResult<()> {
        Ok(())
    }
    fn split_remove(&self, _: &InventoryScope, _: &ProviderHandle) -> ProviderResult<()> {
        Ok(())
    }
    fn inspect(&self, _: &InventoryScope, _: &NativeBinding) -> ProviderResult<NativeSpaceRow> {
        Err(ProviderError::NotFound {
            native_ref: "none".into(),
        })
    }
}

#[test]
fn provider_trait_is_object_safe_and_fake_passes_invariants() {
    let fake = FakeProvider::published(ServerEpoch(Uuid::nil()));
    let scope = fake.scope();
    let provider: Box<dyn Provider> = Box::new(fake);
    match provider.inventory(&scope) {
        InventoryOutcome::Complete(inv) => assert_inventory_invariants(&inv),
        other => panic!("expected complete inventory, got {other:?}"),
    }
    assert!(provider.capabilities().cas_rename);
}

#[test]
fn indeterminate_outcomes_are_classified() {
    let outcomes = [
        InventoryOutcome::Unreachable {
            detail: String::new(),
        },
        InventoryOutcome::AuthFailed {
            detail: String::new(),
        },
        InventoryOutcome::Timeout {
            detail: String::new(),
        },
        InventoryOutcome::Malformed {
            detail: String::new(),
        },
    ];
    for o in outcomes {
        assert!(!o.is_determinate(), "{o:?} must not establish zero rows");
    }
    assert!(
        InventoryOutcome::ServerStopped {
            detail: String::new()
        }
        .is_determinate()
    );
}

#[test]
fn stale_epoch_presentation_fails_typed() {
    let fake = FakeProvider::published(ServerEpoch(Uuid::nil()));
    let scope = fake.scope();
    let stale = NativeBinding {
        native_token: "dmux:host:space".into(),
        server_epoch: ServerEpoch(Uuid::max()),
        root_group: ProviderHandle::Wz(1),
        root_split: ProviderHandle::Wz(1),
    };
    match fake.prepare_presentation(&scope, &stale, None) {
        Err(ProviderError::EpochChanged { .. }) => {}
        other => panic!("stale epoch must fail typed, got {other:?}"),
    }
}

fn create_spec() -> CreateSpec {
    CreateSpec {
        native_token: "dmux:host:space".into(),
        cwd: None,
        bootstrap_argv: vec!["/test-only/pane-bootstrap".into()],
    }
}

/// The assertion the review found missing (report 02 on this file): a
/// managed *read* refuses on `None`. A published server reached through an
/// unmanaged scope is answered with a typed, indeterminate refusal — never a
/// `Complete` inventory a caller could mint registry rows from.
#[test]
fn a_managed_read_without_a_pin_is_refused_not_answered() {
    let fake = FakeProvider::published(ServerEpoch(Uuid::from_u128(7)));
    let unpinned = InventoryScope::unmanaged_endpoint(Backend::Wez, ENDPOINT);
    match fake.inventory(&unpinned) {
        InventoryOutcome::Malformed { detail } => {
            assert!(
                detail.starts_with("wrong_backend_instance: "),
                "refused as the wrong-instance class, not an epoch comparison: {detail}"
            );
            assert!(detail.contains("requires a managed scope"), "{detail}");
        }
        other => panic!("an unpinned read of a published server must refuse, got {other:?}"),
    }
    assert!(
        !fake.inventory(&unpinned).is_determinate(),
        "the refusal establishes nothing about the server's rows"
    );
    // The same pin is what every mutation needs; both adapters refuse it the
    // same way (`tmux.rs` `required_epoch`, `wez.rs` `required_action_epoch`).
    match fake.create(&unpinned, &create_spec()) {
        Err(ProviderError::WrongInstance { detail }) => {
            assert!(detail.contains("requires a managed scope"), "{detail}");
        }
        other => panic!("an unpinned mutation must refuse typed, got {other:?}"),
    }
}

/// A pinned read is a verification, and the verification's failure is the
/// `backend_epoch_changed` fault — whether the server answers with another
/// epoch or with none at all.
#[test]
fn a_pinned_read_refuses_a_server_that_is_not_the_pinned_incarnation() {
    let pinned = InventoryScope::managed(Backend::Wez, ENDPOINT, ServerEpoch(Uuid::from_u128(7)));
    let replaced = FakeProvider::published(ServerEpoch(Uuid::from_u128(8)));
    let unepoched = FakeProvider::unepoched();
    for (server, observed) in [
        (&replaced, "observed 00000000-0000-0000-0000-000000000008"),
        (&unepoched, "observed unepoched"),
    ] {
        match server.inventory(&pinned) {
            InventoryOutcome::Malformed { detail } => {
                assert!(detail.starts_with("backend_epoch_changed: "), "{detail}");
                assert!(detail.ends_with(observed), "{detail}");
            }
            other => panic!("a pinned read of another incarnation must refuse, got {other:?}"),
        }
    }
    match replaced.create(&pinned, &create_spec()) {
        Err(ProviderError::EpochChanged { expected, observed }) => {
            assert_eq!(expected, ServerEpoch(Uuid::from_u128(7)));
            assert_eq!(observed, Some(ServerEpoch(Uuid::from_u128(8))));
        }
        other => {
            panic!("a pinned mutation on another incarnation must refuse typed, got {other:?}")
        }
    }
}

/// What the tmux adapter pins in
/// `inventory_unepoched_server_reports_none_and_never_writes`: a first-contact
/// server under an unmanaged scope is listable, reports no epoch, and stays
/// unaddressable — `create` on it is a typed error, never a spawn.
#[test]
fn a_first_contact_server_is_discoverable_but_never_addressable() {
    let fake = FakeProvider::unepoched();
    let discovery = InventoryScope::unmanaged_endpoint(Backend::Wez, ENDPOINT);
    match fake.inventory(&discovery) {
        InventoryOutcome::Complete(inv) => {
            assert_eq!(inv.server_epoch, None, "nothing vouches for an epoch");
            assert_inventory_invariants(&inv);
        }
        other => panic!("first contact is a complete discovery, got {other:?}"),
    }
    assert!(matches!(
        fake.create(&discovery, &create_spec()),
        Err(ProviderError::WrongInstance { .. })
    ));
}
