//! Hermetic black-box checks for the feature-on public `new` cutover.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use dmux::model::Backend;
use dmux::operations::{OperationEnv, TmuxBootstrapOutcome, tmux_bootstrap};
use dmux::registry::{Registry, RegistryConfig};
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

    /// The owner's registry exactly where the binary will open it
    /// (`XDG_DATA_HOME`), with a scratch lock directory for this process's
    /// own seeding writes.
    fn env(&self) -> OperationEnv {
        fs::create_dir_all(self.data.path().join("dmux")).unwrap();
        OperationEnv {
            db_path: self.data.path().join("dmux/registry.sqlite3"),
            lock_dir: self.runtime.path().to_path_buf(),
        }
    }

    fn registry(&self) -> Registry {
        let env = self.env();
        Registry::open(RegistryConfig::new(env.db_path, env.lock_dir)).unwrap()
    }

    /// A `tmux` on the stub PATH that records every invocation and answers
    /// nothing. Under the gate a `new` that reaches any tmux call leaves the
    /// witness behind; "never called" is proven by its absence.
    fn recording_tmux(&self) -> PathBuf {
        let witness = self.state.path().join("tmux-ran");
        let stub = self.bin.path().join("tmux");
        fs::write(
            &stub,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 1\n",
                witness.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
        witness
    }

    fn meta_counter(&self, column: &str) -> i64 {
        rusqlite::Connection::open_with_flags(
            self.env().db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap()
        .query_row(
            &format!("SELECT {column} FROM meta WHERE id = 1"),
            [],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn bootstrap_request_rows(&self) -> i64 {
        rusqlite::Connection::open_with_flags(
            self.env().db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap()
        .query_row("SELECT COUNT(*) FROM bootstrap_requests", [], |row| {
            row.get(0)
        })
        .unwrap()
    }
}

/// The directory holding the real `tmux` on this process's PATH, for the
/// one control run that needs a genuine scan.
fn real_tmux_dir() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").expect("PATH"))
        .find(|dir| dir.join("tmux").is_file())
        .expect("a real tmux on PATH (the suite already depends on one)")
}

/// A stranger's tmux server on a private `-L` namespace, killed on drop.
struct ScratchTmux {
    namespace: String,
}

impl ScratchTmux {
    fn start(tag: &str, session: &str) -> ScratchTmux {
        let server = ScratchTmux {
            namespace: format!("dmux-newdispatch-{tag}-{}", std::process::id()),
        };
        let out = server.tmux(&["-f", "/dev/null", "new-session", "-d", "-s", session]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        server
    }

    fn tmux(&self, args: &[&str]) -> Output {
        Command::new("tmux")
            .args(["-L", &self.namespace])
            .args(args)
            .output()
            .expect("tmux runs")
    }

    fn sessions(&self) -> Vec<String> {
        let out = self.tmux(&["list-sessions", "-F", "#{session_name}"]);
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect()
    }
}

impl Drop for ScratchTmux {
    fn drop(&mut self) {
        let _ = self.tmux(&["kill-server"]);
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

/// Review finding #12, inverted, through the gated dispatch: a registered,
/// addressable tmux instance whose epoch was never published (the row
/// `dmux-mux-start.sh` leaves when coordination never completes) makes
/// `dmux new` refuse with `backend_epoch_changed` — auto and explicit
/// backend alike — before the lookup, so nothing reserves a Space, the
/// SpaceNo counter and the bootstrap journal stay untouched, and tmux is
/// never invoked at all. The control publishes the stranger server's epoch
/// through the production bootstrap and shows the identical command then
/// proceeds to a verified answer: the stranger's `seed` session becomes the
/// unmanaged same-name blocker it really is, with still nothing created.
#[test]
fn an_unpublished_tmux_instance_refuses_the_gated_new_before_any_reservation_or_tmux_call() {
    let sandbox = Sandbox::new();
    let stranger = ScratchTmux::start("unpublished", "seed");
    let witness = sandbox.recording_tmux();
    let mut registry = sandbox.registry();
    let instance = registry
        .register_backend_instance(Backend::Tmux, Some(&stranger.namespace), None)
        .unwrap();
    assert!(
        registry
            .backend_server(instance)
            .unwrap()
            .server_epoch
            .is_none(),
        "the fixture is only meaningful with server_epoch NULL"
    );
    let head = registry.authority_head().unwrap();
    let counter = sandbox.meta_counter("space_no_counter");
    drop(registry);

    for args in [
        vec!["new", "seed", "--no-connect"],
        vec!["new", "seed", "--backend", "tmux", "--no-connect"],
        vec!["new", "project", "--backend", "tmux", "--no-connect"],
    ] {
        let output = sandbox.run_live(true, &args);
        assert_eq!(
            output.status.code(),
            Some(1),
            "{args:?}: {}",
            stderr(&output)
        );
        assert!(
            stderr(&output).contains("published no server epoch"),
            "{args:?}: {}",
            stderr(&output)
        );
        assert_eq!(stdout(&output), "", "{args:?}: a receipt was printed");
    }

    let registry = sandbox.registry();
    assert!(
        registry.spaces().unwrap().is_empty(),
        "a Space row was minted"
    );
    assert_eq!(
        registry.authority_head().unwrap(),
        head,
        "the revision chain moved: something wrote to the registry"
    );
    assert_eq!(sandbox.meta_counter("space_no_counter"), counter);
    assert_eq!(sandbox.bootstrap_request_rows(), 0);
    assert!(
        !witness.exists(),
        "dmux ran tmux against an unverified server:\n{}",
        fs::read_to_string(&witness).unwrap_or_default()
    );
    assert_eq!(stranger.sessions(), ["seed"]);
    drop(registry);

    // Control: publish the epoch the stranger server actually serves, then
    // run the same gated command with the real tmux reachable. The pinned
    // scan now runs and sees `seed`, so the answer is the unmanaged
    // same-name refusal — a verdict only a verified inventory may reach.
    match tmux_bootstrap(&sandbox.env(), &stranger.namespace).unwrap() {
        TmuxBootstrapOutcome::Bootstrapped { .. }
        | TmuxBootstrapOutcome::AlreadyBound { .. }
        | TmuxBootstrapOutcome::Rebound { .. } => {}
    }
    let control = sandbox
        .command(true, &["new", "seed", "--backend", "tmux", "--no-connect"])
        .env_remove("DMUX_DRY_RUN")
        .env("PATH", real_tmux_dir())
        .output()
        .unwrap();
    assert_eq!(control.status.code(), Some(4), "{}", stderr(&control));
    assert!(
        stderr(&control).contains("UnmanagedSameName"),
        "{}",
        stderr(&control)
    );
    assert!(
        !stderr(&control).contains("published no server epoch"),
        "{}",
        stderr(&control)
    );
    assert!(sandbox.registry().spaces().unwrap().is_empty());
    assert_eq!(stranger.sessions(), ["seed"]);
    assert!(
        !Path::new(&witness).exists(),
        "the stub tmux must not have been reachable in the control run"
    );
}
