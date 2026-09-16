use std::fs;
use std::path::Path;

use crate::walk::gather;

fn tree(entries: &[&str]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for name in entries {
        let path = root.path().join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, "{}").unwrap();
    }
    root
}

fn names(root: &Path) -> Vec<String> {
    let target = root.join(".");
    let mut found: Vec<String> = gather(&target)
        .unwrap()
        .files
        .iter()
        .map(|path| {
            path.strip_prefix(root)
                .unwrap_or(path)
                .display()
                .to_string()
        })
        .collect();
    found.sort();
    found
}

#[test]
fn a_directory_is_walked_for_json_and_nothing_else() {
    let root = tree(&[
        "a.json",
        "deep/b.json",
        "deep/c.JSON",
        "not-json.txt",
        "package.json.bak",
    ]);

    assert_eq!(
        names(root.path()),
        ["a.json", "deep/b.json", "deep/c.JSON"]
    );
}

#[test]
fn the_trees_nobody_means_are_skipped() {
    let root = tree(&[
        "a.json",
        "node_modules/b.json",
        "target/c.json",
        "build/deep/d.json",
        "vendor/e.json",
        ".cache/f.json",
        "__pycache__/g.json",
    ]);

    assert_eq!(names(root.path()), ["a.json"]);
}

#[test]
fn a_file_named_on_the_command_line_is_taken_whatever_it_is_called() {
    // The caller has said what they want by naming it, so `jqfmt .prettierrc`
    // formats it rather than reporting that it is not a `.json` file.
    let root = tree(&[".prettierrc"]);
    let gathered = gather(&root.path().join(".prettierrc")).unwrap();

    assert_eq!(gathered.files.len(), 1);
}

#[test]
fn a_target_that_is_not_there_is_a_failure_rather_than_an_empty_run() {
    let root = tempfile::tempdir().unwrap();
    let error = gather(&root.path().join("absent.json")).unwrap_err();

    assert!(error.contains("absent.json"), "{error}");
}

#[test]
fn a_symlinked_file_is_taken_with_the_tree_it_sits_in() {
    let root = tree(&["real.json"]);
    std::os::unix::fs::symlink(root.path().join("real.json"), root.path().join("link.json"))
        .unwrap();

    assert_eq!(names(root.path()), ["link.json", "real.json"]);
}
