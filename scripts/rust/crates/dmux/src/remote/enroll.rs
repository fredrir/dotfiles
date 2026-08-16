//! Host enrollment: `dmux ssh TARGET` (plan §12.2). Normal OpenSSH host-key
//! handling, a fresh nonce-bound `hello` handshake, HostUid matching even on
//! a new address (enrollment is idempotent by HostUid), alias allocation
//! only for a NEW HostUid, an Openssh route record with the network-class
//! heuristic, then an interactive `ssh TARGET` exec.
//!
//! A missing/incompatible remote agent fails with a plain-ssh suggestion;
//! identity is NEVER invented from a hostname or address.

use std::time::Duration;

use uuid::Uuid;

use crate::error::{ErrorCode, TypedError};
use crate::operations::OperationEnv;
use crate::refs::is_valid_host_label;
use crate::registry::{
    EnrolledHost, NetworkClass, PeerCache, Registry, RegistryConfig, RegistryError, RouteRow,
    RouteSpec, Transport, now_rfc3339,
};
use crate::remote::client::{self, AgentInvocation, RouteInvoker, SshInvoker, request_envelope};
use crate::remote::lineage::{self, PeerLineage, PresentedPeer};
use crate::remote::protocol::{self, Envelope, HelloInfo, HelloPayload};
use crate::remote::routes::{classify_endpoint, default_priority, outcome, wez_domain_name};

/// What a completed enrollment recorded.
#[derive(Debug, Clone)]
pub struct Enrollment {
    pub host: EnrolledHost,
    pub route_id: i64,
    pub network_class: NetworkClass,
    pub hello: HelloInfo,
    pub lineage: PeerLineage,
}

/// `dmux ssh TARGET` entry point (frozen signature for the root's CLI
/// wiring). Enrolls/reaffirms the target's authority, records the route,
/// then execs interactive `ssh TARGET`; on failure prints one message and
/// returns the mapped exit status.
pub fn run(target: &str) -> i32 {
    let env = match OperationEnv::production() {
        Ok(env) => env,
        Err(e) => {
            eprintln!("dmux ssh: environment: {e}");
            return i32::from(ErrorCode::OperationFailed.exit_status().code());
        }
    };
    // Enrollment wants NORMAL interactive OpenSSH host-key handling: no
    // BatchMode, so first-contact key confirmation reaches the terminal.
    let invoker = SshInvoker {
        batch_mode: false,
        connect_timeout: Some(15),
        ..SshInvoker::default()
    };
    let invocation = AgentInvocation::new(protocol::methods::HELLO);
    match enroll_target(
        &env,
        target,
        &invoker,
        &invocation,
        client::DEFAULT_DEADLINE,
    ) {
        Ok(enrollment) => {
            eprintln!(
                "dmux: host {} enrolled as alias {:?}{} ({} route {:?})",
                enrollment.hello.host_uid.0,
                enrollment.host.alias,
                enrollment
                    .host
                    .label
                    .as_deref()
                    .map(|l| format!(" label {l:?}"))
                    .unwrap_or_default(),
                enrollment.network_class.as_str(),
                target,
            );
            use std::os::unix::process::CommandExt;
            let error = std::process::Command::new("ssh").arg(target).exec();
            eprintln!("dmux ssh: exec ssh {target}: {error}");
            i32::from(ErrorCode::OperationFailed.exit_status().code())
        }
        Err(error) => {
            if error.code == ErrorCode::ProviderUnavailable {
                eprintln!(
                    "dmux ssh: {}; enrollment needs a compatible remote dmux — \
                     use plain `ssh {target}` instead",
                    error.message
                );
            } else {
                eprintln!("dmux ssh: {}", error.message);
            }
            i32::from(error.code.exit_status().code())
        }
    }
}

/// Everything `run` does except the final interactive exec — the testable
/// seam (two-host tests drive it over real ssh with scratch seams).
pub fn enroll_target(
    env: &OperationEnv,
    target: &str,
    invoker: &dyn RouteInvoker,
    invocation: &AgentInvocation,
    deadline: Duration,
) -> Result<Enrollment, TypedError> {
    let mut registry = Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir))
        .map_err(|e| TypedError::new(e.error_code(), e.to_string()))?;
    let local = registry
        .identity()
        .map_err(|e| TypedError::new(e.error_code(), e.to_string()))?;

    // Fresh nonce-bound hello (§12.1: freshness gates rollback suspicion).
    let (envelope, hello) = fresh_hello(&registry, target, invoker, invocation, deadline)?;
    if hello.host_uid != envelope.host_uid || hello.registry_uid != envelope.registry_uid {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "hello payload identity disagrees with its envelope",
        ));
    }
    if hello.host_uid == local.host_uid {
        return Err(TypedError::new(
            ErrorCode::Usage,
            format!("{target} answered with THIS host's identity; nothing to enroll"),
        ));
    }

    // Lineage against any cached checkpoint for this HostUid.
    let mut assessment = assess_hello(&registry, &envelope, &hello)?;
    if assessment == PeerLineage::RollbackSuspect {
        // Rollback quarantine (§12.1): confirm with a SECOND fresh
        // handshake before refusing.
        let (second_env, second_hello) =
            fresh_hello(&registry, target, invoker, invocation, deadline)?;
        assessment = assess_hello(&registry, &second_env, &second_hello)?;
        if assessment == PeerLineage::RollbackSuspect {
            return Err(TypedError::new(
                ErrorCode::IdentityConflict,
                format!(
                    "two fresh handshakes report a rolled-back/non-descendant authority head \
                     for host {} — refusing to enroll or mutate (rollback quarantine)",
                    hello.host_uid.0
                ),
            ));
        }
    }
    if !assessment.accepts_response() {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            format!("peer lineage conflict for host {}", hello.host_uid.0),
        ));
    }

    // Idempotent by HostUid: an existing host matches even on a new
    // address; the next alias is allocated only for a new HostUid.
    let label = label_candidate(target);
    let enrolled = match registry.enroll_host(hello.host_uid, label) {
        Ok(enrolled) => enrolled,
        Err(RegistryError::SpellingBound { .. } | RegistryError::InvalidLabel { .. }) => registry
            .enroll_host(hello.host_uid, None)
            .map_err(|e| TypedError::new(e.error_code(), e.to_string()))?,
        Err(e) => return Err(TypedError::new(e.error_code(), e.to_string())),
    };

    if assessment.stores_cache() {
        let snapshot = envelope
            .payload
            .clone()
            .unwrap_or_else(|| serde_json::json!({}));
        registry
            .store_peer_cache(
                hello.host_uid,
                &PeerCache {
                    registry_uid: hello.registry_uid,
                    authority_revision: hello.authority_revision,
                    authority_head_hash: hello.authority_head_hash.clone(),
                    snapshot_json: snapshot,
                    fetched_at: now_rfc3339(),
                },
            )
            .map_err(|e| TypedError::new(e.error_code(), e.to_string()))?;
    }

    // Route record: class heuristic + §8.4 priority; user@ split into the
    // route's username field. Re-enrollment is also the explicit backfill
    // path for pre-P9 rows whose wez_domain is NULL; read-only route listing
    // deliberately never mutates them.
    let (route_id, network_class) = record_enrolled_route(&mut registry, hello.host_uid, target)?;

    Ok(Enrollment {
        host: enrolled,
        route_id,
        network_class,
        hello,
        lineage: assessment,
    })
}

/// Record the OpenSSH route learned by enrollment. Kept separate from the
/// handshake so idempotency of the persisted route identity and P9 domain
/// name can be tested without faking the external ssh process.
fn record_enrolled_route(
    registry: &mut Registry,
    host_uid: crate::model::HostUid,
    target: &str,
) -> Result<(i64, NetworkClass), TypedError> {
    let (username, endpoint) = split_target(target);
    let network_class = classify_endpoint(target);
    let route_id = registry
        .upsert_route(&RouteSpec {
            host_uid,
            transport: Transport::Openssh,
            endpoint: endpoint.to_string(),
            username: username.map(str::to_string),
            wez_domain: Some(wez_domain_name(host_uid, Transport::Openssh, endpoint)),
            network_class,
            priority: default_priority(network_class),
            required_capability: None,
            trust_fingerprint: None,
            enabled: true,
        })
        .map_err(|e| TypedError::new(e.error_code(), e.to_string()))?;
    // Outcome recording is diagnostic and intentionally does not advance
    // the authority chain. A failure here cannot invalidate enrollment.
    let _ = registry.record_route_outcome(route_id, outcome::OK);
    Ok((route_id, network_class))
}

/// One fresh nonce-bound hello exchange with `target`, validated end to
/// end (protocol, uid echo, well-formedness, nonce echo).
fn fresh_hello(
    registry: &Registry,
    target: &str,
    invoker: &dyn RouteInvoker,
    invocation: &AgentInvocation,
    deadline: Duration,
) -> Result<(Envelope, HelloInfo), TypedError> {
    let local = registry
        .identity()
        .map_err(|e| TypedError::new(e.error_code(), e.to_string()))?;
    let head = registry
        .authority_head()
        .map_err(|e| TypedError::new(e.error_code(), e.to_string()))?;
    let request_uid = Uuid::new_v4();
    let nonce = Uuid::new_v4();
    let payload = serde_json::to_value(HelloPayload { nonce: Some(nonce) })
        .map_err(|e| TypedError::new(ErrorCode::OperationFailed, e.to_string()))?;
    let request = request_envelope(
        &local,
        &head,
        protocol::methods::HELLO,
        request_uid,
        payload,
    );
    let bytes = serde_json::to_vec(&request)
        .map_err(|e| TypedError::new(ErrorCode::OperationFailed, e.to_string()))?;
    let mut hello_invocation = invocation.clone();
    hello_invocation.method = protocol::methods::HELLO.to_string();
    let route = synthetic_route(target);
    let argv = invoker.argv_for(&route, &hello_invocation);
    let envelope =
        client::call_argv(&argv, &bytes, request_uid, deadline).map_err(|f| f.typed_error())?;
    let payload = envelope.payload.clone().ok_or_else(|| {
        TypedError::new(ErrorCode::OperationFailed, "hello response had no payload")
    })?;
    let hello: HelloInfo = serde_json::from_value(payload).map_err(|e| {
        TypedError::new(
            ErrorCode::ProviderUnavailable,
            format!("remote agent answered, but not with a v1 hello: {e}"),
        )
    })?;
    if hello.nonce != Some(nonce) {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "hello response did not echo the fresh nonce",
        ));
    }
    Ok((envelope, hello))
}

fn assess_hello(
    registry: &Registry,
    envelope: &Envelope,
    hello: &HelloInfo,
) -> Result<PeerLineage, TypedError> {
    let cached = registry
        .peer_cache(hello.host_uid)
        .map_err(|e| TypedError::new(e.error_code(), e.to_string()))?;
    Ok(lineage::assess(
        cached.as_ref(),
        &PresentedPeer {
            registry_uid: envelope.registry_uid,
            revision: envelope.authority_revision,
            head_hash: envelope.authority_head_hash.clone(),
        },
        Some(&hello.revision_chain),
        true,
    ))
}

/// A synthetic (unpersisted) route row so the shared invoker seam can spell
/// the transport argv before any route exists.
fn synthetic_route(target: &str) -> RouteRow {
    let (username, endpoint) = split_target(target);
    RouteRow {
        route_id: 0,
        host_uid: crate::model::HostUid(Uuid::nil()),
        transport: Transport::Openssh,
        endpoint: endpoint.to_string(),
        username: username.map(str::to_string),
        wez_domain: None,
        network_class: classify_endpoint(target),
        priority: 0,
        required_capability: None,
        trust_fingerprint: None,
        enabled: true,
        last_outcome: None,
        last_outcome_at: None,
    }
}

fn split_target(target: &str) -> (Option<&str>, &str) {
    match target.split_once('@') {
        Some((user, host)) if !user.is_empty() && !host.is_empty() => (Some(user), host),
        _ => (None, target),
    }
}

/// A target spelling doubles as the host label only when it already IS a
/// valid label; addresses/user@ forms never invent one.
fn label_candidate(target: &str) -> Option<&str> {
    is_valid_host_label(target).then_some(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> (tempfile::TempDir, Registry) {
        let scratch = tempfile::tempdir().unwrap();
        let registry = Registry::open(RegistryConfig::new(
            scratch.path().join("registry.sqlite3"),
            scratch.path().join("locks"),
        ))
        .unwrap();
        (scratch, registry)
    }

    #[test]
    fn target_splitting_and_label_candidates() {
        assert_eq!(split_target("archie"), (None, "archie"));
        assert_eq!(split_target("fredrir@archie"), (Some("fredrir"), "archie"));
        assert_eq!(split_target("@archie"), (None, "@archie"));
        assert_eq!(label_candidate("archie"), Some("archie"));
        assert_eq!(label_candidate("10.77.77.2"), None);
        assert_eq!(label_candidate("fredrir@archie"), None);
        assert_eq!(label_candidate("Archie"), None);
    }

    #[test]
    fn enrollment_route_is_idempotent_and_backfills_a_stable_domain() {
        let (_scratch, mut registry) = registry();
        let host_uid = crate::model::HostUid(Uuid::from_u128(0x00112233445566778899aabbccddeeff));
        let enrolled = registry.enroll_host(host_uid, Some("archie")).unwrap();

        // Model an existing pre-P9 enrollment row. Re-enrollment of that
        // exact address updates it in place instead of allocating a route.
        let route_id = registry
            .upsert_route(&RouteSpec {
                host_uid,
                transport: Transport::Openssh,
                endpoint: "10.77.77.2".into(),
                username: Some("fredrir".into()),
                wez_domain: None,
                network_class: NetworkClass::Usb,
                priority: default_priority(NetworkClass::Usb),
                required_capability: None,
                trust_fingerprint: None,
                enabled: true,
            })
            .unwrap();
        let before_listing = registry.authority_head().unwrap().revision;
        assert!(
            registry.routes_for(host_uid).unwrap()[0]
                .wez_domain
                .is_none()
        );
        assert_eq!(
            registry.authority_head().unwrap().revision,
            before_listing,
            "read-only listing must leave legacy NULL rows untouched"
        );

        let (backfilled_id, class) =
            record_enrolled_route(&mut registry, host_uid, "fredrir@10.77.77.2").unwrap();
        assert_eq!(backfilled_id, route_id);
        assert_eq!(class, NetworkClass::Usb);
        let backfilled = registry.routes_for(host_uid).unwrap().remove(0);
        let domain = backfilled
            .wez_domain
            .expect("re-enrollment backfills domain");
        assert_eq!(
            domain,
            wez_domain_name(host_uid, Transport::Openssh, "10.77.77.2")
        );

        // Same HostUid/address is a pure route upsert; changing mutable
        // username metadata still cannot change its native domain.
        let (again_id, _) =
            record_enrolled_route(&mut registry, host_uid, "another@10.77.77.2").unwrap();
        assert_eq!(again_id, route_id);
        let again = registry.routes_for(host_uid).unwrap().remove(0);
        assert_eq!(again.username.as_deref(), Some("another"));
        assert_eq!(again.wez_domain.as_deref(), Some(domain.as_str()));
        let before_identical = registry.authority_head().unwrap().revision;
        let (identical_id, _) =
            record_enrolled_route(&mut registry, host_uid, "another@10.77.77.2").unwrap();
        assert_eq!(identical_id, route_id);
        assert_eq!(
            registry.authority_head().unwrap().revision,
            before_identical,
            "identical re-enrollment is an authority no-op"
        );
        assert_eq!(
            registry.enroll_host(host_uid, None).unwrap().alias,
            enrolled.alias
        );

        // A second physical route to the same HostUid has its own stable
        // native domain but does not allocate a second host/alias.
        let (tailscale_id, tailscale_class) =
            record_enrolled_route(&mut registry, host_uid, "100.101.5.9").unwrap();
        assert_ne!(tailscale_id, route_id);
        assert_eq!(tailscale_class, NetworkClass::Tailscale);
        let routes = registry.routes_for(host_uid).unwrap();
        assert_eq!(routes.len(), 2);
        assert_ne!(routes[0].wez_domain, routes[1].wez_domain);
        assert_eq!(registry.hosts().unwrap().len(), 2, "local plus one peer");
    }
}
