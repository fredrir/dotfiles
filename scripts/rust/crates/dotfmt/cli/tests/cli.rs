#![forbid(unsafe_code)]

mod common;

use common::dotfmt;
use std::fs;
use testkit::tree_pairs;

#[test]
fn help_completions_and_metadata_describe_the_unified_cli() {
    let root = tree_pairs(&[]);
    let output = dotfmt(root.path()).arg("--help").run();
    assert!(output.success(), "{output:?}");
    for flag in [
        "--check",
        "--add",
        "--sync",
        "--dialect",
        "--lang",
        "--editor",
        "--stdin",
        "--owns",
        "--verbose",
        "--quiet",
        "--completions",
    ] {
        assert!(output.stdout.contains(flag), "missing {flag}: {output:?}");
    }
    assert!(!output.stdout.contains("placeholder"));
    let output = dotfmt(root.path()).args(["--completions", "zsh"]).run();
    assert!(output.success(), "{output:?}");
    assert!(output.stdout.contains("#compdef dotfmt"));
    assert!(output.stdout.contains("--lang"));
    let output = dotfmt(root.path()).arg("--command-dump").run();
    assert!(output.success(), "{output:?}");
    assert!(output.stdout.contains("\"--lang\""));
    assert!(
        output
            .stdout
            .contains("\"choices\":[\"conf\",\"json\",\"lua\",\"md\",\"markdown\"]"),
        "{output:?}"
    );
}

#[test]
fn add_prompts_and_sync_only_updates_an_existing_unified_config() {
    let root = tree_pairs(&[]);
    let output = dotfmt(root.path()).arg("--sync").run();
    assert!(!output.success(), "{output:?}");
    assert!(!root.path().join("dotfmt.dotfile").exists());
    let output = dotfmt(root.path()).arg("--add").stdin("n\n").run();
    assert!(output.success(), "{output:?}");
    assert!(!root.path().join("dotfmt.dotfile").exists());
    let output = dotfmt(root.path()).arg("--add").stdin("y\n").run();
    assert!(output.success(), "{output:?}");
    let bundled = fs::read_to_string(root.path().join("dotfmt.dotfile")).unwrap();
    for language in ["conf {", "markdown {", "json {", "lua {"] {
        assert!(bundled.contains(language));
    }
    fs::write(root.path().join("dotfmt.dotfile"), "json {}\n").unwrap();
    let output = dotfmt(root.path()).arg("--sync").run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("dotfmt.dotfile")).unwrap(),
        bundled
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn verbose_output_keeps_heading_and_language_rows() {
    let root = tree_pairs(&[("dotfmt.dotfile", "json {}\n"), ("a.json", "{}\n")]);
    let output = dotfmt(root.path()).args(["-v", "--check", "."]).run();
    assert!(output.success(), "{output:?}");
    assert!(
        output.stderr.starts_with("\n  dotfmt  .  check\n\n"),
        "{output:?}"
    );
    assert!(output.stderr.contains("json"));
    assert!(output.stderr.contains("1 file checked"));
}

#[test]
fn conflicting_flags_remain_usage_errors() {
    let root = tree_pairs(&[]);
    for args in [
        ["--check", "--add"],
        ["--check", "--sync"],
        ["--add", "--sync"],
        ["--verbose", "--quiet"],
    ] {
        assert_eq!(dotfmt(root.path()).args(args).run().code(), Some(2));
    }
}
