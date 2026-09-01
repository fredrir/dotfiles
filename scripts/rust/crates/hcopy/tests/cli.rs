use std::process::{Command, Output};

fn push(args: &[&str]) -> Output {
    run(env!("CARGO_BIN_EXE_hpush"), args)
}

fn pull(args: &[&str]) -> Output {
    run(env!("CARGO_BIN_EXE_hpull"), args)
}

fn run(program: &str, args: &[&str]) -> Output {
    Command::new(program).args(args).output().expect("it runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn under_home(name: &str) -> String {
    format!("{}/{name}", std::env::var("HOME").expect("a home"))
}

#[test]
fn completions_are_available_for_both_directions() {
    let output = push(&["--completions", "zsh"]);
    assert!(output.status.success());
    assert!(stdout(&output).contains("#compdef hpush"));

    let output = pull(&["--completions", "zsh"]);
    assert!(output.status.success());
    assert!(stdout(&output).contains("#compdef hpull"));
}

#[test]
fn the_completions_offer_the_flags_that_exist() {
    let script = stdout(&push(&["--completions", "zsh"]));
    for flag in ["--dry-run", "--checksum", "--all", "--yes", "--to"] {
        assert!(script.contains(flag), "the script never mentions {flag}");
    }
    assert!(!script.contains("--from"));
}

#[test]
fn help_describes_each_direction() {
    assert!(stdout(&push(&["--help"])).starts_with("Copy a path from this machine"));
    assert!(stdout(&pull(&["--help"])).starts_with("Copy a path from the other machine"));
}

#[test]
fn each_direction_reports_a_version() {
    assert!(push(&["--version"]).status.success());
    assert!(pull(&["--version"]).status.success());
}

#[test]
fn a_path_outside_home_is_refused_before_anything_is_opened() {
    let output = push(&["/etc/hosts"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("inside your home directory"));
}

#[test]
fn home_itself_is_not_a_path_to_copy() {
    let output = push(&["~"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("whole home directory"));
}

#[test]
fn a_source_that_is_not_there_is_named_in_the_error() {
    let missing = under_home("hcopy-definitely-not-here-8f3a1c");
    let output = push(&[&missing]);
    assert_eq!(output.status.code(), Some(1));
    let complaint = stderr(&output);
    assert!(complaint.starts_with("hpush: local source does not exist"));
    assert!(complaint.contains(&missing));
}

#[test]
fn an_unknown_flag_is_refused_rather_than_ignored() {
    let output = push(&["--nope"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("--nope"));
}

#[test]
fn neither_direction_answers_the_other_ones_flag() {
    assert_eq!(push(&["--from", "~/x"]).status.code(), Some(2));
    assert_eq!(pull(&["--to", "~/x"]).status.code(), Some(2));
}

#[test]
fn only_one_path_is_taken() {
    let output = push(&["one", "two"]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn the_command_dump_describes_the_parser() {
    let dump = stdout(&push(&["--command-dump"]));
    assert!(dump.starts_with("C\thpush\t"));
    assert!(dump.contains("\targument\tpath\t"));
    assert!(dump.contains("\toption\tto\t--to\t"));
}
