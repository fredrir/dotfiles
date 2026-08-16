//! The remote command string, fed to a real shell.
//!
//! This is the test class whose absence let the equals-expansion bug ship:
//! the dry-run tests pin what the command string looks like, but only a
//! shell can say what it does. A stub ssh plays the remote side — it answers
//! the listing probe, then hands any other command string to a real shell
//! with a stub tmux that prints its argv. The `=name` target must arrive as
//! one intact argument. zsh matters most: its EQUALS option rewrites an
//! unquoted `=word` into a PATH lookup, so `=main` dies with "main not
//! found" and `=ls` silently becomes a filesystem path.
//!
//! Every remote command now opens with `REMOTE_PATH_PREFIX`, which rebuilds
//! PATH with `$HOME/.local/bin` first — so the stub tmux lives in a fake
//! remote home's `.local/bin` and the stub ssh overrides HOME to that home.
//! The prefix's own expansion then finds the stub before any real
//! /opt/homebrew/bin/tmux on the machine running the tests. The stub also
//! keeps that directory at the front of the inherited PATH as a backstop:
//! even if a regression dropped the prefix, no real tmux could ever answer.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

/// The remote sessions the stub ssh reports: a plain name, a name that
/// collides with a PATH command, and a name with a space.
const SESSIONS: &[&str] = &["main", "ls", "a b"];

/// Same literal `cli.rs` pins: the PATH prefix every remote command opens
/// with, here only for the one dry-run expectation this file checks.
const REMOTE_PATH: &str = r#"PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH" "#;

const SSH_STUB: &str = r#"#!/bin/sh
# Fake remote: answer the listing probe; hand any other command string to
# the "remote login shell" as the fake remote user, whose ~/.local/bin
# holds the stub tmux — the first directory the command's own PATH prefix
# resolves. Prepending it to the inherited PATH too is a backstop: a
# command missing the prefix must still hit the stub, never a real tmux.
for last in "$@"; do :; done
case "$last" in
*list-sessions*)
    printf 'main|1700000000|1|0\nls|1700000100|1|0\na b|1700000200|1|0\n'
    ;;
*)
    HOME="$REMOTE_HOME" PATH="$REMOTE_HOME/.local/bin:$PATH" \
        exec "$REMOTE_SHELL" -c "$last"
    ;;
esac
"#;

const TMUX_STUB: &str = "#!/bin/sh\nprintf '%s\\n' \"$@\"\n";

struct Remote {
    bin: TempDir,
    home: TempDir,
    state: TempDir,
    shell: PathBuf,
}

impl Remote {
    /// None when the asked-for shell is not installed: skip, don't fail.
    fn new(shell: &str) -> Option<Remote> {
        let shell = find_on_path(shell)?;
        let remote = Remote {
            bin: tempfile::tempdir().unwrap(),
            home: tempfile::tempdir().unwrap(),
            state: tempfile::tempdir().unwrap(),
            shell,
        };
        write_stub(&remote.bin.path().join("ssh"), SSH_STUB);
        let local_bin = remote.home.path().join(".local/bin");
        fs::create_dir_all(&local_bin).unwrap();
        write_stub(&local_bin.join("tmux"), TMUX_STUB);
        Some(remote)
    }

    fn dmux(&self, args: &[&str], dry_run: bool) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dmux"));
        command
            .args(args)
            .env("PATH", self.bin.path())
            .env("XDG_DATA_HOME", self.state.path())
            .env("XDG_STATE_HOME", self.state.path())
            .env("XDG_RUNTIME_DIR", self.state.path())
            .env("REMOTE_SHELL", &self.shell)
            .env("REMOTE_HOME", self.home.path())
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("DMUX_WEZ_FIRST")
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
        if dry_run {
            command.env("DMUX_DRY_RUN", "1");
        } else {
            command.env_remove("DMUX_DRY_RUN");
        }
        command.output().expect("dmux runs")
    }
}

fn write_stub(path: &Path, script: &str) {
    fs::write(path, script).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

fn peer() -> &'static str {
    if cfg!(target_os = "macos") {
        "archie"
    } else {
        "macie"
    }
}

/// Attach every test session over the stub transport and assert the target
/// reached tmux as a single, intact `=name` argument.
fn targets_survive(shell: &str) {
    let Some(remote) = Remote::new(shell) else {
        eprintln!("skipping: {shell} not on PATH");
        return;
    };
    for session in SESSIONS {
        let output = remote.dmux(&["con", session, "--host", peer()], false);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "{shell}: attaching '{session}' failed: {stderr}"
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let argv: Vec<&str> = stdout.lines().collect();
        let target = format!("={session}");
        assert_eq!(
            argv,
            ["attach", "-t", &target],
            "{shell}: '{session}' did not arrive intact"
        );
    }
}

#[test]
fn remote_targets_survive_zsh() {
    targets_survive("zsh");
}

#[test]
fn remote_targets_survive_sh() {
    targets_survive("sh");
}

/// `new --dir` and the trailing command ride the same ssh command string:
/// a directory with a space, and a command word with one, must each arrive
/// at the remote tmux as a single argument.
fn new_directory_survives(shell: &str) {
    let Some(remote) = Remote::new(shell) else {
        eprintln!("skipping: {shell} not on PATH");
        return;
    };
    let output = remote.dmux(
        &[
            "new",
            "scratch",
            "--dir",
            "/tmp/a b",
            "--host",
            peer(),
            "--",
            "echo",
            "hi there",
        ],
        false,
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{shell}: new failed: {stderr}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let argv: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        argv,
        [
            "new-session",
            "-A",
            "-s",
            "scratch",
            "-c",
            "/tmp/a b",
            "echo",
            "hi there"
        ],
        "{shell}: --dir or command did not arrive intact"
    );
}

#[test]
fn a_new_directory_survives_zsh() {
    new_directory_survives("zsh");
}

#[test]
fn a_new_directory_survives_sh() {
    new_directory_survives("sh");
}

/// The remote toggle round-trip: two real attaches record current and
/// previous under the peer's key, and `dmux -H peer -` plans an attach of
/// the one before this one.
#[test]
fn a_remote_toggle_reattaches_the_previous_session() {
    let Some(remote) = Remote::new("sh") else {
        eprintln!("skipping: sh not on PATH");
        return;
    };
    let peer = peer();
    assert!(
        remote
            .dmux(&["con", "main", "--host", peer], false)
            .status
            .success()
    );
    assert!(
        remote
            .dmux(&["con", "ls", "--host", peer], false)
            .status
            .success()
    );
    let output = remote.dmux(&["--host", peer, "-"], true);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "toggle failed: {stderr}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!(
            "would exec: ssh -o 'ConnectTimeout=5' -t {peer} '{REMOTE_PATH}exec tmux attach -t '\\''=main'\\'''\n"
        )
    );
}

/// Before anything is recorded for the peer, the toggle refuses honestly.
#[test]
fn a_remote_toggle_without_history_says_so() {
    let Some(remote) = Remote::new("sh") else {
        eprintln!("skipping: sh not on PATH");
        return;
    };
    let output = remote.dmux(&["--host", peer(), "-"], true);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("no previous session"));
}
