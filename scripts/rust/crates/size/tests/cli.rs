//! Black-box checks on what the shell sees; the measuring itself is covered
//! by the unit tests next to it.

use std::process::{Command, Output};

fn size(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_size"))
        .args(args)
        .output()
        .expect("size runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn completions_are_available() {
    let output = size(&["--completions", "zsh"]);
    assert!(output.status.success());
    assert!(stdout(&output).contains("#compdef size"));
}

/// The shared `--completions` flag is flattened in, and clap will hand a
/// flattened struct's documentation to the command it lands in.
#[test]
fn help_describes_this_tool() {
    assert!(
        stdout(&size(&["--help"])).starts_with("Sizes and line counts for files and directories")
    );
}

#[test]
fn a_missing_target_fails() {
    let output = size(&["definitely-not-here"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("no such file or directory"));
}
