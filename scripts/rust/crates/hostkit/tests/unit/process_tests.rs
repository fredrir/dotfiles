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
