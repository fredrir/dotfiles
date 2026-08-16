//! Black-box checks at the feature-on public `con`/shorthand cutover.
//!
//! Every feature-on validation case stops before owner resolution.  The
//! sandbox pins all persistent paths and asserts that no registry was opened,
//! so these tests cannot consult or mutate the invoking user's dmux state.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};

use dmux::model::HostUid;
use dmux::registry::{Registry, RegistryConfig};
use tempfile::TempDir;
use uuid::Uuid;

struct Sandbox {
    root: TempDir,
    bin: TempDir,
    state: TempDir,
    runtime: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let runtime = tempfile::tempdir().unwrap();
        fs::set_permissions(runtime.path(), fs::Permissions::from_mode(0o700)).unwrap();
        Sandbox {
            root: tempfile::tempdir().unwrap(),
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
            .env("XDG_DATA_HOME", self.root.path())
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
        self.command(feature_on, args).output().expect("dmux runs")
    }

    fn run_without_dry_run(&self, feature_on: bool, args: &[&str]) -> Output {
        self.command(feature_on, args)
            .env_remove("DMUX_DRY_RUN")
            .output()
            .expect("dmux runs")
    }

    fn stub_tmux(&self) {
        let path = self.bin.path().join("tmux");
        fs::write(
            &path,
            "#!/bin/sh\ncase \"$1\" in\nlist-sessions) printf 'alpha|1700000000|1|0\\n' ;;\nesac\n",
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn assert_registry_untouched(&self) {
        assert!(
            !self.root.path().join("dmux/registry.sqlite3").exists(),
            "CLI-only validation unexpectedly opened the owner registry"
        );
    }

    fn enroll_unlabelled_peer(&self) {
        let data_dir = self.root.path().join("dmux");
        let lock_dir = self.runtime.path().join("dmux");
        fs::create_dir_all(&data_dir).unwrap();
        fs::create_dir_all(&lock_dir).unwrap();
        fs::set_permissions(&lock_dir, fs::Permissions::from_mode(0o700)).unwrap();
        let mut registry = Registry::open(RegistryConfig::new(
            data_dir.join("registry.sqlite3"),
            lock_dir,
        ))
        .unwrap();
        let enrolled = registry
            .enroll_host(
                HostUid(Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap()),
                None,
            )
            .unwrap();
        assert_eq!(enrolled.alias, "b");
        assert_eq!(enrolled.label, None);
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn feature_on_refuses_legacy_create_and_native_window_before_owner_resolution() {
    let sandbox = Sandbox::new();
    for args in [
        &["con", "2", "--create"][..],
        &["con", "2", "-A"][..],
        &["con", "2", "--window", "1"][..],
        &["con", "2", "-w1"][..],
    ] {
        let output = sandbox.run(true, args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
    sandbox.assert_registry_untouched();
}

#[test]
fn feature_on_clap_boundary_rejects_ambiguous_selector_backend_and_child_options() {
    let sandbox = Sandbox::new();
    for args in [
        &["con"][..],
        &["con", "2", "--name", "project"][..],
        &["con", "2", "--backend", "bogus"][..],
        &["con", "2", "--group", "bad", "--split", "bad"][..],
    ] {
        let output = sandbox.run(true, args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
    sandbox.assert_registry_untouched();
}

#[test]
fn feature_on_standalone_child_validation_is_typed_and_pre_resolution() {
    let sandbox = Sandbox::new();
    let epoch = "33333333-3333-4333-8333-333333333333";
    for args in [
        vec!["con", "2", "--group", "not-a-child"],
        vec!["con", "2", "--group", &format!("p{epoch}.tx-9")],
        vec!["con", "2", "--split", &format!("p{epoch}.tx-09")],
    ] {
        let output = sandbox.run(true, &args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(stderr(&output).contains("child ref"), "{args:?}");
    }
    sandbox.assert_registry_untouched();
}

#[test]
fn feature_on_tmux_gui_contradiction_uses_the_typed_usage_exit() {
    let sandbox = Sandbox::new();
    let output = sandbox
        .command(true, &["con", "2", "--backend", "tmux", "--launch-gui"])
        .env_remove("DMUX_DRY_RUN")
        .env("TMUX", "/definitely/stale,999,0")
        .env("TMUX_PANE", "%999")
        .output()
        .expect("dmux runs");
    assert_eq!(output.status.code(), Some(2));
    let error = stderr(&output);
    assert!(error.contains("--launch-gui"), "{error}");
    assert!(!error.contains("DMUX_CONTEXT_VERSION"), "{error}");
    sandbox.assert_registry_untouched();
}

#[test]
fn feature_on_static_ref_errors_beat_a_stale_tmux_environment() {
    let sandbox = Sandbox::new();
    let epoch = "33333333-3333-4333-8333-333333333333";
    let cases = [
        vec!["con".to_string(), "0".to_string()],
        vec!["con".to_string(), "--name".to_string(), String::new()],
        vec![
            "con".to_string(),
            format!("2/g{epoch}.tx-1"),
            "--group".to_string(),
            format!("g{epoch}.tx-2"),
        ],
    ];
    for args in cases {
        let borrowed: Vec<_> = args.iter().map(String::as_str).collect();
        let output = sandbox
            .command(true, &borrowed)
            .env_remove("DMUX_DRY_RUN")
            .env("TMUX", "/definitely/stale,999,0")
            .env("TMUX_PANE", "%999")
            .output()
            .expect("dmux runs");
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        let error = stderr(&output);
        assert!(!error.contains("DMUX_CONTEXT_VERSION"), "{error}");
        assert!(!error.contains("managed tmux client"), "{error}");
    }
    sandbox.assert_registry_untouched();
}

#[test]
fn feature_on_bare_dash_routes_to_stable_previous_with_typed_not_found() {
    let sandbox = Sandbox::new();
    let output = sandbox.run_without_dry_run(true, &["-"]);
    assert_eq!(output.status.code(), Some(3));
    assert!(stderr(&output).contains("no previous Space is recorded"));
}

#[test]
fn feature_on_dry_run_refuses_before_planning_or_exposing_an_attach_token() {
    let sandbox = Sandbox::new();
    for args in [
        &["con", "2"][..],
        &["2"][..],
        &["-"][..],
        &["attach", "2"][..],
    ] {
        let output = sandbox.run(true, args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        let error = stderr(&output);
        assert!(error.contains("DMUX_DRY_RUN cannot preview"), "{error}");
        assert!(!stdout(&output).contains("--token"), "{args:?}");
    }
    sandbox.assert_registry_untouched();
}

#[test]
fn feature_on_accepts_dynamic_host_spellings_before_typed_resolution() {
    let sandbox = Sandbox::new();
    let canonical_host = "22222222-2222-4222-8222-222222222222";
    for args in [
        vec!["--host", "b", "con", "2"],
        vec!["--host", "custom-owner", "2"],
        vec!["--host", canonical_host, "-"],
    ] {
        let output = sandbox.run(true, &args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        let error = stderr(&output);
        assert!(error.contains("DMUX_DRY_RUN cannot preview"), "{error}");
        assert!(!error.contains("unknown legacy host"), "{error}");
    }
    sandbox.assert_registry_untouched();
}

#[test]
fn feature_on_normalizes_legacy_machine_names_to_authority_aliases() {
    let sandbox = Sandbox::new();
    sandbox.enroll_unlabelled_peer();
    let (local, peer) = if cfg!(target_os = "macos") {
        ("macie", "archie")
    } else {
        ("archie", "macie")
    };
    for requested in [local, peer] {
        let output = sandbox.run_without_dry_run(true, &["--host", requested, "-"]);
        assert_eq!(output.status.code(), Some(3), "{requested}");
        let error = stderr(&output);
        assert!(error.contains("no previous Space is recorded"), "{error}");
        assert!(!error.contains("no enrolled host matches"), "{error}");
    }
}

#[test]
fn feature_on_shorthand_usage_errors_keep_the_typed_usage_exit() {
    let sandbox = Sandbox::new();
    for args in [
        &["2", "--backend", "tmux"][..],
        &["project", "-w1"][..],
        &["-", "extra"][..],
    ] {
        let output = sandbox.run(true, args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
    sandbox.assert_registry_untouched();
}

#[test]
fn flag_off_refuses_feature_only_connect_options_instead_of_ignoring_them() {
    let sandbox = Sandbox::new();
    let epoch = "33333333-3333-4333-8333-333333333333";
    for args in [
        vec!["con", "--name", "alpha"],
        vec!["con", "alpha", "--backend", "wez"],
        vec!["con", "alpha", "--group", &format!("g{epoch}.tx-1")],
        vec!["con", "alpha", "--split", &format!("p{epoch}.tx-1")],
        vec!["con", "alpha", "--launch-gui"],
    ] {
        let output = sandbox.run(false, &args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(stderr(&output).contains("require DMUX_WEZ_FIRST=1"));
    }
}

#[test]
fn flag_off_keeps_legacy_positional_window_dispatch() {
    let sandbox = Sandbox::new();
    sandbox.stub_tmux();

    let window = sandbox.run(false, &["con", "alpha", "-w", "1"]);
    assert!(window.status.success(), "{}", stderr(&window));
    assert_eq!(
        stdout(&window),
        "would exec: tmux attach -t '=alpha' ';' select-window -t '=alpha:1'\n"
    );
}
