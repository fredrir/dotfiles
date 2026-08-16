//! Automatic creation decision and its explanation (plan §8.3, §8.2 steps
//! 6–7). Pure: callers assemble the observed context; nothing here probes.
//!
//! Root-owned (plan §19, W3).

use crate::error::{ErrorCode, TypedError};
use crate::model::Backend;
use crate::refs::{NameError, validate_new_name};
use crate::resolve::NewLookup;

/// Eligibility state of the preferred (USB) route at decision time.
/// The distinction is load-bearing (plan §8.3): only a POSITIVE absence
/// observation permits automatic tmux; probe failures refuse instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteState {
    /// Verified end-to-end: domain operations, SSH identity, auth, remote
    /// dmux protocol, Wez version compatibility, non-creating preflight.
    PositivelyUsable,
    /// Positively observed `route_absent`/`usb_link_down` (no enrolled USB
    /// route or an authoritative link-state signal).
    PositivelyAbsent,
    /// DNS failure, refusal/reset, timeout — NOT proof of "unwired".
    ProbeFailed,
    /// Link usable but Wez auth/version/protocol failed (acceptance 19):
    /// exit 6, create neither backend.
    AuthOrCompatFailed,
}

/// The local half of the §8.3 table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalEnv {
    /// A live trusted GUI bridge/controller exists.
    pub trusted_gui_bridge: bool,
    /// The persistent unix mux service is present and version-compatible.
    pub wez_service_compatible: bool,
}

/// The remote half; `None` context means the target host is local.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteEnv {
    /// Plain/headless SSH session — always tmux, even over a live cable.
    pub plain_ssh: bool,
    pub trusted_wez_controller: bool,
    pub usb: RouteState,
    /// An explicitly usable verified alternate route (e.g. Tailscale) —
    /// only consulted for explicit `--backend wez` (plan §8.3 table).
    pub verified_alternate_route: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreationContext {
    pub explicit_backend: Option<Backend>,
    pub local: LocalEnv,
    pub remote: Option<RemoteEnv>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendDecision {
    Create(Backend),
    /// Typed refusal: creation must not proceed on either backend.
    Refuse(ErrorCode, &'static str),
}

/// The §8.3 decision table, with an explanation trace.
pub fn decide_backend(ctx: &CreationContext) -> (BackendDecision, Vec<String>) {
    let mut trace = Vec::new();
    let decision = match (ctx.explicit_backend, &ctx.remote) {
        (Some(Backend::Tmux), _) => {
            trace.push("explicit --backend tmux".into());
            BackendDecision::Create(Backend::Tmux)
        }
        (Some(Backend::Wez), remote) => {
            let usable = match remote {
                None => {
                    trace.push("explicit --backend wez on the local authority".into());
                    ctx.local.trusted_gui_bridge && ctx.local.wez_service_compatible
                }
                Some(r) => {
                    trace.push(
                        "explicit --backend wez requires an explicitly usable verified route \
                         (USB or Tailscale)"
                            .into(),
                    );
                    r.trusted_wez_controller
                        && (r.usb == RouteState::PositivelyUsable || r.verified_alternate_route)
                }
            };
            if usable {
                trace.push("verified Wez route/controller available".into());
                BackendDecision::Create(Backend::Wez)
            } else {
                trace.push("no verified usable Wez route: refuse, never fall back".into());
                BackendDecision::Refuse(
                    ErrorCode::ProviderUnavailable,
                    "explicit wez backend without a verified usable route",
                )
            }
        }
        (None, None) => {
            if ctx.local.trusted_gui_bridge && ctx.local.wez_service_compatible {
                trace.push(
                    "local, trusted Wez controller/bridge, compatible persistent unix mux".into(),
                );
                BackendDecision::Create(Backend::Wez)
            } else {
                trace.push("local plain/headless/untrusted or stale Wez environment".into());
                BackendDecision::Create(Backend::Tmux)
            }
        }
        (None, Some(r)) => {
            if r.plain_ssh {
                trace.push("plain/headless SSH even over a physical USB link".into());
                BackendDecision::Create(Backend::Tmux)
            } else {
                match r.usb {
                    RouteState::PositivelyUsable if r.trusted_wez_controller => {
                        trace.push(
                            "remote, trusted Wez controller, positively usable USB route".into(),
                        );
                        BackendDecision::Create(Backend::Wez)
                    }
                    RouteState::PositivelyUsable => {
                        trace.push(
                            "USB usable but the Wez controller is not trusted: refuse".into(),
                        );
                        BackendDecision::Refuse(
                            ErrorCode::AuthFailed,
                            "untrusted controller on a usable route",
                        )
                    }
                    RouteState::PositivelyAbsent => {
                        trace.push("USB route positively absent/unwired".into());
                        BackendDecision::Create(Backend::Tmux)
                    }
                    RouteState::ProbeFailed => {
                        trace.push(
                            "route probe failed (DNS/refusal/timeout) — not proof of unwired: \
                             refuse rather than choose tmux"
                                .into(),
                        );
                        BackendDecision::Refuse(
                            ErrorCode::RouteUnavailable,
                            "route eligibility indeterminate",
                        )
                    }
                    RouteState::AuthOrCompatFailed => {
                        trace.push(
                            "USB reachable but Wez auth/version/protocol failed: exit 6, \
                             create neither backend"
                                .into(),
                        );
                        BackendDecision::Refuse(
                            ErrorCode::AuthFailed,
                            "wez authentication/compatibility failure",
                        )
                    }
                }
            }
        }
    };
    (decision, trace)
}

// ---------------------------------------------------------------------------
// The combined shadow `new` plan (§8.2 steps 5–7 + §8.3): lookup outcome +
// name validation + creation decision, with a decision explanation. Native
// execution (leases, journals, spawn) is P6.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewPlan {
    Connect {
        backend: Backend,
        space: crate::model::SpaceUid,
    },
    Create {
        backend: Backend,
    },
    Fail(TypedError),
}

pub struct Explained<T> {
    pub value: T,
    pub trace: Vec<String>,
}

pub fn plan_new(name: &str, lookup: NewLookup, ctx: &CreationContext) -> Explained<NewPlan> {
    let mut trace = vec![format!(
        "operand {name:?} treated as literal exact name for lookup"
    )];
    let value = match lookup {
        NewLookup::Connect { backend, space, no } => {
            trace.push(format!(
                "exact existing {backend} match (SpaceNo {no}): connect, never create"
            ));
            NewPlan::Connect { backend, space }
        }
        NewLookup::Ambiguous { wez, tmux } => {
            trace.push("selectable matches on both backends: ambiguous".into());
            NewPlan::Fail(TypedError::new(
                ErrorCode::AmbiguousTarget,
                format!(
                    "name exists on both backends: wez SpaceNo {} and tmux SpaceNo {}; \
                     constrain with --backend",
                    wez.1, tmux.1
                ),
            ))
        }
        NewLookup::Blocked {
            backend, reason, ..
        } => {
            trace.push(format!("blocking {backend} result: {reason:?}"));
            NewPlan::Fail(TypedError::new(
                reason.error_code(),
                format!("{backend} record for this name is not selectable: {reason:?}"),
            ))
        }
        NewLookup::Indeterminate { backend } => {
            trace.push(format!(
                "{backend} inventory indeterminate: cannot exclude an existing Space"
            ));
            NewPlan::Fail(TypedError::new(
                ErrorCode::ProviderUnavailable,
                format!("{backend} inventory unavailable; creation fails closed (plan §2.10)"),
            ))
        }
        NewLookup::OppositeNameConflict {
            existing_backend, ..
        } => {
            trace.push(format!(
                "name exists only on {existing_backend}: conflict without --allow-name-collision"
            ));
            NewPlan::Fail(TypedError::new(
                ErrorCode::NameConflict,
                format!(
                    "name already exists on {existing_backend}; pass --allow-name-collision \
                     to create beside it intentionally"
                ),
            ))
        }
        NewLookup::ProceedCreate { constraint } => {
            debug_assert_eq!(constraint, ctx.explicit_backend);
            match validate_new_name(name) {
                Err(err) => {
                    trace.push(format!("new-name grammar rejected the operand: {err:?}"));
                    NewPlan::Fail(TypedError::new(
                        ErrorCode::InvalidName,
                        name_error_message(name, err),
                    ))
                }
                Ok(()) => {
                    let (decision, mut policy_trace) = decide_backend(ctx);
                    trace.append(&mut policy_trace);
                    match decision {
                        BackendDecision::Create(backend) => NewPlan::Create { backend },
                        BackendDecision::Refuse(code, why) => {
                            NewPlan::Fail(TypedError::new(code, why))
                        }
                    }
                }
            }
        }
    };
    Explained { value, trace }
}

fn name_error_message(name: &str, err: NameError) -> String {
    match err {
        NameError::Empty => "a Space name cannot be empty".into(),
        NameError::TooLong => format!("name {name:?} exceeds 64 characters"),
        NameError::BadStart => format!("name {name:?} must start with a letter"),
        NameError::BadChar => {
            format!("name {name:?} may contain only letters, digits, '_' and '-'")
        }
        NameError::IdShaped => {
            format!("name {name:?} is ID-shaped (letters+digits) and reserved by the ref grammar")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(trusted: bool, compatible: bool) -> CreationContext {
        CreationContext {
            explicit_backend: None,
            local: LocalEnv {
                trusted_gui_bridge: trusted,
                wez_service_compatible: compatible,
            },
            remote: None,
        }
    }

    fn remote(plain_ssh: bool, trusted: bool, usb: RouteState) -> CreationContext {
        CreationContext {
            explicit_backend: None,
            local: LocalEnv {
                trusted_gui_bridge: false,
                wez_service_compatible: false,
            },
            remote: Some(RemoteEnv {
                plain_ssh,
                trusted_wez_controller: trusted,
                usb,
                verified_alternate_route: false,
            }),
        }
    }

    /// The §8.3 table, row by row.
    #[test]
    fn automatic_decision_table() {
        use BackendDecision::*;
        // Local, trusted, compatible → Wez (acceptance 1).
        assert_eq!(decide_backend(&local(true, true)).0, Create(Backend::Wez));
        // Local plain/headless/untrusted/stale → tmux (acceptance 2).
        assert_eq!(decide_backend(&local(false, true)).0, Create(Backend::Tmux));
        assert_eq!(decide_backend(&local(true, false)).0, Create(Backend::Tmux));
        // Remote trusted + usable USB → Wez (acceptance 16).
        assert_eq!(
            decide_backend(&remote(false, true, RouteState::PositivelyUsable)).0,
            Create(Backend::Wez)
        );
        // USB positively absent → tmux (acceptance 17).
        assert_eq!(
            decide_backend(&remote(false, true, RouteState::PositivelyAbsent)).0,
            Create(Backend::Tmux)
        );
        // Plain SSH even over a live cable → tmux (acceptance 18).
        assert_eq!(
            decide_backend(&remote(true, true, RouteState::PositivelyUsable)).0,
            Create(Backend::Tmux)
        );
        // Auth/version failure on a reachable link → refuse, neither backend
        // (acceptance 19).
        let (d, _) = decide_backend(&remote(false, true, RouteState::AuthOrCompatFailed));
        assert!(matches!(d, Refuse(ErrorCode::AuthFailed, _)), "{d:?}");
        // Probe failure is not proof of unwired → refuse, never tmux.
        let (d, _) = decide_backend(&remote(false, true, RouteState::ProbeFailed));
        assert!(matches!(d, Refuse(ErrorCode::RouteUnavailable, _)), "{d:?}");
        // Untrusted controller on a usable route → refuse.
        let (d, _) = decide_backend(&remote(false, false, RouteState::PositivelyUsable));
        assert!(matches!(d, Refuse(ErrorCode::AuthFailed, _)), "{d:?}");
    }

    #[test]
    fn explicit_backend_rows() {
        use BackendDecision::*;
        // Explicit tmux is unconditional.
        let mut ctx = remote(false, false, RouteState::ProbeFailed);
        ctx.explicit_backend = Some(Backend::Tmux);
        assert_eq!(decide_backend(&ctx).0, Create(Backend::Tmux));
        // Explicit wez over a verified alternate (Tailscale) route works.
        let mut ctx = remote(false, true, RouteState::PositivelyAbsent);
        ctx.explicit_backend = Some(Backend::Wez);
        ctx.remote.as_mut().unwrap().verified_alternate_route = true;
        assert_eq!(decide_backend(&ctx).0, Create(Backend::Wez));
        // Explicit wez with no verified route refuses; no tmux fallback.
        let mut ctx = remote(false, true, RouteState::ProbeFailed);
        ctx.explicit_backend = Some(Backend::Wez);
        let (d, _) = decide_backend(&ctx);
        assert!(
            matches!(d, Refuse(ErrorCode::ProviderUnavailable, _)),
            "{d:?}"
        );
    }

    #[test]
    fn plan_new_validates_grammar_only_on_the_creation_path() {
        use crate::model::SpaceUid;
        use uuid::Uuid;
        // ID-shaped operand still CONNECTS to an existing adopted legacy
        // Space (lookup treats the operand as a literal)...
        let existing = NewLookup::Connect {
            backend: Backend::Tmux,
            space: SpaceUid(Uuid::nil()),
            no: crate::model::SpaceNo(std::num::NonZeroU64::new(1).unwrap()),
        };
        let plan = plan_new("proj2", existing, &local(true, true));
        assert!(matches!(plan.value, NewPlan::Connect { .. }));
        // ...but cannot be created fresh.
        let plan = plan_new(
            "proj2",
            NewLookup::ProceedCreate { constraint: None },
            &local(true, true),
        );
        match plan.value {
            NewPlan::Fail(err) => assert_eq!(err.code, ErrorCode::InvalidName),
            other => panic!("expected invalid_name, got {other:?}"),
        }
        // A valid name on the same path creates per policy.
        let plan = plan_new(
            "project",
            NewLookup::ProceedCreate { constraint: None },
            &local(true, true),
        );
        assert_eq!(
            plan.value,
            NewPlan::Create {
                backend: Backend::Wez
            }
        );
        assert!(!plan.trace.is_empty());
    }
}
