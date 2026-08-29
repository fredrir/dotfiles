use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn doc_purge(args: &[&str], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_doc-purge"))
        .args(args)
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("doc-purge runs");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("wrote stdin");
    child.wait_with_output().expect("doc-purge finishes")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn tree() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a.rs"), "// note\nlet x = 1;\n").unwrap();
    fs::write(root.path().join("b.py"), "x = 1  # note\n").unwrap();
    fs::write(root.path().join("c.txt"), "# note\n").unwrap();
    root
}

fn read(root: &Path, name: &str) -> String {
    fs::read_to_string(root.join(name)).unwrap()
}

fn path(root: &Path) -> &str {
    root.to_str().unwrap()
}

#[test]
fn bare_it_prints_help() {
    let output = doc_purge(&[], "");
    assert!(output.status.success());
    assert!(stdout(&output).contains("Usage: doc-purge"));
}

#[test]
fn a_dry_run_writes_nothing() {
    let root = tree();
    let output = doc_purge(&[path(root.path()), "--dry"], "");
    assert!(output.status.success());
    assert!(stdout(&output).contains("-1"));
    assert_eq!(read(root.path(), "a.rs"), "// note\nlet x = 1;\n");
    assert_eq!(read(root.path(), "b.py"), "x = 1  # note\n");
}

#[test]
fn answering_no_writes_nothing() {
    let root = tree();
    let output = doc_purge(&[path(root.path())], "n\n");
    assert!(output.status.success());
    assert!(stdout(&output).contains("cancelled"));
    assert_eq!(read(root.path(), "a.rs"), "// note\nlet x = 1;\n");
}

#[test]
fn answering_yes_purges() {
    let root = tree();
    let output = doc_purge(&[path(root.path())], "\n");
    assert!(output.status.success());
    assert_eq!(read(root.path(), "a.rs"), "let x = 1;\n");
    assert_eq!(read(root.path(), "b.py"), "x = 1\n");
}

#[test]
fn the_yes_flag_skips_the_prompt() {
    let root = tree();
    let output = doc_purge(&[path(root.path()), "-y"], "");
    assert!(output.status.success());
    assert_eq!(read(root.path(), "a.rs"), "let x = 1;\n");
}

#[test]
fn a_closed_stdin_writes_nothing() {
    let root = tree();
    let output = doc_purge(&[path(root.path())], "");
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(read(root.path(), "a.rs"), "// note\nlet x = 1;\n");
}

#[test]
fn a_type_filter_leaves_the_other_types_alone() {
    let root = tree();
    let output = doc_purge(&[path(root.path()), "-y", "-t", "py"], "");
    assert!(output.status.success());
    assert_eq!(read(root.path(), "a.rs"), "// note\nlet x = 1;\n");
    assert_eq!(read(root.path(), "b.py"), "x = 1\n");
}

#[test]
fn a_type_filter_takes_a_leading_dot_and_a_language_name() {
    let root = tree();
    let output = doc_purge(&[path(root.path()), "-y", "-t", ".py,rust"], "");
    assert!(output.status.success());
    assert_eq!(read(root.path(), "a.rs"), "let x = 1;\n");
    assert_eq!(read(root.path(), "b.py"), "x = 1\n");
}

#[test]
fn an_unknown_type_names_the_known_ones() {
    let root = tree();
    let output = doc_purge(&[path(root.path()), "-t", "nope"], "");
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("unknown type: nope"));
    assert!(stderr(&output).contains("py"));
}

#[test]
fn a_file_of_an_unread_type_is_reported() {
    let root = tree();
    let target = root.path().join("c.txt");
    let output = doc_purge(&[target.to_str().unwrap(), "--dry"], "");
    assert!(output.status.success());
    assert!(stdout(&output).contains("not a file type doc-purge reads"));
    assert_eq!(read(root.path(), "c.txt"), "# note\n");
}

#[test]
fn a_missing_target_is_an_error() {
    let root = tree();
    let target = root.path().join("nope");
    let output = doc_purge(&[target.to_str().unwrap()], "");
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("no such file or directory"));
}

#[test]
fn a_tree_with_nothing_to_purge_says_so() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a.rs"), "let x = 1;\n").unwrap();
    let output = doc_purge(&[path(root.path()), "--dry"], "");
    assert!(output.status.success());
    assert!(stdout(&output).contains("nothing to purge"));
}

#[test]
fn file_permissions_survive_a_purge() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let root = tree();
        let target = root.path().join("a.rs");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
        let output = doc_purge(&[path(root.path()), "-y"], "");
        assert!(output.status.success());
        let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
    }
}
