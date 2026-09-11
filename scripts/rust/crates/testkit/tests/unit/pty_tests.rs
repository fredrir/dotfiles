use std::time::{Duration, Instant};

use super::*;

#[test]
fn a_new_pty_carries_the_window_size_it_was_asked_for() {
    let (_master, slave, _) = open_pty(30, 110);
    let size = rustix::termios::tcgetwinsize(&slave).unwrap();
    assert_eq!((size.ws_row, size.ws_col), (30, 110));
}

#[test]
fn the_opening_state_is_the_state_of_the_slave() {
    let (_master, slave, before) = open_pty(24, 80);
    let now = terminal_state(&slave);
    assert_ne!(before.c_lflag & (libc::ECHO | libc::ICANON), 0);
    assert_eq!(
        before.c_lflag & (libc::ECHO | libc::ICANON | libc::ISIG),
        now.c_lflag & (libc::ECHO | libc::ICANON | libc::ISIG)
    );
    assert_eq!(before.c_cc[libc::VMIN], now.c_cc[libc::VMIN]);
}

#[test]
fn reading_drains_what_the_slave_wrote_and_then_stops() {
    let (master, slave, _) = open_pty(24, 80);
    (&slave).write_all(b"hello").unwrap();
    let mut output = Vec::new();
    read_available(&master, &mut output, 500);
    assert_eq!(output, b"hello".to_vec());
    read_available(&master, &mut output, 0);
    assert_eq!(output, b"hello".to_vec());
}

#[test]
fn a_cursor_query_is_answered_once_for_each_request() {
    let (master, slave, _) = open_pty(24, 80);
    let flags = OFlag::from_bits_retain(fcntl(&slave, FcntlArg::F_GETFL).unwrap());
    fcntl(&slave, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).unwrap();
    let mut replied = 0;
    reply_to_cursor_queries(&master, b"\x1b[6n\x1b[6n", &mut replied);
    reply_to_cursor_queries(&master, b"\x1b[6n\x1b[6n", &mut replied);
    assert_eq!(replied, 2);
    (&master).write_all(b"\n").unwrap();
    let mut replies = Vec::new();
    read_available(&slave, &mut replies, 500);
    assert_eq!(replies, b"\x1b[24;1R\x1b[24;1R\n".to_vec());
}

#[test]
fn the_last_cursor_position_report_wins() {
    assert_eq!(last_cursor_column(b"\x1b[24;1H\x1b[9;7H"), Some(7));
    assert_eq!(last_cursor_column(b"\x1b[5H"), Some(5));
    assert_eq!(last_cursor_column(b"nothing to see"), None);
}

#[test]
fn a_child_wired_to_the_pty_owns_it_as_a_terminal() {
    let (master, slave, _) = open_pty(24, 80);
    let (input, output, errors) = stdio(&slave);
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "tty > /dev/null && printf attached"])
        .stdin(input)
        .stdout(output)
        .stderr(errors);
    take_controlling_terminal(&mut command);
    let mut child = command.spawn().unwrap();
    drop(slave);
    let mut captured = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        read_available(&master, &mut captured, 100);
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "the child never finished");
    };
    read_available(&master, &mut captured, 0);
    assert!(status.success(), "{:?}", String::from_utf8_lossy(&captured));
    assert_eq!(String::from_utf8_lossy(&captured), "attached");
}
