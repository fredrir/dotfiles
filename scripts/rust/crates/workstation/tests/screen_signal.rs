#![cfg(unix)]

use std::os::fd::AsRawFd;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use testkit::pty::{open_pty, read_available, stdio, take_controlling_terminal, terminal_state};
use workstation::Screen;
use workstation::screen::{SignalGuard, SignalOptions, termination_requested, termination_signal};

const CHILD: &str = "WORKSTATION_SCREEN_SIGNAL_CHILD";
const OPTIONS_CHILD: &str = "WORKSTATION_SIGNAL_OPTIONS_CHILD";

static HOOK_RAN: AtomicBool = AtomicBool::new(false);

fn mark_hook_ran() {
    HOOK_RAN.store(true, Ordering::Release);
}

#[allow(unsafe_code)]
#[test]
fn termination_signal_restores_terminal_and_status() {
    if std::env::var_os(CHILD).is_some() {
        let mut screen = Screen::open().unwrap().unwrap();
        screen.draw(&["waiting".to_string()]).unwrap();
        let _ = screen.key();
        drop(screen);
        panic!("termination signal was not propagated");
    }

    let (master, slave, before) = open_pty(24, 80);
    let (input, activity, errors) = stdio(&slave);
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "termination_signal_restores_terminal_and_status",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .stdin(input)
        .stdout(activity)
        .stderr(errors);
    take_controlling_terminal(&mut command);
    let mut child = command.spawn().unwrap();
    drop(slave);

    let mut output = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        read_available(&master, &mut output, 100);
        if output.windows(6).any(|window| window == b"\x1b[?25l") {
            break;
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!("screen exited before drawing: {status:?}");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("screen did not draw before the deadline");
        }
    }

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
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("signal did not stop the screen before the deadline");
        }
    };
    read_available(&master, &mut output, 0);

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
    assert!(output.windows(6).any(|window| window == b"\x1b[?25h"));
}

#[allow(unsafe_code)]
#[test]
fn signal_options_run_the_hook_disarm_the_handlers_and_keep_the_number() {
    if let Some(directory) = std::env::var_os(OPTIONS_CHILD) {
        let directory = std::path::PathBuf::from(directory);
        let guard = SignalGuard::with_options(SignalOptions {
            hook: Some(mark_hook_ran),
            reset_to_default: true,
            reraise_on_drop: false,
            restart_syscalls: false,
        })
        .unwrap();
        std::fs::write(directory.join("ready"), "1").unwrap();
        while !termination_requested() {
            std::thread::sleep(Duration::from_millis(5));
        }
        let report = format!(
            "{} {} {} {}",
            u8::from(HOOK_RAN.load(Ordering::Acquire)),
            u8::from(disposition(libc::SIGINT) == libc::SIG_DFL),
            u8::from(disposition(libc::SIGTERM) == libc::SIG_DFL),
            u8::from(disposition(libc::SIGHUP) == libc::SIG_DFL),
        );
        drop(guard);
        let signal = termination_signal();
        std::fs::write(directory.join("report"), format!("{report} {signal}")).unwrap();
        std::process::exit(128 + signal);
    }

    let temporary = tempfile::tempdir().unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "signal_options_run_the_hook_disarm_the_handlers_and_keep_the_number",
            "--nocapture",
        ])
        .env(OPTIONS_CHILD, temporary.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let ready = temporary.path().join("ready");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("the guarded child exited before it armed: {status:?}");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("the guarded child did not arm before the deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    assert_eq!(
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) },
        0
    );
    let status = child.wait().unwrap();
    let report = std::fs::read_to_string(temporary.path().join("report")).unwrap_or_default();
    assert_eq!(report, format!("1 1 1 1 {}", libc::SIGTERM));
    assert_eq!(status.signal(), None);
    assert_eq!(status.code(), Some(128 + libc::SIGTERM));
}

#[allow(unsafe_code)]
#[test]
fn dropping_the_inner_guard_leaves_the_outer_handler_armed() {
    let original = disposition(libc::SIGINT);
    let outer = SignalGuard::new().unwrap();
    let armed = disposition(libc::SIGINT);
    assert_ne!(armed, libc::SIG_DFL);

    let inner = SignalGuard::new().unwrap();
    drop(inner);
    assert_eq!(disposition(libc::SIGINT), armed);

    drop(outer);
    assert_eq!(disposition(libc::SIGINT), original);
}

#[allow(unsafe_code)]
fn disposition(signal: libc::c_int) -> libc::sighandler_t {
    let mut current: libc::sigaction = unsafe { std::mem::zeroed() };
    assert_eq!(
        unsafe { libc::sigaction(signal, std::ptr::null(), &raw mut current) },
        0
    );
    current.sa_sigaction
}
