//! Black-box checks on flags, outputs and transport selection.
//!
//! Hermetic on purpose: PATH points at a directory holding a fake tmux, so
//! no live server is ever consulted, and DMUX_DRY_RUN=1 turns every exec
//! into a printed plan.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};

use tempfile::TempDir;

struct Sandbox {
    bin: TempDir,
    state: TempDir,
}

impl Sandbox {
    /// An empty PATH: neither tmux nor wezterm exist.
    fn empty() -> Sandbox {
        Sandbox {
            bin: tempfile::tempdir().unwrap(),
            state: tempfile::tempdir().unwrap(),
        }
    }

    /// A fake tmux serving two sessions: alpha (attached), then beta.
    fn with_tmux() -> Sandbox {
        let sandbox = Sandbox::empty();
        let tmux = sandbox.bin.path().join("tmux");
        fs::write(
            &tmux,
            "#!/bin/sh\ncase \"$1\" in\n\
             list-sessions) printf 'alpha|1700000000|2|1\\nbeta|1700000100|1|0\\n' ;;\n\
             esac\n",
        )
        .unwrap();
        fs::set_permissions(&tmux, fs::Permissions::from_mode(0o755)).unwrap();
        sandbox
    }

    fn dmux(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_dmux"))
            .args(args)
            .env("PATH", self.bin.path())
            .env("XDG_STATE_HOME", self.state.path())
            .env("DMUX_DRY_RUN", "1")
            .env_remove("TMUX")
            .env_remove("WEZTERM_UNIX_SOCKET")
            .env_remove("WEZTERM_PANE")
            .env_remove("NO_COLOR")
            .output()
            .expect("dmux runs")
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn peer() -> &'static str {
    if cfg!(target_os = "macos") {
        "archie"
    } else {
        "macie"
    }
}

fn this_host() -> &'static str {
    if cfg!(target_os = "macos") {
        "macie"
    } else {
        "archie"
    }
}

#[test]
fn both_version_flags_print_version_and_name() {
    let sandbox = Sandbox::empty();
    let expected = format!("{} (dmux)\n", env!("CARGO_PKG_VERSION"));
    for flag in ["-v", "--version"] {
        let output = sandbox.dmux(&[flag]);
        assert!(output.status.success());
        assert_eq!(stdout(&output), expected);
    }
}

#[test]
fn help_describes_this_tool() {
    let sandbox = Sandbox::empty();
    let output = sandbox.dmux(&["--help"]);
    assert!(stdout(&output).starts_with("Wezterm-mux and tmux sessions"));
}

#[test]
fn completions_are_static_zsh() {
    let sandbox = Sandbox::empty();
    let output = sandbox.dmux(&["--completions", "zsh"]);
    assert!(output.status.success());
    assert!(stdout(&output).contains("#compdef dmux"));
}

#[test]
fn an_invalid_session_name_is_rejected() {
    let sandbox = Sandbox::empty();
    for target in ["bad name", "semi;colon", "=oops"] {
        let output = sandbox.dmux(&["con", target]);
        assert_eq!(output.status.code(), Some(1));
        assert!(stderr(&output).contains("letters, numbers"));
    }
}

#[test]
fn an_unknown_host_is_a_usage_error() {
    let sandbox = Sandbox::empty();
    let output = sandbox.dmux(&["--host", "bogus", "ls"]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn ls_shows_the_tmux_sessions() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["ls"]);
    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("alpha"));
    assert!(text.contains("beta"));
    assert!(text.contains("tmux"));
    assert!(text.contains("attached"));
}

#[test]
fn ls_json_carries_the_full_rows() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["ls", "--json"]);
    let rows: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(rows[0]["index"], 1);
    assert_eq!(rows[0]["name"], "alpha");
    assert_eq!(rows[0]["kind"], "tmux");
    assert_eq!(rows[0]["created"], 1_700_000_000);
    assert_eq!(rows[0]["windows"], 2);
    assert_eq!(rows[0]["attached"], true);
    assert_eq!(rows[1]["name"], "beta");
    assert_eq!(rows[1]["attached"], false);
}

#[test]
fn ls_names_is_plain_lines() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["ls", "--names"]);
    assert_eq!(stdout(&output), "alpha\nbeta\n");
}

#[test]
fn bare_dmux_on_a_pipe_is_ls() {
    let sandbox = Sandbox::with_tmux();
    assert_eq!(stdout(&sandbox.dmux(&[])), stdout(&sandbox.dmux(&["ls"])));
}

#[test]
fn con_attaches_an_existing_session() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["con", "alpha"]);
    assert!(output.status.success());
    assert_eq!(stdout(&output), "would exec: tmux attach -t =alpha\n");
}

#[test]
fn the_aliases_reach_con() {
    let sandbox = Sandbox::with_tmux();
    for verb in ["attach", "a"] {
        let output = sandbox.dmux(&[verb, "alpha"]);
        assert_eq!(stdout(&output), "would exec: tmux attach -t =alpha\n");
    }
}

#[test]
fn an_unknown_word_falls_through_to_con() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["beta"]);
    assert_eq!(stdout(&output), "would exec: tmux attach -t =beta\n");
}

#[test]
fn a_numeric_target_resolves_against_the_listing() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["con", "2"]);
    assert_eq!(stdout(&output), "would exec: tmux attach -t =beta\n");
}

#[test]
fn inside_tmux_con_switches_the_client() {
    let sandbox = Sandbox::with_tmux();
    let output = Command::new(env!("CARGO_BIN_EXE_dmux"))
        .args(["con", "alpha"])
        .env("PATH", sandbox.bin.path())
        .env("XDG_STATE_HOME", sandbox.state.path())
        .env("DMUX_DRY_RUN", "1")
        .env("TMUX", "/tmp/tmux-0/default,1,0")
        .env_remove("WEZTERM_UNIX_SOCKET")
        .output()
        .unwrap();
    assert_eq!(
        stdout(&output),
        "would exec: tmux switch-client -t =alpha\n"
    );
}

#[test]
fn a_window_is_selected_after_the_attach() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["con", "alpha", "-w", "2"]);
    assert_eq!(
        stdout(&output),
        "would exec: tmux attach -t =alpha ';' select-window -t =alpha:2\n"
    );
}

#[test]
fn con_refuses_to_create() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["con", "nosuch"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "dmux: no session 'nosuch' (dmux new nosuch to create it)\n"
    );
}

#[test]
fn new_creates_and_attaches_locally() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["new", "scratch"]);
    assert_eq!(
        stdout(&output),
        "would exec: tmux new-session -A -s scratch\n"
    );
}

#[test]
fn a_remote_named_session_goes_over_ssh() {
    let sandbox = Sandbox::empty();
    let output = sandbox.dmux(&["--host", peer(), "new", "scratch"]);
    assert_eq!(
        stdout(&output),
        format!(
            "would exec: ssh -t {} 'exec tmux new-session -A -s scratch'\n",
            peer()
        )
    );
}

#[test]
fn a_bare_remote_attach_outside_wezterm_is_tmux_main() {
    let sandbox = Sandbox::empty();
    let output = sandbox.dmux(&["--host", peer()]);
    assert_eq!(
        stdout(&output),
        format!(
            "would exec: ssh -t {} 'exec tmux new-session -A -s main'\n",
            peer()
        )
    );
}

#[test]
fn a_bare_remote_attach_inside_wezterm_spawns_a_native_domain() {
    let sandbox = Sandbox::empty();
    let output = Command::new(env!("CARGO_BIN_EXE_dmux"))
        .args(["--host", peer()])
        .env("PATH", sandbox.bin.path())
        .env("XDG_STATE_HOME", sandbox.state.path())
        .env("DMUX_DRY_RUN", "1")
        .env("WEZTERM_UNIX_SOCKET", "/tmp/wezterm-sock")
        .env_remove("TMUX")
        .output()
        .unwrap();
    let expected = format!("would exec: wezterm cli spawn --domain-name {}-", peer());
    assert!(stdout(&output).starts_with(&expected));
}

#[test]
fn dash_toggles_to_the_recorded_session() {
    let sandbox = Sandbox::with_tmux();
    let dir = sandbox.state.path().join("dmux");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("last"), format!("{} alpha\n", this_host())).unwrap();
    let output = sandbox.dmux(&["-"]);
    assert!(output.status.success());
    assert_eq!(stdout(&output), "would exec: tmux attach -t =alpha\n");
}

#[test]
fn dash_without_state_says_so() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["-"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("no previous session"));
}

#[test]
fn rm_kills_with_an_exact_target() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["rm", "--yes", "beta"]);
    assert!(output.status.success());
    assert_eq!(stdout(&output), "would run: tmux kill-session -t =beta\n");
}

#[test]
fn rm_with_a_window_kills_only_that_window() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["rm", "--yes", "alpha", "-w", "2"]);
    assert_eq!(stdout(&output), "would run: tmux kill-window -t =alpha:2\n");
}

#[test]
fn rm_refuses_a_target_that_does_not_exist() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["rm", "--yes", "nosuch"]);
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn rename_validates_both_names() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["rename", "alpha", "fresh"]);
    assert_eq!(
        stdout(&output),
        "would run: tmux rename-session -t =alpha fresh\n"
    );
    let output = sandbox.dmux(&["rename", "alpha", "bad name"]);
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn a_remote_listing_needs_ssh() {
    let sandbox = Sandbox::empty();
    let output = sandbox.dmux(&["ls", "--host", peer()]);
    assert_eq!(output.status.code(), Some(1));
}
