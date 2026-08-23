//! Exact resolution: the §8.2 partition and durable-registry-plus-live
//! lookup. No fuzzy matching, no cross-host search, no creation here —
//! `lookup_for_new` returns a typed decision that policy/operations act on.
//!
//! Root-owned (plan §19, W3).

use crate::backend::InventoryOutcome;
use crate::error::{ErrorCode, TypedError};
use crate::inventory::BackendScans;
use crate::model::{Backend, Health, HostUid, Lifecycle, SpaceNo, SpaceUid};
use crate::refs::{HostToken, SpaceRefShape};
use crate::registry::{BindingRow, BindingState, HostLifecycle, HostRow, SpaceRow};

/// What the live scan showed for one bound record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveState {
    Live {
        multi_window: bool,
    },
    /// A complete inventory that does not contain the bound token.
    AbsentInComplete,
    /// Owner-proven stopped server.
    Stopped,
    /// Any indeterminate outcome — establishes nothing (plan §2.10).
    Indeterminate,
}

/// Why a record is `blocking` rather than `selectable` (plan §8.2 step 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockReason {
    LifecycleReserved,
    LifecycleDeleting,
    LifecycleConflict,
    OperationInProgress,
    Unhealthy(Health),
    /// Active record with no current native binding.
    NoBinding,
    /// Active + bound but missing from a complete inventory.
    ActiveAbsent,
    /// Active + bound but the owner's server is proven stopped. §8.2: dmux
    /// may start the fixed service and repeat the partition (P6); until a
    /// healthy live binding reappears this blocks.
    ServerStopped,
    /// The scan could not establish the record's state.
    IndeterminateObservation,
    /// An unmanaged native resource carries the same name.
    UnmanagedSameName,
    /// Live but spanning multiple native mux windows (plan §2.3).
    MultiWindow,
}

impl BlockReason {
    pub fn error_code(self) -> ErrorCode {
        use BlockReason::*;
        match self {
            LifecycleReserved | LifecycleDeleting | OperationInProgress => {
                ErrorCode::OperationInProgress
            }
            LifecycleConflict => ErrorCode::IdentityConflict,
            ActiveAbsent => ErrorCode::SpaceAbsent,
            ServerStopped | IndeterminateObservation => ErrorCode::ProviderUnavailable,
            Unhealthy(_) | NoBinding | MultiWindow => ErrorCode::RepairRequired,
            UnmanagedSameName => ErrorCode::RepairRequired,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionClass {
    /// Managed active + live + healthy with one verified binding.
    Selectable,
    Blocking(BlockReason),
    /// deleted/aborted: never matches, identifiers stay unavailable.
    Terminal,
}

/// Classify one durable record against its live state (plan §8.2 step 5).
pub fn classify_record(
    space: &SpaceRow,
    has_current_binding: bool,
    live: LiveState,
    unfinished_operation: bool,
) -> PartitionClass {
    use PartitionClass::*;
    if space.lifecycle.is_terminal() {
        return Terminal;
    }
    if unfinished_operation {
        return Blocking(BlockReason::OperationInProgress);
    }
    match space.lifecycle {
        Lifecycle::Reserved => return Blocking(BlockReason::LifecycleReserved),
        Lifecycle::Deleting => return Blocking(BlockReason::LifecycleDeleting),
        Lifecycle::Conflict => return Blocking(BlockReason::LifecycleConflict),
        Lifecycle::Active => {}
        Lifecycle::Deleted | Lifecycle::Aborted => unreachable!("terminal handled above"),
    }
    if space.health != Health::Healthy {
        return Blocking(BlockReason::Unhealthy(space.health));
    }
    if !has_current_binding {
        return Blocking(BlockReason::NoBinding);
    }
    match live {
        LiveState::Live {
            multi_window: false,
        } => Selectable,
        LiveState::Live { multi_window: true } => Blocking(BlockReason::MultiWindow),
        LiveState::AbsentInComplete => Blocking(BlockReason::ActiveAbsent),
        LiveState::Stopped => Blocking(BlockReason::ServerStopped),
        LiveState::Indeterminate => Blocking(BlockReason::IndeterminateObservation),
    }
}

/// One backend's exact-name view, summarized for the decision table. The
/// registry's live-name unique index guarantees at most one occupying
/// (non-terminal) record per name per instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassSummary {
    /// Determinate scan, no occupying record, no unmanaged same-name row.
    NoMatch,
    Selectable {
        space: SpaceUid,
        no: SpaceNo,
    },
    Blocking {
        reason: BlockReason,
        space: Option<SpaceUid>,
    },
    /// The provider outcome was indeterminate: existence cannot be ruled
    /// in or out.
    Indeterminate,
}

/// Build a backend's summary from durable candidates plus the scan outcome.
/// `candidates` are this backend's records whose logical name equals the
/// operand exactly (case-sensitive bytes), with binding and unfinished-op
/// state; `unmanaged_same_name` counts complete-scan rows carrying the name
/// with no current binding.
pub fn summarize_backend(
    scan: &InventoryOutcome,
    candidates: &[(SpaceRow, Option<BindingRow>, bool)],
    name: &str,
) -> ClassSummary {
    if !scan.is_determinate() {
        return ClassSummary::Indeterminate;
    }
    let mut result = ClassSummary::NoMatch;
    for (space, binding, unfinished) in candidates {
        debug_assert_eq!(space.logical_name, name);
        let current = binding
            .as_ref()
            .filter(|b| b.binding_state == BindingState::Current);
        let live = match (scan, current) {
            (InventoryOutcome::Complete(inv), Some(b)) => {
                match inv.rows.iter().find(|r| r.native_token == b.native_token) {
                    Some(row) => LiveState::Live {
                        multi_window: row.multi_window,
                    },
                    None => LiveState::AbsentInComplete,
                }
            }
            (InventoryOutcome::Complete(_), None) => LiveState::AbsentInComplete,
            (InventoryOutcome::ServerStopped { .. }, _) => LiveState::Stopped,
            _ => unreachable!("determinate checked above"),
        };
        match classify_record(space, current.is_some(), live, *unfinished) {
            PartitionClass::Terminal => {}
            PartitionClass::Selectable => {
                result = ClassSummary::Selectable {
                    space: space.space_uid,
                    no: space.space_no,
                };
            }
            PartitionClass::Blocking(reason) => {
                // Blocking outranks selectable within one backend: the plan
                // returns the typed state error rather than counting a
                // second record as "the one match".
                return ClassSummary::Blocking {
                    reason,
                    space: Some(space.space_uid),
                };
            }
        }
    }
    if let InventoryOutcome::Complete(inv) = scan {
        let unmanaged = inv.rows.iter().any(|r| {
            r.native_name == name
                && !candidates.iter().any(|(_, b, _)| {
                    b.as_ref().is_some_and(|b| {
                        b.binding_state == BindingState::Current && b.native_token == r.native_token
                    })
                })
        });
        // An unmanaged same-name row blocks even beside a selectable
        // managed record: allocation/selection cannot ignore it.
        if unmanaged {
            return ClassSummary::Blocking {
                reason: BlockReason::UnmanagedSameName,
                space: None,
            };
        }
    }
    result
}

/// The typed outcome of the `new`-style exact lookup (plan §8.2 steps 5–7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewLookup {
    /// Exact existing match: connect, never create.
    Connect {
        backend: Backend,
        space: SpaceUid,
        no: SpaceNo,
    },
    /// Two selectable results under auto: error listing both stable refs.
    Ambiguous {
        wez: (SpaceUid, SpaceNo),
        tmux: (SpaceUid, SpaceNo),
    },
    /// A blocking result in the relevant scope: its typed state error.
    Blocked {
        backend: Backend,
        reason: BlockReason,
        space: Option<SpaceUid>,
    },
    /// A required inventory was indeterminate: neither select nor create.
    Indeterminate { backend: Backend },
    /// Explicit backend, no match there, opposite backend has the name.
    OppositeNameConflict {
        existing_backend: Backend,
        space: SpaceUid,
    },
    /// No match anywhere (or explicitly acknowledged collision): creation
    /// policy may proceed. `constraint` is `None` for auto.
    ProceedCreate { constraint: Option<Backend> },
}

pub fn lookup_for_new(
    constraint: Option<Backend>,
    allow_collision: bool,
    wez: ClassSummary,
    tmux: ClassSummary,
) -> NewLookup {
    use ClassSummary as C;
    match constraint {
        // Auto (§8.2 step 6): blocking anywhere wins; then any
        // indeterminacy refuses (one known match plus one indeterminate
        // provider cannot exclude ambiguity); then pure selection.
        None => {
            for (backend, side) in [(Backend::Wez, wez), (Backend::Tmux, tmux)] {
                if let C::Blocking { reason, space } = side {
                    return NewLookup::Blocked {
                        backend,
                        reason,
                        space,
                    };
                }
            }
            for (backend, side) in [(Backend::Wez, wez), (Backend::Tmux, tmux)] {
                if side == C::Indeterminate {
                    return NewLookup::Indeterminate { backend };
                }
            }
            match (wez, tmux) {
                (C::Selectable { space: w, no: wn }, C::Selectable { space: t, no: tn }) => {
                    NewLookup::Ambiguous {
                        wez: (w, wn),
                        tmux: (t, tn),
                    }
                }
                (C::Selectable { space, no }, C::NoMatch) => NewLookup::Connect {
                    backend: Backend::Wez,
                    space,
                    no,
                },
                (C::NoMatch, C::Selectable { space, no }) => NewLookup::Connect {
                    backend: Backend::Tmux,
                    space,
                    no,
                },
                (C::NoMatch, C::NoMatch) => NewLookup::ProceedCreate { constraint: None },
                _ => unreachable!("blocking and indeterminate handled above"),
            }
        }
        // Explicit backend B (§8.2 step 7).
        Some(b) => {
            let (mine, other, other_backend) = match b {
                Backend::Wez => (wez, tmux, Backend::Tmux),
                Backend::Tmux => (tmux, wez, Backend::Wez),
            };
            match mine {
                C::Indeterminate => NewLookup::Indeterminate { backend: b },
                C::Blocking { reason, space } => NewLookup::Blocked {
                    backend: b,
                    reason,
                    space,
                },
                // The backend constraint is authoritative for this
                // noncreating selection, irrespective of the opposite
                // provider's state.
                C::Selectable { space, no } => NewLookup::Connect {
                    backend: b,
                    space,
                    no,
                },
                C::NoMatch => match other {
                    // Any path that may create requires both determinate;
                    // --allow-name-collision never waives that.
                    C::Indeterminate => NewLookup::Indeterminate {
                        backend: other_backend,
                    },
                    C::Blocking { reason, space } => NewLookup::Blocked {
                        backend: other_backend,
                        reason,
                        space,
                    },
                    C::Selectable { space, .. } if !allow_collision => {
                        NewLookup::OppositeNameConflict {
                            existing_backend: other_backend,
                            space,
                        }
                    }
                    C::Selectable { .. } | C::NoMatch => NewLookup::ProceedCreate {
                        constraint: Some(b),
                    },
                },
            }
        }
    }
}

/// Convenience: full lookup from raw parts.
pub fn lookup_name(
    name: &str,
    constraint: Option<Backend>,
    allow_collision: bool,
    scans: &BackendScans,
    wez_candidates: &[(SpaceRow, Option<BindingRow>, bool)],
    tmux_candidates: &[(SpaceRow, Option<BindingRow>, bool)],
) -> NewLookup {
    let wez = summarize_backend(&scans.wez, wez_candidates, name);
    let tmux = summarize_backend(&scans.tmux, tmux_candidates, name);
    lookup_for_new(constraint, allow_collision, wez, tmux)
}

// ---------------------------------------------------------------------------
// Space-ref resolution: §6.2's precedence, in one place (ADR 012 WS-D.3).
//
// `refs::parse_ref` classifies a spelling structurally (the seven-step
// parsing precedence). Everything after that — which owner a shape names,
// how `--host` and the local authority default in, what the `--name` escape
// means, and how a locator is looked up on that owner — lives here and
// nowhere else. Verbs that look the Space up themselves call
// [`resolve_space_ref`]; verbs that hand the lookup to the owner authority
// (`con`, `rm` — the owner may be remote) call [`scope_space_ref`] and pass
// the [`ScopedSpaceRef`] on as their query. `tests/resolver_truth_table.rs`
// drives exactly these entry points, so it vouches for production.

/// Exact owner-side lookup locator. `Uid` and `Number` are stable across
/// rename; `Name` is exact, case-sensitive bytes on exactly one owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerLocator {
    Uid(SpaceUid),
    Number(SpaceNo),
    Name(String),
}

impl OwnerLocator {
    /// Whether a Space carrying these identity fields is the one this
    /// locator names. Lifecycle is the lookup's question, not this one's.
    pub fn matches(&self, space_uid: SpaceUid, space_no: SpaceNo, logical_name: &str) -> bool {
        match self {
            OwnerLocator::Uid(uid) => space_uid == *uid,
            OwnerLocator::Number(no) => space_no == *no,
            OwnerLocator::Name(name) => logical_name == name,
        }
    }
}

/// What a verb was handed to select a Space with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceSelector<'a> {
    /// A §6.2 ref after structural parsing.
    Shape(&'a SpaceRefShape),
    /// The `--name` escape ("external legacy names remain operable by stable
    /// ID or an explicit `--name` selector"): the literal name, never
    /// structurally parsed, on the explicit or local owner only — bare names
    /// are never searched across hosts.
    ExactName(&'a str),
}

/// The hosts §6.2's defaulting reads: the local authority `a`, and an
/// explicit `--host` already resolved to an enrolled owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostContext {
    pub local: HostUid,
    pub explicit: Option<HostUid>,
}

impl HostContext {
    /// The owner of a selector that encodes none: "then explicit `--host`,
    /// otherwise … local authority `a`" (§6.2).
    pub fn default_owner(&self) -> HostUid {
        self.explicit.unwrap_or(self.local)
    }
}

/// A selector after host scoping: one owner and the exact locator on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedSpaceRef {
    pub owner: HostUid,
    pub locator: OwnerLocator,
}

/// The host a shape encodes, before resolution: a canonical URI names its
/// owner by `HostUid`; the other shapes carry a token or nothing.
pub fn embedded_host(shape: &SpaceRefShape) -> Option<HostToken> {
    match shape {
        SpaceRefShape::Canonical { host, .. } => Some(HostToken::Uid(*host)),
        SpaceRefShape::Numbered { host, .. } | SpaceRefShape::Named { host, .. } => host.clone(),
    }
}

/// The locator a shape carries, whichever host it names.
fn shape_locator(shape: &SpaceRefShape) -> OwnerLocator {
    match shape {
        SpaceRefShape::Canonical { space, .. } => OwnerLocator::Uid(*space),
        SpaceRefShape::Numbered { no, .. } => OwnerLocator::Number(*no),
        SpaceRefShape::Named { name, .. } => OwnerLocator::Name(name.clone()),
    }
}

/// `--host` beside an encoded owner must agree with it. §6.2's "an encoded
/// ref wins, then explicit `--host`" orders the *defaults*; two different
/// owners named at once is a contradiction, refused rather than resolved
/// either way.
pub fn require_consistent_owner(
    explicit: Option<HostUid>,
    embedded: Option<HostUid>,
) -> Result<(), TypedError> {
    if let (Some(explicit), Some(embedded)) = (explicit, embedded)
        && explicit != embedded
    {
        return Err(TypedError::new(
            ErrorCode::InvalidRef,
            format!(
                "--host owner {} contradicts reference owner {}",
                explicit.0, embedded.0
            ),
        ));
    }
    Ok(())
}

/// §6.2 host scoping for one selector. `resolve_host` turns an embedded
/// token into an enrolled owner — `a` is whatever the host table says the
/// local authority is, never a literal here — and its error is the answer
/// for an unknown or tombstoned token: never a fallback to a logical name.
pub fn scope_space_ref(
    selector: SpaceSelector<'_>,
    context: HostContext,
    mut resolve_host: impl FnMut(&HostToken) -> Result<HostUid, TypedError>,
) -> Result<ScopedSpaceRef, TypedError> {
    match selector {
        SpaceSelector::ExactName(name) => {
            if name.is_empty() {
                return Err(TypedError::new(
                    ErrorCode::InvalidRef,
                    "exact Space name cannot be empty",
                ));
            }
            Ok(ScopedSpaceRef {
                owner: context.default_owner(),
                locator: OwnerLocator::Name(name.to_string()),
            })
        }
        SpaceSelector::Shape(shape) => {
            let embedded = embedded_host(shape)
                .map(|token| resolve_host(&token))
                .transpose()?;
            require_consistent_owner(context.explicit, embedded)?;
            Ok(ScopedSpaceRef {
                owner: embedded.unwrap_or_else(|| context.default_owner()),
                locator: shape_locator(shape),
            })
        }
    }
}

/// One Space as the lookup sees it: a durable row, a live-correlated marker,
/// or an owner's remote answer.
pub trait SpaceCandidate {
    fn space_uid(&self) -> SpaceUid;
    fn space_no(&self) -> SpaceNo;
    fn logical_name(&self) -> &str;
    fn lifecycle(&self) -> Lifecycle;
}

impl SpaceCandidate for SpaceRow {
    fn space_uid(&self) -> SpaceUid {
        self.space_uid
    }

    fn space_no(&self) -> SpaceNo {
        self.space_no
    }

    fn logical_name(&self) -> &str {
        &self.logical_name
    }

    fn lifecycle(&self) -> Lifecycle {
        self.lifecycle
    }
}

/// The outcome of looking a locator up on its owner. The matched candidate
/// comes back whole, so the verb applies its own lifecycle gate (a child verb
/// wants `active`; `rm` may finish a `deleting` row) without a second search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefResolution<C> {
    Space(C),
    /// The ref names a terminal record: exit 3 as `space_deleted`, distinct
    /// from not-found — a retired number or UID is never free (§6.1).
    Deleted(C),
    NotFound,
    /// More than one live Space answers — a name held on both backends.
    /// Never guessed; the caller says to use the number.
    AmbiguousName(Vec<C>),
}

/// Look an owner-scoped locator up in that owner's candidates. Names of
/// terminal records are free (a deleted `proj` is not `proj`); numbers and
/// UIDs are permanent, so a terminal match is reported as such.
pub fn resolve_locator<C: SpaceCandidate>(
    locator: &OwnerLocator,
    candidates: Vec<C>,
) -> RefResolution<C> {
    let (live, terminal): (Vec<C>, Vec<C>) = candidates
        .into_iter()
        .filter(|c| locator.matches(c.space_uid(), c.space_no(), c.logical_name()))
        .partition(|c| !c.lifecycle().is_terminal());
    match live.len() {
        1 => RefResolution::Space(live.into_iter().next().expect("one live match")),
        n if n > 1 => RefResolution::AmbiguousName(live),
        _ => match locator {
            OwnerLocator::Name(_) => RefResolution::NotFound,
            OwnerLocator::Uid(_) | OwnerLocator::Number(_) => match terminal.into_iter().next() {
                Some(retired) => RefResolution::Deleted(retired),
                None => RefResolution::NotFound,
            },
        },
    }
}

/// The one resolver: scope the selector, fetch that owner's candidates, look
/// the locator up. `candidates` receives the scoped owner so a verb can read
/// the local registry or ask a remote owner — whichever §6.2 selected.
pub fn resolve_space_ref<C: SpaceCandidate>(
    selector: SpaceSelector<'_>,
    context: HostContext,
    resolve_host: impl FnMut(&HostToken) -> Result<HostUid, TypedError>,
    candidates: impl FnOnce(HostUid) -> Result<Vec<C>, TypedError>,
) -> Result<(ScopedSpaceRef, RefResolution<C>), TypedError> {
    let scoped = scope_space_ref(selector, context, resolve_host)?;
    let resolution = resolve_locator(&scoped.locator, candidates(scoped.owner)?);
    Ok((scoped, resolution))
}

/// The production host-token rule for a verb that reads the host table
/// itself: enrolled rows only; a `HostUid` must be enrolled; an alias or
/// label spelling must match exactly one enrolled owner (§6.2: labels and
/// aliases are never rebound, and `a` is minted for the local authority at
/// registry open).
pub fn resolve_enrolled_host(hosts: &[HostRow], token: &HostToken) -> Result<HostUid, TypedError> {
    let spelling = match token {
        HostToken::Uid(uid) => uid.0.to_string(),
        HostToken::AliasOrLabel(spelling) => spelling.clone(),
    };
    let matches: Vec<&HostRow> = hosts
        .iter()
        .filter(|host| host.lifecycle == HostLifecycle::Enrolled)
        .filter(|host| match token {
            HostToken::Uid(uid) => host.host_uid == *uid,
            HostToken::AliasOrLabel(spelling) => {
                host.alias.as_deref() == Some(spelling) || host.label.as_deref() == Some(spelling)
            }
        })
        .collect();
    match matches.as_slice() {
        [one] => Ok(one.host_uid),
        [] => Err(TypedError::new(
            ErrorCode::NotFound,
            format!("no enrolled host matches {spelling:?}"),
        )),
        _ => Err(TypedError::new(
            ErrorCode::AmbiguousTarget,
            format!("host spelling {spelling:?} matches more than one enrolled owner"),
        )),
    }
}
