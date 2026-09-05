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
    let root = testkit::tree(&["one=1", ".hidden=2", "deep/two=3", "deep/deeper/three=4"]);
    assert_eq!(files_in(root.path()), 4);
}

#[test]
fn a_nested_repository_is_not_counted() {
    let root = testkit::tree(&["one=1", "nested/.git/", "nested/two=2"]);
    assert_eq!(files_in(root.path()), 1);
}
