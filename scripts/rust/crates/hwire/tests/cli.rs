use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Output, Stdio};

use hostkit::{Host, Route};

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

fn hwire_env(args: &[&str], variables: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hwire"));
    command
        .args(args)
        .env_remove("SSH_CONNECTION")
        .env_remove("HWIRE_SESSION");
    for (name, value) in variables {
        command.env(name, value);
    }
    command.output().expect("hwire runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn fixed_ssh_connection(route: Route) -> String {
    let this = Host::this().expect("tests run on a supported workstation OS");
    format!(
        "{} 54321 {} 22",
        this.peer().address(route).unwrap(),
        this.address(route).unwrap()
    )
}

fn tls_stamp(route: Route) -> String {
    let this = Host::this().expect("tests run on a supported workstation OS");
    format!(
        "v1:{}:{}:{}:tls",
        this.peer().name(),
        this.name(),
        route.name()
    )
}

fn expected_remote(transport: &str) -> String {
    let this = Host::this().expect("tests run on a supported workstation OS");
    format!("{transport} {} --> {}\n", this.peer().name(), this.name())
}

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
    assert_eq!(hwire(&["--all", "--route", "cable"]).status.code(), Some(2));
    assert_eq!(hwire(&["--all", "--both"]).status.code(), Some(2));
    assert_eq!(hwire(&["--up", "--down"]).status.code(), Some(2));
    assert_eq!(hwire(&["--token", &"a".repeat(32)]).status.code(), Some(2));
    assert_eq!(
        hwire(&["--info", "--route", "cable"]).status.code(),
        Some(2)
    );
    assert_eq!(hwire(&["--verbose"]).status.code(), Some(2));
    assert_eq!(hwire(&["--watch"]).status.code(), Some(2));
    assert_eq!(hwire(&["archie"]).status.code(), Some(2));
    assert_eq!(
        hwire(&["--info", "--watch", "--interval", "0"])
            .status
            .code(),
        Some(1)
    );
}

#[test]
fn established_ssh_information_uses_the_actual_server_address() {
    let connection = fixed_ssh_connection(Route::Cable);
    let output = hwire_env(
        &["--info", "--color", "never"],
        &[("SSH_CONNECTION", &connection)],
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), expected_remote("CABLE"));
}

#[test]
fn stamped_tls_information_names_the_route_and_overlay() {
    let stamp = tls_stamp(Route::Cable);
    let output = hwire_env(&["-i", "--color", "never"], &[("HWIRE_SESSION", &stamp)]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), expected_remote("CABLE - TLS"));
}

#[test]
fn ssh_evidence_wins_over_an_outer_tls_stamp() {
    let connection = fixed_ssh_connection(Route::Wifi);
    let stamp = tls_stamp(Route::Cable);
    let output = hwire_env(
        &["-i", "--json"],
        &[("SSH_CONNECTION", &connection), ("HWIRE_SESSION", &stamp)],
    );
    assert!(output.status.success(), "{output:?}");
    let document: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(document["mode"], "ssh");
    assert_eq!(document["session"]["route"], "wifi");
    assert_eq!(document["session"]["tls"], false);
}

#[test]
fn forced_color_survives_redirection() {
    let connection = fixed_ssh_connection(Route::Cable);
    let output = hwire_env(
        &["-i", "--color", "always"],
        &[("SSH_CONNECTION", &connection)],
    );
    assert!(output.status.success(), "{output:?}");
    assert!(stdout(&output).contains("\x1b["));
}

#[test]
fn forced_measurement_color_overrides_no_color() {
    let server = Server::start(None);
    let output = Command::new(env!("CARGO_BIN_EXE_hwire"))
        .args([
            "--at",
            &server.address,
            "--latency",
            "--samples",
            "2",
            "--color",
            "always",
        ])
        .env("NO_COLOR", "1")
        .output()
        .expect("hwire measurement runs");
    assert!(output.status.success(), "{output:?}");
    assert!(stdout(&output).contains("\x1b["));
}

#[test]
fn watch_prints_only_meaningful_state_changes() {
    let connection = fixed_ssh_connection(Route::Cable);
    let output = Command::new(env!("CARGO_BIN_EXE_hwire"))
        .args(["-i", "--watch", "--interval", "0.001", "--color", "never"])
        .env("SSH_CONNECTION", connection)
        .env_remove("HWIRE_SESSION")
        .env("HWIRE_WATCH_ITERATIONS", "2")
        .output()
        .expect("hwire watch runs");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output).lines().count(), 1);
}

#[test]
fn an_explicit_target_reports_a_missing_ssh_client_as_failure() {
    let output = Command::new(env!("CARGO_BIN_EXE_hwire"))
        .args(["-i", "--color", "never", "archie"])
        .env("PATH", "")
        .env_remove("SSH_CONNECTION")
        .env_remove("HWIRE_SESSION")
        .output()
        .expect("hwire runs without ssh in PATH");
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(stdout(&output).contains("UNKNOWN"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("ssh"));
}

#[test]
fn the_completions_flag_answers_for_this_tool() {
    let output = hwire(&["--completions", "zsh"]);
    assert!(output.status.success());
    assert!(stdout(&output).contains("#compdef hwire"));
    assert!(stdout(&output).contains("--info"));
    assert!(stdout(&output).contains("--watch"));
    assert!(stdout(&output).contains("--color"));
    assert!(stdout(&output).contains("functions[_hwire_clap]"));
    assert!(stdout(&output).contains("*:SSH host:_hosts"));
}

#[test]
fn zsh_completion_understands_grouped_info_flags_and_requirements() {
    let generated = stdout(&hwire(&["--completions", "zsh"]));
    let harness = r#"
_arguments() { print -rl -- "$@" }
typeset -a words
words=(hwire -iv '')
CURRENT=3
_hwire
print -- __ROOT__
words=(hwire '')
CURRENT=2
_hwire
print -- __WATCH__
words=(hwire -i --watch '')
CURRENT=4
_hwire
"#;
    let mut child = Command::new("zsh")
        .args(["-f"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("zsh is installed for zsh completion tests");
    let script = format!("autoload -Uz compinit\ncompinit -D\n{generated}\n{harness}");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let output = stdout(&output);
    let (info, rest) = output.split_once("__ROOT__\n").unwrap();
    let (root, watch) = rest.split_once("__WATCH__\n").unwrap();
    assert!(info.contains("*:SSH host:_hosts"), "{info}");
    assert!(!root.contains("--verbose"), "{root}");
    assert!(!root.contains("--watch"), "{root}");
    assert!(!root.contains("--token=["), "{root}");
    assert!(watch.contains("--interval"), "{watch}");
    assert!(watch.contains("--notify"), "{watch}");
}
