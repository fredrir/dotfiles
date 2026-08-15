//! One glance at everything transport selection depends on.
//!
//! Each line is a fact dmux would otherwise act on silently: which machine
//! this is, whether wezterm and tmux are within reach, whether the cable
//! answers, and what `dmux -` would attach. The slow probes run on their own
//! threads so the whole report costs one ssh timeout, not the sum. `--json`
//! emits the same probes as an object of `name: {ok, detail}` for scripts;
//! the human report is unchanged.

use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::Duration;

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
    ExitCode::SUCCESS
}

fn machine(context: &Context, report: &Report) -> ExitCode {
    let probe = |ok: bool, detail: String| serde_json::json!({ "ok": ok, "detail": detail });
    let (state_ok, state_text) = state_detail(context);
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
}
