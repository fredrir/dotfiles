#![cfg(unix)]

use std::fs::File;
use std::io::{ErrorKind, Read};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn termination_restores_raw_mode_cursor_and_alternate_screen() {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let (master, slave, before) = open_pty();
    let input = slave.try_clone().unwrap();
    let output = slave.try_clone().unwrap();
    let errors = slave.try_clone().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-hop"));
    command
        .env("HOME", &home)
        .env("TERM", "xterm-256color")
        .stdin(Stdio::from(input))
        .stdout(Stdio::from(output))
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

    let mut captured = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        read_available(&master, &mut captured, 100);
        if captured
            .windows(b"\x1b[?1049h".len())
            .any(|window| window == b"\x1b[?1049h")
        {
            break;
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!("picker exited before drawing: {status:?}");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("picker did not enter its alternate screen");
        }
    }

    assert_eq!(
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) },
        0
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        read_available(&master, &mut captured, 100);
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("picker did not stop after SIGTERM");
        }
    };
    read_available(&master, &mut captured, 0);

    let after = terminal_state(master.as_raw_fd());
    assert_eq!(status.signal(), Some(libc::SIGTERM));
    assert_eq!(
        before.c_lflag & (libc::ECHO | libc::ICANON | libc::ISIG | libc::IEXTEN),
        after.c_lflag & (libc::ECHO | libc::ICANON | libc::ISIG | libc::IEXTEN)
    );
    assert_eq!(
        before.c_iflag & (libc::IXON | libc::ICRNL),
        after.c_iflag & (libc::IXON | libc::ICRNL)
    );
    assert_eq!(before.c_cc[libc::VMIN], after.c_cc[libc::VMIN]);
    assert_eq!(before.c_cc[libc::VTIME], after.c_cc[libc::VTIME]);
    assert!(
        captured
            .windows(b"\x1b[?1049l".len())
            .any(|window| window == b"\x1b[?1049l"),
        "alternate screen was not left"
    );
    assert!(
        captured
            .windows(b"\x1b[?25h".len())
            .any(|window| window == b"\x1b[?25h"),
        "cursor was not restored"
    );
    assert!(
        captured
            .windows(b"\x1b[?1000l".len())
            .any(|window| window == b"\x1b[?1000l"),
        "mouse capture was not released"
    );
}

fn open_pty() -> (File, File, libc::termios) {
    let mut master: RawFd = -1;
    let mut slave: RawFd = -1;
    let mut state = std::mem::MaybeUninit::<libc::termios>::uninit();
    let mut size = libc::winsize {
        ws_row: 30,
        ws_col: 110,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    assert_eq!(
        unsafe {
            libc::openpty(
                &raw mut master,
                &raw mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &raw mut size,
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
    let _ = unsafe { libc::poll(&raw mut descriptor, 1, timeout) };
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
