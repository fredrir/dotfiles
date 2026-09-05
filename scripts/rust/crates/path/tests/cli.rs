use std::fs;
use std::process::Output;

use testkit::{Bin, stdout, tree};

fn line(output: &Output) -> String {
    assert!(output.status.success());
    stdout(output).trim_end().to_string()
}

#[test]
fn a_repository_root_prints_a_slash() {
    let root = tree(&[".git/", "sub/file.txt"]);
    let output = Bin::new(env!("CARGO_BIN_EXE_path"))
        .current_dir(root.path())
        .env("HOME", "/nowhere")
        .output();
    assert_eq!(line(&output), "/");
}

#[test]
fn paths_inside_a_repository_are_relative_to_its_root() {
    let root = tree(&[".git/", "sub/file.txt"]);
    let output = Bin::new(env!("CARGO_BIN_EXE_path"))
        .arg("sub/file.txt")
        .current_dir(root.path())
        .env("HOME", "/nowhere")
        .output();
    assert_eq!(line(&output), "/sub/file.txt");
}

#[test]
fn a_target_that_does_not_exist_still_describes_itself() {
    let root = tree(&[".git/", "sub/file.txt"]);
    let output = Bin::new(env!("CARGO_BIN_EXE_path"))
        .arg("missing/deep.txt")
        .current_dir(root.path())
        .env("HOME", "/nowhere")
        .output();
    assert_eq!(line(&output), "/missing/deep.txt");
}

#[test]
fn full_prints_the_resolved_path() {
    let root = tree(&[".git/", "sub/file.txt"]);
    let real = fs::canonicalize(root.path()).unwrap();
    let output = Bin::new(env!("CARGO_BIN_EXE_path"))
        .args(["-f", "sub/file.txt"])
        .current_dir(root.path())
        .env("HOME", "/nowhere")
        .output();
    assert_eq!(
        line(&output),
        real.join("sub/file.txt").display().to_string()
    );
}

#[test]
fn outside_a_repository_the_home_directory_is_a_tilde() {
    let home = tree(&["docs/"]);
    let bare = Bin::new(env!("CARGO_BIN_EXE_path"))
        .current_dir(home.path())
        .env("HOME", home.path())
        .output();
    let named = Bin::new(env!("CARGO_BIN_EXE_path"))
        .arg("docs")
        .current_dir(home.path())
        .env("HOME", home.path())
        .output();
    assert_eq!(line(&bare), "~");
    assert_eq!(line(&named), "~/docs");
}

#[test]
fn outside_both_the_path_is_absolute() {
    let home = tree(&[]);
    let output = Bin::new(env!("CARGO_BIN_EXE_path"))
        .arg("/usr/share")
        .current_dir(home.path())
        .env("HOME", "/nowhere")
        .output();
    assert_eq!(line(&output), "/usr/share");
}

#[test]
fn extra_arguments_are_a_usage_error() {
    let home = tree(&[]);
    let output = Bin::new(env!("CARGO_BIN_EXE_path"))
        .args(["a", "b"])
        .current_dir(home.path())
        .env("HOME", home.path())
        .output();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn completions_are_available() {
    let home = tree(&[]);
    let output = Bin::new(env!("CARGO_BIN_EXE_path"))
        .args(["--completions", "zsh"])
        .current_dir(home.path())
        .env("HOME", home.path())
        .output();
    assert!(output.status.success());
    assert!(stdout(&output).contains("#compdef path"));
}

#[test]
fn help_describes_this_tool() {
    let home = tree(&[]);
    let output = Bin::new(env!("CARGO_BIN_EXE_path"))
        .arg("--help")
        .current_dir(home.path())
        .env("HOME", home.path())
        .output();
    assert!(
        stdout(&output)
            .starts_with("Print the repository-relative or home-relative path of a target")
    );
}
