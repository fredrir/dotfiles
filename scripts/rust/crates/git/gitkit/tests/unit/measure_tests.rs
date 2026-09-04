use super::*;

#[test]
fn a_last_line_without_a_newline_is_still_a_line() {
    assert_eq!(count_lines(b""), 0);
    assert_eq!(count_lines(b"one\n"), 1);
    assert_eq!(count_lines(b"one\ntwo"), 2);
    assert_eq!(count_lines(b"\n\n"), 2);
}

#[test]
fn a_nul_early_on_means_binary() {
    assert!(!is_binary(b"plain text\n"));
    assert!(is_binary(b"text\0more"));
}

#[test]
fn files_are_counted_through_the_whole_tree() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    fs::write(root.join("one"), "1").unwrap();
    fs::write(root.join(".hidden"), "2").unwrap();
    fs::create_dir_all(root.join("deep/deeper")).unwrap();
    fs::write(root.join("deep/two"), "3").unwrap();
    fs::write(root.join("deep/deeper/three"), "4").unwrap();
    assert_eq!(files_in(root), 4);
}

#[test]
fn a_nested_repository_is_not_counted() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    fs::write(root.join("one"), "1").unwrap();
    fs::create_dir_all(root.join("nested/.git")).unwrap();
    fs::write(root.join("nested/two"), "2").unwrap();
    assert_eq!(files_in(root), 1);
}
