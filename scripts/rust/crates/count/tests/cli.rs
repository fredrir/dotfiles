use testkit::{Bin, stderr, stdout, tree};

#[test]
fn prints_the_number_of_entries() {
    let root = tree(&["a", ".b", "sub/c"]);
    let output = Bin::new(env!("CARGO_BIN_EXE_count"))
        .arg(root.path())
        .output();
    assert!(output.status.success());
    assert_eq!(stdout(&output), "3\n");
}

#[test]
fn bundled_short_flags_combine() {
    let root = tree(&["a", ".b", "sub/c"]);
    let output = Bin::new(env!("CARGO_BIN_EXE_count"))
        .arg("-rd")
        .arg(root.path())
        .output();
    assert_eq!(stdout(&output), "3\n");
}

#[test]
fn long_flags_match_the_short_ones() {
    let root = tree(&["a", ".b", "sub/c"]);
    let bundled = Bin::new(env!("CARGO_BIN_EXE_count"))
        .arg("-rd")
        .arg(root.path())
        .output();
    let spelled = Bin::new(env!("CARGO_BIN_EXE_count"))
        .args(["--recursive", "--no-hidden"])
        .arg(root.path())
        .output();
    assert_eq!(stdout(&bundled), stdout(&spelled));
}

#[test]
fn a_file_target_fails() {
    let root = tree(&["a", ".b", "sub/c"]);
    let output = Bin::new(env!("CARGO_BIN_EXE_count"))
        .arg(root.path().join("a"))
        .output();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("not a directory"));
}

#[test]
fn a_missing_directory_is_told_apart_from_a_file() {
    let root = tree(&["a", ".b", "sub/c"]);
    let output = Bin::new(env!("CARGO_BIN_EXE_count"))
        .arg(root.path().join("nope"))
        .output();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("no such file or directory"));
}

#[test]
fn a_missing_argument_is_a_usage_error() {
    let output = Bin::new(env!("CARGO_BIN_EXE_count")).output();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn completions_need_no_directory() {
    let output = Bin::new(env!("CARGO_BIN_EXE_count"))
        .args(["--completions", "zsh"])
        .output();
    assert!(output.status.success());
    assert!(stdout(&output).contains("#compdef count"));
}

#[test]
fn the_command_dump_describes_the_parser() {
    let output = Bin::new(env!("CARGO_BIN_EXE_count"))
        .arg("--command-dump")
        .output();
    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.starts_with("C\tcount\t0\tCount items inside a directory"));
    assert!(text.contains("\toption\trecursive\t-r,--recursive\t"));
    assert!(text.contains("\targument\tdirectory\t\tDIRECTORY\t"));
}

#[test]
fn help_describes_this_tool() {
    let output = Bin::new(env!("CARGO_BIN_EXE_count")).arg("--help").output();
    assert!(stdout(&output).starts_with("Count items inside a directory"));
}
