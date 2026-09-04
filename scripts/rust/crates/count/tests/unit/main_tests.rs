use super::*;

fn tree() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let path = |name: &str| root.path().join(name);
    fs::write(path("a"), "").unwrap();
    fs::write(path(".b"), "").unwrap();
    fs::create_dir(path("sub")).unwrap();
    fs::write(path("sub/c"), "").unwrap();
    fs::create_dir(path("sub/.hid")).unwrap();
    fs::write(path("sub/.hid/d"), "").unwrap();
    fs::create_dir(path(".hidden")).unwrap();
    fs::write(path(".hidden/e"), "").unwrap();
    root
}

#[test]
fn counts_direct_children() {
    let root = tree();
    assert_eq!(count_children(root.path(), false).unwrap(), 4);
}

#[test]
fn counts_every_descendant() {
    let root = tree();
    assert_eq!(count_recursive(root.path(), false).entries, 8);
}

#[test]
fn skips_hidden_children() {
    let root = tree();
    assert_eq!(count_children(root.path(), true).unwrap(), 2);
}

#[test]
fn hidden_directories_take_their_subtree_with_them() {
    let root = tree();
    assert_eq!(count_recursive(root.path(), true).entries, 3);
}

#[test]
fn an_empty_directory_counts_zero() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(count_children(root.path(), false).unwrap(), 0);
    assert_eq!(count_recursive(root.path(), false).entries, 0);
}

#[test]
fn unreadable_directories_are_reported_not_counted() {
    let root = tree();
    let tally = count_recursive(&root.path().join("missing"), false);
    assert_eq!(tally.entries, 0);
    assert_eq!(tally.unreadable, 1);
}

#[cfg(unix)]
#[test]
fn linked_directories_count_once_and_are_not_followed() {
    let root = tree();
    std::os::unix::fs::symlink(root.path().join("sub"), root.path().join("link")).unwrap();
    assert_eq!(count_children(root.path(), false).unwrap(), 5);
    assert_eq!(count_recursive(root.path(), false).entries, 9);
}

#[test]
fn a_file_is_not_a_directory() {
    let root = tree();
    assert!(require_directory(&root.path().join("a")).is_err());
    assert!(require_directory(&root.path().join("missing")).is_err());
    assert!(require_directory(root.path()).is_ok());
}
