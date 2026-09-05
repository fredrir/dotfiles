use super::*;

const SHELL: &str = "/bin/sh";

#[test]
fn a_successful_run_reports_success_and_a_zero_code() {
    let ran = Bin::new("true").run();
    assert!(ran.success(), "{ran:?}");
    assert_eq!(ran.code(), Some(0));
}

#[test]
fn a_failing_run_keeps_its_exit_code() {
    let ran = Bin::new("false").run();
    assert!(!ran.success());
    assert_eq!(ran.code(), Some(1));
}

#[test]
fn both_streams_are_captured_as_text_and_as_bytes() {
    let ran = Bin::new(SHELL)
        .args(["-c", "printf out; printf err >&2"])
        .run();
    assert_eq!(ran.stdout, "out");
    assert_eq!(ran.stderr, "err");
    assert_eq!(ran.output.stdout, b"out".to_vec());
    assert!(ran.status.success());
}

#[test]
fn arguments_arrive_one_at_a_time_or_together() {
    let ran = Bin::new(SHELL)
        .arg("-c")
        .args(["printf '%s-%s' \"$1\" \"$2\"", "sh", "one", "two"])
        .run();
    assert_eq!(ran.stdout, "one-two");
}

#[test]
fn stdin_is_fed_from_a_string() {
    let ran = Bin::new(SHELL).args(["-c", "cat"]).stdin("hello").run();
    assert_eq!(ran.stdout, "hello");
}

#[test]
fn a_run_without_stdin_reads_nothing() {
    let ran = Bin::new(SHELL).args(["-c", "cat"]).run();
    assert_eq!(ran.stdout, "");
    assert!(ran.success(), "{ran:?}");
}

#[test]
fn the_environment_is_set_and_removed() {
    let ran = Bin::new(SHELL)
        .args(["-c", "printenv KEPT; printenv DROPPED; true"])
        .env("KEPT", "kept")
        .env("DROPPED", "dropped")
        .env_remove("DROPPED")
        .run();
    assert_eq!(ran.stdout, "kept\n");
}

#[test]
fn plain_asks_for_no_color_and_eighty_columns() {
    let ran = Bin::new(SHELL)
        .args(["-c", "printenv NO_COLOR; printenv COLUMNS"])
        .plain()
        .run();
    assert_eq!(ran.stdout, "1\n80\n");
}

#[test]
fn the_current_directory_is_where_the_binary_runs() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("marker"), "here").unwrap();
    let ran = Bin::new(SHELL)
        .args(["-c", "cat marker"])
        .current_dir(root.path())
        .run();
    assert_eq!(ran.stdout, "here");
}

#[test]
fn a_signalled_run_has_no_exit_code() {
    let ran = Bin::new(SHELL).args(["-c", "kill -TERM $$"]).run();
    assert_eq!(ran.code(), None);
    assert!(!ran.success());
}

#[test]
fn the_free_helpers_decode_a_plain_output() {
    let output = Command::new(SHELL)
        .args(["-c", "printf out; printf err >&2"])
        .output()
        .unwrap();
    assert_eq!(stdout(&output), "out");
    assert_eq!(stderr(&output), "err");
}
