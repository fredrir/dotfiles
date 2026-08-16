//! P4 gate: the exhaustive §8.2 lookup truth table, the stopped/absent
//! durable-record cases, and ref resolution. Root-owned (plan §19).

use std::num::NonZeroU64;

use dmux::backend::{
    InventoryOutcome, NativeGroupRow, NativeInventory, NativeSpaceRow, NativeSplitRow,
};
use dmux::model::{
    Backend, BackendInstanceUid, Health, HostUid, Lifecycle, ProviderHandle, SpaceNo, SpaceUid,
};
use dmux::refs::parse_ref;
use dmux::registry::{BindingRow, BindingState, NativeKind, SpaceRow};
use dmux::resolve::{
    BlockReason, ClassSummary, LiveState, NewLookup, PartitionClass, RefResolution,
    classify_record, lookup_for_new, resolve_space_ref, summarize_backend,
};

fn uid(n: u128) -> SpaceUid {
    SpaceUid(uuid::Uuid::from_u128(n))
}

fn no(n: u64) -> SpaceNo {
    SpaceNo(NonZeroU64::new(n).unwrap())
}

fn space(n: u64, name: &str, lifecycle: Lifecycle, health: Health) -> SpaceRow {
    SpaceRow {
        space_uid: uid(n as u128),
        owner: HostUid(uuid::Uuid::nil()),
        space_no: no(n),
        backend_instance: BackendInstanceUid(uuid::Uuid::nil()),
        logical_name: name.into(),
        lifecycle,
        health,
        created_at: String::new(),
        updated_at: String::new(),
        deleted_at: None,
    }
}

fn binding(n: u64, token: &str) -> BindingRow {
    BindingRow {
        binding_id: n as i64,
        space_uid: uid(n as u128),
        native_token: token.into(),
        native_kind: NativeKind::TmuxSessionId,
        binding_state: BindingState::Current,
        observation: dmux::model::Observation::Live,
    }
}

fn native(token: &str, name: &str) -> NativeSpaceRow {
    NativeSpaceRow {
        native_token: token.into(),
        native_name: name.into(),
        multi_window: false,
        groups: vec![NativeGroupRow {
            handle: ProviderHandle::Tx(0),
            title: None,
            splits: vec![NativeSplitRow {
                handle: ProviderHandle::Tx(0),
                title: None,
                cwd: None,
            }],
        }],
    }
}

fn complete(rows: Vec<NativeSpaceRow>) -> InventoryOutcome {
    InventoryOutcome::Complete(NativeInventory {
        server_epoch: None,
        rows,
    })
}

// ---------------------------------------------------------------------------
// Exhaustive §8.2 truth table over
// constraint × wez class × tmux class × allow-collision.

#[derive(Clone, Copy, Debug, PartialEq)]
enum Cls {
    N, // determinate, no match
    S, // selectable
    B, // blocking
    I, // indeterminate
}

fn summary(cls: Cls, backend: Backend) -> ClassSummary {
    let (s, n) = match backend {
        Backend::Wez => (uid(1), no(1)),
        Backend::Tmux => (uid(2), no(2)),
    };
    match cls {
        Cls::N => ClassSummary::NoMatch,
        Cls::S => ClassSummary::Selectable { space: s, no: n },
        Cls::B => ClassSummary::Blocking {
            reason: BlockReason::ActiveAbsent,
            space: Some(s),
        },
        Cls::I => ClassSummary::Indeterminate,
    }
}

/// Shorthand for the expected outcome of one row.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Want {
    ConnectWez,
    ConnectTmux,
    Ambiguous,
    BlockedWez,
    BlockedTmux,
    IndetWez,
    IndetTmux,
    ConflictOppositeWez,  // existing on wez blocks an explicit tmux create
    ConflictOppositeTmux, // existing on tmux blocks an explicit wez create
    CreateAuto,
    CreateWez,
    CreateTmux,
}

fn check(constraint: Option<Backend>, wez: Cls, tmux: Cls, allow: bool, want: Want) {
    let got = lookup_for_new(
        constraint,
        allow,
        summary(wez, Backend::Wez),
        summary(tmux, Backend::Tmux),
    );
    let matches = match want {
        Want::ConnectWez => matches!(
            got,
            NewLookup::Connect {
                backend: Backend::Wez,
                ..
            }
        ),
        Want::ConnectTmux => matches!(
            got,
            NewLookup::Connect {
                backend: Backend::Tmux,
                ..
            }
        ),
        Want::Ambiguous => matches!(got, NewLookup::Ambiguous { .. }),
        Want::BlockedWez => matches!(
            got,
            NewLookup::Blocked {
                backend: Backend::Wez,
                ..
            }
        ),
        Want::BlockedTmux => matches!(
            got,
            NewLookup::Blocked {
                backend: Backend::Tmux,
                ..
            }
        ),
        Want::IndetWez => matches!(
            got,
            NewLookup::Indeterminate {
                backend: Backend::Wez
            }
        ),
        Want::IndetTmux => matches!(
            got,
            NewLookup::Indeterminate {
                backend: Backend::Tmux
            }
        ),
        Want::ConflictOppositeWez => matches!(
            got,
            NewLookup::OppositeNameConflict {
                existing_backend: Backend::Wez,
                ..
            }
        ),
        Want::ConflictOppositeTmux => matches!(
            got,
            NewLookup::OppositeNameConflict {
                existing_backend: Backend::Tmux,
                ..
            }
        ),
        Want::CreateAuto => got == NewLookup::ProceedCreate { constraint: None },
        Want::CreateWez => {
            got == NewLookup::ProceedCreate {
                constraint: Some(Backend::Wez),
            }
        }
        Want::CreateTmux => {
            got == NewLookup::ProceedCreate {
                constraint: Some(Backend::Tmux),
            }
        }
    };
    assert!(
        matches,
        "constraint={constraint:?} wez={wez:?} tmux={tmux:?} allow={allow} → got {got:?}, wanted {want:?}"
    );
}

#[test]
fn auto_truth_table() {
    use Cls::*;
    use Want::*;
    // (wez, tmux) → expected; allow-collision is irrelevant under auto.
    let table = [
        ((N, N), CreateAuto),
        ((N, S), ConnectTmux),
        ((N, B), BlockedTmux),
        ((N, I), IndetTmux),
        ((S, N), ConnectWez),
        ((S, S), Ambiguous),
        ((S, B), BlockedTmux),
        ((S, I), IndetTmux),
        ((B, N), BlockedWez),
        ((B, S), BlockedWez),
        ((B, B), BlockedWez),
        ((B, I), BlockedWez),
        ((I, N), IndetWez),
        ((I, S), IndetWez),
        // Blocking on a determinate side outranks the other side's
        // indeterminacy (§8.2 step 5 fires before step 6).
        ((I, B), BlockedTmux),
        ((I, I), IndetWez),
    ];
    for ((wez, tmux), want) in table {
        for allow in [false, true] {
            check(None, wez, tmux, allow, want);
        }
    }
}

#[test]
fn explicit_wez_truth_table() {
    use Cls::*;
    use Want::*;
    // (wez=constrained side, tmux=opposite, allow) → expected.
    let table = [
        // Constrained side indeterminate: nothing proceeds.
        ((I, N), false, IndetWez),
        ((I, S), false, IndetWez),
        ((I, B), false, IndetWez),
        ((I, I), false, IndetWez),
        // Constrained side blocking: its typed error.
        ((B, N), false, BlockedWez),
        ((B, S), false, BlockedWez),
        ((B, B), false, BlockedWez),
        ((B, I), false, BlockedWez),
        // Constrained side selectable: select irrespective of the opposite
        // provider (noncreating; the constraint is authoritative).
        ((S, N), false, ConnectWez),
        ((S, S), false, ConnectWez),
        ((S, B), false, ConnectWez),
        ((S, I), false, ConnectWez),
        // No match on the constrained side: the opposite decides.
        ((N, N), false, CreateWez),
        ((N, S), false, ConflictOppositeTmux),
        ((N, B), false, BlockedTmux),
        ((N, I), false, IndetTmux),
        // --allow-name-collision changes exactly one row...
        ((N, S), true, CreateWez),
        // ...and never waives blocking or inventory safety.
        ((N, B), true, BlockedTmux),
        ((N, I), true, IndetTmux),
        ((I, S), true, IndetWez),
    ];
    for ((wez, tmux), allow, want) in table {
        check(Some(Backend::Wez), wez, tmux, allow, want);
    }
}

#[test]
fn explicit_tmux_truth_table_mirrors() {
    use Cls::*;
    use Want::*;
    let table = [
        // Constrained (tmux) side indeterminate → reported on tmux.
        ((I, N), false, IndetTmux),
        ((I, S), false, IndetTmux),
        // Constrained side blocking/selectable behaves like the wez table.
        ((B, I), false, BlockedTmux),
        ((S, I), false, ConnectTmux),
        ((S, S), false, ConnectTmux),
        // No match on tmux → the opposite (wez) side decides; indeterminacy
        // and blocking are reported on the side that owns them.
        ((N, N), false, CreateTmux),
        ((N, S), false, ConflictOppositeWez),
        ((N, B), false, BlockedWez),
        ((N, I), false, IndetWez),
        ((N, S), true, CreateTmux),
        ((N, B), true, BlockedWez),
        ((N, I), true, IndetWez),
    ];
    // Note argument order: check(constraint, wez, tmux, ...) — here the
    // constrained side is tmux, so the tuple is (tmux, wez).
    for ((tmux, wez), allow, want) in table {
        check(Some(Backend::Tmux), wez, tmux, allow, want);
    }
}

// ---------------------------------------------------------------------------
// Stopped/absent durable-record cases (P4 gate).

#[test]
fn active_record_with_stopped_service_blocks_as_provider_unavailable() {
    let s = space(1, "proj", Lifecycle::Active, Health::Healthy);
    let cls = classify_record(&s, true, LiveState::Stopped, false);
    assert_eq!(cls, PartitionClass::Blocking(BlockReason::ServerStopped));
    // End-to-end through the summary: a stopped scan is determinate but the
    // durable record still blocks — creation cannot allocate a replacement.
    let summary = summarize_backend(
        &InventoryOutcome::ServerStopped {
            detail: "proof".into(),
        },
        &[(s, Some(binding(1, "$0")), false)],
        "proj",
    );
    let ClassSummary::Blocking { reason, .. } = summary else {
        panic!("expected blocking, got {summary:?}");
    };
    assert_eq!(reason, BlockReason::ServerStopped);
    assert_eq!(
        reason.error_code(),
        dmux::error::ErrorCode::ProviderUnavailable
    );
}

#[test]
fn active_record_absent_from_complete_scan_is_space_absent() {
    let s = space(1, "proj", Lifecycle::Active, Health::Healthy);
    let summary = summarize_backend(
        &complete(vec![native("$9", "other")]),
        &[(s, Some(binding(1, "$0")), false)],
        "proj",
    );
    let ClassSummary::Blocking { reason, .. } = summary else {
        panic!("{summary:?}")
    };
    assert_eq!(reason, BlockReason::ActiveAbsent);
    assert_eq!(reason.error_code(), dmux::error::ErrorCode::SpaceAbsent);
}

#[test]
fn unmanaged_same_name_blocks_allocation() {
    let summary = summarize_backend(&complete(vec![native("$3", "proj")]), &[], "proj");
    assert_eq!(
        summary,
        ClassSummary::Blocking {
            reason: BlockReason::UnmanagedSameName,
            space: None
        }
    );
}

#[test]
fn terminal_records_never_match_but_partition_reports_them() {
    let s = space(1, "proj", Lifecycle::Deleted, Health::Healthy);
    assert_eq!(
        classify_record(&s, false, LiveState::Indeterminate, false),
        PartitionClass::Terminal
    );
    // A deleted record alone yields NoMatch — the name is free (identity is
    // not: UID/number stay retired, which the registry enforces).
    let summary = summarize_backend(&complete(vec![]), &[(s, None, false)], "proj");
    assert_eq!(summary, ClassSummary::NoMatch);
}

#[test]
fn unfinished_operation_and_unhealthy_block() {
    let s = space(1, "proj", Lifecycle::Active, Health::Healthy);
    assert_eq!(
        classify_record(
            &s,
            true,
            LiveState::Live {
                multi_window: false
            },
            true
        ),
        PartitionClass::Blocking(BlockReason::OperationInProgress)
    );
    let s = space(2, "proj", Lifecycle::Active, Health::Unstamped);
    assert_eq!(
        classify_record(
            &s,
            true,
            LiveState::Live {
                multi_window: false
            },
            false
        ),
        PartitionClass::Blocking(BlockReason::Unhealthy(Health::Unstamped))
    );
    let s = space(3, "proj", Lifecycle::Active, Health::Healthy);
    assert_eq!(
        classify_record(&s, true, LiveState::Live { multi_window: true }, false),
        PartitionClass::Blocking(BlockReason::MultiWindow)
    );
}

// ---------------------------------------------------------------------------
// Space-ref resolution (local shadow scope).

#[test]
fn ref_resolution_cases() {
    let host = HostUid(uuid::Uuid::nil());
    let spaces = vec![
        space(1, "alpha", Lifecycle::Active, Health::Healthy),
        space(2, "dup", Lifecycle::Active, Health::Healthy),
        space(3, "dup", Lifecycle::Active, Health::Healthy),
        space(4, "old", Lifecycle::Deleted, Health::Healthy),
    ];
    let resolve = |input: &str| {
        let parsed = parse_ref(input).unwrap();
        resolve_space_ref(&parsed.space, host, &spaces)
    };
    // Permanent local number.
    assert_eq!(resolve("1"), RefResolution::Space(uid(1)));
    // A deleted Space's number resolves to Deleted (exit 3), never reused.
    assert_eq!(resolve("4"), RefResolution::Deleted(uid(4)));
    assert_eq!(resolve("9"), RefResolution::NotFound);
    // Exact bare name; duplicates are ambiguous, never guessed.
    assert_eq!(resolve("alpha"), RefResolution::Space(uid(1)));
    assert_eq!(
        resolve("dup"),
        RefResolution::AmbiguousName(vec![uid(2), uid(3)])
    );
    // `a:` prefixes the local authority; enrolled remotes are P7.
    assert_eq!(resolve("a:1"), RefResolution::Space(uid(1)));
    assert_eq!(resolve("b:1"), RefResolution::UnsupportedHostScope);
    // Canonical URI for the local host.
    let uri = format!(
        "dmux://{}/spaces/{}",
        uuid::Uuid::nil(),
        uuid::Uuid::from_u128(1)
    );
    assert_eq!(resolve(&uri), RefResolution::Space(uid(1)));
}
