
use std::fs;
use std::process::{Command, Output};

fn count(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_count"))
        .args(args)
        .output()
        .expect("count runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn tree() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a"), "").unwrap();
    fs::write(root.path().join(".b"), "").unwrap();
    fs::create_dir(root.path().join("sub")).unwrap();
    fs::write(root.path().join("sub/c"), "").unwrap();
    root
}

#[test]
fn prints_the_number_of_entries() {
    let root = tree();
    let output = count(&[root.path().to_str().unwrap()]);
    assert!(output.status.success());
    assert_eq!(stdout(&output), "3\n");
}

#[test]
fn bundled_short_flags_combine() {
    let root = tree();
    let output = count(&["-rd", root.path().to_str().unwrap()]);
    assert_eq!(stdout(&output), "3\n");
}

#[test]
fn long_flags_match_the_short_ones() {
    let root = tree();
    let bundled = count(&["-rd", root.path().to_str().unwrap()]);
    let spelled = count(&["--recursive", "--no-hidden", root.path().to_str().unwrap()]);
    assert_eq!(stdout(&bundled), stdout(&spelled));
}

#[test]
fn a_file_target_fails() {
    let root = tree();
    let output = count(&[root.path().join("a").to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("not a directory"));
}

#[test]
fn a_missing_directory_is_told_apart_from_a_file() {
    let root = tree();
    let output = count(&[root.path().join("nope").to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("no such file or directory"));
}

#[test]
fn a_missing_argument_is_a_usage_error() {
    assert_eq!(count(&[]).status.code(), Some(2));
}

#[test]
fn completions_need_no_directory() {
    let output = count(&["--completions", "zsh"]);
    assert!(output.status.success());
    assert!(stdout(&output).contains("#compdef count"));
}

#[test]
fn the_command_dump_describes_the_parser() {
    let output = count(&["--command-dump"]);
    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.starts_with("C\tcount\t0\tCount items inside a directory"));
    assert!(text.contains("\toption\trecursive\t-r,--recursive\t"));
    assert!(text.contains("\targument\tdirectory\t\tDIRECTORY\t"));
}

#[test]
fn help_describes_this_tool() {
    assert!(stdout(&count(&["--help"])).starts_with("Count items inside a directory"));
}
