use std::fs;

use testkit::{Bin, TempDir, at, names, stderr, stdout, tree};

fn nested() -> TempDir {
    tree(&[
        "documents/documents/doc_1.txt",
        "documents/documents/doc_2.txt",
        "documents/documents/.env",
    ])
}

#[test]
fn the_nesting_is_gone_and_nothing_was_said_about_it() {
    let root = nested();
    let output = Bin::new(env!("CARGO_BIN_EXE_flatten"))
        .arg(at(&root, "documents"))
        .plain()
        .stdin("")
        .output();
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
    Bin::new(env!("CARGO_BIN_EXE_flatten"))
        .arg(at(&root, "documents"))
        .plain()
        .stdin("")
        .output();
    let output = Bin::new(env!("CARGO_BIN_EXE_flatten"))
        .arg(at(&root, "documents"))
        .plain()
        .stdin("")
        .output();
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
    let output = Bin::new(env!("CARGO_BIN_EXE_flatten"))
        .arg("-n")
        .arg(at(&root, "documents"))
        .plain()
        .stdin("")
        .output();
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
    let output = Bin::new(env!("CARGO_BIN_EXE_flatten"))
        .arg("-v")
        .arg(at(&root, "documents"))
        .plain()
        .stdin("")
        .output();
    assert!(stdout(&output).contains("documents/.env -> .env"));
}

#[test]
fn a_deep_flatten_asks_before_it_starts() {
    let root = tree(&["pack/sub/x/b.txt"]);
    let output = Bin::new(env!("CARGO_BIN_EXE_flatten"))
        .arg("-d")
        .arg(at(&root, "pack"))
        .plain()
        .stdin("n\n")
        .output();
    assert!(output.status.success());
    assert!(stdout(&output).contains("Continue? [Y/n]"));
    assert!(stdout(&output).contains("flatten: cancelled"));
    assert!(root.path().join("pack/sub/x/b.txt").exists());
}

#[test]
fn a_deep_flatten_brings_everything_up() {
    let root = tree(&[
        "pack/sub/x/b.txt",
        "pack/empty/",
        "pack/README",
        "pack/sub/a.txt",
    ]);
    let output = Bin::new(env!("CARGO_BIN_EXE_flatten"))
        .arg("-d")
        .arg(at(&root, "pack"))
        .plain()
        .stdin("y\n")
        .output();
    assert!(output.status.success());
    assert_eq!(
        names(&root.path().join("pack")),
        ["README", "a.txt", "b.txt"]
    );
}

#[test]
fn a_name_two_entries_want_is_asked_about_one_at_a_time() {
    let root = tree(&[
        "pack/notes.txt=top",
        "pack/a/notes.txt=from-a",
        "pack/b/notes.txt=from-b",
    ]);

    // Take the first, decline the second, then start.
    let output = Bin::new(env!("CARGO_BIN_EXE_flatten"))
        .arg("-d")
        .arg(at(&root, "pack"))
        .plain()
        .stdin("y\nn\ny\n")
        .output();
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
    let root = tree(&[
        "pack/notes.txt=top",
        "pack/a/notes.txt=from-a",
        "pack/b/notes.txt=from-b",
    ]);

    let output = Bin::new(env!("CARGO_BIN_EXE_flatten"))
        .arg("-d")
        .arg(at(&root, "pack"))
        .plain()
        .stdin("a\ny\n")
        .output();
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
    let root = tree(&["pack/notes.txt=top", "pack/a/notes.txt=from-a"]);
    let output = Bin::new(env!("CARGO_BIN_EXE_flatten"))
        .args(["-d", "-y"])
        .arg(at(&root, "pack"))
        .plain()
        .stdin("")
        .output();
    assert!(output.status.success());
    assert!(!stdout(&output).contains("[Y/n"));
    assert_eq!(
        fs::read_to_string(root.path().join("pack/notes.txt")).unwrap(),
        "from-a"
    );
}

#[test]
fn every_directory_named_is_flattened() {
    let root = tree(&["one/inner/f.txt", "two/inner/f.txt"]);
    let output = Bin::new(env!("CARGO_BIN_EXE_flatten"))
        .args([at(&root, "one"), at(&root, "two")])
        .plain()
        .stdin("")
        .output();
    assert!(output.status.success());
    assert_eq!(names(&root.path().join("one")), ["f.txt"]);
    assert_eq!(names(&root.path().join("two")), ["f.txt"]);
}

#[test]
fn a_bad_directory_stops_that_one_and_not_the_run() {
    let root = tree(&["two/inner/f.txt"]);
    let output = Bin::new(env!("CARGO_BIN_EXE_flatten"))
        .args([at(&root, "missing"), at(&root, "two")])
        .plain()
        .stdin("")
        .output();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("no such file or directory"));
    assert_eq!(names(&root.path().join("two")), ["f.txt"]);
}

#[test]
fn a_file_target_fails() {
    let root = tree(&["a"]);
    let output = Bin::new(env!("CARGO_BIN_EXE_flatten"))
        .arg(at(&root, "a"))
        .plain()
        .stdin("")
        .output();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("not a directory"));
}

#[test]
fn a_missing_argument_is_a_usage_error() {
    let output = Bin::new(env!("CARGO_BIN_EXE_flatten"))
        .plain()
        .stdin("")
        .output();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn a_deep_flatten_of_the_home_directory_is_refused() {
    let output = Bin::new(env!("CARGO_BIN_EXE_flatten"))
        .args(["-d", "-y"])
        .arg(std::env::var("HOME").unwrap())
        .plain()
        .stdin("")
        .output();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("refusing to flatten your home directory"));
}

#[test]
fn completions_need_no_directory() {
    let output = Bin::new(env!("CARGO_BIN_EXE_flatten"))
        .args(["--completions", "zsh"])
        .plain()
        .stdin("")
        .output();
    assert!(output.status.success());
    assert!(stdout(&output).contains("#compdef flatten"));
}

#[test]
fn help_describes_this_tool() {
    let output = Bin::new(env!("CARGO_BIN_EXE_flatten"))
        .arg("--help")
        .plain()
        .stdin("")
        .output();
    assert!(stdout(&output).starts_with("Lift a directory's contents up out of"));
}
