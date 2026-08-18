//! Black-box checks on the flags, output, prompts and exit codes callers
//! depend on.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn flatten(args: &[&str], answers: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_flatten"))
        .args(args)
        .env("NO_COLOR", "1")
        .env("COLUMNS", "80")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("flatten runs");
    child
        .stdin
        .take()
        .expect("stdin is a pipe")
        .write_all(answers.as_bytes())
        .expect("the answers are read");
    child.wait_with_output().expect("flatten finishes")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The example: a folder holding one folder of the same name.
fn nested() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let inner = root.path().join("documents/documents");
    fs::create_dir_all(&inner).unwrap();
    for name in ["doc_1.txt", "doc_2.txt", ".env"] {
        fs::write(inner.join(name), "").unwrap();
    }
    root
}

fn names(path: &Path) -> Vec<String> {
    let mut found: Vec<String> = fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().display().to_string())
        .collect();
    found.sort();
    found
}

fn at(root: &tempfile::TempDir, path: &str) -> String {
    root.path().join(path).display().to_string()
}

#[test]
fn the_nesting_is_gone_and_nothing_was_said_about_it() {
    let root = nested();
    let output = flatten(&[&at(&root, "documents")], "");
    assert!(output.status.success());
    assert_eq!(stdout(&output), "");
    assert_eq!(stderr(&output), "");
    assert_eq!(
        names(&root.path().join("documents")),
        [".env", "doc_1.txt", "doc_2.txt"]
    );
}

#[test]
fn running_it_again_changes_nothing_and_still_succeeds() {
    let root = nested();
    flatten(&[&at(&root, "documents")], "");
    let output = flatten(&[&at(&root, "documents")], "");
    assert!(output.status.success());
    assert_eq!(stdout(&output), "");
    assert_eq!(
        names(&root.path().join("documents")),
        [".env", "doc_1.txt", "doc_2.txt"]
    );
}

#[test]
fn a_dry_run_shows_the_moves_and_moves_nothing() {
    let root = nested();
    let output = flatten(&["-n", &at(&root, "documents")], "");
    assert!(output.status.success());
    let shown = stdout(&output);
    assert!(
        shown.contains("documents/doc_1.txt -> doc_1.txt"),
        "{shown}"
    );
    assert!(
        shown.contains("3 entries moved, 1 directory removed"),
        "{shown}"
    );
    assert_eq!(names(&root.path().join("documents")), ["documents"]);
}

#[test]
fn verbose_names_each_move() {
    let root = nested();
    let output = flatten(&["-v", &at(&root, "documents")], "");
    assert!(stdout(&output).contains("documents/.env -> .env"));
}

#[test]
fn a_deep_flatten_asks_before_it_starts() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("pack/sub/x")).unwrap();
    fs::write(root.path().join("pack/sub/x/b.txt"), "").unwrap();
    let output = flatten(&["-d", &at(&root, "pack")], "n\n");
    assert!(output.status.success());
    assert!(stdout(&output).contains("Continue? [Y/n]"));
    assert!(stdout(&output).contains("flatten: cancelled"));
    assert!(root.path().join("pack/sub/x/b.txt").exists());
}

#[test]
fn a_deep_flatten_brings_everything_up() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("pack/sub/x")).unwrap();
    fs::create_dir_all(root.path().join("pack/empty")).unwrap();
    fs::write(root.path().join("pack/README"), "").unwrap();
    fs::write(root.path().join("pack/sub/a.txt"), "").unwrap();
    fs::write(root.path().join("pack/sub/x/b.txt"), "").unwrap();
    let output = flatten(&["-d", &at(&root, "pack")], "y\n");
    assert!(output.status.success());
    assert_eq!(
        names(&root.path().join("pack")),
        ["README", "a.txt", "b.txt"]
    );
}

#[test]
fn a_name_two_entries_want_is_asked_about_one_at_a_time() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("pack/a")).unwrap();
    fs::create_dir_all(root.path().join("pack/b")).unwrap();
    fs::write(root.path().join("pack/notes.txt"), "top").unwrap();
    fs::write(root.path().join("pack/a/notes.txt"), "from-a").unwrap();
    fs::write(root.path().join("pack/b/notes.txt"), "from-b").unwrap();

    // Take the first, decline the second, then start.
    let output = flatten(&["-d", &at(&root, "pack")], "y\nn\ny\n");
    assert!(output.status.success());
    let shown = stdout(&output);
    assert!(
        shown.contains("replace notes.txt with a/notes.txt? [Y/n/a]"),
        "{shown}"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("pack/notes.txt")).unwrap(),
        "from-a"
    );
    // The one declined is still where it was, and so is what held it.
    assert!(root.path().join("pack/b/notes.txt").exists());
}

#[test]
fn answering_all_takes_every_name_without_asking_again() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("pack/a")).unwrap();
    fs::create_dir_all(root.path().join("pack/b")).unwrap();
    fs::write(root.path().join("pack/notes.txt"), "top").unwrap();
    fs::write(root.path().join("pack/a/notes.txt"), "from-a").unwrap();
    fs::write(root.path().join("pack/b/notes.txt"), "from-b").unwrap();

    let output = flatten(&["-d", &at(&root, "pack")], "a\ny\n");
    assert!(output.status.success());
    assert_eq!(names(&root.path().join("pack")), ["notes.txt"]);
    // The last one settled is the one left there.
    assert_eq!(
        fs::read_to_string(root.path().join("pack/notes.txt")).unwrap(),
        "from-b"
    );
}

#[test]
fn yes_answers_the_collisions_as_well_as_the_start() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("pack/a")).unwrap();
    fs::write(root.path().join("pack/notes.txt"), "top").unwrap();
    fs::write(root.path().join("pack/a/notes.txt"), "from-a").unwrap();
    let output = flatten(&["-d", "-y", &at(&root, "pack")], "");
    assert!(output.status.success());
    assert!(!stdout(&output).contains("[Y/n"));
    assert_eq!(
        fs::read_to_string(root.path().join("pack/notes.txt")).unwrap(),
        "from-a"
    );
}

#[test]
fn every_directory_named_is_flattened() {
    let root = tempfile::tempdir().unwrap();
    for pack in ["one", "two"] {
        fs::create_dir_all(root.path().join(pack).join("inner")).unwrap();
        fs::write(root.path().join(pack).join("inner/f.txt"), "").unwrap();
    }
    let output = flatten(&[&at(&root, "one"), &at(&root, "two")], "");
    assert!(output.status.success());
    assert_eq!(names(&root.path().join("one")), ["f.txt"]);
    assert_eq!(names(&root.path().join("two")), ["f.txt"]);
}

#[test]
fn a_bad_directory_stops_that_one_and_not_the_run() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("two/inner")).unwrap();
    fs::write(root.path().join("two/inner/f.txt"), "").unwrap();
    let output = flatten(&[&at(&root, "missing"), &at(&root, "two")], "");
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("no such file or directory"));
    assert_eq!(names(&root.path().join("two")), ["f.txt"]);
}

#[test]
fn a_file_target_fails() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a"), "").unwrap();
    let output = flatten(&[&at(&root, "a")], "");
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("not a directory"));
}

#[test]
fn a_missing_argument_is_a_usage_error() {
    assert_eq!(flatten(&[], "").status.code(), Some(2));
}

#[test]
fn a_deep_flatten_of_the_home_directory_is_refused() {
    let output = flatten(&["-d", "-y", &std::env::var("HOME").unwrap()], "");
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("refusing to flatten your home directory"));
}

#[test]
fn completions_need_no_directory() {
    let output = flatten(&["--completions", "zsh"], "");
    assert!(output.status.success());
    assert!(stdout(&output).contains("#compdef flatten"));
}

/// The shared `--completions` flag is flattened in, and clap will hand a
/// flattened struct's documentation to the command it lands in.
#[test]
fn help_describes_this_tool() {
    let output = flatten(&["--help"], "");
    assert!(stdout(&output).starts_with("Lift a directory's contents up out of"));
}
