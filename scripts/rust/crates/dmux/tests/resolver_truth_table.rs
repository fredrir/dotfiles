//! P4 gate: the exhaustive §8.2 lookup truth table, the stopped/absent
//! durable-record cases, and §6.2 ref resolution. Root-owned (plan §19).
//!
//! The ref-resolution half drives the entry points production calls (ADR
//! 012 WS-D.3): `scope_space_ref` is what `con`/`rm` call before handing
//! the owner a query, `resolve_locator` is the lookup every local verb and
//! the GUI perform, and `resolve_space_ref` composes the two. Nothing here
//! is a fixture-only path; a rule asserted below is the rule the CLI runs.

use std::num::NonZeroU64;

use dmux::backend::{
    InventoryOutcome, NativeGroupRow, NativeInventory, NativeSpaceRow, NativeSplitRow,
};
use dmux::error::{ErrorCode, TypedError};
use dmux::model::{
    Backend, BackendInstanceUid, Health, HostUid, Lifecycle, ProviderHandle, SpaceNo, SpaceUid,
};
use dmux::refs::{HostToken, parse_ref};
use dmux::registry::{BindingRow, BindingState, HostLifecycle, HostRow, NativeKind, SpaceRow};
use dmux::resolve::{
    BlockReason, ClassSummary, HostContext, LiveState, NewLookup, OwnerLocator, PartitionClass,
    RefResolution, ScopedSpaceRef, SpaceSelector, classify_record, lookup_for_new,
    resolve_enrolled_host, resolve_locator, resolve_space_ref, scope_space_ref, summarize_backend,
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
        server_epoch: None,
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
// §6.2 Space-ref resolution, through the production entry points.

const LOCAL: HostUid = HostUid(uuid::Uuid::nil());
const REMOTE: HostUid = HostUid(uuid::Uuid::from_u128(0xb));

/// The host table every production site resolves tokens against: `a` is the
/// local authority, `b`/`archie` the one enrolled peer, anything else — an
/// unknown alias, a tombstoned host, a foreign UID — is an error, never a
/// name. Modelled as the closure the resolver takes, exactly as `con`/`rm`
/// hand it their authority's `resolve_host`.
fn hosts(token: &HostToken) -> Result<HostUid, TypedError> {
    match token {
        HostToken::Uid(uid) if *uid == LOCAL || *uid == REMOTE => Ok(*uid),
        HostToken::AliasOrLabel(t) if t == "a" => Ok(LOCAL),
        HostToken::AliasOrLabel(t) if t == "b" || t == "archie" => Ok(REMOTE),
        other => Err(TypedError::new(
            ErrorCode::NotFound,
            format!("no enrolled host matches {other:?}"),
        )),
    }
}

fn context(explicit: Option<HostUid>) -> HostContext {
    HostContext {
        local: LOCAL,
        explicit,
    }
}

fn shape(input: &str) -> dmux::refs::SpaceRefShape {
    parse_ref(input).unwrap().space
}

fn scope(input: &str, explicit: Option<HostUid>) -> Result<ScopedSpaceRef, TypedError> {
    scope_space_ref(
        SpaceSelector::Shape(&shape(input)),
        context(explicit),
        hosts,
    )
}

fn local_rows() -> Vec<SpaceRow> {
    vec![
        space(1, "alpha", Lifecycle::Active, Health::Healthy),
        space(2, "dup", Lifecycle::Active, Health::Healthy),
        space(3, "dup", Lifecycle::Active, Health::Healthy),
        space(4, "old", Lifecycle::Deleted, Health::Healthy),
        space(5, "held", Lifecycle::Deleting, Health::Healthy),
    ]
}

/// Resolve one spelling the way a local verb does: the local registry's rows
/// are the candidates whenever §6.2 scoped the ref to the local authority;
/// a remote owner gets an empty answer here (the verb would ask the peer).
fn resolve(input: &str) -> (ScopedSpaceRef, RefResolution<SpaceRow>) {
    resolve_space_ref(
        SpaceSelector::Shape(&shape(input)),
        context(None),
        hosts,
        |owner| {
            Ok(if owner == LOCAL {
                local_rows()
            } else {
                Vec::new()
            })
        },
    )
    .unwrap()
}

fn resolved_uid(outcome: &RefResolution<SpaceRow>) -> Option<SpaceUid> {
    match outcome {
        RefResolution::Space(row) | RefResolution::Deleted(row) => Some(row.space_uid),
        _ => None,
    }
}

#[test]
fn ref_resolution_cases() {
    // Permanent local number; the matched row comes back whole, lifecycle
    // included, so the verb applies its own gate (`deleting` is not active).
    let (scoped, got) = resolve("1");
    assert_eq!(scoped.owner, LOCAL);
    assert_eq!(scoped.locator, OwnerLocator::Number(no(1)));
    assert!(matches!(&got, RefResolution::Space(row) if row.space_uid == uid(1)));
    let (_, got) = resolve("5");
    assert!(
        matches!(&got, RefResolution::Space(row) if row.lifecycle == Lifecycle::Deleting),
        "{got:?}"
    );
    // A deleted Space's number resolves to Deleted (exit 3, `space_deleted`),
    // never reused and never "not found".
    assert!(matches!(&resolve("4").1, RefResolution::Deleted(row) if row.space_uid == uid(4)));
    assert_eq!(resolve("9").1, RefResolution::NotFound);
    // Exact bare name; duplicates across backends are ambiguous, never
    // guessed; a deleted record's name is free.
    assert_eq!(resolved_uid(&resolve("alpha").1), Some(uid(1)));
    let RefResolution::AmbiguousName(rows) = resolve("dup").1 else {
        panic!("dup must be ambiguous");
    };
    assert_eq!(
        rows.iter().map(|row| row.space_uid).collect::<Vec<_>>(),
        vec![uid(2), uid(3)]
    );
    assert_eq!(resolve("old").1, RefResolution::NotFound);
    // `a:` is the local authority through the host table, not a literal.
    assert_eq!(resolved_uid(&resolve("a:1").1), Some(uid(1)));
    // Canonical URI for the local host, and for the deleted record.
    let uri = |n: u128| format!("dmux://{}/spaces/{}", LOCAL.0, uuid::Uuid::from_u128(n));
    assert_eq!(resolved_uid(&resolve(&uri(1)).1), Some(uid(1)));
    assert!(matches!(resolve(&uri(4)).1, RefResolution::Deleted(_)));
    assert_eq!(resolve(&uri(9)).1, RefResolution::NotFound);
}

/// §6.2: "an encoded ref wins, then explicit `--host`, otherwise bare names
/// … use local authority `a`"; `b2`, `b:2`, `archie:project` and
/// `<host-uuid>:2` all encode their owner; an unknown or tombstoned host
/// token is an error, never a fallback to a logical name.
#[test]
fn host_scoping_follows_the_grammar_defaulting_order() {
    // Encoded owner wins, in every encoded form.
    for (input, locator) in [
        ("b2", OwnerLocator::Number(no(2))),
        ("b:2", OwnerLocator::Number(no(2))),
        ("archie:project", OwnerLocator::Name("project".into())),
        (&format!("{}:2", REMOTE.0), OwnerLocator::Number(no(2))),
        (
            &format!("dmux://{}/spaces/{}", REMOTE.0, uuid::Uuid::from_u128(7)),
            OwnerLocator::Uid(uid(7)),
        ),
    ] {
        let scoped = scope(input, None).unwrap();
        assert_eq!(scoped.owner, REMOTE, "{input}");
        assert_eq!(scoped.locator, locator, "{input}");
        // …and still wins with a `--host` that agrees.
        assert_eq!(scope(input, Some(REMOTE)).unwrap().owner, REMOTE, "{input}");
    }
    // A `--host` that names another owner than the ref is a contradiction,
    // refused as an invalid ref before any owner is consulted.
    let error = scope("b2", Some(LOCAL)).unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidRef);
    assert!(error.message.contains("contradicts reference owner"));
    // Nothing encoded: explicit `--host`, then the local authority.
    assert_eq!(scope("2", Some(REMOTE)).unwrap().owner, REMOTE);
    assert_eq!(scope("project", Some(REMOTE)).unwrap().owner, REMOTE);
    assert_eq!(scope("2", None).unwrap().owner, LOCAL);
    assert_eq!(scope("project", None).unwrap().owner, LOCAL);
    // An unknown host token is the host table's error — never a name.
    let error = scope("zz:2", None).unwrap_err();
    assert_eq!(error.code, ErrorCode::NotFound);
    let error = scope("zz:project", None).unwrap_err();
    assert_eq!(error.code, ErrorCode::NotFound);
    let foreign = HostUid(uuid::Uuid::from_u128(0xdead));
    let error = scope(
        &format!("dmux://{}/spaces/{}", foreign.0, uuid::Uuid::from_u128(1)),
        None,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::NotFound);
}

/// The `--name` escape is a literal on the explicit-or-local owner: no
/// grammar, no host search, no empty name.
#[test]
fn the_exact_name_escape_is_literal_and_owner_local() {
    let scoped = scope_space_ref(SpaceSelector::ExactName("b2"), context(None), hosts).unwrap();
    assert_eq!(scoped.owner, LOCAL);
    assert_eq!(scoped.locator, OwnerLocator::Name("b2".into()));
    let scoped = scope_space_ref(
        SpaceSelector::ExactName("dmux://x"),
        context(Some(REMOTE)),
        hosts,
    )
    .unwrap();
    assert_eq!(scoped.owner, REMOTE);
    assert_eq!(scoped.locator, OwnerLocator::Name("dmux://x".into()));
    let error = scope_space_ref(SpaceSelector::ExactName(""), context(None), hosts).unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidRef);
    // The same literal, looked up: the name `3` is not the Space numbered 3.
    let rows = vec![
        space(3, "3", Lifecycle::Active, Health::Healthy),
        space(4, "four", Lifecycle::Active, Health::Healthy),
    ];
    let by_name = resolve_locator(&OwnerLocator::Name("4".into()), rows.clone());
    assert_eq!(by_name, RefResolution::NotFound);
    let by_name = resolve_locator(&OwnerLocator::Name("3".into()), rows.clone());
    assert!(matches!(by_name, RefResolution::Space(row) if row.space_no == no(3)));
}

/// Acceptance case 44: a deprecated listing index can never silently become
/// a stable ID. Bare digits are a permanent `SpaceNo` in every verb — the
/// grammar has no index form and the resolver accepts no index input — so a
/// listing whose first row is Space 2 resolves `1` to the retired Space 1
/// and `3` to nothing, never to "the third row". `--row` is the one explicit
/// compatibility spelling, and it lives in `rm` beside the listing it
/// indexes, handing the resolver a `Number` it echoes on stderr.
#[test]
fn a_bare_number_is_a_space_no_never_a_listing_index() {
    let listing_order = vec![
        space(2, "second", Lifecycle::Active, Health::Healthy),
        space(5, "fifth", Lifecycle::Active, Health::Healthy),
        space(1, "first", Lifecycle::Deleted, Health::Healthy),
    ];
    let lookup = |input: &str| {
        resolve_space_ref(
            SpaceSelector::Shape(&shape(input)),
            context(None),
            hosts,
            |_| Ok(listing_order.clone()),
        )
        .unwrap()
    };
    let (scoped, got) = lookup("1");
    assert_eq!(scoped.locator, OwnerLocator::Number(no(1)));
    assert!(matches!(got, RefResolution::Deleted(row) if row.logical_name == "first"));
    let (_, got) = lookup("2");
    assert!(matches!(got, RefResolution::Space(row) if row.logical_name == "second"));
    let (_, got) = lookup("3");
    assert_eq!(
        got,
        RefResolution::NotFound,
        "row 3 of the listing is not Space 3"
    );
    let (_, got) = lookup("5");
    assert!(matches!(got, RefResolution::Space(row) if row.logical_name == "fifth"));
    // Non-canonical digits are invalid refs, not names and not indices.
    for bad in ["0", "01", "b0", "b01"] {
        assert!(parse_ref(bad).is_err(), "{bad}");
    }
    // The selector grammar is closed: a shape or the literal `--name`.
    fn exhaustive(selector: SpaceSelector<'_>) -> &'static str {
        match selector {
            SpaceSelector::Shape(_) => "shape",
            SpaceSelector::ExactName(_) => "exact name",
        }
    }
    assert_eq!(exhaustive(SpaceSelector::ExactName("x")), "exact name");
}

/// The production host-token rule for verbs reading the host table: only
/// enrolled rows answer, by exact alias or label, uniquely.
#[test]
fn enrolled_host_resolution_rules() {
    let row = |uid: HostUid, alias: &str, label: Option<&str>, lifecycle| HostRow {
        host_uid: uid,
        alias: Some(alias.into()),
        label: label.map(str::to_string),
        lifecycle,
        enrolled_at: String::new(),
        tombstoned_at: None,
    };
    let gone = HostUid(uuid::Uuid::from_u128(0xc));
    let table = vec![
        row(LOCAL, "a", Some("macie"), HostLifecycle::Enrolled),
        row(REMOTE, "b", Some("archie"), HostLifecycle::Enrolled),
        row(gone, "c", Some("old"), HostLifecycle::Tombstoned),
    ];
    let alias = |s: &str| HostToken::AliasOrLabel(s.into());
    assert_eq!(resolve_enrolled_host(&table, &alias("a")).unwrap(), LOCAL);
    assert_eq!(
        resolve_enrolled_host(&table, &alias("macie")).unwrap(),
        LOCAL
    );
    assert_eq!(resolve_enrolled_host(&table, &alias("b")).unwrap(), REMOTE);
    assert_eq!(
        resolve_enrolled_host(&table, &alias("archie")).unwrap(),
        REMOTE
    );
    assert_eq!(
        resolve_enrolled_host(&table, &HostToken::Uid(REMOTE)).unwrap(),
        REMOTE
    );
    for token in [
        alias("c"),
        alias("old"),
        HostToken::Uid(gone),
        alias("nobody"),
    ] {
        let error = resolve_enrolled_host(&table, &token).unwrap_err();
        assert_eq!(error.code, ErrorCode::NotFound, "{token:?}");
    }
    // Labels are case-sensitive and exact.
    assert!(resolve_enrolled_host(&table, &alias("Archie")).is_err());
    let mut twice = table.clone();
    twice.push(row(gone, "d", Some("archie"), HostLifecycle::Enrolled));
    assert_eq!(
        resolve_enrolled_host(&twice, &alias("archie"))
            .unwrap_err()
            .code,
        ErrorCode::AmbiguousTarget
    );
}
