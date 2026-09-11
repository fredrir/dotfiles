#![forbid(unsafe_code)]
#![cfg(unix)]

use nix::sys::signal::{Signal, kill, raise};
use nix::unistd::Pid;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use testkit::pty::{open_pty, read_available, stdio, take_controlling_terminal, terminal_state};
use ui_terminal::Screen;
use ui_terminal::screen::{SignalGuard, SignalOptions, termination_requested, termination_signal};

const CHILD: &str = "WORKSTATION_SCREEN_SIGNAL_CHILD";
const OPTIONS_CHILD: &str = "WORKSTATION_SIGNAL_OPTIONS_CHILD";
const RESIZE_CHILD: &str = "UI_SCREEN_RESIZE_CHILD";

static CANCELLED: AtomicBool = AtomicBool::new(false);

#[test]
fn idle_screen_reports_terminal_resize_without_a_keypress() {
    if std::env::var_os(RESIZE_CHILD).is_some() {
        let mut screen = Screen::open().unwrap().unwrap();
        assert_eq!(screen.poll_event(Duration::ZERO).unwrap(), None);
        screen.draw(&["resize-ready".into()]).unwrap();
        assert_eq!(
            screen.poll_event(Duration::from_secs(2)).unwrap(),
            Some(ui_terminal::Event::Resize {
                width: 41,
                height: 9
            })
        );
        return;
    }
    let (master, slave, before) = open_pty(24, 80);
    let (input, output, errors) = stdio(&slave);
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "idle_screen_reports_terminal_resize_without_a_keypress",
            "--nocapture",
        ])
        .env(RESIZE_CHILD, "1")
        .stdin(input)
        .stdout(output)
        .stderr(errors);
    take_controlling_terminal(&mut command);
    let mut child = command.spawn().unwrap();
    drop(slave);
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut captured = Vec::new();
    loop {
        read_available(&master, &mut captured, 50);
        if captured
            .windows("resize-ready".len())
            .any(|window| window == b"resize-ready")
        {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("screen did not become ready");
        }
        assert!(
            child.try_wait().unwrap().is_none(),
            "{:?}",
            String::from_utf8_lossy(&captured)
        );
    }
    rustix::termios::tcsetwinsize(
        &master,
        rustix::termios::Winsize {
            ws_row: 9,
            ws_col: 41,
            ws_xpixel: 0,
            ws_ypixel: 0,
        },
    )
    .unwrap();
    loop {
        read_available(&master, &mut captured, 50);
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "{:?}", String::from_utf8_lossy(&captured));
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("screen did not report resize");
        }
    }
    assert_eq!(terminal_state(&master).c_lflag, before.c_lflag);
}

#[test]
fn termination_signal_restores_terminal_and_status() {
    if std::env::var_os(CHILD).is_some() {
        let before = terminal_state(std::io::stdin());
        let mut screen = Screen::open().unwrap().unwrap();
        let raw = terminal_state(std::io::stdin());
        assert_eq!(raw.c_oflag, before.c_oflag);
        assert_eq!(
            raw.c_lflag & (libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN),
            0
        );
        assert_eq!(raw.c_iflag & (libc::IXON | libc::ICRNL), 0);
        assert_eq!((raw.c_cc[libc::VMIN], raw.c_cc[libc::VTIME]), (0, 1));
        assert_eq!(screen.size(), Some((80, 24)));
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

    kill(Pid::from_raw(child.id() as i32), Signal::SIGTERM).unwrap();
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

    let after = terminal_state(&master);
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

#[test]
fn signal_options_set_cancellation_and_keep_the_number() {
    if let Some(directory) = std::env::var_os(OPTIONS_CHILD) {
        let directory = std::path::PathBuf::from(directory);
        let guard = SignalGuard::with_options(SignalOptions {
            cancellation: Some(&CANCELLED),
            reset_to_default: true,
            reraise_on_drop: false,
            restart_syscalls: false,
        })
        .unwrap();
        std::fs::write(directory.join("ready"), "1").unwrap();
        while !termination_requested() {
            std::thread::sleep(Duration::from_millis(5));
        }
        let report = u8::from(CANCELLED.load(Ordering::Acquire));
        drop(guard);
        let signal = termination_signal();
        std::fs::write(directory.join("report"), format!("{report} {signal}")).unwrap();
        std::process::exit(128 + signal);
    }

    let temporary = tempfile::tempdir().unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "signal_options_set_cancellation_and_keep_the_number",
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

    kill(Pid::from_raw(child.id() as i32), Signal::SIGTERM).unwrap();
    let status = child.wait().unwrap();
    let report = std::fs::read_to_string(temporary.path().join("report")).unwrap_or_default();
    assert_eq!(report, format!("1 {}", libc::SIGTERM));
    assert_eq!(status.signal(), None);
    assert_eq!(status.code(), Some(128 + libc::SIGTERM));
}

#[test]
fn dropping_the_inner_guard_restores_outer_cancellation_and_pending_signal() {
    const TEST: &str = "dropping_the_inner_guard_restores_outer_cancellation_and_pending_signal";
    if std::env::var("SIGNAL_GUARD_TEST").as_deref() != Ok(TEST) {
        assert_eq!(guard_child(TEST).signal(), Some(libc::SIGINT));
        return;
    }
    let outer = SignalGuard::with_options(SignalOptions {
        cancellation: Some(&CANCELLED),
        reraise_on_drop: false,
        ..Default::default()
    })
    .unwrap();
    raise(Signal::SIGTERM).unwrap();
    let inner = SignalGuard::with_options(SignalOptions {
        reraise_on_drop: false,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(termination_signal(), libc::SIGTERM);
    CANCELLED.store(false, Ordering::Release);
    drop(inner);
    raise(Signal::SIGHUP).unwrap();
    assert!(CANCELLED.load(Ordering::Acquire));
    assert_eq!(termination_signal(), libc::SIGHUP);
    drop(outer);
    raise(Signal::SIGINT).unwrap();
    panic!("original disposition was not restored");
}

#[test]
fn dropping_guards_out_of_order_keeps_the_active_cancellation() {
    const TEST: &str = "dropping_guards_out_of_order_keeps_the_active_cancellation";
    if std::env::var("SIGNAL_GUARD_TEST").as_deref() != Ok(TEST) {
        assert_eq!(guard_child(TEST).signal(), Some(libc::SIGTERM));
        return;
    }
    let outer = SignalGuard::with_options(SignalOptions {
        reraise_on_drop: false,
        ..Default::default()
    })
    .unwrap();
    let inner = SignalGuard::with_options(SignalOptions {
        cancellation: Some(&CANCELLED),
        reraise_on_drop: false,
        ..Default::default()
    })
    .unwrap();
    drop(outer);
    raise(Signal::SIGINT).unwrap();
    assert!(CANCELLED.load(Ordering::Acquire));
    drop(inner);
    raise(Signal::SIGTERM).unwrap();
    panic!("original disposition was not restored");
}

#[test]
fn dropping_an_inactive_guard_does_not_reraise_before_active_cleanup() {
    const TEST: &str = "dropping_an_inactive_guard_does_not_reraise_before_active_cleanup";
    if std::env::var("SIGNAL_GUARD_TEST").as_deref() != Ok(TEST) {
        assert!(guard_child(TEST).success());
        return;
    }
    let outer = SignalGuard::new().unwrap();
    let inner = SignalGuard::with_options(SignalOptions {
        reset_to_default: true,
        reraise_on_drop: false,
        ..Default::default()
    })
    .unwrap();
    raise(Signal::SIGTERM).unwrap();
    drop(outer);
    assert_eq!(termination_signal(), libc::SIGTERM);
    drop(inner);
}

#[test]
fn reset_to_default_makes_each_second_termination_signal_fatal() {
    const TEST: &str = "reset_to_default_makes_each_second_termination_signal_fatal";
    if let Ok(signal) = std::env::var("SECOND_TERMINATION_SIGNAL") {
        let _guard = SignalGuard::with_options(SignalOptions {
            cancellation: Some(&CANCELLED),
            reset_to_default: true,
            reraise_on_drop: false,
            ..Default::default()
        })
        .unwrap();
        raise(Signal::SIGTERM).unwrap();
        assert!(CANCELLED.load(Ordering::Acquire));
        raise(Signal::try_from(signal.parse::<i32>().unwrap()).unwrap()).unwrap();
        panic!("second signal was not fatal");
    }
    for signal in [Signal::SIGINT, Signal::SIGTERM, Signal::SIGHUP] {
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .env("SECOND_TERMINATION_SIGNAL", (signal as i32).to_string())
            .status()
            .unwrap();
        assert_eq!(status.signal(), Some(signal as i32));
    }
}

fn guard_child(test: &str) -> std::process::ExitStatus {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env("SIGNAL_GUARD_TEST", test)
        .status()
        .unwrap()
}
