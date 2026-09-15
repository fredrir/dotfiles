#![forbid(unsafe_code)]
#![cfg(all(unix, feature = "crossterm"))]

use std::fs::File;
use std::io::Write;
use std::process::{Child, Command, ExitStatus};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use testkit::pty::{open_pty, read_available, stdio, take_controlling_terminal};
use ui_terminal::{Input, RawSession, Waited};

const HANGUP_CHILD: &str = "UI_INPUT_HANGUP_CHILD";
const KEY_CHILD: &str = "UI_INPUT_KEY_CHILD";
const IDLE_CHILD: &str = "UI_INPUT_IDLE_CHILD";
const RESIZE_CHILD: &str = "UI_INPUT_RESIZE_CHILD";

/// The child gives up first, so a child that never noticed anything reports
/// itself with an exit code rather than being killed by an ambiguous timeout.
const GIVE_UP: Duration = Duration::from_secs(10);
const PATIENCE: Duration = Duration::from_secs(30);

/// A child on its own pty, in the state real callers run in: raw mode, and a
/// signal guard that swallows the SIGHUP a closing terminal delivers. Noticing
/// the hangup is then the only way left for it to end.
fn attached() -> (RawSession, Input) {
    let session = RawSession::new().unwrap();
    let input = Input::new().unwrap();
    println!("ready");
    (session, input)
}

/// A fork by one test inherits every descriptor another test has open, so a
/// pty opened here would stay alive in a sibling's child and never hang up.
/// Opening and spawning under one lock keeps each pty to its own test.
static SPAWNING: Mutex<()> = Mutex::new(());

fn ready_child(test: &str, marker: &str) -> (File, Child) {
    let (master, mut child) = {
        let _serial = SPAWNING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (master, slave, _) = open_pty(24, 80);
        let (input, output, errors) = stdio(&slave);
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", test, "--nocapture"])
            .env(marker, "1")
            .stdin(input)
            .stdout(output)
            .stderr(errors);
        take_controlling_terminal(&mut command);
        (master, command.spawn().unwrap())
    };

    let deadline = Instant::now() + PATIENCE;
    let mut captured = Vec::new();
    loop {
        read_available(&master, &mut captured, 50);
        if captured.windows(5).any(|window| window == b"ready") {
            return (master, child);
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!(
                "the child ended before it was ready: {status:?} {:?}",
                String::from_utf8_lossy(&captured)
            );
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("the child never became ready");
        }
    }
}

fn ended(child: &mut Child, whine: &str) -> ExitStatus {
    let deadline = Instant::now() + PATIENCE;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("{whine}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn wait_reports_hangup_when_the_terminal_goes_away() {
    const TEST: &str = "wait_reports_hangup_when_the_terminal_goes_away";
    if std::env::var_os(HANGUP_CHILD).is_some() {
        let (_session, input) = attached();
        let deadline = Instant::now() + GIVE_UP;
        loop {
            if let Waited::HangUp = input.wait(Duration::from_millis(50)).unwrap() {
                std::process::exit(0);
            }
            if Instant::now() >= deadline {
                std::process::exit(2);
            }
        }
    }

    let (master, mut child) = ready_child(TEST, HANGUP_CHILD);
    drop(master);
    let status = ended(&mut child, "the hangup was never noticed");
    assert_eq!(status.code(), Some(0), "{status:?}");
}

#[test]
fn wait_delivers_keys_from_a_live_terminal() {
    const TEST: &str = "wait_delivers_keys_from_a_live_terminal";
    if std::env::var_os(KEY_CHILD).is_some() {
        let (_session, input) = attached();
        let deadline = Instant::now() + GIVE_UP;
        loop {
            if let Waited::Event(crossterm::event::Event::Key(key)) =
                input.wait(Duration::from_millis(50)).unwrap()
                && key.code == crossterm::event::KeyCode::Char('q')
            {
                std::process::exit(0);
            }
            if Instant::now() >= deadline {
                std::process::exit(2);
            }
        }
    }

    let (master, mut child) = ready_child(TEST, KEY_CHILD);
    (&master).write_all(b"q").unwrap();
    let status = ended(&mut child, "the keypress was never delivered");
    assert_eq!(status.code(), Some(0), "{status:?}");
}

#[test]
fn an_idle_wait_spends_its_timeout_rather_than_spinning() {
    const TEST: &str = "an_idle_wait_spends_its_timeout_rather_than_spinning";
    if std::env::var_os(IDLE_CHILD).is_some() {
        let (_session, input) = attached();
        let start = Instant::now();
        let waited = input.wait(Duration::from_millis(200)).unwrap();
        let spent = start.elapsed();
        assert!(matches!(waited, Waited::Idle), "{waited:?}");
        assert!(
            spent >= Duration::from_millis(150),
            "returned early: {spent:?}"
        );
        std::process::exit(0);
    }

    let (master, mut child) = ready_child(TEST, IDLE_CHILD);
    let status = ended(&mut child, "the idle wait never finished");
    drop(master);
    assert_eq!(status.code(), Some(0), "{status:?}");
}

#[test]
fn wait_reports_a_resize_while_the_terminal_sits_idle() {
    const TEST: &str = "wait_reports_a_resize_while_the_terminal_sits_idle";
    if std::env::var_os(RESIZE_CHILD).is_some() {
        let (_session, input) = attached();
        let deadline = Instant::now() + GIVE_UP;
        loop {
            if let Waited::Event(crossterm::event::Event::Resize(columns, rows)) =
                input.wait(Duration::from_millis(50)).unwrap()
            {
                assert_eq!((columns, rows), (41, 9));
                std::process::exit(0);
            }
            if Instant::now() >= deadline {
                std::process::exit(2);
            }
        }
    }

    let (master, mut child) = ready_child(TEST, RESIZE_CHILD);
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
    let status = ended(&mut child, "the resize was never reported");
    drop(master);
    assert_eq!(status.code(), Some(0), "{status:?}");
}
