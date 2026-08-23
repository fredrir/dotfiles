//! `dmux doctor` names the backend-instance state (ADR 012 WS-B.4; plan
//! §5.2 as amended; review finding #14).
//!
//! Both operator-facing epoch remedies end "re-run `dmux doctor`", and doctor
//! used to report green while the registry named a dead pid. These drive the
//! real binary against a scratch registry under `XDG_DATA_HOME` and a scratch
//! runtime directory under `DMUX_RUNTIME_DIR`, with every external probe
//! stubbed on PATH (no ssh leaves this machine), and assert that the JSON
//! document names a synthetic state-F row `"F"`/`stale_incarnation` and a
//! synthetic state-E row `"E"`, under both `--format json` and the
//! deprecated bare `--json` shape — and that doctor opened the registry
//! read-only (the live file's mtime and contents are untouched).

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use dmux::model::{Backend, ServerEpoch};
use dmux::registry::{Registry, RegistryConfig};
use serde_json::Value;
use uuid::Uuid;

struct Sandbox {
    home: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Sandbox {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join("dmux")).unwrap();
        fs::create_dir_all(home.path().join("rt")).unwrap();
        fs::set_permissions(home.path().join("rt"), fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir_all(home.path().join("bin")).unwrap();
        let sandbox = Sandbox { home };
        // No probe leaves this machine: ssh refuses at once, tmux and
        // wezterm answer nothing unless a test installs its own stub.
        sandbox.stub("ssh", "exit 255");
        sandbox.stub("tmux", "exit 1");
        sandbox.stub("wezterm", "exit 1");
        sandbox
    }

    fn stub(&self, name: &str, script: &str) -> PathBuf {
        let path = self.home.path().join("bin").join(name);
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn db_path(&self) -> PathBuf {
        self.home.path().join("dmux/registry.sqlite3")
    }

    fn runtime_dir(&self) -> PathBuf {
        self.home.path().join("rt")
    }

    fn registry(&self) -> Registry {
        Registry::open(RegistryConfig::new(
            self.db_path(),
            self.home.path().join("seed-locks"),
        ))
        .unwrap()
    }

    fn doctor(&self, args: &[&str]) -> Output {
        let path = std::env::join_paths([
            self.home.path().join("bin"),
            PathBuf::from("/usr/bin"),
            PathBuf::from("/bin"),
        ])
        .unwrap();
        Command::new(env!("CARGO_BIN_EXE_dmux"))
            .args(args)
            .env("PATH", path)
            .env("HOME", self.home.path())
            .env("XDG_DATA_HOME", self.home.path())
            .env("XDG_STATE_HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.home.path().join("config"))
            .env("DMUX_RUNTIME_DIR", self.runtime_dir())
            .env_remove("DMUX_WEZ_FIRST")
            .env_remove("DMUX_WEZ_BIN")
            .env_remove("DMUX_WEZ_CONFIG")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("WEZTERM_UNIX_SOCKET")
            .env_remove("WEZTERM_PANE")
            .env_remove("TERM_PROGRAM")
            .stdin(Stdio::null())
            .output()
            .expect("dmux runs")
    }
}

fn document(out: &Output) -> Value {
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "doctor exits 0: {text} {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(text.trim().lines().count(), 1, "not one document: {text:?}");
    serde_json::from_str(text.trim()).unwrap_or_else(|e| panic!("{e}: {text}"))
}

fn instance_row<'a>(rows: &'a Value, backend: &str) -> &'a Value {
    rows.as_array()
        .unwrap_or_else(|| panic!("backend_instances is an array: {rows}"))
        .iter()
        .find(|row| row["backend"] == backend)
        .unwrap_or_else(|| panic!("no {backend} row in {rows}"))
}

/// A pid nothing holds: spawn and reap a child, then use its pid.
fn dead_pid() -> i64 {
    let child = Command::new("true").spawn().unwrap();
    let pid = i64::from(child.id());
    let _ = child.wait_with_output();
    pid
}

/// Macie's shape (ADR 012 §3.1): the registry publishes an epoch against a
/// pid that has exited, with the socket witnesses of a socket that is gone.
#[test]
fn a_published_incarnation_whose_process_is_dead_is_reported_as_state_f() {
    let sandbox = Sandbox::new();
    let epoch = ServerEpoch(Uuid::new_v4());
    let socket = sandbox.runtime_dir().join("wez-dmux.sock");
    let pid = dead_pid();
    {
        let mut registry = sandbox.registry();
        let instance = registry
            .register_backend_instance(Backend::Wez, Some(socket.to_str().unwrap()), None)
            .unwrap();
        registry
            .publish_backend_server(
                instance,
                epoch,
                Some(pid),
                Some("macos:1787009464:403265"),
                Some(16777231),
                Some(10519741),
            )
            .unwrap();
    }
    let db_before = fs::metadata(sandbox.db_path()).unwrap();
    let bytes_before = fs::read(sandbox.db_path()).unwrap();

    let doc = document(&sandbox.doctor(&["--format", "json", "doctor"]));
    assert_eq!(doc["action"], "doctor");
    assert_eq!(doc["ok"], true, "doctor reports, it does not fail: {doc}");
    let wez = instance_row(&doc["result"]["backend_instances"], "wez");
    assert_eq!(wez["state"], "F", "{wez}");
    assert_eq!(wez["state_name"], "stale_incarnation", "{wez}");
    assert_eq!(wez["published"]["epoch"], epoch.0.to_string(), "{wez}");
    assert_eq!(wez["published"]["pid"], pid, "{wez}");
    assert_eq!(wez["observed"]["process"], "dead", "{wez}");
    let detail = wez["detail"].as_str().unwrap();
    assert!(detail.contains("stale incarnation"), "{detail}");
    assert!(
        detail.contains(&format!("process {pid} is dead")),
        "{detail}"
    );
    let remedy = wez["remedy"].as_str().unwrap();
    assert!(
        remedy.contains(&format!(
            "dmux repair retire-incarnation --backend wez --epoch {}",
            epoch.0
        )),
        "{remedy}"
    );
    assert!(remedy.contains("holds no user panes"), "{remedy}");
    let tmux = instance_row(&doc["result"]["backend_instances"], "tmux");
    assert_eq!(tmux["state"], "A", "{tmux}");
    assert_eq!(tmux["state_name"], "not_registered", "{tmux}");

    // The seam is reported, as a finding, with the directory it redirects to.
    assert_eq!(doc["result"]["runtime_dir"]["ok"], false, "{doc}");
    let runtime = doc["result"]["runtime_dir"]["detail"].as_str().unwrap();
    assert!(
        runtime.starts_with(sandbox.runtime_dir().to_str().unwrap())
            && runtime.contains("DMUX_RUNTIME_DIR="),
        "{runtime}"
    );
    assert_eq!(doc["result"]["registry_snapshot"]["ok"], true, "{doc}");
    assert!(
        doc["result"]["registry_snapshot"]["detail"]
            .as_str()
            .unwrap()
            .contains("read-only snapshot"),
        "{doc}"
    );
    assert_eq!(
        doc["result"]["ssh_peer"]["ok"], false,
        "the stub ssh refused, so nothing left this machine: {doc}"
    );

    // The deprecated bare shape carries the same array.
    let bare = document(&sandbox.doctor(&["doctor", "--json"]));
    assert_eq!(
        instance_row(&bare["backend_instances"], "wez")["state"],
        "F"
    );
    assert_eq!(bare["runtime_dir"]["ok"], false);

    // Read-only: the live registry file is byte-identical and untouched.
    let db_after = fs::metadata(sandbox.db_path()).unwrap();
    assert_eq!(
        db_after.mtime(),
        db_before.mtime(),
        "doctor wrote the registry"
    );
    assert_eq!(fs::read(sandbox.db_path()).unwrap(), bytes_before);

    // Human output has one line per instance naming the state.
    let human = sandbox.doctor(&["doctor"]);
    assert!(human.status.success());
    let text = String::from_utf8_lossy(&human.stdout);
    let line = text
        .lines()
        .find(|line| line.starts_with("wez instance"))
        .unwrap_or_else(|| panic!("no wez instance line in {text}"));
    assert!(line.contains("F (stale_incarnation)"), "{line}");
    assert!(
        text.lines()
            .any(|line| line.starts_with("tmux instance") && line.contains("A (not_registered)")),
        "{text}"
    );
    assert!(
        text.contains("remedy: the published incarnation is stale"),
        "{text}"
    );
}

/// A published incarnation the host agrees with — this process's pid and
/// OS start witness, the socket it bound, a `wezterm` that answers the
/// sentinel of the published epoch — is state E, and the inventory under
/// the published epoch is reported with it.
#[test]
fn a_published_incarnation_the_host_agrees_with_is_reported_as_state_e() {
    let sandbox = Sandbox::new();
    let epoch = ServerEpoch(Uuid::new_v4());
    let socket = sandbox.runtime_dir().join("wez-dmux.sock");
    let _listener = UnixListener::bind(&socket).unwrap();
    let meta = fs::metadata(&socket).unwrap();
    let pid = i64::from(std::process::id());
    let token = dmux::runtime::process_start_token_for_pid(std::process::id()).unwrap();
    {
        let mut registry = sandbox.registry();
        let instance = registry
            .register_backend_instance(Backend::Wez, Some(socket.to_str().unwrap()), None)
            .unwrap();
        registry
            .publish_backend_server(
                instance,
                epoch,
                Some(pid),
                Some(&token),
                Some(meta.dev() as i64),
                Some(meta.ino() as i64),
            )
            .unwrap();
    }
    let wezterm = sandbox.stub(
        "wezterm",
        &format!(
            r#"printf '%s' '[{{"window_id":0,"tab_id":0,"pane_id":0,"workspace":"dmux:system:{}"}},{{"window_id":1,"tab_id":1,"pane_id":1,"workspace":"dmux:ws:dotfiles"}}]'"#,
            epoch.0
        ),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_dmux"))
        .args(["--format", "json", "doctor"])
        .env(
            "PATH",
            std::env::join_paths([
                sandbox.home.path().join("bin"),
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin"),
            ])
            .unwrap(),
        )
        .env("HOME", sandbox.home.path())
        .env("XDG_DATA_HOME", sandbox.home.path())
        .env("XDG_STATE_HOME", sandbox.home.path())
        .env("XDG_CONFIG_HOME", sandbox.home.path().join("config"))
        .env("DMUX_RUNTIME_DIR", sandbox.runtime_dir())
        .env("DMUX_WEZ_BIN", &wezterm)
        .env("DMUX_WEZ_CONFIG", "/dev/null")
        .env_remove("DMUX_WEZ_FIRST")
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .env_remove("WEZTERM_UNIX_SOCKET")
        .env_remove("WEZTERM_PANE")
        .env_remove("TERM_PROGRAM")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let doc = document(&out);
    let wez = instance_row(&doc["result"]["backend_instances"], "wez");
    assert_eq!(wez["state"], "E", "{wez}");
    assert_eq!(wez["state_name"], "published_live", "{wez}");
    assert_eq!(wez["observed"]["process"], "alive", "{wez}");
    assert_eq!(wez["observed"]["socket"]["ino"], meta.ino(), "{wez}");
    assert_eq!(wez["observed"]["inventory"]["outcome"], "complete", "{wez}");
    assert_eq!(
        wez["observed"]["inventory"]["server_epoch"],
        epoch.0.to_string(),
        "{wez}"
    );
    assert_eq!(wez["observed"]["inventory"]["rows"], 1, "{wez}");
    assert_eq!(
        wez["observed"]["inventory"]["native_names"][0],
        "dmux:ws:dotfiles"
    );
    assert!(
        wez["observed"]["descriptor"].is_null(),
        "no descriptor was written: {wez}"
    );
    assert!(wez["remedy"].is_null(), "{wez}");
    assert!(
        wez["detail"].as_str().unwrap().contains("the host agrees"),
        "{wez}"
    );
}
