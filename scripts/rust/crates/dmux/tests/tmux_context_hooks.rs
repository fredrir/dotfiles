//! P9 managed-tmux hook seam: hidden-dispatch gating plus the actual tmux
//! 3.7b hook-format split. The live test substitutes only the hidden dmux
//! executable with an argv recorder; tmux itself expands and executes the
//! checked-in managed configuration.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use uuid::Uuid;

fn dmux() -> Command {
    Command::new(env!("CARGO_BIN_EXE_dmux"))
}

#[test]
fn feature_off_hidden_hook_is_a_silent_noop() {
    let output = dmux()
        .args([
            "_tmux-context-refresh",
            "--event=unsupported",
            "--hook-client=",
            "--client-name=",
            "--client-pid=not-a-pid",
            "--client-tty=",
            "--session-id=",
            "--window-id=",
            "--pane-id=",
        ])
        .env_remove("DMUX_WEZ_FIRST")
        .env_remove("DMUX_TMUX_HOOK_DEBUG")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn direct_debug_invocation_rejects_unknown_and_mixed_hook_shapes_once() {
    let scratch = tempfile::tempdir().unwrap();
    let base = [
        "_tmux-context-refresh",
        "--client-name=/dev/pts/7",
        "--client-pid=7",
        "--client-tty=/dev/pts/7",
        "--session-id=$7",
        "--window-id=@7",
        "--pane-id=%7",
        "--namespace=dmux-test",
    ];
    for (event, hook_client, expected_code) in [
        ("window-linked", "", 2),
        ("client-attached", "/dev/pts/8", 4),
        ("after-select-pane", "/dev/pts/7", 4),
    ] {
        let output = dmux()
            .args(base)
            .arg(format!("--event={event}"))
            .arg(format!("--hook-client={hook_client}"))
            .arg("--data-dir")
            .arg(scratch.path())
            .arg("--lock-dir")
            .arg(scratch.path())
            .env("DMUX_WEZ_FIRST", "1")
            .env("DMUX_TMUX_HOOK_DEBUG", "1")
            .env_remove("TMUX")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(expected_code));
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(stderr.lines().count(), 1, "{stderr:?}");
        assert!(stderr.starts_with("dmux _tmux-context-refresh: "));
        assert!(stderr.len() <= 640, "diagnostic was not bounded");
    }
}

struct LiveTmux {
    _scratch: tempfile::TempDir,
    namespace: String,
    capture: PathBuf,
    clients: Vec<Child>,
}

impl LiveTmux {
    fn new() -> Self {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path();
        let capture = root.join("claims.log");
        fs::write(&capture, []).unwrap();
        let recorder = root.join("capture-hook");
        fs::write(
            &recorder,
            b"#!/bin/sh\n/usr/bin/printf '%s\\n' \"$*\" >> \"$DMUX_HOOK_CAPTURE\"\n",
        )
        .unwrap();
        fs::set_permissions(&recorder, fs::Permissions::from_mode(0o700)).unwrap();

        let template =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../shared/tmux/dmux-managed.conf");
        let conf = fs::read_to_string(template).unwrap();
        let conf = conf
            .replace("@DMUX@ _tmux-bootstrap", "/usr/bin/true")
            .replace("@DMUX@ _tmux-context-refresh", recorder.to_str().unwrap());
        let conf_path = root.join("managed.conf");
        fs::write(&conf_path, conf).unwrap();

        let namespace = format!("dmux-hook-live-{}", Uuid::new_v4().simple());
        let output = Command::new("tmux")
            .args(["-L", &namespace, "-f"])
            .arg(&conf_path)
            .args(["new-session", "-d", "-s", "one"])
            .env("DMUX_HOOK_CAPTURE", &capture)
            .env("DMUX_WEZ_FIRST", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "starting live tmux: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let this = Self {
            _scratch: scratch,
            namespace,
            capture,
            clients: Vec::new(),
        };
        assert!(this.tmux(&["new-window", "-d", "-t", "one", "-n", "two"]));
        assert!(this.tmux(&["split-window", "-d", "-t", "one:0"]));
        assert!(this.tmux(&["split-window", "-d", "-t", "one:1"]));
        assert!(this.tmux(&["new-session", "-d", "-s", "other"]));
        assert!(this.tmux(&["split-window", "-d", "-t", "other:0"]));
        // Ignore server/setup hooks; the assertions below start with real
        // attached client events.
        fs::write(&this.capture, []).unwrap();
        this
    }

    fn tmux(&self, args: &[&str]) -> bool {
        Command::new("tmux")
            .args(["-L", &self.namespace])
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    }

    fn attach(&mut self, session: &str) -> usize {
        let mut command = Command::new("script");
        command
            .env("DMUX_HOOK_TEST_NAMESPACE", &self.namespace)
            .env("DMUX_HOOK_TEST_SESSION", session)
            .env("TERM", "xterm-256color");
        #[cfg(target_os = "macos")]
        command.args([
            "-q",
            "/dev/null",
            "env",
            "sh",
            "-c",
            "exec tmux -L \"$DMUX_HOOK_TEST_NAMESPACE\" attach-session -t \"$DMUX_HOOK_TEST_SESSION\"",
        ]);
        #[cfg(target_os = "linux")]
        command.args([
            "-q",
            "-c",
            "exec tmux -L \"$DMUX_HOOK_TEST_NAMESPACE\" attach-session -t \"$DMUX_HOOK_TEST_SESSION\"",
            "/dev/null",
        ]);
        let child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        self.clients.push(child);
        let expected = self.clients.len();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.client_count_is(expected) {
            assert!(Instant::now() < deadline, "live tmux client did not attach");
            thread::sleep(Duration::from_millis(20));
        }
        expected - 1
    }

    fn client_count_is(&self, expected: usize) -> bool {
        let output = Command::new("tmux")
            .args(["-L", &self.namespace, "list-clients", "-F", "#{client_pid}"])
            .output();
        output.is_ok_and(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).lines().count() == expected
        })
    }

    fn send(&mut self, index: usize, bytes: &[u8]) {
        self.clients[index]
            .stdin
            .as_mut()
            .unwrap()
            .write_all(bytes)
            .unwrap();
        self.clients[index].stdin.as_mut().unwrap().flush().unwrap();
    }

    fn lines(&self) -> Vec<ClaimLine> {
        fs::read_to_string(&self.capture)
            .unwrap()
            .lines()
            .map(ClaimLine::parse)
            .collect()
    }

    fn wait_for<F>(&self, start: usize, predicate: F) -> ClaimLine
    where
        F: Fn(&ClaimLine) -> bool,
    {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(line) = self.lines().into_iter().skip(start).find(&predicate) {
                return line;
            }
            assert!(
                Instant::now() < deadline,
                "expected tmux hook was not captured"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for LiveTmux {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.namespace, "kill-server"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        for child in &mut self.clients {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[derive(Debug, Clone)]
struct ClaimLine(BTreeMap<String, String>);

impl ClaimLine {
    fn parse(line: &str) -> Self {
        let fields = line
            .split_ascii_whitespace()
            .map(|field| {
                let (name, value) = field
                    .strip_prefix("--")
                    .and_then(|field| field.split_once('='))
                    .unwrap_or_else(|| panic!("malformed captured hook argument {field:?}"));
                (name.to_string(), value.to_string())
            })
            .collect();
        Self(fields)
    }

    fn get(&self, key: &str) -> &str {
        self.0.get(key).map(String::as_str).unwrap_or("")
    }

    fn assert_canonical(&self) {
        assert!(self.get("client-name").starts_with("/dev/"), "{self:?}");
        assert_eq!(self.get("client-tty"), self.get("client-name"));
        assert!(
            self.get("client-pid")
                .parse::<u32>()
                .is_ok_and(|pid| pid > 0),
            "{self:?}"
        );
        assert_native_id(self.get("session-id"), '$');
        assert_native_id(self.get("window-id"), '@');
        assert_native_id(self.get("pane-id"), '%');
    }
}

fn assert_native_id(value: &str, prefix: char) {
    assert_eq!(value.chars().next(), Some(prefix), "{value:?}");
    assert!(value[1..].bytes().all(|byte| byte.is_ascii_digit()));
    assert!(value.len() > 1);
}

#[test]
fn managed_config_passes_the_real_tmux_37_client_and_move_hook_shapes() {
    let mut live = LiveTmux::new();
    let first = live.attach("one");
    let attached = live.wait_for(0, |line| line.get("event") == "client-attached");
    attached.assert_canonical();
    assert_eq!(attached.get("hook-client"), attached.get("client-name"));
    let first_pid = attached.get("client-pid").to_string();

    let session_changed = live.wait_for(0, |line| {
        line.get("event") == "client-session-changed" && line.get("client-pid") == first_pid
    });
    session_changed.assert_canonical();
    assert_eq!(
        session_changed.get("hook-client"),
        session_changed.get("client-name")
    );

    // A second live client makes "latest active client" transitions
    // observable. Input to the first must name the first exact tuple.
    let _second = live.attach("one");
    let start = live.lines().len();
    live.send(first, b"\x02");
    let active = live.wait_for(start, |line| {
        line.get("event") == "client-active" && line.get("client-pid") == first_pid
    });
    active.assert_canonical();
    assert_eq!(active.get("hook-client"), active.get("client-name"));
    live.send(first, b"\x07"); // cancel the pending prefix command

    let start = live.lines().len();
    live.send(first, b"\x02n");
    let selected_window = live.wait_for(start, |line| {
        line.get("event") == "after-select-window" && line.get("client-pid") == first_pid
    });
    selected_window.assert_canonical();
    assert_eq!(selected_window.get("hook-client"), "");

    let start = live.lines().len();
    live.send(first, b"\x02o");
    let selected_pane = live.wait_for(start, |line| {
        line.get("event") == "after-select-pane" && line.get("client-pid") == first_pid
    });
    selected_pane.assert_canonical();
    assert_eq!(selected_pane.get("hook-client"), "");

    // break-pane is tmux's native pane-to-new-window move. 3.7b reports it
    // through session-window-changed, not after-select-window.
    let start = live.lines().len();
    live.send(first, b"\x02!");
    let moved = live.wait_for(start, |line| {
        line.get("event") == "session-window-changed" && line.get("client-pid") == first_pid
    });
    moved.assert_canonical();
    assert_eq!(moved.get("hook-client"), "");

    // Explicit exact-client session switching retains the client-hook
    // shape; no ordinal or active-client selection is involved.
    let start = live.lines().len();
    assert!(live.tmux(&[
        "switch-client",
        "-c",
        attached.get("client-name"),
        "-t",
        "other",
    ]));
    let switched = live.wait_for(start, |line| {
        line.get("event") == "client-session-changed"
            && line.get("client-pid") == first_pid
            && line.get("session-id") != attached.get("session-id")
    });
    switched.assert_canonical();
    assert_eq!(switched.get("hook-client"), switched.get("client-name"));
}
