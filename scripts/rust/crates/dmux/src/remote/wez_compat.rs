//! Remote-Wez compatibility and presentation capability gate (plan
//! §§12.3, 13, 17).
//!
//! Protocol v1 deliberately keeps the frozen envelope/`HelloInfo` shapes.
//! WezTerm compatibility is therefore carried by additive strings in the
//! existing `capabilities` vectors:
//!
//! - `wez:build:<exact wezterm_version()>`
//! - `wez:path:<canonical absolute owner executable>`
//! - `wez:socket:<owner's managed mux socket>`
//! - [`CAP_ATTACH_NO_CREATE`] (P0 `attach_no_create.json`)
//! - [`CAP_ACTIVATE_EXISTING`] (P0 `activate_existing.json`)
//!
//! A report is positive only after bounded, non-mutating `wezterm --version`
//! and `wezterm start --help` argv probes.  Neither invocation selects or
//! starts a mux server.  The controller runs the same probe for its local
//! build, then [`assess_automatic_remote_wez`] requires an exact build match
//! and every presentation token.  Until a compatibility matrix replaces
//! this rule, missing/legacy/malformed/mismatched reports are terminal for
//! automatic Wez selection; they never imply an automatic tmux fallback.

use std::fmt;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::childio::{BoundedCapture, bounded_read, join_capture, kill_process_group};
use crate::error::{ErrorCode, TypedError};

/// Existing coarse backend capability, retained for route compatibility.
pub const CAP_WEZ: &str = "wez";
/// Existing coarse tmux capability.  A refused Wez assessment reports
/// whether this remains available for an *explicit* `--backend tmux`.
pub const CAP_TMUX: &str = "tmux";

/// Prefix for the exact opaque build returned by `wezterm --version`.
pub const WEZ_BUILD_PREFIX: &str = "wez:build:";
/// Canonical absolute executable path used by the owner probe. Wez ssh
/// domains consume this as `remote_wezterm_path`; controllers must never
/// infer an owner OS path.
pub const WEZ_PATH_PREFIX: &str = "wez:path:";
/// Prefix for the owner's managed mux socket.  The controller pins this
/// into every managed ssh domain's `override_proxy_command`; left to
/// itself, the first WezTerm connect runs a bare `wezterm cli --prefer-mux
/// proxy`, which falls through to default socket discovery and auto-starts
/// a second, unmanaged owner-side server (plan §2 decision 16, §15.1).
pub const WEZ_SOCKET_PREFIX: &str = "wez:socket:";

/// The service publishes exactly one socket name beneath the runtime dir;
/// see [`crate::runtime::WEZ_SOCKET_FILE`] and the descriptor validator in
/// `wez/domains/init.lua`, which enforces the same tail.
const MANAGED_SOCKET_SUFFIX: &str = "/dmux/wez-dmux.sock";

/// `sun_path` is 104 bytes including the NUL on macOS, so nothing longer
/// was ever bound.
const MAX_SOCKET_LEN: usize = 103;

/// P0 `attach_no_create.json`: `start --always-new-process --domain D
/// --attach`, guarded by the sentinel precondition, attaches without
/// creating an owner pane/workspace.
pub const CAP_ATTACH_NO_CREATE: &str = "wez:presentation:attach_no_create:v1";
/// P0 `activate_existing.json`: the acknowledged GUI bridge uses pcall'd
/// `wezterm.mux.set_active_workspace`, never create-on-miss
/// `SwitchToWorkspace`.
pub const CAP_ACTIVATE_EXISTING: &str = "wez:presentation:activate_existing:v1";

/// Capabilities every controller and owner must positively report before
/// automatic remote-Wez selection.  Build equality is checked separately.
pub const REQUIRED_PRESENTATION_CAPABILITIES: [&str; 2] =
    [CAP_ATTACH_NO_CREATE, CAP_ACTIVATE_EXISTING];

/// Owner hello probes have a short independent bound.  This is not the mux
/// provider's socket deadline: neither probe is allowed to contact a server.
pub const DEFAULT_PROBE_DEADLINE: Duration = Duration::from_secs(2);

const MAX_PROBE_OUTPUT: usize = 16 * 1024;
const MAX_BUILD_LEN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WezProbeError {
    Spawn(String),
    Timeout,
    Failed {
        argv: String,
        status: i32,
        detail: String,
    },
    InvalidVersion(String),
    InvalidExecutable(String),
    MissingAttachSurface(String),
}

impl fmt::Display for WezProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WezProbeError::Spawn(detail) => write!(f, "spawn: {detail}"),
            WezProbeError::Timeout => f.write_str("probe deadline elapsed"),
            WezProbeError::Failed {
                argv,
                status,
                detail,
            } => write!(f, "{argv} exited {status}: {detail}"),
            WezProbeError::InvalidVersion(detail) => {
                write!(f, "invalid `wezterm --version` output: {detail}")
            }
            WezProbeError::InvalidExecutable(detail) => {
                write!(f, "invalid WezTerm executable: {detail}")
            }
            WezProbeError::MissingAttachSurface(detail) => {
                write!(f, "required attach-only argv surface is absent: {detail}")
            }
        }
    }
}

/// The local positive report used both by the owner hello and by the GUI
/// controller before comparing itself with that hello.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WezCapabilityReport {
    pub build: String,
    pub wezterm_path: String,
    pub capabilities: Vec<String>,
}

/// Probe one installed WezTerm without loading config, selecting an
/// endpoint, or starting a server.  Argv is fixed and shell-free:
///
/// ```text
/// <wezterm> --version
/// <wezterm> --skip-config start --help
/// ```
///
/// The second command proves the installed executable exposes the exact P0
/// attach-only flags.  `activate_existing` is a dmux bridge protocol
/// capability tied to this exact build; its runtime heartbeat is separately
/// preflighted by P9 before presentation.
pub fn probe_wezterm_capabilities(
    wezterm_bin: &str,
    deadline: Duration,
) -> Result<WezCapabilityReport, WezProbeError> {
    let wezterm_path = canonical_executable(wezterm_bin)?;
    let version = run_probe(&wezterm_path, &["--version"], deadline)?;
    let build = parse_version(&version.stdout)?;

    let help = run_probe(
        &wezterm_path,
        &["--skip-config", "start", "--help"],
        deadline,
    )?;
    let help_text = String::from_utf8_lossy(&help.stdout);
    for required in ["--always-new-process", "--domain", "--attach"] {
        if !help_text.split_whitespace().any(|word| word == required) {
            return Err(WezProbeError::MissingAttachSurface(format!(
                "`wezterm --skip-config start --help` omitted {required}"
            )));
        }
    }

    Ok(WezCapabilityReport {
        capabilities: vec![
            CAP_WEZ.to_string(),
            format!("{WEZ_BUILD_PREFIX}{build}"),
            format!("{WEZ_PATH_PREFIX}{wezterm_path}"),
            CAP_ATTACH_NO_CREATE.to_string(),
            CAP_ACTIVATE_EXISTING.to_string(),
        ],
        build,
        wezterm_path,
    })
}

fn canonical_executable(wezterm_bin: &str) -> Result<String, WezProbeError> {
    if wezterm_bin.is_empty() || wezterm_bin.chars().any(char::is_control) {
        return Err(WezProbeError::InvalidExecutable(
            "path is empty or contains a control character".into(),
        ));
    }
    let supplied = Path::new(wezterm_bin);
    let candidate = if supplied.components().count() > 1 || supplied.is_absolute() {
        supplied.to_path_buf()
    } else {
        std::env::var_os("PATH")
            .and_then(|path| {
                std::env::split_paths(&path)
                    .map(|dir| dir.join(supplied))
                    .find(|candidate| candidate.is_file())
            })
            .ok_or_else(|| {
                WezProbeError::InvalidExecutable(format!(
                    "{wezterm_bin:?} is not an executable on PATH"
                ))
            })?
    };
    let canonical = fs::canonicalize(&candidate).map_err(|error| {
        WezProbeError::InvalidExecutable(format!("{}: {error}", candidate.display()))
    })?;
    let metadata = fs::metadata(&canonical).map_err(|error| {
        WezProbeError::InvalidExecutable(format!("{}: {error}", canonical.display()))
    })?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err(WezProbeError::InvalidExecutable(format!(
            "{} is not an executable regular file",
            canonical.display()
        )));
    }
    let canonical = canonical.into_os_string().into_string().map_err(|_| {
        WezProbeError::InvalidExecutable("canonical path is not valid UTF-8".into())
    })?;
    if !valid_reported_path(&canonical) {
        return Err(WezProbeError::InvalidExecutable(format!(
            "canonical path {canonical:?} is not a strict absolute path"
        )));
    }
    Ok(canonical)
}

struct ProbeOutput {
    stdout: Vec<u8>,
}

/// Total extra time a probe may spend letting its readers drain after the
/// child has been reaped. Mirrors `remote::client`'s transport grace.
const PROBE_DRAIN_GRACE: Duration = Duration::from_secs(2);

/// Kill the probe's whole process group, reap the direct child, then let both
/// readers finish within one shared grace.
///
/// The deadline and wait-error paths are otherwise identical, and both must
/// do the group kill *before* the joins so the inherited pipe write ends are
/// already closed when the readers are waited on.
fn abandon_probe(
    child: &mut std::process::Child,
    stdout_reader: std::thread::JoinHandle<BoundedCapture>,
    stderr_reader: std::thread::JoinHandle<BoundedCapture>,
) {
    kill_process_group(child.id());
    let _ = child.kill();
    let _ = child.wait();
    let until = Instant::now() + PROBE_DRAIN_GRACE;
    let _ = join_capture(stdout_reader, until);
    let _ = join_capture(stderr_reader, until);
}

/// Bounded child runner specialized to the two read-only probes above.
/// Output readers keep draining after their bounded capture fills, so a
/// noisy/broken executable cannot deadlock on a full pipe; see
/// [`crate::childio`] for the shared machinery and the hazards it answers.
fn run_probe(
    wezterm_bin: &str,
    args: &[&str],
    deadline: Duration,
) -> Result<ProbeOutput, WezProbeError> {
    let mut command = Command::new(wezterm_bin);
    command
        .args(args)
        .env_remove("WEZTERM_PANE")
        .env_remove("WEZTERM_UNIX_SOCKET")
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        // Isolate the fixed probe so timeout/exit can close pipes inherited
        // by an accidental descendant as well as the direct child.
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| WezProbeError::Spawn(error.to_string()))?;

    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let stdout_reader = std::thread::spawn(move || bounded_read(stdout, MAX_PROBE_OUTPUT));
    let stderr_reader = std::thread::spawn(move || bounded_read(stderr, MAX_PROBE_OUTPUT));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                kill_process_group(child.id());
                break status;
            }
            Ok(None) if started.elapsed() >= deadline => {
                abandon_probe(&mut child, stdout_reader, stderr_reader);
                return Err(WezProbeError::Timeout);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(error) => {
                abandon_probe(&mut child, stdout_reader, stderr_reader);
                return Err(WezProbeError::Spawn(format!("wait: {error}")));
            }
        }
    };
    // Bounded even on the success path: the group kill above closes the write
    // ends an inherited descendant held, but a plain `join()` here would
    // still hand a surviving grandchild the power to hang the probe past its
    // deadline. One budget across both, as in `remote::client`.
    let drain_until = Instant::now() + PROBE_DRAIN_GRACE;
    let stdout = join_capture(stdout_reader, drain_until).unwrap_or_default();
    let stderr = join_capture(stderr_reader, drain_until).unwrap_or_default();
    if !status.success() {
        return Err(WezProbeError::Failed {
            argv: format!("{} {}", wezterm_bin, args.join(" ")),
            status: status.code().unwrap_or(-1),
            detail: first_line(&stderr.bytes, "no diagnostic"),
        });
    }
    Ok(ProbeOutput {
        stdout: stdout.bytes,
    })
}

fn first_line(bytes: &[u8], fallback: &str) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn parse_version(stdout: &[u8]) -> Result<String, WezProbeError> {
    let text = std::str::from_utf8(stdout)
        .map_err(|error| WezProbeError::InvalidVersion(error.to_string()))?;
    let mut lines = text.lines();
    let line = lines
        .next()
        .ok_or_else(|| WezProbeError::InvalidVersion("empty output".into()))?;
    if lines.any(|extra| !extra.trim().is_empty()) {
        return Err(WezProbeError::InvalidVersion(
            "more than one non-empty line".into(),
        ));
    }
    let build = line.strip_prefix("wezterm ").ok_or_else(|| {
        WezProbeError::InvalidVersion("expected the `wezterm BUILD` shape".into())
    })?;
    if !valid_build(build) {
        return Err(WezProbeError::InvalidVersion(format!(
            "build token {build:?} is empty, too long, or contains unsupported bytes"
        )));
    }
    Ok(build.to_string())
}

fn valid_build(build: &str) -> bool {
    !build.is_empty()
        && build.len() <= MAX_BUILD_LEN
        && build
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+'))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildReportError {
    Malformed(String),
    Ambiguous(Vec<String>),
}

impl fmt::Display for BuildReportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildReportError::Malformed(token) => {
                write!(f, "malformed WezTerm build capability {token:?}")
            }
            BuildReportError::Ambiguous(builds) => {
                write!(f, "multiple WezTerm builds reported: {}", builds.join(", "))
            }
        }
    }
}

/// Extract exactly zero or one well-formed build from an extensible
/// capability vector.  Duplicate tokens are ambiguous rather than silently
/// choosing one.
pub fn reported_wezterm_build(capabilities: &[String]) -> Result<Option<String>, BuildReportError> {
    let mut builds = Vec::new();
    for token in capabilities {
        let Some(build) = token.strip_prefix(WEZ_BUILD_PREFIX) else {
            continue;
        };
        if !valid_build(build) {
            return Err(BuildReportError::Malformed(token.clone()));
        }
        builds.push(build.to_string());
    }
    match builds.len() {
        0 => Ok(None),
        1 => Ok(builds.pop()),
        _ => Err(BuildReportError::Ambiguous(builds)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathReportError {
    Malformed(String),
    Ambiguous(Vec<String>),
}

impl fmt::Display for PathReportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathReportError::Malformed(token) => {
                write!(f, "malformed WezTerm path capability {token:?}")
            }
            PathReportError::Ambiguous(paths) => {
                write!(f, "multiple WezTerm paths reported: {}", paths.join(", "))
            }
        }
    }
}

/// Reported owner paths are spliced verbatim into the managed domain's
/// proxy command, which the remote login shell re-parses.  Neither
/// published shape ever needs quoting, so one that would is refused rather
/// than escaped.
pub fn unquoted_shell_word(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-'))
}

fn valid_reported_path(path: &str) -> bool {
    if path.is_empty() || path.chars().any(char::is_control) {
        return false;
    }
    let path = Path::new(path);
    if !path.is_absolute() {
        return false;
    }
    if path.components().any(|component| {
        !matches!(
            component,
            std::path::Component::RootDir | std::path::Component::Normal(_)
        )
    }) {
        return false;
    }
    let normalized: PathBuf = path.components().collect();
    normalized.to_str() == path.to_str()
}

/// Extract the owner executable path for a Wez ssh-domain manifest. The
/// positive owner probe already proved this canonical path names an
/// executable regular file; the controller validates the transmitted fact
/// is exactly one strict absolute, control-free path and never guesses one.
pub fn reported_remote_wezterm_path(
    capabilities: &[String],
) -> Result<Option<String>, PathReportError> {
    let mut paths = Vec::new();
    for token in capabilities {
        let Some(path) = token.strip_prefix(WEZ_PATH_PREFIX) else {
            continue;
        };
        if !valid_reported_path(path) || !unquoted_shell_word(path) {
            return Err(PathReportError::Malformed(token.clone()));
        }
        paths.push(path.to_string());
    }
    match paths.len() {
        0 => Ok(None),
        1 => Ok(paths.pop()),
        _ => Err(PathReportError::Ambiguous(paths)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SocketReportError {
    Malformed(String),
    Ambiguous(Vec<String>),
}

impl fmt::Display for SocketReportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SocketReportError::Malformed(token) => {
                write!(f, "malformed managed Wez socket capability {token:?}")
            }
            SocketReportError::Ambiguous(sockets) => write!(
                f,
                "multiple managed Wez sockets reported: {}",
                sockets.join(", ")
            ),
        }
    }
}

/// The one endpoint the owner's service published: a strict absolute,
/// shell-bare path whose final components are the fixed
/// `dmux/wez-dmux.sock` the runtime descriptor and the Lua descriptor
/// validator also pin.
pub fn valid_managed_socket(socket: &str) -> bool {
    socket.len() <= MAX_SOCKET_LEN
        && valid_reported_path(socket)
        && unquoted_shell_word(socket)
        && socket
            .strip_suffix(MANAGED_SOCKET_SUFFIX)
            .is_some_and(|runtime_base| !runtime_base.is_empty())
}

/// Extract the owner's managed socket for the ssh domain's proxy command.
/// The owner reports its own endpoint because only it knows its runtime
/// dir; the controller proves the shape and never guesses or completes one.
pub fn reported_managed_wez_socket(
    capabilities: &[String],
) -> Result<Option<String>, SocketReportError> {
    let mut sockets = Vec::new();
    for token in capabilities {
        let Some(socket) = token.strip_prefix(WEZ_SOCKET_PREFIX) else {
            continue;
        };
        if !valid_managed_socket(socket) {
            return Err(SocketReportError::Malformed(token.clone()));
        }
        sockets.push(socket.to_string());
    }
    match sockets.len() {
        0 => Ok(None),
        1 => Ok(sockets.pop()),
        _ => Err(SocketReportError::Ambiguous(sockets)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteWezRefusal {
    ControllerWezMissing,
    OwnerWezMissing,
    ControllerBuildMissing,
    OwnerBuildMissing,
    ControllerBuildMalformed(String),
    OwnerBuildMalformed(String),
    OwnerPathMissing,
    OwnerPathMalformed(String),
    OwnerSocketMissing,
    OwnerSocketMalformed(String),
    BuildMismatch { controller: String, owner: String },
    ControllerCapabilityMissing(String),
    OwnerCapabilityMissing(String),
}

impl RemoteWezRefusal {
    fn is_version_failure(&self) -> bool {
        matches!(
            self,
            RemoteWezRefusal::ControllerBuildMissing
                | RemoteWezRefusal::OwnerBuildMissing
                | RemoteWezRefusal::ControllerBuildMalformed(_)
                | RemoteWezRefusal::OwnerBuildMalformed(_)
                | RemoteWezRefusal::BuildMismatch { .. }
        )
    }
}

impl fmt::Display for RemoteWezRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RemoteWezRefusal::ControllerWezMissing => {
                f.write_str("controller did not positively report WezTerm")
            }
            RemoteWezRefusal::OwnerWezMissing => {
                f.write_str("owner did not positively report a managed WezTerm backend")
            }
            RemoteWezRefusal::ControllerBuildMissing => {
                f.write_str("controller omitted its exact WezTerm build")
            }
            RemoteWezRefusal::OwnerBuildMissing => {
                f.write_str("owner omitted its exact WezTerm build (legacy/incompatible agent)")
            }
            RemoteWezRefusal::ControllerBuildMalformed(detail) => {
                write!(f, "controller build report is malformed: {detail}")
            }
            RemoteWezRefusal::OwnerBuildMalformed(detail) => {
                write!(f, "owner build report is malformed: {detail}")
            }
            RemoteWezRefusal::OwnerPathMissing => {
                f.write_str("owner omitted its canonical absolute WezTerm executable path")
            }
            RemoteWezRefusal::OwnerPathMalformed(detail) => {
                write!(
                    f,
                    "owner WezTerm executable path report is malformed: {detail}"
                )
            }
            RemoteWezRefusal::OwnerSocketMissing => {
                f.write_str("owner omitted its managed Wez mux socket")
            }
            RemoteWezRefusal::OwnerSocketMalformed(detail) => {
                write!(f, "owner managed Wez socket report is malformed: {detail}")
            }
            RemoteWezRefusal::BuildMismatch { controller, owner } => write!(
                f,
                "controller WezTerm build {controller:?} differs from owner build {owner:?}"
            ),
            RemoteWezRefusal::ControllerCapabilityMissing(capability) => write!(
                f,
                "controller omitted required presentation capability {capability:?}"
            ),
            RemoteWezRefusal::OwnerCapabilityMissing(capability) => write!(
                f,
                "owner omitted required presentation capability {capability:?}"
            ),
        }
    }
}

/// Structured result deliberately separates automatic Wez eligibility from
/// explicit tmux availability.  A refusal must be surfaced; callers must
/// not turn `explicit_tmux_available=true` into an automatic fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteWezAssessment {
    pub build: Option<String>,
    pub refusal: Option<RemoteWezRefusal>,
    pub explicit_tmux_available: bool,
}

impl RemoteWezAssessment {
    pub fn is_eligible(&self) -> bool {
        self.refusal.is_none() && self.build.is_some()
    }

    /// Convert a refusal to the plan's typed exit-6 surface.  The explicit
    /// tmux hint is informational, never authorization to fall back.
    pub fn typed_error(&self) -> Option<TypedError> {
        let refusal = self.refusal.as_ref()?;
        let code = if refusal.is_version_failure() {
            ErrorCode::VersionMismatch
        } else {
            ErrorCode::ProviderUnavailable
        };
        let tmux = if self.explicit_tmux_available {
            " explicit `--backend tmux` remains available; automatic fallback is forbidden"
        } else {
            " the owner did not advertise tmux either; automatic fallback is forbidden"
        };
        Some(TypedError::new(
            code,
            format!("automatic remote Wez refused: {refusal};{tmux}"),
        ))
    }
}

/// Enforce the pre-compatibility-matrix rule from plan §17.  Both sides
/// must positively report Wez, one exact well-formed build, the same build,
/// and every required presentation capability.
pub fn assess_automatic_remote_wez(
    controller_capabilities: &[String],
    owner_capabilities: &[String],
) -> RemoteWezAssessment {
    let explicit_tmux_available = contains(owner_capabilities, CAP_TMUX);
    let refused = |refusal| RemoteWezAssessment {
        build: None,
        refusal: Some(refusal),
        explicit_tmux_available,
    };

    if !contains(controller_capabilities, CAP_WEZ) {
        return refused(RemoteWezRefusal::ControllerWezMissing);
    }
    if !contains(owner_capabilities, CAP_WEZ) {
        return refused(RemoteWezRefusal::OwnerWezMissing);
    }

    let controller_build = match reported_wezterm_build(controller_capabilities) {
        Ok(Some(build)) => build,
        Ok(None) => return refused(RemoteWezRefusal::ControllerBuildMissing),
        Err(error) => {
            return refused(RemoteWezRefusal::ControllerBuildMalformed(
                error.to_string(),
            ));
        }
    };
    let owner_build = match reported_wezterm_build(owner_capabilities) {
        Ok(Some(build)) => build,
        Ok(None) => return refused(RemoteWezRefusal::OwnerBuildMissing),
        Err(error) => {
            return refused(RemoteWezRefusal::OwnerBuildMalformed(error.to_string()));
        }
    };
    if controller_build != owner_build {
        return refused(RemoteWezRefusal::BuildMismatch {
            controller: controller_build,
            owner: owner_build,
        });
    }

    match reported_remote_wezterm_path(owner_capabilities) {
        Ok(Some(_)) => {}
        Ok(None) => return refused(RemoteWezRefusal::OwnerPathMissing),
        Err(error) => {
            return refused(RemoteWezRefusal::OwnerPathMalformed(error.to_string()));
        }
    }
    // Without it the controller cannot pin the domain's proxy command, and
    // an unpinned proxy auto-starts an unmanaged owner-side server.
    match reported_managed_wez_socket(owner_capabilities) {
        Ok(Some(_)) => {}
        Ok(None) => return refused(RemoteWezRefusal::OwnerSocketMissing),
        Err(error) => {
            return refused(RemoteWezRefusal::OwnerSocketMalformed(error.to_string()));
        }
    }

    for capability in REQUIRED_PRESENTATION_CAPABILITIES {
        if !contains(controller_capabilities, capability) {
            return refused(RemoteWezRefusal::ControllerCapabilityMissing(
                capability.to_string(),
            ));
        }
        if !contains(owner_capabilities, capability) {
            return refused(RemoteWezRefusal::OwnerCapabilityMissing(
                capability.to_string(),
            ));
        }
    }

    RemoteWezAssessment {
        build: Some(controller_build),
        refusal: None,
        explicit_tmux_available,
    }
}

/// Convenience seam for the P9 controller: feed its local positive probe
/// and the already identity/lineage-validated owner hello into the gate.
pub fn assess_automatic_remote_wez_hello(
    controller: &WezCapabilityReport,
    owner: &crate::remote::protocol::HelloInfo,
) -> RemoteWezAssessment {
    assess_automatic_remote_wez(&controller.capabilities, &owner.capabilities)
}

/// Strict controller seam: returns the common exact build or the typed
/// exit-6 refusal.  Explicit tmux remains a separate caller-selected path;
/// this helper never chooses it.
pub fn require_automatic_remote_wez_hello(
    controller: &WezCapabilityReport,
    owner: &crate::remote::protocol::HelloInfo,
) -> Result<String, TypedError> {
    let assessment = assess_automatic_remote_wez_hello(controller, owner);
    if let Some(error) = assessment.typed_error() {
        return Err(error);
    }
    assessment.build.ok_or_else(|| {
        TypedError::new(
            ErrorCode::OperationFailed,
            "remote Wez assessment produced an inconsistent internal result",
        )
    })
}

fn contains(capabilities: &[String], required: &str) -> bool {
    capabilities.iter().any(|token| token == required)
}
