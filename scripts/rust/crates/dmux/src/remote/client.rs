//! Client side of the owner-agent RPC (plan §12.1): build one request
//! envelope, spawn a transport argv, write the envelope, read one response
//! envelope under a dmux-imposed deadline, validate it, and walk the
//! verified route list under the frozen retry matrix (ADR 009 §4,
//! acceptance 21/22):
//!
//! - ONLY enumerated pre-authentication transport failures (connection
//!   refused/reset, connect-stage timeout, no route, DNS — ssh exit 255
//!   with those stderr classes — or a local spawn failure) try the next
//!   verified enabled route, by priority.
//! - Auth, host-key, identity, protocol, mutation, and postcondition
//!   failures never retry, and no route outcome ever falls back to another
//!   backend.
//! - A cross-route retry re-sends the byte-identical request envelope
//!   (identical request_uid/method/payload).
//! - Every attempt records a stable typed outcome token on its route.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use uuid::Uuid;

use crate::error::{ErrorCode, TypedError};
use crate::model::HostUid;
use crate::registry::{
    AuthorityHead, PeerCache, Registry, RegistryIdentity, RouteRow, now_rfc3339,
};
use crate::remote::lineage::{self, PeerLineage, PresentedPeer};
use crate::remote::protocol::{
    self, Envelope, HelloInfo, PROTOCOL_VERSION, canonical_payload_sha256,
};
use crate::remote::routes::outcome;

/// Default dmux-imposed end-to-end deadline for one bounded RPC exchange.
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Request construction

/// Build a request envelope carrying the CLIENT's own authority identity
/// fields plus the canonical payload digest. Built once per operation; a
/// cross-route retry re-serializes this identical value.
pub fn request_envelope(
    identity: &RegistryIdentity,
    head: &AuthorityHead,
    method: &str,
    request_uid: Uuid,
    payload: Value,
) -> Envelope {
    Envelope {
        protocol_version: PROTOCOL_VERSION,
        request_uid,
        method: method.to_string(),
        payload_sha256: canonical_payload_sha256(&payload),
        host_uid: identity.host_uid,
        registry_uid: identity.registry_uid,
        authority_revision: head.revision,
        authority_head_hash: head.head_hash.clone(),
        backend_instance_uid: None,
        server_epoch: None,
        capabilities: vec![format!("proto:{PROTOCOL_VERSION}")],
        payload: Some(payload),
        error: None,
    }
}

// ---------------------------------------------------------------------------
// Transport seam

/// How the hidden agent subcommand is spelled for a route. The invoker maps
/// a route row to the FULL spawn argv; tests substitute fake transports.
#[derive(Debug, Clone)]
pub struct AgentInvocation {
    pub method: String,
    pub protocol: u32,
    /// Remote dmux spelling (usually `dmux`; two-host tests use the scratch
    /// build's absolute path).
    pub remote_bin: String,
    /// Hidden test seams forwarded to the remote agent; production is None.
    pub data_dir: Option<String>,
    pub lock_dir: Option<String>,
}

impl AgentInvocation {
    pub fn new(method: &str) -> Self {
        AgentInvocation {
            method: method.to_string(),
            protocol: PROTOCOL_VERSION,
            remote_bin: "dmux".to_string(),
            data_dir: None,
            lock_dir: None,
        }
    }

    /// The `_agent` argument vector after the binary spelling.
    pub fn agent_args(&self) -> Vec<String> {
        let mut args = vec![
            "_agent".to_string(),
            "--protocol".to_string(),
            self.protocol.to_string(),
            self.method.clone(),
        ];
        if let Some(dir) = &self.data_dir {
            args.push("--data-dir".to_string());
            args.push(dir.clone());
        }
        if let Some(dir) = &self.lock_dir {
            args.push("--lock-dir".to_string());
            args.push(dir.clone());
        }
        args
    }
}

/// Maps one verified route row to the exact spawn argv for one RPC.
pub trait RouteInvoker {
    fn argv_for(&self, route: &RouteRow, invocation: &AgentInvocation) -> Vec<String>;
}

/// Production transport: `ssh [-oBatchMode=yes] [user@]endpoint dmux
/// _agent ...`. BatchMode keeps a bounded RPC from hanging on interactive
/// prompts; enrollment (which WANTS normal interactive host-key handling)
/// disables it.
#[derive(Debug, Clone)]
pub struct SshInvoker {
    pub batch_mode: bool,
    pub connect_timeout: Option<u32>,
    /// Extra `ssh` options placed before the destination (e.g.
    /// ControlMaster multiplexing). Never part of the remote command.
    pub extra_options: Vec<String>,
}

impl Default for SshInvoker {
    fn default() -> Self {
        SshInvoker {
            batch_mode: true,
            connect_timeout: Some(10),
            extra_options: Vec::new(),
        }
    }
}

impl RouteInvoker for SshInvoker {
    fn argv_for(&self, route: &RouteRow, invocation: &AgentInvocation) -> Vec<String> {
        let mut argv = vec!["ssh".to_string()];
        if self.batch_mode {
            argv.push("-oBatchMode=yes".to_string());
        }
        if let Some(secs) = self.connect_timeout {
            argv.push(format!("-oConnectTimeout={secs}"));
        }
        argv.extend(self.extra_options.iter().cloned());
        let destination = match &route.username {
            Some(user) => format!("{user}@{}", route.endpoint),
            None => route.endpoint.clone(),
        };
        argv.push(destination);
        argv.push(invocation.remote_bin.clone());
        argv.extend(invocation.agent_args());
        argv
    }
}

/// Direct subprocess transport (tests + local): the route endpoint is
/// ignored; the invocation's `remote_bin` is executed directly.
#[derive(Debug, Clone, Default)]
pub struct DirectInvoker;

impl RouteInvoker for DirectInvoker {
    fn argv_for(&self, _route: &RouteRow, invocation: &AgentInvocation) -> Vec<String> {
        let mut argv = vec![invocation.remote_bin.clone()];
        argv.extend(invocation.agent_args());
        argv
    }
}

// ---------------------------------------------------------------------------
// One transport exchange

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawReply {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Spawn `argv`, write `request_bytes` to stdin, read stdout/stderr to EOF
/// under the dmux-imposed deadline.
pub fn exchange(
    argv: &[String],
    request_bytes: &[u8],
    deadline: Duration,
) -> Result<RawReply, SpawnFailure> {
    let (program, args) = argv.split_first().ok_or_else(|| SpawnFailure {
        detail: "empty transport argv".to_string(),
    })?;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| SpawnFailure {
            detail: format!("spawn {program}: {e}"),
        })?;
    // Write the one request document, then close stdin so the agent's
    // read-to-EOF completes.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(request_bytes);
    }
    let mut stdout_pipe = child.stdout.take().expect("piped stdout");
    let mut stderr_pipe = child.stderr.take().expect("piped stderr");
    let out_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        buf
    });
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        buf
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = out_reader.join().unwrap_or_default();
                let stderr = err_reader.join().unwrap_or_default();
                return Ok(RawReply {
                    status: status.code().unwrap_or(-1),
                    stdout: String::from_utf8_lossy(&stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&stderr).into_owned(),
                });
            }
            Ok(None) => {
                if started.elapsed() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = out_reader.join();
                    let _ = err_reader.join();
                    return Err(SpawnFailure {
                        detail: format!(
                            "dmux-imposed deadline ({}ms) elapsed",
                            deadline.as_millis()
                        ),
                    });
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => {
                let _ = child.kill();
                return Err(SpawnFailure {
                    detail: format!("wait {program}: {e}"),
                });
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnFailure {
    pub detail: String,
}

impl SpawnFailure {
    fn is_deadline(&self) -> bool {
        self.detail.starts_with("dmux-imposed deadline")
    }
}

// ---------------------------------------------------------------------------
// Failure classification (the §12.1/§8.3 matrix)

/// Terminal-vs-retryable classification of one attempt.
#[derive(Debug, Clone, PartialEq)]
pub enum AttemptFailure {
    /// Enumerated pre-authentication transport failure — the ONLY class
    /// that may try the next verified route.
    Transport { detail: String },
    /// SSH authentication failure.
    Auth { detail: String },
    /// Host-key verification / SSH identity failure.
    HostKey { detail: String },
    /// Remote dmux agent missing or not runnable.
    CommandMissing { detail: String },
    /// The dmux-imposed deadline elapsed after the exchange began.
    Timeout { detail: String },
    /// Response was not one well-formed envelope.
    Malformed { detail: String },
    /// Exact protocol version mismatch.
    Protocol { detail: String },
    /// Responder is not the expected enrolled HostUid.
    Identity { detail: String },
    /// RegistryUid/lineage conflict or confirmed rollback (plan §12.1).
    Lineage { detail: String },
    /// The agent answered with a typed error inside a valid envelope.
    Agent(TypedError),
}

impl AttemptFailure {
    /// Acceptance 21: only the transport class retries.
    pub fn retries_next_route(&self) -> bool {
        matches!(self, AttemptFailure::Transport { .. })
    }

    /// The stable token recorded on the route for this attempt.
    pub fn outcome_token(&self) -> &'static str {
        match self {
            AttemptFailure::Transport { .. } => outcome::TRANSPORT_UNREACHABLE,
            AttemptFailure::Auth { .. } => outcome::AUTH_FAILED,
            AttemptFailure::HostKey { .. } => outcome::HOST_KEY_FAILED,
            AttemptFailure::CommandMissing { .. } => outcome::COMMAND_MISSING,
            AttemptFailure::Timeout { .. } => outcome::TIMEOUT,
            AttemptFailure::Malformed { .. } => outcome::MALFORMED_RESPONSE,
            AttemptFailure::Protocol { .. } => outcome::PROTOCOL_MISMATCH,
            AttemptFailure::Identity { .. } => outcome::HOST_IDENTITY_CHANGED,
            AttemptFailure::Lineage { .. } => outcome::LINEAGE_CONFLICT,
            AttemptFailure::Agent(_) => outcome::AGENT_ERROR,
        }
    }

    /// The typed error surfaced to the caller when this attempt is final.
    pub fn typed_error(&self) -> TypedError {
        match self {
            AttemptFailure::Transport { detail } => TypedError::new(
                ErrorCode::RouteUnavailable,
                format!("transport failure: {detail}"),
            ),
            AttemptFailure::Auth { detail } => TypedError::new(ErrorCode::AuthFailed, detail),
            AttemptFailure::HostKey { detail } => {
                TypedError::new(ErrorCode::HostIdentityChanged, detail)
            }
            AttemptFailure::CommandMissing { detail } => TypedError::new(
                ErrorCode::ProviderUnavailable,
                format!("no compatible dmux agent on the remote: {detail}"),
            ),
            AttemptFailure::Timeout { detail } => TypedError::new(
                ErrorCode::RouteUnavailable,
                format!("{detail}; result unknown — retry with the SAME request UID"),
            ),
            AttemptFailure::Malformed { detail } => TypedError::new(
                ErrorCode::OperationFailed,
                format!("malformed agent response: {detail}"),
            ),
            AttemptFailure::Protocol { detail } => {
                TypedError::new(ErrorCode::ProtocolMismatch, detail)
            }
            AttemptFailure::Identity { detail } => {
                TypedError::new(ErrorCode::HostIdentityChanged, detail)
            }
            AttemptFailure::Lineage { detail } => {
                TypedError::new(ErrorCode::IdentityConflict, detail)
            }
            AttemptFailure::Agent(error) => error.clone(),
        }
    }
}

/// The pre-authentication transport stderr classes (§8.3, exhaustive by
/// substring on ssh's exit-255 diagnostics).
const TRANSPORT_STDERR: &[&str] = &[
    "connection refused",
    "connection reset",
    "connection timed out",
    "operation timed out",
    "timed out",
    "no route to host",
    "network is unreachable",
    "could not resolve hostname",
    "name or service not known",
    "temporary failure in name resolution",
    "nodename nor servname provided",
    // The server closed the TCP stream during connect/kex — before
    // authentication ("Connection closed by <addr> port <n>"; sshd rate
    // limiting/MaxStartups early drop presents this way). Post-auth
    // closures print "Connection to <host> closed" instead, which does
    // NOT match this pattern.
    "connection closed by",
];

const HOSTKEY_STDERR: &[&str] = &[
    "host key verification failed",
    "remote host identification has changed",
    "no matching host key type",
    "host key for",
];

const AUTH_STDERR: &[&str] = &[
    "permission denied",
    "too many authentication failures",
    "authentication failed",
    "no supported authentication methods",
];

/// Classify one ssh exit-255 stderr. Unknown 255 diagnostics are NOT
/// retried: retry eligibility must be positively enumerated (§8.3).
pub fn classify_ssh_255(stderr: &str) -> AttemptFailure {
    let lower = stderr.to_lowercase();
    let detail = || {
        stderr
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("ssh exited 255")
            .to_string()
    };
    // Host-key classes first: "Host key verification failed" must never be
    // mistaken for a generic failure.
    if HOSTKEY_STDERR.iter().any(|p| lower.contains(p)) {
        return AttemptFailure::HostKey { detail: detail() };
    }
    if AUTH_STDERR.iter().any(|p| lower.contains(p)) {
        return AttemptFailure::Auth { detail: detail() };
    }
    if TRANSPORT_STDERR.iter().any(|p| lower.contains(p)) {
        return AttemptFailure::Transport { detail: detail() };
    }
    AttemptFailure::Malformed {
        detail: format!("unclassified ssh failure: {}", detail()),
    }
}

/// Interpret one raw exchange as an envelope or a classified failure.
/// The agent exits nonzero for typed errors while still writing one valid
/// envelope, so a parseable envelope always wins over the exit status.
pub fn interpret_reply(reply: &RawReply, request_uid: Uuid) -> Result<Envelope, AttemptFailure> {
    if let Ok(envelope) = serde_json::from_str::<Envelope>(reply.stdout.trim()) {
        if envelope.protocol_version != PROTOCOL_VERSION {
            return Err(AttemptFailure::Protocol {
                detail: format!(
                    "agent speaks protocol {}, this client requires exactly {PROTOCOL_VERSION}",
                    envelope.protocol_version
                ),
            });
        }
        if envelope.request_uid != request_uid {
            return Err(AttemptFailure::Malformed {
                detail: format!(
                    "response echoes request {} but {} was sent",
                    envelope.request_uid, request_uid
                ),
            });
        }
        if !(envelope.payload.is_some() ^ envelope.error.is_some()) {
            return Err(AttemptFailure::Malformed {
                detail: "response carries neither/both of payload and error".into(),
            });
        }
        // A protocol_mismatch typed error is a protocol failure even
        // through a valid envelope.
        if let Some(error) = &envelope.error {
            if error.code == ErrorCode::ProtocolMismatch {
                return Err(AttemptFailure::Protocol {
                    detail: error.message.clone(),
                });
            }
            return Err(AttemptFailure::Agent(error.clone()));
        }
        return Ok(envelope);
    }
    match reply.status {
        255 => Err(classify_ssh_255(&reply.stderr)),
        127 => Err(AttemptFailure::CommandMissing {
            detail: first_line(&reply.stderr, "exit 127"),
        }),
        _ if reply.stderr.to_lowercase().contains("command not found")
            || reply.stderr.to_lowercase().contains("no such file") =>
        {
            Err(AttemptFailure::CommandMissing {
                detail: first_line(&reply.stderr, "command not found"),
            })
        }
        status => Err(AttemptFailure::Malformed {
            detail: format!(
                "no envelope on stdout (exit {status}): {}",
                first_line(&reply.stderr, "empty stderr")
            ),
        }),
    }
}

fn first_line(text: &str, fallback: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

/// One attempt over one argv: exchange + interpret. Spawn failures are
/// transport-class (§8.3/ADR 009 §4); the dmux deadline is terminal.
pub fn call_argv(
    argv: &[String],
    request_bytes: &[u8],
    request_uid: Uuid,
    deadline: Duration,
) -> Result<Envelope, AttemptFailure> {
    match exchange(argv, request_bytes, deadline) {
        Ok(reply) => interpret_reply(&reply, request_uid),
        Err(failure) if failure.is_deadline() => Err(AttemptFailure::Timeout {
            detail: failure.detail,
        }),
        Err(failure) => Err(AttemptFailure::Transport {
            detail: failure.detail,
        }),
    }
}

// ---------------------------------------------------------------------------
// Route walk (retry matrix) + response identity/lineage validation

/// What the caller expects the responder to prove (§12.1).
#[derive(Debug, Clone)]
pub struct PeerExpectation {
    pub host_uid: HostUid,
    /// Capability the operation needs, for route eligibility filtering.
    pub need_capability: Option<String>,
    /// True when this call is a fresh nonce-bound hello whose presented
    /// head claims to be current (rollback-suspect eligibility).
    pub claimed_current: bool,
}

#[derive(Debug)]
pub struct RouteCallOutcome {
    pub envelope: Envelope,
    pub route_id: i64,
    pub lineage: PeerLineage,
}

/// Walk the verified enabled routes for `host` in priority order under the
/// frozen retry matrix, re-sending the byte-identical request envelope on a
/// cross-route retry, recording a typed outcome token per attempt, and
/// validating the winning response's identity + lineage (§12.1).
///
/// Lineage handling: the peer checkpoint is stored/advanced only on first
/// contact or a verified `hello` chain proof; conflicts and confirmed
/// rollback refuse the response. `hello` responses carry their chain in the
/// payload; other methods validate against the cached checkpoint only.
pub fn call_over_routes(
    registry: &mut Registry,
    expectation: &PeerExpectation,
    request: &Envelope,
    invoker: &dyn RouteInvoker,
    invocation: &AgentInvocation,
    deadline: Duration,
) -> Result<RouteCallOutcome, TypedError> {
    let routes = registry
        .routes_for(expectation.host_uid)
        .map_err(|e| TypedError::new(e.error_code(), e.to_string()))?;
    let routes = crate::remote::routes::eligible(routes, expectation.need_capability.as_deref());
    if routes.is_empty() {
        return Err(TypedError::new(
            ErrorCode::RouteUnavailable,
            format!("no enabled route to host {}", expectation.host_uid.0),
        ));
    }
    // Serialized exactly once: every route attempt sends identical bytes.
    let request_bytes = serde_json::to_vec(request)
        .map_err(|e| TypedError::new(ErrorCode::OperationFailed, e.to_string()))?;

    let mut last_failure: Option<AttemptFailure> = None;
    for route in &routes {
        let argv = invoker.argv_for(route, invocation);
        match call_argv(&argv, &request_bytes, request.request_uid, deadline) {
            Ok(envelope) => {
                let checked = validate_identity_and_lineage(registry, expectation, &envelope);
                match checked {
                    Ok(lineage) => {
                        let _ = registry.record_route_outcome(route.route_id, outcome::OK);
                        return Ok(RouteCallOutcome {
                            envelope,
                            route_id: route.route_id,
                            lineage,
                        });
                    }
                    Err(failure) => {
                        let _ =
                            registry.record_route_outcome(route.route_id, failure.outcome_token());
                        // Identity/lineage failures are terminal (§12.1).
                        return Err(failure.typed_error());
                    }
                }
            }
            Err(failure) => {
                let _ = registry.record_route_outcome(route.route_id, failure.outcome_token());
                if failure.retries_next_route() {
                    last_failure = Some(failure);
                    continue;
                }
                return Err(failure.typed_error());
            }
        }
    }
    let failure = last_failure.expect("non-empty route list ended without an outcome");
    let mut error = failure.typed_error();
    error.message = format!(
        "every enabled route failed pre-authentication; last: {}",
        error.message
    );
    Err(error)
}

/// §12.1 response validation: enrolled HostUid, then RegistryUid + lineage
/// against the cached peer checkpoint. Stores/advances the checkpoint per
/// the lineage policy. Never regresses the cache.
fn validate_identity_and_lineage(
    registry: &mut Registry,
    expectation: &PeerExpectation,
    envelope: &Envelope,
) -> Result<PeerLineage, AttemptFailure> {
    if envelope.host_uid != expectation.host_uid {
        return Err(AttemptFailure::Identity {
            detail: format!(
                "responder presented HostUid {} but {} is enrolled for this route",
                envelope.host_uid.0, expectation.host_uid.0
            ),
        });
    }
    let cached =
        registry
            .peer_cache(expectation.host_uid)
            .map_err(|e| AttemptFailure::Malformed {
                detail: format!("peer cache: {e}"),
            })?;
    let presented = PresentedPeer {
        registry_uid: envelope.registry_uid,
        revision: envelope.authority_revision,
        head_hash: envelope.authority_head_hash.clone(),
    };
    // A hello response carries the ancestry proof in its payload.
    let hello: Option<HelloInfo> = if envelope.method == protocol::methods::HELLO {
        envelope
            .payload
            .as_ref()
            .and_then(|p| serde_json::from_value(p.clone()).ok())
    } else {
        None
    };
    let proof = hello.as_ref().map(|h| h.revision_chain.as_slice());
    let assessment = lineage::assess(
        cached.as_ref(),
        &presented,
        proof,
        expectation.claimed_current,
    );
    if !assessment.accepts_response() {
        return Err(AttemptFailure::Lineage {
            detail: format!(
                "peer lineage {assessment:?}: presented registry {} revision {} head {}",
                presented.registry_uid.0, presented.revision, presented.head_hash
            ),
        });
    }
    if assessment.stores_cache() {
        let snapshot = envelope
            .payload
            .clone()
            .unwrap_or_else(|| serde_json::json!({}));
        let _ = registry.store_peer_cache(
            expectation.host_uid,
            &PeerCache {
                registry_uid: presented.registry_uid,
                authority_revision: presented.revision,
                authority_head_hash: presented.head_hash.clone(),
                snapshot_json: snapshot,
                fetched_at: now_rfc3339(),
            },
        );
    }
    Ok(assessment)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_255_classification_follows_the_matrix() {
        let transport =
            classify_ssh_255("ssh: connect to host 10.77.77.2 port 22: Connection refused\n");
        assert!(transport.retries_next_route());
        assert_eq!(transport.outcome_token(), outcome::TRANSPORT_UNREACHABLE);

        for text in [
            "ssh: connect to host x port 22: Operation timed out",
            "ssh: connect to host x port 22: No route to host",
            "ssh: Could not resolve hostname nope: Name or service not known",
            "ssh: connect to host x port 22: Network is unreachable",
            "kex_exchange_identification: Connection closed by remote host\r\n\
             Connection closed by 10.77.77.2 port 22",
        ] {
            assert!(classify_ssh_255(text).retries_next_route(), "{text}");
        }

        let auth = classify_ssh_255("fredrir@archie: Permission denied (publickey).\n");
        assert!(!auth.retries_next_route());
        assert_eq!(auth.outcome_token(), outcome::AUTH_FAILED);
        assert_eq!(auth.typed_error().code, ErrorCode::AuthFailed);

        let hostkey = classify_ssh_255("@@@ WARNING @@@\nHost key verification failed.\n");
        assert!(!hostkey.retries_next_route());
        assert_eq!(hostkey.outcome_token(), outcome::HOST_KEY_FAILED);

        // "Host key ... Permission ..." — host-key wins over auth.
        let both = classify_ssh_255("Host key verification failed. Permission denied");
        assert_eq!(both.outcome_token(), outcome::HOST_KEY_FAILED);

        // Unknown 255 diagnostics are terminal, not retried.
        let unknown = classify_ssh_255("kex_exchange_identification: banner exchange broke\n");
        assert!(!unknown.retries_next_route());
    }

    #[test]
    fn a_valid_error_envelope_beats_the_exit_status() {
        let identity = uuid::Uuid::from_u128(1);
        let envelope = serde_json::json!({
            "protocol_version": 1,
            "request_uid": identity.to_string(),
            "method": "rename",
            "payload_sha256": "00",
            "host_uid": uuid::Uuid::from_u128(2).to_string(),
            "registry_uid": uuid::Uuid::from_u128(3).to_string(),
            "authority_revision": 1,
            "authority_head_hash": "sha256:x",
            "capabilities": [],
            "error": {"code": "name_conflict", "message": "taken"}
        });
        let reply = RawReply {
            status: 4,
            stdout: envelope.to_string(),
            stderr: String::new(),
        };
        match interpret_reply(&reply, identity) {
            Err(AttemptFailure::Agent(error)) => {
                assert_eq!(error.code, ErrorCode::NameConflict);
            }
            other => panic!("expected typed agent error, got {other:?}"),
        }
    }

    #[test]
    fn missing_remote_command_is_terminal_not_transport() {
        let reply = RawReply {
            status: 127,
            stdout: String::new(),
            stderr: "bash: line 1: dmux: command not found\n".into(),
        };
        let failure = interpret_reply(&reply, uuid::Uuid::nil()).unwrap_err();
        assert!(!failure.retries_next_route());
        assert_eq!(failure.outcome_token(), outcome::COMMAND_MISSING);
        assert_eq!(failure.typed_error().code, ErrorCode::ProviderUnavailable);
    }

    #[test]
    fn wrong_request_uid_echo_is_malformed() {
        let sent = uuid::Uuid::from_u128(10);
        let envelope = serde_json::json!({
            "protocol_version": 1,
            "request_uid": uuid::Uuid::from_u128(11).to_string(),
            "method": "hello",
            "payload_sha256": "00",
            "host_uid": uuid::Uuid::from_u128(2).to_string(),
            "registry_uid": uuid::Uuid::from_u128(3).to_string(),
            "authority_revision": 1,
            "authority_head_hash": "sha256:x",
            "capabilities": [],
            "payload": {}
        });
        let reply = RawReply {
            status: 0,
            stdout: envelope.to_string(),
            stderr: String::new(),
        };
        let failure = interpret_reply(&reply, sent).unwrap_err();
        assert_eq!(failure.outcome_token(), outcome::MALFORMED_RESPONSE);
    }

    #[test]
    fn ssh_invoker_builds_the_fixed_hidden_command() {
        let route = RouteRow {
            route_id: 1,
            host_uid: HostUid(uuid::Uuid::nil()),
            transport: crate::registry::Transport::Openssh,
            endpoint: "archie".into(),
            username: Some("fredrir".into()),
            wez_domain: None,
            network_class: crate::registry::NetworkClass::Usb,
            priority: 10,
            required_capability: None,
            trust_fingerprint: None,
            enabled: true,
            last_outcome: None,
            last_outcome_at: None,
        };
        let invocation = AgentInvocation::new("hello");
        let argv = SshInvoker::default().argv_for(&route, &invocation);
        assert_eq!(
            argv,
            vec![
                "ssh",
                "-oBatchMode=yes",
                "-oConnectTimeout=10",
                "fredrir@archie",
                "dmux",
                "_agent",
                "--protocol",
                "1",
                "hello"
            ]
        );
    }
}
