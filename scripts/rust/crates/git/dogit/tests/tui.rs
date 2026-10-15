#![cfg(unix)]
#![forbid(unsafe_code)]

use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use testkit::executable;
use testkit::pty::{open_pty, read_available, stdio, take_controlling_terminal, terminal_state};

const KEYBOARD_PUSH: &[u8] = b"\x1b[>1u";
const KEYBOARD_POP: &[u8] = b"\x1b[<1u";

struct Fixture {
    directory: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repo");
        fs::create_dir(&root).unwrap();
        let fixture = Self { directory, root };
        fixture.git(&["init", "-q", "-b", "main"]);
        fixture.git(&["config", "user.name", "Dogit integration"]);
        fixture.git(&["config", "user.email", "dogit@example.invalid"]);
        fixture.git(&["config", "commit.gpgsign", "false"]);
        fixture.git(&["config", "core.autocrlf", "false"]);
        fixture.git(&["config", "core.hooksPath", "/dev/null"]);
        fixture
    }

    fn configure(&self, command: &mut Command) {
        command
            .current_dir(&self.root)
            .env("HOME", self.directory.path().join("home"))
            .env("XDG_STATE_HOME", self.directory.path().join("state"))
            .env("XDG_CONFIG_HOME", self.directory.path().join("config"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor")
            .env("DOGIT_REDUCED_MOTION", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_EDITOR")
            .env_remove("GIT_SEQUENCE_EDITOR")
            .env_remove("GIT_CONFIG_COUNT");
    }

    fn git(&self, args: &[&str]) -> Vec<u8> {
        let mut command = Command::new("git");
        self.configure(&mut command);
        let output = command
            .arg("--literal-pathspecs")
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    fn write(&self, path: &str, content: &[u8]) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn commit(&self, path: &str, content: &[u8]) {
        self.write(path, content);
        self.git(&["add", "--", path]);
        self.git(&["commit", "-qm", "initial fixture"]);
    }

    fn editor(&self) -> PathBuf {
        let editor = self.directory.path().join("editor");
        executable(
            &editor,
            "#!/bin/sh\nset -eu\n[ -t 0 ] && [ -t 1 ] && [ -t 2 ]\nprintf '%s\\0' \"$@\" >> \"$DOGIT_TEST_EDITOR_ARGS\"\nprintf 'DOGIT_EDITOR_FINISHED\\n'\n",
        );
        editor
    }

    fn open(&self, args: &[&str]) -> Terminal {
        self.open_with(args, |_| {})
    }

    fn open_with(&self, args: &[&str], configure: impl FnOnce(&mut Command)) -> Terminal {
        let (master, slave, before) = open_pty(34, 124);
        let (input, output, errors) = stdio(&slave);
        let mut command = Command::new(env!("CARGO_BIN_EXE_dogit"));
        self.configure(&mut command);
        command
            .args(args)
            .stdin(input)
            .stdout(output)
            .stderr(errors)
            .env("GIT_EDITOR", self.editor())
            .env(
                "DOGIT_TEST_EDITOR_ARGS",
                self.directory.path().join("editor-args"),
            )
            .env("DOGIT_TEST_HOOK", self.directory.path().join("hook-ran"));
        configure(&mut command);
        take_controlling_terminal(&mut command);
        let child = command.spawn().unwrap();
        drop(slave);
        let mut terminal = Terminal {
            child,
            master,
            output: Vec::new(),
            initial_flags: [
                before.c_iflag as u64,
                before.c_oflag as u64,
                before.c_lflag as u64,
            ],
        };
        terminal.wait_for("Changes", 0);
        terminal
    }

    fn saved_state(&self) -> serde_json::Value {
        let files = fs::read_dir(self.directory.path().join("state/dogit")).unwrap();
        let file = files
            .map(Result::unwrap)
            .find(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "json")
            })
            .unwrap();
        serde_json::from_slice(&fs::read(file.path()).unwrap()).unwrap()
    }
}

struct Terminal {
    child: Child,
    master: File,
    output: Vec<u8>,
    initial_flags: [u64; 3],
}

impl Terminal {
    fn send(&mut self, keys: &[u8]) -> usize {
        read_available(&self.master, &mut self.output, 0);
        let start = self.output.len();
        self.master.write_all(keys).unwrap();
        start
    }

    fn wait_for(&mut self, text: &str, from: usize) {
        self.wait_until(
            |terminal| String::from_utf8_lossy(&terminal.output[from..]).contains(text),
            text,
        );
    }

    fn wait_until(&mut self, mut condition: impl FnMut(&Self) -> bool, description: &str) {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            read_available(&self.master, &mut self.output, 20);
            if condition(self) {
                return;
            }
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "dogit exited while waiting for {description}: {}",
                self.transcript()
            );
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {description}: {}",
                self.transcript()
            );
        }
    }

    fn settle(&mut self) {
        let until = Instant::now() + Duration::from_millis(120);
        while Instant::now() < until {
            read_available(&self.master, &mut self.output, 20);
        }
    }

    fn transcript(&self) -> String {
        String::from_utf8_lossy(&self.output[self.output.len().saturating_sub(12_000)..])
            .into_owned()
    }

    fn handoff_finished(&mut self, from: usize) {
        self.wait_until(
            |terminal| {
                terminal.output[from..]
                    .windows(8)
                    .any(|bytes| bytes == b"\x1b[?1049h")
            },
            "terminal handoff to return",
        );
        self.settle();
        let handoff = &self.output[from..];
        let before = handoff
            .windows(KEYBOARD_POP.len())
            .position(|bytes| bytes == KEYBOARD_POP)
            .unwrap();
        let after = handoff
            .windows(KEYBOARD_PUSH.len())
            .position(|bytes| bytes == KEYBOARD_PUSH)
            .unwrap();
        assert!(
            before < after,
            "keyboard protocol was not suspended during handoff"
        );
    }

    fn quit(&mut self) {
        self.exit_with(b"q");
    }

    fn exit_with(&mut self, keys: &[u8]) {
        self.send(keys);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            read_available(&self.master, &mut self.output, 20);
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "{}", self.transcript());
                break;
            }
            assert!(
                Instant::now() < deadline,
                "dogit did not quit: {}",
                self.transcript()
            );
        }
        let after = terminal_state(&self.master);
        assert_eq!(
            self.initial_flags,
            [
                after.c_iflag as u64,
                after.c_oflag as u64,
                after.c_lflag as u64
            ]
        );
        assert!(self.output.windows(8).any(|bytes| bytes == b"\x1b[?1049l"));
        assert!(self.output.windows(6).any(|bytes| bytes == b"\x1b[?25h"));
        let occurrences = |needle: &[u8]| {
            self.output
                .windows(needle.len())
                .filter(|bytes| *bytes == needle)
                .count()
        };
        assert!(occurrences(KEYBOARD_PUSH) > 0);
        assert_eq!(occurrences(KEYBOARD_PUSH), occurrences(KEYBOARD_POP));
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn startup_palette_history_and_quit_restore_terminal() {
    let fixture = Fixture::new();
    fixture.commit("main.rs", b"fn main() {}\n");
    let mut terminal = fixture.open(&[]);
    terminal.wait_for("main", 0);
    let start = terminal.send(b"\x10");
    terminal.wait_for("Commands", start);
    terminal.send(b"history\r");
    terminal.wait_for("initial fixture", start);
    let start = terminal.send(b"\r");
    terminal.wait_for("All changes", start);
    terminal.quit();
    assert!(fixture.saved_state()["widths"]["History"].is_number());
}

#[test]
fn ordinary_text_drag_does_not_invoke_clipboard_or_mutate_git_or_operation_history() {
    let fixture = Fixture::new();
    fixture.commit("file", b"original\n");
    fixture.write("file", b"working changes stay untouched\n");
    let head = fixture.git(&["rev-parse", "HEAD"]);
    let index = fixture.git(&["write-tree"]);
    let worktree = fs::read(fixture.root.join("file")).unwrap();
    let clipboard = fixture.directory.path().join("clipboard");
    let bin = fixture.directory.path().join("bin");
    fs::create_dir(&bin).unwrap();
    executable(
        &bin.join("dclip"),
        "#!/bin/sh\nset -eu\ncat > \"$DOGIT_TEST_CLIPBOARD\"\n",
    );
    let inherited_path = std::env::var_os("PATH").unwrap_or_default();
    let path =
        std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&inherited_path)))
            .unwrap();
    let mut terminal = fixture.open_with(&["compare", "HEAD", "HEAD"], |command| {
        command
            .env("PATH", path)
            .env("DOGIT_TEST_CLIPBOARD", &clipboard);
    });
    let message = "History comparisons are read-only";
    // Ratatui can skip spaces already present on screen between these words.
    terminal.wait_for("read-only", 0);
    terminal.settle();
    // At this fixture's 124×34 size the empty comparison starts at column 39,
    // row 7 in SGR mouse coordinates. App-delivered drags must not copy text;
    // terminal-native selection and copying are handled outside the PTY.
    let last_column = 39 + message.len() - 1;
    terminal.send(format!("\x1b[<0;39;7M\x1b[<32;{last_column};7M").as_bytes());
    terminal.settle();
    assert!(
        !clipboard.exists(),
        "dragging must not invoke the clipboard"
    );
    terminal.send(format!("\x1b[<0;{last_column};7m").as_bytes());
    terminal.settle();
    assert!(!clipboard.exists(), "releasing a drag must not copy text");
    terminal.quit();
    assert!(!clipboard.exists(), "text copying belongs to the terminal");
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(fixture.git(&["write-tree"]), index);
    assert_eq!(fs::read(fixture.root.join("file")).unwrap(), worktree);
    assert!(
        fixture.saved_state()["operations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn direct_line_and_hunk_staging_preserve_worktree_and_can_unstage() {
    let fixture = Fixture::new();
    fixture.commit("file", b"first\nlast\n");
    let worktree = b"first\nchosen\nremaining\nlast\n";
    fixture.write("file", worktree);
    let mut terminal = fixture.open(&[]);
    terminal.wait_for("remaining", 0);
    terminal.send(b"2jj s");
    terminal.wait_until(
        |_| fixture.git(&["show", ":file"]) == b"first\nchosen\nlast\n",
        "only chosen line staged",
    );
    terminal.settle();
    let start = terminal.send(b"b");
    terminal.wait_for("Staged", start);
    terminal.settle();
    terminal.send(b"jj u");
    terminal.wait_until(
        |_| fixture.git(&["show", ":file"]) == b"first\nlast\n",
        "selected line unstaged",
    );
    terminal.settle();
    let start = terminal.send(b"b");
    terminal.wait_for("remaining", start);
    terminal.send(b"s");
    terminal.wait_until(
        |_| fixture.git(&["show", ":file"]) == worktree,
        "whole hunk staged",
    );
    terminal.settle();
    assert_eq!(fs::read(fixture.root.join("file")).unwrap(), worktree);
    terminal.quit();
    let state = fixture.saved_state();
    assert!(
        state["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|operation| operation["action"] == "Stage selection")
    );
}

#[test]
fn editing_directory_and_literal_filename_hands_off_a_real_terminal() {
    let fixture = Fixture::new();
    let directory = "a directory's";
    let file = "a directory's/f '; $(touch sentinel).rs";
    fixture.commit(file, b"old\n");
    fixture.write(file, b"new\n");
    let mut terminal = fixture.open(&[]);
    terminal.wait_for(directory, 0);
    let start = terminal.send(b"\x1b[He");
    terminal.wait_for("DOGIT_EDITOR_FINISHED", start);
    terminal.handoff_finished(start);
    let start = terminal.send(b"je");
    terminal.wait_for("DOGIT_EDITOR_FINISHED", start);
    terminal.handoff_finished(start);
    terminal.quit();
    let arguments = fs::read(fixture.directory.path().join("editor-args")).unwrap();
    let paths = arguments
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(paths.len(), 2);
    assert_eq!(
        paths[0],
        fixture.root.join(directory).as_os_str().as_encoded_bytes()
    );
    assert_eq!(
        paths[1],
        fixture.root.join(file).as_os_str().as_encoded_bytes()
    );
    assert!(!fixture.root.join("sentinel").exists());
    assert!(
        fixture.saved_state()["operations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn empty_editor_commit_cancellation_is_quiet_and_preserves_index() {
    let fixture = Fixture::new();
    fixture.commit("file", b"old\n");
    fixture.write("file", b"staged\n");
    fixture.git(&["add", "file"]);
    let head = fixture.git(&["rev-parse", "HEAD"]);
    let mut terminal = fixture.open(&[]);
    terminal.wait_for("main", 0);
    let start = terminal.send(b"c");
    terminal.wait_for("DOGIT_EDITOR_FINISHED", start);
    terminal.handoff_finished(start);
    terminal.quit();
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(fixture.git(&["show", ":file"]), b"staged\n");
    assert!(
        fixture.saved_state()["operations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn inline_commit_runs_interactive_hooks_and_pushes_to_local_remote() {
    let fixture = Fixture::new();
    fixture.commit("file", b"old\n");
    let remote = fixture.directory.path().join("remote.git");
    fixture.git(&["init", "--bare", "-q", remote.to_str().unwrap()]);
    fixture.git(&["remote", "add", "origin", remote.to_str().unwrap()]);
    fixture.git(&["push", "-qu", "origin", "main"]);
    let hooks = fixture.directory.path().join("hooks");
    fs::create_dir(&hooks).unwrap();
    executable(
        &hooks.join("pre-commit"),
        "#!/bin/sh\nset -eu\n[ -t 1 ] && [ -t 2 ]\n: < /dev/tty\nprintf 'hook had a terminal\\n' > \"$DOGIT_TEST_HOOK\"\n",
    );
    fixture.git(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
    fixture.write("file", b"committed\n");
    fixture.git(&["add", "file"]);
    let mut terminal = fixture.open(&[]);
    terminal.wait_for("main", 0);
    let start = terminal.send(b"C");
    terminal.wait_for("Create commit", start);
    let start = terminal.send(b"Commit 'from' $(touch sentinel) dogit\r\rA useful body.\x13");
    terminal.handoff_finished(start);
    assert_eq!(
        fixture.git(&["log", "-1", "--format=%B"]),
        b"Commit 'from' $(touch sentinel) dogit\n\nA useful body.\n\n"
    );
    assert_eq!(
        fs::read(fixture.directory.path().join("hook-ran")).unwrap(),
        b"hook had a terminal\n"
    );
    assert!(!fixture.root.join("sentinel").exists());
    let start = terminal.send(b"P");
    terminal.handoff_finished(start);
    terminal.quit();
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(&remote)
        .args(["rev-parse", "main"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, fixture.git(&["rev-parse", "HEAD"]));
    let state = fixture.saved_state();
    let operations = state["operations"].as_array().unwrap();
    assert!(
        operations
            .iter()
            .any(|operation| operation["action"] == "Commit" && operation["failed"] == false)
    );
    assert!(
        operations
            .iter()
            .any(|operation| operation["action"] == "Push" && operation["failed"] == false)
    );
}

#[test]
fn rejected_hook_returns_to_the_workbench_and_keeps_staged_changes() {
    let fixture = Fixture::new();
    fixture.commit("file", b"old\n");
    let hooks = fixture.directory.path().join("hooks");
    fs::create_dir(&hooks).unwrap();
    executable(
        &hooks.join("pre-commit"),
        "#!/bin/sh\nprintf 'fix this before committing\\n' >&2\nexit 1\n",
    );
    fixture.git(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
    fixture.write("file", b"staged\n");
    fixture.git(&["add", "file"]);
    let head = fixture.git(&["rev-parse", "HEAD"]);
    let mut terminal = fixture.open(&[]);
    terminal.wait_for("main", 0);
    let start = terminal.send(b"C");
    terminal.wait_for("Create commit", start);
    let start = terminal.send(b"Blocked commit\x13");
    terminal.wait_for("fix this before committing", start);
    terminal.handoff_finished(start);
    terminal.quit();
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(fixture.git(&["show", ":file"]), b"staged\n");
    assert_eq!(fixture.saved_state()["operations"][0]["failed"], true);
}

#[test]
fn editor_handoff_preserves_non_utf8_filename_bytes() {
    use std::os::unix::ffi::OsStrExt;
    let fixture = Fixture::new();
    let filename = std::ffi::OsStr::from_bytes(b"odd\xff' file.rs");
    let path = fixture.root.join(filename);
    fs::write(&path, b"old payload\n").unwrap();
    fixture.git(&["add", "."]);
    fixture.git(&["commit", "-qm", "initial fixture"]);
    fs::write(&path, b"new payload\n").unwrap();
    let mut terminal = fixture.open(&[]);
    terminal.wait_for("new payload", 0);
    let start = terminal.send(b"e");
    terminal.wait_for("DOGIT_EDITOR_FINISHED", start);
    terminal.handoff_finished(start);
    terminal.quit();
    let arguments = fs::read(fixture.directory.path().join("editor-args")).unwrap();
    assert_eq!(
        arguments.strip_suffix(&[0]).unwrap(),
        path.as_os_str().as_bytes()
    );
}

#[test]
fn dragged_pane_width_survives_workspace_switches_and_restart() {
    let fixture = Fixture::new();
    fixture.commit("file", b"original\n");
    fixture.write("file", b"resize fixture\n");
    let mut terminal = fixture.open(&[]);
    terminal.wait_for("resize fixture", 0);
    // Grab two cells to the right of the divider, retaining the grab offset.
    terminal.send(b"\x1b[<0;38;12M\x1b[<32;50;12M\x1b[<0;50;12m");
    terminal.settle();
    let start = terminal.send(b"\t");
    terminal.wait_for("initial fixture", start);
    let start = terminal.send(b"\x1b[Z");
    terminal.wait_for("resize fixture", start);
    terminal.quit();
    assert_eq!(fixture.saved_state()["widths"]["Changes"], 46);
    let mut restarted = fixture.open(&[]);
    restarted.wait_for("resize fixture", 0);
    restarted.quit();
    assert_eq!(fixture.saved_state()["widths"]["Changes"], 46);
}

#[test]
fn destructive_confirmation_refuses_external_edits_then_discards_exact_file() {
    let fixture = Fixture::new();
    fixture.commit("file", b"original\n");
    fixture.write("file", b"selected edit\n");
    fixture.write("unrelated", b"keep this untracked file\n");
    let mut terminal = fixture.open(&[]);
    terminal.wait_for("selected edit", 0);
    let start = terminal.send(b"d");
    terminal.wait_for("Discard selected changes", start);
    fixture.write("file", b"external edit after confirmation\n");
    let start = terminal.send(b"\r");
    terminal.wait_for("changed after confirmation", start);
    assert_eq!(
        fs::read(fixture.root.join("file")).unwrap(),
        b"external edit after confirmation\n"
    );
    assert_eq!(fixture.git(&["show", ":file"]), b"original\n");
    let start = terminal.send(b"d");
    terminal.wait_for("Discard selected changes", start);
    terminal.send(b"\r");
    terminal.wait_until(
        |_| fs::read(fixture.root.join("file")).unwrap() == b"original\n",
        "confirmed selected file discarded",
    );
    terminal.settle();
    terminal.quit();
    assert_eq!(
        fs::read(fixture.root.join("unrelated")).unwrap(),
        b"keep this untracked file\n"
    );
    assert_eq!(fixture.git(&["show", ":file"]), b"original\n");
}

#[test]
fn composer_reviews_all_staged_files_without_unstaged_content() {
    let fixture = Fixture::new();
    fixture.commit("first.rs", b"fn original() {}\n");
    fixture.write("first.rs", b"fn staged_one() {}\n");
    fixture.write("second.rs", b"fn staged_two() {}\n");
    fixture.git(&["add", "."]);
    fixture.write("first.rs", b"fn working_only() {}\n");
    let mut terminal = fixture.open(&[]);
    terminal.wait_for("working_only", 0);
    let start = terminal.send(b"C");
    terminal.wait_for("Commit contents", start);
    terminal.wait_for("staged_one", start);
    terminal.wait_for("staged_two", start);
    let composer = String::from_utf8_lossy(&terminal.output[start..]);
    assert!(composer.contains("first.rs"));
    assert!(composer.contains("second.rs"));
    assert!(!composer.contains("working_only"));
    terminal.send(b"\x1b");
    terminal.settle();
    terminal.quit();
    assert_eq!(fixture.git(&["show", ":first.rs"]), b"fn staged_one() {}\n");
    assert_eq!(
        fixture.git(&["show", ":second.rs"]),
        b"fn staged_two() {}\n"
    );
    assert_eq!(
        fs::read(fixture.root.join("first.rs")).unwrap(),
        b"fn working_only() {}\n"
    );
}

#[test]
fn escape_closes_composer_then_exits_and_restores_terminal() {
    let fixture = Fixture::new();
    fixture.commit("file", b"original\n");
    fixture.write("file", b"staged\n");
    fixture.git(&["add", "file"]);
    let mut terminal = fixture.open(&[]);
    terminal.wait_for("main", 0);
    let start = terminal.send(b"C");
    terminal.wait_for("Create commit", start);
    terminal.send(b"\x1b");
    terminal.settle();
    assert!(terminal.child.try_wait().unwrap().is_none());
    terminal.exit_with(b"\x1b");
    assert_eq!(fixture.git(&["show", ":file"]), b"staged\n");
}

#[test]
fn shift_number_shortcuts_switch_workspaces_with_explicit_key_reporting() {
    let fixture = Fixture::new();
    fixture.commit("file", b"original\n");
    fixture.write("file", b"working preview\n");
    let mut terminal = fixture.open(&[]);
    terminal.wait_for("working preview", 0);
    let start = terminal.send(b"\x1b[50;2u"); // Shift+2
    terminal.wait_for("initial fixture", start);
    let start = terminal.send(b"\x1b[49;2u"); // Shift+1
    terminal.wait_for("working preview", start);
    for number in '2'..='7' {
        terminal.send(format!("\x1b[{};2u", number as u32).as_bytes());
        terminal.settle();
    }
    let start = terminal.send(b"\x1b[49;2u");
    terminal.wait_for("working preview", start);
    let start = terminal.send(b"/");
    terminal.wait_for("Search", start);
    terminal.send(b"\x1b");
    terminal.settle();
    terminal.exit_with(b"\x1b");
    let saved = fixture.saved_state();
    for workspace in [
        "Changes",
        "History",
        "Branches",
        "Stashes",
        "Worktrees",
        "Compare",
        "Operations",
    ] {
        assert!(
            saved["widths"][workspace].is_number(),
            "{workspace} was not visited"
        );
    }
}
