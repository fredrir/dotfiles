#![forbid(unsafe_code)]

use testkit::{Bin, stdout};

fn dclip(args: &[&str]) -> Bin {
    Bin::new(env!("CARGO_BIN_EXE_dclip"))
        .args(args)
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .env_remove("HWIRE_SESSION")
        .env_remove("SSH_CONNECTION")
        .env_remove("SSH_TTY")
}

#[test]
fn the_completions_flag_answers_for_this_tool() {
    let output = dclip(&["--completions", "zsh"]).output();
    assert!(output.status.success(), "{output:?}");
    assert!(stdout(&output).contains("#compdef dclip"), "{output:?}");
}

#[test]
fn the_holder_stays_out_of_the_completions() {
    let script = stdout(&dclip(&["--completions", "zsh"]).output());
    assert!(script.contains("serve"), "{script}");
    assert!(!script.contains("hold"), "{script}");
}

#[test]
fn printing_and_serving_are_exclusive() {
    assert_eq!(dclip(&["-o", "serve"]).output().status.code(), Some(2));
}

#[cfg(target_os = "linux")]
#[test]
fn a_headless_shell_outside_the_mux_has_nothing_to_paste_from() {
    let output = dclip(&["-o"]).output();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(testkit::stderr(&output).trim(), "dclip: no clipboard");
}

#[test]
fn an_ssh_shell_pastes_nothing_even_beside_a_native_clipboard() {
    let output = dclip(&["-o"]).env("SSH_TTY", "/dev/pts/9").output();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(
        testkit::stderr(&output).trim(),
        "dclip: no clipboard over ssh"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_stamp_from_the_wrong_direction_is_ignored() {
    let output = dclip(&["-o"])
        .env("HWIRE_SESSION", "v1:archie:macie:cable:tls")
        .output();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(testkit::stderr(&output).trim(), "dclip: no clipboard");
}

#[cfg(target_os = "linux")]
#[test]
fn a_headless_copy_reaches_the_terminal_as_osc52_without_the_last_newline() {
    use std::io::Write;
    use std::process::Command;
    use std::time::{Duration, Instant};

    use testkit::pty::{open_pty, read_available, stdio, take_controlling_terminal};

    let (master, slave, _) = open_pty(24, 80);
    let (input, output, errors) = stdio(&slave);
    let mut command = Command::new(env!("CARGO_BIN_EXE_dclip"));
    command
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .env_remove("HWIRE_SESSION")
        .env_remove("SSH_CONNECTION")
        .env_remove("SSH_TTY")
        .stdin(input)
        .stdout(output)
        .stderr(errors);
    take_controlling_terminal(&mut command);
    let mut child = command.spawn().unwrap();
    (&master).write_all(b"hello\n\x04").unwrap();
    let mut output = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        read_available(&master, &mut output, 20);
        if let Some(status) = child.try_wait().unwrap() {
            read_available(&master, &mut output, 0);
            break status;
        }
        assert!(Instant::now() < deadline, "dclip timed out");
    };
    assert!(status.success(), "{}", String::from_utf8_lossy(&output));
    let written = String::from_utf8_lossy(&output);
    assert!(written.contains("\x1b]52;c;aGVsbG8=\x1b\\"), "{written:?}");
}
