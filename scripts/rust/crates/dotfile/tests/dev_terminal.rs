#![cfg(unix)]

use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

use testkit::pty::{open_pty, read_available, stdio, take_controlling_terminal, terminal_state};
use testkit::{executable, tree_pairs};

struct TerminalRun {
    child: Child,
    master: File,
    output: Vec<u8>,
}

impl TerminalRun {
    fn start(command: &mut Command) -> Self {
        let (master, slave, _) = open_pty(24, 100);
        let (input, output, errors) = stdio(&slave);
        command.stdin(input).stdout(output).stderr(errors);
        take_controlling_terminal(command);
        Self {
            child: command.spawn().unwrap(),
            master,
            output: Vec::new(),
        }
    }

    fn until(&mut self, condition: impl Fn(&[u8]) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition(&self.output) {
            read_available(&self.master, &mut self.output, 20);
            assert!(
                Instant::now() < deadline,
                "output timed out: {}",
                String::from_utf8_lossy(&self.output)
            );
        }
    }

    fn finish(&mut self) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            read_available(&self.master, &mut self.output, 20);
            if let Some(status) = self.child.try_wait().unwrap() {
                read_available(&self.master, &mut self.output, 0);
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "task timed out: {}",
                String::from_utf8_lossy(&self.output)
            );
        }
    }
}

impl Drop for TerminalRun {
    fn drop(&mut self) {
        if self.child.try_wait().unwrap().is_some() {
            return;
        }
        let _ = Command::new("kill")
            .args(["-INT", &self.child.id().to_string()])
            .status();
        let deadline = Instant::now() + Duration::from_secs(3);
        while self.child.try_wait().unwrap().is_none() && Instant::now() < deadline {
            read_available(&self.master, &mut self.output, 20);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn direct_and_nested_interactive_shells_finish_without_claiming_the_terminal() {
    let root = tree_pairs(&[
        ("config/targets.dotfile", ""),
        ("scripts/rust/Cargo.toml", "[workspace]\nmembers = []\n"),
        ("scripts/python/tests/", ""),
        (
            "shared/zsh/tests/tmux.zsh",
            "[[ -o interactive ]] || exit 3\nprint 'interactive shell passed'\n",
        ),
        ("bin/", ""),
    ]);
    executable(
        &root.path().join("bin/uv"),
        "#!/bin/sh\nexec zsh -dfi \"$DOTFILE_ROOT/shared/zsh/tests/tmux.zsh\"\n",
    );
    let mut command = Command::new(env!("CARGO_BIN_EXE_dotfile"));
    command
        .args(["dev", "test", "--lang", "python,shell", "--jobs", "2"])
        .env("DOTFILE_ROOT", root.path())
        .env(
            "PATH",
            format!(
                "{}:{}",
                root.path().join("bin").display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        );
    let mut run = TerminalRun::start(&mut command);
    let before = terminal_state(run.master.as_raw_fd());
    let status = run.finish();
    let text = String::from_utf8_lossy(&run.output);
    assert!(status.success(), "{text}");
    assert_eq!(text.matches("interactive shell passed").count(), 2);
    assert!(text.contains("2 passed, 0 failed"), "{text}");
    let after = terminal_state(run.master.as_raw_fd());
    assert_eq!(before.c_lflag, after.c_lflag);
    assert_eq!(before.c_iflag, after.c_iflag);
    assert_eq!(before.c_oflag, after.c_oflag);
}

#[test]
fn task_output_is_visible_before_the_task_can_finish_and_is_not_replayed() {
    let root = tree_pairs(&[
        ("config/targets.dotfile", ""),
        (
            "scripts/rust/Cargo.toml",
            "[workspace]\nmembers = ['crates/demo']\n",
        ),
        (
            "scripts/rust/crates/demo/Cargo.toml",
            "[package]\nname = 'demo'\n",
        ),
        ("scripts/python/tests/", ""),
        ("bin/", ""),
    ]);
    executable(
        &root.path().join("bin/cargo"),
        "#!/bin/sh\nprintf 'live stdout\\n'\nprintf 'live stderr\\n' >&2\nwhile [ ! -f \"$DOTFILE_ROOT/release\" ]; do sleep 0.01; done\nprintf 'finished task\\n'\n",
    );
    let mut command = Command::new(env!("CARGO_BIN_EXE_dotfile"));
    command
        .args(["dev", "test", "--pkg", "demo"])
        .env("DOTFILE_ROOT", root.path())
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", root.path().join("bin").display()),
        );
    let mut run = TerminalRun::start(&mut command);
    run.until(|output| {
        let text = String::from_utf8_lossy(output);
        text.contains("live stdout") && text.contains("live stderr")
    });
    assert!(run.child.try_wait().unwrap().is_none());
    fs::write(root.path().join("release"), "").unwrap();
    let status = run.finish();
    let text = String::from_utf8_lossy(&run.output);
    assert!(status.success(), "{text}");
    for expected in ["live stdout", "live stderr", "finished task"] {
        assert_eq!(text.matches(expected).count(), 1, "{text}");
    }
}

#[test]
fn terminal_ctrl_c_cancels_a_detached_task_that_ignores_interrupts() {
    use std::io::Write;

    let root = tree_pairs(&[
        ("config/targets.dotfile", ""),
        (
            "scripts/rust/Cargo.toml",
            "[workspace]\nmembers = ['crates/demo']\n",
        ),
        (
            "scripts/rust/crates/demo/Cargo.toml",
            "[package]\nname = 'demo'\n",
        ),
        ("scripts/python/tests/", ""),
        ("bin/", ""),
    ]);
    executable(
        &root.path().join("bin/cargo"),
        "#!/bin/sh\ntrap '' INT TERM\nprintf 'ready for interrupt\\n'\nsleep 30\n",
    );
    let mut command = Command::new(env!("CARGO_BIN_EXE_dotfile"));
    command
        .args(["dev", "test", "--pkg", "demo"])
        .env("DOTFILE_ROOT", root.path())
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", root.path().join("bin").display()),
        );
    let mut run = TerminalRun::start(&mut command);
    run.until(|output| String::from_utf8_lossy(output).contains("ready for interrupt"));
    run.master.write_all(b"\x03").unwrap();
    let status = run.finish();
    let text = String::from_utf8_lossy(&run.output);
    assert_eq!(status.code(), Some(130), "{text}");
    assert!(text.contains("1 cancelled"), "{text}");
}
