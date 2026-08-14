//! Black-box checks on the output and exit codes callers depend on.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn run(cwd: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_path"))
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .output()
        .expect("path runs")
}

fn line(output: &Output) -> String {
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_string()
}

fn repo() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join(".git")).unwrap();
    fs::create_dir(root.path().join("sub")).unwrap();
    fs::write(root.path().join("sub/file.txt"), "").unwrap();
    root
}

#[test]
fn a_repository_root_prints_a_slash() {
    let root = repo();
    assert_eq!(line(&run(root.path(), Path::new("/nowhere"), &[])), "/");
}

#[test]
fn paths_inside_a_repository_are_relative_to_its_root() {
    let root = repo();
    let output = run(root.path(), Path::new("/nowhere"), &["sub/file.txt"]);
    assert_eq!(line(&output), "/sub/file.txt");
}

#[test]
fn a_target_that_does_not_exist_still_describes_itself() {
    let root = repo();
    let output = run(root.path(), Path::new("/nowhere"), &["missing/deep.txt"]);
    assert_eq!(line(&output), "/missing/deep.txt");
}

#[test]
fn full_prints_the_resolved_path() {
    let root = repo();
    let real = fs::canonicalize(root.path()).unwrap();
    let output = run(root.path(), Path::new("/nowhere"), &["-f", "sub/file.txt"]);
    assert_eq!(
        line(&output),
        real.join("sub/file.txt").display().to_string()
    );
}

#[test]
fn outside_a_repository_the_home_directory_is_a_tilde() {
    let home = tempfile::tempdir().unwrap();
    fs::create_dir(home.path().join("docs")).unwrap();
    assert_eq!(line(&run(home.path(), home.path(), &[])), "~");
    assert_eq!(line(&run(home.path(), home.path(), &["docs"])), "~/docs");
}

#[test]
fn outside_both_the_path_is_absolute() {
    let home = tempfile::tempdir().unwrap();
    let output = run(home.path(), Path::new("/nowhere"), &["/usr/share"]);
    assert_eq!(line(&output), "/usr/share");
}

#[test]
fn extra_arguments_are_a_usage_error() {
    let home = tempfile::tempdir().unwrap();
    let output = run(home.path(), home.path(), &["a", "b"]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn completions_are_available() {
    let home = tempfile::tempdir().unwrap();
    let output = run(home.path(), home.path(), &["--completions", "zsh"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("#compdef path"));
}

/// The shared `--completions` flag is flattened in, and clap will hand a
/// flattened struct's documentation to the command it lands in.
#[test]
fn help_describes_this_tool() {
    let home = tempfile::tempdir().unwrap();
    let output = run(home.path(), home.path(), &["--help"]);
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .starts_with("Print the repository-relative or home-relative path of a target")
    );
}
