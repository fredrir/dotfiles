#![cfg(unix)]

use std::process::Command;
use std::time::{Duration, Instant};

use super::*;

#[test]
fn captures_both_pipes_and_preserves_exit_status() {
    let result = output(
        Command::new("sh").args(["-c", "printf 'out'; printf 'err' >&2; exit 7"]),
        CaptureLimits::default(),
        Duration::from_secs(2),
    )
    .unwrap();
    assert_eq!(result.status.code(), Some(7));
    assert_eq!(result.stdout, b"out");
    assert_eq!(result.stderr, b"err");
}

#[test]
fn bounded_pipes_are_drained_without_deadlock() {
    let result = output(
        Command::new("sh").args([
            "-c",
            "head -c 100000 /dev/zero; head -c 100000 /dev/zero >&2",
        ]),
        CaptureLimits {
            stdout: 29,
            stderr: 31,
        },
        Duration::from_secs(2),
    )
    .unwrap();
    assert!(result.status.success());
    assert_eq!(result.stdout.len(), 29);
    assert_eq!(result.stderr.len(), 31);
    assert!(result.stdout_truncated);
    assert!(result.stderr_truncated);
}

#[test]
fn timeout_includes_descendants_holding_output_pipes() {
    let started = Instant::now();
    let error = output(
        Command::new("sh").args(["-c", "sleep 30 & wait"]),
        CaptureLimits::default(),
        Duration::from_millis(50),
    )
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn successful_parent_cannot_leave_pipe_readers_waiting() {
    let started = Instant::now();
    let result = output(
        Command::new("sh").args(["-c", "sleep 30 & printf done; exit 0"]),
        CaptureLimits::default(),
        Duration::from_secs(2),
    )
    .unwrap();
    assert!(result.status.success());
    assert_eq!(result.stdout, b"done");
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn streamed_output_stops_at_the_file_size_limit() {
    let destination = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/null")
        .unwrap();
    let error = output_to_file_limited(
        Command::new("sh").args(["-c", "head -c 100000 /dev/zero"]),
        &destination,
        1024,
        500,
        Duration::from_secs(2),
    )
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("maximum file size"));
}

#[test]
fn cancellation_interrupts_a_child_without_waiting_for_the_deadline() {
    let started = Instant::now();
    let error = output_cancellable(
        Command::new("sh").args(["-c", "sleep 30 & wait"]),
        CaptureLimits::default(),
        Duration::from_secs(60),
        &|| started.elapsed() >= Duration::from_millis(30),
    )
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn terminating_capture_preserves_early_failure_and_diagnostics() {
    let result = output_terminating(
        Command::new("sh").args(["-c", "printf 'refused' >&2; exit 7"]),
        CaptureLimits::default(),
        Duration::from_secs(1),
        Duration::from_secs(1),
        &|| false,
    )
    .unwrap();
    assert!(!result.deadline_reached);
    assert_eq!(result.output.status.code(), Some(7));
    assert_eq!(result.output.stderr, b"refused");
}

#[test]
fn terminating_capture_sends_term_at_deadline_and_retains_cleanup_output() {
    let began = Instant::now();
    let result = output_terminating(
        Command::new("sh").args([
            "-c",
            "trap 'printf cleaned; exit 0' TERM; printf started; while :; do sleep 1; done",
        ]),
        CaptureLimits::default(),
        Duration::from_millis(100),
        Duration::from_secs(2),
        &|| false,
    )
    .unwrap();
    assert!(result.deadline_reached);
    assert!(result.output.status.success());
    assert_eq!(result.output.stdout, b"startedcleaned");
    assert!(began.elapsed() < Duration::from_secs(1));
}

#[test]
fn terminating_capture_preserves_descendant_cleanup_after_the_leader_exits() {
    let began = Instant::now();
    let result = output_terminating(
        Command::new("sh").args([
            "-c",
            "trap 'exit 0' TERM; sh -c 'trap \"sleep 0.15; printf cleaned; exit 0\" TERM; printf ready; while :; do sleep 1; done' & wait",
        ]),
        CaptureLimits::default(),
        Duration::from_millis(200),
        Duration::from_secs(2),
        &|| false,
    )
    .unwrap();
    assert!(result.deadline_reached);
    assert!(result.output.status.success());
    assert_eq!(result.output.stdout, b"readycleaned");
    assert!(began.elapsed() < Duration::from_secs(1));
}

#[test]
fn terminating_capture_bounds_descendant_cleanup_after_the_leader_exits() {
    let began = Instant::now();
    let result = output_terminating(
        Command::new("sh").args([
            "-c",
            "trap 'exit 0' TERM; sh -c 'trap \"\" TERM; printf ready; sleep 30 & wait' & wait",
        ]),
        CaptureLimits::default(),
        Duration::from_millis(200),
        Duration::from_millis(100),
        &|| false,
    )
    .unwrap();
    assert!(result.deadline_reached);
    assert!(result.output.status.success());
    assert_eq!(result.output.stdout, b"ready");
    assert!(began.elapsed() < Duration::from_secs(1));
}

#[test]
fn terminating_capture_cancels_descendant_cleanup_after_the_leader_exits() {
    let began = Instant::now();
    let error = output_terminating(
        Command::new("sh").args([
            "-c",
            "trap 'exit 0' TERM; sh -c 'trap \"\" TERM; printf ready; sleep 30 & wait' & wait",
        ]),
        CaptureLimits::default(),
        Duration::from_millis(200),
        Duration::from_secs(30),
        &|| began.elapsed() >= Duration::from_millis(350),
    )
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    assert!(began.elapsed() < Duration::from_secs(1));
}

#[test]
fn terminating_capture_kills_a_group_that_ignores_term_at_grace_deadline() {
    use std::os::unix::process::ExitStatusExt;
    let began = Instant::now();
    let result = output_terminating(
        Command::new("sh").args(["-c", "trap '' TERM; printf started; sleep 30 & wait"]),
        CaptureLimits::default(),
        Duration::from_millis(100),
        Duration::from_millis(100),
        &|| false,
    )
    .unwrap();
    assert!(result.deadline_reached);
    assert_eq!(result.output.status.signal(), Some(9));
    assert_eq!(result.output.stdout, b"started");
    assert!(began.elapsed() < Duration::from_secs(1));
}

#[test]
fn terminating_capture_cancellation_does_not_wait_for_cleanup_grace() {
    let began = Instant::now();
    let error = output_terminating(
        Command::new("sh").args(["-c", "trap '' TERM; sleep 30 & wait"]),
        CaptureLimits::default(),
        Duration::from_millis(30),
        Duration::from_secs(30),
        &|| began.elapsed() > Duration::from_millis(100),
    )
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    assert!(began.elapsed() < Duration::from_secs(1));
}
