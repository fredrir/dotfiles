//! Typed errors, JSON error codes, and the exit-status mapping.
//! Plan §16.3 (exit statuses), §16.2 (JSON contract).
//!
//! The exit table is contract: 0 success/idempotent no-op, 1 backend/internal
//! failure, 2 usage/validation, 3 not found/deleted, 4 ambiguity/conflict,
//! 5 confirmation required or declined, 6 unavailable/auth/protocol/version,
//! 7 partial.

use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ExitStatus {
    Success = 0,
    OperationFailure = 1,
    Usage = 2,
    NotFound = 3,
    Conflict = 4,
    ConfirmationRequired = 5,
    Unavailable = 6,
    Partial = 7,
}

impl ExitStatus {
    pub fn code(self) -> u8 {
        self as u8
    }
}

/// Stable machine-readable error codes for the JSON contract. More specific
/// than exit statuses; each maps to exactly one exit status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ErrorCode {
    // 2 — usage/validation
    Usage,
    InvalidRef,
    InvalidName,
    // 3 — target not found/deleted
    NotFound,
    SpaceAbsent,
    SpaceDeleted,
    // 4 — ambiguity, name conflict, backend mismatch, identity conflict
    AmbiguousTarget,
    NameConflict,
    BackendMismatch,
    IdentityConflict,
    RepairRequired,
    /// P2 additive: another unfinished operation/lease holder owns the target.
    OperationInProgress,
    /// P2 additive: idempotency-key reuse with a different payload digest.
    IdempotencyReuse,
    // 5 — confirmation
    ConfirmationRequired,
    ConfirmationDeclined,
    // 6 — host/route/provider unavailable, auth, protocol, version
    ProviderUnavailable,
    RouteUnavailable,
    BridgeUnavailable,
    AuthFailed,
    HostIdentityChanged,
    VersionMismatch,
    ProtocolMismatch,
    // 1 — backend/internal operation failure
    OperationFailed,
    RegistryBusy,
    BackendEpochChanged,
    WrongBackendInstance,
    PostconditionFailed,
    // 7 — partial success
    PartialResult,
}

impl ErrorCode {
    pub fn exit_status(self) -> ExitStatus {
        use ErrorCode::*;
        match self {
            Usage | InvalidRef | InvalidName => ExitStatus::Usage,
            NotFound | SpaceAbsent | SpaceDeleted => ExitStatus::NotFound,
            AmbiguousTarget | NameConflict | BackendMismatch | IdentityConflict
            | RepairRequired | OperationInProgress | IdempotencyReuse => ExitStatus::Conflict,
            ConfirmationRequired | ConfirmationDeclined => ExitStatus::ConfirmationRequired,
            ProviderUnavailable | RouteUnavailable | BridgeUnavailable | AuthFailed
            | HostIdentityChanged | VersionMismatch | ProtocolMismatch => ExitStatus::Unavailable,
            OperationFailed | RegistryBusy | BackendEpochChanged | WrongBackendInstance
            | PostconditionFailed => ExitStatus::OperationFailure,
            PartialResult => ExitStatus::Partial,
        }
    }

    /// The exact snake_case token used in JSON `errors[]` documents.
    pub fn as_str(self) -> &'static str {
        use ErrorCode::*;
        match self {
            Usage => "usage",
            InvalidRef => "invalid_ref",
            InvalidName => "invalid_name",
            NotFound => "not_found",
            SpaceAbsent => "space_absent",
            SpaceDeleted => "space_deleted",
            AmbiguousTarget => "ambiguous_target",
            NameConflict => "name_conflict",
            BackendMismatch => "backend_mismatch",
            IdentityConflict => "identity_conflict",
            RepairRequired => "repair_required",
            OperationInProgress => "operation_in_progress",
            IdempotencyReuse => "idempotency_reuse",
            ConfirmationRequired => "confirmation_required",
            ConfirmationDeclined => "confirmation_declined",
            ProviderUnavailable => "provider_unavailable",
            RouteUnavailable => "route_unavailable",
            BridgeUnavailable => "bridge_unavailable",
            AuthFailed => "auth_failed",
            HostIdentityChanged => "host_identity_changed",
            VersionMismatch => "version_mismatch",
            ProtocolMismatch => "protocol_mismatch",
            OperationFailed => "operation_failed",
            RegistryBusy => "registry_busy",
            BackendEpochChanged => "backend_epoch_changed",
            WrongBackendInstance => "wrong_backend_instance",
            PostconditionFailed => "postcondition_failed",
            PartialResult => "partial_result",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One typed error as it appears in JSON `errors[]` (plan §16.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedError {
    pub code: ErrorCode,
    pub message: String,
    /// Stable ref or native ref the error is about, when one exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

impl TypedError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        TypedError {
            code,
            message: message.into(),
            target: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_maps_into_the_plan_exit_table() {
        use ErrorCode::*;
        let table: &[(ErrorCode, u8)] = &[
            (Usage, 2),
            (InvalidRef, 2),
            (NotFound, 3),
            (SpaceAbsent, 3),
            (AmbiguousTarget, 4),
            (NameConflict, 4),
            (BackendMismatch, 4),
            (IdentityConflict, 4),
            (ConfirmationRequired, 5),
            (ConfirmationDeclined, 5),
            (ProviderUnavailable, 6),
            (AuthFailed, 6),
            (VersionMismatch, 6),
            (ProtocolMismatch, 6),
            (HostIdentityChanged, 6),
            (BridgeUnavailable, 6),
            (OperationFailed, 1),
            (RegistryBusy, 1),
            (BackendEpochChanged, 1),
            (WrongBackendInstance, 1),
            (PartialResult, 7),
        ];
        for &(code, exit) in table {
            assert_eq!(code.exit_status().code(), exit, "{code:?}");
        }
    }

    #[test]
    fn p2_additive_codes_map_to_conflict() {
        // P2 registry additions: both are "someone else owns this" conflicts.
        for code in [ErrorCode::OperationInProgress, ErrorCode::IdempotencyReuse] {
            assert_eq!(code.exit_status().code(), 4, "{code:?}");
        }
        assert_eq!(
            ErrorCode::OperationInProgress.as_str(),
            "operation_in_progress"
        );
        assert_eq!(ErrorCode::IdempotencyReuse.as_str(), "idempotency_reuse");
    }

    #[test]
    fn codes_serialize_to_snake_case_tokens() {
        let doc = serde_json::to_value(TypedError::new(
            ErrorCode::BackendEpochChanged,
            "epoch changed mid-operation",
        ))
        .unwrap();
        assert_eq!(doc["code"], "backend_epoch_changed");
        assert!(doc.get("target").is_none());
        let back: TypedError = serde_json::from_value(doc).unwrap();
        assert_eq!(back.code, ErrorCode::BackendEpochChanged);
    }
}
