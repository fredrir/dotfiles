#![forbid(unsafe_code)]

mod common;

use common::dotfmt;
use std::fs;
use testkit::tree_pairs;

#[test]
fn bib_files_are_formatted_and_configured_separately() {
    let root = tree_pairs(&[
        (
            "dotfmt.dotfile",
            "bib {\n indent = 4\n align = false\n trailing_comma = true\n}\n",
        ),
        ("refs.bib", "@ARTICLE{k,Title={T},Year=2024}\n"),
    ]);
    let output = dotfmt(root.path()).arg(".").run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("refs.bib")).unwrap(),
        "@article{k,\n    title = {T},\n    year = 2024,\n}"
    );
}

#[test]
fn bib_supports_check_stdin_editor_and_owns() {
    let root = tree_pairs(&[("dotfmt.dotfile", "bib {}\n")]);
    let output = dotfmt(root.path())
        .args(["--stdin", "refs.bib"])
        .stdin("@misc{k,title={T}}\n")
        .run();
    assert!(output.success(), "{output:?}");
    assert!(output.stdout.contains("title = {T}"));

    let output = dotfmt(root.path())
        .args(["-e", "refs.bib"])
        .stdin("@misc{k,title={T}}\n")
        .run();
    assert!(output.success(), "{output:?}");
    assert!(output.stdout.contains("title = {T}"));

    let output = dotfmt(root.path())
        .args(["--owns", "-l", "bib"])
        .stdin("refs.bib\0paper.tex\0")
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(output.stdout, "refs.bib\0");
}

#[test]
fn malformed_bib_is_reported_and_left_unchanged() {
    let original = "@article{x, title = {unclosed\n";
    let root = tree_pairs(&[("dotfmt.dotfile", "bib {}\n"), ("bad.bib", original)]);
    let output = dotfmt(root.path()).arg("bad.bib").run();
    assert_eq!(output.code(), Some(1), "{output:?}");
    assert!(output.stderr.contains("unbalanced braces"), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("bad.bib")).unwrap(),
        original
    );
}

#[test]
fn bib_dialect_override_is_rejected() {
    let root = tree_pairs(&[("dotfmt.dotfile", "bib {}\n"), ("a.bib", "@misc{k}")]);
    let output = dotfmt(root.path())
        .args(["-l", "bib", "--dialect", "biblatex", "a.bib"])
        .run();
    assert_eq!(output.code(), Some(1), "{output:?}");
    assert!(output.stderr.contains("bib has no"), "{output:?}");
}

#[test]
fn editor_mode_passes_through_a_rejected_bibliography() {
    let root = tree_pairs(&[("dotfmt.dotfile", "bib {}\n")]);
    let input = "@article{x, title = {unclosed\n";
    let output = dotfmt(root.path())
        .args(["-e", "bad.bib"])
        .stdin(input)
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(output.stdout, input);
}
