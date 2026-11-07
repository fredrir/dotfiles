#![forbid(unsafe_code)]

mod common;

use common::dotfmt;
use std::fs;
use testkit::tree_pairs;

fn tree() -> tempfile::TempDir {
    tree_pairs(&[
        ("bad.json", "{\"a\":}\n"),
        ("bad.yaml", "a: [1,\n"),
        ("bad.toml", "a = [1\n"),
        ("bad.lua", "local x = {\n"),
        ("good.json", "{\"a\": 1}\n"),
        ("good.yaml", "a: 1\n"),
        ("good.toml", "a = 1\n"),
        ("good.lua", "local x = 1\n"),
        ("repairable.json", "{\"a\": 1,}\n"),
    ])
}

#[test]
fn diagnose_reports_malformed_non_script_files() {
    let root = tree();
    for (name, expected) in [
        ("bad.json", "bad.json:1:6: unexpected }"),
        ("bad.yaml", "bad.yaml:2:1"),
        ("bad.toml", "bad.toml:2:1"),
        ("bad.lua", "bad.lua:1:11"),
    ] {
        let output = dotfmt(root.path()).args(["--diagnose", name]).run();
        assert_eq!(output.code(), Some(1), "{name}: {output:?}");
        assert!(output.stderr.contains(expected), "{name}: {output:?}");
        assert!(output.stdout.is_empty(), "{name}: {output:?}");
    }

    let output = dotfmt(root.path())
        .args(["--diagnose", "--json", "bad.json"])
        .run();
    assert_eq!(output.code(), Some(1), "{output:?}");
    let value: serde_json::Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(value["files"][0]["path"], "bad.json");
    assert_eq!(value["files"][0]["language"], "json");
    assert_eq!(value["files"][0]["error"], "bad.json:1:6: unexpected }");
}

#[test]
fn diagnose_accepts_valid_and_repairable_files() {
    let root = tree();
    for name in [
        "good.json",
        "good.yaml",
        "good.toml",
        "good.lua",
        "repairable.json",
    ] {
        let output = dotfmt(root.path()).args(["--diagnose", name]).run();
        assert!(output.success(), "{name}: {output:?}");
        assert!(output.stdout.is_empty(), "{name}: {output:?}");
        assert!(output.stderr.is_empty(), "{name}: {output:?}");
    }
    assert_eq!(
        fs::read_to_string(root.path().join("repairable.json")).unwrap(),
        "{\"a\": 1,}\n"
    );
}

#[test]
fn fix_still_reports_malformed_non_script_files() {
    let root = tree();
    let output = dotfmt(root.path()).args(["--fix", "bad.lua"]).run();
    assert_eq!(output.code(), Some(1), "{output:?}");
    assert!(output.stderr.contains("bad.lua:1:11"), "{output:?}");
}
