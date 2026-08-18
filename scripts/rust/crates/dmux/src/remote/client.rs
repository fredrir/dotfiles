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

use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;
use uuid::Uuid;

use crate::childio::{BoundedCapture, bounded_read, kill_process_group};
use crate::error::{ErrorCode, TypedError};
use crate::model::{BackendInstanceUid, HostUid, ServerEpoch};
use crate::recovery::{RecoveryControlAction, RecoveryControlRequest, RecoveryInspection};
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
// Narrow owner-recovery controller surface

/// The enrolled owner and, when already known, the exact Wez server
/// incarnation a recovery command is allowed to address.  The owner still
/// resolves its own registry and runtime paths; these qualifiers are only
/// stale-target fences carried in the common envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryOwnerContext {
    pub host_uid: HostUid,
    pub backend_instance_uid: Option<BackendInstanceUid>,
    pub server_epoch: Option<ServerEpoch>,
}

impl RecoveryOwnerContext {
    /// Discover/inspect the owner's current Wez incarnation.
    pub fn new(host_uid: HostUid) -> Self {
        RecoveryOwnerContext {
            host_uid,
            backend_instance_uid: None,
            server_epoch: None,
        }
    }

    /// Fence a control request to an incarnation learned from a prior
    /// inspection/hello.  A restart between the two calls is then refused.
    pub fn qualified(
        host_uid: HostUid,
        backend_instance_uid: BackendInstanceUid,
        server_epoch: ServerEpoch,
    ) -> Self {
        RecoveryOwnerContext {
            host_uid,
            backend_instance_uid: Some(backend_instance_uid),
            server_epoch: Some(server_epoch),
        }
    }
}

/// Public recovery verbs mapped one-to-one to owner-agent methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryOwnerCommand {
    Status,
    Resume,
    Abort,
}

impl RecoveryOwnerCommand {
    fn method(self) -> &'static str {
        match self {
            RecoveryOwnerCommand::Status => protocol::methods::RECOVERY_STATUS,
            RecoveryOwnerCommand::Resume => protocol::methods::RECOVERY_RESUME,
            RecoveryOwnerCommand::Abort => protocol::methods::RECOVERY_ABORT,
        }
    }

    fn control_action(self) -> Option<RecoveryControlAction> {
        match self {
            RecoveryOwnerCommand::Status => None,
            RecoveryOwnerCommand::Resume => Some(RecoveryControlAction::Resume),
            RecoveryOwnerCommand::Abort => Some(RecoveryControlAction::Abort),
        }
    }
}

/// Typed owner response.  No controller caller has to reproduce envelope,
/// route-retry, peer-identity, or lineage validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryOwnerReply {
    Status(RecoveryInspection),
    Control(RecoveryControlRequest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryOwnerOutcome {
    pub reply: RecoveryOwnerReply,
    pub route_id: i64,
    pub lineage: PeerLineage,
}

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

/// Cap on the reply document one transport child may hand back.
///
/// stdout here carries exactly ONE JSON envelope, and the largest legitimate
/// one is a `hello` whose payload holds the peer's FULL recorded revision
/// chain from genesis ([`crate::remote::protocol::HelloInfo::revision_chain`],
/// roughly 250 bytes per link) — so this admits a peer with some sixteen
/// thousand authority revisions, orders of magnitude past any real registry.
/// It sits deliberately between the crate's other single-document bounds:
/// far above one GUI bridge message ([`crate::gui::MAX_MESSAGE_BYTES`],
/// 64 KiB) because an envelope carries a whole payload, and far below a
/// recovery manifest (16 MiB) because an envelope is not a manifest.
///
/// Past the cap the capture is a PREFIX, and [`interpret_reply`] refuses it
/// as a typed malformed response rather than letting `serde_json` report the
/// truncation as a confusing parse error.
const MAX_REPLY_STDOUT_BYTES: usize = 4 * 1024 * 1024;

/// Cap on one transport child's diagnostics.
///
/// Only [`first_line`] and [`classify_ssh_255`] ever read this stream, and
/// the largest real ssh diagnostic — the changed-host-key warning block — is
/// about a kilobyte, so this leaves ~60x headroom.  Truncation drops the
/// TAIL, so the first line these two report survives it; a genuine ssh
/// diagnostic pushed past the cap by peer noise simply matches no frozen
/// shape and lands in the conservative unclassified-255 class, which is
/// terminal rather than retried.
const MAX_REPLY_STDERR_BYTES: usize = 64 * 1024;

/// How long a reader thread may still be draining after the child is gone
/// before it is abandoned instead of joined.
///
/// Once the direct child has exited, everything it wrote is already in the
/// pipe buffer and drains in microseconds.  The only way to exceed this is a
/// surviving descendant holding the write end open — precisely the case that
/// must not turn into an unbounded join.
const READER_DRAIN_GRACE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawReply {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
    /// The child wrote more than [`MAX_REPLY_STDOUT_BYTES`], so `stdout`
    /// holds only a prefix of what it sent.  A prefix of an envelope is not
    /// a short envelope — it is not an envelope at all — so
    /// [`interpret_reply`] refuses it outright.
    pub stdout_truncated: bool,
}

/// Whether the transport child keeps this process's controlling-terminal
/// foreground process group.
///
/// `read_to_end` on a child pipe returns only when EVERY write end is
/// closed — the direct child's and every descendant's that inherited the
/// descriptor.  Killing one pid therefore does not guarantee EOF, and the
/// join that follows can outlive the deadline it was meant to obey.  A child
/// spawned as its own group leader can be signalled as a group, which does
/// guarantee it (see [`crate::childio`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportIsolation {
    /// The child leads its own process group, so the exit and deadline paths
    /// can close pipe write ends inherited by a backgrounded `ControlMaster`,
    /// a `ProxyCommand` helper, or an askpass child as well as the direct
    /// child's own.
    ///
    /// Such a group is a BACKGROUND group of this session's controlling
    /// terminal, so any read of `/dev/tty` in it raises `SIGTTIN` and stops
    /// the child.  Use this only for a transport that cannot prompt: the
    /// route walk's `ssh -oBatchMode=yes`, or a direct local agent.
    OwnProcessGroup,
    /// The child shares this process's group, so OpenSSH's `/dev/tty`
    /// prompts — first-contact host-key confirmation, key passphrases —
    /// still reach the user.  Enrollment's deliberately non-BatchMode hello
    /// (`remote::enroll`) needs exactly that, and pays for it with a
    /// deadline that can only kill the direct child; the bounded reader
    /// joins below are what keep even that case from hanging forever.
    SharedProcessGroup,
}

/// Spawn `argv`, write `request_bytes` to stdin, read a bounded stdout and
/// stderr under the dmux-imposed deadline.
///
/// This signature keeps the caller's process group, because the one caller
/// of it outside the route walk is enrollment's non-BatchMode hello, whose
/// host-key confirmation must reach the terminal.  The route walk calls
/// [`exchange_with`] with [`TransportIsolation::OwnProcessGroup`].
pub fn exchange(
    argv: &[String],
    request_bytes: &[u8],
    deadline: Duration,
) -> Result<RawReply, SpawnFailure> {
    exchange_with(
        argv,
        request_bytes,
        deadline,
        TransportIsolation::SharedProcessGroup,
    )
}

/// [`exchange`], with the child's process-group isolation chosen explicitly.
pub fn exchange_with(
    argv: &[String],
    request_bytes: &[u8],
    deadline: Duration,
    isolation: TransportIsolation,
) -> Result<RawReply, SpawnFailure> {
    let (program, args) = argv.split_first().ok_or_else(|| SpawnFailure {
        detail: "empty transport argv".to_string(),
    })?;
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if isolation == TransportIsolation::OwnProcessGroup {
        // Isolate the transport so exit/timeout can close pipes inherited by
        // a descendant as well as by the direct child.
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|e| SpawnFailure {
        detail: format!("spawn {program}: {e}"),
    })?;

    // Drain both pipes BEFORE writing the request. A peer that answers
    // before it finishes reading would otherwise fill its stdout pipe while
    // this thread is still blocked in `write_all`, and neither side could
    // progress — a deadlock reached before the deadline loop even starts.
    // Both captures are bounded, so no reply and no diagnostic stream can
    // grow local memory without limit for the length of the deadline.
    let stdout_pipe = child.stdout.take().expect("piped stdout");
    let stderr_pipe = child.stderr.take().expect("piped stderr");
    let out_reader = std::thread::spawn(move || bounded_read(stdout_pipe, MAX_REPLY_STDOUT_BYTES));
    let err_reader = std::thread::spawn(move || bounded_read(stderr_pipe, MAX_REPLY_STDERR_BYTES));

    // Write the one request document, then close stdin so the agent's
    // read-to-EOF completes.
    //
    // The write is no longer discarded: a partial write followed by an error
    // hands the peer a truncated document that still looks well formed, and
    // idempotency is keyed on that document's own digest. A BROKEN PIPE is
    // the deliberate exception — it means the child is already gone and its
    // exit status is the real diagnostic. A missing remote `dmux` closes
    // stdin exactly that way, and must stay the terminal `CommandMissing`
    // class instead of becoming a retryable transport failure that re-sends
    // the request to every other route.
    if let Some(mut stdin) = child.stdin.take()
        && let Err(error) = stdin.write_all(request_bytes)
        && error.kind() != std::io::ErrorKind::BrokenPipe
    {
        drop(stdin);
        abandon_child(&mut child, isolation, out_reader, err_reader);
        return Err(SpawnFailure {
            detail: format!("writing the request to {program} stdin: {error}"),
        });
    }

    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // The direct child is gone; close any write end an inherited
                // descendant still holds so both captures reach EOF.
                if isolation == TransportIsolation::OwnProcessGroup {
                    kill_process_group(child.id());
                }
                let stdout = join_capture(out_reader).ok_or_else(|| SpawnFailure {
                    detail: format!(
                        "{program} exited but its stdout pipe stayed open past {}ms — \
                         a surviving descendant still holds it",
                        READER_DRAIN_GRACE.as_millis()
                    ),
                })?;
                let stderr = join_capture(err_reader).unwrap_or_default();
                return Ok(RawReply {
                    status: status.code().unwrap_or(-1),
                    stdout: into_lossy_string(stdout.bytes),
                    stderr: into_lossy_string(stderr.bytes),
                    stdout_truncated: stdout.truncated,
                });
            }
            Ok(None) => {
                if started.elapsed() >= deadline {
                    abandon_child(&mut child, isolation, out_reader, err_reader);
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
                abandon_child(&mut child, isolation, out_reader, err_reader);
                return Err(SpawnFailure {
                    detail: format!("wait {program}: {e}"),
                });
            }
        }
    }
}

/// Kill the transport child — and, when it was isolated, everything it
/// spawned — then let both readers finish, mirroring the ordering of
/// `wez_compat::run_probe`: group kill, direct kill, reap, join.
///
/// The group kill comes first so the inherited pipe write ends are already
/// closed by the time the joins run; the direct `kill`/`wait` still runs so
/// a shared-group child is terminated and reaped rather than left a zombie.
fn abandon_child(
    child: &mut Child,
    isolation: TransportIsolation,
    out_reader: JoinHandle<BoundedCapture>,
    err_reader: JoinHandle<BoundedCapture>,
) {
    if isolation == TransportIsolation::OwnProcessGroup {
        kill_process_group(child.id());
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = join_capture(out_reader);
    let _ = join_capture(err_reader);
}

/// Join one bounded reader, or abandon it after [`READER_DRAIN_GRACE`].
///
/// `std` has no timed join, and an untimed one is the whole hazard: a
/// descendant holding the inherited write end keeps the reader blocked in
/// `read`, so a plain `join()` after the deadline would void the deadline.
/// Abandoning the thread costs one bounded buffer that is freed when the
/// pipe finally closes; blocking on it costs the guarantee.
fn join_capture(reader: JoinHandle<BoundedCapture>) -> Option<BoundedCapture> {
    let until = Instant::now() + READER_DRAIN_GRACE;
    while !reader.is_finished() {
        if Instant::now() >= until {
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    reader.join().ok()
}

/// `String::from_utf8_lossy(&bytes).into_owned()` without the second
/// allocation: valid UTF-8 (the only shape a real reply has) reuses the
/// capture's buffer, and only the lossy path — byte-identical to the old
/// one — allocates.
fn into_lossy_string(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
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

/// One frozen OpenSSH exit-255 diagnostic, recognized inside a SINGLE
/// stderr line rather than anywhere in the captured blob.
///
/// The blob is not ssh's alone: the same stream carries whatever the remote
/// command wrote, so a bare `contains` over it lets a peer pick this
/// client's failure class with one line of prose.  ssh writes its
/// diagnostics as `<context>: <context>: <reason>`, so a real one always
/// OPENS one of a line's `": "`-delimited segments — `opener` is anchored
/// to a segment start for exactly that reason, and prose that merely
/// mentions the phrase mid-sentence opens no segment.
///
/// `also` names the rest of a two-part OpenSSH sentence whose middle is
/// host-specific; it must follow `opener` within that same segment.
struct SshDiagnostic {
    opener: &'static str,
    also: Option<&'static str>,
}

/// A one-part diagnostic: the segment begins with this reason.
const fn shape(opener: &'static str) -> SshDiagnostic {
    SshDiagnostic { opener, also: None }
}

/// A two-part OpenSSH sentence: the segment begins with `opener` and the
/// rest of the sentence follows somewhere after the host-specific middle.
const fn sentence(opener: &'static str, also: &'static str) -> SshDiagnostic {
    SshDiagnostic {
        opener,
        also: Some(also),
    }
}

/// The pre-authentication transport stderr classes (§8.3, exhaustive by
/// anchored shape on ssh's exit-255 diagnostics).
const TRANSPORT_STDERR: &[SshDiagnostic] = &[
    shape("connection refused"),
    // Also covers the "connection reset by peer" spelling.
    shape("connection reset"),
    // The two real timeout spellings only. A bare "timed out" used to be
    // listed as a catch-all and matched anywhere in the blob, which let peer
    // prose promote an unclassified — deliberately terminal — 255 into the
    // one retryable class.
    shape("connection timed out"),
    shape("operation timed out"),
    shape("no route to host"),
    shape("network is unreachable"),
    shape("could not resolve hostname"),
    shape("name or service not known"),
    shape("temporary failure in name resolution"),
    shape("nodename nor servname provided"),
    // The server closed the TCP stream during connect/kex — before
    // authentication ("Connection closed by <addr> port <n>"; sshd rate
    // limiting/MaxStartups early drop presents this way, as does
    // "kex_exchange_identification: Connection closed by remote host").
    // Post-auth closures print "Connection to <host> closed" instead, which
    // opens no segment with this reason and so does NOT match.
    shape("connection closed by"),
];

const HOSTKEY_STDERR: &[SshDiagnostic] = &[
    shape("host key verification failed"),
    // "WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!" — the warning
    // banner opens a segment after "warning: ".
    shape("remote host identification has changed"),
    // "Unable to negotiate with <addr> port <n>: no matching host key type
    // found. Their offer: ..." — the reason opens the second segment.
    shape("no matching host key type"),
    // The WHOLE OpenSSH sentence, not the bare phrase. "host key for" on
    // its own is three common words: matched loosely it let one line of
    // peer-authored text force the terminal HostKey class, disabling route
    // failover and persisting `host_key_failed` — the strongest trust
    // signal the route table records — on a lie.
    sentence(
        "host key for",
        "has changed and you have requested strict checking",
    ),
];

const AUTH_STDERR: &[SshDiagnostic] = &[
    // "<user>@<host>: Permission denied (publickey)." — the reason opens
    // the segment after the destination.
    shape("permission denied"),
    // "Received disconnect from <addr> port <n>:<r>: Too many
    // authentication failures".
    shape("too many authentication failures"),
    shape("authentication failed"),
    shape("no supported authentication methods"),
];

/// The `": "`-delimited segments of one diagnostic line, from the whole line
/// down to each tail that begins just after a separator.
fn diagnostic_segments(line: &str) -> impl Iterator<Item = &str> {
    std::iter::once(line).chain(
        line.match_indices(": ")
            .map(|(at, separator)| &line[at + separator.len()..]),
    )
}

/// Does any single line of `lines` (already trimmed and lowercased) carry
/// one of `shapes`?
fn any_line_matches(lines: &[String], shapes: &[SshDiagnostic]) -> bool {
    lines.iter().any(|line| {
        diagnostic_segments(line).any(|segment| {
            shapes.iter().any(|shape| {
                segment
                    .strip_prefix(shape.opener)
                    .is_some_and(|rest| shape.also.is_none_or(|tail| rest.contains(tail)))
            })
        })
    })
}

/// Classify one ssh exit-255 stderr. Unknown 255 diagnostics are NOT
/// retried: retry eligibility must be positively enumerated (§8.3).
///
/// Matching is per line and anchored (see [`SshDiagnostic`]), because this
/// capture also carries the remote command's stderr. That raises the bar
/// from "any prose containing the phrase" to "a line that IS the ssh
/// sentence"; it cannot make a merged stream attributable, so a peer able to
/// reproduce a diagnostic verbatim can still pick a class. Anchoring is what
/// keeps ordinary output — a log line, a banner, a build message — from
/// doing it by accident.
pub fn classify_ssh_255(stderr: &str) -> AttemptFailure {
    let lines: Vec<String> = stderr
        .lines()
        .map(|line| line.trim().to_lowercase())
        .filter(|line| !line.is_empty())
        .collect();
    let detail = || {
        stderr
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("ssh exited 255")
            .to_string()
    };
    // Class-major, host-key classes first: "Host key verification failed"
    // must never be mistaken for a generic failure, wherever in the capture
    // it appears relative to the other lines.
    if any_line_matches(&lines, HOSTKEY_STDERR) {
        return AttemptFailure::HostKey { detail: detail() };
    }
    if any_line_matches(&lines, AUTH_STDERR) {
        return AttemptFailure::Auth { detail: detail() };
    }
    if any_line_matches(&lines, TRANSPORT_STDERR) {
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
    // Checked before the parse, and regardless of whether the prefix happens
    // to parse: a capture that hit its cap is not the document the peer
    // sent, and an oversized reply must be a typed failure rather than a
    // silently short buffer whose parse error names a column instead of the
    // bound it broke.
    if reply.stdout_truncated {
        return Err(AttemptFailure::Malformed {
            detail: format!(
                "agent response exceeded the {MAX_REPLY_STDOUT_BYTES}-byte reply bound"
            ),
        });
    }
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
///
/// Keeps this process's group, so enrollment's non-BatchMode hello can still
/// prompt on the terminal; the route walk uses [`call_argv_with`] and
/// [`TransportIsolation::OwnProcessGroup`].
pub fn call_argv(
    argv: &[String],
    request_bytes: &[u8],
    request_uid: Uuid,
    deadline: Duration,
) -> Result<Envelope, AttemptFailure> {
    call_argv_with(
        argv,
        request_bytes,
        request_uid,
        deadline,
        TransportIsolation::SharedProcessGroup,
    )
}

/// [`call_argv`], with the transport child's process-group isolation chosen
/// explicitly.
pub fn call_argv_with(
    argv: &[String],
    request_bytes: &[u8],
    request_uid: Uuid,
    deadline: Duration,
    isolation: TransportIsolation,
) -> Result<Envelope, AttemptFailure> {
    match exchange_with(argv, request_bytes, deadline, isolation) {
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
        // The route walk is the bounded machine-to-machine RPC: BatchMode
        // means the child never prompts, so it can be isolated and its
        // descendants signalled when the deadline expires.
        match call_argv_with(
            &argv,
            &request_bytes,
            request.request_uid,
            deadline,
            TransportIsolation::OwnProcessGroup,
        ) {
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

/// Call one already identity-verified, enabled route by its registry row id.
///
/// This is intentionally narrower than [`call_over_routes`]: it performs no
/// fallback.  It exists for the interactive tmux attach channel, where a
/// fresh hello first establishes the winning route and the subsequently
/// minted single-use token must be bound to that exact route.  The common
/// envelope, response identity, lineage, and typed-outcome validation remain
/// identical to the normal route walk.
pub fn call_over_pinned_route(
    registry: &mut Registry,
    expectation: &PeerExpectation,
    route_id: i64,
    request: &Envelope,
    invoker: &dyn RouteInvoker,
    invocation: &AgentInvocation,
    deadline: Duration,
) -> Result<RouteCallOutcome, TypedError> {
    let routes = registry
        .routes_for(expectation.host_uid)
        .map_err(|e| TypedError::new(e.error_code(), e.to_string()))?;
    let route = crate::remote::routes::eligible(routes, expectation.need_capability.as_deref())
        .into_iter()
        .find(|route| route.route_id == route_id)
        .ok_or_else(|| {
            TypedError::new(
                ErrorCode::RouteUnavailable,
                format!(
                    "verified route {route_id} is no longer enabled/eligible for host {}",
                    expectation.host_uid.0
                ),
            )
        })?;
    let request_bytes = serde_json::to_vec(request)
        .map_err(|e| TypedError::new(ErrorCode::OperationFailed, e.to_string()))?;
    let argv = invoker.argv_for(&route, invocation);
    match call_argv_with(
        &argv,
        &request_bytes,
        request.request_uid,
        deadline,
        TransportIsolation::OwnProcessGroup,
    ) {
        Ok(envelope) => match validate_identity_and_lineage(registry, expectation, &envelope) {
            Ok(lineage) => {
                let _ = registry.record_route_outcome(route.route_id, outcome::OK);
                Ok(RouteCallOutcome {
                    envelope,
                    route_id: route.route_id,
                    lineage,
                })
            }
            Err(failure) => {
                let _ = registry.record_route_outcome(route.route_id, failure.outcome_token());
                Err(failure.typed_error())
            }
        },
        Err(failure) => {
            let _ = registry.record_route_outcome(route.route_id, failure.outcome_token());
            // Even a normally retryable transport class is terminal here:
            // retrying another route would violate the token's route bind.
            Err(failure.typed_error())
        }
    }
}

/// Call one recovery verb over production SSH transport.  The caller opens
/// the production registry and resolves the enrolled HostUid; this helper
/// owns every protocol/route/lineage detail after that point.
pub fn call_recovery_owner(
    registry: &mut Registry,
    context: RecoveryOwnerContext,
    command: RecoveryOwnerCommand,
) -> Result<RecoveryOwnerOutcome, TypedError> {
    let method = command.method();
    call_recovery_owner_with(
        registry,
        context,
        command,
        &SshInvoker::default(),
        &AgentInvocation::new(method),
        DEFAULT_DEADLINE,
    )
}

/// Injectable form of [`call_recovery_owner`] used by the real two-registry
/// protocol tests.  `invocation` supplies only the transport binary and
/// scratch owner paths: method/protocol are forcibly set from `command`, so
/// a caller cannot make the argv disagree with the signed envelope.
pub fn call_recovery_owner_with(
    registry: &mut Registry,
    context: RecoveryOwnerContext,
    command: RecoveryOwnerCommand,
    invoker: &dyn RouteInvoker,
    invocation: &AgentInvocation,
    deadline: Duration,
) -> Result<RecoveryOwnerOutcome, TypedError> {
    let method = command.method();
    let identity = registry
        .identity()
        .map_err(|error| TypedError::new(error.error_code(), error.to_string()))?;
    let head = registry
        .authority_head()
        .map_err(|error| TypedError::new(error.error_code(), error.to_string()))?;
    let mut request = request_envelope(
        &identity,
        &head,
        method,
        Uuid::new_v4(),
        serde_json::json!({}),
    );
    request.backend_instance_uid = context.backend_instance_uid;
    request.server_epoch = context.server_epoch;

    let mut exact_invocation = invocation.clone();
    exact_invocation.method = method.to_string();
    exact_invocation.protocol = PROTOCOL_VERSION;
    let outcome = call_over_routes(
        registry,
        &PeerExpectation {
            host_uid: context.host_uid,
            need_capability: None,
            claimed_current: false,
        },
        &request,
        invoker,
        &exact_invocation,
        deadline,
    )?;
    if outcome.envelope.method != method {
        return Err(recovery_protocol_error(format!(
            "owner response method {:?} does not match request {method:?}",
            outcome.envelope.method
        )));
    }
    let payload = outcome.envelope.payload.clone().ok_or_else(|| {
        recovery_protocol_error("successful owner recovery response omitted payload")
    })?;

    let reply = match command {
        RecoveryOwnerCommand::Status => {
            let inspection: RecoveryInspection =
                serde_json::from_value(payload).map_err(|error| {
                    recovery_protocol_error(format!("owner recovery_status payload: {error}"))
                })?;
            verify_recovery_incarnation(
                context,
                &outcome.envelope,
                inspection.backend_instance_uid,
                inspection.server_epoch,
            )?;
            RecoveryOwnerReply::Status(inspection)
        }
        RecoveryOwnerCommand::Resume | RecoveryOwnerCommand::Abort => {
            let receipt: RecoveryControlRequest =
                serde_json::from_value(payload).map_err(|error| {
                    recovery_protocol_error(format!("owner {method} payload: {error}"))
                })?;
            verify_recovery_incarnation(
                context,
                &outcome.envelope,
                receipt.backend_instance_uid,
                Some(receipt.server_epoch),
            )?;
            let expected = command
                .control_action()
                .expect("control command has an expected action");
            if receipt.action != expected {
                return Err(recovery_protocol_error(format!(
                    "owner {method} returned {:?} control receipt",
                    receipt.action
                )));
            }
            RecoveryOwnerReply::Control(receipt)
        }
    };
    Ok(RecoveryOwnerOutcome {
        reply,
        route_id: outcome.route_id,
        lineage: outcome.lineage,
    })
}

fn verify_recovery_incarnation(
    context: RecoveryOwnerContext,
    envelope: &Envelope,
    instance: BackendInstanceUid,
    epoch: Option<ServerEpoch>,
) -> Result<(), TypedError> {
    if envelope.backend_instance_uid != Some(instance) {
        return Err(recovery_protocol_error(format!(
            "owner recovery payload names backend instance {} but envelope names {:?}",
            instance.0,
            envelope.backend_instance_uid.map(|value| value.0)
        )));
    }
    if envelope.server_epoch != epoch {
        return Err(recovery_protocol_error(format!(
            "owner recovery payload names epoch {:?} but envelope names {:?}",
            epoch.map(|value| value.0),
            envelope.server_epoch.map(|value| value.0)
        )));
    }
    if let Some(expected) = context.backend_instance_uid
        && expected != instance
    {
        return Err(recovery_protocol_error(format!(
            "owner response changed qualified backend instance {} to {}",
            expected.0, instance.0
        )));
    }
    if let Some(expected) = context.server_epoch
        && Some(expected) != epoch
    {
        return Err(recovery_protocol_error(format!(
            "owner response changed qualified server epoch {} to {:?}",
            expected.0,
            epoch.map(|value| value.0)
        )));
    }
    Ok(())
}

fn recovery_protocol_error(message: impl Into<String>) -> TypedError {
    TypedError::new(ErrorCode::ProtocolMismatch, message)
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
        // Propagated, exactly like the read above it. This checkpoint is the
        // anti-rollback anchor, not diagnostic state: with none stored,
        // `lineage::assess` answers `FirstContact` for ANY presented
        // identity, so the Conflict and RollbackSuspect branches are dead
        // code and a peer may swap its RegistryUid unchallenged; with a
        // stale one, a replayed older revision grades `Current` instead of
        // `RollbackSuspect`. A discarded error here is therefore not "retry
        // next time" — it pins this host in the weakest lineage state for
        // good, and reports it as a clean assessment. A busy registry
        // (`store_peer_cache` runs an immediate transaction) and a revision
        // the store refuses rather than narrows both land here.
        registry
            .store_peer_cache(
                expectation.host_uid,
                &PeerCache {
                    registry_uid: presented.registry_uid,
                    authority_revision: presented.revision,
                    authority_head_hash: presented.head_hash.clone(),
                    snapshot_json: snapshot,
                    fetched_at: now_rfc3339(),
                },
            )
            .map_err(|e| AttemptFailure::Malformed {
                detail: format!("persisting the peer lineage checkpoint: {e}"),
            })?;
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
            ..RawReply::default()
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
            stderr: "bash: line 1: dmux: command not found\n".into(),
            ..RawReply::default()
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
            ..RawReply::default()
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

    // -----------------------------------------------------------------
    // Bounded captures and the isolated transport

    /// `/bin/sh -c <script>` as a transport argv.
    fn sh(script: &str) -> Vec<String> {
        vec!["/bin/sh".to_string(), "-c".to_string(), script.to_string()]
    }

    #[test]
    fn a_truncated_stdout_capture_is_a_typed_failure_not_a_parse_error() {
        // The bytes here are a perfectly good envelope: truncation alone
        // must refuse the reply, because a capture that hit its cap is a
        // prefix of what the peer sent, whatever it happens to parse as.
        let sent = uuid::Uuid::from_u128(31);
        let envelope = serde_json::json!({
            "protocol_version": 1,
            "request_uid": sent.to_string(),
            "method": "spaces",
            "payload_sha256": "00",
            "host_uid": uuid::Uuid::from_u128(2).to_string(),
            "registry_uid": uuid::Uuid::from_u128(3).to_string(),
            "authority_revision": 1,
            "authority_head_hash": "sha256:x",
            "capabilities": [],
            "payload": {}
        });
        let intact = RawReply {
            status: 0,
            stdout: envelope.to_string(),
            ..RawReply::default()
        };
        interpret_reply(&intact, sent).expect("the untruncated reply is accepted");

        let truncated = RawReply {
            stdout_truncated: true,
            ..intact
        };
        let failure = interpret_reply(&truncated, sent).unwrap_err();
        assert_eq!(failure.outcome_token(), outcome::MALFORMED_RESPONSE);
        assert!(!failure.retries_next_route());
        assert!(
            failure
                .typed_error()
                .message
                .contains(&MAX_REPLY_STDOUT_BYTES.to_string()),
            "the refusal names the bound it broke: {}",
            failure.typed_error().message
        );
    }

    #[test]
    fn a_firehose_reply_is_capped_and_refused_instead_of_buffered_whole() {
        // Without a cap this child's output is bounded only by the 30s
        // deadline and the link rate. `bounded_read` keeps draining past the
        // cap, so the child still runs to completion rather than wedging on
        // a full pipe.
        let over = MAX_REPLY_STDOUT_BYTES + 128 * 1024;
        let reply = exchange_with(
            &sh(&format!(
                "cat >/dev/null; dd if=/dev/zero bs=4096 count={} 2>/dev/null | tr '\\0' 'a'",
                over / 4096
            )),
            b"{}",
            Duration::from_secs(30),
            TransportIsolation::OwnProcessGroup,
        )
        .expect("the transport returns rather than allocating without limit");
        assert_eq!(reply.stdout.len(), MAX_REPLY_STDOUT_BYTES);
        assert!(reply.stdout_truncated);
        assert_eq!(reply.status, 0);

        let failure = interpret_reply(&reply, uuid::Uuid::nil()).unwrap_err();
        assert_eq!(failure.outcome_token(), outcome::MALFORMED_RESPONSE);
    }

    #[test]
    fn an_isolated_transport_does_not_wait_on_a_grandchild_holding_the_pipe() {
        // The direct child answers and exits at once but leaves a
        // backgrounded grandchild holding the inherited stdout/stderr write
        // ends, so neither pipe reaches EOF on the direct child's exit
        // alone. Only the group-wide kill closes them; without it this join
        // outlives the deadline it exists to enforce.
        let started = Instant::now();
        let reply = exchange_with(
            &sh("cat >/dev/null; sleep 30 & printf 'ok'; exit 0"),
            b"{}",
            Duration::from_secs(30),
            TransportIsolation::OwnProcessGroup,
        )
        .expect("the isolated transport returns");
        assert_eq!(reply.stdout, "ok");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a held pipe must not stall the exchange: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_shared_group_transport_gives_up_on_a_held_pipe_rather_than_blocking() {
        // Enrollment keeps this process's group so its host-key prompt
        // reaches the terminal, which means it cannot signal a group. The
        // bounded join is the only thing standing between a descendant
        // holding the pipe and a permanent hang, so it must end in a typed
        // failure well inside the caller's deadline.
        let started = Instant::now();
        let failure = exchange_with(
            &sh("cat >/dev/null; sleep 5 & printf 'ok'; exit 0"),
            b"{}",
            Duration::from_secs(60),
            TransportIsolation::SharedProcessGroup,
        )
        .expect_err("a pipe that never closes is a failure, not a hang");
        assert!(
            failure.detail.contains("stayed open"),
            "unexpected detail: {}",
            failure.detail
        );
        assert!(!failure.is_deadline());
        assert!(
            started.elapsed() < READER_DRAIN_GRACE + Duration::from_secs(3),
            "the grace period must bound the join: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn the_deadline_holds_when_the_whole_group_is_still_running() {
        let started = Instant::now();
        let failure = exchange_with(
            &sh("sleep 30 & sleep 30"),
            b"{}",
            Duration::from_millis(200),
            TransportIsolation::OwnProcessGroup,
        )
        .expect_err("the deadline elapses");
        assert!(failure.is_deadline());
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the deadline must bound the joins too: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_child_that_never_reads_the_request_is_still_classified_by_its_exit() {
        // A missing remote `dmux` closes stdin the instant the login shell
        // exits, so this write fails with EPIPE after a partial write. That
        // must NOT become a retryable transport failure: 127 is the terminal
        // "no agent over there" answer, and turning it into a route-walk
        // trigger would re-send the request to every other route.
        let request = vec![b'x'; 256 * 1024];
        let reply = exchange_with(
            &sh("printf 'sh: dmux: command not found\\n' >&2; exit 127"),
            &request,
            Duration::from_secs(30),
            TransportIsolation::OwnProcessGroup,
        )
        .expect("a child that ignores stdin still yields its exit status");
        assert_eq!(reply.status, 127);
        let failure = interpret_reply(&reply, uuid::Uuid::nil()).unwrap_err();
        assert_eq!(failure.outcome_token(), outcome::COMMAND_MISSING);
        assert!(!failure.retries_next_route());
    }

    // -----------------------------------------------------------------
    // Anchored ssh-255 classification

    #[test]
    fn peer_authored_prose_cannot_pick_the_failure_class() {
        // Every line here contains a phrase the old whole-blob `contains`
        // matched. None of them is an ssh diagnostic, so none may classify.
        for text in [
            // The exact substring that used to force the terminal HostKey
            // class, and with it `host_key_failed` on the route.
            "dmux-agent: host key for cluster-7 is rotated weekly\n",
            "remote: fatal: the host key for cluster-7 could not be read\n",
            // The inverse: prose that used to promote an unclassified 255
            // into the one retryable class.
            "nightly job timed out after 30s; see the log\n",
            "warning: the build timed out\n",
        ] {
            let failure = classify_ssh_255(text);
            assert_eq!(
                failure.outcome_token(),
                outcome::MALFORMED_RESPONSE,
                "peer prose classified: {text:?} -> {failure:?}"
            );
            assert!(!failure.retries_next_route(), "{text:?}");
        }

        // With a genuine ssh diagnostic alongside it, the diagnostic — not
        // the prose — decides. Before anchoring, the first line forced
        // HostKey here: failover disabled for the host and a false
        // host-key-compromise alarm persisted on the route.
        let mixed = "dmux-agent: host key for cluster-7 is rotated weekly\n\
                     ssh: connect to host archie port 22: Connection timed out\n";
        let failure = classify_ssh_255(mixed);
        assert_eq!(failure.outcome_token(), outcome::TRANSPORT_UNREACHABLE);
        assert!(failure.retries_next_route());

        // And the reverse direction stays terminal: peer prose about a
        // timeout cannot make an unclassified ssh failure retryable.
        let noisy = "dmux-agent: the nightly build timed out\n\
                     kex_exchange_identification: banner exchange broke\n";
        assert!(!classify_ssh_255(noisy).retries_next_route());
    }

    #[test]
    fn genuine_ssh_diagnostics_still_classify_after_anchoring() {
        for text in [
            "ssh: connect to host archie port 22: Connection refused",
            "ssh: connect to host archie port 22: Connection timed out",
            "ssh: connect to host archie port 22: Operation timed out",
            "ssh: connect to host archie port 22: No route to host",
            "ssh: connect to host archie port 22: Network is unreachable",
            "ssh: Could not resolve hostname nope: Name or service not known",
            "ssh: Could not resolve hostname nope: Temporary failure in name resolution",
            "ssh: Could not resolve hostname nope: nodename nor servname provided, or not known",
            "kex_exchange_identification: read: Connection reset by peer",
            "kex_exchange_identification: Connection closed by remote host",
            "Connection closed by 10.77.77.2 port 22",
        ] {
            let failure = classify_ssh_255(text);
            assert_eq!(
                failure.outcome_token(),
                outcome::TRANSPORT_UNREACHABLE,
                "{text:?} -> {failure:?}"
            );
        }

        for (text, token) in [
            ("Host key verification failed.", outcome::HOST_KEY_FAILED),
            (
                "@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\n\
                 WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!\n\
                 Offending ECDSA key in /home/fredrir/.ssh/known_hosts:3",
                outcome::HOST_KEY_FAILED,
            ),
            (
                "Host key for archie has changed and you have requested strict checking.",
                outcome::HOST_KEY_FAILED,
            ),
            (
                "Unable to negotiate with 10.0.0.1 port 22: no matching host key type found. \
                 Their offer: ssh-rsa",
                outcome::HOST_KEY_FAILED,
            ),
            (
                "fredrir@archie: Permission denied (publickey).",
                outcome::AUTH_FAILED,
            ),
            (
                "Received disconnect from 10.0.0.1 port 22:2: Too many authentication failures",
                outcome::AUTH_FAILED,
            ),
            (
                "No supported authentication methods available (server sent: publickey)",
                outcome::AUTH_FAILED,
            ),
        ] {
            let failure = classify_ssh_255(text);
            assert_eq!(failure.outcome_token(), token, "{text:?} -> {failure:?}");
            assert!(!failure.retries_next_route(), "{text:?}");
        }

        // "Connection to <host> closed" is a POST-auth closure and still
        // must not be read as the pre-auth "Connection closed by" class.
        assert_eq!(
            classify_ssh_255("Connection to archie closed.").outcome_token(),
            outcome::MALFORMED_RESPONSE
        );
    }

    // -----------------------------------------------------------------
    // Peer lineage checkpoint persistence

    fn scratch_registry() -> (tempfile::TempDir, Registry) {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = Registry::open(crate::registry::RegistryConfig::new(
            dir.path().join("registry.sqlite3"),
            dir.path().join("locks"),
        ))
        .expect("open scratch registry");
        (dir, registry)
    }

    fn peer_response(host: HostUid) -> Envelope {
        serde_json::from_value(serde_json::json!({
            "protocol_version": 1,
            "request_uid": uuid::Uuid::from_u128(77).to_string(),
            "method": "spaces",
            "payload_sha256": "00",
            "host_uid": host.0.to_string(),
            "registry_uid": uuid::Uuid::from_u128(88).to_string(),
            "authority_revision": 12,
            "authority_head_hash": "sha256:peer-head",
            "capabilities": [],
            "payload": {"spaces": []}
        }))
        .expect("a well-formed response envelope")
    }

    #[test]
    fn a_failed_lineage_checkpoint_write_is_propagated_not_discarded() {
        let (_dir, mut registry) = scratch_registry();
        let host = HostUid(uuid::Uuid::from_u128(0x51));
        let envelope = peer_response(host);
        let expectation = PeerExpectation {
            host_uid: host,
            need_capability: None,
            claimed_current: false,
        };

        // The host is not enrolled, so the checkpoint's foreign key refuses
        // the write — the same shape as a busy or read-only registry. A
        // discarded error here would return a clean `FirstContact` while
        // leaving no anchor, so the NEXT contact is `FirstContact` too and
        // the peer may present any RegistryUid it likes, forever.
        let failure = validate_identity_and_lineage(&mut registry, &expectation, &envelope)
            .expect_err("an unpersisted anti-rollback anchor is not a clean assessment");
        assert_eq!(failure.outcome_token(), outcome::MALFORMED_RESPONSE);
        assert!(
            failure.typed_error().message.contains("lineage checkpoint"),
            "unexpected message: {}",
            failure.typed_error().message
        );
        assert_eq!(registry.peer_cache(host).unwrap(), None);

        // Positive control: with the host enrolled the same response stores
        // its checkpoint and reports first contact, unchanged.
        registry.enroll_host(host, None).expect("enroll");
        let assessment = validate_identity_and_lineage(&mut registry, &expectation, &envelope)
            .expect("a storable checkpoint assesses cleanly");
        assert_eq!(assessment, PeerLineage::FirstContact);
        let cached = registry
            .peer_cache(host)
            .unwrap()
            .expect("the checkpoint is durable");
        assert_eq!(cached.authority_revision, 12);
        assert_eq!(cached.authority_head_hash, "sha256:peer-head");

        // And with the anchor in place the next identical response is
        // graded against it rather than trusted afresh.
        assert_eq!(
            validate_identity_and_lineage(&mut registry, &expectation, &envelope).unwrap(),
            PeerLineage::Current
        );
    }
}
