//! The legacy default path (`DMUX_WEZ_FIRST` unset) against the managed
//! service: the wezterm probe is pinned to the descriptor's socket, bounded
//! by a dmux-side deadline, and never presents the reserved
//! `dmux:system:<epoch>` sentinel — not as a row, not as an attach target
//! (ADR 012 WS-C; review finding #9; plan §15.1, §21; ADR 001).
//!
//! Hermetic like `tests/cli.rs`: PATH holds a recording `wezterm` stand-in
//! and nothing else, the runtime directory is a scratch `DMUX_RUNTIME_DIR`
//! holding the descriptor under test, and `DMUX_DRY_RUN=1` turns every exec
//! into a printed plan — except where one test deliberately lets the exec
//! reach the stand-in, so the environment the activate carried is witnessed
//! by the process that received it.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use tempfile::TempDir;

/// The epoch the live Macie descriptor carried when finding #9 was
/// reproduced; any UUID would do, this one keeps the repro legible.
const EPOCH: &str = "895ca35a-78ac-4ff7-ae9c-222a9aee3a81";

/// What a shell inside the GUI carries: the GUI's own socket, which is not
/// the service's.
const GUI_SOCKET: &str = "/tmp/gui-sock";

const LIST_ARGV: &str = "argv=cli --no-auto-start list --format json";

struct Sandbox {
    bin: TempDir,
    state: TempDir,
    /// The scratch runtime directory the binary reads the descriptor from.
    /// Mode 0700, current-user-owned: the descriptor reader refuses any
    /// other shape, so a fixture that drifted would show up as "no pin".
    runtime: TempDir,
}

impl Sandbox {
    fn new() -> Sandbox {
        let runtime = tempfile::tempdir().unwrap();
        fs::set_permissions(runtime.path(), fs::Permissions::from_mode(0o700)).unwrap();
        Sandbox {
            bin: tempfile::tempdir().unwrap(),
            state: tempfile::tempdir().unwrap(),
            runtime,
        }
    }

    fn sentinel() -> String {
        format!("dmux:system:{EPOCH}")
    }

    /// The socket the descriptor names. Nothing listens on it — the
    /// stand-in is the server — so its length is of no concern here.
    fn service_socket(&self) -> String {
        self.runtime
            .path()
            .join("wez-dmux.sock")
            .display()
            .to_string()
    }

    /// A descriptor in the shape `mux-startup` writes — the field set the
    /// reader's `deny_unknown_fields` admits — at the mode it requires.
    fn descriptor(&self, state: &str) {
        let text = serde_json::json!({
            "descriptor_version": 1,
            "state": state,
            "epoch": EPOCH,
            "pid": 54528,
            "socket": self.service_socket(),
            "socket_dev": 16777233,
            "socket_ino": 14788383,
            "start_token": "macos:1787136165:343346",
            "backend_instance_uid": null,
            "boot_nonce": "f856b455-b4cd-40cb-ae37-0d0332424485",
            "boot_id": "macos:1787129835:171112",
            "written_by": "mux-startup",
            "written_at": "2026-08-19T10:42:45Z",
        })
        .to_string();
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(self.runtime.path().join("wez-dmux.json"))
            .unwrap();
        file.write_all(text.as_bytes()).unwrap();
    }

    fn stub(&self, name: &str, script: &str) {
        let path = self.bin.path().join(name);
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn witness_path(&self) -> PathBuf {
        self.bin.path().join("wezterm-ran")
    }

    /// A `wezterm` stand-in that records, on every call, the socket it was
    /// handed and its argv, and answers `list` with `panes`. The pin and
    /// the absence of an activate are then proven by what the stand-in
    /// saw, not by trusting the plan dmux printed.
    fn stub_wezterm(&self, panes: &str) {
        let script = format!(
            "printf 'socket=%s %s\\n' \"${{WEZTERM_UNIX_SOCKET-<unset>}}\" \"argv=$*\" >> '{}'\n\
             case \"$*\" in\n  *' list '*) printf '%s' '{panes}' ;;\nesac",
            self.witness_path().display()
        );
        self.stub("wezterm", &script);
    }

    /// One line per call the stand-in received, in order.
    fn witness(&self) -> Vec<String> {
        fs::read_to_string(self.witness_path())
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// The hermetic environment `tests/cli.rs` uses, plus the runtime seam.
    /// Flag-off exactly as that harness states it: `DMUX_WEZ_FIRST` removed
    /// and `DMUX_LEGACY_POLICY=1`, so at the §21 step 9 flip this file keeps
    /// exercising the legacy surface it asserts.
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dmux"));
        command
            .args(args)
            .env("PATH", self.bin.path())
            .env("XDG_DATA_HOME", self.state.path())
            .env("XDG_STATE_HOME", self.state.path())
            .env("XDG_RUNTIME_DIR", self.state.path())
            .env("DMUX_RUNTIME_DIR", self.runtime.path())
            .env("DMUX_DRY_RUN", "1")
            .env("DMUX_LEGACY_POLICY", "1")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("DMUX_WEZ_FIRST")
            .env_remove("WEZTERM_UNIX_SOCKET")
            .env_remove("WEZTERM_PANE")
            .env_remove("TERM_PROGRAM")
            .env_remove("NO_COLOR");
        command
    }

    fn dmux(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("dmux runs")
    }

    /// As a shell inside the GUI sees it (`hosts::Context::resolve`): an
    /// ambient `WEZTERM_UNIX_SOCKET` — the GUI's, not the service's — and
    /// no tmux around to distrust it.
    fn inside_wezterm(&self, args: &[&str]) -> Command {
        let mut command = self.command(args);
        command.env("WEZTERM_UNIX_SOCKET", GUI_SOCKET);
        command
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn rows(output: &Output) -> Vec<serde_json::Value> {
    assert!(output.status.success(), "{}", stderr(output));
    serde_json::from_str::<serde_json::Value>(&stdout(output))
        .unwrap_or_else(|error| panic!("{error}: {}", stdout(output)))
        .as_array()
        .expect("a JSON array")
        .clone()
}

/// `wezterm cli list --format json`, reduced to the fields the listing
/// reads: `(window_id, pane_id, workspace)`.
fn panes(panes: &[(u64, u64, &str)]) -> String {
    let entries: Vec<String> = panes
        .iter()
        .map(|(window, pane, workspace)| {
            format!(r#"{{"window_id":{window},"pane_id":{pane},"workspace":"{workspace}"}}"#)
        })
        .collect();
    format!("[{}]", entries.join(","))
}

/// ADR 012 WS-C regression 1: `dmux ls --json` flag-off against a server
/// holding only the sentinel answers `[]` — in every output shape — and
/// each probe reached that server through the descriptor's socket, with
/// `--no-auto-start` kept.
#[test]
fn flag_off_ls_json_against_a_sentinel_only_server_lists_zero_wez_rows() {
    let sandbox = Sandbox::new();
    sandbox.descriptor("ready");
    let sentinel = Sandbox::sentinel();
    sandbox.stub_wezterm(&panes(&[(1, 1, &sentinel), (1, 2, &sentinel)]));

    let output = sandbox.dmux(&["ls", "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "[]\n");
    assert_eq!(stdout(&sandbox.dmux(&["ls", "--wez", "--json"])), "[]\n");
    assert_eq!(stdout(&sandbox.dmux(&["ls", "--names"])), "");
    assert_eq!(stdout(&sandbox.dmux(&["ls"])), "");

    let calls = sandbox.witness();
    let expected = format!("socket={} {LIST_ARGV}", sandbox.service_socket());
    assert_eq!(calls, [expected.as_str(); 4], "{calls:?}");
}

/// The pin does not wait for `ready`: a `starting` or `failed` descriptor
/// still names the only socket the legacy path may talk to, and the
/// sentinel is filtered from what that socket answers.
#[test]
fn a_starting_or_failed_descriptor_still_pins_the_socket() {
    for state in ["starting", "failed"] {
        let sandbox = Sandbox::new();
        sandbox.descriptor(state);
        sandbox.stub_wezterm(&panes(&[(1, 1, &Sandbox::sentinel()), (2, 2, "work")]));

        let rows = rows(&sandbox.dmux(&["ls", "--json"]));
        assert_eq!(rows.len(), 1, "{state}: {rows:?}");
        assert_eq!(rows[0]["name"], "work", "{state}");
        assert_eq!(rows[0]["index"], 1, "{state}");
        assert_eq!(
            sandbox.witness(),
            [format!("socket={} {LIST_ARGV}", sandbox.service_socket())],
            "{state}"
        );
    }
}

/// With no descriptor nothing is pinned — today's discovery stands, and an
/// ambient GUI socket is what the probe inherits — but the sentinel is
/// filtered whichever server answered.
#[test]
fn without_a_descriptor_the_probe_is_unpinned_and_the_sentinel_still_hidden() {
    let sandbox = Sandbox::new();
    sandbox.stub_wezterm(&panes(&[(1, 1, &Sandbox::sentinel()), (2, 2, "work")]));

    let rows = rows(&sandbox.dmux(&["ls", "--json"]));
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["name"], "work");
    assert_eq!(rows[0]["kind"], "wez");
    assert_eq!(rows[0]["index"], 1);
    assert_eq!(sandbox.witness(), [format!("socket=<unset> {LIST_ARGV}")]);

    let output = sandbox.inside_wezterm(&["ls", "--json"]).output().unwrap();
    let inside = self::rows(&output);
    assert_eq!(inside.len(), 1, "{inside:?}");
    assert_eq!(inside[0]["name"], "work");
    assert_eq!(
        sandbox.witness()[1],
        format!("socket={GUI_SOCKET} {LIST_ARGV}")
    );
}

/// The descriptor's socket wins over the ambient one a shell inside the GUI
/// carries: `WEZTERM_UNIX_SOCKET` is the sole endpoint selector (ADR 001),
/// so leaving the GUI's value in place would list the GUI, not the service.
#[test]
fn the_pin_overrides_the_ambient_gui_socket() {
    let sandbox = Sandbox::new();
    sandbox.descriptor("ready");
    sandbox.stub_wezterm(&panes(&[(1, 1, &Sandbox::sentinel())]));

    let output = sandbox.inside_wezterm(&["ls", "--json"]).output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "[]\n");
    assert_eq!(
        sandbox.witness(),
        [format!("socket={} {LIST_ARGV}", sandbox.service_socket())]
    );
}

/// ADR 012 WS-C regression 2, end to end: the sentinel cannot be attached —
/// not by its name, not by the row index it would have held — and the
/// stand-in records no `activate-pane`; no plan is printed either. The
/// planner's own refusal for a sentinel row is pinned by
/// `attach::tests::the_sentinel_is_refused_before_context_pane_or_exec`;
/// this proves the binary never builds such a row to begin with.
#[test]
fn a_sentinel_cannot_be_attached_and_nothing_is_activated() {
    let sandbox = Sandbox::new();
    sandbox.descriptor("ready");
    let sentinel = Sandbox::sentinel();
    sandbox.stub_wezterm(&panes(&[(1, 1, &sentinel)]));

    for target in [sentinel.as_str(), "1"] {
        let output = sandbox.inside_wezterm(&["con", target]).output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "{target}: {}",
            stderr(&output)
        );
        assert!(
            !stdout(&output).contains("would exec"),
            "{target}: {}",
            stdout(&output)
        );
        assert!(!stderr(&output).is_empty(), "{target}");
    }

    let calls = sandbox.witness();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert!(
        calls.iter().all(|call| call.ends_with(LIST_ARGV)),
        "only listings, never an activate: {calls:?}"
    );
}

/// A real workspace beside the sentinel still attaches by its lowest pane —
/// the dry-run plan is byte-identical to the baseline's — and when the exec
/// is let through to the stand-in, the `activate-pane` it receives carries
/// the descriptor's socket, not the GUI's: the same server the pane id came
/// from.
#[test]
fn a_workspace_attach_execs_activate_pane_on_the_service_socket() {
    let sandbox = Sandbox::new();
    sandbox.descriptor("ready");
    sandbox.stub_wezterm(&panes(&[
        (1, 1, &Sandbox::sentinel()),
        (2, 5, "work"),
        (2, 4, "work"),
    ]));

    let output = sandbox.inside_wezterm(&["con", "work"]).output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "would exec: wezterm cli activate-pane --pane-id 4\n"
    );

    let output = sandbox
        .inside_wezterm(&["con", "work"])
        .env_remove("DMUX_DRY_RUN")
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "");

    let calls = sandbox.witness();
    assert_eq!(calls.len(), 3, "{calls:?}");
    let socket = sandbox.service_socket();
    assert_eq!(calls[0], format!("socket={socket} {LIST_ARGV}"));
    assert_eq!(calls[1], format!("socket={socket} {LIST_ARGV}"));
    assert_eq!(
        calls[2],
        format!("socket={socket} argv=cli activate-pane --pane-id 4")
    );
}

/// ADR 001 rule 1: a wezterm that never answers is abandoned at the
/// dmux-side deadline. The listing comes back empty instead of hanging the
/// shell, and the wedged child goes with it.
#[test]
fn a_wedged_wezterm_is_abandoned_at_the_deadline() {
    let sandbox = Sandbox::new();
    sandbox.descriptor("ready");
    sandbox.stub("wezterm", "exec /bin/sleep 60");

    let started = Instant::now();
    let output = sandbox.dmux(&["ls", "--json"]);
    let elapsed = started.elapsed();
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output), "[]\n");
    assert!(
        elapsed >= Duration::from_secs(10),
        "returned before the 10 s deadline: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(30),
        "the deadline did not bound the probe: {elapsed:?}"
    );
}
