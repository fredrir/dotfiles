#![forbid(unsafe_code)]

mod common;

use common::dotfmt;
use std::fs;
use testkit::tree_pairs;

const CONFIG: &str =
    "{\n indent = 2\n final_newline = false\n}\njson {}\nlua {}\nmarkdown {}\nconf {}\n";

#[test]
fn config_exclusions_apply_to_files_stdin_and_ownership() {
    let root = tree_pairs(&[
        (
            "dotfmt.dotfile",
            "json {}\nlua {}\nexcluded_files {\n skip.json\n}\n",
        ),
        ("skip.json", "{\"x\":1}"),
        ("keep.lua", "local x=1"),
    ]);
    let output = dotfmt(root.path())
        .args(["--stdin", "skip.json"])
        .stdin("unformatted input")
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(output.stdout, "unformatted input");
    let output = dotfmt(root.path()).arg("skip.json").run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("skip.json")).unwrap(),
        "{\"x\":1}"
    );
    let output = dotfmt(root.path())
        .arg("--owns")
        .stdin("skip.json\0keep.lua\0unknown.txt\0")
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(output.stdout, "keep.lua\0");
    assert!(output.stderr.is_empty());
}

#[test]
fn custom_language_patterns_assign_unknown_extensions() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", "conf {\n include {\n  *.ssh\n }\n}\n"),
        ("hosts.ssh", "Host   example\n  User    user\n"),
    ]);
    let output = dotfmt(root.path())
        .arg("--owns")
        .stdin("hosts.ssh\0other.txt\0")
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(output.stdout, "hosts.ssh\0");
    let output = dotfmt(root.path()).arg(".").run();
    assert!(output.success(), "{output:?}");
}

#[test]
fn a_dialect_cannot_be_applied_to_mixed_languages() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", CONFIG),
        ("a.json", "{}"),
        ("b.lua", "local x=1"),
    ]);
    let output = dotfmt(root.path()).args(["--dialect", "luau", "."]).run();
    assert!(!output.success(), "{output:?}");
    assert!(
        output.stderr.contains("one selected language"),
        "{output:?}"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("b.lua")).unwrap(),
        "local x=1"
    );
}

#[test]
fn repeatable_and_comma_delimited_languages_select_multiple_engines() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", CONFIG),
        ("a.json", "{\"a\":1}"),
        ("b.lua", "local x=1"),
        ("c.md", "hello\n"),
    ]);
    let output = dotfmt(root.path())
        .args(["-l", "json,lua", "--lang", "json", "."])
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("b.lua")).unwrap(),
        "local x = 1"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("c.md")).unwrap(),
        "hello\n"
    );
}

#[test]
fn explicit_files_within_a_directory_keep_their_forced_language() {
    let root = tree_pairs(&[("dotfmt.dotfile", "json {}\n"), ("body", "{\"a\":1}")]);
    let output = dotfmt(root.path()).args(["-l", "json", ".", "body"]).run();
    assert!(output.success(), "{output:?}");
    assert!(
        fs::read_to_string(root.path().join("body"))
            .unwrap()
            .contains("\"a\": 1")
    );
}

#[test]
fn language_filter_does_not_silently_succeed_for_an_unconfigured_directory() {
    let root = tree_pairs(&[("dotfmt.dotfile", "lua {}\n"), ("a.json", "{}")]);
    let output = dotfmt(root.path()).args(["-l", "json", "."]).run();
    assert!(!output.success(), "{output:?}");
    assert!(
        output.stderr.contains("json is not configured"),
        "{output:?}"
    );
}

#[test]
fn directory_language_filter_resolves_custom_conflicts_without_claiming_other_files() {
    let root = tree_pairs(&[
        (
            "dotfmt.dotfile",
            "conf { include { *.custom } }\nlua { include { *.custom } }\n",
        ),
        ("a.custom", "x=1  \n"),
        ("a.lua", "local x=1"),
        ("a.unknown", "x=1  \n"),
    ]);
    let output = dotfmt(root.path()).args(["-l", "conf", "."]).run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("a.custom")).unwrap(),
        "x=1"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("a.lua")).unwrap(),
        "local x=1"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("a.unknown")).unwrap(),
        "x=1  \n"
    );
    let owned = dotfmt(root.path())
        .args(["--owns", "-l", "conf"])
        .stdin("a.custom\0a.lua\0a.unknown\0")
        .run();
    assert!(owned.success(), "{owned:?}");
    assert_eq!(owned.stdout, "a.custom\0");
}

#[test]
fn relative_nested_exclusions_apply_to_files_stdin_and_ownership() {
    let root = tree_pairs(&[
        (
            "nested/dotfmt.dotfile",
            "json {}\nexcluded_files { /a.json }\n",
        ),
        ("nested/a.json", "{\"x\":1}"),
    ]);
    let original = "{\"x\":1}";
    let output = dotfmt(root.path())
        .args(["-l", "json", "nested/a.json"])
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("nested/a.json")).unwrap(),
        original
    );
    let output = dotfmt(root.path())
        .args(["--stdin", "nested/a.json"])
        .stdin(original)
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(output.stdout, original);
    let output = dotfmt(root.path())
        .arg("--owns")
        .stdin("nested/a.json\0")
        .run();
    assert!(output.success(), "{output:?}");
    assert!(output.stdout.is_empty());
}

#[test]
fn multiple_requested_languages_use_each_explicit_files_local_configuration() {
    let root = tree_pairs(&[
        ("json/dotfmt.dotfile", "json {}\n"),
        ("json/a.json", "{\"a\":1}"),
        ("lua/dotfmt.dotfile", "lua {}\n"),
        ("lua/b.lua", "local x=1"),
    ]);
    let output = dotfmt(root.path())
        .args(["-l", "json,lua", "json/a.json", "lua/b.lua"])
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("json/a.json")).unwrap(),
        "{\n  \"a\": 1\n}\n"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("lua/b.lua")).unwrap(),
        "local x = 1"
    );
    let output = dotfmt(root.path())
        .args(["-l", "json,lua", "--stdin", "json/a.json"])
        .stdin("{\"a\":1}")
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(output.stdout, "{\n  \"a\": 1\n}\n");
}

#[test]
fn a_selected_dialect_does_not_apply_to_other_enabled_languages() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", "conf {}\njson {}\nlua {}\nmarkdown {}\n"),
        ("a.lua", "local x: number=1"),
    ]);
    let output = dotfmt(root.path())
        .args(["--dialect", "luau", "a.lua"])
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("a.lua")).unwrap(),
        "local x: number = 1"
    );
}
