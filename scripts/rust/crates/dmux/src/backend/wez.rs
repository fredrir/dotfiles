//! Wez provider adapter (plan §11.1, P3b) — the strict READ side of the
//! frozen [`Provider`] contract against one exact `wezterm-mux-server`
//! unix socket.
//!
//! Endpoint semantics: `InventoryScope.endpoint` is the **exact socket
//! path** of the single enrolled backend instance. Every spawned command is
//! the ADR 001 frozen invocation template, built as an argv vector plus an
//! explicit environment delta — never a shell string:
//!
//! ```text
//! env -u WEZTERM_PANE -u TMUX -u TMUX_PANE \
//!   WEZTERM_UNIX_SOCKET=<exact-socket> \
//!   <wezterm-bin> --config-file <dmux-managed-config> \
//!   cli --no-auto-start <subcmd> [--format json]
//! ```
//!
//! Strictness rules (ADR 001, ADR 002):
//! - `WEZTERM_UNIX_SOCKET` must be **non-empty**: an empty value falls
//!   through to wezterm's own socket discovery (ADR 006 finding) — an empty
//!   `scope.endpoint` is treated as a typed programming error, never sent.
//! - `--no-auto-start` is always present; neither listing nor any CLI call
//!   may auto-start a server (auto-start would even drop `--config-file`,
//!   ADR 002).
//! - Every child gets a dmux-imposed deadline: a live-but-silent socket
//!   hangs the stock CLI forever; the `timeout` outcome is manufactured by
//!   dmux killing the child.
//! - Pre-flight identity probe before trusting output: stat the socket path
//!   and classify (`ENOENT` → stopped, wrong file type → malformed), then
//!   `connect(2)` errno (`ECONNREFUSED` → stale socket/stopped). This
//!   adapter runs on the owner host, so those classifications are
//!   owner-local proof per §8.1.
//! - Sentinel-in-list is the TOCTOU-immune handshake: the reserved
//!   `dmux:system:<epoch>` workspace must be present in the very `list`
//!   JSON the scan consumes (ADR 002). A missing or duplicated sentinel
//!   means an unmanaged/replaced server — the rows are never trusted.
//!
//! Identity seam (P5): [`IdentityExpectation`] carries the service-recorded
//! server PID and start token. When a PID is provided the probe verifies the
//! socket peer against it (`LOCAL_PEERPID` on macOS, `SO_PEERCRED` on
//! Linux). The start token rides in the seam for the P5 runtime-descriptor
//! wiring; a socket cannot prove a start token by itself.
//!
//! Exit codes and stderr text are diagnostics only, never the classifier
//! (ADR 001): typed outcomes come from the dmux-side probe, the sentinel
//! handshake, and JSON parsing of the response actually consumed.
//!
//! Mutations (create/rename/remove/group_*/split_*/presentation) land in P6
//! under fenced journals/leases; P3b freezes their exact argv builders
//! ([`spawn_workspace_invocation`] and friends) so P6 wires them under the
//! fences without re-deriving the template. Until then the trait's mutation
//! verbs return a typed [`ProviderError::NativeFailure`] naming P6.
//!
//! Specialist-owned (plan §19, W2); the trait and result types in
//! `backend/mod.rs` are the frozen root-owned contract.

use std::io::Read;
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;
use uuid::Uuid;

use crate::backend::{
    Capabilities, CreateSpec, InventoryOutcome, InventoryScope, NativeBinding, NativeGroupRow,
    NativeInventory, NativeSpaceRow, NativeSplitRow, PresentationTarget, Provider, ProviderError,
    ProviderResult,
};
use crate::model::{Backend, ProviderHandle, ServerEpoch, WEZ_SENTINEL_PREFIX};

/// Environment variables scrubbed from every child (ADR 001 template). A
/// provider running inside a WezTerm pane or a tmux client must never let
/// the ambient mux leak into endpoint selection.
pub const SCRUBBED_ENV: [&str; 3] = ["WEZTERM_PANE", "TMUX", "TMUX_PANE"];

/// The sole endpoint selector (ADR 001: config order and `--prefer-mux`
/// play no part). Must always be set non-empty.
pub const SOCKET_ENV: &str = "WEZTERM_UNIX_SOCKET";

/// Default per-child deadline. ADR 001: a silent socket hangs the stock CLI
/// for >12s with no built-in timeout; dmux kills the child at the deadline.
const DEFAULT_DEADLINE: Duration = Duration::from_secs(10);

/// Stable stderr emitted by a stock (codec-45) server answering the fork CAS
/// verb (ADR 006). Classifying this exact error as capability-missing is the
/// sanctioned **positive** probe path; connect success never implies
/// capability (the CLI performs no codec handshake at all).
pub const CAS_MISSING_PDU_STDERR: &str = "invalid PDU Invalid { ident: 63 }";

// ---------------------------------------------------------------------------
// Invocation template (pure builders)
// ---------------------------------------------------------------------------

/// One fully-specified child invocation: exact argv plus the environment
/// delta the runner must apply. Built by pure functions so unit tests assert
/// the frozen template byte-for-byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WezInvocation {
    /// `[wezterm-bin, --config-file, <cfg>, cli, --no-auto-start, ...]`.
    pub argv: Vec<String>,
    /// Exactly `[(WEZTERM_UNIX_SOCKET, <non-empty exact socket>)]`.
    pub env_set: Vec<(String, String)>,
    /// Exactly [`SCRUBBED_ENV`].
    pub env_remove: Vec<String>,
}

/// The ADR 001 frozen invocation template for one `wezterm cli` subcommand.
/// Fails on an empty socket: an empty `WEZTERM_UNIX_SOCKET` falls through to
/// wezterm's socket discovery (ADR 006) — that is a dmux programming error,
/// never a runtime condition to paper over.
pub fn cli_invocation(
    wezterm_bin: &str,
    config_file: &str,
    socket: &str,
    cli_args: &[&str],
) -> Result<WezInvocation, String> {
    if socket.is_empty() {
        return Err(format!(
            "empty {SOCKET_ENV} endpoint: an empty value falls through to wezterm \
             socket discovery (ADR 006); the exact service socket is mandatory"
        ));
    }
    let mut argv = Vec::with_capacity(cli_args.len() + 5);
    argv.push(wezterm_bin.to_string());
    argv.push("--config-file".to_string());
    argv.push(config_file.to_string());
    argv.push("cli".to_string());
    argv.push("--no-auto-start".to_string());
    argv.extend(cli_args.iter().map(|s| s.to_string()));
    Ok(WezInvocation {
        argv,
        env_set: vec![(SOCKET_ENV.to_string(), socket.to_string())],
        env_remove: SCRUBBED_ENV.iter().map(|s| s.to_string()).collect(),
    })
}

fn require_bootstrap(verb: &str, bootstrap_argv: &[String]) -> Result<(), String> {
    if bootstrap_argv.is_empty() {
        return Err(format!(
            "{verb} requires the bootstrap helper argv (ADR 004); \
             the provider never spawns a bare default shell"
        ));
    }
    Ok(())
}

/// Space create (plan §11.1): `cli --no-auto-start spawn --new-window
/// --workspace <opaque-key> [--cwd <dir>] -- <bootstrap argv...>`.
pub fn spawn_workspace_invocation(
    wezterm_bin: &str,
    config_file: &str,
    socket: &str,
    workspace_key: &str,
    cwd: Option<&str>,
    bootstrap_argv: &[String],
) -> Result<WezInvocation, String> {
    if workspace_key.is_empty() {
        return Err("spawn --workspace requires a non-empty opaque key".into());
    }
    require_bootstrap("spawn --new-window", bootstrap_argv)?;
    let mut args: Vec<String> = vec![
        "spawn".into(),
        "--new-window".into(),
        "--workspace".into(),
        workspace_key.into(),
    ];
    if let Some(cwd) = cwd {
        args.push("--cwd".into());
        args.push(cwd.into());
    }
    args.push("--".into());
    args.extend(bootstrap_argv.iter().cloned());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    cli_invocation(wezterm_bin, config_file, socket, &refs)
}

/// Group create (plan §11.1): `spawn --window-id <only-window-id> -- ...`.
pub fn spawn_group_invocation(
    wezterm_bin: &str,
    config_file: &str,
    socket: &str,
    window_id: u64,
    cwd: Option<&str>,
    bootstrap_argv: &[String],
) -> Result<WezInvocation, String> {
    require_bootstrap("spawn --window-id", bootstrap_argv)?;
    let mut args: Vec<String> = vec!["spawn".into(), "--window-id".into(), window_id.to_string()];
    if let Some(cwd) = cwd {
        args.push("--cwd".into());
        args.push(cwd.into());
    }
    args.push("--".into());
    args.extend(bootstrap_argv.iter().cloned());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    cli_invocation(wezterm_bin, config_file, socket, &refs)
}

/// Split create (plan §11.1): `split-pane --pane-id <exact-pane-id> -- ...`.
pub fn split_pane_invocation(
    wezterm_bin: &str,
    config_file: &str,
    socket: &str,
    pane_id: u64,
    cwd: Option<&str>,
    bootstrap_argv: &[String],
) -> Result<WezInvocation, String> {
    require_bootstrap("split-pane", bootstrap_argv)?;
    let mut args: Vec<String> = vec!["split-pane".into(), "--pane-id".into(), pane_id.to_string()];
    if let Some(cwd) = cwd {
        args.push("--cwd".into());
        args.push(cwd.into());
    }
    args.push("--".into());
    args.extend(bootstrap_argv.iter().cloned());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    cli_invocation(wezterm_bin, config_file, socket, &refs)
}

/// Group rename (plan §11.1): `set-tab-title --tab-id <exact-tab-id> TITLE`.
pub fn set_tab_title_invocation(
    wezterm_bin: &str,
    config_file: &str,
    socket: &str,
    tab_id: u64,
    title: &str,
) -> Result<WezInvocation, String> {
    cli_invocation(
        wezterm_bin,
        config_file,
        socket,
        &["set-tab-title", "--tab-id", &tab_id.to_string(), title],
    )
}

/// Removal building block (plan §11.1, ADR 005): `kill-pane --pane-id N`.
/// There is no public atomic kill-workspace; P6 drives bounded
/// re-list/kill convergence over this builder.
pub fn kill_pane_invocation(
    wezterm_bin: &str,
    config_file: &str,
    socket: &str,
    pane_id: u64,
) -> Result<WezInvocation, String> {
    cli_invocation(
        wezterm_bin,
        config_file,
        socket,
        &["kill-pane", "--pane-id", &pane_id.to_string()],
    )
}

// ---------------------------------------------------------------------------
// CAS capability probe seam (ADR 006, wired in P6)
// ---------------------------------------------------------------------------

/// Classification of one fork-CAS capability probe response. Capability is
/// proven only by a **positive** probe (`wezterm cli` has no codec
/// handshake; connect success is silently half-capable, never proof).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CasProbe {
    /// The CAS verb executed (exit 0): fork primitive present.
    Capable,
    /// The stable stock-server rejection of PDU ident 63: capability
    /// missing, guaranteed zero mutation (ADR 006).
    Missing,
    /// Anything else: not evidence in either direction.
    Indeterminate(String),
}

/// Pure classifier for the P6 capability probe. This is the one sanctioned
/// use of stderr in classification: ADR 006 froze the exact
/// `invalid PDU Invalid {{ ident: 63 }}` reason as the capability-missing
/// signal; everything else stays indeterminate diagnostics.
pub fn classify_cas_probe(exit_ok: bool, stderr: &str) -> CasProbe {
    if exit_ok {
        return CasProbe::Capable;
    }
    if stderr.contains(CAS_MISSING_PDU_STDERR) {
        return CasProbe::Missing;
    }
    CasProbe::Indeterminate(stderr.trim().to_string())
}

// ---------------------------------------------------------------------------
// Command-runner seam
// ---------------------------------------------------------------------------

/// Completed child process observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    /// Exit code; `-1` when terminated by a signal.
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl RunOutput {
    pub fn ok(&self) -> bool {
        self.status == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// The wezterm binary could not be spawned (`ENOENT`).
    MissingBinary { detail: String },
    /// The dmux-imposed deadline elapsed; the child was killed.
    Timeout { detail: String },
    /// Any other spawn/IO failure.
    Io { detail: String },
}

/// Pre-flight endpoint classification (ADR 001). Produced by dmux's own
/// stat/connect probe, never inferred from wezterm exit codes or stderr.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// Path is a socket, `connect(2)` succeeded, and — when an expected PID
    /// was supplied — the socket peer matched it.
    Connectable,
    /// `ENOENT`: nothing at the path (owner-local; sockets are never
    /// unlinked on shutdown, so absence means never started or cleaned up).
    Absent { detail: String },
    /// The path exists but is not a unix socket: invalid endpoint.
    NotSocket { detail: String },
    /// `ECONNREFUSED`: a stale socket file whose server is gone.
    Refused { detail: String },
    /// `EACCES`/`EPERM` on stat or connect.
    Denied { detail: String },
    /// A live socket whose peer PID does not match the service-recorded
    /// server PID: wrong backend instance (ADR 001 imposter case).
    WrongPeer { detail: String },
    /// Any other probe failure: indeterminate.
    Failed { detail: String },
}

/// Injectable execution seam: the provider builds exact invocations and the
/// runner (a) classifies the endpoint pre-flight and (b) executes argv under
/// a deadline. Unit tests substitute a scripted runner asserting exact
/// argv/env and feeding canned JSON; production uses [`SystemRunner`].
pub trait WezRunner {
    fn probe(&self, socket_path: &str, expected_server_pid: Option<u32>) -> ProbeOutcome;
    fn run(&self, invocation: &WezInvocation, deadline: Duration) -> Result<RunOutput, RunError>;
}

/// Real runner: `std::process::Command` over argv arrays, never a shell,
/// with the invocation's exact environment delta applied.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemRunner;

impl WezRunner for SystemRunner {
    fn probe(&self, socket_path: &str, expected_server_pid: Option<u32>) -> ProbeOutcome {
        use std::os::unix::fs::FileTypeExt;
        // Follow symlinks: wezterm resolves the env socket through
        // `connect(2)` semantics, so the published path may be a symlink
        // (spike 1 socket-replacement evidence).
        let meta = match std::fs::metadata(socket_path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return ProbeOutcome::Absent {
                    detail: format!("{socket_path}: {e}"),
                };
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return ProbeOutcome::Denied {
                    detail: format!("{socket_path}: {e}"),
                };
            }
            Err(e) => {
                return ProbeOutcome::Failed {
                    detail: format!("stat {socket_path}: {e}"),
                };
            }
        };
        if !meta.file_type().is_socket() {
            return ProbeOutcome::NotSocket {
                detail: format!("{socket_path} is not a unix socket"),
            };
        }
        let stream = match UnixStream::connect(socket_path) {
            Ok(stream) => stream,
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                return ProbeOutcome::Refused {
                    detail: format!("stale socket {socket_path}: {e}"),
                };
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return ProbeOutcome::Absent {
                    detail: format!("{socket_path}: {e}"),
                };
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return ProbeOutcome::Denied {
                    detail: format!("{socket_path}: {e}"),
                };
            }
            Err(e) => {
                return ProbeOutcome::Failed {
                    detail: format!("connect {socket_path}: {e}"),
                };
            }
        };
        if let Some(expected) = expected_server_pid {
            match socket_peer_pid(&stream) {
                Ok(peer) if peer == expected => {}
                Ok(peer) => {
                    return ProbeOutcome::WrongPeer {
                        detail: format!(
                            "socket peer pid {peer} != service-recorded server pid {expected}"
                        ),
                    };
                }
                Err(detail) => {
                    // Asked to verify identity but unable to: never trust.
                    return ProbeOutcome::Failed {
                        detail: format!("peer identity unverifiable: {detail}"),
                    };
                }
            }
        }
        ProbeOutcome::Connectable
    }

    fn run(&self, invocation: &WezInvocation, deadline: Duration) -> Result<RunOutput, RunError> {
        let (program, args) = invocation.argv.split_first().ok_or_else(|| RunError::Io {
            detail: "empty argv".into(),
        })?;
        let mut cmd = Command::new(program);
        cmd.args(args);
        for key in &invocation.env_remove {
            cmd.env_remove(key);
        }
        for (key, value) in &invocation.env_set {
            cmd.env(key, value);
        }
        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    RunError::MissingBinary {
                        detail: format!("{program}: {e}"),
                    }
                } else {
                    RunError::Io {
                        detail: format!("spawn {program}: {e}"),
                    }
                }
            })?;

        let mut stdout_pipe = child.stdout.take().expect("piped stdout");
        let mut stderr_pipe = child.stderr.take().expect("piped stderr");
        let stdout_reader = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stdout_pipe.read_to_end(&mut buf);
            buf
        });
        let stderr_reader = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stderr_pipe.read_to_end(&mut buf);
            buf
        });

        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let stdout = stdout_reader.join().unwrap_or_default();
                    let stderr = stderr_reader.join().unwrap_or_default();
                    return Ok(RunOutput {
                        status: status.code().unwrap_or(-1),
                        stdout,
                        stderr,
                    });
                }
                Ok(None) => {
                    if started.elapsed() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        let _ = stdout_reader.join();
                        let _ = stderr_reader.join();
                        return Err(RunError::Timeout {
                            detail: format!(
                                "{program} exceeded {}ms dmux deadline (ADR 001: the stock \
                                 CLI has no timeout of its own)",
                                deadline.as_millis()
                            ),
                        });
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => {
                    let _ = child.kill();
                    return Err(RunError::Io {
                        detail: format!("wait {program}: {e}"),
                    });
                }
            }
        }
    }
}

/// PID of the process on the far end of a connected unix socket.
#[cfg(target_os = "macos")]
fn socket_peer_pid(stream: &UnixStream) -> Result<u32, String> {
    use std::os::fd::AsRawFd;
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut len,
        )
    };
    if rc != 0 {
        return Err(format!(
            "LOCAL_PEERPID: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(pid as u32)
}

#[cfg(target_os = "linux")]
fn socket_peer_pid(stream: &UnixStream) -> Result<u32, String> {
    use std::os::fd::AsRawFd;
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if rc != 0 {
        return Err(format!("SO_PEERCRED: {}", std::io::Error::last_os_error()));
    }
    Ok(cred.pid as u32)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn socket_peer_pid(_stream: &UnixStream) -> Result<u32, String> {
    Err("peer-pid probe unsupported on this platform".into())
}

// ---------------------------------------------------------------------------
// Identity seam (P5)
// ---------------------------------------------------------------------------

/// Service-descriptor identity expectations (plan §15.1, ADR 001). The
/// descriptor itself arrives with P5; until then both fields default to
/// `None` and the probe verifies reachability + file type + connect only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdentityExpectation {
    /// Service-recorded server PID. When present the probe reads the socket
    /// peer PID (`LOCAL_PEERPID`/`SO_PEERCRED`) and fails typed on mismatch.
    pub server_pid: Option<u32>,
    /// Service-recorded start token. Carried through the seam now so the P5
    /// wiring has a stable shape; a socket cannot prove a start token by
    /// itself — P5 compares it against the runtime descriptor.
    pub start_token: Option<String>,
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// Wez adapter over an injectable runner. One provider instance serves one
/// managed backend instance; the constructor takes the wezterm binary path
/// and the dmux-managed `--config-file` path, while every scoped operation
/// targets `scope.endpoint` — the exact service socket.
pub struct WezProvider<R: WezRunner> {
    runner: R,
    wezterm_bin: String,
    config_file: String,
    deadline: Duration,
    identity: IdentityExpectation,
}

impl WezProvider<SystemRunner> {
    pub fn new(wezterm_bin: impl Into<String>, config_file: impl Into<String>) -> Self {
        Self::with_runner(wezterm_bin, config_file, SystemRunner)
    }
}

impl<R: WezRunner> WezProvider<R> {
    pub fn with_runner(
        wezterm_bin: impl Into<String>,
        config_file: impl Into<String>,
        runner: R,
    ) -> Self {
        WezProvider {
            runner,
            wezterm_bin: wezterm_bin.into(),
            config_file: config_file.into(),
            deadline: DEFAULT_DEADLINE,
            identity: IdentityExpectation::default(),
        }
    }

    pub fn with_deadline(mut self, deadline: Duration) -> Self {
        self.deadline = deadline;
        self
    }

    /// Install the P5 service-descriptor identity expectation.
    pub fn with_identity(mut self, identity: IdentityExpectation) -> Self {
        self.identity = identity;
        self
    }

    /// One verified scan: probe → exact-socket `list --format json` →
    /// sentinel/epoch handshake → grouped rows.
    fn scan(&self, scope: &InventoryScope) -> Result<NativeInventory, ScanFail> {
        if scope.backend != Backend::Wez {
            return Err(ScanFail::WrongInstance(format!(
                "wez provider handed a {} scope",
                scope.backend
            )));
        }
        if scope.endpoint.is_empty() {
            return Err(ScanFail::Malformed(format!(
                "empty {SOCKET_ENV} endpoint: an empty value falls through to wezterm \
                 socket discovery (ADR 006); exact endpoint required (programming error)"
            )));
        }
        match self.runner.probe(&scope.endpoint, self.identity.server_pid) {
            ProbeOutcome::Connectable => {}
            ProbeOutcome::Absent { detail } => {
                return Err(ScanFail::Stopped(format!("socket absent: {detail}")));
            }
            ProbeOutcome::Refused { detail } => {
                return Err(ScanFail::Stopped(format!("connection refused: {detail}")));
            }
            ProbeOutcome::NotSocket { detail } => {
                return Err(ScanFail::Malformed(format!("invalid endpoint: {detail}")));
            }
            ProbeOutcome::Denied { detail } => {
                return Err(ScanFail::Permission(detail));
            }
            ProbeOutcome::WrongPeer { detail } => {
                return Err(ScanFail::WrongInstance(detail));
            }
            ProbeOutcome::Failed { detail } => {
                return Err(ScanFail::Unreachable(detail));
            }
        }
        let invocation = cli_invocation(
            &self.wezterm_bin,
            &self.config_file,
            &scope.endpoint,
            &["list", "--format", "json"],
        )
        .map_err(ScanFail::Malformed)?;
        let out = match self.runner.run(&invocation, self.deadline) {
            Ok(out) => out,
            Err(RunError::MissingBinary { detail }) => {
                return Err(ScanFail::CommandMissing(detail));
            }
            Err(RunError::Timeout { detail }) => return Err(ScanFail::Timeout(detail)),
            Err(RunError::Io { detail }) => return Err(ScanFail::Malformed(detail)),
        };
        if !out.ok() {
            // The probe said connectable, yet the CLI failed: indeterminate.
            // stderr is carried as diagnostics only, never parsed for
            // classification (ADR 001).
            return Err(ScanFail::Malformed(format!(
                "wezterm cli list exited {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        let stdout = String::from_utf8(out.stdout)
            .map_err(|e| ScanFail::Malformed(format!("non-utf8 wezterm list output: {e}")))?;
        let rows: Vec<ListRow> = serde_json::from_str(&stdout)
            .map_err(|e| ScanFail::Malformed(format!("unparseable wezterm list JSON: {e}")))?;

        // Sentinel handshake (ADR 002): exactly one distinct
        // `dmux:system:<epoch>` workspace must ride in this very response.
        let mut sentinels: Vec<&str> = rows
            .iter()
            .filter_map(|r| {
                r.workspace
                    .starts_with(WEZ_SENTINEL_PREFIX)
                    .then_some(r.workspace.as_str())
            })
            .collect();
        sentinels.sort_unstable();
        sentinels.dedup();
        let sentinel = match sentinels.as_slice() {
            [] => {
                return Err(ScanFail::Malformed(format!(
                    "sentinel missing: no {WEZ_SENTINEL_PREFIX}<epoch> workspace in list \
                     (ADR 002: unmanaged or replaced server; rows discarded)"
                )));
            }
            [one] => *one,
            many => {
                return Err(ScanFail::Malformed(format!(
                    "sentinel duplicate: {} distinct {WEZ_SENTINEL_PREFIX}* workspaces \
                     ({}); backend unavailable, rows discarded",
                    many.len(),
                    many.join(", ")
                )));
            }
        };
        let epoch_text = &sentinel[WEZ_SENTINEL_PREFIX.len()..];
        let epoch = ServerEpoch(Uuid::parse_str(epoch_text).map_err(|e| {
            ScanFail::Malformed(format!("unparseable sentinel epoch {epoch_text:?}: {e}"))
        })?);
        if let Some(expected) = scope.expected_epoch
            && expected != epoch
        {
            return Err(ScanFail::EpochChanged {
                expected,
                observed: Some(epoch),
            });
        }

        let user_rows: Vec<&ListRow> = rows
            .iter()
            .filter(|r| !r.workspace.starts_with(WEZ_SENTINEL_PREFIX))
            .collect();
        let rows = assemble_rows(&user_rows).map_err(ScanFail::Malformed)?;
        Ok(NativeInventory {
            server_epoch: Some(epoch),
            rows,
        })
    }

    /// Scan mapped for `ProviderResult` read verbs.
    fn scan_complete(&self, scope: &InventoryScope) -> ProviderResult<NativeInventory> {
        self.scan(scope).map_err(|f| f.into_provider_error())
    }

    /// Cross-check a stale binding against the caller's scope before use.
    fn binding_epoch(
        scope: &InventoryScope,
        binding: &NativeBinding,
    ) -> ProviderResult<ServerEpoch> {
        if let Some(expected) = scope.expected_epoch
            && expected != binding.server_epoch
        {
            return Err(ProviderError::EpochChanged {
                expected,
                observed: Some(binding.server_epoch),
            });
        }
        Ok(binding.server_epoch)
    }

    /// P6 boundary: every Wez mutation runs under the fenced operation
    /// journal/lease machinery that lands in P6. The argv builders above are
    /// frozen now precisely so P6 wires them under those fences; calling a
    /// mutation verb today is a typed failure, never a silent no-op.
    fn mutation_unimplemented(verb: &str) -> ProviderError {
        ProviderError::NativeFailure {
            detail: format!(
                "wez {verb}: wez mutations land in P6 (fenced journal/lease \
                 integration); P3b freezes the argv builders only"
            ),
        }
    }
}

/// Internal typed scan failure; carries enough structure for both the
/// `InventoryOutcome` and `ProviderError` mappings.
enum ScanFail {
    /// Owner-local proof (this adapter runs on the owner host): ENOENT or
    /// ECONNREFUSED on the exact service socket.
    Stopped(String),
    Unreachable(String),
    CommandMissing(String),
    Malformed(String),
    Timeout(String),
    Permission(String),
    EpochChanged {
        expected: ServerEpoch,
        observed: Option<ServerEpoch>,
    },
    WrongInstance(String),
}

impl ScanFail {
    fn into_outcome(self) -> InventoryOutcome {
        match self {
            ScanFail::Stopped(detail) => InventoryOutcome::ServerStopped { detail },
            ScanFail::Unreachable(detail) => InventoryOutcome::Unreachable { detail },
            ScanFail::CommandMissing(detail) => InventoryOutcome::CommandMissing { detail },
            ScanFail::Malformed(detail) => InventoryOutcome::Malformed { detail },
            ScanFail::Timeout(detail) => InventoryOutcome::Timeout { detail },
            ScanFail::Permission(detail) => InventoryOutcome::PermissionFailure { detail },
            // The orchestration layer maps this detail to the
            // `backend_epoch_changed` error code (plan §8.1).
            ScanFail::EpochChanged { expected, observed } => InventoryOutcome::Malformed {
                detail: format!(
                    "backend_epoch_changed: expected {} observed {}",
                    expected.0,
                    observed.map_or("none".to_string(), |e| e.0.to_string())
                ),
            },
            ScanFail::WrongInstance(detail) => InventoryOutcome::Malformed {
                detail: format!("wrong_backend_instance: {detail}"),
            },
        }
    }

    fn into_provider_error(self) -> ProviderError {
        match self {
            ScanFail::Stopped(detail) => ProviderError::NativeFailure {
                detail: format!("wez server stopped: {detail}"),
            },
            ScanFail::Unreachable(detail) => ProviderError::NativeFailure {
                detail: format!("wez endpoint unreachable: {detail}"),
            },
            ScanFail::CommandMissing(detail) => ProviderError::NativeFailure {
                detail: format!("wezterm binary missing: {detail}"),
            },
            ScanFail::Malformed(detail) => ProviderError::NativeFailure { detail },
            ScanFail::Timeout(detail) => ProviderError::Timeout { detail },
            ScanFail::Permission(detail) => ProviderError::NativeFailure {
                detail: format!("wez endpoint permission failure: {detail}"),
            },
            ScanFail::EpochChanged { expected, observed } => {
                ProviderError::EpochChanged { expected, observed }
            }
            ScanFail::WrongInstance(detail) => ProviderError::WrongInstance { detail },
        }
    }
}

// ---------------------------------------------------------------------------
// List parsing and grouping (fixture-tested)
// ---------------------------------------------------------------------------

/// One `cli list --format json` pane row (spike 1/5 evidence schema:
/// window_id, tab_id, pane_id, workspace, size, title, cwd, tab_title, ...).
/// Unknown fields are ignored; missing required IDs are a malformed scan.
#[derive(Debug, Deserialize)]
struct ListRow {
    window_id: u64,
    tab_id: u64,
    pane_id: u64,
    workspace: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    tab_title: Option<String>,
}

/// Parse a wezterm cwd URI (`file://host/path`) into a plain path. Keeps
/// the raw string when it does not parse (plan §11.1); host-matching policy
/// (plan §11.3) lives above the provider.
fn parse_cwd_uri(raw: &str) -> String {
    let Some(rest) = raw.strip_prefix("file://") else {
        return raw.to_string();
    };
    // `rest` is `<authority><absolute path>`; the path starts at the first
    // slash (empty authority for local paths: `file:///Users/...`).
    let Some(slash) = rest.find('/') else {
        return raw.to_string();
    };
    match percent_decode(&rest[slash..]) {
        Some(path) => path,
        None => raw.to_string(),
    }
}

/// Minimal percent-decoder; `None` on malformed escapes or non-UTF8.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let hex = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn non_empty(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Group user pane rows: workspace key → unique `tab_id` Groups → `pane_id`
/// Splits, in first-seen order. `window_id` feeds ONLY the one-window
/// diagnosis (`multi_window` = more than one distinct window in the
/// workspace, plan §2.3) — it is never the Group count (§11.1). Native IDs
/// are globally unique in wezterm; any duplicate or a tab spanning two
/// workspaces/windows means the response is inconsistent and the scan is
/// reported malformed, never guessed at.
fn assemble_rows(rows: &[&ListRow]) -> Result<Vec<NativeSpaceRow>, String> {
    let mut spaces: Vec<NativeSpaceRow> = Vec::new();
    // (tab_id -> (workspace, window_id)), (pane_id) integrity tracking.
    let mut tab_owner: Vec<(u64, String, u64)> = Vec::new();
    let mut window_ids: Vec<(String, Vec<u64>)> = Vec::new();
    let mut seen_panes: Vec<u64> = Vec::new();

    for row in rows {
        if seen_panes.contains(&row.pane_id) {
            return Err(format!(
                "duplicate pane_id {} in list response",
                row.pane_id
            ));
        }
        seen_panes.push(row.pane_id);
        match tab_owner.iter().find(|(tab, _, _)| *tab == row.tab_id) {
            Some((_, ws, win)) => {
                if *ws != row.workspace || *win != row.window_id {
                    return Err(format!(
                        "tab_id {} spans ({ws:?}, window {win}) and ({:?}, window {}): \
                         inconsistent list response",
                        row.tab_id, row.workspace, row.window_id
                    ));
                }
            }
            None => {
                tab_owner.push((row.tab_id, row.workspace.clone(), row.window_id));
            }
        }

        let space = match spaces.iter_mut().find(|s| s.native_token == row.workspace) {
            Some(space) => space,
            None => {
                spaces.push(NativeSpaceRow {
                    native_token: row.workspace.clone(),
                    native_name: row.workspace.clone(),
                    groups: Vec::new(),
                    multi_window: false,
                });
                window_ids.push((row.workspace.clone(), Vec::new()));
                spaces.last_mut().expect("just pushed")
            }
        };
        let windows = &mut window_ids
            .iter_mut()
            .find(|(ws, _)| *ws == row.workspace)
            .expect("window tracker in step with spaces")
            .1;
        if !windows.contains(&row.window_id) {
            windows.push(row.window_id);
        }

        let handle = ProviderHandle::Wz(row.tab_id);
        let group = match space.groups.iter_mut().find(|g| g.handle == handle) {
            Some(group) => group,
            None => {
                space.groups.push(NativeGroupRow {
                    handle,
                    title: non_empty(&row.tab_title),
                    splits: Vec::new(),
                });
                space.groups.last_mut().expect("just pushed")
            }
        };
        if group.title.is_none() {
            group.title = non_empty(&row.tab_title);
        }
        group.splits.push(NativeSplitRow {
            handle: ProviderHandle::Wz(row.pane_id),
            title: non_empty(&row.title),
            cwd: non_empty(&row.cwd).map(|c| parse_cwd_uri(&c)),
        });
    }

    for space in &mut spaces {
        let windows = &window_ids
            .iter()
            .find(|(ws, _)| *ws == space.native_token)
            .expect("window tracker in step with spaces")
            .1;
        space.multi_window = windows.len() > 1;
    }
    Ok(spaces)
}

// ---------------------------------------------------------------------------
// Provider impl
// ---------------------------------------------------------------------------

impl<R: WezRunner> Provider for WezProvider<R> {
    /// Static P3b capabilities. `probed` names the checks every inventory
    /// enforces (dmux-side socket classification and the sentinel-in-list
    /// handshake). `cas_rename` stays `false` until P6 runs the POSITIVE
    /// capability probe against the pinned fork build — fork version match,
    /// or [`classify_cas_probe`] on a live CAS attempt (ADR 006: `wezterm
    /// cli` has no codec handshake, so connect success never proves the
    /// primitive; the stock server answers ident 63 with the stable
    /// [`CAS_MISSING_PDU_STDERR`] reason and zero mutation).
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            backend: Backend::Wez,
            cas_rename: false,
            probed: vec![
                "socket_classify".to_string(),
                "sentinel_handshake".to_string(),
            ],
        }
    }

    /// Strict owner-side inventory (plan §11.1): pre-flight probe, one
    /// exact-socket `list --format json` under a dmux deadline, sentinel
    /// handshake in the same response, reserved rows excluded, grouping by
    /// workspace → unique tab → pane, one-window diagnosis from distinct
    /// `window_id`s.
    fn inventory(&self, scope: &InventoryScope) -> InventoryOutcome {
        match self.scan(scope) {
            Ok(inv) => InventoryOutcome::Complete(inv),
            Err(fail) => fail.into_outcome(),
        }
    }

    fn create(&self, _: &InventoryScope, _: &CreateSpec) -> ProviderResult<NativeBinding> {
        Err(Self::mutation_unimplemented("create"))
    }

    /// Wez presentation is GUI orchestration over the bridge/`--launch-gui`
    /// path (plan §9.3, P9); the owner provider never executes it, and the
    /// route-registry domain name it needs does not exist before P5/P9.
    fn prepare_presentation(
        &self,
        _: &InventoryScope,
        _: &NativeBinding,
        _: Option<&ProviderHandle>,
    ) -> ProviderResult<PresentationTarget> {
        Err(Self::mutation_unimplemented(
            "prepare_presentation (GUI orchestration, P9)",
        ))
    }

    /// A Wez logical rename is registry-only (plan §2.5); the native CAS
    /// rename exists solely for adoption/repair and is P6 work gated on the
    /// fork capability probe (ADR 006).
    fn rename(&self, _: &InventoryScope, _: &NativeBinding, _: &str) -> ProviderResult<()> {
        Err(Self::mutation_unimplemented("rename"))
    }

    fn remove(&self, _: &InventoryScope, _: &NativeBinding) -> ProviderResult<()> {
        Err(Self::mutation_unimplemented("remove"))
    }

    fn group_list(
        &self,
        scope: &InventoryScope,
        binding: &NativeBinding,
    ) -> ProviderResult<Vec<NativeGroupRow>> {
        let expected = Self::binding_epoch(scope, binding)?;
        let inv = self.scan_complete(scope)?;
        if inv.server_epoch != Some(expected) {
            return Err(ProviderError::EpochChanged {
                expected,
                observed: inv.server_epoch,
            });
        }
        inv.rows
            .into_iter()
            .find(|r| r.native_token == binding.native_token)
            .map(|r| r.groups)
            .ok_or_else(|| ProviderError::NotFound {
                native_ref: binding.native_token.clone(),
            })
    }

    fn group_new(
        &self,
        _: &InventoryScope,
        _: &NativeBinding,
        _: &CreateSpec,
    ) -> ProviderResult<ProviderHandle> {
        Err(Self::mutation_unimplemented("group_new"))
    }

    /// Wez Group/Split activation is GUI-local correlation after import
    /// (plan §11.1), not an owner-provider mutation.
    fn group_activate(&self, _: &InventoryScope, _: &ProviderHandle) -> ProviderResult<()> {
        Err(Self::mutation_unimplemented("group_activate"))
    }

    fn group_rename(&self, _: &InventoryScope, _: &ProviderHandle, _: &str) -> ProviderResult<()> {
        Err(Self::mutation_unimplemented("group_rename"))
    }

    fn group_remove(&self, _: &InventoryScope, _: &ProviderHandle) -> ProviderResult<()> {
        Err(Self::mutation_unimplemented("group_remove"))
    }

    fn split_list(
        &self,
        scope: &InventoryScope,
        group: &ProviderHandle,
    ) -> ProviderResult<Vec<NativeSplitRow>> {
        let ProviderHandle::Wz(_) = group else {
            return Err(ProviderError::WrongInstance {
                detail: format!("not a wez tab handle: {group}"),
            });
        };
        let inv = self.scan_complete(scope)?;
        inv.rows
            .into_iter()
            .flat_map(|r| r.groups)
            .find(|g| g.handle == *group)
            .map(|g| g.splits)
            .ok_or_else(|| ProviderError::NotFound {
                native_ref: group.to_string(),
            })
    }

    fn split_new(
        &self,
        _: &InventoryScope,
        _: &ProviderHandle,
        _: &CreateSpec,
    ) -> ProviderResult<ProviderHandle> {
        Err(Self::mutation_unimplemented("split_new"))
    }

    fn split_activate(&self, _: &InventoryScope, _: &ProviderHandle) -> ProviderResult<()> {
        Err(Self::mutation_unimplemented("split_activate"))
    }

    fn split_remove(&self, _: &InventoryScope, _: &ProviderHandle) -> ProviderResult<()> {
        Err(Self::mutation_unimplemented("split_remove"))
    }

    /// Re-list and return the one row for `binding.native_token`. The whole
    /// scan (probe, sentinel handshake, epoch check) reruns; a changed epoch
    /// is `EpochChanged`, an absent workspace `NotFound`.
    fn inspect(
        &self,
        scope: &InventoryScope,
        binding: &NativeBinding,
    ) -> ProviderResult<NativeSpaceRow> {
        let expected = Self::binding_epoch(scope, binding)?;
        let inv = self.scan_complete(scope)?;
        if inv.server_epoch != Some(expected) {
            return Err(ProviderError::EpochChanged {
                expected,
                observed: inv.server_epoch,
            });
        }
        inv.rows
            .into_iter()
            .find(|r| r.native_token == binding.native_token)
            .ok_or_else(|| ProviderError::NotFound {
                native_ref: binding.native_token.clone(),
            })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use super::*;

    const BIN: &str = "/opt/homebrew/bin/wezterm";
    const CFG: &str = "/etc/dmux/wez.lua";
    const SOCK: &str = "/run/dmux/wez.sock";
    const EPOCH: Uuid = Uuid::from_u128(0x1234_5678_9abc_def0_1234_5678_9abc_def0);

    fn fixture(name: &str) -> String {
        let path = format!("{}/tests/fixtures/wez/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("fixture {path}: {e}"))
    }

    struct ScriptedRunner {
        probes: RefCell<VecDeque<ProbeOutcome>>,
        probe_calls: RefCell<Vec<(String, Option<u32>)>>,
        runs: RefCell<VecDeque<Result<RunOutput, RunError>>>,
        run_calls: RefCell<Vec<(WezInvocation, Duration)>>,
    }

    impl ScriptedRunner {
        fn new(probes: Vec<ProbeOutcome>, runs: Vec<Result<RunOutput, RunError>>) -> Self {
            ScriptedRunner {
                probes: RefCell::new(probes.into()),
                probe_calls: RefCell::new(Vec::new()),
                runs: RefCell::new(runs.into()),
                run_calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl WezRunner for &ScriptedRunner {
        fn probe(&self, socket_path: &str, expected_server_pid: Option<u32>) -> ProbeOutcome {
            self.probe_calls
                .borrow_mut()
                .push((socket_path.to_string(), expected_server_pid));
            self.probes
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| panic!("unscripted probe: {socket_path}"))
        }

        fn run(
            &self,
            invocation: &WezInvocation,
            deadline: Duration,
        ) -> Result<RunOutput, RunError> {
            self.run_calls
                .borrow_mut()
                .push((invocation.clone(), deadline));
            self.runs
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| panic!("unscripted run: {:?}", invocation.argv))
        }
    }

    fn ok(stdout: &str) -> Result<RunOutput, RunError> {
        Ok(RunOutput {
            status: 0,
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        })
    }

    fn provider(runner: &ScriptedRunner) -> WezProvider<&ScriptedRunner> {
        WezProvider::with_runner(BIN, CFG, runner)
    }

    fn scope(expected: Option<ServerEpoch>) -> InventoryScope {
        InventoryScope {
            backend: Backend::Wez,
            endpoint: SOCK.into(),
            expected_epoch: expected,
        }
    }

    fn complete(runner: &ScriptedRunner, expected: Option<ServerEpoch>) -> NativeInventory {
        match provider(runner).inventory(&scope(expected)) {
            InventoryOutcome::Complete(inv) => inv,
            other => panic!("expected complete inventory, got {other:?}"),
        }
    }

    // -- invocation template ------------------------------------------------

    #[test]
    fn list_invocation_argv_and_env_are_exact() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&fixture("list_sentinel_only.json"))],
        );
        let _ = complete(&runner, None);
        assert_eq!(
            runner.probe_calls.borrow().as_slice(),
            &[(SOCK.to_string(), None)]
        );
        let calls = runner.run_calls.borrow();
        let (invocation, deadline) = &calls[0];
        assert_eq!(
            invocation,
            &WezInvocation {
                argv: vec![
                    BIN.into(),
                    "--config-file".into(),
                    CFG.into(),
                    "cli".into(),
                    "--no-auto-start".into(),
                    "list".into(),
                    "--format".into(),
                    "json".into(),
                ],
                env_set: vec![(SOCKET_ENV.into(), SOCK.into())],
                env_remove: vec!["WEZTERM_PANE".into(), "TMUX".into(), "TMUX_PANE".into()],
            }
        );
        assert_eq!(*deadline, DEFAULT_DEADLINE);
    }

    #[test]
    fn identity_expectation_reaches_the_probe() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::WrongPeer {
                detail: "peer 999 != 42".into(),
            }],
            vec![],
        );
        let p = provider(&runner).with_identity(IdentityExpectation {
            server_pid: Some(42),
            start_token: Some("token".into()),
        });
        match p.inventory(&scope(None)) {
            InventoryOutcome::Malformed { detail } => {
                assert!(detail.contains("wrong_backend_instance"), "{detail}");
            }
            other => panic!("wrong peer must be typed, got {other:?}"),
        }
        assert_eq!(
            runner.probe_calls.borrow().as_slice(),
            &[(SOCK.to_string(), Some(42))]
        );
        assert!(
            runner.run_calls.borrow().is_empty(),
            "no CLI after failed probe"
        );
    }

    #[test]
    fn empty_endpoint_is_a_typed_programming_error() {
        let runner = ScriptedRunner::new(vec![], vec![]);
        let mut s = scope(None);
        s.endpoint = String::new();
        match provider(&runner).inventory(&s) {
            InventoryOutcome::Malformed { detail } => {
                assert!(detail.contains("empty WEZTERM_UNIX_SOCKET"), "{detail}");
            }
            other => panic!("empty endpoint must be malformed, got {other:?}"),
        }
        assert!(runner.probe_calls.borrow().is_empty());
        assert!(runner.run_calls.borrow().is_empty());
    }

    #[test]
    fn wrong_backend_scope_is_malformed() {
        let runner = ScriptedRunner::new(vec![], vec![]);
        let mut s = scope(None);
        s.backend = Backend::Tmux;
        match provider(&runner).inventory(&s) {
            InventoryOutcome::Malformed { detail } => {
                assert!(detail.contains("tmux scope"), "{detail}");
            }
            other => panic!("wrong backend must be malformed, got {other:?}"),
        }
    }

    // -- parsing and grouping ----------------------------------------------

    #[test]
    fn two_workspace_fixture_groups_and_extracts_epoch() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&fixture("list_two_workspaces.json"))],
        );
        let inv = complete(&runner, Some(ServerEpoch(EPOCH)));
        assert_eq!(inv.server_epoch, Some(ServerEpoch(EPOCH)));
        assert_eq!(inv.rows.len(), 2, "sentinel excluded from user rows");

        let alpha = &inv.rows[0];
        assert_eq!(alpha.native_token, "alpha");
        assert_eq!(alpha.native_name, "alpha");
        assert!(!alpha.multi_window);
        assert_eq!(alpha.groups.len(), 2, "unique tab_id count");
        let tab10 = &alpha.groups[0];
        assert_eq!(tab10.handle, ProviderHandle::Wz(10));
        assert_eq!(tab10.title.as_deref(), Some("editor"));
        assert_eq!(
            tab10
                .splits
                .iter()
                .map(|s| s.handle.clone())
                .collect::<Vec<_>>(),
            vec![ProviderHandle::Wz(100), ProviderHandle::Wz(101)]
        );
        assert_eq!(tab10.splits[0].title.as_deref(), Some("nvim"));
        assert_eq!(tab10.splits[0].cwd.as_deref(), Some("/Users/fredrir/code"));
        assert_eq!(
            tab10.splits[1].cwd.as_deref(),
            Some("/tmp/with space"),
            "percent-decoded cwd"
        );
        let tab11 = &alpha.groups[1];
        assert_eq!(tab11.handle, ProviderHandle::Wz(11));
        assert_eq!(tab11.title, None, "empty tab_title is None");
        assert_eq!(tab11.splits.len(), 1);
        assert_eq!(tab11.splits[0].title, None);
        assert_eq!(tab11.splits[0].cwd, None, "empty cwd is None");

        // A managed Space key (reserved prefix, NOT the sentinel) stays a
        // user row with the full opaque key as token and name.
        let beta = &inv.rows[1];
        assert!(beta.native_token.starts_with("dmux:"));
        assert!(!beta.native_token.starts_with(WEZ_SENTINEL_PREFIX));
        assert_eq!(beta.native_name, beta.native_token);
        assert_eq!(beta.groups.len(), 1);
        assert_eq!(
            beta.groups[0].splits[0].cwd.as_deref(),
            Some("/srv/data"),
            "file://host/path keeps the path"
        );
    }

    #[test]
    fn multi_window_workspace_is_flagged_not_recounted() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&fixture("list_multi_window.json"))],
        );
        let inv = complete(&runner, Some(ServerEpoch(EPOCH)));
        assert_eq!(inv.rows.len(), 1);
        let row = &inv.rows[0];
        assert_eq!(row.native_token, "mw");
        assert!(row.multi_window, "two distinct window_ids (plan §2.3)");
        // Group count comes from unique tab_id, never window_id (§11.1).
        assert_eq!(row.groups.len(), 2);
    }

    #[test]
    fn sentinel_only_server_is_complete_and_empty() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&fixture("list_sentinel_only.json"))],
        );
        let inv = complete(&runner, Some(ServerEpoch(EPOCH)));
        assert_eq!(inv.server_epoch, Some(ServerEpoch(EPOCH)));
        assert!(inv.rows.is_empty(), "zero user rows is a determinate scan");
    }

    #[test]
    fn missing_sentinel_is_malformed() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&fixture("list_no_sentinel.json"))],
        );
        match provider(&runner).inventory(&scope(None)) {
            InventoryOutcome::Malformed { detail } => {
                assert!(detail.contains("sentinel missing"), "{detail}");
            }
            other => panic!("missing sentinel must discard rows, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_sentinel_is_malformed() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&fixture("list_duplicate_sentinel.json"))],
        );
        match provider(&runner).inventory(&scope(None)) {
            InventoryOutcome::Malformed { detail } => {
                assert!(detail.contains("sentinel duplicate"), "{detail}");
            }
            other => panic!("duplicate sentinel must discard rows, got {other:?}"),
        }
    }

    #[test]
    fn expected_epoch_mismatch_is_backend_epoch_changed() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&fixture("list_two_workspaces.json"))],
        );
        let other = ServerEpoch(Uuid::from_u128(0xdead_beef));
        match provider(&runner).inventory(&scope(Some(other))) {
            InventoryOutcome::Malformed { detail } => {
                assert!(detail.starts_with("backend_epoch_changed"), "{detail}");
                assert!(detail.contains(&EPOCH.to_string()), "{detail}");
            }
            other => panic!("epoch mismatch must discard rows, got {other:?}"),
        }
    }

    #[test]
    fn malformed_json_is_malformed() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&fixture("list_malformed.json"))],
        );
        match provider(&runner).inventory(&scope(None)) {
            InventoryOutcome::Malformed { detail } => {
                assert!(detail.contains("unparseable wezterm list JSON"), "{detail}");
            }
            other => panic!("non-JSON stdout must be malformed, got {other:?}"),
        }
    }

    #[test]
    fn unexpected_schema_is_malformed() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&fixture("list_bad_schema.json"))],
        );
        match provider(&runner).inventory(&scope(None)) {
            InventoryOutcome::Malformed { detail } => {
                assert!(detail.contains("unparseable wezterm list JSON"), "{detail}");
            }
            other => panic!("schema drift must be malformed, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_pane_id_is_malformed() {
        let sentinel = format!(
            r#"[{{"window_id":0,"tab_id":0,"pane_id":0,"workspace":"dmux:system:{EPOCH}"}},
                {{"window_id":1,"tab_id":1,"pane_id":7,"workspace":"a"}},
                {{"window_id":1,"tab_id":2,"pane_id":7,"workspace":"a"}}]"#
        );
        let runner = ScriptedRunner::new(vec![ProbeOutcome::Connectable], vec![ok(&sentinel)]);
        match provider(&runner).inventory(&scope(None)) {
            InventoryOutcome::Malformed { detail } => {
                assert!(detail.contains("duplicate pane_id"), "{detail}");
            }
            other => panic!("duplicate pane must be malformed, got {other:?}"),
        }
    }

    #[test]
    fn unparseable_sentinel_epoch_is_malformed() {
        let text =
            r#"[{"window_id":0,"tab_id":0,"pane_id":0,"workspace":"dmux:system:not-a-uuid"}]"#;
        let runner = ScriptedRunner::new(vec![ProbeOutcome::Connectable], vec![ok(text)]);
        match provider(&runner).inventory(&scope(None)) {
            InventoryOutcome::Malformed { detail } => {
                assert!(detail.contains("unparseable sentinel epoch"), "{detail}");
            }
            other => panic!("bad epoch must be malformed, got {other:?}"),
        }
    }

    // -- probe / run classification ----------------------------------------

    #[test]
    fn probe_outcomes_classify_typed_and_skip_the_cli() {
        let cases: Vec<(ProbeOutcome, fn(&InventoryOutcome) -> bool)> = vec![
            (
                ProbeOutcome::Absent {
                    detail: "ENOENT".into(),
                },
                |o| matches!(o, InventoryOutcome::ServerStopped { .. }),
            ),
            (
                ProbeOutcome::Refused {
                    detail: "ECONNREFUSED".into(),
                },
                |o| matches!(o, InventoryOutcome::ServerStopped { .. }),
            ),
            (
                ProbeOutcome::NotSocket {
                    detail: "regular file".into(),
                },
                |o| matches!(o, InventoryOutcome::Malformed { .. }),
            ),
            (
                ProbeOutcome::Denied {
                    detail: "EACCES".into(),
                },
                |o| matches!(o, InventoryOutcome::PermissionFailure { .. }),
            ),
            (
                ProbeOutcome::WrongPeer {
                    detail: "peer mismatch".into(),
                },
                |o| {
                    matches!(o, InventoryOutcome::Malformed { detail }
                        if detail.contains("wrong_backend_instance"))
                },
            ),
            (
                ProbeOutcome::Failed {
                    detail: "EINTR".into(),
                },
                |o| matches!(o, InventoryOutcome::Unreachable { .. }),
            ),
        ];
        for (probe, check) in cases {
            let runner = ScriptedRunner::new(vec![probe.clone()], vec![]);
            let outcome = provider(&runner).inventory(&scope(None));
            assert!(check(&outcome), "{probe:?} classified as {outcome:?}");
            assert!(
                runner.run_calls.borrow().is_empty(),
                "no CLI child may be spawned after a failed probe ({probe:?})"
            );
        }
    }

    #[test]
    fn run_errors_classify_typed() {
        for (err, check) in [
            (
                RunError::Timeout {
                    detail: "deadline".into(),
                },
                (|o| matches!(o, InventoryOutcome::Timeout { .. }))
                    as fn(&InventoryOutcome) -> bool,
            ),
            (
                RunError::MissingBinary {
                    detail: "ENOENT".into(),
                },
                |o| matches!(o, InventoryOutcome::CommandMissing { .. }),
            ),
            (
                RunError::Io {
                    detail: "broken pipe".into(),
                },
                |o| matches!(o, InventoryOutcome::Malformed { .. }),
            ),
        ] {
            let runner =
                ScriptedRunner::new(vec![ProbeOutcome::Connectable], vec![Err(err.clone())]);
            let outcome = provider(&runner).inventory(&scope(None));
            assert!(check(&outcome), "{err:?} classified as {outcome:?}");
        }
    }

    #[test]
    fn nonzero_exit_is_malformed_with_stderr_as_diagnostics_only() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![Ok(RunOutput {
                status: 1,
                stdout: Vec::new(),
                stderr: b"Corrupt Response: decode_raw_async".to_vec(),
            })],
        );
        match provider(&runner).inventory(&scope(None)) {
            InventoryOutcome::Malformed { detail } => {
                assert!(detail.contains("exited 1"), "{detail}");
                assert!(
                    detail.contains("Corrupt Response"),
                    "diagnostics kept: {detail}"
                );
            }
            other => panic!("CLI failure after connect-OK is malformed, got {other:?}"),
        }
    }

    // -- read verbs ---------------------------------------------------------

    fn binding(token: &str, epoch: Uuid) -> NativeBinding {
        NativeBinding {
            native_token: token.into(),
            server_epoch: ServerEpoch(epoch),
            root_group: ProviderHandle::Wz(10),
            root_split: ProviderHandle::Wz(100),
        }
    }

    #[test]
    fn inspect_returns_the_one_row() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&fixture("list_two_workspaces.json"))],
        );
        let row = provider(&runner)
            .inspect(&scope(Some(ServerEpoch(EPOCH))), &binding("alpha", EPOCH))
            .expect("inspect");
        assert_eq!(row.native_token, "alpha");
        assert_eq!(row.groups.len(), 2);
    }

    #[test]
    fn inspect_absent_token_is_not_found() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&fixture("list_two_workspaces.json"))],
        );
        match provider(&runner).inspect(&scope(None), &binding("missing", EPOCH)) {
            Err(ProviderError::NotFound { native_ref }) => assert_eq!(native_ref, "missing"),
            other => panic!("absent binding must be NotFound, got {other:?}"),
        }
    }

    #[test]
    fn inspect_epoch_mismatch_is_epoch_changed() {
        // Stale binding vs caller scope: rejected before any child spawns.
        let runner = ScriptedRunner::new(vec![], vec![]);
        let stale = binding("alpha", Uuid::from_u128(0xdead_beef));
        match provider(&runner).inspect(&scope(Some(ServerEpoch(EPOCH))), &stale) {
            Err(ProviderError::EpochChanged { expected, observed }) => {
                assert_eq!(expected, ServerEpoch(EPOCH));
                assert_eq!(observed, Some(stale.server_epoch));
            }
            other => panic!("stale binding must be EpochChanged, got {other:?}"),
        }
        assert!(runner.run_calls.borrow().is_empty());

        // Binding epoch vs the epoch the live sentinel proves.
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&fixture("list_two_workspaces.json"))],
        );
        let stale = binding("alpha", Uuid::from_u128(0xdead_beef));
        match provider(&runner).inspect(&scope(None), &stale) {
            Err(ProviderError::EpochChanged { expected, observed }) => {
                assert_eq!(expected, stale.server_epoch);
                assert_eq!(observed, Some(ServerEpoch(EPOCH)));
            }
            other => panic!("live epoch mismatch must be EpochChanged, got {other:?}"),
        }
    }

    #[test]
    fn group_and_split_lists_read_from_the_verified_scan() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable, ProbeOutcome::Connectable],
            vec![
                ok(&fixture("list_two_workspaces.json")),
                ok(&fixture("list_two_workspaces.json")),
            ],
        );
        let p = provider(&runner);
        let groups = p
            .group_list(&scope(Some(ServerEpoch(EPOCH))), &binding("alpha", EPOCH))
            .expect("group_list");
        assert_eq!(groups.len(), 2);
        let splits = p
            .split_list(&scope(Some(ServerEpoch(EPOCH))), &ProviderHandle::Wz(10))
            .expect("split_list");
        assert_eq!(splits.len(), 2);
        match p.split_list(&scope(None), &ProviderHandle::Tx(10)) {
            Err(ProviderError::WrongInstance { .. }) => {}
            other => panic!("tmux handle must be WrongInstance, got {other:?}"),
        }
    }

    // -- mutation boundary --------------------------------------------------

    #[test]
    fn mutation_verbs_fail_typed_until_p6() {
        let runner = ScriptedRunner::new(vec![], vec![]);
        let p = provider(&runner);
        let s = scope(Some(ServerEpoch(EPOCH)));
        let b = binding("alpha", EPOCH);
        let spec = CreateSpec {
            native_token: "dmux:h:s".into(),
            cwd: None,
            bootstrap_argv: vec!["/bin/true".into()],
        };
        let handle = ProviderHandle::Wz(10);
        let results: Vec<Result<(), ProviderError>> = vec![
            p.create(&s, &spec).map(|_| ()),
            p.prepare_presentation(&s, &b, None).map(|_| ()),
            p.rename(&s, &b, "x"),
            p.remove(&s, &b),
            p.group_new(&s, &b, &spec).map(|_| ()),
            p.group_activate(&s, &handle),
            p.group_rename(&s, &handle, "x"),
            p.group_remove(&s, &handle),
            p.split_new(&s, &handle, &spec).map(|_| ()),
            p.split_activate(&s, &handle),
            p.split_remove(&s, &handle),
        ];
        for r in results {
            match r {
                Err(ProviderError::NativeFailure { detail }) => {
                    assert!(detail.contains("land in P6"), "{detail}");
                }
                other => panic!("mutation must fail typed, got {other:?}"),
            }
        }
        assert!(runner.run_calls.borrow().is_empty(), "no child spawned");
    }

    // -- argv builders ------------------------------------------------------

    fn cli_prefix() -> Vec<String> {
        vec![
            BIN.into(),
            "--config-file".into(),
            CFG.into(),
            "cli".into(),
            "--no-auto-start".into(),
        ]
    }

    #[test]
    fn builders_emit_the_frozen_argv() {
        let boot = vec![
            "/usr/local/bin/dmux".to_string(),
            "_bootstrap".into(),
            "uid-1".into(),
        ];

        let inv = spawn_workspace_invocation(BIN, CFG, SOCK, "dmux:h:s", Some("/work"), &boot)
            .expect("spawn workspace");
        let mut want = cli_prefix();
        want.extend(
            [
                "spawn",
                "--new-window",
                "--workspace",
                "dmux:h:s",
                "--cwd",
                "/work",
                "--",
                "/usr/local/bin/dmux",
                "_bootstrap",
                "uid-1",
            ]
            .map(String::from),
        );
        assert_eq!(inv.argv, want);
        assert_eq!(
            inv.env_set,
            vec![(SOCKET_ENV.to_string(), SOCK.to_string())]
        );
        assert_eq!(inv.env_remove, vec!["WEZTERM_PANE", "TMUX", "TMUX_PANE"]);

        let inv = spawn_group_invocation(BIN, CFG, SOCK, 4, None, &boot).expect("spawn group");
        let mut want = cli_prefix();
        want.extend(
            [
                "spawn",
                "--window-id",
                "4",
                "--",
                "/usr/local/bin/dmux",
                "_bootstrap",
                "uid-1",
            ]
            .map(String::from),
        );
        assert_eq!(inv.argv, want);

        let inv = split_pane_invocation(BIN, CFG, SOCK, 7, Some("/work"), &boot).expect("split");
        let mut want = cli_prefix();
        want.extend(
            [
                "split-pane",
                "--pane-id",
                "7",
                "--cwd",
                "/work",
                "--",
                "/usr/local/bin/dmux",
                "_bootstrap",
                "uid-1",
            ]
            .map(String::from),
        );
        assert_eq!(inv.argv, want);

        let inv = set_tab_title_invocation(BIN, CFG, SOCK, 12, "editor").expect("set title");
        let mut want = cli_prefix();
        want.extend(["set-tab-title", "--tab-id", "12", "editor"].map(String::from));
        assert_eq!(inv.argv, want);

        let inv = kill_pane_invocation(BIN, CFG, SOCK, 9).expect("kill pane");
        let mut want = cli_prefix();
        want.extend(["kill-pane", "--pane-id", "9"].map(String::from));
        assert_eq!(inv.argv, want);
    }

    #[test]
    fn builders_reject_empty_socket_and_empty_bootstrap() {
        let boot = vec!["/bin/true".to_string()];
        let err = cli_invocation(BIN, CFG, "", &["list"]).unwrap_err();
        assert!(err.contains("empty WEZTERM_UNIX_SOCKET"), "{err}");
        let err = spawn_workspace_invocation(BIN, CFG, "", "k", None, &boot).unwrap_err();
        assert!(err.contains("empty WEZTERM_UNIX_SOCKET"), "{err}");
        let err = spawn_workspace_invocation(BIN, CFG, SOCK, "k", None, &[]).unwrap_err();
        assert!(err.contains("bootstrap helper argv"), "{err}");
        let err = spawn_group_invocation(BIN, CFG, SOCK, 1, None, &[]).unwrap_err();
        assert!(err.contains("bootstrap helper argv"), "{err}");
        let err = split_pane_invocation(BIN, CFG, SOCK, 1, None, &[]).unwrap_err();
        assert!(err.contains("bootstrap helper argv"), "{err}");
        let err = spawn_workspace_invocation(BIN, CFG, SOCK, "", None, &boot).unwrap_err();
        assert!(err.contains("non-empty opaque key"), "{err}");
    }

    // -- capabilities and CAS probe seam -------------------------------------

    #[test]
    fn capabilities_report_read_side_probes_and_no_cas() {
        let runner = ScriptedRunner::new(vec![], vec![]);
        let caps = provider(&runner).capabilities();
        assert_eq!(caps.backend, Backend::Wez);
        assert!(!caps.cas_rename, "cas_rename needs the P6 positive probe");
        assert_eq!(
            caps.probed,
            vec![
                "socket_classify".to_string(),
                "sentinel_handshake".to_string()
            ]
        );
    }

    #[test]
    fn cas_probe_classifier_matches_adr_006() {
        assert_eq!(classify_cas_probe(true, ""), CasProbe::Capable);
        assert_eq!(
            classify_cas_probe(false, "Error: invalid PDU Invalid { ident: 63 }"),
            CasProbe::Missing
        );
        match classify_cas_probe(false, "failed to connect") {
            CasProbe::Indeterminate(detail) => assert_eq!(detail, "failed to connect"),
            other => panic!("unknown stderr must stay indeterminate, got {other:?}"),
        }
    }

    // -- cwd URI parsing ----------------------------------------------------

    #[test]
    fn cwd_uri_parsing() {
        assert_eq!(parse_cwd_uri("file:///Users/fredrir/"), "/Users/fredrir/");
        assert_eq!(parse_cwd_uri("file://otherhost/srv/data"), "/srv/data");
        assert_eq!(parse_cwd_uri("file:///tmp/with%20space"), "/tmp/with space");
        assert_eq!(parse_cwd_uri("not-a-uri"), "not-a-uri", "raw kept");
        assert_eq!(parse_cwd_uri("file://nohost"), "file://nohost", "raw kept");
        assert_eq!(
            parse_cwd_uri("file:///bad%zzescape"),
            "file:///bad%zzescape",
            "invalid escape keeps raw"
        );
    }
}
