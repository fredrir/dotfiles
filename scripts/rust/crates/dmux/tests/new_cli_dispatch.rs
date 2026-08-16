//! Hermetic black-box checks for the feature-on public `new` cutover.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};

use tempfile::TempDir;

struct Sandbox {
    data: TempDir,
    bin: TempDir,
    state: TempDir,
    runtime: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let runtime = tempfile::tempdir().unwrap();
        fs::set_permissions(runtime.path(), fs::Permissions::from_mode(0o700)).unwrap();
        Sandbox {
            data: tempfile::tempdir().unwrap(),
            bin: tempfile::tempdir().unwrap(),
            state: tempfile::tempdir().unwrap(),
            runtime,
        }
    }

    fn command(&self, feature_on: bool, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dmux"));
        command
            .args(args)
            .env("PATH", self.bin.path())
            .env("XDG_DATA_HOME", self.data.path())
            .env("XDG_STATE_HOME", self.state.path())
            .env("XDG_RUNTIME_DIR", self.runtime.path())
            .env("DMUX_DRY_RUN", "1")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("DMUX_CONTEXT_VERSION")
            .env_remove("DMUX_BACKEND")
            .env_remove("DMUX_HOST_UID")
            .env_remove("DMUX_SPACE_UID")
            .env_remove("DMUX_SPACE_NO")
            .env_remove("DMUX_DOMAIN")
            .env_remove("DMUX_SERVER_EPOCH")
            .env_remove("DMUX_GROUP_REF")
            .env_remove("DMUX_SPLIT_REF")
            .env_remove("WEZTERM_UNIX_SOCKET")
            .env_remove("WEZTERM_PANE")
            .env_remove("TERM_PROGRAM")
            .env_remove("NO_COLOR");
        if feature_on {
            command.env("DMUX_WEZ_FIRST", "1");
        } else {
            command.env_remove("DMUX_WEZ_FIRST");
        }
        command
    }

    fn run(&self, feature_on: bool, args: &[&str]) -> Output {
        self.command(feature_on, args).output().unwrap()
    }

    fn run_live(&self, feature_on: bool, args: &[&str]) -> Output {
        self.command(feature_on, args)
            .env_remove("DMUX_DRY_RUN")
            .output()
            .unwrap()
    }

    fn registry_exists(&self) -> bool {
        self.data.path().join("dmux/registry.sqlite3").exists()
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn feature_on_dry_run_refuses_before_registry_or_reservation() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(
        true,
        &["new", "project", "--backend", "tmux", "--no-connect"],
    );
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(stderr(&output).contains("cannot preview a Wez-first new operation"));
    assert!(!sandbox.registry_exists());
}

#[test]
fn static_collision_and_launch_usage_fail_before_registry() {
    for args in [
        vec!["new", "project", "--allow-name-collision"],
        vec!["new", "project", "--backend", "tmux", "--launch-gui"],
    ] {
        let sandbox = Sandbox::new();
        let output = sandbox.run(true, &args);
        assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
        assert!(
            stderr(&output).contains("requires explicit --backend")
                || stderr(&output).contains("valid only with the Wez backend")
        );
        assert!(!sandbox.registry_exists());
    }
}

#[test]
fn flag_off_rejects_new_only_flags_but_preserves_positional_legacy_new() {
    let sandbox = Sandbox::new();
    let rejected = sandbox.run(false, &["new", "project", "--no-connect"]);
    assert_eq!(rejected.status.code(), Some(2), "{}", stderr(&rejected));
    assert!(stderr(&rejected).contains("require DMUX_WEZ_FIRST=1"));

    let legacy = sandbox.run(false, &["new", "project"]);
    assert!(legacy.status.success(), "{}", stderr(&legacy));
    assert!(stdout(&legacy).contains("project"), "{}", stdout(&legacy));
    assert!(!sandbox.registry_exists());
}

#[test]
fn dynamic_host_spelling_reaches_typed_authority_resolution() {
    let sandbox = Sandbox::new();
    let output = sandbox.run_live(
        true,
        &[
            "--host",
            "remote-label",
            "new",
            "project",
            "--backend",
            "tmux",
            "--no-connect",
        ],
    );
    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    assert!(stderr(&output).contains("no enrolled host matches"));
    assert!(!stderr(&output).contains("unknown legacy host"));
}

#[test]
fn bounded_no_connect_ignores_bogus_tmux_client_environment() {
    let sandbox = Sandbox::new();
    let output = sandbox
        .command(
            true,
            &["new", "project", "--backend", "tmux", "--no-connect"],
        )
        .env_remove("DMUX_DRY_RUN")
        .env("TMUX", "/bogus/dmux-managed,999999,0")
        .env("TMUX_PANE", "%999999")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(6), "{}", stderr(&output));
    assert!(stderr(&output).contains("no managed tmux instance"));
    assert!(!stderr(&output).contains("tmux client context"));
    assert!(!stderr(&output).contains("wrong backend instance"));
}
