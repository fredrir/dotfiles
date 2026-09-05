#![cfg(unix)]

use std::io;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use testkit::Bin;
use testkit::pty::{open_pty, read_available, reply_to_cursor_queries, stdio, terminal_state};
use tui_kit::{Inline, Teardown};

const CHILD: &str = "TUI_KIT_INLINE_RAW_CHILD";

const RAW: libc::tcflag_t = libc::ECHO | libc::ICANON;

#[test]
fn dropping_an_inline_turns_raw_mode_back_off() {
    if let Some(report) = std::env::var_os(CHILD) {
        let before = terminal_state(io::stdin().as_raw_fd());
        let inline = Inline::new(io::stdout(), 3, Teardown::KeepViewport).unwrap();
        let during = terminal_state(io::stdin().as_raw_fd());
        drop(inline);
        let after = terminal_state(io::stdin().as_raw_fd());
        std::fs::write(
            PathBuf::from(report),
            format!("{} {} {}", before.c_lflag, during.c_lflag, after.c_lflag),
        )
        .unwrap();
        return;
    }

    let temporary = testkit::tree(&[]);
    let report = temporary.path().join("flags");
    let (master, slave, before) = open_pty(24, 80);
    let (input, output, errors) = stdio(&slave);
    let mut child = Bin::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "dropping_an_inline_turns_raw_mode_back_off",
            "--nocapture",
        ])
        .env(CHILD, &report)
        .env("TERM", "xterm-256color")
        .stdio(input, output, errors)
        .spawn();
    drop(slave);

    let mut captured = Vec::new();
    let mut replies = 0;
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        read_available(&master, &mut captured, 100);
        reply_to_cursor_queries(&master, &captured, &mut replies);
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("the inline child did not finish before the deadline");
        }
    };
    read_available(&master, &mut captured, 0);
    assert!(status.success(), "{:?}", String::from_utf8_lossy(&captured));

    let reported = std::fs::read_to_string(&report).unwrap();
    let flags: Vec<libc::tcflag_t> = reported
        .split_whitespace()
        .map(|flag| flag.parse().unwrap())
        .collect();
    assert_eq!(flags.len(), 3, "{reported}");
    assert_eq!(flags[0], before.c_lflag);
    assert_ne!(flags[0] & RAW, 0);
    assert_eq!(flags[1] & RAW, 0);
    assert_eq!(flags[2], flags[0]);
    assert_eq!(
        terminal_state(master.as_raw_fd()).c_lflag & RAW,
        before.c_lflag & RAW
    );
}
