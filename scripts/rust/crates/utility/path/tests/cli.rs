#![forbid(unsafe_code)]

use std::fs;
use std::process::Output;

use testkit::{Bin, stdout, tree};

fn line(output: &Output) -> String {
    assert!(output.status.success());
    stdout(output).trim_end().to_string()
}

#[test]
fn git_at_the_repository_root_prints_an_empty_line() {
    let root = tree(&[".git/", "sub/file.txt"]);
    let output = Bin::new(env!("CARGO_BIN_EXE_path"))
        .arg("-g")
        .current_dir(root.path())
        .env("HOME", "/nowhere")
        .output();
    assert_eq!(line(&output), "");
}

#[test]
fn git_paths_are_relative_to_the_repository_root() {
    let root = tree(&[".git/", "sub/file.txt"]);
    let output = Bin::new(env!("CARGO_BIN_EXE_path"))
        .args(["--git", "file.txt"])
        .current_dir(root.path().join("sub"))
        .env("HOME", "/nowhere")
        .output();
    assert_eq!(line(&output), "sub/file.txt");
}

#[test]
fn a_target_that_does_not_exist_still_describes_itself() {
    let root = tree(&[".git/", "sub/file.txt"]);
    let output = Bin::new(env!("CARGO_BIN_EXE_path"))
        .args(["-g", "missing/deep.txt"])
        .current_dir(root.path())
        .env("HOME", "/nowhere")
        .output();
    assert_eq!(line(&output), "missing/deep.txt");
}

#[test]
fn git_outside_a_repository_falls_back_to_the_home_directory() {
    let home = tree(&["docs/"]);
    let output = Bin::new(env!("CARGO_BIN_EXE_path"))
        .args(["-g", "docs"])
        .current_dir(home.path())
        .env("HOME", home.path())
        .output();
    assert_eq!(line(&output), "~/docs");
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
fn the_home_directory_is_used_even_inside_a_repository() {
    let home = tree(&[".git/", "dotfiles/sub/file.txt"]);
    let repo = home.path().join("dotfiles");
    for flags in [&[][..], &["-r"], &["--relative"]] {
        let output = Bin::new(env!("CARGO_BIN_EXE_path"))
            .args(flags)
            .arg("sub/file.txt")
            .current_dir(&repo)
            .env("HOME", home.path())
            .output();
        assert_eq!(line(&output), "~/dotfiles/sub/file.txt");
    }
}

#[test]
fn relative_outside_home_prints_the_full_path() {
    let home = tree(&[]);
    let output = Bin::new(env!("CARGO_BIN_EXE_path"))
        .args(["-r", "/usr/share"])
        .current_dir(home.path())
        .env("HOME", home.path())
        .output();
    assert_eq!(line(&output), "/usr/share");
}

#[test]
fn full_relative_and_git_conflict() {
    let home = tree(&[]);
    for flags in [["-f", "-r"], ["-f", "-g"], ["-r", "-g"]] {
        let output = Bin::new(env!("CARGO_BIN_EXE_path"))
            .args(flags)
            .current_dir(home.path())
            .env("HOME", home.path())
            .output();
        assert_eq!(output.status.code(), Some(2));
    }
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
            .starts_with("Print the home-relative or repository-relative path of a target")
    );
}
