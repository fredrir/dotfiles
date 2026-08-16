//! Exact resolution: the §8.2 partition and durable-registry-plus-live
//! lookup. No fuzzy matching, no cross-host search, no creation here —
//! `lookup_for_new` returns a typed decision that policy/operations act on.
//!
//! Root-owned (plan §19, W3).

use crate::backend::InventoryOutcome;
use crate::error::ErrorCode;
use crate::inventory::BackendScans;
use crate::model::{Backend, Health, HostUid, Lifecycle, SpaceNo, SpaceUid};
use crate::refs::{HostToken, SpaceRefShape};
use crate::registry::{BindingRow, BindingState, SpaceRow};

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
// Space-ref resolution against the local registry (shadow scope: local
// authority only; enrolled-remote scopes arrive with P7).

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefResolution {
    Space(SpaceUid),
    /// The ref matched a terminal record: exit 3, distinct from not-found.
    Deleted(SpaceUid),
    NotFound,
    /// Bare/host-qualified name matching more than one backend's Space.
    AmbiguousName(Vec<SpaceUid>),
    /// Host tokens beyond the local authority are not resolvable in shadow
    /// mode (P7 adds enrollment).
    UnsupportedHostScope,
}

pub fn resolve_space_ref(
    shape: &SpaceRefShape,
    local_host: HostUid,
    spaces: &[SpaceRow],
) -> RefResolution {
    let local_token_ok = |host: &Option<HostToken>| match host {
        None => true,
        Some(HostToken::Uid(uid)) => *uid == local_host,
        Some(HostToken::AliasOrLabel(t)) => t == "a",
    };
    match shape {
        SpaceRefShape::Canonical { host, space } => {
            if *host != local_host {
                return RefResolution::UnsupportedHostScope;
            }
            match spaces.iter().find(|s| s.space_uid == *space) {
                Some(s) if s.lifecycle.is_terminal() => RefResolution::Deleted(s.space_uid),
                Some(s) => RefResolution::Space(s.space_uid),
                None => RefResolution::NotFound,
            }
        }
        SpaceRefShape::Numbered { host, no } => {
            if !local_token_ok(host) {
                return RefResolution::UnsupportedHostScope;
            }
            // SpaceNo is never reused, so at most one record matches.
            match spaces.iter().find(|s| s.space_no == *no) {
                Some(s) if s.lifecycle.is_terminal() => RefResolution::Deleted(s.space_uid),
                Some(s) => RefResolution::Space(s.space_uid),
                None => RefResolution::NotFound,
            }
        }
        SpaceRefShape::Named { host, name } => {
            if !local_token_ok(host) {
                return RefResolution::UnsupportedHostScope;
            }
            let matches: Vec<&SpaceRow> = spaces
                .iter()
                .filter(|s| !s.lifecycle.is_terminal() && s.logical_name == *name)
                .collect();
            match matches.as_slice() {
                [] => RefResolution::NotFound,
                [one] => RefResolution::Space(one.space_uid),
                many => RefResolution::AmbiguousName(many.iter().map(|s| s.space_uid).collect()),
            }
        }
    }
}
