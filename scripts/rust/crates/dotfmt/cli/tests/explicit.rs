#![forbid(unsafe_code)]

mod common;

use common::dotfmt;
use testkit::tree_pairs;

#[test]
fn explicitly_named_unmapped_files_fail_instead_of_silently_succeeding() {
    let root = tree_pairs(&[("dotfmt.dotfile", "json {}\n"), ("notes.txt", "hello\n")]);
    let output = dotfmt(root.path()).arg("notes.txt").run();
    assert_eq!(output.code(), Some(1), "{output:?}");
    assert!(
        output.stderr.contains("no configured language"),
        "{output:?}"
    );
}
