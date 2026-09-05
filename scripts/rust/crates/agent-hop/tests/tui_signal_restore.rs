#![cfg(unix)]

use std::os::fd::AsRawFd;
use std::os::unix::process::ExitStatusExt;
use std::process::Command;
use std::time::{Duration, Instant};

use testkit::pty::{open_pty, read_available, stdio, take_controlling_terminal, terminal_state};
use testkit::{at, tree};

#[allow(unsafe_code)]
#[test]
fn termination_restores_raw_mode_cursor_and_alternate_screen() {
    let temporary = tree(&["home/"]);
    let home = at(&temporary, "home");
    let (master, slave, before) = open_pty(30, 110);
    let (input, activity, errors) = stdio(&slave);
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-hop"));
    command
        .env("HOME", &home)
        .env("TERM", "xterm-256color")
        .stdin(input)
        .stdout(activity)
        .stderr(errors);
    take_controlling_terminal(&mut command);
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
