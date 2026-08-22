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
//!   cli --no-auto-start [--prefer-mux] <subcmd> [--format json]
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
//! Every native verb — the child reads `split_list`, `normalize_plan` and
//! `sole_window_id` included — first takes the published epoch the caller's
//! managed scope carries (`required_action_epoch`, or `binding_epoch` for a
//! binding-bearing verb, which additionally holds the binding to that pin —
//! never the other way round) and pins every scan to it. An unmanaged scope
//! (`InventoryScope::unmanaged_endpoint`) is refused typed before the
//! endpoint is probed: nothing on it is verifiable, so no native ID is
//! addressable and no mutation may run (ADR 012 WS-A.6, review finding #6).
//! Only `inventory` — the discovery read — scans an unmanaged endpoint.
//!
//! Mutations (P6, plan §11.1/§14): create/group_new/split_new/group_rename/
//! remove/group_remove/split_remove are implemented here over the frozen
//! P3b argv builders. The provider layer is pure native semantics — exact
//! locators in, typed results out; the root's fenced journals/leases live
//! ABOVE this module. Every mutation brackets the native call with complete
//! sentinel-verified scans: epoch verified before and after (a flip is
//! [`ProviderError::EpochChanged`]), the one-window invariant (plan §2.3)
//! checked before and after (violations are [`ProviderError::MultiWindow`];
//! whole-Space `remove` is the sanctioned exception), and postconditions
//! re-listed — never inferred from exit codes. `create` performs a keyed
//! lookup first and NEVER spawns when the workspace key already exists
//! (acknowledgement-loss replay protection, plan §10.2): the existing state
//! comes back inside a typed [`ProviderError::PostconditionFailed`].
//!
//! Normalization (P8a, plan §10.3): `normalize_plan` computes the
//! deterministic multi-window merge plan from one verified same-epoch scan
//! — target = the LOWEST native window id of the workspace, moves = every
//! pane of every other window in ascending (window_id, pane_id) order; a
//! sole-window workspace plans zero moves. `normalize_apply` re-scans
//! pinned to the plan's epoch, re-derives the plan and requires EQUALITY
//! with the confirmed one (any drift refuses `normalize_drift:` with zero
//! mutation), executes each move through the exact `move-pane-to-new-tab
//! --pane-id P --window-id <target>` argv, and proves single-window
//! convergence with the bounded ADR 005 re-list pattern
//! (`normalize_unconverged:` at the bound). Panes are moved, never killed,
//! and panes outside the plan are never touched.
//!
//! The legacy trait methods `prepare_presentation`/`group_activate`/
//! `split_activate` remain GUI-orchestration boundaries. P9 owner-side keys
//! use the separate exact APIs (`activate_group_exact`,
//! `select_split_direction`, `resize_split_exact`,
//! `toggle_split_zoom_exact`): all require a caller-pinned epoch, bracket
//! the action with sentinel scans, and add `--prefer-mux` so an incidental
//! GUI endpoint is never selected. `rename` remains registry-only (plan
//! §2.5; native reassignment is the CAS adoption verb).
//!
//! CAS adoption verb (ADR 006, fork codec 46): [`WezProvider::cas_rename_workspace`]
//! invokes `cli rename-workspace --window-id N --if-workspace OLD
//! [--if-sole-window] NEW` through the configured [`WezProvider::with_cas_binary`]
//! fork CLI and classifies the frozen stderr shapes into
//! [`CasRenameOutcome`]. [`WezProvider::probe_cas_rename`] is the amended
//! positive capability probe: a CAS against the known-nonexistent window id
//! `u64::MAX` where `no_such_window` proves the verb exists (zero mutation
//! possible) and the stable invalid-PDU reason proves it does not.
//!
//! Specialist-owned (plan §19, W2); the trait and result types in
//! `backend/mod.rs` are the frozen root-owned contract.

use std::io::Read;
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::backend::{
    Capabilities, CreateSpec, GroupActivationResult, InventoryOutcome, InventoryScope,
    NativeBinding, NativeGroupRow, NativeInventory, NativeSpaceRow, NativeSplitRow, NormalizeMove,
    NormalizePlan, PresentationTarget, Provider, ProviderError, ProviderResult, SplitDirection,
    SplitDirectionResult, SplitResizeResult, SplitSpec, SplitZoomResult,
};
use crate::model::{Backend, ProviderHandle, ServerEpoch, WEZ_SENTINEL_PREFIX};

/// Environment variables scrubbed from every child (ADR 001 template). A
/// provider running inside a WezTerm pane or a tmux client must never let
/// the ambient mux leak into endpoint selection.
pub const SCRUBBED_ENV: [&str; 3] = ["WEZTERM_PANE", "TMUX", "TMUX_PANE"];

/// The sole endpoint identity selector. `--prefer-mux` selects connection
/// mode for P9 actions but never supplies or substitutes this exact socket.
/// Must always be set non-empty.
pub const SOCKET_ENV: &str = "WEZTERM_UNIX_SOCKET";

/// Default per-child deadline. ADR 001: a silent socket hangs the stock CLI
/// for >12s with no built-in timeout; dmux kills the child at the deadline.
const DEFAULT_DEADLINE: Duration = Duration::from_secs(10);

/// ADR 005 removal bound: max re-list/kill rounds before returning a typed
/// partial (`PostconditionFailed` with the surviving pane ids). Reused as
/// the §10.3 normalize-apply re-list/move bound (`normalize_unconverged:`).
pub const REMOVE_MAX_ROUNDS: usize = 5;

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

/// P9 owner-control invocation. `--prefer-mux` is mandatory in addition to
/// `--no-auto-start`: the exact socket is still supplied through
/// `WEZTERM_UNIX_SOCKET`, and the CLI must not prefer an incidental GUI
/// endpoint while applying owner-side pane layout operations.
pub fn mux_cli_invocation(
    wezterm_bin: &str,
    config_file: &str,
    socket: &str,
    cli_args: &[&str],
) -> Result<WezInvocation, String> {
    let mut invocation = cli_invocation(wezterm_bin, config_file, socket, cli_args)?;
    // Prefix is: BIN --config-file CFG cli --no-auto-start <subcommand>.
    // `--prefer-mux` is a `cli` option and therefore precedes the subcommand.
    invocation.argv.insert(5, "--prefer-mux".to_string());
    Ok(invocation)
}

fn wez_direction(direction: SplitDirection) -> &'static str {
    match direction {
        SplitDirection::Left => "Left",
        SplitDirection::Right => "Right",
        SplitDirection::Up => "Up",
        SplitDirection::Down => "Down",
    }
}

pub fn activate_tab_invocation(
    wezterm_bin: &str,
    config_file: &str,
    socket: &str,
    tab_id: u64,
) -> Result<WezInvocation, String> {
    mux_cli_invocation(
        wezterm_bin,
        config_file,
        socket,
        &["activate-tab", "--tab-id", &tab_id.to_string()],
    )
}

pub fn activate_pane_invocation(
    wezterm_bin: &str,
    config_file: &str,
    socket: &str,
    pane_id: u64,
) -> Result<WezInvocation, String> {
    mux_cli_invocation(
        wezterm_bin,
        config_file,
        socket,
        &["activate-pane", "--pane-id", &pane_id.to_string()],
    )
}

pub fn get_pane_direction_invocation(
    wezterm_bin: &str,
    config_file: &str,
    socket: &str,
    pane_id: u64,
    direction: SplitDirection,
) -> Result<WezInvocation, String> {
    mux_cli_invocation(
        wezterm_bin,
        config_file,
        socket,
        &[
            "get-pane-direction",
            "--pane-id",
            &pane_id.to_string(),
            wez_direction(direction),
        ],
    )
}

pub fn adjust_pane_size_invocation(
    wezterm_bin: &str,
    config_file: &str,
    socket: &str,
    pane_id: u64,
    direction: SplitDirection,
    amount: u16,
) -> Result<WezInvocation, String> {
    mux_cli_invocation(
        wezterm_bin,
        config_file,
        socket,
        &[
            "adjust-pane-size",
            "--pane-id",
            &pane_id.to_string(),
            "--amount",
            &amount.to_string(),
            wez_direction(direction),
        ],
    )
}

pub fn toggle_zoom_pane_invocation(
    wezterm_bin: &str,
    config_file: &str,
    socket: &str,
    pane_id: u64,
) -> Result<WezInvocation, String> {
    mux_cli_invocation(
        wezterm_bin,
        config_file,
        socket,
        &["zoom-pane", "--pane-id", &pane_id.to_string(), "--toggle"],
    )
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
/// The placement flag is always explicit so the argv is deterministic.
pub fn split_pane_invocation(
    wezterm_bin: &str,
    config_file: &str,
    socket: &str,
    pane_id: u64,
    direction: SplitDirection,
    percent: Option<u8>,
    cwd: Option<&str>,
    bootstrap_argv: &[String],
) -> Result<WezInvocation, String> {
    require_bootstrap("split-pane", bootstrap_argv)?;
    let mut args: Vec<String> = vec!["split-pane".into(), "--pane-id".into(), pane_id.to_string()];
    args.push(
        match direction {
            SplitDirection::Left => "--left",
            SplitDirection::Right => "--right",
            SplitDirection::Up => "--top",
            SplitDirection::Down => "--bottom",
        }
        .into(),
    );
    if let Some(percent) = percent {
        args.push("--percent".into());
        args.push(percent.to_string());
    }
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

/// Normalization move (plan §10.3): `move-pane-to-new-tab --pane-id <P>
/// --window-id <target>`. Moves one exact pane into a NEW tab of the
/// target window; the source window dies natively once its last pane has
/// moved out. Stock CLI verb (present in the pinned fork build too);
/// `normalize_apply` drives the bounded re-list convergence over it.
pub fn move_pane_invocation(
    wezterm_bin: &str,
    config_file: &str,
    socket: &str,
    pane_id: u64,
    window_id: u64,
) -> Result<WezInvocation, String> {
    cli_invocation(
        wezterm_bin,
        config_file,
        socket,
        &[
            "move-pane-to-new-tab",
            "--pane-id",
            &pane_id.to_string(),
            "--window-id",
            &window_id.to_string(),
        ],
    )
}

/// CAS adoption verb (ADR 006, fork codec 46): `cli --no-auto-start
/// rename-workspace --window-id N --if-workspace OLD [--if-sole-window] NEW`.
/// Only meaningful through the fork `wezterm` CLI; the stock CLI rejects the
/// `--window-id/--if-workspace` arguments at argv parse.
pub fn cas_rename_invocation(
    wezterm_bin: &str,
    config_file: &str,
    socket: &str,
    window_id: u64,
    expected_workspace: &str,
    expect_sole_window: bool,
    new_workspace: &str,
) -> Result<WezInvocation, String> {
    if expected_workspace.is_empty() || new_workspace.is_empty() {
        return Err("cas rename requires non-empty expected and new workspace keys".into());
    }
    let window = window_id.to_string();
    let mut args: Vec<&str> = vec![
        "rename-workspace",
        "--window-id",
        &window,
        "--if-workspace",
        expected_workspace,
    ];
    if expect_sole_window {
        args.push("--if-sole-window");
    }
    args.push(new_workspace);
    cli_invocation(wezterm_bin, config_file, socket, &args)
}

// ---------------------------------------------------------------------------
// CAS classification (ADR 006, live-verified stderr shapes)
// ---------------------------------------------------------------------------

/// Stable marker preceding every typed fork-CAS failure token on stderr
/// (live-verified against the pinned fork build, e.g.
/// `ERROR wezterm > rename-workspace-if failed: workspace_mismatch
/// window_id=1 actual="new"; terminating`).
pub const CAS_FAILED_MARKER: &str = "rename-workspace-if failed: ";

/// Typed outcome of one fork CAS rename (mirrors the wire enum, ADR 006).
/// Every non-`Renamed` variant is a server-guaranteed zero-mutation result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CasRenameOutcome {
    Renamed,
    /// The window's workspace was not `expected_workspace`; `actual` is the
    /// server-reported current key.
    WorkspaceMismatch {
        actual: String,
    },
    NoSuchWindow,
    /// `--if-sole-window` was requested and other windows share the
    /// workspace.
    NotSoleWindow,
}

/// Classification of one CAS CLI exchange. Pure so unit tests pin the frozen
/// stderr shapes byte-for-byte. This is the one sanctioned use of stderr in
/// classification (ADR 006 froze the failure prefixes and the stock-server
/// `invalid PDU Invalid {{ ident: 63 }}` reason); anything else is
/// `Unclassified` diagnostics, never a guess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CasClassification {
    Outcome(CasRenameOutcome),
    /// The stable stock-server rejection of PDU ident 63: capability
    /// missing, guaranteed zero mutation (ADR 006).
    CapabilityMissing,
    /// Not evidence in either direction (connect failures, argv-parse
    /// errors from a non-fork CLI, unparseable mismatch payloads, ...).
    Unclassified(String),
}

/// Pure classifier for one fork-CAS invocation result.
pub fn classify_cas_rename(exit_ok: bool, stderr: &str) -> CasClassification {
    if exit_ok {
        return CasClassification::Outcome(CasRenameOutcome::Renamed);
    }
    if stderr.contains(CAS_MISSING_PDU_STDERR) {
        return CasClassification::CapabilityMissing;
    }
    let Some(idx) = stderr.find(CAS_FAILED_MARKER) else {
        return CasClassification::Unclassified(stderr.trim().to_string());
    };
    let rest = &stderr[idx + CAS_FAILED_MARKER.len()..];
    if rest.starts_with("no_such_window") {
        return CasClassification::Outcome(CasRenameOutcome::NoSuchWindow);
    }
    if rest.starts_with("not_sole_window") {
        return CasClassification::Outcome(CasRenameOutcome::NotSoleWindow);
    }
    if rest.starts_with("workspace_mismatch") {
        // Frozen shape: `workspace_mismatch window_id=N actual="<key>"`.
        // A key containing a double quote would truncate here; opaque dmux
        // keys never contain one, and an unparseable payload stays
        // unclassified rather than guessed.
        if let Some(at) = rest.find("actual=\"")
            && let Some(end) = rest[at + 8..].find('"')
        {
            return CasClassification::Outcome(CasRenameOutcome::WorkspaceMismatch {
                actual: rest[at + 8..at + 8 + end].to_string(),
            });
        }
        return CasClassification::Unclassified(format!(
            "workspace_mismatch without parseable actual: {}",
            rest.trim()
        ));
    }
    CasClassification::Unclassified(rest.trim().to_string())
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

/// Exact read-only owner tree returned from one bounded, exact-socket
/// `list --format json` scan. It includes the reserved sentinel and every
/// physical pane, including an outer pane whose logical GUI marker may
/// currently describe an inner tmux Space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WezNativeTreeWitness {
    pub server_epoch: ServerEpoch,
    pub sentinel_window_id: u64,
    pub sentinel_tab_id: u64,
    pub sentinel_pane_id: u64,
    pub panes: Vec<WezNativePaneWitness>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WezNativePaneWitness {
    pub window_id: u64,
    pub tab_id: u64,
    pub pane_id: u64,
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
    /// Optional fork `wezterm` CLI used ONLY for the CAS adoption verb and
    /// its capability probe (ADR 006). The CAS PDU exists solely in the
    /// pinned fork build; reads keep using `wezterm_bin` (a codec-45 CLI
    /// lists fine against a codec-46 server). When unset, CAS calls fall
    /// back to `wezterm_bin` — against a stock CLI the argv parse fails and
    /// classifies as a typed unclassified failure, never a guess.
    cas_binary: Option<String>,
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
            cas_binary: None,
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

    /// Install the fork `wezterm` CLI path used for CAS operations (ADR
    /// 006). See the field docs on [`WezProvider`] for the fallback rule.
    pub fn with_cas_binary(mut self, cas_binary: impl Into<String>) -> Self {
        self.cas_binary = Some(cas_binary.into());
        self
    }

    fn cas_bin(&self) -> &str {
        self.cas_binary.as_deref().unwrap_or(&self.wezterm_bin)
    }

    /// Capture the exact sentinel identity and complete physical pane tree
    /// without auto-starting or mutating the mux. The same scan proves the
    /// socket/epoch; multiple sentinel panes or duplicate native IDs are
    /// terminal rather than normalized.
    pub fn native_tree_witness(
        &self,
        scope: &InventoryScope,
    ) -> ProviderResult<WezNativeTreeWitness> {
        let scan = self.scan(scope).map_err(ScanFail::into_provider_error)?;
        let [sentinel] = scan.sentinel_panes.as_slice() else {
            return Err(ProviderError::NativeFailure {
                detail: format!(
                    "expected exactly one sentinel pane for epoch {}, found {}",
                    scan.epoch.0,
                    scan.sentinel_panes.len()
                ),
            });
        };
        let mut panes: Vec<_> = scan
            .panes
            .iter()
            .chain(scan.sentinel_panes.iter())
            .map(|pane| WezNativePaneWitness {
                window_id: pane.window_id,
                tab_id: pane.tab_id,
                pane_id: pane.pane_id,
            })
            .collect();
        panes.sort();
        let original_len = panes.len();
        panes.dedup();
        if panes.len() != original_len {
            return Err(ProviderError::NativeFailure {
                detail: "wez exact native tree contains duplicate window/tab/pane tuples".into(),
            });
        }
        let mut pane_ids = panes.iter().map(|pane| pane.pane_id).collect::<Vec<_>>();
        pane_ids.sort_unstable();
        pane_ids.dedup();
        if pane_ids.len() != panes.len() {
            return Err(ProviderError::NativeFailure {
                detail: "wez exact native tree contains a duplicate pane ID".into(),
            });
        }
        Ok(WezNativeTreeWitness {
            server_epoch: scan.epoch,
            sentinel_window_id: sentinel.window_id,
            sentinel_tab_id: sentinel.tab_id,
            sentinel_pane_id: sentinel.pane_id,
            panes,
        })
    }

    /// Scope validation plus the dmux-side endpoint identity probe (ADR
    /// 001): shared by every scan and by the CAS capability probe.
    fn preflight(&self, scope: &InventoryScope) -> Result<(), ScanFail> {
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
            ProbeOutcome::Connectable => Ok(()),
            ProbeOutcome::Absent { detail } => {
                Err(ScanFail::Stopped(format!("socket absent: {detail}")))
            }
            ProbeOutcome::Refused { detail } => {
                Err(ScanFail::Stopped(format!("connection refused: {detail}")))
            }
            ProbeOutcome::NotSocket { detail } => {
                Err(ScanFail::Malformed(format!("invalid endpoint: {detail}")))
            }
            ProbeOutcome::Denied { detail } => Err(ScanFail::Permission(detail)),
            ProbeOutcome::WrongPeer { detail } => Err(ScanFail::WrongInstance(detail)),
            ProbeOutcome::Failed { detail } => Err(ScanFail::Unreachable(detail)),
        }
    }

    /// One verified scan: probe → exact-socket `list --format json` →
    /// sentinel/epoch handshake → grouped rows plus the raw user pane
    /// tuples mutations need (window ids, tab↔pane parentage).
    fn scan(&self, scope: &InventoryScope) -> Result<DetailedScan, ScanFail> {
        self.scan_with_mux_preference(scope, false)
    }

    /// P9 owner-side action scan. Every CLI call in an exact child action,
    /// including its pre/post witnesses, carries both no-auto-start and
    /// prefer-mux; ordinary inventory retains its frozen P3 argv.
    fn scan_prefer_mux(&self, scope: &InventoryScope) -> Result<DetailedScan, ScanFail> {
        self.scan_with_mux_preference(scope, true)
    }

    fn scan_with_mux_preference(
        &self,
        scope: &InventoryScope,
        prefer_mux: bool,
    ) -> Result<DetailedScan, ScanFail> {
        self.preflight(scope)?;
        let invocation = if prefer_mux {
            mux_cli_invocation(
                &self.wezterm_bin,
                &self.config_file,
                &scope.endpoint,
                &["list", "--format", "json"],
            )
        } else {
            cli_invocation(
                &self.wezterm_bin,
                &self.config_file,
                &scope.endpoint,
                &["list", "--format", "json"],
            )
        }
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
        if let Some(expected) = scope.expected_epoch()
            && expected != epoch
        {
            return Err(ScanFail::EpochChanged {
                expected,
                observed: Some(epoch),
            });
        }

        let sentinel_rows: Vec<&ListRow> = rows
            .iter()
            .filter(|row| row.workspace == sentinel)
            .collect();
        let user_rows: Vec<&ListRow> = rows
            .iter()
            .filter(|r| !r.workspace.starts_with(WEZ_SENTINEL_PREFIX))
            .collect();
        let assembled = assemble_rows(&user_rows).map_err(ScanFail::Malformed)?;
        let pane_ref = |r: &&ListRow| PaneRef {
            window_id: r.window_id,
            tab_id: r.tab_id,
            pane_id: r.pane_id,
            workspace: r.workspace.clone(),
            geometry: r.size.as_ref().and_then(|size| {
                Some(PaneGeometry {
                    cols: size.cols,
                    rows: size.rows,
                    left: r.left_col?,
                    top: r.top_row?,
                })
            }),
            is_active: r.is_active,
            is_zoomed: r.is_zoomed,
        };
        let panes = user_rows.iter().map(pane_ref).collect();
        let sentinel_panes = sentinel_rows.iter().map(pane_ref).collect();
        Ok(DetailedScan {
            epoch,
            rows: assembled,
            panes,
            sentinel_panes,
        })
    }

    /// Scan mapped for `ProviderResult` verbs, pinned to `expected`: the
    /// sentinel-proven epoch must match, else the typed
    /// [`ProviderError::EpochChanged`]. There is no unpinned form — every
    /// verb obtains its pin through [`Self::required_action_epoch`] (or
    /// [`Self::binding_epoch`]) before its first native command, so an
    /// unmanaged scope can never reach one (ADR 012 WS-A.6, review finding
    /// #6). Mutations call this twice — before (pin from scope/binding) and
    /// after (pin from the pre-scan) — so a server restart mid-mutation can
    /// never pass verification.
    fn verified_scan(
        &self,
        scope: &InventoryScope,
        expected: ServerEpoch,
    ) -> ProviderResult<DetailedScan> {
        let scan = self.scan(scope).map_err(|f| f.into_provider_error())?;
        if expected != scan.epoch {
            return Err(ProviderError::EpochChanged {
                expected,
                observed: Some(scan.epoch),
            });
        }
        Ok(scan)
    }

    /// The pin for a binding-bearing verb: the scope's published epoch,
    /// cross-checked against the binding before any command. An unpinned
    /// scope refuses `WrongInstance` like [`Self::required_action_epoch`] —
    /// the binding's own `server_epoch` is never the pin, because the
    /// caller used to synthesise that binding from the live scan and the
    /// fence then compared the server against itself (ADR 012 WS-A.8,
    /// review findings #5/#18). A binding whose epoch is not the pin is a
    /// stale ref from another incarnation: `EpochChanged`, no command.
    fn binding_epoch(
        scope: &InventoryScope,
        binding: &NativeBinding,
    ) -> ProviderResult<ServerEpoch> {
        let expected = Self::required_action_epoch(scope)?;
        if binding.server_epoch != expected {
            return Err(ProviderError::EpochChanged {
                expected,
                observed: Some(binding.server_epoch),
            });
        }
        Ok(expected)
    }

    /// GUI-orchestration boundary: presentation/activation verbs are never
    /// owner-provider operations (plan §9.3, §11.1); they land with the P9
    /// bridge. Calling one here is a typed failure, never a silent no-op.
    fn gui_orchestration(verb: &str) -> ProviderError {
        ProviderError::NativeFailure {
            detail: format!(
                "wez {verb}: GUI orchestration (plan §9.3/§11.1, P9 bridge); \
                 never an owner-provider operation"
            ),
        }
    }

    /// Execute one mutation invocation under the dmux deadline, mapping
    /// runner failures to typed provider errors. Exit status is returned for
    /// diagnostics; postconditions always come from re-list verification.
    fn run_mutation(&self, invocation: WezInvocation) -> ProviderResult<RunOutput> {
        match self.runner.run(&invocation, self.deadline) {
            Ok(out) => Ok(out),
            Err(RunError::MissingBinary { detail }) => Err(ProviderError::NativeFailure {
                detail: format!("wezterm binary missing: {detail}"),
            }),
            Err(RunError::Timeout { detail }) => Err(ProviderError::Timeout { detail }),
            Err(RunError::Io { detail }) => Err(ProviderError::NativeFailure { detail }),
        }
    }

    /// Parse the ADR 004 frozen spawn-return format (`<pane_id>\n` only).
    /// A nonzero exit is a typed native failure (nothing spawned to
    /// correlate); exit 0 with unparseable stdout means the mutation likely
    /// happened but the binding is indeterminate — `PostconditionFailed`, so
    /// the journal reconciles via keyed re-list instead of respawning.
    fn parse_spawn_pane_id(verb: &str, out: &RunOutput) -> ProviderResult<u64> {
        if !out.ok() {
            return Err(ProviderError::NativeFailure {
                detail: format!(
                    "wez {verb} exited {}: {}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            });
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let trimmed = text.trim();
        trimmed
            .parse()
            .map_err(|_| ProviderError::PostconditionFailed {
                detail: format!(
                    "wez {verb} exited 0 but returned unparseable spawn output {trimmed:?} \
                     (ADR 004 freezes `<pane_id>\\n`); native state indeterminate — \
                     reconcile via keyed re-list, never respawn"
                ),
            })
    }

    /// One-window invariant guard (plan §2.3): the workspace must span
    /// exactly one native mux window. Returns the sole window id.
    fn sole_window(scan: &DetailedScan, token: &str) -> ProviderResult<u64> {
        let windows = scan.workspace_windows(token);
        match windows.as_slice() {
            [] => Err(ProviderError::NotFound {
                native_ref: token.to_string(),
            }),
            [one] => Ok(*one),
            many => Err(ProviderError::MultiWindow {
                native_ref: token.to_string(),
                window_count: many.len() as u32,
            }),
        }
    }

    /// The fail-closed pin every native verb takes before its first
    /// command: the published epoch the caller's managed scope carries. An
    /// unmanaged scope is a typed [`ProviderError::WrongInstance`] — nothing
    /// on that endpoint is verifiable, so no native ID is addressable and no
    /// mutation may run (ADR 012 WS-A.6; plan §8.1, §11.1). A managed read
    /// without a pin refuses the same way: an unverified read of a managed
    /// server is still unverified (ADR 012 WS-A.13).
    fn required_action_epoch(scope: &InventoryScope) -> ProviderResult<ServerEpoch> {
        if scope.backend != Backend::Wez {
            return Err(ProviderError::WrongInstance {
                detail: format!("wez provider handed a {} scope", scope.backend),
            });
        }
        scope.expected_epoch().ok_or(ProviderError::WrongInstance {
            detail: "managed wez operation requires a managed scope carrying the published server epoch; \
                     nothing on an unpinned endpoint is verifiable and no native ID is addressable \
                     without a sentinel-proven incarnation"
                .into(),
        })
    }

    fn verified_action_scan(
        &self,
        scope: &InventoryScope,
        expected: ServerEpoch,
    ) -> ProviderResult<DetailedScan> {
        let scan = self
            .scan_prefer_mux(scope)
            .map_err(|fail| fail.into_provider_error())?;
        if scan.epoch != expected {
            return Err(ProviderError::EpochChanged {
                expected,
                observed: Some(scan.epoch),
            });
        }
        Ok(scan)
    }

    /// Execute one P9 owner-side primitive. The stable prefix distinguishes
    /// an absent CLI/PDU capability from an ordinary native failure while
    /// retaining [`ProviderError`] compatibility for existing callers.
    fn run_owner_action(&self, verb: &str, invocation: WezInvocation) -> ProviderResult<String> {
        let out = self.run_mutation(invocation)?;
        if !out.ok() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let lower = stderr.to_ascii_lowercase();
            let missing = lower.contains("unrecognized subcommand")
                || lower.contains("unknown command")
                || lower.contains("invalid pdu")
                || lower.contains("unexpected argument");
            let prefix = if missing {
                "wez_owner_capability_missing"
            } else {
                "wez_owner_action_failed"
            };
            return Err(ProviderError::NativeFailure {
                detail: format!("{prefix}:{verb}: exit {}: {}", out.status, stderr.trim()),
            });
        }
        String::from_utf8(out.stdout).map_err(|error| ProviderError::NativeFailure {
            detail: format!("wez_owner_action_failed:{verb}: non-utf8 stdout: {error}"),
        })
    }

    fn require_same_pane_parent(
        verb: &str,
        before: &PaneRef,
        after: &PaneRef,
    ) -> ProviderResult<()> {
        if before.workspace != after.workspace
            || before.window_id != after.window_id
            || before.tab_id != after.tab_id
        {
            return Err(ProviderError::PostconditionFailed {
                detail: format!(
                    "wez {verb}: pane {} changed parent from {:?}/window {}/tab {} to \
                     {:?}/window {}/tab {}",
                    before.pane_id,
                    before.workspace,
                    before.window_id,
                    before.tab_id,
                    after.workspace,
                    after.window_id,
                    after.tab_id
                ),
            });
        }
        Ok(())
    }

    /// Activate one exact native tab under an epoch-pinned, sentinel-verified
    /// mux socket. The owner command never creates a tab or workspace.
    pub fn activate_group_exact(
        &self,
        scope: &InventoryScope,
        group: &ProviderHandle,
    ) -> ProviderResult<GroupActivationResult> {
        let ProviderHandle::Wz(tab_id) = group else {
            return Err(ProviderError::WrongInstance {
                detail: format!("not a wez tab handle: {group}"),
            });
        };
        let expected = Self::required_action_epoch(scope)?;
        let pre = self.verified_action_scan(scope, expected)?;
        let owner =
            pre.tab_first_pane(*tab_id)
                .cloned()
                .ok_or_else(|| ProviderError::NotFound {
                    native_ref: group.to_string(),
                })?;
        Self::sole_window(&pre, &owner.workspace)?;
        let invocation = activate_tab_invocation(
            &self.wezterm_bin,
            &self.config_file,
            &scope.endpoint,
            *tab_id,
        )
        .map_err(|detail| ProviderError::NativeFailure { detail })?;
        self.run_owner_action("activate-tab", invocation)?;
        let post = self.verified_action_scan(scope, expected)?;
        Self::sole_window(&post, &owner.workspace)?;
        let after =
            post.tab_first_pane(*tab_id)
                .ok_or_else(|| ProviderError::PostconditionFailed {
                    detail: format!("wez group activation target tab {tab_id} vanished"),
                })?;
        Self::require_same_pane_parent("group activation", &owner, after)?;
        Ok(GroupActivationResult {
            server_epoch: expected,
            target: group.clone(),
        })
    }

    /// Resolve an adjacent Split from one exact pane, then activate only the
    /// returned exact pane. The pinned CLI's get-direction handler computes
    /// from the tab's active pane, so the origin is activated first; this is
    /// part of the verified sequence, not an ordinal fallback.
    pub fn select_split_direction(
        &self,
        scope: &InventoryScope,
        origin: &ProviderHandle,
        direction: SplitDirection,
    ) -> ProviderResult<SplitDirectionResult> {
        let ProviderHandle::Wz(origin_id) = origin else {
            return Err(ProviderError::WrongInstance {
                detail: format!("not a wez pane handle: {origin}"),
            });
        };
        let expected = Self::required_action_epoch(scope)?;
        let pre = self.verified_action_scan(scope, expected)?;
        let origin_pre = pre
            .pane(*origin_id)
            .cloned()
            .ok_or_else(|| ProviderError::NotFound {
                native_ref: origin.to_string(),
            })?;
        Self::sole_window(&pre, &origin_pre.workspace)?;

        let activate_origin = activate_pane_invocation(
            &self.wezterm_bin,
            &self.config_file,
            &scope.endpoint,
            *origin_id,
        )
        .map_err(|detail| ProviderError::NativeFailure { detail })?;
        self.run_owner_action("activate-pane(origin)", activate_origin)?;
        let get = get_pane_direction_invocation(
            &self.wezterm_bin,
            &self.config_file,
            &scope.endpoint,
            *origin_id,
            direction,
        )
        .map_err(|detail| ProviderError::NativeFailure { detail })?;
        let stdout = self.run_owner_action("get-pane-direction", get)?;
        let text = stdout.trim();
        let target_id = if text.is_empty() {
            None
        } else {
            Some(
                text.parse::<u64>()
                    .map_err(|_| ProviderError::PostconditionFailed {
                        detail: format!(
                            "wez get-pane-direction returned non-pane output {text:?} for pane \
                         {origin_id}"
                        ),
                    })?,
            )
        };
        if target_id == Some(*origin_id) {
            return Err(ProviderError::PostconditionFailed {
                detail: format!(
                    "wez get-pane-direction returned its exact origin pane {origin_id}"
                ),
            });
        }
        if let Some(target_id) = target_id {
            let target_pre =
                pre.pane(target_id)
                    .ok_or_else(|| ProviderError::PostconditionFailed {
                        detail: format!(
                            "wez get-pane-direction returned unlisted pane {target_id} from origin \
                         {origin_id}"
                        ),
                    })?;
            Self::require_same_pane_parent("directional selection", &origin_pre, target_pre)?;
            let activate_target = activate_pane_invocation(
                &self.wezterm_bin,
                &self.config_file,
                &scope.endpoint,
                target_id,
            )
            .map_err(|detail| ProviderError::NativeFailure { detail })?;
            self.run_owner_action("activate-pane(target)", activate_target)?;
        }

        let post = self.verified_action_scan(scope, expected)?;
        Self::sole_window(&post, &origin_pre.workspace)?;
        let origin_post =
            post.pane(*origin_id)
                .ok_or_else(|| ProviderError::PostconditionFailed {
                    detail: format!("wez directional origin pane {origin_id} vanished"),
                })?;
        Self::require_same_pane_parent("directional selection", &origin_pre, origin_post)?;
        let active_id = target_id.unwrap_or(*origin_id);
        let active = post
            .pane(active_id)
            .ok_or_else(|| ProviderError::PostconditionFailed {
                detail: format!("wez directional target pane {active_id} vanished"),
            })?;
        Self::require_same_pane_parent("directional selection", &origin_pre, active)?;
        if active.is_active != Some(true) {
            return Err(ProviderError::PostconditionFailed {
                detail: format!(
                    "wez directional target pane {active_id} did not re-list active: {:?}",
                    active.is_active
                ),
            });
        }
        Ok(SplitDirectionResult {
            server_epoch: expected,
            origin: origin.clone(),
            target: target_id.map(ProviderHandle::Wz),
        })
    }

    /// Resize an exact pane and return whether its stable list geometry
    /// changed. A boundary-constrained no-op is still a verified success.
    pub fn resize_split_exact(
        &self,
        scope: &InventoryScope,
        split: &ProviderHandle,
        direction: SplitDirection,
        amount: u16,
    ) -> ProviderResult<SplitResizeResult> {
        let ProviderHandle::Wz(pane_id) = split else {
            return Err(ProviderError::WrongInstance {
                detail: format!("not a wez pane handle: {split}"),
            });
        };
        if amount == 0 {
            return Err(ProviderError::NativeFailure {
                detail: "wez split resize amount must be greater than zero".into(),
            });
        }
        let expected = Self::required_action_epoch(scope)?;
        let pre = self.verified_action_scan(scope, expected)?;
        let before = pre
            .pane(*pane_id)
            .cloned()
            .ok_or_else(|| ProviderError::NotFound {
                native_ref: split.to_string(),
            })?;
        Self::sole_window(&pre, &before.workspace)?;
        let before_geometry =
            before
                .geometry
                .ok_or_else(|| ProviderError::PostconditionFailed {
                    detail: format!(
                        "wez resize pre-scan omitted stable geometry for pane {pane_id}"
                    ),
                })?;
        let invocation = adjust_pane_size_invocation(
            &self.wezterm_bin,
            &self.config_file,
            &scope.endpoint,
            *pane_id,
            direction,
            amount,
        )
        .map_err(|detail| ProviderError::NativeFailure { detail })?;
        self.run_owner_action("adjust-pane-size", invocation)?;
        let post = self.verified_action_scan(scope, expected)?;
        Self::sole_window(&post, &before.workspace)?;
        let after = post
            .pane(*pane_id)
            .ok_or_else(|| ProviderError::PostconditionFailed {
                detail: format!("wez resize target pane {pane_id} vanished"),
            })?;
        Self::require_same_pane_parent("resize", &before, after)?;
        let after_geometry = after
            .geometry
            .ok_or_else(|| ProviderError::PostconditionFailed {
                detail: format!("wez resize post-scan omitted stable geometry for pane {pane_id}"),
            })?;
        Ok(SplitResizeResult {
            server_epoch: expected,
            target: split.clone(),
            changed: before_geometry != after_geometry,
        })
    }

    /// Toggle zoom for one exact pane and prove the stable list flag flipped
    /// in the same tab and server epoch.
    pub fn toggle_split_zoom_exact(
        &self,
        scope: &InventoryScope,
        split: &ProviderHandle,
    ) -> ProviderResult<SplitZoomResult> {
        let ProviderHandle::Wz(pane_id) = split else {
            return Err(ProviderError::WrongInstance {
                detail: format!("not a wez pane handle: {split}"),
            });
        };
        let expected = Self::required_action_epoch(scope)?;
        let pre = self.verified_action_scan(scope, expected)?;
        let before = pre
            .pane(*pane_id)
            .cloned()
            .ok_or_else(|| ProviderError::NotFound {
                native_ref: split.to_string(),
            })?;
        Self::sole_window(&pre, &before.workspace)?;
        let before_zoomed = before
            .is_zoomed
            .ok_or_else(|| ProviderError::PostconditionFailed {
                detail: format!("wez zoom pre-scan omitted zoom state for pane {pane_id}"),
            })?;
        let invocation = toggle_zoom_pane_invocation(
            &self.wezterm_bin,
            &self.config_file,
            &scope.endpoint,
            *pane_id,
        )
        .map_err(|detail| ProviderError::NativeFailure { detail })?;
        self.run_owner_action("zoom-pane", invocation)?;
        let post = self.verified_action_scan(scope, expected)?;
        Self::sole_window(&post, &before.workspace)?;
        let after = post
            .pane(*pane_id)
            .ok_or_else(|| ProviderError::PostconditionFailed {
                detail: format!("wez zoom target pane {pane_id} vanished"),
            })?;
        Self::require_same_pane_parent("zoom", &before, after)?;
        let zoomed = after
            .is_zoomed
            .ok_or_else(|| ProviderError::PostconditionFailed {
                detail: format!("wez zoom post-scan omitted zoom state for pane {pane_id}"),
            })?;
        if zoomed == before_zoomed {
            return Err(ProviderError::PostconditionFailed {
                detail: format!(
                    "wez zoom postcondition did not flip for pane {pane_id}: {before_zoomed} -> \
                     {zoomed}"
                ),
            });
        }
        Ok(SplitZoomResult {
            server_epoch: expected,
            target: split.clone(),
            zoomed,
        })
    }
}

/// One raw user pane tuple from a verified list (sentinel rows excluded).
/// Mutations need the native window ids and tab↔pane parentage that the
/// normalized [`NativeSpaceRow`] deliberately hides.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PaneRef {
    window_id: u64,
    tab_id: u64,
    pane_id: u64,
    workspace: String,
    geometry: Option<PaneGeometry>,
    is_active: Option<bool>,
    is_zoomed: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PaneGeometry {
    cols: usize,
    rows: usize,
    left: usize,
    top: usize,
}

/// One complete sentinel-verified scan: the proven epoch, the normalized
/// rows the trait returns, and the raw pane tuples mutation verification
/// consumes. Internal to the adapter; the trait surface stays frozen.
struct DetailedScan {
    epoch: ServerEpoch,
    rows: Vec<NativeSpaceRow>,
    panes: Vec<PaneRef>,
    sentinel_panes: Vec<PaneRef>,
}

impl DetailedScan {
    fn into_inventory(self) -> NativeInventory {
        NativeInventory {
            server_epoch: Some(self.epoch),
            rows: self.rows,
        }
    }

    fn row(&self, token: &str) -> Option<&NativeSpaceRow> {
        self.rows.iter().find(|r| r.native_token == token)
    }

    /// Distinct native window ids of one workspace, first-seen order.
    fn workspace_windows(&self, token: &str) -> Vec<u64> {
        let mut windows = Vec::new();
        for pane in self.panes.iter().filter(|p| p.workspace == token) {
            if !windows.contains(&pane.window_id) {
                windows.push(pane.window_id);
            }
        }
        windows
    }

    fn workspace_panes(&self, token: &str) -> Vec<u64> {
        self.panes
            .iter()
            .filter(|p| p.workspace == token)
            .map(|p| p.pane_id)
            .collect()
    }

    /// Distinct tab ids of one workspace, first-seen order.
    fn workspace_tabs(&self, token: &str) -> Vec<u64> {
        let mut tabs = Vec::new();
        for pane in self.panes.iter().filter(|p| p.workspace == token) {
            if !tabs.contains(&pane.tab_id) {
                tabs.push(pane.tab_id);
            }
        }
        tabs
    }

    fn pane(&self, pane_id: u64) -> Option<&PaneRef> {
        self.panes.iter().find(|p| p.pane_id == pane_id)
    }

    /// First-listed pane of one tab (the deterministic split anchor).
    fn tab_first_pane(&self, tab_id: u64) -> Option<&PaneRef> {
        self.panes.iter().find(|p| p.tab_id == tab_id)
    }

    fn tab_panes(&self, tab_id: u64) -> Vec<u64> {
        self.panes
            .iter()
            .filter(|p| p.tab_id == tab_id)
            .map(|p| p.pane_id)
            .collect()
    }

    /// Deterministic §10.3 merge plan derived from THIS verified scan:
    /// target = the LOWEST native window id of the workspace, moves = every
    /// pane of every OTHER window in ascending (window_id, pane_id) order.
    /// Zero rows for the key is a typed [`ProviderError::NotFound`]; a
    /// sole-window workspace derives an empty move list ("nothing to do",
    /// never an error). Pure derivation — `normalize_apply` re-derives and
    /// compares for drift, so determinism here IS the drift detector.
    fn derive_normalize_plan(&self, token: &str) -> ProviderResult<NormalizePlan> {
        let panes: Vec<&PaneRef> = self.panes.iter().filter(|p| p.workspace == token).collect();
        let target_window =
            panes
                .iter()
                .map(|p| p.window_id)
                .min()
                .ok_or_else(|| ProviderError::NotFound {
                    native_ref: token.to_string(),
                })?;
        let mut moves: Vec<NormalizeMove> = panes
            .iter()
            .filter(|p| p.window_id != target_window)
            .map(|p| NormalizeMove {
                pane_id: p.pane_id,
                from_window: p.window_id,
            })
            .collect();
        moves.sort_unstable_by_key(|m| (m.from_window, m.pane_id));
        Ok(NormalizePlan {
            native_token: token.to_string(),
            server_epoch: self.epoch,
            target_window,
            moves,
        })
    }

    /// Workspace one native window currently lists under, if any.
    fn window_workspace(&self, window_id: u64) -> Option<&str> {
        self.panes
            .iter()
            .find(|p| p.window_id == window_id)
            .map(|p| p.workspace.as_str())
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
    #[serde(default)]
    size: Option<ListSize>,
    #[serde(default)]
    left_col: Option<usize>,
    #[serde(default)]
    top_row: Option<usize>,
    #[serde(default)]
    is_active: Option<bool>,
    #[serde(default)]
    is_zoomed: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ListSize {
    rows: usize,
    cols: usize,
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
    /// Static capabilities. `probed` names the checks every inventory
    /// enforces (dmux-side socket classification and the sentinel-in-list
    /// handshake). `cas_rename` stays statically `false` HERE by design:
    /// capability is a property of one live server endpoint, and this
    /// method receives no scope/endpoint to probe — a cached answer would
    /// silently survive a server swap. The explicit per-endpoint API is
    /// [`WezProvider::probe_cas_rename`] (positive probe, ADR 006 amended);
    /// orchestration calls it under its own fences and carries the result.
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
            Ok(scan) => InventoryOutcome::Complete(scan.into_inventory()),
            Err(fail) => fail.into_outcome(),
        }
    }

    /// Space create (plan §11.1): complete keyed pre-scan, ONE `spawn
    /// --new-window --workspace <key>`, complete same-epoch post-scan
    /// verifying exactly one window / one tab / one pane carrying the
    /// returned pane id. An already-present key is a typed
    /// `PostconditionFailed` carrying the existing state — never a second
    /// spawn (acknowledgement-loss replay, plan §10.2): journal replay must
    /// rebind through the keyed lookup or report conflict.
    fn create(&self, scope: &InventoryScope, spec: &CreateSpec) -> ProviderResult<NativeBinding> {
        let expected = Self::required_action_epoch(scope)?;
        let pre = self.verified_scan(scope, expected)?;
        if let Some(row) = pre.row(&spec.native_token) {
            let windows = pre.workspace_windows(&spec.native_token);
            let panes = pre.workspace_panes(&spec.native_token);
            return Err(ProviderError::PostconditionFailed {
                detail: format!(
                    "workspace_exists: key {:?} already present under epoch {} with \
                     window(s) {:?}, {} group(s), pane(s) {:?}; create never respawns — \
                     journal replay must rebind via keyed lookup or report conflict",
                    spec.native_token,
                    pre.epoch.0,
                    windows,
                    row.groups.len(),
                    panes
                ),
            });
        }
        let invocation = spawn_workspace_invocation(
            &self.wezterm_bin,
            &self.config_file,
            &scope.endpoint,
            &spec.native_token,
            spec.cwd.as_deref(),
            &spec.bootstrap_argv,
        )
        .map_err(|detail| ProviderError::NativeFailure { detail })?;
        let out = self.run_mutation(invocation)?;
        let pane_id = Self::parse_spawn_pane_id("create (spawn --new-window)", &out)?;

        let post = self.verified_scan(scope, pre.epoch)?;
        let row =
            post.row(&spec.native_token)
                .ok_or_else(|| ProviderError::PostconditionFailed {
                    detail: format!(
                        "wez create: workspace {:?} absent in the same-epoch re-list after \
                     spawn returned pane {pane_id}",
                        spec.native_token
                    ),
                })?;
        Self::sole_window(&post, &spec.native_token)?;
        let (group, split) = match row.groups.as_slice() {
            [group] => match group.splits.as_slice() {
                [split] if split.handle == ProviderHandle::Wz(pane_id) => {
                    (group.handle.clone(), split.handle.clone())
                }
                splits => {
                    return Err(ProviderError::PostconditionFailed {
                        detail: format!(
                            "wez create: workspace {:?} re-listed with splits {:?} instead \
                             of exactly the spawned pane {pane_id}",
                            spec.native_token,
                            splits.iter().map(|s| s.handle.clone()).collect::<Vec<_>>()
                        ),
                    });
                }
            },
            groups => {
                return Err(ProviderError::PostconditionFailed {
                    detail: format!(
                        "wez create: workspace {:?} re-listed with {} groups instead of \
                         exactly one (spawned pane {pane_id})",
                        spec.native_token,
                        groups.len()
                    ),
                });
            }
        };
        Ok(NativeBinding {
            native_token: spec.native_token.clone(),
            server_epoch: post.epoch,
            root_group: group,
            root_split: split,
        })
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
        Err(Self::gui_orchestration("prepare_presentation"))
    }

    /// Always a typed rejection: a Wez logical rename is registry-only
    /// (plan §2.5) — the opaque native workspace key never changes, so
    /// there is nothing for the provider to rename. Native workspace
    /// reassignment exists solely for adoption/repair through the fork CAS
    /// verb: run [`WezProvider::probe_cas_rename`] and, on a positive
    /// probe, [`WezProvider::cas_rename_workspace`] (ADR 006).
    fn rename(&self, _: &InventoryScope, _: &NativeBinding, _: &str) -> ProviderResult<()> {
        Err(ProviderError::NativeFailure {
            detail: "wez rename: a Wez logical rename is registry-only (plan §2.5); the \
                     opaque native workspace key never changes. Native reassignment is \
                     the adoption/repair CAS verb — probe_cas_rename then \
                     cas_rename_workspace (ADR 006)"
                .to_string(),
        })
    }

    /// Whole-Space removal (plan §14, ADR 005): bounded re-list/kill
    /// convergence, max [`REMOVE_MAX_ROUNDS`] rounds. Each round is a full
    /// sentinel/epoch-verified scan → exact `kill-pane` per listed pane
    /// (an already-dead exit-1 is a benign race; the next re-list decides) →
    /// re-list. Observed absence is confirmed by one FINAL verification
    /// list before returning Ok. Hitting the bound is a typed
    /// `PostconditionFailed` naming the surviving pane ids — the layer
    /// above reports partial and never tombstones. Multi-window workspaces
    /// are removable: §2.3 sanctions confirmed whole-Space removal as
    /// repair, so no one-window guard applies here.
    fn remove(&self, scope: &InventoryScope, binding: &NativeBinding) -> ProviderResult<()> {
        let expected = Self::binding_epoch(scope, binding)?;
        let mut survivors: Vec<u64> = Vec::new();
        for _round in 0..REMOVE_MAX_ROUNDS {
            let scan = self.verified_scan(scope, expected)?;
            let panes = scan.workspace_panes(&binding.native_token);
            if panes.is_empty() {
                // Converged at observation time — ADR 005: exit 0 is
                // point-in-time only, so confirm with a final list.
                let fin = self.verified_scan(scope, expected)?;
                let resurfaced = fin.workspace_panes(&binding.native_token);
                if resurfaced.is_empty() {
                    return Ok(());
                }
                survivors = resurfaced;
                continue;
            }
            for pane in &panes {
                let invocation = kill_pane_invocation(
                    &self.wezterm_bin,
                    &self.config_file,
                    &scope.endpoint,
                    *pane,
                )
                .map_err(|detail| ProviderError::NativeFailure { detail })?;
                // Exit status intentionally unused (ADR 001/005): the
                // already-dead race exits 1 benignly and the next verified
                // re-list is the only truth; runner failures stay typed.
                let _ = self.run_mutation(invocation)?;
            }
            survivors = panes;
        }
        Err(ProviderError::PostconditionFailed {
            detail: format!(
                "remove_unconverged: workspace {:?} still holds pane(s) {:?} after \
                 {REMOVE_MAX_ROUNDS} re-list/kill rounds (ADR 005 bound); partial — \
                 report, never tombstone",
                binding.native_token, survivors
            ),
        })
    }

    fn group_list(
        &self,
        scope: &InventoryScope,
        binding: &NativeBinding,
    ) -> ProviderResult<Vec<NativeGroupRow>> {
        let expected = Self::binding_epoch(scope, binding)?;
        let scan = self.verified_scan(scope, expected)?;
        scan.rows
            .into_iter()
            .find(|r| r.native_token == binding.native_token)
            .map(|r| r.groups)
            .ok_or_else(|| ProviderError::NotFound {
                native_ref: binding.native_token.clone(),
            })
    }

    /// Group create (plan §11.1): `spawn --window-id <only-window-id>` into
    /// the workspace's sole window, then same-epoch re-list verifying the
    /// spawned pane landed in a NEW tab of that workspace/window. One-window
    /// invariant checked before (it also yields the target window id) and
    /// after.
    fn group_new(
        &self,
        scope: &InventoryScope,
        binding: &NativeBinding,
        spec: &CreateSpec,
    ) -> ProviderResult<ProviderHandle> {
        let expected = Self::binding_epoch(scope, binding)?;
        let pre = self.verified_scan(scope, expected)?;
        if pre.row(&binding.native_token).is_none() {
            return Err(ProviderError::NotFound {
                native_ref: binding.native_token.clone(),
            });
        }
        let window = Self::sole_window(&pre, &binding.native_token)?;
        let pre_tabs = pre.workspace_tabs(&binding.native_token);
        let invocation = spawn_group_invocation(
            &self.wezterm_bin,
            &self.config_file,
            &scope.endpoint,
            window,
            spec.cwd.as_deref(),
            &spec.bootstrap_argv,
        )
        .map_err(|detail| ProviderError::NativeFailure { detail })?;
        let out = self.run_mutation(invocation)?;
        let pane_id = Self::parse_spawn_pane_id("group_new (spawn --window-id)", &out)?;

        let post = self.verified_scan(scope, pre.epoch)?;
        Self::sole_window(&post, &binding.native_token)?;
        let pane = post
            .pane(pane_id)
            .ok_or_else(|| ProviderError::PostconditionFailed {
                detail: format!(
                    "wez group_new: spawned pane {pane_id} absent in the same-epoch re-list"
                ),
            })?;
        if pane.workspace != binding.native_token
            || pane.window_id != window
            || pre_tabs.contains(&pane.tab_id)
        {
            return Err(ProviderError::PostconditionFailed {
                detail: format!(
                    "wez group_new: pane {pane_id} re-listed in workspace {:?} window {} \
                     tab {} (wanted a NEW tab of workspace {:?} window {window}, \
                     existing tabs {pre_tabs:?})",
                    pane.workspace, pane.window_id, pane.tab_id, binding.native_token
                ),
            });
        }
        Ok(ProviderHandle::Wz(pane.tab_id))
    }

    /// Wez Group/Split activation is GUI-local correlation after import
    /// (plan §11.1), not an owner-provider mutation.
    fn group_activate(&self, _: &InventoryScope, _: &ProviderHandle) -> ProviderResult<()> {
        Err(Self::gui_orchestration("group_activate"))
    }

    /// Group rename (plan §11.1): `set-tab-title --tab-id`, bracketed by
    /// verified scans; the post-scan must re-list the tab with the new
    /// title (empty title normalizes to None).
    fn group_rename(
        &self,
        scope: &InventoryScope,
        handle: &ProviderHandle,
        title: &str,
    ) -> ProviderResult<()> {
        let ProviderHandle::Wz(tab_id) = handle else {
            return Err(ProviderError::WrongInstance {
                detail: format!("not a wez tab handle: {handle}"),
            });
        };
        let expected = Self::required_action_epoch(scope)?;
        let pre = self.verified_scan(scope, expected)?;
        let owner = pre
            .tab_first_pane(*tab_id)
            .ok_or_else(|| ProviderError::NotFound {
                native_ref: handle.to_string(),
            })?;
        let workspace = owner.workspace.clone();
        Self::sole_window(&pre, &workspace)?;
        let invocation = set_tab_title_invocation(
            &self.wezterm_bin,
            &self.config_file,
            &scope.endpoint,
            *tab_id,
            title,
        )
        .map_err(|detail| ProviderError::NativeFailure { detail })?;
        let out = self.run_mutation(invocation)?;
        if !out.ok() {
            return Err(ProviderError::NativeFailure {
                detail: format!(
                    "wez group_rename (set-tab-title --tab-id {tab_id}) exited {}: {}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            });
        }
        let post = self.verified_scan(scope, pre.epoch)?;
        Self::sole_window(&post, &workspace)?;
        let row = post
            .row(&workspace)
            .ok_or_else(|| ProviderError::PostconditionFailed {
                detail: format!("wez group_rename: workspace {workspace:?} absent in the re-list"),
            })?;
        let group = row
            .groups
            .iter()
            .find(|g| g.handle == *handle)
            .ok_or_else(|| ProviderError::PostconditionFailed {
                detail: format!("wez group_rename: tab {tab_id} absent in the re-list"),
            })?;
        let want = (!title.is_empty()).then(|| title.to_string());
        if group.title != want {
            return Err(ProviderError::PostconditionFailed {
                detail: format!(
                    "wez group_rename: tab {tab_id} re-listed with title {:?}, wanted {:?}",
                    group.title, want
                ),
            });
        }
        Ok(())
    }

    /// Group removal: exact kills of the tab's listed panes plus verified
    /// absence. Refuses (typed) when the tab is the workspace's last Group:
    /// that would delete the Space through a hidden cascade (plan §7.2) —
    /// whole-Space removal must go through `remove`.
    fn group_remove(&self, scope: &InventoryScope, handle: &ProviderHandle) -> ProviderResult<()> {
        let ProviderHandle::Wz(tab_id) = handle else {
            return Err(ProviderError::WrongInstance {
                detail: format!("not a wez tab handle: {handle}"),
            });
        };
        let expected = Self::required_action_epoch(scope)?;
        let pre = self.verified_scan(scope, expected)?;
        let owner = pre
            .tab_first_pane(*tab_id)
            .ok_or_else(|| ProviderError::NotFound {
                native_ref: handle.to_string(),
            })?;
        let workspace = owner.workspace.clone();
        Self::sole_window(&pre, &workspace)?;
        let tabs = pre.workspace_tabs(&workspace);
        if tabs == [*tab_id] {
            return Err(ProviderError::NativeFailure {
                detail: format!(
                    "refused_last_group: tab {tab_id} is the last Group of workspace \
                     {workspace:?}; removing it would delete the Space through a hidden \
                     cascade (plan §7.2) — use whole-Space remove"
                ),
            });
        }
        for pane in pre.tab_panes(*tab_id) {
            let invocation =
                kill_pane_invocation(&self.wezterm_bin, &self.config_file, &scope.endpoint, pane)
                    .map_err(|detail| ProviderError::NativeFailure { detail })?;
            // Already-dead exit-1 is a benign race (ADR 005); the verified
            // re-list below is the only postcondition authority.
            let _ = self.run_mutation(invocation)?;
        }
        let post = self.verified_scan(scope, pre.epoch)?;
        let remaining = post.tab_panes(*tab_id);
        if !remaining.is_empty() {
            return Err(ProviderError::PostconditionFailed {
                detail: format!(
                    "wez group_remove: tab {tab_id} still holds pane(s) {remaining:?} \
                     after exact kills"
                ),
            });
        }
        if post.row(&workspace).is_none() {
            return Err(ProviderError::PostconditionFailed {
                detail: format!(
                    "wez group_remove: workspace {workspace:?} vanished while removing \
                     tab {tab_id} (concurrent removal?)"
                ),
            });
        }
        Self::sole_window(&post, &workspace)?;
        Ok(())
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
        let expected = Self::required_action_epoch(scope)?;
        let scan = self.verified_scan(scope, expected)?;
        scan.rows
            .into_iter()
            .flat_map(|r| r.groups)
            .find(|g| g.handle == *group)
            .map(|g| g.splits)
            .ok_or_else(|| ProviderError::NotFound {
                native_ref: group.to_string(),
            })
    }

    /// Split create (plan §11.1): `split-pane --pane-id <anchor>` where the
    /// anchor is the group's first-listed pane (deterministic; targeting a
    /// specific pane is a later CLI-layer concern), then same-epoch re-list
    /// verifying the new pane landed in the SAME tab. One-window invariant
    /// checked before and after.
    fn split_new(
        &self,
        scope: &InventoryScope,
        group: &ProviderHandle,
        split: &SplitSpec,
    ) -> ProviderResult<ProviderHandle> {
        let spec = &split.spec;
        let ProviderHandle::Wz(tab_id) = group else {
            return Err(ProviderError::WrongInstance {
                detail: format!("not a wez tab handle: {group}"),
            });
        };
        let expected = Self::required_action_epoch(scope)?;
        let pre = self.verified_scan(scope, expected)?;
        let anchor = pre
            .tab_first_pane(*tab_id)
            .ok_or_else(|| ProviderError::NotFound {
                native_ref: group.to_string(),
            })?;
        let workspace = anchor.workspace.clone();
        let anchor_pane = anchor.pane_id;
        Self::sole_window(&pre, &workspace)?;
        let invocation = split_pane_invocation(
            &self.wezterm_bin,
            &self.config_file,
            &scope.endpoint,
            anchor_pane,
            split.direction,
            split.percent,
            spec.cwd.as_deref(),
            &spec.bootstrap_argv,
        )
        .map_err(|detail| ProviderError::NativeFailure { detail })?;
        let out = self.run_mutation(invocation)?;
        let pane_id = Self::parse_spawn_pane_id("split_new (split-pane --pane-id)", &out)?;

        let post = self.verified_scan(scope, pre.epoch)?;
        Self::sole_window(&post, &workspace)?;
        let pane = post
            .pane(pane_id)
            .ok_or_else(|| ProviderError::PostconditionFailed {
                detail: format!(
                    "wez split_new: split pane {pane_id} absent in the same-epoch re-list"
                ),
            })?;
        if pane.tab_id != *tab_id || pane.workspace != workspace {
            return Err(ProviderError::PostconditionFailed {
                detail: format!(
                    "wez split_new: pane {pane_id} re-listed in workspace {:?} tab {} \
                     (wanted tab {tab_id} of workspace {workspace:?})",
                    pane.workspace, pane.tab_id
                ),
            });
        }
        Ok(ProviderHandle::Wz(pane_id))
    }

    fn split_activate(&self, _: &InventoryScope, _: &ProviderHandle) -> ProviderResult<()> {
        Err(Self::gui_orchestration("split_activate"))
    }

    fn activate_group_exact(
        &self,
        scope: &InventoryScope,
        group: &ProviderHandle,
    ) -> ProviderResult<GroupActivationResult> {
        WezProvider::activate_group_exact(self, scope, group)
    }

    fn select_split_direction(
        &self,
        scope: &InventoryScope,
        origin: &ProviderHandle,
        direction: SplitDirection,
    ) -> ProviderResult<SplitDirectionResult> {
        WezProvider::select_split_direction(self, scope, origin, direction)
    }

    fn resize_split_exact(
        &self,
        scope: &InventoryScope,
        split: &ProviderHandle,
        direction: SplitDirection,
        amount: u16,
    ) -> ProviderResult<SplitResizeResult> {
        WezProvider::resize_split_exact(self, scope, split, direction, amount)
    }

    fn toggle_split_zoom_exact(
        &self,
        scope: &InventoryScope,
        split: &ProviderHandle,
    ) -> ProviderResult<SplitZoomResult> {
        WezProvider::toggle_split_zoom_exact(self, scope, split)
    }

    /// Split removal: one exact `kill-pane` plus verified absence. Refuses
    /// (typed) when the pane is the workspace's last Split: the provider
    /// never silently deletes a Space through split_remove (plan §7.2; the
    /// Group-level cascade of a last-pane-in-tab is the CLI's refusal, not
    /// the provider's).
    fn split_remove(&self, scope: &InventoryScope, handle: &ProviderHandle) -> ProviderResult<()> {
        let ProviderHandle::Wz(pane_id) = handle else {
            return Err(ProviderError::WrongInstance {
                detail: format!("not a wez pane handle: {handle}"),
            });
        };
        let expected = Self::required_action_epoch(scope)?;
        let pre = self.verified_scan(scope, expected)?;
        let pane = pre.pane(*pane_id).ok_or_else(|| ProviderError::NotFound {
            native_ref: handle.to_string(),
        })?;
        let workspace = pane.workspace.clone();
        Self::sole_window(&pre, &workspace)?;
        if pre.workspace_panes(&workspace) == [*pane_id] {
            return Err(ProviderError::NativeFailure {
                detail: format!(
                    "refused_last_pane: pane {pane_id} is the last Split of workspace \
                     {workspace:?}; removing it would delete the Space through a hidden \
                     cascade (plan §7.2) — use whole-Space remove"
                ),
            });
        }
        let invocation = kill_pane_invocation(
            &self.wezterm_bin,
            &self.config_file,
            &scope.endpoint,
            *pane_id,
        )
        .map_err(|detail| ProviderError::NativeFailure { detail })?;
        // Already-dead exit-1 is a benign race (ADR 005); the verified
        // re-list below is the only postcondition authority.
        let _ = self.run_mutation(invocation)?;
        let post = self.verified_scan(scope, pre.epoch)?;
        if post.pane(*pane_id).is_some() {
            return Err(ProviderError::PostconditionFailed {
                detail: format!("wez split_remove: pane {pane_id} still listed after kill-pane"),
            });
        }
        if post.row(&workspace).is_some() {
            Self::sole_window(&post, &workspace)?;
        }
        Ok(())
    }

    /// Local Wez normalization plan (plan §10.3, P8a): one scan pinned to
    /// the scope's published epoch, then the deterministic merge plan —
    /// target is the LOWEST native window id of the workspace, moves are
    /// every pane of every OTHER window in ascending (window_id, pane_id)
    /// order. Strictly read-only: the plan is rendered for confirmation
    /// above this layer and applied by [`Provider::normalize_apply`] under
    /// the caller's exclusive fence. A sole-window workspace returns Ok with
    /// zero moves ("nothing to do"); zero rows for the opaque key is a typed
    /// [`ProviderError::NotFound`].
    fn normalize_plan(
        &self,
        scope: &InventoryScope,
        native_token: &str,
    ) -> ProviderResult<NormalizePlan> {
        let expected = Self::required_action_epoch(scope)?;
        let scan = self.verified_scan(scope, expected)?;
        scan.derive_normalize_plan(native_token)
    }

    /// Apply a previously confirmed merge plan (plan §10.3): re-scan pinned
    /// to `plan.server_epoch` (a flip is [`ProviderError::EpochChanged`];
    /// native IDs discarded), re-derive the plan from the live tree and
    /// require it to EQUAL the confirmed plan — any drift (a new pane, a
    /// vanished pane, a changed window set) refuses with the stable
    /// `normalize_drift:` detail prefix and ZERO mutation. Then each move
    /// runs the exact `move-pane-to-new-tab --pane-id P --window-id
    /// <target>` argv (exit status is diagnostics only, ADR 001/005), and
    /// bounded re-list rounds ([`REMOVE_MAX_ROUNDS`], the ADR 005 kill-
    /// convergence pattern) must PROVE exactly one remaining window — the
    /// target — with every planned pane surviving in it. Rounds re-issue
    /// only PLANNED moves still pending: panes outside the plan are never
    /// touched and nothing is ever killed. Hitting the bound is a typed
    /// `normalize_unconverged:` partial — quarantined, never half-managed.
    /// An empty (sole-window) plan is a verified no-op success.
    fn normalize_apply(&self, scope: &InventoryScope, plan: &NormalizePlan) -> ProviderResult<()> {
        let token = plan.native_token.as_str();
        let pre = self.verified_scan(scope, plan.server_epoch)?;
        let derived = match pre.derive_normalize_plan(token) {
            Ok(derived) => derived,
            Err(ProviderError::NotFound { .. }) => {
                return Err(ProviderError::PostconditionFailed {
                    detail: format!(
                        "normalize_drift: workspace {token:?} has zero live panes in the \
                         same-epoch re-list (confirmed plan: target window {}, {} \
                         move(s)); re-plan required",
                        plan.target_window,
                        plan.moves.len()
                    ),
                });
            }
            Err(other) => return Err(other),
        };
        if derived != *plan {
            return Err(ProviderError::PostconditionFailed {
                detail: format!(
                    "normalize_drift: confirmed plan for {token:?} no longer matches the \
                     live tree: planned target {} moves {:?}; derived target {} moves \
                     {:?}; re-plan required",
                    plan.target_window, plan.moves, derived.target_window, derived.moves
                ),
            });
        }
        if plan.moves.is_empty() {
            // Sole-window workspace: verified nothing-to-do success.
            return Ok(());
        }
        for mv in &plan.moves {
            let invocation = move_pane_invocation(
                &self.wezterm_bin,
                &self.config_file,
                &scope.endpoint,
                mv.pane_id,
                plan.target_window,
            )
            .map_err(|detail| ProviderError::NativeFailure { detail })?;
            // Exit status intentionally unused (ADR 001/005 pattern): the
            // bounded verified re-list below is the sole postcondition
            // authority; runner failures stay typed.
            let _ = self.run_mutation(invocation)?;
        }
        let mut last_windows: Vec<u64> = Vec::new();
        let mut last_missing: Vec<u64> = Vec::new();
        for _round in 0..REMOVE_MAX_ROUNDS {
            let scan = self.verified_scan(scope, plan.server_epoch)?;
            let windows = scan.workspace_windows(token);
            let missing: Vec<u64> = plan
                .moves
                .iter()
                .map(|m| m.pane_id)
                .filter(|id| scan.pane(*id).is_none_or(|p| p.workspace != token))
                .collect();
            let converged = matches!(windows.as_slice(), [w] if *w == plan.target_window);
            if converged && missing.is_empty() {
                return Ok(());
            }
            // Re-issue only PLANNED moves still pending; a pane outside the
            // plan is never touched (it makes convergence impossible and
            // the bound below reports it typed).
            for mv in &plan.moves {
                let pending = scan
                    .pane(mv.pane_id)
                    .is_some_and(|p| p.workspace == token && p.window_id != plan.target_window);
                if pending {
                    let invocation = move_pane_invocation(
                        &self.wezterm_bin,
                        &self.config_file,
                        &scope.endpoint,
                        mv.pane_id,
                        plan.target_window,
                    )
                    .map_err(|detail| ProviderError::NativeFailure { detail })?;
                    let _ = self.run_mutation(invocation)?;
                }
            }
            last_windows = windows;
            last_missing = missing;
        }
        Err(ProviderError::PostconditionFailed {
            detail: format!(
                "normalize_unconverged: workspace {token:?} still spans window(s) \
                 {last_windows:?} (target {}) with planned pane(s) {last_missing:?} \
                 missing after {REMOVE_MAX_ROUNDS} re-list/move rounds (ADR 005 \
                 bound); partial — quarantined, never half-managed",
                plan.target_window
            ),
        })
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
        let scan = self.verified_scan(scope, expected)?;
        scan.rows
            .into_iter()
            .find(|r| r.native_token == binding.native_token)
            .ok_or_else(|| ProviderError::NotFound {
                native_ref: binding.native_token.clone(),
            })
    }
}

// ---------------------------------------------------------------------------
// CAS adoption verb and capability probe (ADR 006, inherent — not on the
// frozen trait; orchestration reaches these through the concrete provider)
// ---------------------------------------------------------------------------

/// Expected-workspace token used by the capability probe. The probed window
/// id is `u64::MAX`, which never exists (`WindowId` is process-monotonic,
/// ADR 006), so no mutation is possible regardless of these tokens.
const CAS_PROBE_WORKSPACE: &str = "dmux:probe:cas";

impl<R: WezRunner> WezProvider<R> {
    /// Sole native window id of one workspace token, from a scan pinned to
    /// the scope's published epoch (an unmanaged scope is refused before the
    /// endpoint is probed, ADR 012 WS-A.6). [`NativeSpaceRow`]
    /// deliberately hides window ids, but the adopt flow needs one to feed
    /// [`WezProvider::cas_rename_workspace`] — this is the sanctioned
    /// lookup. Absent workspace ⇒ typed [`ProviderError::NotFound`];
    /// spanning multiple windows ⇒ [`ProviderError::MultiWindow`] with the
    /// exact count (plan §2.3: such a resource permits only listing,
    /// inspect/export, normalization repair, or confirmed removal).
    /// Point-in-time like every read: the CAS call itself re-verifies
    /// `(window_id, expected_workspace, sole-window)` atomically server-side
    /// (ADR 006), so a stale answer can never mutate the wrong window.
    pub fn sole_window_id(
        &self,
        scope: &InventoryScope,
        native_token: &str,
    ) -> ProviderResult<u64> {
        let expected = Self::required_action_epoch(scope)?;
        let scan = self.verified_scan(scope, expected)?;
        Self::sole_window(&scan, native_token)
    }

    /// Positive CAS capability probe (amended ADR 006): issue the CAS verb
    /// through the configured fork CLI against the known-nonexistent window
    /// id `u64::MAX` and classify the result.
    ///
    /// - `no_such_window` ⇒ `Ok(true)`: the verb executed server-side, and
    ///   against a nonexistent window zero mutation was possible.
    /// - the stable `invalid PDU Invalid {{ ident: 63 }}` reason ⇒
    ///   `Ok(false)`: stock codec-45 server, capability missing, zero
    ///   mutation guaranteed.
    /// - anything else (connect failures, a non-fork CLI rejecting the
    ///   argv, an impossible `Renamed`) ⇒ typed error — never a guess.
    ///
    /// Connect success is never proof (`wezterm cli` performs no codec
    /// handshake), which is why [`Provider::capabilities`] stays statically
    /// `cas_rename: false` and this per-endpoint probe is the explicit API.
    pub fn probe_cas_rename(&self, scope: &InventoryScope) -> ProviderResult<bool> {
        self.preflight(scope)
            .map_err(ScanFail::into_provider_error)?;
        let invocation = cas_rename_invocation(
            self.cas_bin(),
            &self.config_file,
            &scope.endpoint,
            u64::MAX,
            CAS_PROBE_WORKSPACE,
            false,
            CAS_PROBE_WORKSPACE,
        )
        .map_err(|detail| ProviderError::NativeFailure { detail })?;
        let out = self.run_mutation(invocation)?;
        let stderr = String::from_utf8_lossy(&out.stderr);
        match classify_cas_rename(out.ok(), &stderr) {
            CasClassification::Outcome(CasRenameOutcome::NoSuchWindow) => Ok(true),
            CasClassification::CapabilityMissing => Ok(false),
            other => Err(ProviderError::NativeFailure {
                detail: format!(
                    "cas_probe_indeterminate: CAS against window id u64::MAX classified \
                     as {other:?} instead of no_such_window/invalid-PDU; capability \
                     unknown (never guessed)"
                ),
            }),
        }
    }

    /// Fork CAS workspace reassignment (ADR 006): `cli rename-workspace
    /// --window-id N --if-workspace OLD [--if-sole-window] NEW` through the
    /// configured fork CLI, bracketed by scans pinned to the scope's
    /// published epoch (the dmux-side epoch bracket ADR 006 requires; an
    /// unmanaged scope is refused before any command, ADR 012 WS-A.6).
    /// Typed CAS outcomes come back as [`CasRenameOutcome`] — every
    /// non-`Renamed` variant is a
    /// server-guaranteed zero-mutation result. A `Renamed` outcome is
    /// additionally verified in the post-scan (the window must list under
    /// `new_workspace`). A stock server rejecting PDU ident 63 is a typed
    /// `NativeFailure` whose detail starts `cas_capability_missing:` —
    /// callers gate on [`WezProvider::probe_cas_rename`] first.
    pub fn cas_rename_workspace(
        &self,
        scope: &InventoryScope,
        window_id: u64,
        expected_workspace: &str,
        new_workspace: &str,
        expect_sole_window: bool,
    ) -> ProviderResult<CasRenameOutcome> {
        let expected = Self::required_action_epoch(scope)?;
        let pre = self.verified_scan(scope, expected)?;
        let invocation = cas_rename_invocation(
            self.cas_bin(),
            &self.config_file,
            &scope.endpoint,
            window_id,
            expected_workspace,
            expect_sole_window,
            new_workspace,
        )
        .map_err(|detail| ProviderError::NativeFailure { detail })?;
        let out = self.run_mutation(invocation)?;
        let stderr = String::from_utf8_lossy(&out.stderr);
        let outcome = match classify_cas_rename(out.ok(), &stderr) {
            CasClassification::Outcome(outcome) => outcome,
            CasClassification::CapabilityMissing => {
                return Err(ProviderError::NativeFailure {
                    detail: format!(
                        "cas_capability_missing: server rejected PDU ident 63 \
                         ({CAS_MISSING_PDU_STDERR}); stock codec-45 server, zero \
                         mutation (ADR 006) — gate on probe_cas_rename"
                    ),
                });
            }
            CasClassification::Unclassified(detail) => {
                return Err(ProviderError::NativeFailure {
                    detail: format!("cas_rename_unclassified: {detail}"),
                });
            }
        };
        let post = self.verified_scan(scope, pre.epoch)?;
        if outcome == CasRenameOutcome::Renamed {
            match post.window_workspace(window_id) {
                Some(ws) if ws == new_workspace => {}
                observed => {
                    return Err(ProviderError::PostconditionFailed {
                        detail: format!(
                            "wez cas_rename: server reported Renamed but window \
                             {window_id} re-lists under {observed:?} instead of \
                             {new_workspace:?}"
                        ),
                    });
                }
            }
        }
        Ok(outcome)
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
        match expected {
            Some(epoch) => InventoryScope::managed(Backend::Wez, SOCK, epoch),
            // audit(unmanaged_endpoint): test-only scope(Option) helper
            None => InventoryScope::unmanaged_endpoint(Backend::Wez, SOCK),
        }
    }

    /// The managed scope every native verb needs: pinned to the epoch the
    /// canned servers' sentinel serves (ADR 012 WS-A.6/A.8). `scope(None)`
    /// is kept only for the discovery read (`inventory`), the capability
    /// probe, and the refusal tests that prove the fence.
    fn pinned() -> InventoryScope {
        scope(Some(ServerEpoch(EPOCH)))
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
    fn native_tree_witness_is_exact_and_rejects_same_epoch_sentinel_duplicates() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&fixture("list_two_workspaces.json"))],
        );
        let witness = provider(&runner)
            .native_tree_witness(&scope(Some(ServerEpoch(EPOCH))))
            .unwrap();
        assert_eq!(witness.server_epoch, ServerEpoch(EPOCH));
        assert_eq!(
            (
                witness.sentinel_window_id,
                witness.sentinel_tab_id,
                witness.sentinel_pane_id,
            ),
            (0, 0, 0)
        );
        assert_eq!(
            witness
                .panes
                .iter()
                .map(|pane| pane.pane_id)
                .collect::<Vec<_>>(),
            vec![0, 100, 101, 102, 103]
        );

        let duplicate = format!(
            r#"[
              {{"window_id":0,"tab_id":0,"pane_id":0,"workspace":"dmux:system:{EPOCH}"}},
              {{"window_id":9,"tab_id":9,"pane_id":9,"workspace":"dmux:system:{EPOCH}"}}
            ]"#
        );
        let runner = ScriptedRunner::new(vec![ProbeOutcome::Connectable], vec![ok(&duplicate)]);
        let error = provider(&runner)
            .native_tree_witness(&scope(Some(ServerEpoch(EPOCH))))
            .unwrap_err();
        assert!(
            format!("{error:?}").contains("exactly one sentinel pane"),
            "{error:?}"
        );
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
        match provider(&runner).inspect(&pinned(), &binding("missing", EPOCH)) {
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

        // Pin and binding agree on an epoch the live sentinel does not
        // serve: the scan refuses, naming the pin and the live epoch.
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&fixture("list_two_workspaces.json"))],
        );
        let stale = binding("alpha", Uuid::from_u128(0xdead_beef));
        match provider(&runner).inspect(&scope(Some(stale.server_epoch)), &stale) {
            Err(ProviderError::EpochChanged { expected, observed }) => {
                assert_eq!(expected, stale.server_epoch);
                assert_eq!(observed, Some(ServerEpoch(EPOCH)));
            }
            other => panic!("live epoch mismatch must be EpochChanged, got {other:?}"),
        }
        assert_eq!(
            runner.run_calls.borrow().len(),
            1,
            "the verifying list only"
        );
    }

    /// WS-A.8 (review findings #5/#18): `binding_epoch` used to answer the
    /// binding's own epoch on an unpinned scope, and the caller synthesised
    /// that binding from the live scan — the fence compared the server
    /// against itself. Now an unpinned scope refuses `WrongInstance` and a
    /// binding whose epoch is not the pin refuses `EpochChanged`, both with
    /// the endpoint never probed; a matching binding yields the pin.
    #[test]
    fn binding_epoch_refuses_an_unpinned_scope_and_a_binding_off_the_pin() {
        let runner = ScriptedRunner::new(vec![], vec![]);
        let p = provider(&runner);
        let matching = binding("alpha", EPOCH);
        let stale = binding("alpha", Uuid::from_u128(0xdead_beef));

        // Unpinned: the binding's self-report is not a pin.
        let unpinned = scope(None);
        let results: Vec<(&str, ProviderResult<()>)> = vec![
            ("inspect", p.inspect(&unpinned, &matching).map(|_| ())),
            (
                "group_new",
                p.group_new(&unpinned, &matching, &spec("alpha"))
                    .map(|_| ()),
            ),
            ("remove", p.remove(&unpinned, &matching)),
            ("group_list", p.group_list(&unpinned, &matching).map(|_| ())),
        ];
        for (verb, result) in results {
            match result {
                Err(ProviderError::WrongInstance { detail }) => {
                    assert!(detail.contains("managed scope"), "{verb}: {detail}");
                }
                other => panic!("{verb} on an unpinned scope must be WrongInstance, got {other:?}"),
            }
        }

        // Pinned, binding off the pin: refused before any command.
        let results: Vec<(&str, ProviderResult<()>)> = vec![
            ("inspect", p.inspect(&pinned(), &stale).map(|_| ())),
            (
                "group_new",
                p.group_new(&pinned(), &stale, &spec("alpha")).map(|_| ()),
            ),
            ("remove", p.remove(&pinned(), &stale)),
            ("group_list", p.group_list(&pinned(), &stale).map(|_| ())),
        ];
        for (verb, result) in results {
            match result {
                Err(ProviderError::EpochChanged { expected, observed }) => {
                    assert_eq!(expected, ServerEpoch(EPOCH), "{verb}");
                    assert_eq!(observed, Some(stale.server_epoch), "{verb}");
                }
                other => {
                    panic!("{verb} with a binding off the pin must be EpochChanged, got {other:?}")
                }
            }
        }
        assert!(
            runner.probe_calls.borrow().is_empty(),
            "endpoint never probed"
        );
        assert!(runner.run_calls.borrow().is_empty(), "no native command");

        // Pinned, binding on the pin: the pin is what the scan is held to.
        assert_eq!(
            WezProvider::<&ScriptedRunner>::binding_epoch(&pinned(), &matching).unwrap(),
            ServerEpoch(EPOCH)
        );
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
        match p.split_list(&pinned(), &ProviderHandle::Tx(10)) {
            Err(ProviderError::WrongInstance { detail }) => {
                assert!(detail.contains("not a wez tab handle"), "{detail}");
            }
            other => panic!("tmux handle must be WrongInstance, got {other:?}"),
        }
        // A managed read without a pin is refused before the endpoint is
        // probed (ADR 012 WS-A.6/A.13): the runner has no third probe.
        match p.split_list(&scope(None), &ProviderHandle::Wz(10)) {
            Err(ProviderError::WrongInstance { detail }) => {
                assert!(detail.contains("managed scope"), "{detail}");
            }
            other => panic!("unpinned split_list must be WrongInstance, got {other:?}"),
        }
        assert_eq!(
            runner.probe_calls.borrow().len(),
            2,
            "two pinned scans only"
        );
    }

    // -- non-provider verbs stay typed ---------------------------------------

    #[test]
    fn gui_and_rename_verbs_stay_typed() {
        let runner = ScriptedRunner::new(vec![], vec![]);
        let p = provider(&runner);
        let s = scope(Some(ServerEpoch(EPOCH)));
        let b = binding("alpha", EPOCH);
        let handle = ProviderHandle::Wz(10);
        for r in [
            p.prepare_presentation(&s, &b, None).map(|_| ()),
            p.group_activate(&s, &handle),
            p.split_activate(&s, &handle),
        ] {
            match r {
                Err(ProviderError::NativeFailure { detail }) => {
                    assert!(detail.contains("GUI orchestration"), "{detail}");
                }
                other => panic!("GUI verb must fail typed, got {other:?}"),
            }
        }
        match p.rename(&s, &b, "x") {
            Err(ProviderError::NativeFailure { detail }) => {
                assert!(detail.contains("registry-only"), "{detail}");
                assert!(detail.contains("cas_rename_workspace"), "{detail}");
            }
            other => panic!("wez rename must fail typed, got {other:?}"),
        }
        assert!(runner.run_calls.borrow().is_empty(), "no child spawned");
    }

    // -- scripted mutations ---------------------------------------------------

    /// Canned list JSON: the ADR 002 sentinel under `epoch` plus the given
    /// `(window_id, tab_id, pane_id, workspace)` user rows.
    fn canned_epoch(epoch: Uuid, rows: &[(u64, u64, u64, &str)]) -> String {
        let mut out = format!(
            r#"[{{"window_id":0,"tab_id":0,"pane_id":0,"workspace":"dmux:system:{epoch}"}}"#
        );
        for (w, t, p, ws) in rows {
            out.push_str(&format!(
                r#",{{"window_id":{w},"tab_id":{t},"pane_id":{p},"workspace":"{ws}"}}"#
            ));
        }
        out.push(']');
        out
    }

    fn canned(rows: &[(u64, u64, u64, &str)]) -> String {
        canned_epoch(EPOCH, rows)
    }

    /// Canned stable `cli list --format json` rows for P9 action witnesses.
    /// `(window, tab, pane, workspace, cols, rows, left, top, active, zoomed)`.
    fn action_canned(
        rows: &[(u64, u64, u64, &str, usize, usize, usize, usize, bool, bool)],
    ) -> String {
        let mut out = format!(
            r#"[{{"window_id":0,"tab_id":0,"pane_id":0,"workspace":"dmux:system:{EPOCH}"}}"#
        );
        for (window, tab, pane, workspace, cols, rows, left, top, active, zoomed) in rows {
            out.push_str(&format!(
                r#",{{"window_id":{window},"tab_id":{tab},"pane_id":{pane},"workspace":"{workspace}","size":{{"cols":{cols},"rows":{rows}}},"left_col":{left},"top_row":{top},"is_active":{active},"is_zoomed":{zoomed}}}"#
            ));
        }
        out.push(']');
        out
    }

    fn fail(stderr: &str) -> Result<RunOutput, RunError> {
        Ok(RunOutput {
            status: 1,
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        })
    }

    fn spec(token: &str) -> CreateSpec {
        CreateSpec {
            native_token: token.into(),
            cwd: None,
            bootstrap_argv: vec!["/bin/true".into()],
        }
    }

    #[test]
    fn owner_action_builders_are_exact_and_always_prefer_mux_without_auto_start() {
        let invocations = [
            activate_tab_invocation(BIN, CFG, SOCK, 10).unwrap(),
            activate_pane_invocation(BIN, CFG, SOCK, 100).unwrap(),
            get_pane_direction_invocation(BIN, CFG, SOCK, 100, SplitDirection::Left).unwrap(),
            adjust_pane_size_invocation(BIN, CFG, SOCK, 100, SplitDirection::Down, 3).unwrap(),
            toggle_zoom_pane_invocation(BIN, CFG, SOCK, 100).unwrap(),
        ];
        for invocation in &invocations {
            assert_eq!(
                &invocation.argv[..6],
                [
                    BIN,
                    "--config-file",
                    CFG,
                    "cli",
                    "--no-auto-start",
                    "--prefer-mux"
                ]
            );
            assert_eq!(invocation.env_set, vec![(SOCKET_ENV.into(), SOCK.into())]);
            assert_eq!(invocation.env_remove, SCRUBBED_ENV.map(str::to_string));
        }
        assert_eq!(
            invocations[2].argv,
            [
                BIN,
                "--config-file",
                CFG,
                "cli",
                "--no-auto-start",
                "--prefer-mux",
                "get-pane-direction",
                "--pane-id",
                "100",
                "Left",
            ]
            .map(str::to_string)
        );
        assert_eq!(
            invocations[3].argv,
            [
                BIN,
                "--config-file",
                CFG,
                "cli",
                "--no-auto-start",
                "--prefer-mux",
                "adjust-pane-size",
                "--pane-id",
                "100",
                "--amount",
                "3",
                "Down",
            ]
            .map(str::to_string)
        );
        assert_eq!(
            invocations[4].argv,
            [
                BIN,
                "--config-file",
                CFG,
                "cli",
                "--no-auto-start",
                "--prefer-mux",
                "zoom-pane",
                "--pane-id",
                "100",
                "--toggle",
            ]
            .map(str::to_string)
        );
    }

    #[test]
    fn exact_group_activation_dispatches_through_provider_trait_and_rechecks_epoch() {
        let listing = action_canned(&[(1, 10, 100, "alpha", 80, 24, 0, 0, true, false)]);
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable, ProbeOutcome::Connectable],
            vec![ok(&listing), ok(""), ok(&listing)],
        );
        let concrete = provider(&runner);
        let provider: &dyn Provider = &concrete;
        let result = provider
            .activate_group_exact(&scope(Some(ServerEpoch(EPOCH))), &ProviderHandle::Wz(10))
            .unwrap();
        assert_eq!(
            result,
            GroupActivationResult {
                server_epoch: ServerEpoch(EPOCH),
                target: ProviderHandle::Wz(10),
            }
        );
        let calls = runner.run_calls.borrow();
        assert_eq!(
            calls[0].0,
            mux_cli_invocation(BIN, CFG, SOCK, &["list", "--format", "json"]).unwrap()
        );
        assert_eq!(
            calls[1].0,
            activate_tab_invocation(BIN, CFG, SOCK, 10).unwrap()
        );
        assert_eq!(calls[2].0, calls[0].0);
    }

    #[test]
    fn exact_direction_selects_only_the_returned_pane_and_reports_edge_as_none() {
        let pre = action_canned(&[
            (1, 10, 100, "alpha", 40, 24, 0, 0, true, false),
            (1, 10, 101, "alpha", 40, 24, 40, 0, false, false),
        ]);
        let post = action_canned(&[
            (1, 10, 100, "alpha", 40, 24, 0, 0, false, false),
            (1, 10, 101, "alpha", 40, 24, 40, 0, true, false),
        ]);
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable, ProbeOutcome::Connectable],
            vec![ok(&pre), ok(""), ok("101\n"), ok(""), ok(&post)],
        );
        let result = provider(&runner)
            .select_split_direction(
                &scope(Some(ServerEpoch(EPOCH))),
                &ProviderHandle::Wz(100),
                SplitDirection::Right,
            )
            .unwrap();
        assert_eq!(result.target, Some(ProviderHandle::Wz(101)));
        let calls = runner.run_calls.borrow();
        assert_eq!(
            calls[1].0,
            activate_pane_invocation(BIN, CFG, SOCK, 100).unwrap()
        );
        assert_eq!(
            calls[2].0,
            get_pane_direction_invocation(BIN, CFG, SOCK, 100, SplitDirection::Right).unwrap()
        );
        assert_eq!(
            calls[3].0,
            activate_pane_invocation(BIN, CFG, SOCK, 101).unwrap()
        );

        let edge = action_canned(&[(1, 10, 100, "alpha", 80, 24, 0, 0, true, false)]);
        let edge_runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable, ProbeOutcome::Connectable],
            vec![ok(&edge), ok(""), ok("\n"), ok(&edge)],
        );
        let result = provider(&edge_runner)
            .select_split_direction(
                &scope(Some(ServerEpoch(EPOCH))),
                &ProviderHandle::Wz(100),
                SplitDirection::Left,
            )
            .unwrap();
        assert_eq!(result.target, None, "edge is a typed no-op, never a guess");
        assert_eq!(
            edge_runner.run_calls.borrow().len(),
            4,
            "no target activation"
        );
    }

    #[test]
    fn exact_resize_and_zoom_verify_geometry_and_zoom_postconditions() {
        let resize_pre = action_canned(&[
            (1, 10, 100, "alpha", 40, 24, 0, 0, true, false),
            (1, 10, 101, "alpha", 40, 24, 40, 0, false, false),
        ]);
        let resize_post = action_canned(&[
            (1, 10, 100, "alpha", 43, 24, 0, 0, true, false),
            (1, 10, 101, "alpha", 37, 24, 43, 0, false, false),
        ]);
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable, ProbeOutcome::Connectable],
            vec![ok(&resize_pre), ok(""), ok(&resize_post)],
        );
        let resized = provider(&runner)
            .resize_split_exact(
                &scope(Some(ServerEpoch(EPOCH))),
                &ProviderHandle::Wz(100),
                SplitDirection::Right,
                3,
            )
            .unwrap();
        assert!(resized.changed);
        assert_eq!(
            runner.run_calls.borrow()[1].0,
            adjust_pane_size_invocation(BIN, CFG, SOCK, 100, SplitDirection::Right, 3).unwrap()
        );

        let zoom_pre = action_canned(&[(1, 10, 100, "alpha", 80, 24, 0, 0, true, false)]);
        let zoom_post = action_canned(&[(1, 10, 100, "alpha", 80, 24, 0, 0, true, true)]);
        let zoom_runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable, ProbeOutcome::Connectable],
            vec![ok(&zoom_pre), ok(""), ok(&zoom_post)],
        );
        let zoomed = provider(&zoom_runner)
            .toggle_split_zoom_exact(&scope(Some(ServerEpoch(EPOCH))), &ProviderHandle::Wz(100))
            .unwrap();
        assert!(zoomed.zoomed);
        assert_eq!(
            zoom_runner.run_calls.borrow()[1].0,
            toggle_zoom_pane_invocation(BIN, CFG, SOCK, 100).unwrap()
        );
    }

    #[test]
    fn owner_capability_absence_and_failed_zoom_postcondition_are_typed() {
        let pre = action_canned(&[(1, 10, 100, "alpha", 80, 24, 0, 0, true, false)]);
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&pre), fail("error: unrecognized subcommand 'zoom-pane'")],
        );
        match provider(&runner)
            .toggle_split_zoom_exact(&scope(Some(ServerEpoch(EPOCH))), &ProviderHandle::Wz(100))
        {
            Err(ProviderError::NativeFailure { detail }) => {
                assert!(
                    detail.starts_with("wez_owner_capability_missing:zoom-pane:"),
                    "{detail}"
                );
            }
            other => panic!("missing zoom primitive must be typed, got {other:?}"),
        }

        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable, ProbeOutcome::Connectable],
            vec![ok(&pre), ok(""), ok(&pre)],
        );
        match provider(&runner)
            .toggle_split_zoom_exact(&scope(Some(ServerEpoch(EPOCH))), &ProviderHandle::Wz(100))
        {
            Err(ProviderError::PostconditionFailed { detail }) => {
                assert!(detail.contains("did not flip"), "{detail}");
            }
            other => panic!("unchanged zoom state must fail postcondition, got {other:?}"),
        }
    }

    #[test]
    fn owner_actions_refuse_unpinned_child_ids_without_spawning() {
        let runner = ScriptedRunner::new(vec![], vec![]);
        match provider(&runner).resize_split_exact(
            &scope(None),
            &ProviderHandle::Wz(100),
            SplitDirection::Right,
            3,
        ) {
            Err(ProviderError::WrongInstance { detail }) => {
                assert!(detail.contains("managed scope"), "{detail}");
            }
            other => panic!("unpinned child action must fail closed, got {other:?}"),
        }
        assert!(runner.probe_calls.borrow().is_empty());
        assert!(runner.run_calls.borrow().is_empty());
    }

    /// The wez analogue of tmux's `create_on_unepoched_server_is_a_typed_error`
    /// (review finding #6, ADR 012 WS-A.6). Unpinned, `create` is refused
    /// before the endpoint is probed; pinned to an epoch the live sentinel
    /// does not serve (a replaced server), the one verifying list refuses
    /// `EpochChanged` and nothing is spawned.
    #[test]
    fn create_on_unpinned_scope_is_a_typed_error() {
        let runner = ScriptedRunner::new(vec![], vec![]);
        match provider(&runner).create(&scope(None), &spec("dmux:h:s")) {
            Err(ProviderError::WrongInstance { detail }) => {
                assert!(detail.contains("managed scope"), "{detail}");
            }
            other => panic!("unpinned create must be WrongInstance, got {other:?}"),
        }
        assert!(
            runner.probe_calls.borrow().is_empty(),
            "endpoint never probed"
        );
        assert!(runner.run_calls.borrow().is_empty(), "no native command");

        let runner = ScriptedRunner::new(vec![ProbeOutcome::Connectable], vec![ok(&canned(&[]))]);
        let claimed = ServerEpoch(Uuid::from_u128(0xfeed_face));
        match provider(&runner).create(&scope(Some(claimed)), &spec("dmux:h:s")) {
            Err(ProviderError::EpochChanged { expected, observed }) => {
                assert_eq!(expected, claimed);
                assert_eq!(observed, Some(ServerEpoch(EPOCH)));
            }
            other => panic!("replaced server must be EpochChanged, got {other:?}"),
        }
        assert_eq!(runner.run_calls.borrow().len(), 1, "list only, no spawn");
    }

    /// The nine verbs the review found opening with an unpinned scan
    /// (finding #6: `create`, `group_rename`, `group_remove`, `split_list`,
    /// `split_new`, `split_remove`, `normalize_plan`, `sole_window_id`,
    /// `cas_rename_workspace`). Each refuses an unmanaged scope typed with
    /// the endpoint never probed and no command run — the reads included,
    /// because a managed read without a pin is equally unverified (ADR 012
    /// WS-A.13).
    #[test]
    fn every_fenced_verb_refuses_an_unpinned_scope_before_any_command() {
        let runner = ScriptedRunner::new(vec![], vec![]);
        let p = provider(&runner).with_cas_binary("/fork/wezterm");
        let s = scope(None);
        let results: Vec<(&str, ProviderResult<()>)> = vec![
            ("create", p.create(&s, &spec("k")).map(|_| ())),
            (
                "group_rename",
                p.group_rename(&s, &ProviderHandle::Wz(10), "title"),
            ),
            ("group_remove", p.group_remove(&s, &ProviderHandle::Wz(10))),
            (
                "split_list",
                p.split_list(&s, &ProviderHandle::Wz(10)).map(|_| ()),
            ),
            (
                "split_new",
                p.split_new(&s, &ProviderHandle::Wz(10), &spec("k").into())
                    .map(|_| ()),
            ),
            ("split_remove", p.split_remove(&s, &ProviderHandle::Wz(100))),
            ("normalize_plan", p.normalize_plan(&s, "mw").map(|_| ())),
            ("sole_window_id", p.sole_window_id(&s, "alpha").map(|_| ())),
            (
                "cas_rename_workspace",
                p.cas_rename_workspace(&s, 1, "old", "new", true)
                    .map(|_| ()),
            ),
        ];
        for (verb, result) in results {
            match result {
                Err(ProviderError::WrongInstance { detail }) => {
                    assert!(detail.contains("managed scope"), "{verb}: {detail}");
                }
                other => panic!("{verb} on an unpinned scope must be WrongInstance, got {other:?}"),
            }
        }
        assert!(
            runner.probe_calls.borrow().is_empty(),
            "endpoint never probed"
        );
        assert!(runner.run_calls.borrow().is_empty(), "no native command");
    }

    #[test]
    fn create_spawns_once_and_returns_verified_binding() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable, ProbeOutcome::Connectable],
            vec![
                ok(&canned(&[])),
                ok("7\n"),
                ok(&canned(&[(3, 4, 7, "dmux:h:s")])),
            ],
        );
        let binding = provider(&runner)
            .create(&scope(Some(ServerEpoch(EPOCH))), &spec("dmux:h:s"))
            .expect("create");
        assert_eq!(binding.native_token, "dmux:h:s");
        assert_eq!(binding.server_epoch, ServerEpoch(EPOCH));
        assert_eq!(binding.root_group, ProviderHandle::Wz(4));
        assert_eq!(binding.root_split, ProviderHandle::Wz(7));
        let calls = runner.run_calls.borrow();
        assert_eq!(calls.len(), 3, "list, spawn, list");
        let want =
            spawn_workspace_invocation(BIN, CFG, SOCK, "dmux:h:s", None, &["/bin/true".into()])
                .unwrap();
        assert_eq!(calls[1].0, want, "spawn uses the frozen builder");
    }

    #[test]
    fn create_existing_key_is_typed_conflict_without_spawn() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&canned(&[(3, 4, 7, "dmux:h:s")]))],
        );
        match provider(&runner).create(&pinned(), &spec("dmux:h:s")) {
            Err(ProviderError::PostconditionFailed { detail }) => {
                assert!(detail.starts_with("workspace_exists"), "{detail}");
                assert!(detail.contains("never respawns"), "{detail}");
            }
            other => panic!("existing key must be typed conflict, got {other:?}"),
        }
        assert_eq!(
            runner.run_calls.borrow().len(),
            1,
            "keyed lookup only — a second spawn is forbidden"
        );
    }

    #[test]
    fn create_epoch_flip_after_spawn_is_epoch_changed() {
        let other = Uuid::from_u128(0xdead_beef);
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable, ProbeOutcome::Connectable],
            vec![
                ok(&canned(&[])),
                ok("7\n"),
                ok(&canned_epoch(other, &[(3, 4, 7, "dmux:h:s")])),
            ],
        );
        match provider(&runner).create(&pinned(), &spec("dmux:h:s")) {
            Err(ProviderError::EpochChanged { expected, observed }) => {
                assert_eq!(expected, ServerEpoch(EPOCH));
                assert_eq!(observed, Some(ServerEpoch(other)));
            }
            other => panic!("epoch flip must be EpochChanged, got {other:?}"),
        }
    }

    #[test]
    fn create_multi_window_result_is_typed() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable, ProbeOutcome::Connectable],
            vec![
                ok(&canned(&[])),
                ok("7\n"),
                ok(&canned(&[(3, 4, 7, "k"), (5, 6, 8, "k")])),
            ],
        );
        match provider(&runner).create(&pinned(), &spec("k")) {
            Err(ProviderError::MultiWindow {
                native_ref,
                window_count,
            }) => {
                assert_eq!(native_ref, "k");
                assert_eq!(window_count, 2);
            }
            other => panic!("multi-window create must be typed, got {other:?}"),
        }
    }

    #[test]
    fn create_unparseable_spawn_output_is_postcondition_failed() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&canned(&[])), ok("")],
        );
        match provider(&runner).create(&pinned(), &spec("k")) {
            Err(ProviderError::PostconditionFailed { detail }) => {
                assert!(detail.contains("unparseable spawn output"), "{detail}");
            }
            other => panic!("lost pane id must be indeterminate, got {other:?}"),
        }
    }

    #[test]
    fn group_new_verifies_parent_and_new_tab() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable, ProbeOutcome::Connectable],
            vec![
                ok(&canned(&[(1, 10, 100, "alpha")])),
                ok("9\n"),
                ok(&canned(&[(1, 10, 100, "alpha"), (1, 11, 9, "alpha")])),
            ],
        );
        let handle = provider(&runner)
            .group_new(&pinned(), &binding("alpha", EPOCH), &spec("alpha"))
            .expect("group_new");
        assert_eq!(handle, ProviderHandle::Wz(11));
        let calls = runner.run_calls.borrow();
        let want = spawn_group_invocation(BIN, CFG, SOCK, 1, None, &["/bin/true".into()]).unwrap();
        assert_eq!(calls[1].0, want, "spawn targets the sole window id");
    }

    #[test]
    fn group_new_multi_window_before_is_refused() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&canned(&[(1, 10, 100, "mw"), (2, 11, 101, "mw")]))],
        );
        match provider(&runner).group_new(&pinned(), &binding("mw", EPOCH), &spec("mw")) {
            Err(ProviderError::MultiWindow { window_count, .. }) => assert_eq!(window_count, 2),
            other => panic!("multi-window parent must refuse, got {other:?}"),
        }
        assert_eq!(runner.run_calls.borrow().len(), 1, "list only, no spawn");
    }

    #[test]
    fn split_new_lands_in_same_tab() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable, ProbeOutcome::Connectable],
            vec![
                ok(&canned(&[(1, 10, 100, "alpha")])),
                ok("12\n"),
                ok(&canned(&[(1, 10, 100, "alpha"), (1, 10, 12, "alpha")])),
            ],
        );
        let handle = provider(&runner)
            .split_new(&pinned(), &ProviderHandle::Wz(10), &spec("alpha").into())
            .expect("split_new");
        assert_eq!(handle, ProviderHandle::Wz(12));
        let calls = runner.run_calls.borrow();
        let want = split_pane_invocation(
            BIN,
            CFG,
            SOCK,
            100,
            SplitDirection::Down,
            None,
            None,
            &["/bin/true".into()],
        )
        .unwrap();
        assert_eq!(calls[1].0, want, "split anchors the tab's first pane");
    }

    #[test]
    fn split_new_wrong_tab_result_is_postcondition_failed() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable, ProbeOutcome::Connectable],
            vec![
                ok(&canned(&[(1, 10, 100, "alpha")])),
                ok("12\n"),
                ok(&canned(&[(1, 10, 100, "alpha"), (1, 11, 12, "alpha")])),
            ],
        );
        match provider(&runner).split_new(&pinned(), &ProviderHandle::Wz(10), &spec("alpha").into())
        {
            Err(ProviderError::PostconditionFailed { detail }) => {
                assert!(detail.contains("wanted tab 10"), "{detail}");
            }
            other => panic!("stray split must be typed, got {other:?}"),
        }
    }

    #[test]
    fn group_rename_verifies_title_in_relist() {
        let post = format!(
            r#"[{{"window_id":0,"tab_id":0,"pane_id":0,"workspace":"dmux:system:{EPOCH}"}},
                {{"window_id":1,"tab_id":10,"pane_id":100,"workspace":"alpha","tab_title":"editor"}}]"#
        );
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable, ProbeOutcome::Connectable],
            vec![ok(&canned(&[(1, 10, 100, "alpha")])), ok(""), ok(&post)],
        );
        provider(&runner)
            .group_rename(&pinned(), &ProviderHandle::Wz(10), "editor")
            .expect("group_rename");
        let calls = runner.run_calls.borrow();
        let want = set_tab_title_invocation(BIN, CFG, SOCK, 10, "editor").unwrap();
        assert_eq!(calls[1].0, want);
    }

    #[test]
    fn group_rename_title_mismatch_is_postcondition_failed() {
        let post = format!(
            r#"[{{"window_id":0,"tab_id":0,"pane_id":0,"workspace":"dmux:system:{EPOCH}"}},
                {{"window_id":1,"tab_id":10,"pane_id":100,"workspace":"alpha","tab_title":"other"}}]"#
        );
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable, ProbeOutcome::Connectable],
            vec![ok(&canned(&[(1, 10, 100, "alpha")])), ok(""), ok(&post)],
        );
        match provider(&runner).group_rename(&pinned(), &ProviderHandle::Wz(10), "editor") {
            Err(ProviderError::PostconditionFailed { detail }) => {
                assert!(detail.contains("wanted"), "{detail}");
            }
            other => panic!("title mismatch must be typed, got {other:?}"),
        }
    }

    #[test]
    fn remove_converges_with_final_verification() {
        let runner = ScriptedRunner::new(
            vec![
                ProbeOutcome::Connectable,
                ProbeOutcome::Connectable,
                ProbeOutcome::Connectable,
            ],
            vec![
                ok(&canned(&[(1, 10, 100, "alpha"), (1, 10, 101, "alpha")])),
                ok(""),
                ok(""),
                ok(&canned(&[])),
                ok(&canned(&[])),
            ],
        );
        provider(&runner)
            .remove(&pinned(), &binding("alpha", EPOCH))
            .expect("remove");
        let calls = runner.run_calls.borrow();
        assert_eq!(calls.len(), 5, "list, kill, kill, re-list, FINAL list");
        assert_eq!(
            calls[1].0,
            kill_pane_invocation(BIN, CFG, SOCK, 100).unwrap()
        );
        assert_eq!(
            calls[2].0,
            kill_pane_invocation(BIN, CFG, SOCK, 101).unwrap()
        );
    }

    #[test]
    fn remove_bound_hit_reports_survivors() {
        let mut probes = Vec::new();
        let mut runs = Vec::new();
        for round in 0..REMOVE_MAX_ROUNDS as u64 {
            probes.push(ProbeOutcome::Connectable);
            runs.push(ok(&canned(&[(1, 10, 100 + round, "alpha")])));
            runs.push(ok("")); // kill
        }
        let runner = ScriptedRunner::new(probes, runs);
        match provider(&runner).remove(&pinned(), &binding("alpha", EPOCH)) {
            Err(ProviderError::PostconditionFailed { detail }) => {
                assert!(detail.starts_with("remove_unconverged"), "{detail}");
                assert!(detail.contains("104"), "last survivors named: {detail}");
                assert!(detail.contains("never tombstone"), "{detail}");
            }
            other => panic!("bound hit must be typed partial, got {other:?}"),
        }
    }

    #[test]
    fn split_remove_last_pane_refused_without_kill() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&canned(&[(1, 10, 100, "alpha")]))],
        );
        match provider(&runner).split_remove(&pinned(), &ProviderHandle::Wz(100)) {
            Err(ProviderError::NativeFailure { detail }) => {
                assert!(detail.starts_with("refused_last_pane"), "{detail}");
            }
            other => panic!("last pane must refuse, got {other:?}"),
        }
        assert_eq!(runner.run_calls.borrow().len(), 1, "list only, no kill");
    }

    #[test]
    fn group_remove_last_group_refused_without_kill() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&canned(&[(1, 10, 100, "alpha"), (1, 10, 101, "alpha")]))],
        );
        match provider(&runner).group_remove(&pinned(), &ProviderHandle::Wz(10)) {
            Err(ProviderError::NativeFailure { detail }) => {
                assert!(detail.starts_with("refused_last_group"), "{detail}");
            }
            other => panic!("last group must refuse, got {other:?}"),
        }
        assert_eq!(runner.run_calls.borrow().len(), 1, "list only, no kill");
    }

    #[test]
    fn split_remove_kills_exactly_and_verifies_absence() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable, ProbeOutcome::Connectable],
            vec![
                ok(&canned(&[(1, 10, 100, "alpha"), (1, 10, 101, "alpha")])),
                fail("Error: no such pane"), // already-dead race is benign
                ok(&canned(&[(1, 10, 100, "alpha")])),
            ],
        );
        provider(&runner)
            .split_remove(&pinned(), &ProviderHandle::Wz(101))
            .expect("split_remove");
        let calls = runner.run_calls.borrow();
        assert_eq!(
            calls[1].0,
            kill_pane_invocation(BIN, CFG, SOCK, 101).unwrap()
        );
    }

    // -- normalization (plan §10.3, P8a) -------------------------------------

    /// The confirmed plan the scripted apply tests execute: workspace `mw`
    /// merging window 2's two panes into target window 1.
    fn mw_plan() -> NormalizePlan {
        NormalizePlan {
            native_token: "mw".into(),
            server_epoch: ServerEpoch(EPOCH),
            target_window: 1,
            moves: vec![
                NormalizeMove {
                    pane_id: 200,
                    from_window: 2,
                },
                NormalizeMove {
                    pane_id: 201,
                    from_window: 2,
                },
            ],
        }
    }

    #[test]
    fn move_pane_invocation_argv_is_exact() {
        let inv = move_pane_invocation(BIN, CFG, SOCK, 42, 7).expect("move pane");
        let mut want = cli_prefix();
        want.extend(
            [
                "move-pane-to-new-tab",
                "--pane-id",
                "42",
                "--window-id",
                "7",
            ]
            .map(String::from),
        );
        assert_eq!(inv.argv, want);
        assert_eq!(
            inv.env_set,
            vec![(SOCKET_ENV.to_string(), SOCK.to_string())]
        );
        assert_eq!(inv.env_remove, vec!["WEZTERM_PANE", "TMUX", "TMUX_PANE"]);
        let err = move_pane_invocation(BIN, CFG, "", 42, 7).unwrap_err();
        assert!(err.contains("empty WEZTERM_UNIX_SOCKET"), "{err}");
    }

    #[test]
    fn normalize_plan_is_deterministic_lowest_window_ascending_moves() {
        // Windows deliberately listed out of order: the target is the
        // LOWEST window id and moves come in ascending (window_id, pane_id)
        // order regardless of list order; other workspaces are ignored.
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&canned(&[
                (9, 90, 901, "mw"),
                (2, 20, 200, "mw"),
                (9, 90, 900, "mw"),
                (2, 21, 201, "mw"),
                (5, 50, 500, "mw"),
                (1, 10, 100, "other"),
            ]))],
        );
        let plan = provider(&runner)
            .normalize_plan(&scope(Some(ServerEpoch(EPOCH))), "mw")
            .expect("plan");
        assert_eq!(plan.native_token, "mw");
        assert_eq!(plan.server_epoch, ServerEpoch(EPOCH));
        assert_eq!(plan.target_window, 2, "lowest window id wins");
        assert_eq!(
            plan.moves,
            vec![
                NormalizeMove {
                    pane_id: 500,
                    from_window: 5
                },
                NormalizeMove {
                    pane_id: 900,
                    from_window: 9
                },
                NormalizeMove {
                    pane_id: 901,
                    from_window: 9
                },
            ]
        );
        assert_eq!(
            runner.run_calls.borrow().len(),
            1,
            "one verified list; planning is strictly read-only"
        );
    }

    #[test]
    fn normalize_plan_sole_window_is_empty_and_absent_is_not_found() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable, ProbeOutcome::Connectable],
            vec![
                ok(&canned(&[(3, 30, 300, "solo"), (3, 31, 301, "solo")])),
                ok(&canned(&[(3, 30, 300, "solo")])),
            ],
        );
        let p = provider(&runner);
        let plan = p.normalize_plan(&pinned(), "solo").expect("plan");
        assert_eq!(plan.target_window, 3);
        assert!(plan.moves.is_empty(), "sole window: nothing to do");
        match p.normalize_plan(&pinned(), "missing") {
            Err(ProviderError::NotFound { native_ref }) => assert_eq!(native_ref, "missing"),
            other => panic!("zero rows must be NotFound, got {other:?}"),
        }
    }

    #[test]
    fn normalize_apply_moves_planned_panes_and_converges() {
        let pre = canned(&[(1, 10, 100, "mw"), (2, 20, 200, "mw"), (2, 21, 201, "mw")]);
        let post = canned(&[(1, 10, 100, "mw"), (1, 30, 200, "mw"), (1, 31, 201, "mw")]);
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable, ProbeOutcome::Connectable],
            vec![ok(&pre), ok(""), ok(""), ok(&post)],
        );
        provider(&runner)
            .normalize_apply(&scope(Some(ServerEpoch(EPOCH))), &mw_plan())
            .expect("normalize_apply");
        let calls = runner.run_calls.borrow();
        assert_eq!(calls.len(), 4, "gate list, move, move, verifying re-list");
        assert_eq!(
            calls[1].0,
            move_pane_invocation(BIN, CFG, SOCK, 200, 1).unwrap()
        );
        assert_eq!(
            calls[2].0,
            move_pane_invocation(BIN, CFG, SOCK, 201, 1).unwrap()
        );
    }

    #[test]
    fn normalize_apply_epoch_flip_is_epoch_changed_without_mutation() {
        let other = Uuid::from_u128(0xdead_beef);
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&canned_epoch(
                other,
                &[(1, 10, 100, "mw"), (2, 20, 200, "mw")],
            ))],
        );
        match provider(&runner).normalize_apply(&pinned(), &mw_plan()) {
            Err(ProviderError::EpochChanged { expected, observed }) => {
                assert_eq!(expected, ServerEpoch(EPOCH));
                assert_eq!(observed, Some(ServerEpoch(other)));
            }
            other => panic!("epoch flip must be EpochChanged, got {other:?}"),
        }
        assert_eq!(runner.run_calls.borrow().len(), 1, "list only, zero moves");
    }

    #[test]
    fn normalize_apply_drift_is_refused_without_mutation() {
        // A pane spawned since the plan was confirmed.
        let drifted = canned(&[
            (1, 10, 100, "mw"),
            (2, 20, 200, "mw"),
            (2, 21, 201, "mw"),
            (7, 70, 700, "mw"),
        ]);
        let runner = ScriptedRunner::new(vec![ProbeOutcome::Connectable], vec![ok(&drifted)]);
        match provider(&runner).normalize_apply(&pinned(), &mw_plan()) {
            Err(ProviderError::PostconditionFailed { detail }) => {
                assert!(detail.starts_with("normalize_drift:"), "{detail}");
                assert!(detail.contains("re-plan"), "{detail}");
            }
            other => panic!("new pane must refuse as drift, got {other:?}"),
        }
        assert_eq!(runner.run_calls.borrow().len(), 1, "list only, zero moves");

        // A planned pane vanished.
        let vanished = canned(&[(1, 10, 100, "mw"), (2, 20, 200, "mw")]);
        let runner = ScriptedRunner::new(vec![ProbeOutcome::Connectable], vec![ok(&vanished)]);
        match provider(&runner).normalize_apply(&pinned(), &mw_plan()) {
            Err(ProviderError::PostconditionFailed { detail }) => {
                assert!(detail.starts_with("normalize_drift:"), "{detail}");
            }
            other => panic!("vanished pane must refuse as drift, got {other:?}"),
        }
        assert_eq!(runner.run_calls.borrow().len(), 1);

        // The whole workspace vanished between plan and apply.
        let gone = canned(&[(1, 10, 100, "other")]);
        let runner = ScriptedRunner::new(vec![ProbeOutcome::Connectable], vec![ok(&gone)]);
        match provider(&runner).normalize_apply(&pinned(), &mw_plan()) {
            Err(ProviderError::PostconditionFailed { detail }) => {
                assert!(detail.starts_with("normalize_drift:"), "{detail}");
                assert!(detail.contains("zero live panes"), "{detail}");
            }
            other => panic!("vanished workspace must refuse as drift, got {other:?}"),
        }
        assert_eq!(runner.run_calls.borrow().len(), 1);
    }

    #[test]
    fn normalize_apply_sole_window_plan_is_verified_noop() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&canned(&[(1, 10, 100, "solo"), (1, 11, 101, "solo")]))],
        );
        let plan = NormalizePlan {
            native_token: "solo".into(),
            server_epoch: ServerEpoch(EPOCH),
            target_window: 1,
            moves: vec![],
        };
        provider(&runner)
            .normalize_apply(&pinned(), &plan)
            .expect("empty plan is a verified no-op success");
        assert_eq!(
            runner.run_calls.borrow().len(),
            1,
            "one verifying list, zero mutation"
        );
    }

    #[test]
    fn normalize_apply_never_touches_unplanned_panes_and_reports_unconverged() {
        // The gate passes against the exact planned tree; an interloper
        // window appears mid-apply. Rounds must never move pane 700 (it is
        // outside the plan) and the bound reports unconverged typed.
        let pre = canned(&[(1, 10, 100, "mw"), (2, 20, 200, "mw"), (2, 21, 201, "mw")]);
        let stuck = canned(&[
            (1, 10, 100, "mw"),
            (1, 30, 200, "mw"),
            (1, 31, 201, "mw"),
            (7, 70, 700, "mw"),
        ]);
        let mut probes = vec![ProbeOutcome::Connectable];
        let mut runs = vec![ok(&pre), ok(""), ok("")];
        for _ in 0..REMOVE_MAX_ROUNDS {
            probes.push(ProbeOutcome::Connectable);
            runs.push(ok(&stuck));
        }
        let runner = ScriptedRunner::new(probes, runs);
        match provider(&runner).normalize_apply(&pinned(), &mw_plan()) {
            Err(ProviderError::PostconditionFailed { detail }) => {
                assert!(detail.starts_with("normalize_unconverged:"), "{detail}");
                assert!(detail.contains("window(s) [1, 7]"), "{detail}");
                assert!(detail.contains("never half-managed"), "{detail}");
            }
            other => panic!("interloper must leave unconverged typed, got {other:?}"),
        }
        let calls = runner.run_calls.borrow();
        // gate list, move 200, move 201, then bare re-lists every round:
        // the planned panes already sit in the target, so nothing is
        // pending and the unplanned pane is never moved.
        assert_eq!(calls.len(), 3 + REMOVE_MAX_ROUNDS);
        for (invocation, _) in calls.iter().skip(3) {
            assert!(
                invocation.argv.contains(&"list".to_string()),
                "no move for the unplanned pane: {:?}",
                invocation.argv
            );
        }
    }

    #[test]
    fn normalize_apply_bound_hit_reissues_only_the_pending_planned_move() {
        // Pane 201 never actually arrives (server-side race): each round
        // re-issues exactly that planned move until the bound trips typed.
        let pre = canned(&[(1, 10, 100, "mw"), (2, 20, 200, "mw"), (2, 21, 201, "mw")]);
        let half = canned(&[(1, 10, 100, "mw"), (1, 30, 200, "mw"), (2, 21, 201, "mw")]);
        let mut probes = vec![ProbeOutcome::Connectable];
        let mut runs = vec![ok(&pre), ok(""), ok("")];
        for _ in 0..REMOVE_MAX_ROUNDS {
            probes.push(ProbeOutcome::Connectable);
            runs.push(ok(&half));
            runs.push(ok("")); // re-issued move for pane 201
        }
        let runner = ScriptedRunner::new(probes, runs);
        match provider(&runner).normalize_apply(&pinned(), &mw_plan()) {
            Err(ProviderError::PostconditionFailed { detail }) => {
                assert!(detail.starts_with("normalize_unconverged:"), "{detail}");
                assert!(detail.contains("target 1"), "{detail}");
            }
            other => panic!("bound hit must be typed partial, got {other:?}"),
        }
        let calls = runner.run_calls.borrow();
        let want = move_pane_invocation(BIN, CFG, SOCK, 201, 1).unwrap();
        let reissues = calls.iter().filter(|(inv, _)| *inv == want).count();
        assert_eq!(
            reissues,
            1 + REMOVE_MAX_ROUNDS,
            "initial move plus one re-issue per round for the pending pane"
        );
        let moved = move_pane_invocation(BIN, CFG, SOCK, 200, 1).unwrap();
        assert_eq!(
            calls.iter().filter(|(inv, _)| *inv == moved).count(),
            1,
            "an already-arrived pane is never re-moved"
        );
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

        let inv = split_pane_invocation(
            BIN,
            CFG,
            SOCK,
            7,
            SplitDirection::Right,
            Some(40),
            Some("/work"),
            &boot,
        )
        .expect("split");
        let mut want = cli_prefix();
        want.extend(
            [
                "split-pane",
                "--pane-id",
                "7",
                "--right",
                "--percent",
                "40",
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
        let err = split_pane_invocation(BIN, CFG, SOCK, 1, SplitDirection::Down, None, None, &[])
            .unwrap_err();
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

    // Live-captured fork/stock stderr lines (pinned fork build, 2026-08-16).
    const LIVE_MISMATCH: &str = "15:50:23.031  ERROR  wezterm > rename-workspace-if failed: \
                                 workspace_mismatch window_id=1 actual=\"new\"; terminating";
    const LIVE_NO_WINDOW: &str = "15:50:23.087  ERROR  wezterm > rename-workspace-if failed: \
                                  no_such_window window_id=18446744073709551615; terminating";
    const LIVE_NOT_SOLE: &str = "15:50:23.193  ERROR  wezterm > rename-workspace-if failed: \
                                 not_sole_window window_id=1 other_window_ids=[2]; terminating";
    const LIVE_INVALID_PDU: &str = "15:50:44.993  ERROR  wezterm > unexpected response Ok(ErrorResponse(ErrorResponse { \
         reason: \"Error: invalid PDU Invalid { ident: 63 }\" })); terminating";
    const LIVE_STOCK_CLAP: &str = "error: unexpected argument '--window-id' found";

    #[test]
    fn cas_classifier_matches_live_fork_stderr() {
        assert_eq!(
            classify_cas_rename(true, ""),
            CasClassification::Outcome(CasRenameOutcome::Renamed)
        );
        assert_eq!(
            classify_cas_rename(false, LIVE_MISMATCH),
            CasClassification::Outcome(CasRenameOutcome::WorkspaceMismatch {
                actual: "new".into()
            })
        );
        assert_eq!(
            classify_cas_rename(false, LIVE_NO_WINDOW),
            CasClassification::Outcome(CasRenameOutcome::NoSuchWindow)
        );
        assert_eq!(
            classify_cas_rename(false, LIVE_NOT_SOLE),
            CasClassification::Outcome(CasRenameOutcome::NotSoleWindow)
        );
        assert_eq!(
            classify_cas_rename(false, LIVE_INVALID_PDU),
            CasClassification::CapabilityMissing
        );
        for unclassified in [LIVE_STOCK_CLAP, "failed to connect", ""] {
            match classify_cas_rename(false, unclassified) {
                CasClassification::Unclassified(_) => {}
                other => panic!("{unclassified:?} must stay unclassified, got {other:?}"),
            }
        }
        // A mismatch whose actual cannot be parsed is never guessed.
        match classify_cas_rename(false, "rename-workspace-if failed: workspace_mismatch huh") {
            CasClassification::Unclassified(detail) => {
                assert!(detail.contains("without parseable actual"), "{detail}");
            }
            other => panic!("unparseable actual must stay unclassified, got {other:?}"),
        }
    }

    #[test]
    fn cas_rename_invocation_argv_is_exact() {
        let inv = cas_rename_invocation(BIN, CFG, SOCK, 7, "old", true, "new").unwrap();
        let mut want = cli_prefix();
        want.extend(
            [
                "rename-workspace",
                "--window-id",
                "7",
                "--if-workspace",
                "old",
                "--if-sole-window",
                "new",
            ]
            .map(String::from),
        );
        assert_eq!(inv.argv, want);
        let inv = cas_rename_invocation(BIN, CFG, SOCK, 7, "old", false, "new").unwrap();
        assert!(!inv.argv.contains(&"--if-sole-window".to_string()));
        assert!(cas_rename_invocation(BIN, CFG, SOCK, 7, "", false, "new").is_err());
        assert!(cas_rename_invocation(BIN, CFG, SOCK, 7, "old", false, "").is_err());
    }

    #[test]
    fn probe_cas_rename_classifies_positive_negative_and_indeterminate() {
        // no_such_window against u64::MAX ⇒ capable.
        let runner =
            ScriptedRunner::new(vec![ProbeOutcome::Connectable], vec![fail(LIVE_NO_WINDOW)]);
        assert!(provider(&runner).probe_cas_rename(&scope(None)).unwrap());
        let calls = runner.run_calls.borrow();
        let want = cas_rename_invocation(
            BIN,
            CFG,
            SOCK,
            u64::MAX,
            "dmux:probe:cas",
            false,
            "dmux:probe:cas",
        )
        .unwrap();
        assert_eq!(calls[0].0, want, "probe targets the impossible window id");
        drop(calls);

        // invalid PDU ⇒ not capable.
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![fail(LIVE_INVALID_PDU)],
        );
        assert!(!provider(&runner).probe_cas_rename(&scope(None)).unwrap());

        // Anything else ⇒ typed error, never a guess.
        let runner =
            ScriptedRunner::new(vec![ProbeOutcome::Connectable], vec![fail(LIVE_STOCK_CLAP)]);
        match provider(&runner).probe_cas_rename(&scope(None)) {
            Err(ProviderError::NativeFailure { detail }) => {
                assert!(detail.starts_with("cas_probe_indeterminate"), "{detail}");
            }
            other => panic!("indeterminate probe must be typed, got {other:?}"),
        }
    }

    #[test]
    fn cas_rename_workspace_uses_cas_binary_and_verifies_renamed() {
        let fork = "/fork/wezterm";
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable, ProbeOutcome::Connectable],
            vec![
                ok(&canned(&[(1, 10, 100, "old")])),
                ok(""),
                ok(&canned(&[(1, 10, 100, "new")])),
            ],
        );
        let outcome = provider(&runner)
            .with_cas_binary(fork)
            .cas_rename_workspace(&pinned(), 1, "old", "new", true)
            .expect("cas rename");
        assert_eq!(outcome, CasRenameOutcome::Renamed);
        let calls = runner.run_calls.borrow();
        assert_eq!(
            calls[0].0.argv[0], BIN,
            "reads keep using the stock/read binary"
        );
        let want = cas_rename_invocation(fork, CFG, SOCK, 1, "old", true, "new").unwrap();
        assert_eq!(calls[1].0, want, "CAS goes through the fork CLI");
    }

    #[test]
    fn cas_rename_renamed_without_relist_proof_is_postcondition_failed() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable, ProbeOutcome::Connectable],
            vec![
                ok(&canned(&[(1, 10, 100, "old")])),
                ok(""),
                ok(&canned(&[(1, 10, 100, "old")])),
            ],
        );
        match provider(&runner).cas_rename_workspace(&pinned(), 1, "old", "new", false) {
            Err(ProviderError::PostconditionFailed { detail }) => {
                assert!(detail.contains("Renamed but window"), "{detail}");
            }
            other => panic!("unproven rename must be typed, got {other:?}"),
        }
    }

    #[test]
    fn cas_rename_mismatch_is_typed_outcome_and_capability_missing_is_typed_error() {
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable, ProbeOutcome::Connectable],
            vec![
                ok(&canned(&[(1, 10, 100, "new")])),
                fail(LIVE_MISMATCH),
                ok(&canned(&[(1, 10, 100, "new")])),
            ],
        );
        let outcome = provider(&runner)
            .cas_rename_workspace(&pinned(), 1, "old", "other", false)
            .expect("typed outcome");
        assert_eq!(
            outcome,
            CasRenameOutcome::WorkspaceMismatch {
                actual: "new".into()
            }
        );

        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&canned(&[(1, 10, 100, "old")])), fail(LIVE_INVALID_PDU)],
        );
        match provider(&runner).cas_rename_workspace(&pinned(), 1, "old", "new", false) {
            Err(ProviderError::NativeFailure { detail }) => {
                assert!(detail.starts_with("cas_capability_missing"), "{detail}");
            }
            other => panic!("stock server must be typed capability-missing, got {other:?}"),
        }
    }

    #[test]
    fn sole_window_id_answers_present_absent_and_multi_window() {
        // Present with one window: the id, from a sentinel-verified scan.
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&canned(&[(3, 10, 100, "alpha"), (3, 11, 101, "alpha")]))],
        );
        assert_eq!(
            provider(&runner)
                .sole_window_id(&scope(Some(ServerEpoch(EPOCH))), "alpha")
                .expect("sole window"),
            3
        );

        // Absent workspace: typed NotFound.
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&canned(&[(3, 10, 100, "alpha")]))],
        );
        match provider(&runner).sole_window_id(&pinned(), "missing") {
            Err(ProviderError::NotFound { native_ref }) => assert_eq!(native_ref, "missing"),
            other => panic!("absent workspace must be NotFound, got {other:?}"),
        }

        // Multi-window: typed MultiWindow with the exact count.
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&canned(&[
                (1, 10, 100, "mw"),
                (2, 11, 101, "mw"),
                (4, 12, 102, "mw"),
            ]))],
        );
        match provider(&runner).sole_window_id(&pinned(), "mw") {
            Err(ProviderError::MultiWindow {
                native_ref,
                window_count,
            }) => {
                assert_eq!(native_ref, "mw");
                assert_eq!(window_count, 3);
            }
            other => panic!("multi-window must be typed, got {other:?}"),
        }

        // The scan is verified: a wrong pinned epoch rejects typed.
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&canned(&[(3, 10, 100, "alpha")]))],
        );
        let wrong = ServerEpoch(Uuid::from_u128(0xdead_beef));
        match provider(&runner).sole_window_id(&scope(Some(wrong)), "alpha") {
            Err(ProviderError::EpochChanged { .. }) => {}
            other => panic!("wrong epoch must reject typed, got {other:?}"),
        }
    }

    // -- WS-E.2: reproductions against a replacement server ------------------
    //
    // Review report 08 §7 had call-chain proof only for `cas_rename_workspace`
    // (wez.rs:2806 at 493e92c) and `sole_window_id` (:2744). The runner plays
    // a server whose sentinel epoch is not the pin — what a replaced
    // `wezterm-mux-server` on the managed socket looks like to the adapter
    // (plan §15.1, ADR 002). The live counterpart, with a real second server
    // behind the published path, is `provider_wez::
    // replacement_server_refuses_cas_rename_and_sole_window_id_typed`.

    /// Finding #6 inverted for `cas_rename_workspace`: unpinned, the CAS verb
    /// never reaches the endpoint; pinned against a replacement, the one
    /// verifying list refuses `EpochChanged` and no `rename-workspace` is
    /// recorded.
    #[test]
    fn cas_rename_on_a_replacement_server_refuses_epoch_changed_without_renaming() {
        let runner = ScriptedRunner::new(vec![], vec![]);
        match provider(&runner)
            .with_cas_binary("/fork/wezterm")
            .cas_rename_workspace(&scope(None), 1, "old", "new", true)
        {
            Err(ProviderError::WrongInstance { detail }) => {
                assert!(detail.contains("managed scope"), "{detail}");
            }
            other => panic!("unpinned CAS must be WrongInstance, got {other:?}"),
        }
        assert!(
            runner.probe_calls.borrow().is_empty(),
            "endpoint never probed"
        );
        assert!(runner.run_calls.borrow().is_empty(), "no native command");

        let replacement = Uuid::from_u128(0x9e9e_9e9e_9e9e_9e9e);
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&canned_epoch(replacement, &[(1, 10, 100, "old")]))],
        );
        match provider(&runner)
            .with_cas_binary("/fork/wezterm")
            .cas_rename_workspace(&pinned(), 1, "old", "new", true)
        {
            Err(ProviderError::EpochChanged { expected, observed }) => {
                assert_eq!(expected, ServerEpoch(EPOCH));
                assert_eq!(observed, Some(ServerEpoch(replacement)));
            }
            other => panic!("replacement server must be EpochChanged, got {other:?}"),
        }
        let calls = runner.run_calls.borrow();
        assert_eq!(calls.len(), 1, "the verifying list only");
        let argvs: Vec<&Vec<String>> = calls.iter().map(|(inv, _)| &inv.argv).collect();
        assert!(
            argvs
                .iter()
                .all(|argv| !argv.iter().any(|a| a == "rename-workspace")),
            "no rename-workspace reached the replacement: {argvs:?}"
        );
    }

    /// Finding #6 inverted for `sole_window_id`: unpinned, no probe and no
    /// list; pinned against a replacement, `EpochChanged` after the one
    /// verifying list and no window id is ever answered.
    #[test]
    fn sole_window_id_on_a_replacement_server_refuses_epoch_changed() {
        let runner = ScriptedRunner::new(vec![], vec![]);
        match provider(&runner).sole_window_id(&scope(None), "old") {
            Err(ProviderError::WrongInstance { detail }) => {
                assert!(detail.contains("managed scope"), "{detail}");
            }
            other => panic!("unpinned lookup must be WrongInstance, got {other:?}"),
        }
        assert!(
            runner.probe_calls.borrow().is_empty(),
            "endpoint never probed"
        );
        assert!(runner.run_calls.borrow().is_empty(), "no native command");

        let replacement = Uuid::from_u128(0x9e9e_9e9e_9e9e_9e9e);
        let runner = ScriptedRunner::new(
            vec![ProbeOutcome::Connectable],
            vec![ok(&canned_epoch(replacement, &[(1, 10, 100, "old")]))],
        );
        match provider(&runner).sole_window_id(&pinned(), "old") {
            Err(ProviderError::EpochChanged { expected, observed }) => {
                assert_eq!(expected, ServerEpoch(EPOCH));
                assert_eq!(observed, Some(ServerEpoch(replacement)));
            }
            other => panic!("replacement server must be EpochChanged, got {other:?}"),
        }
        assert_eq!(
            runner.run_calls.borrow().len(),
            1,
            "the verifying list only"
        );
    }

    /// The frozen fork CAS failure shapes (ADR 006; live-captured stderr),
    /// driven through `cas_rename_workspace` under a pinned scope rather than
    /// through the bare classifier: each non-`Renamed` outcome is the typed
    /// result ADR 006 names, the CAS argv is the frozen one, and the
    /// same-epoch post-scan is still taken.
    #[test]
    fn cas_rename_failure_shapes_are_typed_outcomes_under_a_pinned_scope() {
        let cases = [
            (LIVE_NO_WINDOW, CasRenameOutcome::NoSuchWindow),
            (LIVE_NOT_SOLE, CasRenameOutcome::NotSoleWindow),
            (
                LIVE_MISMATCH,
                CasRenameOutcome::WorkspaceMismatch {
                    actual: "new".into(),
                },
            ),
        ];
        for (stderr, want) in cases {
            let runner = ScriptedRunner::new(
                vec![ProbeOutcome::Connectable, ProbeOutcome::Connectable],
                vec![
                    ok(&canned(&[(1, 10, 100, "old")])),
                    fail(stderr),
                    ok(&canned(&[(1, 10, 100, "old")])),
                ],
            );
            let outcome = provider(&runner)
                .with_cas_binary("/fork/wezterm")
                .cas_rename_workspace(&pinned(), 1, "old", "new", true)
                .unwrap_or_else(|e| panic!("{stderr:?} must be a typed outcome, got {e:?}"));
            assert_eq!(outcome, want, "{stderr:?}");
            let calls = runner.run_calls.borrow();
            assert_eq!(calls.len(), 3, "list, CAS, verifying list");
            assert_eq!(
                calls[1].0,
                cas_rename_invocation("/fork/wezterm", CFG, SOCK, 1, "old", true, "new").unwrap()
            );
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
