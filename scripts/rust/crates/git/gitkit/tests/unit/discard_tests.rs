use super::*;

#[test]
fn a_directory_goes_with_everything_in_it() {
    let root = testkit::tree(&["tree/deep/file=x"]);
    let target = root.path().join("tree");
    assert!(remove_directory(&target).unwrap());
    assert!(!target.exists());
}

#[test]
fn a_nested_repository_survives_and_keeps_its_parents() {
    let root = testkit::tree(&["tree/nested/.git/", "tree/nested/kept=x", "tree/gone=x"]);
    let target = root.path().join("tree");
    assert!(!remove_directory(&target).unwrap());
    assert!(target.join("nested/kept").exists());
    assert!(!target.join("gone").exists());
}

#[test]
fn pruning_stops_at_the_root_and_at_anything_left() {
    let root = testkit::tree(&["a/b/", "a/kept=x"]);
    let root = root.path();
    let file = root.join("a/b/file");
    prune(root, &file);
    assert!(!root.join("a/b").exists());
    assert!(root.join("a/kept").exists());
    assert!(root.exists());
}
