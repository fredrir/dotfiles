#![forbid(unsafe_code)]

use testkit::{Bin, TempDir, at, names, stderr, stdout, tree};

fn bin() -> Bin {
    Bin::new(env!("CARGO_BIN_EXE_rm-emptydirs")).plain()
}

fn sample() -> TempDir {
    tree(&["pack/file=keep", "pack/empty/", "pack/also_empty/"])
}

#[test]
fn bare_it_prints_help() {
    let output = bin().stdin("").output();
    assert!(output.status.success());
    assert!(stdout(&output).contains("Usage: rm-emptydirs"));
}

#[test]
fn a_dry_run_lists_what_would_go_and_changes_nothing() {
    let root = sample();
    let output = bin().arg(at(&root, "pack")).arg("--dry").stdin("").output();
    assert!(output.status.success());
    let shown = stdout(&output);
    assert!(shown.contains("empty"), "{shown}");
    assert!(shown.contains("2 empty folders"), "{shown}");
    assert!(root.path().join("pack/empty").exists());
    assert!(root.path().join("pack/also_empty").exists());
}

#[test]
fn answering_yes_removes_the_empty_folders() {
    let root = sample();
    let output = bin().arg(at(&root, "pack")).stdin("\n").output();
    assert!(output.status.success());
    assert_eq!(names(&root.path().join("pack")), ["file"]);
}

#[test]
fn answering_no_changes_nothing() {
    let root = sample();
    let output = bin().arg(at(&root, "pack")).stdin("n\n").output();
    assert!(output.status.success());
    assert!(stdout(&output).contains("cancelled"));
    assert_eq!(names(&root.path().join("pack")), ["also_empty", "empty", "file"]);
}

#[test]
fn the_yes_flag_skips_the_prompt() {
    let root = sample();
    let output = bin()
        .arg(at(&root, "pack"))
        .arg("-y")
        .stdin("")
        .output();
    assert!(output.status.success());
    assert!(!stdout(&output).contains("Continue?"));
    assert_eq!(names(&root.path().join("pack")), ["file"]);
}

#[test]
fn a_closed_stdin_removes_nothing() {
    let root = sample();
    let output = bin().arg(at(&root, "pack")).stdin("").output();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(names(&root.path().join("pack")), ["also_empty", "empty", "file"]);
}

#[test]
fn a_folder_emptied_by_removal_goes_too() {
    let root = tree(&["pack/inner/deep/"]);
    let output = bin()
        .arg(at(&root, "pack"))
        .arg("-y")
        .stdin("")
        .output();
    assert!(output.status.success());
    // The target itself stays even though it ends up empty.
    assert!(root.path().join("pack").exists());
    assert!(!root.path().join("pack/inner").exists());
}

#[test]
fn the_usual_skip_list_is_left_alone() {
    let root = tree(&["pack/node_modules/", "pack/empty/"]);
    let output = bin()
        .arg(at(&root, "pack"))
        .arg("-y")
        .stdin("")
        .output();
    assert!(output.status.success());
    assert_eq!(names(&root.path().join("pack")), ["node_modules"]);
}

#[test]
fn the_all_flag_removes_skipped_folders_too() {
    let root = tree(&["pack/node_modules/", "pack/empty/"]);
    let output = bin()
        .arg(at(&root, "pack"))
        .args(["-y", "-a"])
        .stdin("")
        .output();
    assert!(output.status.success());
    assert_eq!(names(&root.path().join("pack")), Vec::<String>::new());
}

#[test]
fn hidden_folders_are_left_alone() {
    let root = tree(&["pack/.config/", "pack/empty/"]);
    let output = bin()
        .arg(at(&root, "pack"))
        .arg("-y")
        .stdin("")
        .output();
    assert!(output.status.success());
    assert_eq!(names(&root.path().join("pack")), [".config"]);
}

#[test]
fn the_all_flag_removes_hidden_folders_too() {
    let root = tree(&["pack/.config/", "pack/empty/"]);
    let output = bin()
        .arg(at(&root, "pack"))
        .args(["-y", "-a"])
        .stdin("")
        .output();
    assert!(output.status.success());
    assert_eq!(names(&root.path().join("pack")), Vec::<String>::new());
}

#[test]
fn a_nested_target_is_never_removed_as_part_of_another() {
    let root = tree(&["pack/sub/"]);
    let output = bin()
        .args([at(&root, "pack"), at(&root, "pack/sub")])
        .arg("-y")
        .stdin("")
        .output();
    assert!(output.status.success());
    assert!(root.path().join("pack/sub").exists());
}

#[test]
fn a_tree_with_nothing_to_remove_says_so() {
    let root = tree(&["pack/file=keep"]);
    let output = bin().arg(at(&root, "pack")).stdin("").output();
    assert!(output.status.success());
    assert!(stdout(&output).contains("nothing to remove"));
}

#[test]
fn verbose_lists_every_row() {
    let mut lines: Vec<String> = (0..14).map(|index| format!("pack/empty_{index}/")).collect();
    let borrowed: Vec<&str> = lines.iter().map(String::as_str).collect();
    let root = tree(&borrowed);

    let terse = bin().arg(at(&root, "pack")).stdin("n\n").output();
    assert!(stdout(&terse).contains("and 2 more"), "{}", stdout(&terse));

    lines.clear();
    let full = bin()
        .arg(at(&root, "pack"))
        .arg("-v")
        .stdin("n\n")
        .output();
    let shown = stdout(&full);
    assert!(!shown.contains("more"), "{shown}");
    assert!(shown.contains("empty_13"), "{shown}");
}

#[test]
fn a_file_target_fails() {
    let root = tree(&["a"]);
    let output = bin().arg(at(&root, "a")).stdin("").output();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("not a directory"));
}

#[test]
fn a_missing_target_is_an_error() {
    let root = tree(&["a"]);
    let output = bin().arg(at(&root, "nope")).stdin("").output();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("no such file or directory"));
}

#[test]
fn completions_need_no_target() {
    let output = bin().args(["--completions", "zsh"]).stdin("").output();
    assert!(output.status.success());
    assert!(stdout(&output).contains("#compdef rm-emptydirs"));
}

#[cfg(unix)]
#[test]
fn a_symlinked_target_is_refused() {
    let root = tree(&["real/"]);
    std::os::unix::fs::symlink(root.path().join("real"), root.path().join("link")).unwrap();
    let output = bin().arg(at(&root, "link")).stdin("").output();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("symbolic link"));
}
