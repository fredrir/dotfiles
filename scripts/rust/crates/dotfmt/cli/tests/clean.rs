#![forbid(unsafe_code)]

mod common;

use common::dotfmt;
use std::fs;
use testkit::tree_pairs;

const CONFIG: &str =
    "{\n indent = 2\n final_newline = false\n}\njson {}\nlua {}\nmarkdown {}\nconf {}\nyaml {}\n";

#[test]
fn clean_requires_the_markdown_language() {
    let root = tree_pairs(&[("dotfmt.dotfile", CONFIG), ("a.md", "**bold**\n")]);
    for args in [
        vec!["--clean", "a.md"],
        vec!["--clean", "-l", "json", "a.md"],
        vec!["--clean", "-l", "json,md", "a.md"],
    ] {
        let output = dotfmt(root.path()).args(args).run();
        assert_eq!(output.code(), Some(1), "{output:?}");
        assert!(
            output.stderr.contains("--clean requires --lang markdown"),
            "{output:?}"
        );
        assert_eq!(
            fs::read_to_string(root.path().join("a.md")).unwrap(),
            "**bold**\n",
            "{output:?}"
        );
    }
}

#[test]
fn clean_formats_and_strips_markdown_formatting() {
    let root = tree_pairs(&[("dotfmt.dotfile", CONFIG), ("a.md", "**bold** text\n")]);
    for args in [
        vec!["-c", "-l", "markdown", "a.md"],
        vec!["--clean", "--lang", "md", "a.md"],
    ] {
        let output = dotfmt(root.path()).args(&args).run();
        assert!(output.success(), "{output:?}");
        assert_eq!(
            fs::read_to_string(root.path().join("a.md")).unwrap(),
            "bold text"
        );
        fs::write(root.path().join("a.md"), "**bold** text\n").unwrap();
    }
    let output = dotfmt(root.path())
        .args(["-c", "-l", "markdown", "--stdin", "note.md"])
        .stdin("## Title\n\n**bold**\n")
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(output.stdout, "Title\n\nbold");
}

#[test]
fn clean_settles_and_check_reports_stripping_as_a_difference() {
    let root = tree_pairs(&[("dotfmt.dotfile", CONFIG), ("a.md", "**bold**\n")]);
    let output = dotfmt(root.path()).args(["--clean", "-l", "md", "a.md"]).run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("a.md")).unwrap(),
        "bold"
    );
    let check = dotfmt(root.path())
        .args(["--clean", "-l", "md", "--check", "a.md"])
        .run();
    assert!(check.success(), "{check:?}");
    fs::write(root.path().join("a.md"), "**bold**\n").unwrap();
    let check = dotfmt(root.path())
        .args(["--clean", "-l", "md", "--check", "a.md"])
        .run();
    assert_eq!(check.code(), Some(1), "{check:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("a.md")).unwrap(),
        "**bold**\n"
    );
}

#[test]
fn clean_conflicts_with_placement_and_ownership_flags() {
    let root = tree_pairs(&[]);
    for args in [
        vec!["--clean", "--add"],
        vec!["--clean", "--sync"],
        vec!["--clean", "--owns"],
    ] {
        assert_eq!(dotfmt(root.path()).args(&args).run().code(), Some(2));
    }
}
