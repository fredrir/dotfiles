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
const TREE_KEYBOARD_PUSH: &[u8] = b"\x1b[>15u";
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
        let root = fs::canonicalize(root).unwrap();
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
        #[allow(clippy::unnecessary_cast)]
        let mut terminal = Terminal {
            child,
            master,
            output: Vec::new(),
            parser: vt100::Parser::new(34, 124, 0),
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
    parser: vt100::Parser,
    initial_flags: [u64; 3],
}

impl Terminal {
    fn read(&mut self, timeout: i32) {
        let start = self.output.len();
        read_available(&self.master, &mut self.output, timeout);
        self.parser.process(&self.output[start..]);
    }

    fn send(&mut self, keys: &[u8]) -> usize {
        self.read(0);
        let start = self.output.len();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut remaining = keys;
        while !remaining.is_empty() {
            match self.master.write(remaining) {
                Ok(0) => panic!("terminal closed while sending input"),
                Ok(written) => remaining = &remaining[written..],
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "terminal stopped accepting input"
                    );
                    self.read(1);
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => panic!("sending terminal input: {error}"),
            }
        }
        start
    }

    fn wait_for(&mut self, text: &str, from: usize) {
        self.wait_until(
            |terminal| {
                String::from_utf8_lossy(&terminal.output[from..]).contains(text)
                    || (terminal.output.len() > from
                        && !text.contains('\x1b')
                        && terminal.parser.screen().contents().contains(text))
            },
            text,
        );
    }

    fn wait_until(&mut self, mut condition: impl FnMut(&Self) -> bool, description: &str) {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            self.read(20);
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
            self.read(20);
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
            self.read(20);
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
        self.assert_restored();
    }

    fn assert_restored(&self) {
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
        assert_eq!(
            occurrences(KEYBOARD_PUSH) + occurrences(TREE_KEYBOARD_PUSH),
            occurrences(KEYBOARD_POP)
        );
        let on = b"\x1b]1337;SetUserVar=APP_SELECTION=MQ==\x07";
        let off = b"\x1b]1337;SetUserVar=APP_SELECTION=MA==\x07";
        if let Some(selected) = self.output.windows(on.len()).rposition(|bytes| bytes == on) {
            assert!(
                self.output[selected..]
                    .windows(off.len())
                    .any(|bytes| bytes == off),
                "selection must not stay active after dogit exits"
            );
        }
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
    terminal.wait_for("Main", 0);
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
fn ordinary_text_drag_does_not_invoke_clipboard_or_mutate_git() {
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
    let mut terminal = fixture.open_with(&["--no-mouse", "compare", "HEAD", "HEAD"], |command| {
        command
            .env("PATH", path)
            .env("DOGIT_TEST_CLIPBOARD", &clipboard);
    });
    let message = "History comparisons are read-only";
    terminal.wait_for("read-only", 0);
    terminal.settle();
    for mode in [1000, 1002, 1003, 1006, 1015] {
        let enable = format!("\x1b[?{mode}h");
        assert!(
            !terminal
                .output
                .windows(enable.len())
                .any(|bytes| bytes == enable.as_bytes()),
            "--no-mouse must leave mouse events with the terminal"
        );
    }
    let last_column = 39 + message.len() - 1;
    terminal.send(format!("\x1b[<0;39;9M\x1b[<32;{last_column};9M").as_bytes());
    terminal.settle();
    assert!(
        !clipboard.exists(),
        "dragging must not invoke the clipboard"
    );
    terminal.send(format!("\x1b[<0;{last_column};9m").as_bytes());
    terminal.settle();
    assert!(!clipboard.exists(), "releasing a drag must not copy text");
    terminal.quit();
    assert!(!clipboard.exists(), "text copying belongs to the terminal");
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(fixture.git(&["write-tree"]), index);
    assert_eq!(fs::read(fixture.root.join("file")).unwrap(), worktree);
    assert!(fixture.saved_state().get("operations").is_none());
}

#[test]
fn mouse_capture_defaults_on_and_the_choice_survives_editor_handoff() {
    for args in [&[][..], &["--no-mouse"][..]] {
        let fixture = Fixture::new();
        fixture.commit("file", b"original\n");
        fixture.write("file", b"working preview\n");
        let mut terminal = fixture.open(args);
        terminal.wait_for("working preview", 0);
        let controls = args.is_empty();
        let start = terminal.send(b"e");
        terminal.wait_for("DOGIT_EDITOR_FINISHED", start);
        terminal.handoff_finished(start);
        terminal.wait_for("1 Unstaged", start);
        let handoff = &terminal.output[start..];
        let editor = handoff
            .windows(b"DOGIT_EDITOR_FINISHED".len())
            .position(|bytes| bytes == b"DOGIT_EDITOR_FINISHED")
            .unwrap();
        if controls {
            assert!(
                handoff[..editor]
                    .windows(8)
                    .any(|bytes| bytes == b"\x1b[?1000l")
            );
        }
        terminal.quit();
        for mode in [1000, 1002, 1003, 1006, 1015] {
            let enable = format!("\x1b[?{mode}h");
            let captures = terminal
                .output
                .windows(enable.len())
                .filter(|bytes| *bytes == enable.as_bytes())
                .count();
            assert_eq!(captures, 2 * usize::from(controls), "mouse mode {mode}");
            let disable = format!("\x1b[?{mode}l");
            assert!(
                terminal
                    .output
                    .windows(disable.len())
                    .any(|bytes| bytes == disable.as_bytes())
            );
        }
    }
}

#[test]
fn mouse_clicks_work_by_default_and_no_mouse_keeps_keyboard_navigation() {
    for args in [&[][..], &["--no-mouse"][..]] {
        let fixture = Fixture::new();
        fixture.commit("file", b"original\n");
        fixture.write("file", b"working preview\n");
        let mut terminal = fixture.open(args);
        terminal.wait_for("working preview", 0);
        let controls = args.is_empty();
        terminal.wait_for("1 Unstaged", 0);
        let start = terminal.send(b"\x1b[<0;17;2M\x1b[<0;17;2m");
        if controls {
            terminal.wait_for("initial fixture", start);
            let start = terminal.send(b"\x1b[<0;4;2M\x1b[<0;4;2m");
            terminal.wait_for("working preview", start);
        } else {
            terminal.settle();
            assert!(
                !terminal
                    .parser
                    .screen()
                    .contents()
                    .contains("initial fixture")
            );
            let start = terminal.send(b"2");
            terminal.wait_for("initial fixture", start);
            let start = terminal.send(b"1");
            terminal.wait_for("working preview", start);
        }
        let quitting = terminal.output.len();
        terminal.exit_with(b"\x03");
        for mode in [1000, 1002, 1003, 1006, 1015] {
            let enable = format!("\x1b[?{mode}h");
            assert_eq!(
                terminal
                    .output
                    .windows(enable.len())
                    .any(|bytes| bytes == enable.as_bytes()),
                controls,
                "mouse mode {mode}",
            );
            if controls {
                let disable = format!("\x1b[?{mode}l");
                assert!(
                    terminal.output[quitting..]
                        .windows(disable.len())
                        .any(|bytes| bytes == disable.as_bytes())
                );
            }
        }
    }
}

#[test]
fn termination_with_mouse_controls_restores_the_terminal() {
    use std::os::unix::process::ExitStatusExt;

    let fixture = Fixture::new();
    fixture.commit("file", b"original\n");
    let mut terminal = fixture.open(&[]);
    terminal.wait_for("Main", 0);
    let start = terminal.send(b"C");
    terminal.wait_for("Create commit", start);
    terminal.send(b"Draft\x1b[1;2H");
    terminal.wait_for("\x1b]1337;SetUserVar=APP_SELECTION=MQ==\x07", start);
    let stopping = terminal.output.len();
    assert!(
        Command::new("kill")
            .args(["-TERM", &terminal.child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        read_available(&terminal.master, &mut terminal.output, 20);
        if let Some(status) = terminal.child.try_wait().unwrap() {
            assert!(status.signal().is_some());
            break;
        }
        assert!(Instant::now() < deadline, "dogit did not handle SIGTERM");
    }
    read_available(&terminal.master, &mut terminal.output, 0);
    terminal.assert_restored();
    assert!(
        terminal.output[stopping..]
            .windows(8)
            .any(|bytes| bytes == b"\x1b[?1000l")
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
    terminal.send(b"j");
    terminal.settle();
    terminal.send(b"\tjj :Stage selected file, hunk, or lines\r");
    terminal.wait_until(
        |_| fixture.git(&["show", ":file"]) == b"first\nchosen\nlast\n",
        "only chosen line staged",
    );
    terminal.settle();
    terminal.send(b"b");
    terminal.wait_until(
        |terminal| {
            let screen = terminal.parser.screen().contents();
            screen.contains("chosen") && !screen.contains("remaining")
        },
        "the staged diff without the remaining unstaged line",
    );
    terminal.settle();
    let start = terminal.send(b"jj :Unstage selected file, hunk, or lines\r");
    terminal.wait_until(
        |_| fixture.git(&["show", ":file"]) == b"first\nlast\n",
        "selected line unstaged",
    );
    terminal.wait_for("remaining", start);
    terminal.settle();
    terminal.send(b":Stage selected file, hunk, or lines\r");
    terminal.wait_until(
        |_| fixture.git(&["show", ":file"]) == worktree,
        "whole hunk staged",
    );
    terminal.settle();
    assert_eq!(fs::read(fixture.root.join("file")).unwrap(), worktree);
    terminal.quit();
}

#[test]
fn split_wrapping_shows_long_line_tails_and_stages_from_a_continuation() {
    let fixture = Fixture::new();
    fixture.commit("file", b"first\nlast\n");
    let chosen = format!("{}{}", "long addition ".repeat(10), "WRAPTAIL ".repeat(4));
    let worktree = format!("first\n{chosen}\nremaining\nlast\n");
    fixture.write("file", worktree.as_bytes());
    let mut terminal = fixture.open(&[]);
    terminal.wait_for("remaining", 0);
    terminal.send(b"j");
    terminal.settle();
    let start = terminal.send(b"\tv");
    terminal.wait_for("WRAPTAIL", start);
    terminal.settle();
    terminal.send(b"jjj :Stage selected file, hunk, or lines\r");
    let staged = format!("first\n{chosen}\nlast\n");
    terminal.wait_until(
        |_| fixture.git(&["show", ":file"]) == staged.as_bytes(),
        "the original long line staged from its wrapped continuation",
    );
    assert_eq!(
        fs::read(fixture.root.join("file")).unwrap(),
        worktree.as_bytes()
    );
    terminal.settle();
    terminal.quit();
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
    assert!(fixture.saved_state().get("operations").is_none());
}

#[test]
fn empty_editor_commit_cancellation_preserves_index() {
    let fixture = Fixture::new();
    fixture.commit("file", b"old\n");
    fixture.write("file", b"staged\n");
    fixture.git(&["add", "file"]);
    let head = fixture.git(&["rev-parse", "HEAD"]);
    let mut terminal = fixture.open(&[]);
    terminal.wait_for("Main", 0);
    let start = terminal.send(b"c");
    terminal.wait_for("DOGIT_EDITOR_FINISHED", start);
    terminal.handoff_finished(start);
    terminal.quit();
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(fixture.git(&["show", ":file"]), b"staged\n");
    assert!(fixture.saved_state().get("operations").is_none());
}

#[test]
fn configured_commit_editor_runs_in_a_native_terminal_with_hooks_and_push() {
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
    let editor = fixture.directory.path().join("commit-editor");
    executable(
        &editor,
        "#!/bin/sh\nset -eu\n[ -t 0 ] && [ -t 1 ] && [ -t 2 ]\nstty -a | grep -q -- 'icanon'\nprintf 'NATIVE_COMMIT_EDITOR_READY\\n'\nIFS= read -r subject\nIFS= read -r body\nprintf '%s\\n\\n%s\\n' \"$subject\" \"$body\" > \"$1\"\n",
    );
    let mut terminal = fixture.open_with(&[], |command| {
        command.env("GIT_EDITOR", &editor);
    });
    terminal.wait_for("Main", 0);
    let start = terminal.send(b"c");
    terminal.wait_for("NATIVE_COMMIT_EDITOR_READY", start);
    terminal.send(b"Commit 'from' $(touch sentinel) dogiX\x7ft\rA useful body.\r");
    terminal.handoff_finished(start);
    terminal.wait_for("Committed!", start);
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
    terminal.wait_for("Pushed!", start);
    terminal.quit();
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(&remote)
        .args(["rev-parse", "main"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, fixture.git(&["rev-parse", "HEAD"]));
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
    terminal.wait_for("Main", 0);
    let start = terminal.send(b"C");
    terminal.wait_for("Commit Message", start);
    let start = terminal.send(b"Blocked commit\r");
    terminal.wait_for("fix this before committing", start);
    terminal.handoff_finished(start);
    terminal.wait_for("Commit failed", start);
    terminal.quit();
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(fixture.git(&["show", ":file"]), b"staged\n");
}

#[test]
fn result_toast_appears_and_expires_without_keyboard_input() {
    let fixture = Fixture::new();
    fixture.commit("file", b"original\n");
    let mut terminal = fixture.open_with(&[], |command| {
        for flag in [
            "DOGIT_REDUCED_MOTION",
            "PREFERS_REDUCED_MOTION",
            "REDUCE_MOTION",
            "REDUCED_MOTION",
        ] {
            command.env_remove(flag);
        }
    });
    terminal.wait_for("Main", 0);
    terminal.send(b"f");
    terminal.wait_until(
        |terminal| terminal.parser.screen().contents().contains("Fetched!"),
        "animated success toast",
    );
    let visible = Instant::now();
    terminal.wait_until(
        |terminal| !terminal.parser.screen().contents().contains("Fetched!"),
        "toast expiry without input",
    );
    assert!(visible.elapsed() >= Duration::from_secs(4));
    terminal.quit();
}

#[test]
#[cfg(not(target_os = "macos"))]
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
    terminal.send(b"\x1b[<0;36;12M\x1b[<32;48;12M\x1b[<0;48;12m");
    terminal.settle();
    let start = terminal.send(b"2");
    terminal.wait_for("initial fixture", start);
    let start = terminal.send(b"1");
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
    let start = terminal.send(b"jd");
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
    terminal.wait_for("Enter to confirm", start);
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
fn inline_composer_reviews_all_staged_files_and_cancellation_preserves_both_sides() {
    let fixture = Fixture::new();
    fixture.commit("first.rs", b"fn original() {}\n");
    fixture.write("first.rs", b"fn staged_one() {}\n");
    fixture.write("second.rs", b"fn staged_two() {}\n");
    fixture.git(&["add", "."]);
    fixture.write("first.rs", b"fn working_only() {}\n");
    let mut terminal = fixture.open(&[]);
    terminal.wait_for("working_only", 0);
    let start = terminal.send(b"C");
    terminal.wait_for("Create commit", start);
    terminal.wait_for("staged_one", start);
    terminal.wait_for("staged_two", start);
    assert!(!fixture.directory.path().join("editor-args").exists());
    terminal.send(b"draft\x1b");
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
fn escape_closes_inline_composer_before_exiting_and_restores_terminal() {
    let fixture = Fixture::new();
    fixture.commit("file", b"original\n");
    fixture.write("file", b"staged\n");
    fixture.git(&["add", "file"]);
    let mut terminal = fixture.open(&[]);
    terminal.wait_for("Main", 0);
    let start = terminal.send(b"C");
    terminal.wait_for("Create commit", start);
    terminal.send(b"\x1b");
    terminal.settle();
    assert!(terminal.child.try_wait().unwrap().is_none());
    terminal.exit_with(b"\x1b");
    assert_eq!(fixture.git(&["show", ":file"]), b"staged\n");
}

#[test]
fn plain_number_shortcuts_switch_workspaces_without_enhanced_key_reporting() {
    let fixture = Fixture::new();
    fixture.commit("file", b"original\n");
    fixture.write("file", b"working preview\n");
    let mut terminal = fixture.open(&[]);
    terminal.wait_for("working preview", 0);
    let start = terminal.send(b"2");
    terminal.wait_for("initial fixture", start);
    let start = terminal.send(b"1");
    terminal.wait_for("working preview", start);
    for number in '2'..='6' {
        terminal.send(&[number as u8]);
        terminal.settle();
    }
    let start = terminal.send(b"1");
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
    ] {
        assert!(
            saved["widths"][workspace].is_number(),
            "{workspace} was not visited"
        );
    }
}

#[test]
fn compact_changes_tree_space_toggles_files_and_preserves_their_contents() {
    let fixture = Fixture::new();
    let cargo = "scripts/rust/crates/dclip/Cargo.toml";
    let source = "scripts/rust/crates/dclip/src/mode.rs";
    fixture.commit(cargo, b"original manifest\n");
    fixture.commit(source, b"original source\n");
    fixture.write(cargo, b"staged manifest\n");
    fixture.git(&["add", cargo]);
    fixture.write(source, b"working source\n");
    let mut terminal = fixture.open(&[]);
    terminal.wait_for("scripts/rust/crates/dclip", 0);
    terminal.wait_for("Cargo.toml", 0);
    terminal.wait_for("mode.rs", 0);
    terminal.wait_for("Staged", 0);
    terminal.send(b"j ");
    terminal.wait_until(
        |_| fixture.git(&["diff", "--cached", "--name-only"]).is_empty(),
        "Space unstages the selected manifest",
    );
    terminal.settle();
    terminal.send(b" ");
    terminal.wait_until(
        |_| fixture.git(&["show", &format!(":{cargo}")]) == b"staged manifest\n",
        "Space stages the selected manifest",
    );
    terminal.settle();
    terminal.send(b"jj ");
    terminal.wait_until(
        |_| fixture.git(&["diff", "--name-only"]).is_empty(),
        "Space stages the nested source file",
    );
    terminal.settle();
    terminal.send(b" ");
    terminal.wait_until(
        |_| fixture.git(&["show", &format!(":{source}")]) == b"original source\n",
        "Space unstages the nested source file",
    );
    terminal.settle();
    terminal.quit();
    assert_eq!(
        fs::read(fixture.root.join(cargo)).unwrap(),
        b"staged manifest\n"
    );
    assert_eq!(
        fs::read(fixture.root.join(source)).unwrap(),
        b"working source\n"
    );
}

#[test]
fn reopening_selects_root_and_modified_keys_toggle_all_changes_from_either_edge() {
    let fixture = Fixture::new();
    fixture.commit("src/a", b"original a\n");
    fixture.commit("src/z", b"original z\n");
    fixture.write("src/a", b"working a\n");
    fixture.write("src/z", b"working z\n");
    for _ in 0..2 {
        let mut terminal = fixture.open(&[]);
        terminal.wait_for("working a", 0);
        terminal.send(b" ");
        terminal.wait_until(
            |_| fixture.git(&["diff", "--cached", "--name-only"]) == b"src/a\nsrc/z\n",
            "startup root stages both files",
        );
        terminal.settle();
        terminal.send(b"\x1b[1;9B ");
        terminal.wait_until(
            |_| fixture.git(&["diff", "--cached", "--name-only"]) == b"src/a\n",
            "Cmd+Down reaches the last file",
        );
        terminal.settle();
        terminal.send(b"\x1b[32;2u");
        terminal.wait_until(
            |_| fixture.git(&["diff", "--name-only"]).is_empty(),
            "Shift+Space stages every remaining change",
        );
        terminal.settle();
        terminal.send(b"\x1b[32;2:2u\x1b[32;2:3u");
        terminal.settle();
        assert!(
            fixture.git(&["diff", "--name-only"]).is_empty(),
            "repeat/release must not toggle staging"
        );
        terminal.send(b"\x1b[32;2u");
        terminal.wait_until(
            |_| fixture.git(&["diff", "--cached", "--name-only"]).is_empty(),
            "Shift+Space unstages all files from the last row",
        );
        terminal.settle();
        terminal.send(b"\x1b[1;9A ");
        terminal.wait_until(
            |_| fixture.git(&["diff", "--cached", "--name-only"]) == b"src/a\nsrc/z\n",
            "Cmd+Up reaches the root",
        );
        terminal.settle();
        terminal.send(b"\x1b[32;2u");
        terminal.wait_until(
            |_| fixture.git(&["diff", "--cached", "--name-only"]).is_empty(),
            "leave both files unstaged for restart",
        );
        terminal.settle();
        let start = terminal.send(b"/");
        terminal.wait_for("Search", start);
        assert!(
            terminal.output[start..]
                .windows(KEYBOARD_PUSH.len())
                .any(|bytes| bytes == KEYBOARD_PUSH),
            "dialogs restore text-oriented keyboard reporting"
        );
        terminal.send(b"Text input: \x1b[200~\xc3\xa5 !\x1b[201~");
        terminal.settle();
        let start = terminal.send(b"\x1b");
        terminal.settle();
        assert!(
            terminal.output[start..]
                .windows(TREE_KEYBOARD_PUSH.len())
                .any(|bytes| bytes == TREE_KEYBOARD_PUSH)
        );
        terminal.send(b"\x1b[1;9B");
        terminal.settle();
        terminal.quit();
    }
}

#[test]
fn wheel_bursts_preserve_every_step_and_click_the_newly_visible_row() {
    let fixture = Fixture::new();
    for index in 0..400 {
        fixture.write(&format!("files/f{index:03}"), b"before\n");
    }
    fixture.git(&["add", "."]);
    fixture.git(&["commit", "-qm", "scroll fixture"]);
    for index in 0..400 {
        fixture.write(&format!("files/f{index:03}"), b"after\n");
    }
    let mut terminal = fixture.open(&[]);
    terminal.wait_for("f000", 0);
    terminal.settle();
    let mut events = b"\x1b[<65;8;4M".repeat(256);
    events.extend_from_slice(b"\x1b[<0;8;3M\x1b[<0;8;3m ");
    terminal.send(&events);
    terminal.wait_until(
        |_| fixture.git(&["diff", "--cached", "--name-only"]) == b"files/f255\n",
        "wheel burst and click target",
    );
    terminal.settle();
    terminal.quit();
}

#[test]
fn inline_editor_commits_identical_text_with_legacy_ss3_and_enhanced_terminal_keys() {
    struct Keys {
        term: &'static str,
        open: &'static [u8],
        left: &'static [u8],
        home: &'static [u8],
        right: &'static [u8],
        end: &'static [u8],
        delete: &'static [u8],
        backspace: &'static [u8],
        undo: &'static [u8],
        shift_left: &'static [u8],
        save: &'static [u8],
    }
    for keys in [
        Keys {
            term: "xterm-256color",
            open: b"C",
            left: b"\x1b[D",
            home: b"\x1b[H",
            right: b"\x1b[C",
            end: b"\x1b[F",
            delete: b"\x1b[3~",
            backspace: b"\x7f",
            undo: b"\x1a",
            shift_left: b"\x1b[1;2D",
            save: b"\r",
        },
        Keys {
            term: "screen-256color",
            open: b"C",
            left: b"\x1bOD",
            home: b"\x1bOH",
            right: b"\x1bOC",
            end: b"\x1bOF",
            delete: b"\x04",
            backspace: b"\x08",
            undo: b"\x1f",
            shift_left: b"\x1b[1;2D",
            save: b"\r",
        },
        Keys {
            term: "xterm-kitty",
            open: b"\x1b[99:67;2u",
            left: b"\x1b[1;1D",
            home: b"\x1b[97;5u",
            right: b"\x1b[102;5u",
            end: b"\x1b[101;5u",
            delete: b"\x1b[100;5u",
            backspace: b"\x1b[127u",
            undo: b"\x1b[122;5u",
            shift_left: b"\x1b[1;2D",
            save: b"\x1b[13u",
        },
    ] {
        let fixture = Fixture::new();
        fixture.git(&["config", "core.precomposeunicode", "false"]);
        fixture.commit("file", b"before\n");
        fixture.write("file", b"staged\n");
        fixture.git(&["add", "file"]);
        fixture.write("file", b"still unstaged\n");
        let mut terminal = fixture.open_with(&[], |command| {
            command.env("TERM", keys.term);
        });
        terminal.wait_for("Main", 0);
        let start = terminal.send(keys.open);
        terminal.wait_for("Create commit", start);
        assert!(
            terminal.output[start..]
                .windows(KEYBOARD_PUSH.len())
                .any(|bytes| bytes == KEYBOARD_PUSH)
        );
        let mut input = b"Fix wrld".to_vec();
        input.extend(keys.left.repeat(3));
        input.extend_from_slice(b"o");
        input.extend_from_slice(keys.home);
        input.extend(keys.right.repeat(3));
        input.extend_from_slice(keys.delete);
        input.extend_from_slice(b" ");
        input.extend_from_slice(keys.end);
        input.extend_from_slice(b"\x1b[13;2u\x0a\x1b[200~");
        input.extend_from_slice("Body å e\u{301} 👩💻".as_bytes());
        input.extend_from_slice(b"\x1b[201~");
        input.extend_from_slice(keys.backspace);
        input.extend_from_slice(keys.undo);
        input.extend_from_slice(keys.shift_left);
        input.extend_from_slice("\x1b[200~界\x1b[201~".as_bytes());
        input.extend_from_slice(b"\x1b[33;1:2u\x1b[33;1:3u");
        input.extend_from_slice(keys.save);
        let start = terminal.send(&input);
        terminal.handoff_finished(start);
        assert_eq!(
            fixture.git(&["log", "-1", "--format=%B"]),
            "Fix world\n\nBody å e\u{301} 界!\n\n".as_bytes(),
            "{}",
            keys.term
        );
        assert_eq!(fixture.git(&["show", "HEAD:file"]), b"staged\n");
        assert_eq!(
            fs::read(fixture.root.join("file")).unwrap(),
            b"still unstaged\n"
        );
        assert!(
            !fixture.directory.path().join("editor-args").exists(),
            "C must stay inline"
        );
        terminal.quit();
    }
}

#[test]
fn inline_selection_copies_exact_text_and_mouse_capture_returns_after_cancel() {
    for fallback in [false, true] {
        let fixture = Fixture::new();
        fixture.commit("file", b"before\n");
        fixture.write("file", b"staged\n");
        fixture.git(&["add", "file"]);
        let head = fixture.git(&["rev-parse", "HEAD"]);
        let index = fixture.git(&["write-tree"]);
        let clipboard = fixture.directory.path().join("clipboard");
        let bin = fixture.directory.path().join("bin");
        fs::create_dir(&bin).unwrap();
        executable(
            &bin.join("dclip"),
            if fallback {
                "#!/bin/sh\nexit 1\n"
            } else {
                "#!/bin/sh\ncat > \"$DOGIT_TEST_CLIPBOARD\"\n"
            },
        );
        let inherited_path = std::env::var_os("PATH").unwrap_or_default();
        let path = std::env::join_paths(
            std::iter::once(bin).chain(std::env::split_paths(&inherited_path)),
        )
        .unwrap();
        let mut terminal = fixture.open_with(&[], |command| {
            command
                .env("PATH", &path)
                .env("DOGIT_TEST_CLIPBOARD", &clipboard);
        });
        terminal.wait_for("Main", 0);
        let start = terminal.send(b"C");
        terminal.wait_for("Create commit", start);
        assert!(
            terminal.output[start..]
                .windows(8)
                .any(|bytes| bytes == b"\x1b[?1000l"),
            "composer should allow native mouse selection"
        );
        let draft = "Subject\n\nBody å e\u{301} 界";
        terminal.send(format!("\x1b[200~{draft}\x1b[201~\x1b[1;5H\x1b[1;6F").as_bytes());
        terminal.wait_for("\x1b]1337;SetUserVar=APP_SELECTION=MQ==\x07", start);
        assert!(
            !clipboard.exists(),
            "selecting alone must not change the clipboard"
        );
        for copy in [
            &b"\x03"[..],
            &b"\x1b[99;9u"[..],
            &b"\x1b[99;6u"[..],
            &b"\x1b[2;5~"[..],
        ] {
            if clipboard.exists() {
                fs::remove_file(&clipboard).unwrap();
            }
            let start = terminal.send(copy);
            if fallback {
                use crossterm::Command as _;
                let mut expected = String::new();
                crossterm::clipboard::CopyToClipboard::to_clipboard_from(draft)
                    .write_ansi(&mut expected)
                    .unwrap();
                terminal.wait_for(&expected, start);
            } else {
                terminal.wait_until(
                    |_| fs::read(&clipboard).is_ok_and(|copied| copied == draft.as_bytes()),
                    "copy exact draft selection",
                );
            }
            assert!(
                terminal.child.try_wait().unwrap().is_none(),
                "copy must not exit dogit"
            );
        }
        terminal.send(b"\x13");
        terminal.settle();
        assert_eq!(
            fixture.git(&["rev-parse", "HEAD"]),
            head,
            "Ctrl+S must not commit"
        );
        assert_eq!(fixture.git(&["write-tree"]), index);
        let start = terminal.send(b"\x1b[D");
        terminal.wait_for("\x1b]1337;SetUserVar=APP_SELECTION=MA==\x07", start);
        let start = terminal.send(b"\x1b[1;6F");
        terminal.wait_for("\x1b]1337;SetUserVar=APP_SELECTION=MQ==\x07", start);
        let start = terminal.send(b"\x1b");
        terminal.wait_for("\x1b]1337;SetUserVar=APP_SELECTION=MA==\x07", start);
        terminal.settle();
        assert!(
            terminal.output[start..]
                .windows(8)
                .any(|bytes| bytes == b"\x1b[?1000h"),
            "normal workbench mouse controls must return"
        );
        terminal.quit();
    }
}
