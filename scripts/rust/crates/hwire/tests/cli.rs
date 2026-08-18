//! End to end over the loopback: a real `hwire serve`, a real measurement
//! against it, and the flags and exit codes callers depend on.
//!
//! `--at` is what makes this possible without a second machine — the same
//! client and server the cable runs, with the ssh in between left out.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Output, Stdio};

/// A server for one test, stopped when the test ends however it ends.
struct Server {
    child: Child,
    address: String,
}

impl Server {
    fn start(token: Option<&str>) -> Server {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hwire"));
        command.args([
            "serve",
            "--bind",
            "127.0.0.1",
            "--port",
            "0",
            "--idle",
            "20",
        ]);
        if let Some(token) = token {
            command.args(["--token", token]);
        }
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("hwire serve starts");
        let mut banner = String::new();
        BufReader::new(child.stdout.take().expect("piped stdout"))
            .read_line(&mut banner)
            .expect("a banner");
        let fields: Vec<&str> = banner.split_whitespace().collect();
        assert_eq!(&fields[..2], &["hwire", "serve"], "banner: {banner:?}");
        Server {
            child,
            address: format!("{}:{}", fields[2], fields[3]),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn hwire(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hwire"))
        .args(args)
        .output()
        .expect("hwire runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The value of a `"name": number` field, without a JSON parser in the way.
fn number(document: &str, name: &str) -> f64 {
    let after = document
        .split_once(&format!("\"{name}\":"))
        .unwrap_or_else(|| panic!("{name} is in the document: {document}"))
        .1;
    after
        .trim_start()
        .split([',', '}'])
        .next()
        .expect("a value")
        .trim()
        .parse()
        .expect("a number")
}

#[test]
fn measures_a_server_it_did_not_start() {
    let server = Server::start(None);
    let output = hwire(&["--at", &server.address, "-t", "0.1", "-n", "20"]);
    assert!(output.status.success(), "{output:?}");
    let printed = stdout(&output);
    assert!(printed.contains("latency"), "{printed}");
    assert!(printed.contains("up"), "{printed}");
    assert!(printed.contains("down"), "{printed}");
}

#[test]
fn the_json_carries_every_measurement() {
    let server = Server::start(None);
    let output = hwire(&["--at", &server.address, "-t", "0.1", "-n", "20", "--json"]);
    assert!(output.status.success(), "{output:?}");
    let document = stdout(&output);
    assert!(document.contains("\"route\":\"direct\""), "{document}");
    assert!(number(&document, "samples") >= 20.0, "{document}");
    assert!(number(&document, "min") > 0.0, "{document}");
    // Loopback is memory, so the only claim worth making is that bytes moved
    // and that they took some time to do it.
    assert!(number(&document, "bits_per_second") > 0.0, "{document}");
    assert!(number(&document, "bytes") > 0.0, "{document}");
    assert!(number(&document, "seconds") > 0.0, "{document}");
}

#[test]
fn one_direction_measures_only_that_direction() {
    let server = Server::start(None);
    let output = hwire(&["--at", &server.address, "-t", "0.1", "-n", "20", "--up"]);
    let printed = stdout(&output);
    assert!(output.status.success(), "{output:?}");
    assert!(printed.contains("\n  up "), "{printed}");
    assert!(!printed.contains("\n  down "), "{printed}");
}

#[test]
fn latency_alone_transfers_nothing() {
    let server = Server::start(None);
    let output = hwire(&["--at", &server.address, "-n", "20", "--latency"]);
    let printed = stdout(&output);
    assert!(output.status.success(), "{output:?}");
    assert!(printed.contains("latency"), "{printed}");
    assert!(!printed.contains("Gbit/s"), "{printed}");
    assert!(!printed.contains("Mbit/s"), "{printed}");
}

#[test]
fn parallel_streams_are_measured_as_one_transfer() {
    let server = Server::start(None);
    let output = hwire(&[
        "--at",
        &server.address,
        "-t",
        "0.1",
        "-n",
        "20",
        "-P",
        "4",
        "--json",
    ]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(number(&stdout(&output), "streams"), 4.0);
}

#[test]
fn a_server_with_a_token_answers_nobody_else() {
    let server = Server::start(Some(&"a".repeat(32)));
    let refused = hwire(&[
        "--at",
        &server.address,
        "--token",
        &"b".repeat(32),
        "-n",
        "5",
        "--latency",
    ]);
    assert_eq!(refused.status.code(), Some(1));

    let allowed = hwire(&[
        "--at",
        &server.address,
        "--token",
        &"a".repeat(32),
        "-n",
        "5",
        "--latency",
    ]);
    assert!(allowed.status.success(), "{allowed:?}");
}

#[test]
fn a_token_that_is_not_one_is_refused_before_anything_is_dialled() {
    let output = hwire(&["--at", "127.0.0.1:9", "--token", "nonsense"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("32 hex digits"),
        "{output:?}"
    );
}

#[test]
fn nothing_listening_is_reported_rather_than_waited_on() {
    let server = Server::start(None);
    let address = server.address.clone();
    drop(server);
    let output = hwire(&["--at", &address, "-n", "5", "--latency"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&output.stderr).is_empty());
}

#[test]
fn the_settings_have_to_make_sense() {
    for arguments in [
        ["--at", "127.0.0.1:9", "-t", "0"].as_slice(),
        ["--at", "127.0.0.1:9", "-P", "0"].as_slice(),
    ] {
        let output = hwire(arguments);
        assert_eq!(output.status.code(), Some(1), "{output:?}");
    }
    // A route cannot be asked for and left to the tool at the same time.
    assert_eq!(
        hwire(&["--both", "--route", "cable"]).status.code(),
        Some(2)
    );
    assert_eq!(hwire(&["--up", "--down"]).status.code(), Some(2));
    assert_eq!(hwire(&["--token", &"a".repeat(32)]).status.code(), Some(2));
}

#[test]
fn the_completions_flag_answers_for_this_tool() {
    let output = hwire(&["--completions", "zsh"]);
    assert!(output.status.success());
    assert!(stdout(&output).contains("#compdef hwire"));
}
