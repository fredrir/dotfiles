use std::fs;
use std::path::Path;

use testkit::{Bin, TempDir, stderr, stdout, tree};

fn sample() -> TempDir {
    tree(&[
        "a.rs=// note\nlet x = 1;\n",
        "b.py=x = 1  # note\n",
        "c.txt=# note\n",
    ])
}

fn read(root: &Path, name: &str) -> String {
    fs::read_to_string(root.join(name)).unwrap()
}

#[test]
fn bare_it_prints_help() {
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert!(output.status.success());
    assert!(stdout(&output).contains("Usage: doc-purge"));
}

#[test]
fn a_dry_run_writes_nothing() {
    let root = sample();
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
        .arg(root.path())
        .arg("--dry")
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert!(output.status.success());
    assert!(stdout(&output).contains("-1"));
    assert_eq!(read(root.path(), "a.rs"), "// note\nlet x = 1;\n");
    assert_eq!(read(root.path(), "b.py"), "x = 1  # note\n");
}

#[test]
fn answering_no_writes_nothing() {
    let root = sample();
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
        .arg(root.path())
        .env("NO_COLOR", "1")
        .stdin("n\n")
        .output();
    assert!(output.status.success());
    assert!(stdout(&output).contains("cancelled"));
    assert_eq!(read(root.path(), "a.rs"), "// note\nlet x = 1;\n");
}

#[test]
fn answering_yes_purges() {
    let root = sample();
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
        .arg(root.path())
        .env("NO_COLOR", "1")
        .stdin("\n")
        .output();
    assert!(output.status.success());
    assert_eq!(read(root.path(), "a.rs"), "let x = 1;\n");
    assert_eq!(read(root.path(), "b.py"), "x = 1\n");
}

#[test]
fn the_yes_flag_skips_the_prompt() {
    let root = sample();
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
        .arg(root.path())
        .arg("-y")
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert!(output.status.success());
    assert_eq!(read(root.path(), "a.rs"), "let x = 1;\n");
}

#[test]
fn a_closed_stdin_writes_nothing() {
    let root = sample();
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
        .arg(root.path())
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(read(root.path(), "a.rs"), "// note\nlet x = 1;\n");
}

#[test]
fn a_type_filter_leaves_the_other_types_alone() {
    let root = sample();
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
        .arg(root.path())
        .args(["-y", "-t", "py"])
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert!(output.status.success());
    assert_eq!(read(root.path(), "a.rs"), "// note\nlet x = 1;\n");
    assert_eq!(read(root.path(), "b.py"), "x = 1\n");
}

#[test]
fn a_type_filter_takes_a_leading_dot_and_a_language_name() {
    let root = sample();
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
        .arg(root.path())
        .args(["-y", "-t", ".py,rust"])
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert!(output.status.success());
    assert_eq!(read(root.path(), "a.rs"), "let x = 1;\n");
    assert_eq!(read(root.path(), "b.py"), "x = 1\n");
}

#[test]
fn an_unknown_type_names_the_known_ones() {
    let root = sample();
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
        .arg(root.path())
        .args(["-t", "nope"])
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("unknown type: nope"));
    assert!(stderr(&output).contains("py"));
}

#[test]
fn a_file_of_an_unread_type_is_reported() {
    let root = sample();
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
        .arg(root.path().join("c.txt"))
        .arg("--dry")
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert!(output.status.success());
    assert!(stdout(&output).contains("not a file type doc-purge reads"));
    assert_eq!(read(root.path(), "c.txt"), "# note\n");
}

#[test]
fn a_missing_target_is_an_error() {
    let root = sample();
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
        .arg(root.path().join("nope"))
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("no such file or directory"));
}

#[test]
fn a_tree_with_nothing_to_purge_says_so() {
    let root = tree(&["a.rs=let x = 1;\n"]);
    let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
        .arg(root.path())
        .arg("--dry")
        .env("NO_COLOR", "1")
        .stdin("")
        .output();
    assert!(output.status.success());
    assert!(stdout(&output).contains("nothing to purge"));
}

#[test]
fn file_permissions_survive_a_purge() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let root = sample();
        let target = root.path().join("a.rs");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
        let output = Bin::new(env!("CARGO_BIN_EXE_doc-purge"))
            .arg(root.path())
            .arg("-y")
            .env("NO_COLOR", "1")
            .stdin("")
            .output();
        assert!(output.status.success());
        let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
    }
}
