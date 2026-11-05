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

#[test]
fn prettierignore_applies_to_explicit_paths_explain_and_owns() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", "typescript {}\n"),
        (".prettierignore", "src/**\n"),
        ("src/a.ts", "const  a=1\n"),
    ]);
    let output = dotfmt(root.path()).args(["--check", "src/a.ts"]).run();
    assert!(output.success(), "{output:?}");
    let output = dotfmt(root.path()).args(["--explain", "src/a.ts"]).run();
    assert!(output.success(), "{output:?}");
    assert!(output.stdout.contains("skipped  excluded"), "{output:?}");
    let output = dotfmt(root.path())
        .args(["--owns"])
        .stdin("src/a.ts\0")
        .run();
    assert!(output.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
}
