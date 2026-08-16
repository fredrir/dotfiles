//! Internal pane-bootstrap helper (plan §11.1, §13.1; ADR 004, ADR 005 §2).
//!
//! Spawned by the owner-side bootstrap broker as the pane program:
//! `pane-bootstrap <request-uid> -- <program...>`. Implements the helper
//! side of the frozen wire contract in `dmux::bootstrap` (its doc comment is
//! the spec):
//!
//! 1. Immediately emits the reserved title `dmux-bootstrap:<uid>` via OSC 2
//!    (DCS `tmux;` passthrough-wrapped when `$TMUX` is set — ADR 005 §2),
//!    writes the [`PaneEnvRecord`] to `<uid>.pane-env`, opens the FIFO
//!    **O_RDWR** (a read-only open would block in `open(2)` and void the
//!    timeout) and polls nonblocking reads for one line, bounded by
//!    [`bootstrap::HELPER_READ_TIMEOUT_SECS`].
//! 2. On a payload it verifies the uid, exports the §13.1 `DMUX_*` marker
//!    environment, emits one SetUserVar OSC 1337 per marker plus the final
//!    `dmux-run:<uid>` title, writes the [`HelperAck`], and `exec`s the
//!    program in place — never spawn-and-wait.
//! 3. On timeout it writes the `<uid>.timeout` marker and exits
//!    [`bootstrap::EXIT_TIMEOUT`] (41). On a protocol violation (missing
//!    FIFO, unparsable payload, uid mismatch) it writes the same marker
//!    shape and exits [`EXIT_PROTOCOL`] (40). User code never runs on any
//!    non-success path.
//!
//! Exit codes: 0 is replaced by the exec'd program; 2 usage; 40 protocol
//! violation (broker never prepared / sent garbage / uid mismatch — distinct
//! from a plain timeout); 41 timeout; 126/127 exec failure.
//!
//! Test seams (production never sets either):
//! - `DMUX_RUNTIME_DIR`: used verbatim as the runtime dir instead of
//!   `dmux::runtime::dmux_runtime_dir()`; the test owns the directory.
//! - `DMUX_BOOTSTRAP_TIMEOUT_SECS`: overrides the FIFO read bound (fractional
//!   seconds accepted).

use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{self, Command};
use std::time::{Duration, Instant};

use dmux::bootstrap::MarkerContext;
use dmux::bootstrap::{self, BootstrapPaths, BootstrapResult, HelperAck, PaneEnvRecord};
use dmux::runtime;
use uuid::Uuid;

const EXIT_USAGE: i32 = 2;
/// Protocol violation: broker never prepared the FIFO, sent an unparsable
/// payload, or sent a result for a different request uid. Distinct from
/// [`bootstrap::EXIT_TIMEOUT`] (41) so takeover can tell "broker absent/slow"
/// from "broker misbehaved".
const EXIT_PROTOCOL: i32 = 40;

fn main() {
    process::exit(run());
}

fn run() -> i32 {
    let args: Vec<String> = env::args().collect();
    let Some((uid, program)) = parse_argv(&args) else {
        eprintln!("usage: pane-bootstrap <request-uid> -- <program> [args...]");
        return EXIT_USAGE;
    };

    // ADR 005 §2: markers/titles must be DCS-passthrough-wrapped inside tmux.
    let in_tmux = env::var_os("TMUX").is_some();

    // (a) Reserved title first: correlation must find this pane even if
    // everything after this line fails.
    emit(&osc_title(&bootstrap::reserved_title(uid)), in_tmux);

    // (b) Runtime dir. Failure here means we cannot even write a marker;
    // report on stderr and exit as a protocol violation.
    let runtime_dir = match resolve_runtime_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("dmux pane-bootstrap[{uid}]: cannot resolve runtime dir: {err}");
            return EXIT_PROTOCOL;
        }
    };
    let paths = BootstrapPaths::new(&runtime_dir, uid);
    // Marker/pane-env writes need the subdir even when the broker never
    // prepared it (the missing-FIFO violation path); creation is idempotent.
    ensure_bootstrap_dir(&runtime_dir);

    // (c) Record the provider-inherited pane identity: one third of the
    // broker's three-way correlation.
    let record = PaneEnvRecord {
        request_uid: uid,
        wezterm_pane: env::var("WEZTERM_PANE").ok(),
        tmux_pane: env::var("TMUX_PANE").ok(),
        helper_pid: process::id(),
    };
    match serde_json::to_string(&record) {
        Ok(json) => {
            if let Err(err) = fs::write(&paths.pane_env, json) {
                return protocol_violation(&paths, uid, &format!("cannot write pane-env: {err}"));
            }
        }
        Err(err) => {
            return protocol_violation(&paths, uid, &format!("cannot encode pane-env: {err}"));
        }
    }

    // Bounded read of the one-line bootstrap result.
    let timeout = read_timeout();
    let line = match read_fifo_line(&paths.fifo, timeout) {
        Ok(Some(line)) => line,
        Ok(None) => {
            write_marker(
                &paths,
                uid,
                &format!(
                    "timed out after {:.1}s waiting for bootstrap result",
                    timeout.as_secs_f64()
                ),
            );
            eprintln!(
                "dmux pane-bootstrap[{uid}]: timed out after {:.1}s waiting for the broker; not running the program",
                timeout.as_secs_f64()
            );
            return bootstrap::EXIT_TIMEOUT;
        }
        Err(FifoError::Missing) => {
            return protocol_violation(
                &paths,
                uid,
                "bootstrap FIFO missing: broker never prepared this request",
            );
        }
        Err(FifoError::NotAFifo) => {
            return protocol_violation(&paths, uid, "bootstrap FIFO path is not a FIFO");
        }
        Err(FifoError::Io(err)) => {
            return protocol_violation(&paths, uid, &format!("bootstrap FIFO error: {err}"));
        }
    };

    let result: BootstrapResult = match serde_json::from_str(line.trim()) {
        Ok(result) => result,
        Err(err) => {
            return protocol_violation(&paths, uid, &format!("unparsable bootstrap result: {err}"));
        }
    };
    if result.request_uid != uid {
        return protocol_violation(
            &paths,
            uid,
            &format!("request uid mismatch: broker sent {}", result.request_uid),
        );
    }

    // §13.1 markers: SetUserVar per field (name = lowercase env name, value
    // base64), then the final run title, then the ack.
    let markers = marker_env(&result.context);
    for (name, value) in &markers {
        emit(
            &osc_set_user_var(&name.to_ascii_lowercase(), value),
            in_tmux,
        );
    }
    emit(&osc_title(&bootstrap::run_title(uid)), in_tmux);

    let ack = HelperAck { request_uid: uid };
    let ack_json = serde_json::to_string(&ack).expect("HelperAck serializes");
    if let Err(err) = fs::write(&paths.ack, ack_json) {
        return protocol_violation(&paths, uid, &format!("cannot write ack: {err}"));
    }

    // exec in place: pane id and PID are preserved (ADR 004). The marker env
    // is exported to the program image; everything else is inherited.
    let mut command = Command::new(&program[0]);
    command.args(&program[1..]);
    for (name, value) in &markers {
        command.env(name, value);
    }
    let err = command.exec();
    eprintln!(
        "dmux pane-bootstrap[{uid}]: exec {} failed: {err}",
        program[0]
    );
    if err.kind() == io::ErrorKind::NotFound {
        127
    } else {
        126
    }
}

/// `<request-uid> -- <program...>`, uid strictly canonical (lowercase
/// hyphenated round-trip — the broker builds argv from `Uuid::to_string`).
fn parse_argv(args: &[String]) -> Option<(Uuid, &[String])> {
    let [_, uid_token, separator, program @ ..] = args else {
        return None;
    };
    if separator != "--" || program.is_empty() {
        return None;
    }
    let uid = Uuid::parse_str(uid_token).ok()?;
    if uid.to_string() != *uid_token {
        return None;
    }
    Some((uid, program))
}

/// Production resolves via the secured platform resolver; tests own the dir
/// and point `DMUX_RUNTIME_DIR` at a scratch path (used verbatim — the
/// ownership/mode checks belong to whoever created it).
fn resolve_runtime_dir() -> io::Result<PathBuf> {
    if let Some(dir) = env::var_os("DMUX_RUNTIME_DIR") {
        return Ok(PathBuf::from(dir));
    }
    runtime::dmux_runtime_dir()
}

fn ensure_bootstrap_dir(runtime_dir: &Path) {
    let dir = runtime_dir.join(bootstrap::BOOTSTRAP_SUBDIR);
    if !dir.exists() {
        let _ =
            std::os::unix::fs::DirBuilderExt::mode(&mut fs::DirBuilder::new(), 0o700).create(&dir);
    }
}

fn read_timeout() -> Duration {
    if let Ok(raw) = env::var("DMUX_BOOTSTRAP_TIMEOUT_SECS")
        && let Ok(secs) = raw.parse::<f64>()
        && secs.is_finite()
        && secs >= 0.0
    {
        return Duration::from_secs_f64(secs);
    }
    Duration::from_secs(bootstrap::HELPER_READ_TIMEOUT_SECS)
}

enum FifoError {
    Missing,
    NotAFifo,
    Io(io::Error),
}

/// Open O_RDWR (mandatory — read-only would block in `open(2)`; our own
/// write end also guarantees reads yield EAGAIN instead of EOF while the
/// broker has not written) plus O_NONBLOCK, then poll for one line.
fn read_fifo_line(path: &Path, timeout: Duration) -> Result<Option<String>, FifoError> {
    use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};

    let mut file = match fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Err(FifoError::Missing),
        Err(err) => return Err(FifoError::Io(err)),
    };
    match file.metadata() {
        Ok(meta) if meta.file_type().is_fifo() => {}
        Ok(_) => return Err(FifoError::NotAFifo),
        Err(err) => return Err(FifoError::Io(err)),
    }

    let start = Instant::now();
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match file.read(&mut chunk) {
            Ok(0) => {} // no data yet (cannot be EOF: we hold a write end)
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                    return Ok(Some(String::from_utf8_lossy(&buf[..pos]).into_owned()));
                }
                continue; // mid-line: read again without sleeping
            }
            Err(err)
                if err.kind() == io::ErrorKind::WouldBlock
                    || err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(FifoError::Io(err)),
        }
        if start.elapsed() >= timeout {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}

/// §13.1 marker environment, in schema order. `DMUX_DOMAIN` is the empty
/// string when the context has no domain.
fn marker_env(context: &MarkerContext) -> Vec<(&'static str, String)> {
    vec![
        ("DMUX_CONTEXT_VERSION", "1".to_string()),
        ("DMUX_HOST_UID", context.host_uid.0.to_string()),
        ("DMUX_SPACE_UID", context.space_uid.0.to_string()),
        ("DMUX_SPACE_NO", context.space_no.to_string()),
        ("DMUX_BACKEND", context.backend.as_str().to_string()),
        ("DMUX_DOMAIN", context.domain.clone().unwrap_or_default()),
        ("DMUX_SERVER_EPOCH", context.server_epoch.0.to_string()),
        ("DMUX_GROUP_REF", context.group_ref.clone()),
        ("DMUX_SPLIT_REF", context.split_ref.clone()),
    ]
}

/// Timeout-style marker file: also written on protocol violations so an
/// orphan scan always finds a machine-readable reason next to the pane-env.
fn write_marker(paths: &BootstrapPaths, uid: Uuid, reason: &str) {
    let json = serde_json::json!({ "uid": uid, "reason": reason });
    let _ = fs::write(&paths.timeout_marker, json.to_string());
}

fn protocol_violation(paths: &BootstrapPaths, uid: Uuid, reason: &str) -> i32 {
    write_marker(paths, uid, reason);
    eprintln!("dmux pane-bootstrap[{uid}]: {reason}; not running the program");
    EXIT_PROTOCOL
}

// ---------------------------------------------------------------------------
// Escape-sequence emission (ADR 005 §2)

fn osc_title(title: &str) -> String {
    format!("\x1b]2;{title}\x07")
}

fn osc_set_user_var(name: &str, value: &str) -> String {
    format!(
        "\x1b]1337;SetUserVar={name}={}\x07",
        base64(value.as_bytes())
    )
}

/// Frozen tmux passthrough recipe: DCS `tmux;` wrap, every ESC in the
/// payload doubled, terminated with ST (`ESC \`).
fn tmux_wrap(sequence: &str) -> String {
    format!("\x1bPtmux;{}\x1b\\", sequence.replace('\x1b', "\x1b\x1b"))
}

fn emit(sequence: &str, in_tmux: bool) {
    let out = if in_tmux {
        tmux_wrap(sequence)
    } else {
        sequence.to_string()
    };
    let mut stdout = io::stdout().lock();
    let _ = stdout.write_all(out.as_bytes());
    let _ = stdout.flush();
}

/// Standard-alphabet base64 with padding (no new dependency; values are
/// short marker strings).
fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(chunk.get(1).copied().unwrap_or(0)) << 8)
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}
