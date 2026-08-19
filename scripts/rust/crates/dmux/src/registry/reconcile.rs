//! Crash reconciliation decisions (plan §10.2, §10.3).
//!
//! These are pure, exhaustively-tested decision functions: given a journal
//! row found in a non-terminal state after a crash, they say what a resuming
//! fenced holder must do. They never mutate anything themselves — the caller
//! performs the scan/native work under the §10.1 locks and the advanced
//! fencing token, then commits the outcome through `registry::Registry`.

use crate::model::{OperationKind, OperationState};

/// What a resuming holder must do for one journal row (plan §10.2 step 4:
/// "reconcile the predecessor's journal/postcondition; continue only from a
/// proven state").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeDuty {
    /// Create in prepared/running/unknown: run the complete keyed lookup by
    /// the reserved opaque/native key BEFORE any spawn (a retry after
    /// acknowledgement loss can therefore never create a second native
    /// resource); then classify with [`decide_create`].
    CreateKeyedLookup,
    /// Rename in prepared/running/unknown: observe the exact old/new native
    /// states, then classify with [`decide_rename`]. Never choose silently.
    RenameObserveStates,
    /// Remove in prepared/running/unknown: re-run exact-content removal and
    /// re-query until verified absence or bounded non-convergence; only
    /// verified absence may become `deleted`.
    RemoveVerifyAbsence,
    /// Adopt/rebind/normalize/stamp in prepared/running/unknown: reconcile
    /// by source token, destination opaque key, and epoch (plan §10.3) into
    /// unmanaged, active-unstamped, healthy, or conflict — never silent
    /// success.
    AdoptionReconcile,
    /// Terminal states: nothing to resume.
    Nothing,
}

impl ResumeDuty {
    /// The token `dmux repair reconcile` prints and puts in its §16.2
    /// document, so an operator sees the duty the table assigned and not a
    /// paraphrase of it.
    pub fn as_str(self) -> &'static str {
        match self {
            ResumeDuty::CreateKeyedLookup => "create_keyed_lookup",
            ResumeDuty::RenameObserveStates => "rename_observe_states",
            ResumeDuty::RemoveVerifyAbsence => "remove_verify_absence",
            ResumeDuty::AdoptionReconcile => "adoption_reconcile",
            ResumeDuty::Nothing => "nothing",
        }
    }
}

/// The duty for a journal row of `kind` found in `state`.
pub fn resume_duty(kind: OperationKind, state: OperationState) -> ResumeDuty {
    if state.is_terminal() {
        return ResumeDuty::Nothing;
    }
    match kind {
        OperationKind::Create => ResumeDuty::CreateKeyedLookup,
        OperationKind::Rename => ResumeDuty::RenameObserveStates,
        OperationKind::Remove => ResumeDuty::RemoveVerifyAbsence,
        OperationKind::Adopt
        | OperationKind::Rebind
        | OperationKind::Normalize
        | OperationKind::Stamp => ResumeDuty::AdoptionReconcile,
    }
}

/// Rename reconciliation over the observed native states (plan §10.2:
/// "reconciliation handles old-only, new-only, both, and neither states
/// explicitly; it never chooses silently when both old and new exist").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameDecision {
    /// Old-only: the native rename has not happened; the native step may be
    /// re-run (or the operation aborted) under the same fence.
    RetryNativeRename,
    /// New-only: the native rename happened; commit the registry-side name
    /// change without touching the native resource again.
    CommitRegistryRename,
    /// Both old and new exist: explicit conflict — a silent pick could
    /// destroy an externally created same-named resource.
    ConflictBothExist,
    /// Neither exists: the resource is gone or the scan is wrong — explicit
    /// conflict, never a fabricated success.
    ConflictNeitherExists,
}

pub fn decide_rename(old_exists: bool, new_exists: bool) -> RenameDecision {
    match (old_exists, new_exists) {
        (true, false) => RenameDecision::RetryNativeRename,
        (false, true) => RenameDecision::CommitRegistryRename,
        (true, true) => RenameDecision::ConflictBothExist,
        (false, false) => RenameDecision::ConflictNeitherExists,
    }
}

/// The outcome of the complete keyed lookup a resuming create must run
/// (plan §10.2 create step 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateScan {
    /// Determinate scan, zero matches for the reserved key.
    ZeroMatches,
    /// Exactly one conforming match (right key, one window, no conflicting
    /// binding).
    OneConforming,
    /// The scan was not determinate (partial/unreachable/malformed).
    Indeterminate,
    /// Multiple matches, a multi-window resource, or a conflicting binding.
    MultipleOrConflicting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateDecision {
    /// Zero matches permits exactly one (re-)create under the same fence.
    RetryCreate,
    /// One conforming match is rebound/finalized — never spawned again.
    RebindAndFinalize,
    /// Everything else fails closed (mark unknown/conflict; no native work).
    FailClosed,
}

pub fn decide_create(scan: CreateScan) -> CreateDecision {
    match scan {
        CreateScan::ZeroMatches => CreateDecision::RetryCreate,
        CreateScan::OneConforming => CreateDecision::RebindAndFinalize,
        CreateScan::Indeterminate | CreateScan::MultipleOrConflicting => CreateDecision::FailClosed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_KINDS: [OperationKind; 7] = [
        OperationKind::Create,
        OperationKind::Rename,
        OperationKind::Remove,
        OperationKind::Adopt,
        OperationKind::Rebind,
        OperationKind::Normalize,
        OperationKind::Stamp,
    ];
    const UNFINISHED: [OperationState; 3] = [
        OperationState::Prepared,
        OperationState::Running,
        OperationState::Unknown,
    ];
    const TERMINAL: [OperationState; 4] = [
        OperationState::Completed,
        OperationState::Failed,
        OperationState::Aborted,
        OperationState::Conflict,
    ];

    #[test]
    fn every_nonterminal_state_has_an_explicit_duty() {
        for kind in ALL_KINDS {
            for state in UNFINISHED {
                assert_ne!(
                    resume_duty(kind, state),
                    ResumeDuty::Nothing,
                    "{kind:?} {state:?}"
                );
            }
            for state in TERMINAL {
                assert_eq!(
                    resume_duty(kind, state),
                    ResumeDuty::Nothing,
                    "{kind:?} {state:?}"
                );
            }
        }
    }

    #[test]
    fn rename_truth_table_never_chooses_silently_on_both() {
        assert_eq!(
            decide_rename(true, false),
            RenameDecision::RetryNativeRename
        );
        assert_eq!(
            decide_rename(false, true),
            RenameDecision::CommitRegistryRename
        );
        assert_eq!(decide_rename(true, true), RenameDecision::ConflictBothExist);
        assert_eq!(
            decide_rename(false, false),
            RenameDecision::ConflictNeitherExists
        );
    }

    #[test]
    fn create_scan_fails_closed_unless_proven() {
        assert_eq!(
            decide_create(CreateScan::ZeroMatches),
            CreateDecision::RetryCreate
        );
        assert_eq!(
            decide_create(CreateScan::OneConforming),
            CreateDecision::RebindAndFinalize
        );
        assert_eq!(
            decide_create(CreateScan::Indeterminate),
            CreateDecision::FailClosed
        );
        assert_eq!(
            decide_create(CreateScan::MultipleOrConflicting),
            CreateDecision::FailClosed
        );
    }
}
