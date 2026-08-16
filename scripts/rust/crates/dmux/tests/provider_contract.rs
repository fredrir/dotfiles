//! Provider conformance harness — root-owned (plan §19, W1).
//!
//! P3a/P3b register their real adapters here; provider agents own only their
//! adapter-specific fixtures (`tests/fixtures/{tmux,wez}/**`, `provider_*.rs`).
//! In P1 the harness runs against an in-memory fake, which pins down trait
//! object-safety and the invariants every `Complete` inventory must satisfy.

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

struct FakeProvider {
    epoch: ServerEpoch,
}

impl FakeProvider {
    fn scope(&self) -> InventoryScope {
        InventoryScope {
            backend: Backend::Wez,
            endpoint: "/tmp/fake/sock".into(),
            expected_epoch: Some(self.epoch),
        }
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
        if scope.expected_epoch.is_some() && scope.expected_epoch != Some(self.epoch) {
            return InventoryOutcome::Malformed {
                detail: "epoch mismatch".into(),
            };
        }
        InventoryOutcome::Complete(NativeInventory {
            server_epoch: Some(self.epoch),
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

    fn create(&self, _: &InventoryScope, spec: &CreateSpec) -> ProviderResult<NativeBinding> {
        assert!(
            !spec.bootstrap_argv.is_empty(),
            "provider spawns the helper, never raw commands"
        );
        Ok(NativeBinding {
            native_token: spec.native_token.clone(),
            server_epoch: self.epoch,
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
        if binding.server_epoch != self.epoch {
            return Err(ProviderError::EpochChanged {
                expected: binding.server_epoch,
                observed: Some(self.epoch),
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
    let fake = FakeProvider {
        epoch: ServerEpoch(Uuid::nil()),
    };
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
    let fake = FakeProvider {
        epoch: ServerEpoch(Uuid::nil()),
    };
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
