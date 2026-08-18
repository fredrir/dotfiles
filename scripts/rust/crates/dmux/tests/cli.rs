//! Black-box checks on flags, outputs and transport selection.
//!
//! Hermetic on purpose: PATH points at a directory holding a fake tmux, so
//! no live server is ever consulted, and DMUX_DRY_RUN=1 turns every exec
//! into a printed plan.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output, Stdio};

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
        sandbox.stub(
            "tmux",
            "case \"$1\" in\n\
             list-sessions) printf 'alpha|1700000000|2|1\\nbeta|1700000100|1|0\\n' ;;\n\
             esac",
        );
        sandbox
    }

    /// The fake tmux plus a fake wezterm serving one workspace, `work`.
    fn with_tmux_and_wez() -> Sandbox {
        let sandbox = Sandbox::with_tmux();
        sandbox.stub(
            "wezterm",
            r#"printf '[{"window_id":1,"pane_id":1,"workspace":"work"}]'"#,
        );
        sandbox
    }

    fn stub(&self, name: &str, script: &str) {
        let path = self.bin.path().join(name);
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A ready-to-run dmux with the hermetic environment; tests that need a
    /// twist (an env var, no dry run, piped stdin) adjust before spawning.
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dmux"));
        command
            .args(args)
            .env("PATH", self.bin.path())
            .env("XDG_DATA_HOME", self.state.path())
            .env("XDG_STATE_HOME", self.state.path())
            .env("XDG_RUNTIME_DIR", self.state.path())
            .env("DMUX_DRY_RUN", "1")
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
        command
    }

    fn dmux(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("dmux runs")
    }

    /// Run with the given text piped into stdin.
    fn dmux_stdin(&self, args: &[&str], input: &str) -> Output {
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("dmux runs");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().expect("dmux finishes")
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The PATH prefix every remote command string opens with, pinned as a
/// literal on purpose: macOS sshd hands non-interactive commands a minimal
/// PATH without Homebrew's tmux, and a drive-by edit to the prefix in
/// `hosts.rs` must show up here, not silently ride along.
const REMOTE_PATH: &str = r#"PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH" "#;

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

/// The ssa/ssm wrappers keep a verb allowlist so an interactive shell can tell
/// a lone Space name from a subcommand without spawning dmux (plan §17, ADR
/// 010 §4). Maintained by hand, that list drifts the first time the CLI grows a
/// verb -- and a missing verb is not inert, it makes `ssa <verb>` create a
/// Space -- so it is derived from the binary here rather than trusted.
///
/// The source is `--completions zsh` rather than `--help`: clap emits one
/// `'name:description'` candidate per line for every subcommand *and* every
/// alias, where help folds aliases into an `[aliases: ...]` suffix on a
/// description that wraps at the terminal width. Both list clap's built-in
/// `help`. Completions also carry the hidden internal verbs, which the wrapper
/// must not forward; they are dropped by the `_` prefix all of them carry, and
/// The wrappers keep a verb allowlist so `ssa ls` lists instead of naming a
/// Space, and it had already drifted once -- 14 verbs listed against 22 in the
/// CLI, so `ssa host` created a Space called "host". Both sides are evaluated
/// rather than parsed: `_verbs` walks clap's own command tree, and the array is
/// read by sourcing it in zsh. Parsing either side was tried and defeated --
/// `--help` and `--completions` render only *visible* aliases, so a plain
/// `#[command(alias = ...)]` slipped past both, and text-scraping the zsh array
/// broke on quoting, comments, and line-continuations that changed no behaviour.
#[test]
fn the_wrapper_verb_allowlist_matches_the_cli() {
    let wrapper_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../shared/zsh/conf.d/91-tmux-attach.zsh"
    );
    let sandbox = Sandbox::empty();

    let cli: BTreeSet<String> = stdout(&sandbox.dmux(&["_verbs"]))
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    assert!(!cli.is_empty(), "`dmux _verbs` listed nothing");

    let wrapper = wrapper_verbs(wrapper_path);
    let missing: Vec<&String> = cli.difference(&wrapper).collect();
    let stale: Vec<&String> = wrapper.difference(&cli).collect();
    assert!(
        missing.is_empty() && stale.is_empty(),
        "91-tmux-attach.zsh drifted from the dmux CLI\n  \
         missing from the wrapper (so `ssa <verb>` would create a Space): {missing:?}\n  \
         stale in the wrapper (no such verb): {stale:?}"
    );
}

/// The wrapper's verb array, obtained by evaluating it rather than reading it.
/// Text-parsing this was tried and abandoned: single-quoting the elements,
/// interleaving a comment, line-continuations, or splitting the assignment in
/// two all changed the parse without changing one byte of runtime behaviour,
/// and a one-line comment that merely mentioned the array name captured the
/// parse entirely. zsh is the only thing that agrees with zsh.
fn wrapper_verbs(wrapper_path: &str) -> BTreeSet<String> {
    let out = Command::new("zsh")
        .args([
            "-f",
            "-c",
            &format!("source {wrapper_path} && print -rl -- $_dmux_verbs"),
        ])
        .output()
        .expect("zsh runs the wrapper");
    assert!(
        out.status.success(),
        "sourcing the wrapper failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
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
    assert_eq!(rows[0]["host"], this_host());
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
fn list_is_an_alias_for_ls() {
    let sandbox = Sandbox::with_tmux();
    assert_eq!(
        stdout(&sandbox.dmux(&["list"])),
        stdout(&sandbox.dmux(&["ls"]))
    );
}

/// A filtered listing keeps each row's index from the full merged set —
/// gaps and all — so the number it prints is the one con and rm act on.
#[test]
fn a_filtered_listing_keeps_the_merged_indices() {
    let sandbox = Sandbox::with_tmux_and_wez();
    let all: serde_json::Value =
        serde_json::from_str(&stdout(&sandbox.dmux(&["ls", "--json"]))).unwrap();
    assert_eq!(all[0]["name"], "work");
    assert_eq!(all[0]["kind"], "wez");
    assert_eq!(all[1]["name"], "alpha");
    let filtered: serde_json::Value =
        serde_json::from_str(&stdout(&sandbox.dmux(&["ls", "--tmux", "--json"]))).unwrap();
    assert_eq!(filtered[0]["name"], "alpha");
    assert_eq!(filtered[0]["index"], 2);
    assert_eq!(filtered[1]["name"], "beta");
    assert_eq!(filtered[1]["index"], 3);
    let wez_only: serde_json::Value =
        serde_json::from_str(&stdout(&sandbox.dmux(&["ls", "--wez", "--json"]))).unwrap();
    assert_eq!(wez_only[0]["name"], "work");
    assert_eq!(wez_only[0]["index"], 1);
    // The indices the filtered listing showed resolve to the same sessions.
    let output = sandbox.dmux(&["con", "2"]);
    assert_eq!(stdout(&output), "would exec: tmux attach -t '=alpha'\n");
    let output = sandbox.dmux(&["rm", "--yes", "3"]);
    assert_eq!(stdout(&output), "would run: tmux kill-session -t '=beta'\n");
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
    assert_eq!(stdout(&output), "would exec: tmux attach -t '=alpha'\n");
}

#[test]
fn the_aliases_reach_con() {
    let sandbox = Sandbox::with_tmux();
    for verb in ["attach", "a"] {
        let output = sandbox.dmux(&[verb, "alpha"]);
        assert_eq!(stdout(&output), "would exec: tmux attach -t '=alpha'\n");
    }
}

#[test]
fn an_unknown_word_falls_through_to_con() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["beta"]);
    assert_eq!(stdout(&output), "would exec: tmux attach -t '=beta'\n");
}

#[test]
fn a_numeric_target_resolves_against_the_listing() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["con", "2"]);
    assert_eq!(stdout(&output), "would exec: tmux attach -t '=beta'\n");
}

#[test]
fn inside_tmux_con_switches_the_client() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox
        .command(&["con", "alpha"])
        .env("TMUX", "/tmp/tmux-0/default,1,0")
        .output()
        .unwrap();
    assert_eq!(
        stdout(&output),
        "would exec: tmux switch-client -t '=alpha'\n"
    );
}

#[test]
fn a_window_is_selected_after_the_attach() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["con", "alpha", "-w", "2"]);
    assert_eq!(
        stdout(&output),
        "would exec: tmux attach -t '=alpha' ';' select-window -t '=alpha:2'\n"
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
fn con_create_attaches_when_the_session_exists() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["con", "-A", "alpha"]);
    assert!(output.status.success());
    assert_eq!(stdout(&output), "would exec: tmux attach -t '=alpha'\n");
}

#[test]
fn con_create_falls_back_to_new() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["con", "--create", "ghost"]);
    assert!(output.status.success());
    assert_eq!(
        stdout(&output),
        "would exec: tmux new-session -A -s ghost\n"
    );
}

#[test]
fn con_create_still_validates_the_name() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["con", "-A", "bad name"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("letters, numbers"));
}

#[test]
fn con_create_plans_a_quoted_remote_new_session() {
    let sandbox = Sandbox::empty();
    // ssh reaches the peer but tmux runs no server there: an empty listing.
    sandbox.stub("ssh", "exit 1");
    let output = sandbox.dmux(&["con", "-A", "ghost", "--host", peer()]);
    assert!(output.status.success());
    assert_eq!(
        stdout(&output),
        format!(
            "would exec: ssh -o 'ConnectTimeout=5' -t {} '{REMOTE_PATH}exec tmux new-session -A -s ghost'\n",
            peer()
        )
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
fn new_takes_a_directory_and_a_command() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["new", "scratch", "--dir", "/tmp/x", "--", "nvim", "."]);
    assert!(output.status.success());
    assert_eq!(
        stdout(&output),
        "would exec: tmux new-session -A -s scratch -c /tmp/x nvim .\n"
    );
}

/// The trailing command needs the `--` separator; a bare extra word is a
/// usage error, not a command.
#[test]
fn a_new_command_needs_the_separator() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["new", "scratch", "nvim"]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn a_remote_new_quotes_the_directory_and_command() {
    let sandbox = Sandbox::empty();
    let output = sandbox.dmux(&[
        "--host",
        peer(),
        "new",
        "scratch",
        "--dir",
        "/tmp/a b",
        "--",
        "echo",
        "hi there",
    ]);
    assert!(output.status.success());
    assert_eq!(
        stdout(&output),
        format!(
            "would exec: ssh -o 'ConnectTimeout=5' -t {} '{REMOTE_PATH}exec tmux new-session -A -s scratch -c '\\''/tmp/a b'\\'' echo '\\''hi there'\\'''\n",
            peer()
        )
    );
}

#[test]
fn detach_inside_tmux_detaches_the_client() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox
        .command(&["detach"])
        .env("TMUX", "/tmp/tmux-0/default,1,0")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(stdout(&output), "would exec: tmux detach-client\n");
}

#[test]
fn disconnect_and_legacy_detach_outside_clients_are_idempotent() {
    let sandbox = Sandbox::empty();
    for command in ["disconnect", "detach"] {
        let output = sandbox.dmux(&[command]);
        assert!(output.status.success(), "{command}");
        assert_eq!(stdout(&output), "nothing attached\n", "{command}");
        assert!(stderr(&output).is_empty(), "{command}");
    }
}

/// A wezterm window is not attached the way a tmux client is; detach says so
/// instead of guessing at an equivalent.
#[test]
fn detach_inside_wezterm_is_refused_honestly() {
    let sandbox = Sandbox::empty();
    let output = sandbox
        .command(&["detach"])
        .env("WEZTERM_UNIX_SOCKET", "/tmp/wezterm-sock")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("wezterm windows do not detach"));
}

#[test]
fn a_remote_named_session_goes_over_ssh() {
    let sandbox = Sandbox::empty();
    let output = sandbox.dmux(&["--host", peer(), "new", "scratch"]);
    assert_eq!(
        stdout(&output),
        format!(
            "would exec: ssh -o 'ConnectTimeout=5' -t {} '{REMOTE_PATH}exec tmux new-session -A -s scratch'\n",
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
            "would exec: ssh -o 'ConnectTimeout=5' -t {} '{REMOTE_PATH}exec tmux new-session -A -s main'\n",
            peer()
        )
    );
}

#[test]
fn a_bare_remote_attach_inside_wezterm_spawns_a_native_domain() {
    let sandbox = Sandbox::empty();
    let output = sandbox
        .command(&["--host", peer()])
        .env("WEZTERM_UNIX_SOCKET", "/tmp/wezterm-sock")
        .output()
        .unwrap();
    let expected = format!("would exec: wezterm cli spawn --domain-name {}-", peer());
    assert!(stdout(&output).starts_with(&expected));
}

/// tmux freezes its server's start environment, so WEZTERM_* seen inside
/// tmux may be a fossil from a wezterm session that no longer watches this
/// terminal: without a live TERM_PROGRAM=WezTerm the bare attach must take
/// the ssh tmux route, not spawn a tab in an unwatched GUI.
#[test]
fn stale_wezterm_env_inside_tmux_is_distrusted() {
    let sandbox = Sandbox::empty();
    let output = sandbox
        .command(&["--host", peer()])
        .env("TMUX", "/tmp/tmux-0/default,1,0")
        .env("WEZTERM_UNIX_SOCKET", "/tmp/wezterm-sock")
        .output()
        .unwrap();
    assert_eq!(
        stdout(&output),
        format!(
            "would exec: ssh -o 'ConnectTimeout=5' -t {} '{REMOTE_PATH}exec tmux new-session -A -s main'\n",
            peer()
        )
    );
    // With the terminal still vouching for wezterm, the native route stands.
    let output = sandbox
        .command(&["--host", peer()])
        .env("TMUX", "/tmp/tmux-0/default,1,0")
        .env("WEZTERM_UNIX_SOCKET", "/tmp/wezterm-sock")
        .env("TERM_PROGRAM", "WezTerm")
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
    assert_eq!(stdout(&output), "would exec: tmux attach -t '=alpha'\n");
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
    assert_eq!(stdout(&output), "would run: tmux kill-session -t '=beta'\n");
}

#[test]
fn rm_with_a_window_kills_only_that_window() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["rm", "--yes", "alpha", "-w", "2"]);
    assert_eq!(
        stdout(&output),
        "would run: tmux kill-window -t '=alpha:2'\n"
    );
}

#[test]
fn rm_all_kills_every_listed_session() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["rm", "--all", "--yes"]);
    assert!(output.status.success());
    assert_eq!(
        stdout(&output),
        "would run: tmux kill-session -t '=alpha'\nwould run: tmux kill-session -t '=beta'\n"
    );
}

/// The confirmation prompt names every session `--all` is about to kill.
#[test]
fn rm_all_confirmation_lists_the_victims() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux_stdin(&["rm", "--all"], "y\n");
    assert!(output.status.success());
    assert!(stderr(&output).contains("2 sessions (alpha, beta)"));
    let output = sandbox.dmux_stdin(&["rm", "--all"], "n\n");
    assert!(output.status.success());
    assert_eq!(stdout(&output), "");
    assert!(stderr(&output).contains("cancelled"));
}

/// Inside tmux, `--all` keeps the session this client sits in — killing it
/// would take the terminal down mid-sweep — and says how to kill it anyway.
#[test]
fn rm_all_keeps_the_session_it_is_inside() {
    let sandbox = Sandbox::empty();
    sandbox.stub(
        "tmux",
        "case \"$1\" in\n\
         list-sessions) printf 'alpha|1700000000|2|1\\nbeta|1700000100|1|0\\n' ;;\n\
         display-message) printf 'alpha\\n' ;;\n\
         esac",
    );
    let output = sandbox
        .command(&["rm", "--all", "--yes"])
        .env("TMUX", "/tmp/tmux-0/default,1,0")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(stdout(&output), "would run: tmux kill-session -t '=beta'\n");
    assert!(stderr(&output).contains("keeping current session 'alpha'"));
}

#[test]
fn rm_all_leaves_wezterm_workspaces_alone() {
    let sandbox = Sandbox::with_tmux_and_wez();
    let output = sandbox.dmux(&["rm", "--all", "--yes"]);
    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("'=alpha'"));
    assert!(text.contains("'=beta'"));
    assert!(!text.contains("work"), "{text}");
    assert!(stderr(&output).contains("wezterm workspace"));
}

#[test]
fn rm_all_with_nothing_running_is_a_no_op() {
    let sandbox = Sandbox::empty();
    let output = sandbox.dmux(&["rm", "--all", "--yes"]);
    assert!(output.status.success());
    assert_eq!(stdout(&output), "");
    assert!(stderr(&output).contains("nothing to kill"));
}

#[test]
fn rm_all_mixes_with_neither_targets_nor_window() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["rm", "--all", "beta"]);
    assert_eq!(output.status.code(), Some(2));
    let output = sandbox.dmux(&["rm", "--all", "-w", "2"]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn delete_is_an_alias_for_rm() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["delete", "--yes", "beta"]);
    assert!(output.status.success());
    assert_eq!(stdout(&output), "would run: tmux kill-session -t '=beta'\n");
}

#[test]
fn rm_refuses_a_target_that_does_not_exist() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["rm", "--yes", "nosuch"]);
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn rename_validates_only_the_new_name() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["rename", "alpha", "fresh"]);
    assert_eq!(
        stdout(&output),
        "would run: tmux rename-session -t '=alpha' fresh\n"
    );
    let output = sandbox.dmux(&["rename", "alpha", "bad name"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("letters, numbers"));
}

/// The old target resolves like con/rm's: a session another tool created
/// with a nonconforming name can be renamed, by that name or by its index.
#[test]
fn rename_reaches_sessions_with_hostile_names() {
    let sandbox = Sandbox::empty();
    sandbox.stub(
        "tmux",
        "case \"$1\" in\n\
         list-sessions) printf 'a b|1700000000|1|0\\n' ;;\n\
         esac",
    );
    for old in ["a b", "1"] {
        let output = sandbox.dmux(&["rename", old, "fresh"]);
        assert!(
            output.status.success(),
            "rename {old}: {:?}",
            stderr(&output)
        );
        assert_eq!(
            stdout(&output),
            "would run: tmux rename-session -t '=a b' fresh\n"
        );
    }
    // But a target that names no session is still refused.
    let output = sandbox.dmux(&["rename", "ghost", "fresh"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("no session 'ghost'"));
}

#[test]
fn rename_refuses_a_wezterm_workspace() {
    let sandbox = Sandbox::with_tmux_and_wez();
    let output = sandbox.dmux(&["rename", "work", "fresh"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("wezterm workspace"));
}

/// The remote rename must quote its `=old` target, or zsh's equals
/// expansion mangles it before tmux ever sees it.
#[test]
fn a_remote_rename_quotes_the_exact_target() {
    let sandbox = Sandbox::empty();
    // The stub peer's tmux answers the listing probe with one session.
    sandbox.stub("ssh", "printf 'alpha|1700000000|1|0\\n'");
    let output = sandbox.dmux(&["rename", "alpha", "fresh", "--host", peer()]);
    assert!(output.status.success());
    assert_eq!(
        stdout(&output),
        format!(
            "would run: ssh -o 'ConnectTimeout=5' {} '{REMOTE_PATH}tmux rename-session -t '\\''=alpha'\\'' fresh'\n",
            peer()
        )
    );
}

#[test]
fn a_remote_listing_needs_ssh() {
    let sandbox = Sandbox::empty();
    let output = sandbox.dmux(&["ls", "--host", peer()]);
    assert_eq!(output.status.code(), Some(1));
}

/// ssh exiting 255 is ssh itself failing; its first stderr line explains
/// why and belongs in the error instead of being thrown away.
#[test]
fn an_unreachable_peer_surfaces_the_ssh_error() {
    let sandbox = Sandbox::empty();
    sandbox.stub(
        "ssh",
        "echo 'ssh: connect to host peer port 22: Connection refused' >&2\nexit 255",
    );
    let output = sandbox.dmux(&["ls", "--host", peer()]);
    assert_eq!(output.status.code(), Some(1));
    let text = stderr(&output);
    assert!(text.contains(&format!("cannot reach {}", peer())), "{text}");
    assert!(text.contains("Connection refused"), "{text}");
}

/// A nonzero exit below 255 is the remote tmux talking ("no server
/// running"): the host is reachable and simply has no sessions.
#[test]
fn a_reachable_peer_without_tmux_is_an_empty_listing() {
    let sandbox = Sandbox::empty();
    sandbox.stub("ssh", "exit 1");
    let output = sandbox.dmux(&["ls", "--host", peer(), "--json"]);
    assert!(output.status.success());
    assert_eq!(stdout(&output), "[]\n");
}

/// Exit 127 is not tmux talking — it is the remote shell failing to find
/// tmux at all (macOS sshd's non-interactive PATH, or tmux missing). That
/// must be a loud error, never a silent `[]` masquerading as no sessions.
#[test]
fn a_peer_whose_shell_cannot_find_tmux_is_a_hard_error() {
    let sandbox = Sandbox::empty();
    sandbox.stub("ssh", "echo 'zsh:1: command not found: tmux' >&2\nexit 127");
    for args in [
        ["ls", "--host", peer()].as_slice(),
        ["ls", "--host", peer(), "--json"].as_slice(),
    ] {
        let output = sandbox.dmux(args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        let text = stderr(&output);
        assert!(
            text.contains(&format!(
                "tmux not found on {} (non-interactive ssh PATH)",
                peer()
            )),
            "{args:?}: {text}"
        );
        assert_eq!(stdout(&output), "", "{args:?}");
    }
}

/// Some shells report the missing command without exiting 127 themselves;
/// the stderr wording is the tell.
#[test]
fn a_command_not_found_on_stderr_is_the_same_hard_error() {
    let sandbox = Sandbox::empty();
    sandbox.stub("ssh", "echo 'sh: tmux: command not found' >&2\nexit 1");
    let output = sandbox.dmux(&["ls", "--host", peer(), "--json"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains(&format!("tmux not found on {}", peer())));
}

#[test]
fn a_remote_json_listing_names_its_host() {
    let sandbox = Sandbox::empty();
    sandbox.stub("ssh", "printf 'main|1700000000|1|0\\n'");
    let output = sandbox.dmux(&["ls", "--host", peer(), "--json"]);
    let rows: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(rows[0]["name"], "main");
    assert_eq!(rows[0]["host"], peer());
}

/// Remote wezterm workspaces are not listable over ssh: a human asking gets
/// an error, while script modes get a well-formed empty result and the
/// explanation on stderr.
#[test]
fn a_remote_wez_listing_is_refused_honestly() {
    let sandbox = Sandbox::empty();
    sandbox.stub("ssh", "printf 'main|1700000000|1|0\\n'");
    let output = sandbox.dmux(&["ls", "--wez", "--host", peer()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("not listable over ssh"));
    let output = sandbox.dmux(&["ls", "--wez", "--json", "--host", peer()]);
    assert!(output.status.success());
    assert_eq!(stdout(&output), "[]\n");
    assert!(stderr(&output).contains("not listable over ssh"));
    let output = sandbox.dmux(&["ls", "--wez", "--names", "--host", peer()]);
    assert!(output.status.success());
    assert_eq!(stdout(&output), "");
    assert!(stderr(&output).contains("not listable over ssh"));
}

#[test]
fn an_empty_listing_is_an_empty_json_array() {
    let sandbox = Sandbox::empty();
    let output = sandbox.dmux(&["ls", "--json"]);
    assert!(output.status.success());
    assert_eq!(stdout(&output), "[]\n");
}

#[test]
fn names_and_json_refuse_to_mix() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux(&["ls", "--names", "--json"]);
    assert_eq!(output.status.code(), Some(2));
}

/// One workspace spanning several panes and windows is one row counting
/// distinct windows, not one row per pane.
#[test]
fn wez_workspaces_deduplicate_panes_into_windows() {
    let sandbox = Sandbox::with_tmux();
    sandbox.stub(
        "wezterm",
        r#"printf '[{"window_id":1,"pane_id":1,"workspace":"work"},{"window_id":1,"pane_id":2,"workspace":"work"},{"window_id":2,"pane_id":3,"workspace":"work"},{"window_id":3,"pane_id":4,"workspace":"other"}]'"#,
    );
    let output = sandbox.dmux(&["ls", "--wez", "--json"]);
    let rows: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(rows[0]["name"], "other");
    assert_eq!(rows[0]["windows"], 1);
    assert_eq!(rows[1]["name"], "work");
    assert_eq!(rows[1]["windows"], 2);
    assert!(rows.get(2).is_none());
}

/// WEZTERM_PANE marks the workspace this process sits in — but only when
/// the wezterm env is trustworthy; inside a foreign tmux it is a fossil.
#[test]
fn the_attached_workspace_is_the_one_holding_wezterm_pane() {
    let sandbox = Sandbox::with_tmux_and_wez();
    let attached = |command: &mut Command| -> serde_json::Value {
        let output = command.output().unwrap();
        let rows: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
        assert_eq!(rows[0]["name"], "work");
        rows[0]["attached"].clone()
    };
    let args = ["ls", "--wez", "--json"];
    assert_eq!(
        attached(sandbox.command(&args).env("WEZTERM_PANE", "1")),
        true
    );
    assert_eq!(
        attached(sandbox.command(&args).env("WEZTERM_PANE", "9")),
        false
    );
    // The same pane id frozen into a tmux environment proves nothing.
    assert_eq!(
        attached(
            sandbox
                .command(&args)
                .env("WEZTERM_PANE", "1")
                .env("TMUX", "/tmp/tmux-0/default,1,0")
        ),
        false
    );
    // Unless the terminal itself still says WezTerm.
    assert_eq!(
        attached(
            sandbox
                .command(&args)
                .env("WEZTERM_PANE", "1")
                .env("TMUX", "/tmp/tmux-0/default,1,0")
                .env("TERM_PROGRAM", "WezTerm")
        ),
        true
    );
}

/// A wezterm that answers with garbage contributes nothing; the tmux half
/// of the listing still stands.
#[test]
fn malformed_wezterm_json_degrades_to_tmux_only() {
    let sandbox = Sandbox::with_tmux();
    sandbox.stub("wezterm", "printf 'not json at all'");
    let output = sandbox.dmux(&["ls", "--json"]);
    assert!(output.status.success());
    let rows: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(rows[0]["name"], "alpha");
    assert_eq!(rows[1]["name"], "beta");
    assert!(rows.get(2).is_none());
}

/// When one of several kills fails the exit code says so, and the rest are
/// still attempted.
#[test]
fn rm_reports_a_partial_failure() {
    let sandbox = Sandbox::empty();
    sandbox.stub(
        "tmux",
        "case \"$1\" in\n\
         list-sessions) printf 'alpha|1700000000|2|1\\nbeta|1700000100|1|0\\n' ;;\n\
         kill-session) [ \"$3\" = '=beta' ] && exit 1; exit 0 ;;\n\
         esac",
    );
    let output = sandbox
        .command(&["rm", "--yes", "alpha", "beta"])
        .env_remove("DMUX_DRY_RUN")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let output = sandbox
        .command(&["rm", "--yes", "alpha"])
        .env_remove("DMUX_DRY_RUN")
        .output()
        .unwrap();
    assert!(output.status.success());
}

#[test]
fn rm_confirmation_reads_yes_and_no() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox.dmux_stdin(&["rm", "beta"], "y\n");
    assert!(output.status.success());
    assert_eq!(stdout(&output), "would run: tmux kill-session -t '=beta'\n");
    assert!(stderr(&output).contains("Kill session 'beta'"));
    let output = sandbox.dmux_stdin(&["rm", "beta"], "n\n");
    assert!(output.status.success());
    assert_eq!(stdout(&output), "");
    assert!(stderr(&output).contains("cancelled"));
}

/// A real (non-dry-run) kill with no terminal on stdin refuses up front
/// instead of blocking on a prompt nobody will answer.
#[test]
fn rm_without_a_terminal_demands_yes() {
    let sandbox = Sandbox::with_tmux();
    let output = sandbox
        .command(&["rm", "beta"])
        .env_remove("DMUX_DRY_RUN")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("--yes"));
}

/// The fallthrough accepts the same `-w/--window` the `con` verb does, in
/// every spelling clap would take there, and routes it identically.
#[test]
fn the_fallthrough_accepts_a_window() {
    let sandbox = Sandbox::with_tmux();
    for args in [
        ["beta", "-w", "2"].as_slice(),
        ["beta", "--window", "2"].as_slice(),
        ["beta", "--window=2"].as_slice(),
        ["beta", "-w2"].as_slice(),
    ] {
        let output = sandbox.dmux(args);
        assert_eq!(
            stdout(&output),
            "would exec: tmux attach -t '=beta' ';' select-window -t '=beta:2'\n",
            "{args:?}"
        );
    }
}

/// Anything beyond the one window flag keeps the strict error.
#[test]
fn the_fallthrough_rejects_other_extras() {
    let sandbox = Sandbox::with_tmux();
    for args in [
        ["beta", "-x", "2"].as_slice(),
        ["beta", "2"].as_slice(),
        ["beta", "-w"].as_slice(),
        ["beta", "-w", "2", "extra"].as_slice(),
    ] {
        let output = sandbox.dmux(args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(
            stderr(&output).contains("unexpected arguments"),
            "{args:?}: {}",
            stderr(&output)
        );
    }
}

/// Inside wezterm (per the trust rule), attaching a wez workspace activates
/// one of its panes so the GUI switches to it; the pane id comes from the
/// same `wezterm cli list` the listing already runs.
#[test]
fn inside_wezterm_a_workspace_attach_activates_a_pane() {
    let sandbox = Sandbox::with_tmux_and_wez();
    let output = sandbox
        .command(&["con", "work"])
        .env("WEZTERM_UNIX_SOCKET", "/tmp/wezterm-sock")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        stdout(&output),
        "would exec: wezterm cli activate-pane --pane-id 1\n"
    );
}

#[test]
fn outside_wezterm_a_workspace_attach_still_refuses() {
    let sandbox = Sandbox::with_tmux_and_wez();
    let output = sandbox.dmux(&["con", "work"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("switch to it inside wezterm"));
}

/// Several panes in a workspace: the lowest pane id is the stable handle.
#[test]
fn the_activated_pane_is_the_workspaces_lowest() {
    let sandbox = Sandbox::with_tmux();
    sandbox.stub(
        "wezterm",
        r#"printf '[{"window_id":2,"pane_id":7,"workspace":"work"},{"window_id":1,"pane_id":3,"workspace":"work"}]'"#,
    );
    let output = sandbox
        .command(&["con", "work"])
        .env("WEZTERM_UNIX_SOCKET", "/tmp/wezterm-sock")
        .output()
        .unwrap();
    assert_eq!(
        stdout(&output),
        "would exec: wezterm cli activate-pane --pane-id 3\n"
    );
}

#[test]
fn a_window_flag_does_not_apply_to_a_workspace() {
    let sandbox = Sandbox::with_tmux_and_wez();
    let output = sandbox
        .command(&["con", "work", "-w", "2"])
        .env("WEZTERM_UNIX_SOCKET", "/tmp/wezterm-sock")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("wezterm workspace"));
}

#[test]
fn doctor_still_prints_the_human_report() {
    let sandbox = Sandbox::with_tmux_and_wez();
    sandbox.stub("ssh", "exit 255");
    let output = sandbox.dmux(&["doctor"]);
    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains("host"), "{text}");
    assert!(text.contains("tmux server"), "{text}");
    assert!(text.contains("running (2 sessions)"), "{text}");
    assert!(text.contains("wezterm cli"), "{text}");
}

/// `--json` carries the same probes as an object of name -> {ok, detail}.
#[test]
fn doctor_json_is_machine_readable() {
    let sandbox = Sandbox::with_tmux_and_wez();
    sandbox.stub("ssh", "exit 255");
    let output = sandbox.dmux(&["doctor", "--json"]);
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    for probe in [
        "host",
        "peer",
        "inside_wezterm",
        "inside_tmux",
        "wezterm_cli",
        "tmux_server",
        "usb_link",
        "ssh_peer",
        "state",
    ] {
        assert!(report[probe]["ok"].is_boolean(), "{probe}");
        assert!(report[probe]["detail"].is_string(), "{probe}");
    }
    assert_eq!(report["host"]["ok"], true);
    assert!(
        report["host"]["detail"]
            .as_str()
            .unwrap()
            .contains(this_host())
    );
    assert_eq!(report["tmux_server"]["ok"], true);
    assert_eq!(report["tmux_server"]["detail"], "running (2 sessions)");
    assert_eq!(report["wezterm_cli"]["ok"], true);
    assert_eq!(report["inside_tmux"]["ok"], false);
    assert_eq!(report["inside_wezterm"]["ok"], false);
    assert_eq!(report["ssh_peer"]["ok"], false);
    assert_eq!(report["state"]["ok"], true);
}

/// Only the standalone toggle spelling is rewritten: a `-` handed to a verb
/// reaches clap untouched and earns the ordinary missing-session error, not
/// a confusing complaint about `@prev`.
#[test]
fn a_dash_after_a_verb_is_not_the_toggle() {
    let sandbox = Sandbox::with_tmux();
    for args in [&["rm", "-"], &["con", "-"]] {
        let output = sandbox.dmux(args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        let text = stderr(&output);
        assert!(text.contains("no session '-'"), "{args:?}: {text}");
        assert!(!text.contains("@prev"), "{args:?}: {text}");
    }
}
