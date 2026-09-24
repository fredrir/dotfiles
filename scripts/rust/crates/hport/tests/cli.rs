#![forbid(unsafe_code)]

use std::process::Output;

use testkit::{Bin, stderr, stdout};

fn hport(args: &[&str]) -> Bin {
    Bin::new(env!("CARGO_BIN_EXE_hport")).args(args)
}

fn run(args: &[&str]) -> Output {
    hport(args).output()
}

#[test]
fn status_without_a_daemon_points_at_setup() {
    let state = tempfile::tempdir().unwrap();
    let output = hport(&[]).env("XDG_STATE_HOME", state.path()).output();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(stderr(&output).contains("hport setup"), "{output:?}");
}

#[test]
fn a_stale_state_file_from_a_dead_daemon_is_not_trusted() {
    let state = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(state.path().join("hport")).unwrap();
    std::fs::write(
        state.path().join("hport/state.json"),
        r#"{"pid":999999,"peer":"archie","route":"cable","connected":true,"error":null,"services":[]}"#,
    )
    .unwrap();
    let output = hport(&[]).env("XDG_STATE_HOME", state.path()).output();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
}

#[test]
fn listeners_prints_one_json_array() {
    let output = run(&["listeners"]);
    assert!(output.status.success(), "{output:?}");
    let text = stdout(&output);
    assert_eq!(text.lines().count(), 1, "{text}");
    assert!(text.starts_with('['), "{text}");
}

#[test]
fn a_listener_started_by_this_test_is_reported() {
    let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port();
    let text = stdout(&run(&["listeners"]));
    assert!(
        text.contains(&format!(r#""port":{port},"address":"127.0.0.1""#)),
        "{text}"
    );
}

#[test]
fn the_completions_flag_answers_for_this_tool() {
    let output = run(&["--completions", "zsh"]);
    assert!(output.status.success(), "{output:?}");
    assert!(stdout(&output).contains("#compdef hport"), "{output:?}");
}

#[test]
fn an_unknown_command_is_a_usage_error() {
    assert_eq!(run(&["nowhere"]).status.code(), Some(2));
}
