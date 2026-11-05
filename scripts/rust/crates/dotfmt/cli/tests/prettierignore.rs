#![forbid(unsafe_code)]

mod common;

use common::dotfmt;
use std::fs;
use testkit::tree_pairs;

#[test]
fn prettierignore_excludes_discovered_files() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", "json {}\ntypescript {\n width = 80\n}\n"),
        (".prettierignore", "*.ts\n"),
        ("keep.json", "{\"a\":1}"),
        ("skip.ts", "const  a=1\n"),
    ]);
    let output = dotfmt(root.path()).arg(".").run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("skip.ts")).unwrap(),
        "const  a=1\n"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("keep.json")).unwrap(),
        "{\n  \"a\": 1\n}\n"
    );
}
