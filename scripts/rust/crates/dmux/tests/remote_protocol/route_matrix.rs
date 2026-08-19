//! The route retry matrix (§12.1, acceptance 21) over fake transports —
//! shell scripts that emit real ssh failure diagnostics or forward to the
//! real `_agent` binary. Proves: a dead endpoint (transport class) tries
//! the next verified route with BYTE-IDENTICAL request content; an auth
//! failure never retries; identity mismatches are terminal; every attempt
//! records its typed outcome token.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use dmux::model::HostUid;
use dmux::registry::{NetworkClass, RouteRow, RouteSpec, Transport};
use dmux::remote::client::{
    AgentInvocation, DEFAULT_DEADLINE, PeerExpectation, RouteInvoker, call_over_routes,
};
use dmux::remote::protocol::{self, HelloInfo};
use dmux::remote::routes::outcome;
use serde_json::json;
use uuid::Uuid;

use crate::util::{DMUX_BIN, Scratch, envelope};

/// Fake transports keyed by route endpoint. Each script appends the exact
/// request bytes it received to a capture file before failing/forwarding.
struct ScriptInvoker {
    dir: PathBuf,
}

impl ScriptInvoker {
    fn new(dir: &std::path::Path, owner: &Scratch) -> ScriptInvoker {
        let invoker = ScriptInvoker {
            dir: dir.to_path_buf(),
        };
        invoker.write(
            "dead",
            "#!/bin/sh\ncat >> \"$0.capture\"\n\
             echo 'ssh: connect to host 10.77.77.9 port 22: Connection refused' >&2\n\
             exit 255\n",
        );
        invoker.write(
            "auth",
            "#!/bin/sh\ncat >> \"$0.capture\"\n\
             echo 'wrong@host: Permission denied (publickey,password).' >&2\n\
             exit 255\n",
        );
        invoker.write(
            "ok",
            &format!(
                "#!/bin/sh\ntee -a \"$0.capture\" | {DMUX_BIN} _agent --protocol 1 hello \
                 --data-dir {} --lock-dir {}\n",
                owner.data.path().display(),
                owner.locks.path().display()
            ),
        );
        invoker
    }

    fn write(&self, name: &str, body: &str) {
        let path = self.script(name);
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn script(&self, name: &str) -> PathBuf {
        self.dir.join(format!("fake-ssh-{name}"))
    }

    fn capture(&self, name: &str) -> Option<Vec<u8>> {
        fs::read(self.dir.join(format!("fake-ssh-{name}.capture"))).ok()
    }
}

impl RouteInvoker for ScriptInvoker {
    fn argv_for(&self, route: &RouteRow, _invocation: &AgentInvocation) -> Vec<String> {
        vec![self.script(&route.endpoint).display().to_string()]
    }
}

/// A CLIENT registry with an enrolled peer and fake routes to it, plus a
/// separate OWNER scratch the "ok" transport forwards to.
fn rig(tag: &str, endpoints: &[&str]) -> (Scratch, Scratch, ScriptInvoker, HostUid) {
    let owner = Scratch::new(&format!("{tag}-owner"));
    let owner_uid = owner.registry().identity().unwrap().host_uid;
    let client = Scratch::new(&format!("{tag}-client"));
    let mut registry = client.registry();
    registry.enroll_host(owner_uid, None).unwrap();
    for (index, endpoint) in endpoints.iter().enumerate() {
        registry
            .upsert_route(&RouteSpec {
                host_uid: owner_uid,
                transport: Transport::Openssh,
                endpoint: endpoint.to_string(),
                username: None,
                wez_domain: None,
                network_class: NetworkClass::Other,
                priority: 10 * (index as i64 + 1),
                required_capability: None,
                trust_fingerprint: None,
                enabled: true,
            })
            .unwrap();
    }
    drop(registry);
    let invoker = ScriptInvoker::new(client.data.path(), &owner);
    (owner, client, invoker, owner_uid)
}

fn outcome_by_endpoint(client: &Scratch, host: HostUid) -> Vec<(String, Option<String>)> {
    client
        .registry()
        .routes_for(host)
        .unwrap()
        .into_iter()
        .map(|r| (r.endpoint, r.last_outcome))
        .collect()
}

#[test]
fn dead_endpoint_tries_next_route_with_identical_bytes() {
    let (_owner, client, invoker, owner_uid) = rig("retry", &["dead", "ok"]);
    let request = envelope(protocol::methods::HELLO, Uuid::new_v4(), json!({}));
    let expectation = PeerExpectation {
        host_uid: owner_uid,
        need_capability: None,
        claimed_current: false,
    };
    let mut registry = client.registry();
    let outcome_result = call_over_routes(
        &mut registry,
        &expectation,
        &request,
        &invoker,
        &AgentInvocation::new(protocol::methods::HELLO),
        DEFAULT_DEADLINE,
    )
    .expect("second route must serve the call");
    drop(registry);

    // The winning envelope really came from the owner agent.
    let hello: HelloInfo =
        serde_json::from_value(outcome_result.envelope.payload.clone().unwrap()).unwrap();
    assert_eq!(hello.host_uid, owner_uid);

    // Acceptance 21/22 heart: both attempts received the SAME bytes.
    let sent = serde_json::to_vec(&request).unwrap();
    assert_eq!(invoker.capture("dead").as_deref(), Some(sent.as_slice()));
    assert_eq!(invoker.capture("ok").as_deref(), Some(sent.as_slice()));

    // Typed outcomes recorded per attempt.
    let outcomes = outcome_by_endpoint(&client, owner_uid);
    assert_eq!(
        outcomes,
        vec![
            (
                "dead".to_string(),
                Some(outcome::TRANSPORT_UNREACHABLE.to_string())
            ),
            ("ok".to_string(), Some(outcome::OK.to_string())),
        ]
    );
}

#[test]
fn auth_failure_is_terminal_and_never_reaches_the_next_route() {
    let (_owner, client, invoker, owner_uid) = rig("auth", &["auth", "ok"]);
    let request = envelope(protocol::methods::HELLO, Uuid::new_v4(), json!({}));
    let expectation = PeerExpectation {
        host_uid: owner_uid,
        need_capability: None,
        claimed_current: false,
    };
    let mut registry = client.registry();
    let error = call_over_routes(
        &mut registry,
        &expectation,
        &request,
        &invoker,
        &AgentInvocation::new(protocol::methods::HELLO),
        DEFAULT_DEADLINE,
    )
    .expect_err("auth failure must be terminal");
    drop(registry);
    assert_eq!(error.code, dmux::error::ErrorCode::AuthFailed);

    // The healthy second route was NEVER attempted.
    assert!(invoker.capture("auth").is_some());
    assert_eq!(invoker.capture("ok"), None);
    let outcomes = outcome_by_endpoint(&client, owner_uid);
    assert_eq!(
        outcomes,
        vec![
            ("auth".to_string(), Some(outcome::AUTH_FAILED.to_string())),
            ("ok".to_string(), None),
        ]
    );
}

#[test]
fn responder_identity_mismatch_is_terminal_with_typed_outcome() {
    // Enroll a DIFFERENT HostUid than the one the owner agent presents.
    let (_owner, client, invoker, _owner_uid) = rig("identity", &[]);
    let imposter = HostUid(Uuid::from_u128(0xBAD));
    let mut registry = client.registry();
    registry.enroll_host(imposter, None).unwrap();
    registry
        .upsert_route(&RouteSpec {
            host_uid: imposter,
            transport: Transport::Openssh,
            endpoint: "ok".into(),
            username: None,
            wez_domain: None,
            network_class: NetworkClass::Other,
            priority: 10,
            required_capability: None,
            trust_fingerprint: None,
            enabled: true,
        })
        .unwrap();
    let request = envelope(protocol::methods::HELLO, Uuid::new_v4(), json!({}));
    let error = call_over_routes(
        &mut registry,
        &PeerExpectation {
            host_uid: imposter,
            need_capability: None,
            claimed_current: false,
        },
        &request,
        &invoker,
        &AgentInvocation::new(protocol::methods::HELLO),
        DEFAULT_DEADLINE,
    )
    .expect_err("wrong responder identity must refuse");
    drop(registry);
    assert_eq!(error.code, dmux::error::ErrorCode::HostIdentityChanged);
    let outcomes = outcome_by_endpoint(&client, imposter);
    assert_eq!(
        outcomes,
        vec![(
            "ok".to_string(),
            Some(outcome::HOST_IDENTITY_CHANGED.to_string())
        )]
    );
}

#[test]
fn all_routes_dead_reports_route_unavailable_after_trying_each() {
    let (_owner, client, invoker, owner_uid) = rig("alldead", &["dead", "dead2"]);
    // Second dead endpoint reuses the dead script body under another name.
    invoker.write(
        "dead2",
        "#!/bin/sh\ncat >> \"$0.capture\"\n\
         echo 'ssh: connect to host 10.77.77.10 port 22: Operation timed out' >&2\n\
         exit 255\n",
    );
    let request = envelope(protocol::methods::HELLO, Uuid::new_v4(), json!({}));
    let mut registry = client.registry();
    let error = call_over_routes(
        &mut registry,
        &PeerExpectation {
            host_uid: owner_uid,
            need_capability: None,
            claimed_current: false,
        },
        &request,
        &invoker,
        &AgentInvocation::new(protocol::methods::HELLO),
        DEFAULT_DEADLINE,
    )
    .expect_err("no route left");
    drop(registry);
    assert_eq!(error.code, dmux::error::ErrorCode::RouteUnavailable);
    assert!(invoker.capture("dead").is_some());
    assert!(invoker.capture("dead2").is_some());
    let outcomes = outcome_by_endpoint(&client, owner_uid);
    assert!(
        outcomes
            .iter()
            .all(|(_, o)| o.as_deref() == Some(outcome::TRANSPORT_UNREACHABLE))
    );
}

#[test]
fn disabled_routes_are_never_attempted() {
    let (_owner, client, invoker, owner_uid) = rig("disabled", &["auth", "ok"]);
    let mut registry = client.registry();
    let auth_route = registry
        .routes_for(owner_uid)
        .unwrap()
        .into_iter()
        .find(|r| r.endpoint == "auth")
        .unwrap();
    registry
        .set_route_enabled(auth_route.route_id, false)
        .unwrap();
    let request = envelope(protocol::methods::HELLO, Uuid::new_v4(), json!({}));
    call_over_routes(
        &mut registry,
        &PeerExpectation {
            host_uid: owner_uid,
            need_capability: None,
            claimed_current: false,
        },
        &request,
        &invoker,
        &AgentInvocation::new(protocol::methods::HELLO),
        DEFAULT_DEADLINE,
    )
    .expect("the enabled route serves the call");
    drop(registry);
    assert_eq!(invoker.capture("auth"), None, "disabled route untouched");
    assert!(invoker.capture("ok").is_some());
}

// ---------------------------------------------------------------------------
// Degraded replies: the agent answered, but before it could echo the uid.

/// A transport that answers with one literal envelope document.
fn canned(invoker: &ScriptInvoker, endpoint: &str, document: &str) {
    invoker.write(
        endpoint,
        &format!("#!/bin/sh\ncat >> \"$0.capture\"\nprintf '%s\\n' '{document}'\n"),
    );
}

/// The environment-resolution path (`remote/agent.rs`, `resolve_env`) run
/// through the REAL binary with an EMPTY environment, so it fails exactly
/// as it did on Archie under Tailscale SSH — before it can read the request
/// id, answering `Uuid::nil()` plus its typed error.
///
/// The caller must see that reason. It must ALSO stay terminal: an
/// environment failure is not one of the enumerated pre-authentication
/// transport failures, so the healthy next route is never tried (§8.3).
#[test]
fn a_degraded_agent_reply_surfaces_its_reason_and_never_tries_the_next_route() {
    let (_owner, client, invoker, owner_uid) = rig("degraded", &["noenv", "ok"]);
    // `env -i` guarantees no HOME/XDG_DATA_HOME/XDG_RUNTIME_DIR at all, and
    // no --data-dir/--lock-dir seam, so `resolve_env` fails before anything
    // is opened or created — this must never reach a real registry.
    invoker.write(
        "noenv",
        &format!(
            "#!/bin/sh\ncat >> \"$0.capture\"\nexec /usr/bin/env -i {DMUX_BIN} \
             _agent --protocol 1 hello\n"
        ),
    );
    let request = envelope(protocol::methods::HELLO, Uuid::new_v4(), json!({}));
    let mut registry = client.registry();
    let error = call_over_routes(
        &mut registry,
        &PeerExpectation {
            host_uid: owner_uid,
            need_capability: None,
            claimed_current: false,
        },
        &request,
        &invoker,
        &AgentInvocation::new(protocol::methods::HELLO),
        DEFAULT_DEADLINE,
    )
    .expect_err("an unusable environment must refuse the call");
    drop(registry);

    assert_eq!(error.code, dmux::error::ErrorCode::OperationFailed);
    assert!(
        error.message.contains("environment"),
        "the caller must see the agent's reason, not a uid complaint: {}",
        error.message
    );
    assert!(
        !error.message.contains("echoes request"),
        "the uid check must not shadow the reason: {}",
        error.message
    );
    // §8.3: terminal. The healthy route is untouched.
    assert!(invoker.capture("noenv").is_some());
    assert_eq!(invoker.capture("ok"), None, "no failover on an agent error");
    assert_eq!(
        outcome_by_endpoint(&client, owner_uid),
        vec![
            ("noenv".to_string(), Some(outcome::AGENT_ERROR.to_string())),
            ("ok".to_string(), None),
        ]
    );
}

/// §12.1's correlation guarantee, unweakened: a NON-nil echo that does not
/// match is a replay or a crossed reply. An error inside it buys it
/// nothing — the reply is refused for the mismatch, its error is not this
/// request's answer, and the route records `malformed_response`.
#[test]
fn a_wrong_non_nil_echo_stays_malformed_even_carrying_an_error() {
    let (_owner, client, invoker, owner_uid) = rig("crossed", &["crossed", "ok"]);
    canned(
        &invoker,
        "crossed",
        &format!(
            r#"{{"protocol_version":1,"request_uid":"{}","method":"hello",{}"#,
            Uuid::from_u128(0x5151),
            r#""payload_sha256":"00","host_uid":"00000000-0000-0000-0000-000000000000",
"registry_uid":"00000000-0000-0000-0000-000000000000","authority_revision":0,
"authority_head_hash":"","capabilities":[],
"error":{"code":"operation_failed","message":"environment: replayed reason"}}"#
                .replace('\n', "")
        ),
    );
    let request = envelope(protocol::methods::HELLO, Uuid::new_v4(), json!({}));
    let mut registry = client.registry();
    let error = call_over_routes(
        &mut registry,
        &PeerExpectation {
            host_uid: owner_uid,
            need_capability: None,
            claimed_current: false,
        },
        &request,
        &invoker,
        &AgentInvocation::new(protocol::methods::HELLO),
        DEFAULT_DEADLINE,
    )
    .expect_err("an uncorrelated reply must refuse the call");
    drop(registry);

    assert!(
        error.message.contains("echoes request"),
        "a crossed reply is a protocol violation: {}",
        error.message
    );
    assert!(
        !error.message.contains("replayed reason"),
        "an uncorrelated reply's error must not answer this request: {}",
        error.message
    );
    assert_eq!(invoker.capture("ok"), None, "terminal, so no failover");
    assert_eq!(
        outcome_by_endpoint(&client, owner_uid),
        vec![
            (
                "crossed".to_string(),
                Some(outcome::MALFORMED_RESPONSE.to_string())
            ),
            ("ok".to_string(), None),
        ]
    );
}

/// A nil echo with NO error answers no request at all, and is still
/// malformed — only the error carries the reason that earns the exception.
#[test]
fn a_nil_echo_without_an_error_is_still_malformed_over_a_route() {
    let (_owner, client, invoker, owner_uid) = rig("nilpayload", &["nilpayload", "ok"]);
    canned(
        &invoker,
        "nilpayload",
        &r#"{"protocol_version":1,
"request_uid":"00000000-0000-0000-0000-000000000000","method":"hello",
"payload_sha256":"00","host_uid":"00000000-0000-0000-0000-000000000000",
"registry_uid":"00000000-0000-0000-0000-000000000000","authority_revision":0,
"authority_head_hash":"","capabilities":[],"payload":{}}"#
            .replace('\n', ""),
    );
    let request = envelope(protocol::methods::HELLO, Uuid::new_v4(), json!({}));
    let mut registry = client.registry();
    let error = call_over_routes(
        &mut registry,
        &PeerExpectation {
            host_uid: owner_uid,
            need_capability: None,
            claimed_current: false,
        },
        &request,
        &invoker,
        &AgentInvocation::new(protocol::methods::HELLO),
        DEFAULT_DEADLINE,
    )
    .expect_err("a nil echo answers no request");
    drop(registry);

    assert!(
        error.message.contains("echoes request"),
        "a nil echo with a payload is malformed: {}",
        error.message
    );
    assert_eq!(invoker.capture("ok"), None);
    assert_eq!(
        outcome_by_endpoint(&client, owner_uid),
        vec![
            (
                "nilpayload".to_string(),
                Some(outcome::MALFORMED_RESPONSE.to_string())
            ),
            ("ok".to_string(), None),
        ]
    );
}
