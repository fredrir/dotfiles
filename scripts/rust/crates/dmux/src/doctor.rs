//! One glance at everything transport selection depends on.
//!
//! Each line is a fact dmux would otherwise act on silently: which machine
//! this is, whether wezterm and tmux are within reach, whether the cable
//! answers, what `dmux -` would attach, and who besides this user can reach
//! the registry directory. The slow probes run on their own threads so the
//! whole report costs one ssh timeout, not the sum. `--json` emits the same
//! probes as an object of `name: {ok, detail}` for scripts; the human report
//! is unchanged.

use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::Duration;

use dmux::registry::{self, DirExposure};
use workstation::Style;

use crate::hosts::{self, Context, Host};
use crate::state;

struct Report {
    this: Host,
    peer: Host,
    wezterm: bool,
    tmux: Option<usize>,
    usb: Option<Duration>,
    ssh: bool,
}

pub fn run(context: &Context, json: bool) -> ExitCode {
    let Ok(this) = Host::this() else {
        return ExitCode::FAILURE;
    };
    let peer = this.peer();
    let wezterm = thread::spawn(wezterm_ok);
    let tmux = thread::spawn(tmux_sessions);
    let usb = thread::spawn(|| hosts::usb_latency(hosts::PROBE_TIMEOUT));
    let peer_name = peer.name();
    let ssh = thread::spawn(move || ssh_ok(peer_name));
    let report = Report {
        this,
        peer,
        wezterm: wezterm.join().unwrap_or(false),
        tmux: tmux.join().unwrap_or(None),
        usb: usb.join().unwrap_or(None),
        ssh: ssh.join().unwrap_or(false),
    };
    if json {
        machine(context, &report)
    } else {
        human(context, &report)
    }
}

fn human(context: &Context, report: &Report) -> ExitCode {
    let style = Style::for_stdout();
    let line = |label: &str, value: String| println!("{label:<15} {value}");
    let status = |ok: bool, detail: &str| {
        if ok {
            style.green(detail)
        } else {
            style.red(detail)
        }
    };
    line(
        "host",
        format!(
            "{} ({})",
            style.green(report.this.name()),
            std::env::consts::OS
        ),
    );
    line("peer", style.dim(&peer_detail(report.peer)));
    line(
        "inside wezterm",
        status(context.inside_wezterm, yes_no(context.inside_wezterm)),
    );
    line(
        "inside tmux",
        status(context.inside_tmux, yes_no(context.inside_tmux)),
    );
    line(
        "wezterm cli",
        status(report.wezterm, reachable(report.wezterm)),
    );
    line(
        "tmux server",
        status(report.tmux.is_some(), &tmux_detail(report.tmux)),
    );
    line(
        "usb link",
        status(report.usb.is_some(), &usb_detail(report.usb)),
    );
    line(
        &format!("ssh {}", report.peer.name()),
        status(report.ssh, reachable(report.ssh)),
    );
    let (state_ok, state_text) = state_detail(context);
    line(
        "state",
        if state_ok {
            style.dim(&state_text)
        } else {
            style.red(&state_text)
        },
    );
    let (registry_ok, registry_text) = registry_detail();
    line(
        "registry dir",
        if registry_ok {
            style.dim(&registry_text)
        } else {
            style.red(&registry_text)
        },
    );
    ExitCode::SUCCESS
}

fn machine(context: &Context, report: &Report) -> ExitCode {
    let probe = |ok: bool, detail: String| serde_json::json!({ "ok": ok, "detail": detail });
    let (state_ok, state_text) = state_detail(context);
    let (registry_ok, registry_text) = registry_detail();
    let probes = serde_json::json!({
        "host": probe(
            true,
            format!("{} ({})", report.this.name(), std::env::consts::OS)
        ),
        "peer": probe(true, peer_detail(report.peer)),
        "inside_wezterm": probe(context.inside_wezterm, yes_no(context.inside_wezterm).to_string()),
        "inside_tmux": probe(context.inside_tmux, yes_no(context.inside_tmux).to_string()),
        "wezterm_cli": probe(report.wezterm, reachable(report.wezterm).to_string()),
        "tmux_server": probe(report.tmux.is_some(), tmux_detail(report.tmux)),
        "usb_link": probe(report.usb.is_some(), usb_detail(report.usb)),
        "ssh_peer": probe(report.ssh, reachable(report.ssh).to_string()),
        "state": probe(state_ok, state_text),
        "registry_dir": probe(registry_ok, registry_text),
    });
    println!("{probes}");
    ExitCode::SUCCESS
}

fn yes_no(answer: bool) -> &'static str {
    if answer { "yes" } else { "no" }
}

fn reachable(ok: bool) -> &'static str {
    if ok { "reachable" } else { "unreachable" }
}

fn peer_detail(peer: Host) -> String {
    format!(
        "{} (usb {}, ts {})",
        peer.name(),
        peer.usb_address(),
        peer.ts_address()
    )
}

fn tmux_detail(sessions: Option<usize>) -> String {
    match sessions {
        Some(1) => "running (1 session)".to_string(),
        Some(count) => format!("running ({count} sessions)"),
        None => "not running".to_string(),
    }
}

fn usb_detail(latency: Option<Duration>) -> String {
    match latency {
        Some(latency) => format!("up ({} ms)", latency.as_millis()),
        None => "down".to_string(),
    }
}

fn state_detail(context: &Context) -> (bool, String) {
    match state::file() {
        Some(path) => (
            true,
            format!(
                "{} (last on {}: {})",
                path.display(),
                context.host.name(),
                state::previous(context.host).unwrap_or_else(|| "nothing".to_string())
            ),
        ),
        None => (false, "unavailable (no HOME)".to_string()),
    }
}

/// Who besides this user can reach the directory the registry sits in.
///
/// The registry file is `0600` and re-hardened on every open, but that only
/// closes the *contents*: a group- or world-traversable parent still leaks
/// the database's existence and name, its `-wal`/`-shm` sidecars and the
/// lock filenames, and a writable one lets another uid put files beside it.
/// The mode of a directory the user already had is deliberately never forced
/// — `--data-dir X` makes X itself the parent — so this is where that
/// decision becomes visible instead of silent. Reported, never repaired:
/// doctor is a report.
///
/// Only the production location is inspected; `doctor` takes no `--data-dir`
/// and never opens the registry, so this costs a couple of `stat` calls and
/// creates nothing.
fn registry_detail() -> (bool, String) {
    let Some(db_path) = registry::production_db_path() else {
        return (false, "unavailable (no HOME)".to_string());
    };
    match registry::parent_dir_exposure(&db_path) {
        Ok(exposure) => (exposure.is_private(), dir_detail(&exposure)),
        Err(error) => (
            false,
            format!(
                "{} (unreadable: {error})",
                db_path.parent().unwrap_or(&db_path).display()
            ),
        ),
    }
}

fn dir_detail(exposure: &DirExposure) -> String {
    format!("{} ({})", exposure.dir.display(), exposure.summary())
}

fn wezterm_ok() -> bool {
    Command::new("wezterm")
        .args(["cli", "--no-auto-start", "list", "--format", "json"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn tmux_sessions() -> Option<usize> {
    let output = Command::new("tmux")
        .args(["list-sessions", "-F", "#{session_name}"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).lines().count())
}

fn ssh_ok(peer: &str) -> bool {
    Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=2",
            peer,
            "true",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn details_spell_out_each_probe_state() {
        assert_eq!(tmux_detail(None), "not running");
        assert_eq!(tmux_detail(Some(1)), "running (1 session)");
        assert_eq!(tmux_detail(Some(3)), "running (3 sessions)");
        assert_eq!(usb_detail(None), "down");
        assert_eq!(usb_detail(Some(Duration::from_millis(7))), "up (7 ms)");
        assert_eq!(yes_no(true), "yes");
        assert_eq!(reachable(false), "unreachable");
        assert!(peer_detail(Host::Archie).starts_with("archie (usb 10.77.77.2"));
    }

    /// The registry line names the directory and what its mode grants, and
    /// it is a finding only when another uid can actually get in. `/tmp`
    /// rather than `$TMPDIR`: on macOS the per-user temp dir is 0700, which
    /// would make every parent unreachable and the assertion vacuous.
    #[test]
    fn the_registry_line_names_the_directory_and_what_its_mode_grants() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::Builder::new()
            .prefix("dmux-doctor-")
            .tempdir_in("/tmp")
            .unwrap();
        let db = dir.path().join("registry.sqlite3");
        let chmod = |mode: u32| {
            std::fs::set_permissions(dir.path(), PermissionsExt::from_mode(mode)).unwrap()
        };

        chmod(0o700);
        let private = registry::parent_dir_exposure(&db).unwrap();
        assert!(private.is_private());
        assert_eq!(
            dir_detail(&private),
            format!("{} (0700, private)", dir.path().display())
        );

        chmod(0o755);
        let exposed = registry::parent_dir_exposure(&db).unwrap();
        assert!(
            !exposed.is_private(),
            "/tmp must be traversable for this assertion to mean anything"
        );
        assert_eq!(
            dir_detail(&exposed),
            format!(
                "{} (0755, any local user can enter and list)",
                dir.path().display()
            )
        );
        chmod(0o700);
    }

    /// Whatever the environment, the probe answers without panicking and
    /// never creates or repairs anything.
    #[test]
    fn the_registry_probe_is_read_only_and_always_answers() {
        let db = registry::production_db_path();
        let before = db.as_ref().map(|path| path.exists());
        let (_ok, detail) = registry_detail();
        assert!(!detail.is_empty());
        assert_eq!(
            db.as_ref().map(|path| path.exists()),
            before,
            "the probe must not create the registry"
        );
        if let Some(db) = db {
            assert!(
                detail.contains(&db.parent().unwrap().display().to_string())
                    || detail.contains("unreadable"),
                "{detail}"
            );
        }
    }
}
