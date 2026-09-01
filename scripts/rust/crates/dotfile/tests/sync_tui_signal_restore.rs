#![cfg(unix)]

use std::fs::{self, File};
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn sync_tui_teardown_reuses_the_viewport_origin_for_completion() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("repo");
    let home = temporary.path().join("home");
    let config = home.join(".config");
    fs::create_dir_all(root.join("config")).unwrap();
    fs::create_dir_all(root.join("environment/test")).unwrap();
    fs::create_dir_all(root.join("shared")).unwrap();
    fs::create_dir_all(&config).unwrap();
    fs::write(
        root.join("config/targets.dotfile"),
        "shared/gitconfig = ~/.gitconfig\n",
    )
    .unwrap();
    fs::write(root.join("environment/test/manifest"), "shared\n").unwrap();
    fs::write(root.join("shared/gitconfig"), "[user]\nname = Test\n").unwrap();
    let backend = temporary.path().join("dotfile-py");
    fs::write(&backend, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&backend, fs::Permissions::from_mode(0o755)).unwrap();
    let binary = env!("CARGO_BIN_EXE_dotfile");

    let (master, slave, _) = open_pty();
    let input = slave.try_clone().unwrap();
    let activity = slave.try_clone().unwrap();
    let errors = slave.try_clone().unwrap();
    let mut command = Command::new(binary);
    command
        .args(["sync", "test"])
        .env("DOTFILE_ROOT", &root)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &config)
        .env("DOTFILE_PYTHON", &backend)
        .env("TERM", "xterm-256color")
        .env_remove("CI")
        .env("NO_COLOR", "1")
        .stdin(Stdio::from(input))
        .stdout(Stdio::from(activity))
        .stderr(Stdio::from(errors));
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
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
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("repo");
    let home = temporary.path().join("home");
    let config = home.join(".config");
    fs::create_dir_all(root.join("config")).unwrap();
    fs::create_dir_all(root.join("environment/test")).unwrap();
    fs::create_dir_all(root.join("shared")).unwrap();
    fs::create_dir_all(&config).unwrap();
    fs::write(
        root.join("config/targets.dotfile"),
        "shared/gitconfig = ~/.gitconfig\n",
    )
    .unwrap();
    fs::write(root.join("environment/test/manifest"), "shared\n").unwrap();
    fs::write(root.join("shared/gitconfig"), "[user]\nname = Test\n").unwrap();
    let backend = temporary.path().join("dotfile-py");
    fs::write(&backend, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&backend, fs::Permissions::from_mode(0o755)).unwrap();
    let binary = env!("CARGO_BIN_EXE_dotfile");
    let initial = Command::new(binary)
        .args(["sync", "test"])
        .env("DOTFILE_ROOT", &root)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &config)
        .env("DOTFILE_PYTHON", &backend)
        .env("CI", "1")
        .output()
        .unwrap();
    assert!(
        initial.status.success(),
        "{}",
        String::from_utf8_lossy(&initial.stderr)
    );

    let (master, slave, before) = open_pty();
    let input = slave.try_clone().unwrap();
    let activity = slave.try_clone().unwrap();
    let errors = slave.try_clone().unwrap();
    let mut command = Command::new(binary);
    command
        .args(["sync", "test"])
        .env("DOTFILE_ROOT", &root)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &config)
        .env("DOTFILE_PYTHON", &backend)
        .env("TERM", "xterm-256color")
        .env_remove("CI")
        .env("NO_COLOR", "1")
        .stdin(Stdio::from(input))
        .stdout(Stdio::from(activity))
        .stderr(Stdio::from(errors));
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
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
fn sync_tui_signal_restores_terminal_and_cursor() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("repo");
    let home = temporary.path().join("home");
    let config = home.join(".config");
    fs::create_dir_all(root.join("config")).unwrap();
    fs::create_dir_all(root.join("environment/test")).unwrap();
    fs::create_dir_all(root.join("shared/vscode")).unwrap();
    fs::create_dir_all(root.join("macos/vscode")).unwrap();
    fs::create_dir_all(config.join("Code/User")).unwrap();
    fs::write(
        root.join("config/targets.dotfile"),
        "shared/vscode/settings.json = ~/.config/Code/User/settings.json\nmacos/vscode = ~/.config/Code/User\n",
    )
    .unwrap();
    fs::write(root.join("environment/test/manifest"), "shared\nmacos\n").unwrap();
    fs::write(
        root.join("shared/vscode/settings.json"),
        "{\"font\": \"mono\"}\n",
    )
    .unwrap();
    fs::write(
        root.join("macos/vscode/settings.macos.json"),
        "{\"theme\": \"dark\"}\n",
    )
    .unwrap();
    let backend = temporary.path().join("dotfile-py");
    fs::write(&backend, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&backend, fs::Permissions::from_mode(0o755)).unwrap();
    let binary = env!("CARGO_BIN_EXE_dotfile");
    let initial = Command::new(binary)
        .args(["sync", "test"])
        .env("DOTFILE_ROOT", &root)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &config)
        .env("DOTFILE_PYTHON", &backend)
        .env("CI", "1")
        .output()
        .unwrap();
    assert!(
        initial.status.success(),
        "{}",
        String::from_utf8_lossy(&initial.stderr)
    );
    fs::write(
        config.join("Code/User/settings.json"),
        "{\"font\": \"sans\", \"theme\": \"dark\"}\n",
    )
    .unwrap();

    let (master, slave, before) = open_pty();
    let input = slave.try_clone().unwrap();
    let activity = slave.try_clone().unwrap();
    let errors = slave.try_clone().unwrap();
    let mut command = Command::new(binary);
    command
        .args(["sync", "test"])
        .env("DOTFILE_ROOT", &root)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &config)
        .env("DOTFILE_PYTHON", &backend)
        .env("TERM", "xterm-256color")
        .env_remove("CI")
        .env("NO_COLOR", "1")
        .stdin(Stdio::from(input))
        .stdout(Stdio::from(activity))
        .stderr(Stdio::from(errors));
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
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

use std::os::fd::AsRawFd;

fn open_pty() -> (File, File, libc::termios) {
    let mut master: RawFd = -1;
    let mut slave: RawFd = -1;
    let mut state = std::mem::MaybeUninit::<libc::termios>::uninit();
    let mut size = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        },
        0
    );
    assert_eq!(unsafe { libc::tcgetattr(slave, state.as_mut_ptr()) }, 0);
    let flags = unsafe { libc::fcntl(master, libc::F_GETFL) };
    assert!(flags >= 0);
    assert_eq!(
        unsafe { libc::fcntl(master, libc::F_SETFL, flags | libc::O_NONBLOCK) },
        0
    );
    (
        unsafe { File::from_raw_fd(master) },
        unsafe { File::from_raw_fd(slave) },
        unsafe { state.assume_init() },
    )
}

fn terminal_state(fd: RawFd) -> libc::termios {
    let mut state = std::mem::MaybeUninit::<libc::termios>::uninit();
    assert_eq!(unsafe { libc::tcgetattr(fd, state.as_mut_ptr()) }, 0);
    unsafe { state.assume_init() }
}

fn read_available(master: &File, output: &mut Vec<u8>, timeout: libc::c_int) {
    let mut descriptor = libc::pollfd {
        fd: master.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let _ = unsafe { libc::poll(&mut descriptor, 1, timeout) };
    let mut reader = master;
    let mut buffer = [0_u8; 4096];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => output.extend_from_slice(&buffer[..read]),
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
            Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
            Err(error) => panic!("read pty: {error}"),
        }
    }
}

fn last_cursor_column(output: &[u8]) -> Option<u16> {
    output
        .windows(2)
        .enumerate()
        .filter(|(_, window)| *window == b"\x1b[")
        .filter_map(|(start, _)| {
            let tail = &output[start + 2..];
            let end = tail.iter().position(|byte| *byte == b'H')?;
            let parameters = std::str::from_utf8(&tail[..end]).ok()?;
            parameters.split(';').next_back()?.parse().ok()
        })
        .next_back()
}

fn reply_to_cursor_queries(master: &File, output: &[u8], replied: &mut usize) {
    let queries = output
        .windows(4)
        .filter(|window| *window == b"\x1b[6n")
        .count();
    while *replied < queries {
        let mut terminal = master;
        terminal.write_all(b"\x1b[24;1R").unwrap();
        *replied += 1;
    }
}
