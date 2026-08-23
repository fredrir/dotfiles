//! Provisional pane bootstrap: the broker↔helper protocol and the broker
//! state machine (plan §11.1, ADR 004). Root-owned (plan §19).
//!
//! Frozen wire contract (the helper binary in `src/bin/pane-bootstrap.rs`
//! implements the other side):
//!
//! 1. Broker journals the request, creates the per-uid FIFO (0600) under
//!    `<runtime>/bootstrap/`, then spawns the helper as the pane program:
//!    `[helper-bin, <request-uid>, "--", <program argv...>]`.
//! 2. Helper immediately: sets the reserved title
//!    `dmux-bootstrap:<request-uid>` (tmux-wrapped per ADR 005 when inside
//!    tmux), writes the [`PaneEnvRecord`] JSON to `<uid>.pane-env`, opens
//!    the FIFO **O_RDWR** (a read-only open would block in `open(2)` and
//!    void the timeout — ADR 004) and reads one line, bounded by
//!    [`HELPER_READ_TIMEOUT_SECS`].
//! 3. Broker correlates three ways (spawn return = reserved-title scan =
//!    recorded pane env), then writes one [`BootstrapResult`] JSON line.
//! 4. Helper verifies the uid matches its argv token, exports the `DMUX_*`
//!    marker environment, emits the SetUserVar markers and the final title
//!    `dmux-run:<request-uid>`, writes the [`HelperAck`] JSON to
//!    `<uid>.ack`, and `exec`s the program in place (pane id and PID are
//!    preserved).
//! 5. Timeout: helper writes `<uid>.timeout` and exits [`EXIT_TIMEOUT`],
//!    never running user code; the pane self-closes, so a takeover finding
//!    zero orphans is a normal state (retry only after confirmed absence).

use std::ffi::CString;
use std::fs;
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::TypedError;
use crate::model::{Backend, BackendInstanceUid, HostUid, ServerEpoch, SpaceNo, SpaceUid};

pub const RESERVED_TITLE_PREFIX: &str = "dmux-bootstrap:";
pub const RUN_TITLE_PREFIX: &str = "dmux-run:";
pub const HELPER_READ_TIMEOUT_SECS: u64 = 30;
pub const EXIT_TIMEOUT: i32 = 41;
pub const BOOTSTRAP_SUBDIR: &str = "bootstrap";

pub fn reserved_title(uid: Uuid) -> String {
    format!("{RESERVED_TITLE_PREFIX}{uid}")
}

pub fn run_title(uid: Uuid) -> String {
    format!("{RUN_TITLE_PREFIX}{uid}")
}

/// The helper argv: uid token first, requested program after `--`
/// (plan §11.1 — future native pane IDs are never guessed; the token plus
/// the reserved title carry identity).
/// The program a managed pane runs when the caller named none: the owner's
/// login shell as `[shell, "-l"]`.  Creation (`new`, `group new`, `split
/// new`) and cold recovery's restore share it, so a managed pane is the
/// shell the owner logs in with and its profile sets the PATH — not
/// `/bin/sh` inheriting the service manager's minimal environment.
pub fn login_shell_program() -> Vec<String> {
    vec![login_shell(), "-l".to_string()]
}

/// `$SHELL` when it is an absolute path to an existing file, else the
/// passwd entry's shell under the same test, else `/bin/sh`.
pub fn login_shell() -> String {
    login_shell_from(
        std::env::var("SHELL").ok().as_deref(),
        passwd_shell().as_deref(),
        |candidate| Path::new(candidate).is_file(),
    )
}

/// The pure rule behind [`login_shell`]: the first candidate that is an
/// absolute path `exists` accepts; a relative spelling (`zsh`), an empty
/// value or a missing file is skipped, never guessed at.
pub fn login_shell_from(
    shell_env: Option<&str>,
    passwd_shell: Option<&str>,
    exists: impl Fn(&str) -> bool,
) -> String {
    [shell_env, passwd_shell]
        .into_iter()
        .flatten()
        .find(|candidate| candidate.starts_with('/') && exists(candidate))
        .map(str::to_string)
        .unwrap_or_else(|| "/bin/sh".to_string())
}

#[cfg(unix)]
fn passwd_shell() -> Option<String> {
    use std::ffi::CStr;
    type PwEntry = libc::passwd;
    let mut entry: PwEntry = unsafe { std::mem::zeroed() };
    let mut buffer = vec![0u8; 16 * 1024];
    let mut found: *mut PwEntry = std::ptr::null_mut();
    // SAFETY: every pointer names a live local for the duration of the
    // call; `getpwuid_r` writes within `buffer.len()` bytes.
    let rc = unsafe {
        libc::getpwuid_r(
            libc::getuid(),
            &mut entry,
            buffer.as_mut_ptr().cast::<libc::c_char>(),
            buffer.len(),
            &mut found,
        )
    };
    if rc != 0 || found.is_null() || entry.pw_shell.is_null() {
        return None;
    }
    // SAFETY: `pw_shell` points into `buffer`, which outlives this read.
    let shell = unsafe { CStr::from_ptr(entry.pw_shell) }.to_str().ok()?;
    (!shell.is_empty()).then(|| shell.to_string())
}

#[cfg(not(unix))]
fn passwd_shell() -> Option<String> {
    None
}

pub fn helper_argv(helper_bin: &str, uid: Uuid, program: &[String]) -> Vec<String> {
    let mut argv = Vec::with_capacity(program.len() + 3);
    argv.push(helper_bin.to_string());
    argv.push(uid.to_string());
    argv.push("--".to_string());
    argv.extend(program.iter().cloned());
    argv
}

/// Per-request filesystem endpoints beneath the verified runtime dir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapPaths {
    pub fifo: PathBuf,
    pub pane_env: PathBuf,
    pub ack: PathBuf,
    pub timeout_marker: PathBuf,
}

impl BootstrapPaths {
    pub fn new(runtime_dir: &Path, uid: Uuid) -> Self {
        let dir = runtime_dir.join(BOOTSTRAP_SUBDIR);
        BootstrapPaths {
            fifo: dir.join(format!("{uid}.fifo")),
            pane_env: dir.join(format!("{uid}.pane-env")),
            ack: dir.join(format!("{uid}.ack")),
            timeout_marker: dir.join(format!("{uid}.timeout")),
        }
    }
}

/// Written by the helper immediately after it sets the reserved title:
/// the provider-inherited pane identity, one third of the correlation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneEnvRecord {
    pub request_uid: Uuid,
    pub wezterm_pane: Option<String>,
    pub tmux_pane: Option<String>,
    pub helper_pid: u32,
}

/// The marker context the helper exports and emits (plan §13.1 fields).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkerContext {
    pub host_uid: HostUid,
    pub space_uid: SpaceUid,
    pub space_no: SpaceNo,
    pub backend: Backend,
    pub domain: Option<String>,
    pub server_epoch: ServerEpoch,
    /// `child_suffix` forms, e.g. `g<epoch>.wz-3` / `p<epoch>.wz-4`.
    pub group_ref: String,
    pub split_ref: String,
}

/// The one-line JSON payload the broker writes into the FIFO after
/// successful correlation (the "signed one-use bootstrap result"; transport
/// authorization is the 0600 per-uid FIFO under the verified runtime dir).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BootstrapResult {
    pub request_uid: Uuid,
    pub context: MarkerContext,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperAck {
    pub request_uid: Uuid,
}

// ---------------------------------------------------------------------------
// Broker side

pub fn prepare(runtime_dir: &Path, uid: Uuid) -> io::Result<BootstrapPaths> {
    let dir = runtime_dir.join(BOOTSTRAP_SUBDIR);
    if !dir.exists() {
        std::os::unix::fs::DirBuilderExt::mode(&mut fs::DirBuilder::new(), 0o700).create(&dir)?;
    }
    let paths = BootstrapPaths::new(runtime_dir, uid);
    let c_path = CString::new(paths.fifo.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "nul in fifo path"))?;
    let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(paths)
}

#[derive(Debug, PartialEq, Eq)]
pub enum SendError {
    /// No reader appeared within the deadline: the helper is gone/never ran.
    NoReader,
    Io(String),
}

/// Write the result line without ever blocking forever: open O_WRONLY with
/// O_NONBLOCK and retry on `ENXIO` (no reader yet) until the deadline.
pub fn send_result(
    paths: &BootstrapPaths,
    result: &BootstrapResult,
    deadline: Duration,
) -> Result<(), SendError> {
    let line = serde_json::to_string(result).map_err(|e| SendError::Io(e.to_string()))?;
    let start = Instant::now();
    loop {
        match open_nonblock_writer(&paths.fifo) {
            Ok(mut file) => {
                return file
                    .write_all(format!("{line}\n").as_bytes())
                    .map_err(|e| SendError::Io(e.to_string()));
            }
            Err(e) if e.raw_os_error() == Some(libc::ENXIO) => {
                if start.elapsed() >= deadline {
                    return Err(SendError::NoReader);
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(SendError::Io(e.to_string())),
        }
    }
}

fn open_nonblock_writer(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

fn poll_json<T: serde::de::DeserializeOwned>(
    path: &Path,
    deadline: Duration,
) -> io::Result<Option<T>> {
    let start = Instant::now();
    loop {
        match fs::read_to_string(path) {
            Ok(text) if !text.trim().is_empty() => {
                return serde_json::from_str(&text)
                    .map(Some)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e));
            }
            Ok(_) | Err(_) if start.elapsed() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(_) => return Ok(None),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        }
    }
}

pub fn read_pane_env(
    paths: &BootstrapPaths,
    deadline: Duration,
) -> io::Result<Option<PaneEnvRecord>> {
    poll_json(&paths.pane_env, deadline)
}

pub fn read_ack(paths: &BootstrapPaths, deadline: Duration) -> io::Result<Option<HelperAck>> {
    poll_json(&paths.ack, deadline)
}

pub fn cleanup(paths: &BootstrapPaths) {
    for p in [
        &paths.fifo,
        &paths.pane_env,
        &paths.ack,
        &paths.timeout_marker,
    ] {
        let _ = fs::remove_file(p);
    }
}

// ---------------------------------------------------------------------------
// Correlation (ADR 004: three-way exact agreement)

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Correlation {
    /// Exactly one reserved-title pane, agreeing with every available
    /// witness.
    Confirmed { pane: String },
    /// No reserved-title pane in a complete same-epoch scan.
    NotFound,
    /// More than one pane bears the reserved title: conflict, no kill.
    Multiple { count: usize },
    /// Witnesses disagree (title-pane vs spawn return vs inherited env):
    /// conflict, never a guess.
    Mismatch { detail: String },
}

/// `titled` are the pane handles bearing `dmux-bootstrap:<uid>` in one
/// complete same-epoch scan; `spawn_return` and `inherited_env` are the
/// other witnesses when available.
pub fn correlate(
    titled: &[String],
    spawn_return: Option<&str>,
    inherited_env: Option<&str>,
) -> Correlation {
    match titled {
        [] => Correlation::NotFound,
        [one] => {
            if let Some(sr) = spawn_return
                && sr != one
            {
                return Correlation::Mismatch {
                    detail: format!("spawn returned {sr} but title scan found {one}"),
                };
            }
            if let Some(env) = inherited_env
                && env != one
            {
                return Correlation::Mismatch {
                    detail: format!("helper inherited {env} but title scan found {one}"),
                };
            }
            Correlation::Confirmed { pane: one.clone() }
        }
        many => Correlation::Multiple { count: many.len() },
    }
}

/// Orphan classification for takeover (ADR 004): zero found is a normal
/// state (timed-out panes self-close) and permits retry only after this
/// confirmed absence; multiple is conflict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrphanScan {
    ConfirmedAbsent,
    ExactlyOne { pane: String },
    Multiple { count: usize },
}

pub fn classify_orphans(titled: &[String]) -> OrphanScan {
    match titled {
        [] => OrphanScan::ConfirmedAbsent,
        [one] => OrphanScan::ExactlyOne { pane: one.clone() },
        many => OrphanScan::Multiple { count: many.len() },
    }
}

// ---------------------------------------------------------------------------
// Journal seam (implemented for `Registry` by the identity agent)

/// Mirrors `bootstrap_requests.state` in registry-v1.sql.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapState {
    Issued,
    Spawned,
    Correlated,
    Acked,
    Completed,
    Timeout,
    Orphaned,
    Conflict,
    Aborted,
}

impl BootstrapState {
    pub fn as_str(self) -> &'static str {
        match self {
            BootstrapState::Issued => "issued",
            BootstrapState::Spawned => "spawned",
            BootstrapState::Correlated => "correlated",
            BootstrapState::Acked => "acked",
            BootstrapState::Completed => "completed",
            BootstrapState::Timeout => "timeout",
            BootstrapState::Orphaned => "orphaned",
            BootstrapState::Conflict => "conflict",
            BootstrapState::Aborted => "aborted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedRequest {
    pub request_uid: Uuid,
    pub operation_uid: Option<Uuid>,
    pub space_uid: Option<SpaceUid>,
    pub backend_instance: BackendInstanceUid,
    pub server_epoch: ServerEpoch,
    /// Exact native parent locator (`@N`/window-id) or None for a new Space.
    pub intended_parent: Option<String>,
    /// Recovery generation + manifest node path, when recovery-created.
    pub recovery_generation: Option<String>,
    pub manifest_node_path: Option<String>,
}

pub trait BootstrapJournal {
    fn bootstrap_issue(&mut self, request: &IssuedRequest) -> Result<(), TypedError>;
    fn bootstrap_spawned(&mut self, uid: Uuid, returned_native_ids: &str)
    -> Result<(), TypedError>;
    fn bootstrap_correlated(
        &mut self,
        uid: Uuid,
        group_ref: &str,
        split_ref: &str,
    ) -> Result<(), TypedError>;
    fn bootstrap_state(&mut self, uid: Uuid, state: BootstrapState) -> Result<(), TypedError>;
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use super::*;

    fn uid() -> Uuid {
        Uuid::from_u128(7)
    }

    fn result() -> BootstrapResult {
        BootstrapResult {
            request_uid: uid(),
            context: MarkerContext {
                host_uid: HostUid(Uuid::nil()),
                space_uid: SpaceUid(Uuid::max()),
                space_no: SpaceNo(NonZeroU64::new(2).unwrap()),
                backend: Backend::Wez,
                domain: Some("unix".into()),
                server_epoch: ServerEpoch(Uuid::nil()),
                group_ref: format!("g{}.wz-3", Uuid::nil()),
                split_ref: format!("p{}.wz-4", Uuid::nil()),
            },
        }
    }

    #[test]
    fn the_login_shell_is_the_first_absolute_existing_candidate() {
        let exists = |c: &str| c == "/bin/zsh" || c == "/bin/sh";
        assert_eq!(
            login_shell_from(Some("/bin/zsh"), Some("/bin/bash"), exists),
            "/bin/zsh"
        );
        // A relative spelling is not a login shell.
        assert_eq!(
            login_shell_from(Some("zsh"), Some("/bin/zsh"), exists),
            "/bin/zsh"
        );
        // A shell that is not installed is skipped, not guessed at.
        assert_eq!(
            login_shell_from(Some("/opt/fish"), Some("/bin/zsh"), exists),
            "/bin/zsh"
        );
        assert_eq!(login_shell_from(Some(""), None, exists), "/bin/sh");
        assert_eq!(login_shell_from(None, None, exists), "/bin/sh");
        assert_eq!(
            login_shell_from(None, Some("/usr/bin/nologin"), exists),
            "/bin/sh"
        );
    }

    #[test]
    fn the_default_program_is_a_login_shell_invocation() {
        let program = login_shell_program();
        assert_eq!(program.len(), 2);
        assert!(program[0].starts_with('/'), "{program:?}");
        assert!(Path::new(&program[0]).is_file(), "{program:?}");
        assert_eq!(program[1], "-l");
    }

    #[test]
    fn helper_argv_shape() {
        let argv = helper_argv(
            "/usr/local/bin/pane-bootstrap",
            uid(),
            &["zsh".into(), "-l".into()],
        );
        assert_eq!(
            argv,
            vec![
                "/usr/local/bin/pane-bootstrap".to_string(),
                uid().to_string(),
                "--".into(),
                "zsh".into(),
                "-l".into(),
            ]
        );
    }

    #[test]
    fn fifo_round_trip_with_a_fake_helper() {
        let dir = tempfile::tempdir().unwrap();
        let paths = prepare(dir.path(), uid()).unwrap();
        let fifo = paths.fifo.clone();
        // Fake helper: open RDWR (the frozen rule), read one line.
        let reader = std::thread::spawn(move || {
            use std::io::BufRead;
            let file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&fifo)
                .unwrap();
            let mut line = String::new();
            std::io::BufReader::new(file).read_line(&mut line).unwrap();
            line
        });
        send_result(&paths, &result(), Duration::from_secs(5)).unwrap();
        let line = reader.join().unwrap();
        let parsed: BootstrapResult = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(parsed, result());
        cleanup(&paths);
        assert!(!paths.fifo.exists());
    }

    #[test]
    fn send_without_reader_times_out_as_no_reader() {
        let dir = tempfile::tempdir().unwrap();
        let paths = prepare(dir.path(), uid()).unwrap();
        let err = send_result(&paths, &result(), Duration::from_millis(120)).unwrap_err();
        assert_eq!(err, SendError::NoReader);
        cleanup(&paths);
    }

    #[test]
    fn pane_env_and_ack_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let paths = prepare(dir.path(), uid()).unwrap();
        assert_eq!(
            read_pane_env(&paths, Duration::from_millis(50)).unwrap(),
            None
        );
        let record = PaneEnvRecord {
            request_uid: uid(),
            wezterm_pane: Some("8".into()),
            tmux_pane: None,
            helper_pid: 1234,
        };
        fs::write(&paths.pane_env, serde_json::to_string(&record).unwrap()).unwrap();
        assert_eq!(
            read_pane_env(&paths, Duration::from_millis(50)).unwrap(),
            Some(record)
        );
        fs::write(
            &paths.ack,
            serde_json::to_string(&HelperAck { request_uid: uid() }).unwrap(),
        )
        .unwrap();
        assert_eq!(
            read_ack(&paths, Duration::from_millis(50)).unwrap(),
            Some(HelperAck { request_uid: uid() })
        );
        cleanup(&paths);
    }

    #[test]
    fn correlation_matrix() {
        let one = ["8".to_string()];
        assert_eq!(
            correlate(&one, Some("8"), Some("8")),
            Correlation::Confirmed { pane: "8".into() }
        );
        // Witnesses are optional but must agree when present.
        assert_eq!(
            correlate(&one, None, None),
            Correlation::Confirmed { pane: "8".into() }
        );
        assert!(matches!(
            correlate(&one, Some("9"), None),
            Correlation::Mismatch { .. }
        ));
        assert!(matches!(
            correlate(&one, None, Some("9")),
            Correlation::Mismatch { .. }
        ));
        assert_eq!(correlate(&[], Some("8"), None), Correlation::NotFound);
        let two = ["8".to_string(), "9".to_string()];
        assert_eq!(
            correlate(&two, None, None),
            Correlation::Multiple { count: 2 }
        );
    }

    #[test]
    fn orphan_scan_matrix() {
        assert_eq!(classify_orphans(&[]), OrphanScan::ConfirmedAbsent);
        assert_eq!(
            classify_orphans(&["%4".to_string()]),
            OrphanScan::ExactlyOne { pane: "%4".into() }
        );
        assert_eq!(
            classify_orphans(&["%4".to_string(), "%5".to_string()]),
            OrphanScan::Multiple { count: 2 }
        );
    }

    #[test]
    fn titles_are_stable_contract() {
        assert_eq!(reserved_title(uid()), format!("dmux-bootstrap:{}", uid()));
        assert_eq!(run_title(uid()), format!("dmux-run:{}", uid()));
    }
}
