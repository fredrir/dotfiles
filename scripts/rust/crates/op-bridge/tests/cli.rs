#![forbid(unsafe_code)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::Output;
use std::thread;

use testkit::{Bin, stderr, stdout};

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("op");
        std::fs::write(&fake, "#!/bin/sh\nprintf 'real op:'\nprintf ' %s' \"$@\"\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        Fixture { dir }
    }

    fn socket(&self) -> PathBuf {
        self.dir.path().join("op-bridge.sock")
    }

    // Answers one request with `reply` and hands back the request line
    fn serve_once(&self, reply: &'static str) -> thread::JoinHandle<String> {
        let listener = UnixListener::bind(self.socket()).unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).unwrap();
            stream.write_all(reply.as_bytes()).unwrap();
            line
        })
    }

    fn op(&self, args: &[&str]) -> Output {
        self.run(&[&["op"], args].concat())
    }

    fn run(&self, args: &[&str]) -> Output {
        Bin::new(env!("CARGO_BIN_EXE_op-bridge"))
            .args(args)
            .env("OP_BRIDGE_SOCKET", self.socket())
            .env("OP_BRIDGE_OP", self.dir.path().join("op"))
            .output()
    }
}

#[test]
fn a_read_is_answered_by_the_bridge_with_a_trailing_newline() {
    let fixture = Fixture::new();
    let server = fixture.serve_once("{\"value\":\"s3cret\"}\n");
    let output = fixture.op(&["read", "op://Dev/pi/credential"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "s3cret\n");
    assert_eq!(
        server.join().unwrap(),
        "{\"read\":\"op://Dev/pi/credential\"}\n"
    );
}

#[test]
fn no_newline_prints_the_bare_value() {
    let fixture = Fixture::new();
    let _server = fixture.serve_once("{\"value\":\"s3cret\"}\n");
    let output = fixture.op(&["read", "-n", "op://Dev/pi/credential"]);
    assert_eq!(stdout(&output), "s3cret");
}

#[test]
fn a_denied_read_fails_without_falling_back() {
    let fixture = Fixture::new();
    let _server = fixture.serve_once("{\"denied\":\"Touch ID: UserCanceled\"}\n");
    let output = fixture.op(&["read", "op://Dev/pi/credential"]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(stderr(&output).contains("UserCanceled"), "{output:?}");
    assert!(!stdout(&output).contains("real op"), "{output:?}");
}

#[test]
fn a_refused_read_falls_back_to_the_real_op() {
    let fixture = Fixture::new();
    let _server = fixture.serve_once("{\"refused\":\"vault Personal is not shared\"}\n");
    let output = fixture.op(&["read", "op://Personal/bank/password"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "real op: read op://Personal/bank/password");
    assert!(stderr(&output).contains("using local op"), "{output:?}");
}

#[test]
fn without_a_bridge_every_command_goes_to_the_real_op() {
    let fixture = Fixture::new();
    assert!(!fixture.socket().exists());
    let output = fixture.op(&["read", "op://Dev/pi/credential"]);
    assert_eq!(stdout(&output), "real op: read op://Dev/pi/credential");
}

#[test]
fn other_commands_and_their_flags_pass_through_untouched() {
    let fixture = Fixture::new();
    let output = fixture.op(&["item", "get", "pi", "--help"]);
    assert_eq!(stdout(&output), "real op: item get pi --help");
}

#[test]
fn reload_asks_the_daemon_and_reports_the_refetch() {
    let fixture = Fixture::new();
    let server = fixture.serve_once("{\"reloaded\":{\"refilled\":2,\"known\":2}}\n");
    let output = fixture.run(&["reload"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "reloaded 2/2\n");
    assert_eq!(server.join().unwrap(), "\"reload\"\n");
}

#[test]
fn a_partial_reload_fails_and_points_at_the_log() {
    let fixture = Fixture::new();
    let _server = fixture.serve_once("{\"reloaded\":{\"refilled\":1,\"known\":2}}\n");
    let output = fixture.run(&["reload"]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(stderr(&output).contains("1/2"), "{output:?}");
}

#[test]
fn reload_without_a_daemon_fails_instead_of_falling_back() {
    let fixture = Fixture::new();
    let output = fixture.run(&["reload"]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(stderr(&output).contains("daemon unreachable"), "{output:?}");
    assert!(!stdout(&output).contains("real op"), "{output:?}");
}
