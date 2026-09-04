use super::*;

#[test]
fn a_directory_goes_with_everything_in_it() {
    let root = tempfile::tempdir().unwrap();
    let tree = root.path().join("tree");
    fs::create_dir_all(tree.join("deep")).unwrap();
    fs::write(tree.join("deep/file"), "x").unwrap();
    assert!(remove_directory(&tree).unwrap());
    assert!(!tree.exists());
}

#[test]
fn a_nested_repository_survives_and_keeps_its_parents() {
    let root = tempfile::tempdir().unwrap();
    let tree = root.path().join("tree");
    fs::create_dir_all(tree.join("nested/.git")).unwrap();
    fs::write(tree.join("nested/kept"), "x").unwrap();
    fs::write(tree.join("gone"), "x").unwrap();
    assert!(!remove_directory(&tree).unwrap());
    assert!(tree.join("nested/kept").exists());
    assert!(!tree.join("gone").exists());
}

#[test]
fn pruning_stops_at_the_root_and_at_anything_left() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let file = root.join("a/b/file");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(root.join("a/kept"), "x").unwrap();
    prune(root, &file);
    assert!(!root.join("a/b").exists());
    assert!(root.join("a/kept").exists());
    assert!(root.exists());
}
