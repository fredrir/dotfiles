//! One glance at everything transport selection depends on.
//!
//! Each line is a fact dmux would otherwise act on silently: which machine
//! this is, whether wezterm and tmux are within reach, whether the cable
//! answers, and what `dmux -` would attach. The slow probes run on their own
//! threads so the whole report costs one ssh timeout, not the sum.

use std::process::{Command, ExitCode, Stdio};
use std::thread;

use workstation::Style;

use crate::hosts::{self, Context, Host};
use crate::state;

pub fn run(context: &Context) -> ExitCode {
    let Ok(this) = Host::this() else {
        return ExitCode::FAILURE;
    };
    let peer = this.peer();
    let wezterm = thread::spawn(wezterm_ok);
    let tmux = thread::spawn(tmux_sessions);
    let usb = thread::spawn(|| hosts::usb_latency(hosts::PROBE_TIMEOUT));
    let peer_name = peer.name();
    let ssh = thread::spawn(move || ssh_ok(peer_name));

    let style = Style::for_stdout();
    let line = |label: &str, value: String| println!("{label:<15} {value}");
    line(
        "host",
        format!("{} ({})", style.green(this.name()), std::env::consts::OS),
    );
    line(
        "peer",
        style.dim(&format!(
            "{} (usb {}, ts {})",
            peer.name(),
            peer.usb_address(),
            peer.ts_address()
        )),
    );
    line("inside wezterm", yes_no(&style, context.inside_wezterm));
    line("inside tmux", yes_no(&style, context.inside_tmux));
    line(
        "wezterm cli",
        match wezterm.join().unwrap_or(false) {
            true => style.green("reachable"),
            false => style.red("unreachable"),
        },
    );
    line(
        "tmux server",
        match tmux.join().unwrap_or(None) {
            Some(1) => style.green("running (1 session)"),
            Some(count) => style.green(&format!("running ({count} sessions)")),
            None => style.red("not running"),
        },
    );
    line(
        "usb link",
        match usb.join().unwrap_or(None) {
            Some(latency) => style.green(&format!("up ({} ms)", latency.as_millis())),
            None => style.red("down"),
        },
    );
    line(
        &format!("ssh {}", peer.name()),
        match ssh.join().unwrap_or(false) {
            true => style.green("reachable"),
            false => style.red("unreachable"),
        },
    );
    line(
        "state",
        match state::file() {
            Some(path) => style.dim(&format!(
                "{} (last on {}: {})",
                path.display(),
                context.host.name(),
                state::previous(context.host).unwrap_or_else(|| "nothing".to_string())
            )),
            None => style.red("unavailable (no HOME)"),
        },
    );
    ExitCode::SUCCESS
}

fn yes_no(style: &Style, answer: bool) -> String {
    if answer {
        style.green("yes")
    } else {
        style.red("no")
    }
}

fn wezterm_ok() -> bool {
    Command::new("wezterm")
        .args(["cli", "list", "--format", "json"])
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
