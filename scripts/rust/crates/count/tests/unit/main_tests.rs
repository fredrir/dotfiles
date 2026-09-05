use super::*;
use testkit::tree;

fn children(directory: &Path, no_hidden: bool) -> usize {
    walk::list(directory, &counting(no_hidden)).unwrap().len()
}

fn every(directory: &Path, no_hidden: bool) -> Walked<()> {
    descendants(directory, &counting(no_hidden))
}

#[test]
fn counts_direct_children() {
    let root = tree(&["a", ".b", "sub/c", "sub/.hid/d", ".hidden/e"]);
    assert_eq!(children(root.path(), false), 4);
}

#[test]
fn counts_every_descendant() {
    let root = tree(&["a", ".b", "sub/c", "sub/.hid/d", ".hidden/e"]);
    assert_eq!(every(root.path(), false).items.len(), 8);
}

#[test]
fn skips_hidden_children() {
    let root = tree(&["a", ".b", "sub/c", "sub/.hid/d", ".hidden/e"]);
    assert_eq!(children(root.path(), true), 2);
}

#[test]
fn hidden_directories_take_their_subtree_with_them() {
    let root = tree(&["a", ".b", "sub/c", "sub/.hid/d", ".hidden/e"]);
    assert_eq!(every(root.path(), true).items.len(), 3);
}

#[test]
fn an_empty_directory_counts_zero() {
    let root = tree(&[]);
    assert_eq!(children(root.path(), false), 0);
    assert_eq!(every(root.path(), false).items.len(), 0);
}

#[test]
fn unreadable_directories_are_reported_not_counted() {
    let root = tree(&["a", ".b", "sub/c", "sub/.hid/d", ".hidden/e"]);
    let walked = every(&root.path().join("missing"), false);
    assert_eq!(walked.items.len(), 0);
    assert_eq!(walked.unreadable, 1);
}

#[test]
fn a_missing_directory_is_named_by_the_listing_error() {
    let root = tree(&["a", ".b", "sub/c", "sub/.hid/d", ".hidden/e"]);
    let error = walk::list(&root.path().join("missing"), &counting(false)).unwrap_err();
    assert!(error.contains("missing"), "{error}");
}

#[cfg(unix)]
#[test]
fn linked_directories_count_once_and_are_not_followed() {
    let root = tree(&["a", ".b", "sub/c", "sub/.hid/d", ".hidden/e"]);
    std::os::unix::fs::symlink(root.path().join("sub"), root.path().join("link")).unwrap();
    assert_eq!(children(root.path(), false), 5);
    assert_eq!(every(root.path(), false).items.len(), 9);
}

#[cfg(unix)]
#[test]
fn a_fifo_counts_as_one_entry_and_is_not_descended() {
    let root = tree(&["a", "sub/c"]);
    let made = std::process::Command::new("mkfifo")
        .arg(root.path().join("pipe"))
        .status()
        .unwrap();
    assert!(made.success());
    assert_eq!(children(root.path(), false), 3);
    let walked = every(root.path(), false);
    assert_eq!(walked.items.len(), 4);
    assert_eq!(walked.unreadable, 0);
    assert_eq!(walked.unknown, 0);
}

#[test]
fn a_file_is_not_a_directory() {
    let root = tree(&["a", ".b", "sub/c", "sub/.hid/d", ".hidden/e"]);
    assert!(path::require_directory(&root.path().join("a")).is_err());
    assert!(path::require_directory(&root.path().join("missing")).is_err());
    assert!(path::require_directory(root.path()).is_ok());
}
