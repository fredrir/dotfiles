#![cfg(unix)]

use std::fs::{self, File};
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

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
    let mut cursor_replied = false;
    while !output.windows(6).any(|window| window == b"\x1b[?25l") && Instant::now() < deadline {
        read_available(&master, &mut output, 100);
        if !cursor_replied && output.windows(4).any(|window| window == b"\x1b[6n") {
            let mut terminal = &master;
            terminal.write_all(b"\x1b[24;1R").unwrap();
            cursor_replied = true;
        }
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
