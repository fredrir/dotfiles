//! Runtime-directory isolation guard, unit level (ADR 012 §3.2 / WS-E.1;
//! plan §20.1 "suite runs leave the live runtime directory unchanged").
//!
//! The property: every production constructor of a runtime or kernel-lock
//! path resolves through `runtime::dmux_runtime_dir()`, and that resolver
//! returns the owner-side `DMUX_RUNTIME_DIR` seam verbatim when one is
//! exported — so a suite that exports the seam never takes a kernel lock,
//! writes a bridge key, or binds a socket in the directory the live service
//! is using. Before this guard existed, only the `_pane-bootstrap` helper
//! read the seam; every `dmux` the suite spawned locked in the live
//! directory regardless (ADR 012 §3.2 counted +18 lock files per run).
//!
//! Two layers, because `cargo test` runs every test binary as a separate
//! process and has no suite-wide setup or teardown in which a single
//! before/after snapshot could live:
//!
//! 1. `tests/run-isolated.sh` is the suite-level guard. It exports the seams
//!    to fresh scratch directories, snapshots every entry under the live
//!    `platform_runtime_dir()`, runs `cargo test -p dmux`, re-snapshots, and
//!    fails naming the new entries when the live directory grew.
//! 2. This file is the unit-level guard, so a regression in any constructor
//!    is caught by bare `cargo test` too: the library constructors are
//!    checked in process (in a re-executed copy of this binary carrying the
//!    seam, so no test mutates the shared process environment), and the CLI
//!    binary is checked by spawning it with the seam and proving its lock
//!    file and bridge key landed beneath the seam.
//!
//! Keep any `DMUX_RUNTIME_DIR` you export SHORT: scratch mux servers bind
//! `<dir>/wez-dmux.sock` directly beneath the seam, and `sun_path` is 104
//! bytes on macOS (108 on Linux). A deep scratch path — a per-session
//! scratchpad, a worktree — fails every socket-binding test with "File name
//! too long". The wrapper puts it directly under `$TMPDIR` and checks the
//! length; the tempdirs this file uses are short for the same reason.
//!
//! `tests/pane_bootstrap.rs` already exercises the helper binary's own
//! resolver under the seam on every test, so it is not repeated here.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use dmux::operations::OperationEnv;
use dmux::registry::RegistryConfig;
use dmux::remote::agent::{self, AgentArgs};
use dmux::runtime::{self, RUNTIME_DIR_SEAM_ENV};

/// Marks the re-executed copy of this binary; never set by anything else.
const INNER_ENV: &str = "DMUX_RUNTIME_DIR_SEAM_GUARD_INNER";

fn seam_from_env() -> Option<PathBuf> {
    runtime::runtime_dir_seam().expect("seam parses")
}

/// A scratch seam plus a scratch data home, both fresh.
struct Scratch {
    runtime: tempfile::TempDir,
    data: tempfile::TempDir,
}

impl Scratch {
    fn new() -> Scratch {
        let runtime = tempfile::tempdir().unwrap();
        // The seam is used verbatim, so the test owns the checks the platform
        // path would get: the bridge key insists on an exactly-0700 runtime
        // dir, and `tempdir()` does not promise one.
        std::fs::set_permissions(
            runtime.path(),
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .unwrap();
        Scratch {
            runtime,
            data: tempfile::tempdir().unwrap(),
        }
    }

    fn seam(&self) -> &Path {
        self.runtime.path()
    }

    /// The CLI under the seams, with every ambient dmux/mux variable
    /// removed so the outcome depends on nothing the developer's shell
    /// carries (the suite runner unsets `DMUX_WEZ_FIRST` for the same
    /// reason).
    fn dmux(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dmux"));
        command
            .args(args)
            .env(RUNTIME_DIR_SEAM_ENV, self.seam())
            .env("XDG_DATA_HOME", self.data.path())
            .env("XDG_STATE_HOME", self.data.path())
            .env_remove("DMUX_WEZ_FIRST")
            .env_remove("DMUX_LEGACY_POLICY")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("WEZTERM_PANE")
            .env_remove("WEZTERM_UNIX_SOCKET");
        command
    }
}

fn describe(output: &Output) -> String {
    format!(
        "status={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The in-process half: every library constructor of a lock or runtime path
/// resolves to the exported seam, and the seam-blind platform resolver does
/// not. Runs in a re-executed copy of this binary that carries the seam.
#[test]
fn library_constructors_resolve_the_seam() {
    if std::env::var_os(INNER_ENV).is_some() {
        let seam = seam_from_env().expect("the inner run carries the seam");

        assert_eq!(runtime::dmux_runtime_dir().unwrap(), seam);
        // operations.rs `OperationEnv::production` — the lock dir every CLI
        // verb, `_agent` and `_attach` build their `OrderedLocks` from.
        assert_eq!(OperationEnv::production().unwrap().lock_dir, seam);
        // registry/mod.rs `RegistryConfig::production` — the lock dir the
        // registry takes the authority gate in.
        assert_eq!(RegistryConfig::production().unwrap().lock_dir, seam);
        // remote/agent.rs `resolve_env` production arm — what a remote peer's
        // `ssh <route> dmux _agent` resolves when it passes no hidden flags.
        let args = AgentArgs {
            protocol: 1,
            method: "hello".into(),
            data_dir: None,
            lock_dir: None,
        };
        assert_eq!(agent::resolve_env(&args).unwrap().lock_dir, seam);
        // gui_cli.rs `ProductionGuiAuthority::production` and new_cli.rs
        // `ProductionNewRuntime::production` take `runtime_dir` straight from
        // `dmux_runtime_dir()` and their lock dir from
        // `OperationEnv::production()`, both asserted above.

        // The seam-blind resolver is the live directory, not the seam: it is
        // what the suite-level guard snapshots.
        let platform = runtime::platform_runtime_dir().unwrap();
        assert_ne!(platform, seam);
        assert!(platform.ends_with("dmux"), "{}", platform.display());
        return;
    }

    let scratch = Scratch::new();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "library_constructors_resolve_the_seam",
            "--nocapture",
        ])
        .env(INNER_ENV, "1")
        .env(RUNTIME_DIR_SEAM_ENV, scratch.seam())
        .env("XDG_DATA_HOME", scratch.data.path())
        .output()
        .expect("this test binary re-executes");
    assert!(output.status.success(), "{}", describe(&output));
}

/// The CLI half, kernel locks: a `dmux` that opens the registry under the
/// seam creates its authority-gate lock file beneath the seam. `_attach`
/// with a bogus token is the cheapest verb that opens the registry before
/// it refuses; it needs no mux, no tmux and no stdin.
#[test]
fn the_cli_takes_its_kernel_locks_under_the_seam() {
    let scratch = Scratch::new();
    let gate = scratch.seam().join("authority-gate.lock");
    assert!(!gate.exists(), "fresh seam");

    let output = scratch
        .dmux(&["_attach", "--token", "not-a-token"])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "a bogus token must be refused:\n{}",
        describe(&output)
    );
    assert!(
        gate.is_file(),
        "authority-gate.lock must be taken beneath the seam, not in the live dir:\n{}",
        describe(&output)
    );
    assert!(
        scratch.data.path().join("dmux/registry.sqlite3").is_file(),
        "the registry must be created beneath XDG_DATA_HOME:\n{}",
        describe(&output)
    );
}

/// The CLI half, the direct resolver call: `_bridge-key` resolves
/// `dmux_runtime_dir()` itself (main.rs) and provisions the bridge key
/// beneath it.
#[test]
fn the_cli_provisions_the_bridge_key_under_the_seam() {
    let scratch = Scratch::new();
    let output = scratch.dmux(&["_bridge-key"]).output().unwrap();
    assert!(output.status.success(), "{}", describe(&output));
    let bridge = scratch.seam().join("bridge");
    assert!(bridge.join("key").is_file(), "{}", describe(&output));
    assert!(bridge.join("key.boot").is_file(), "{}", describe(&output));
}

/// A relative seam fails closed in the CLI as well, rather than resolving
/// against the working directory: nothing is created anywhere.
#[test]
fn a_relative_seam_is_refused_by_the_cli() {
    let scratch = Scratch::new();
    let output = scratch
        .dmux(&["_bridge-key"])
        .env(RUNTIME_DIR_SEAM_ENV, "relative/runtime")
        .current_dir(scratch.seam())
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", describe(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("DMUX_RUNTIME_DIR"),
        "{}",
        describe(&output)
    );
    assert!(
        !scratch.seam().join("relative").exists() && !scratch.seam().join("bridge").exists(),
        "nothing may be created under the working directory"
    );
}
