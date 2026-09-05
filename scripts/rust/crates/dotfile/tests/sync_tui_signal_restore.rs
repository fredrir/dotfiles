#![cfg(unix)]

use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use testkit::pty::{
    last_cursor_column, open_pty, read_available, reply_to_cursor_queries, stdio,
    take_controlling_terminal, terminal_state,
};
use testkit::{Bin, Ran, TempDir, executable, tree_pairs};

const BACKEND: &str = "#!/bin/sh\nexit 0\n";

struct Sandbox {
    temporary: TempDir,
}

impl Sandbox {
    fn new(entries: &[(&str, &str)]) -> Sandbox {
        let mut all = entries.to_vec();
        all.push(("home/.config/", ""));
        let temporary = tree_pairs(&all);
        executable(&temporary.path().join("dotfile-py"), BACKEND);
        Sandbox { temporary }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.temporary.path().join(relative)
    }

    fn environment(&self) -> [(&'static str, PathBuf); 4] {
        [
            ("DOTFILE_ROOT", self.path("repo")),
            ("HOME", self.path("home")),
            ("XDG_CONFIG_HOME", self.path("home/.config")),
            ("DOTFILE_PYTHON", self.path("dotfile-py")),
        ]
    }

    fn batch(&self) -> Ran {
        Bin::new(env!("CARGO_BIN_EXE_dotfile"))
            .args(["sync", "test"])
            .envs(self.environment())
            .env("CI", "1")
            .run()
    }

    fn tui(&self, program: &Path, slave: &File) -> Command {
        let (input, activity, errors) = stdio(slave);
        let mut command = Command::new(program);
        command
            .args(["sync", "test"])
            .envs(self.environment())
            .env("TERM", "xterm-256color")
            .env_remove("CI")
            .env("NO_COLOR", "1")
            .stdin(input)
            .stdout(activity)
            .stderr(errors);
        take_controlling_terminal(&mut command);
        command
    }
}

fn gitconfig_sandbox() -> Sandbox {
    Sandbox::new(&[
        (
            "repo/config/targets.dotfile",
            "shared/gitconfig = ~/.gitconfig\n",
        ),
        ("repo/environment/test/manifest", "shared\n"),
        ("repo/shared/gitconfig", "[user]\nname = Test\n"),
    ])
}

#[test]
fn sync_tui_teardown_reuses_the_viewport_origin_for_completion() {
    let sandbox = gitconfig_sandbox();

    let (master, slave, _) = open_pty(24, 80);
    let mut child = sandbox
        .tui(Path::new(env!("CARGO_BIN_EXE_dotfile")), &slave)
        .spawn()
        .unwrap();
    drop(slave);
    let mut output = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut cursor_replies = 0;
    let status = loop {
        read_available(&master, &mut output, 100);
        reply_to_cursor_queries(&master, &output, &mut cursor_replies);
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "changed sync did not finish");
    };
    read_available(&master, &mut output, 0);
    assert!(
        status.success(),
        "PTY output: {:?}",
        String::from_utf8_lossy(&output)
    );
    assert!(
        output.windows(6).any(|window| window == b"\x1b[?25l"),
        "changed sync did not animate: {:?}",
        String::from_utf8_lossy(&output)
    );
    let completion = output
        .windows("✓ Synced".len())
        .rposition(|window| window == "✓ Synced".as_bytes())
        .unwrap();
    let before_completion = &output[..completion];
    let cleared = before_completion
        .iter()
        .rposition(|byte| *byte == b'J')
        .unwrap();
    let clear_escape = before_completion[..cleared]
        .windows(2)
        .rposition(|window| window == b"\x1b[")
        .unwrap();
    assert!(
        before_completion[clear_escape + 2..cleared]
            .iter()
            .all(u8::is_ascii_digit)
    );
    assert!(
        !before_completion[cleared + 1..]
            .iter()
            .any(|byte| matches!(byte, b'\r' | b'\n')),
        "completion followed reserved rows: {:?}",
        String::from_utf8_lossy(&output)
    );
    assert_eq!(last_cursor_column(before_completion), Some(1));
}

#[test]
fn sync_tui_noop_does_not_open_or_reserve_an_inline_viewport() {
    let sandbox = gitconfig_sandbox();
    let initial = sandbox.batch();
    assert!(initial.success(), "{}", initial.stderr);

    let (master, slave, before) = open_pty(24, 80);
    let mut child = sandbox
        .tui(Path::new(env!("CARGO_BIN_EXE_dotfile")), &slave)
        .spawn()
        .unwrap();
    drop(slave);
    let mut output = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        read_available(&master, &mut output, 100);
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "no-op sync did not finish");
    };
    read_available(&master, &mut output, 0);
    assert!(
        status.success(),
        "PTY output: {:?}",
        String::from_utf8_lossy(&output)
    );
    assert!(
        !output.contains(&b'\x1b'),
        "no-op sync touched the terminal: {:?}",
        String::from_utf8_lossy(&output)
    );
    let output = String::from_utf8(output).unwrap().replace('\r', "");
    assert_eq!(output.lines().collect::<Vec<_>>(), ["✓ Synced"]);
    let after = terminal_state(master.as_raw_fd());
    let terminal_flags = libc::ECHO | libc::ICANON | libc::ISIG;
    assert_eq!(
        before.c_lflag & terminal_flags,
        after.c_lflag & terminal_flags
    );
}

#[test]
fn sync_tui_animates_a_stale_tooling_refresh_before_reexec() {
    let sandbox = Sandbox::new(&[
        (
            "repo/config/targets.dotfile",
            "shared/gitconfig = ~/.gitconfig\n",
        ),
        ("repo/environment/test/manifest", "shared\n"),
        ("repo/shared/gitconfig", "[user]\nname = Test\n"),
        ("home/.local/bin/", ""),
    ]);
    let installed = sandbox.path("home/.local/bin/dotfile");
    std::os::unix::fs::symlink(
        sandbox.path("repo/shared/gitconfig"),
        sandbox.path("home/.gitconfig"),
    )
    .unwrap();
    fs::copy(env!("CARGO_BIN_EXE_dotfile"), &installed).unwrap();
    std::thread::sleep(Duration::from_millis(20));
    executable(&sandbox.path("repo/setup.sh"), "#!/bin/sh\nsleep 0.3\n");

    let (master, slave, _) = open_pty(24, 80);
    let mut child = sandbox.tui(&installed, &slave).spawn().unwrap();
    drop(slave);
    let mut output = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut cursor_replies = 0;
    let status = loop {
        read_available(&master, &mut output, 100);
        reply_to_cursor_queries(&master, &output, &mut cursor_replies);
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "tooling refresh did not finish");
    };
    read_available(&master, &mut output, 0);
    assert!(
        status.success(),
        "PTY output: {:?}",
        String::from_utf8_lossy(&output)
    );
    assert!(
        output.windows(6).any(|window| window == b"\x1b[?25l"),
        "tooling refresh did not animate: {:?}",
        String::from_utf8_lossy(&output)
    );
    let output = String::from_utf8_lossy(&output).replace('\r', "");
    assert!(output.contains("✓ Synced"));
}

#[allow(unsafe_code)]
#[test]
fn sync_tui_signal_restores_terminal_and_cursor() {
    let sandbox = Sandbox::new(&[
        (
            "repo/config/targets.dotfile",
            "shared/vscode/settings.json = ~/.config/Code/User/settings.json\nmacos/vscode = ~/.config/Code/User\n",
        ),
        ("repo/environment/test/manifest", "shared\nmacos\n"),
        ("repo/shared/vscode/settings.json", "{\"font\": \"mono\"}\n"),
        (
            "repo/macos/vscode/settings.macos.json",
            "{\"theme\": \"dark\"}\n",
        ),
        ("home/.config/Code/User/", ""),
    ]);
    let initial = sandbox.batch();
    assert!(initial.success(), "{}", initial.stderr);
    fs::write(
        sandbox.path("home/.config/Code/User/settings.json"),
        "{\"font\": \"sans\", \"theme\": \"dark\"}\n",
    )
    .unwrap();

    let (master, slave, before) = open_pty(24, 80);
    let mut child = sandbox
        .tui(Path::new(env!("CARGO_BIN_EXE_dotfile")), &slave)
        .spawn()
        .unwrap();
    drop(slave);
    let mut output = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut early_status = None;
    let mut cursor_replies = 0;
    while !output.windows(6).any(|window| window == b"\x1b[?25l") && Instant::now() < deadline {
        read_available(&master, &mut output, 100);
        reply_to_cursor_queries(&master, &output, &mut cursor_replies);
        if let Some(status) = child.try_wait().unwrap() {
            early_status = Some(status);
            break;
        }
    }
    assert!(
        output.windows(6).any(|window| window == b"\x1b[?25l"),
        "PTY output: {:?}, early status: {:?}",
        String::from_utf8_lossy(&output),
        early_status
    );
    assert_eq!(
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) },
        0
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        read_available(&master, &mut output, 100);
        reply_to_cursor_queries(&master, &output, &mut cursor_replies);
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "signal did not unwind the TUI");
    };
    read_available(&master, &mut output, 0);
    let after = terminal_state(master.as_raw_fd());
    let terminal_flags = libc::ECHO | libc::ICANON | libc::ISIG;
    assert_eq!(
        before.c_lflag & terminal_flags,
        after.c_lflag & terminal_flags
    );
    assert!(output.windows(6).any(|window| window == b"\x1b[?25h"));
    assert_eq!(status.code(), Some(143));
}
