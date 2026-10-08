#![forbid(unsafe_code)]

mod common;

use common::dotfmt;
use std::fs;
use testkit::tree_pairs;

#[test]
fn markdown_table_autosizing_can_be_enabled_and_overridden_locally() {
    let input = "| Name | Value |\n| --- | --- |\n| long name | long value |";
    let root = tree_pairs(&[
        ("dotfmt.dotfile", "markdown { autosize_table = true }\n"),
        (
            "sub/dotfmt.dotfile",
            "markdown { autosize_table = false }\n",
        ),
        ("a.md", input),
        ("sub/b.md", input),
    ]);
    let output = dotfmt(root.path()).arg(".").run();
    assert!(output.success(), "{output:?}");
    for (path, divider) in [
        ("a.md", "| --------- | ---------- |"),
        ("sub/b.md", "| --- | --- |"),
    ] {
        let formatted = fs::read_to_string(root.path().join(path)).unwrap();
        assert_eq!(formatted.lines().nth(1).unwrap(), divider);
    }
}

#[test]
fn nearer_global_values_override_parent_language_values_and_local_language_wins() {
    let root = tree_pairs(&[
        (
            "dotfmt.dotfile",
            "{\n indent = 2\n}\njson {\n indent = 4\n}\nlua {}\n",
        ),
        (
            "sub/dotfmt.dotfile",
            "{\n indent = 3\n final_newline = false\n}\nlua {\n enabled = false\n}\n",
        ),
        ("sub/deeper/dotfmt.dotfile", "json {\n indent = 1\n}\n"),
        ("sub/a.json", "{\"a\":1}"),
        ("sub/deeper/b.json", "{\"b\":2}"),
        ("sub/skip.lua", "local x=1"),
    ]);
    let output = dotfmt(root.path()).arg(".").run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("sub/a.json")).unwrap(),
        "{\n   \"a\": 1\n}"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("sub/deeper/b.json")).unwrap(),
        "{\n \"b\": 2\n}"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("sub/skip.lua")).unwrap(),
        "local x=1"
    );
}

#[test]
fn missing_or_disabled_language_blocks_are_actionable_errors() {
    let root = tree_pairs(&[("a.json", "{}")]);
    let output = dotfmt(root.path()).args(["-l", "json", "a.json"]).run();
    assert_eq!(output.code(), Some(1), "{output:?}");
    assert!(output.stderr.contains("not configured"), "{output:?}");
    assert!(output.stderr.contains("--add"), "{output:?}");
    fs::write(
        root.path().join("dotfmt.dotfile"),
        "json {\n enabled = false\n}\n",
    )
    .unwrap();
    assert!(
        !dotfmt(root.path())
            .args(["-l", "json", "a.json"])
            .run()
            .success()
    );
}

#[test]
fn invalid_config_is_reported_before_any_files_are_written() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", "json {}\nlua {}\n"),
        ("a.json", "{\"x\":1}"),
        ("sub/dotfmt.dotfile", "lua {\n indent = invalid\n}\n"),
        ("sub/b.lua", "local x=1"),
    ]);
    let output = dotfmt(root.path()).arg(".").run();
    assert_eq!(output.code(), Some(1), "{output:?}");
    assert!(output.stderr.contains("dotfmt.dotfile"), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("a.json")).unwrap(),
        "{\"x\":1}"
    );
}

#[test]
fn global_configuration_is_layered_below_local_configuration() {
    let root = tree_pairs(&[
        (".config/dotfmt/dotfmt.dotfile", "json {\n indent = 4\n}\n"),
        (
            "project/dotfmt.dotfile",
            "{\n indent = 1\n final_newline = false\n}\n",
        ),
        ("project/a.json", "{\"x\":1}"),
    ]);
    let output = dotfmt(root.path()).arg("project").run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("project/a.json")).unwrap(),
        "{\n \"x\": 1\n}"
    );
}

#[test]
fn invalid_unselected_language_settings_prevent_all_file_writes() {
    for setting in ["indent = bad", "unknown = true"] {
        let config = format!("json {{}}\nlua {{ {setting} }}\n");
        let root = tree_pairs(&[("dotfmt.dotfile", &config), ("a.json", "{\"x\":1}")]);
        for args in [vec!["."], vec!["a.json"], vec!["-l", "json", "."]] {
            let output = dotfmt(root.path()).args(args).run();
            assert!(!output.success(), "{setting}: {output:?}");
            assert!(output.stderr.contains("dotfmt.dotfile:2:"), "{output:?}");
            assert_eq!(
                fs::read_to_string(root.path().join("a.json")).unwrap(),
                "{\"x\":1}"
            );
        }
    }
}

#[test]
fn invalid_settings_are_reported_even_when_all_files_are_excluded_or_absent() {
    let root = tree_pairs(&[
        (
            "dotfmt.dotfile",
            "json { indent = bad }\nexcluded_files { * }\n",
        ),
        ("a.json", "{\"x\":1}"),
    ]);
    fs::create_dir(root.path().join("empty")).unwrap();
    for args in [vec!["."], vec!["a.json"], vec!["empty"]] {
        let output = dotfmt(root.path()).args(args).run();
        assert!(!output.success(), "{output:?}");
        assert!(output.stderr.contains("indent"), "{output:?}");
    }
    let output = dotfmt(root.path())
        .args(["--stdin", "a.json"])
        .stdin("{\"x\":1}")
        .run();
    assert!(!output.success(), "{output:?}");
    assert!(output.stdout.is_empty());
    let output = dotfmt(root.path()).arg("--owns").stdin("a.json\0").run();
    assert!(!output.success(), "{output:?}");
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read_to_string(root.path().join("a.json")).unwrap(),
        "{\"x\":1}"
    );
}

#[test]
fn stdin_and_ownership_validate_unselected_enabled_languages() {
    let root = tree_pairs(&[("dotfmt.dotfile", "json {}\nlua { indent = bad }\n")]);
    let output = dotfmt(root.path())
        .args(["--stdin", "a.json"])
        .stdin("{}")
        .run();
    assert!(!output.success(), "{output:?}");
    assert!(output.stdout.is_empty());
    let output = dotfmt(root.path())
        .args(["--owns", "-l", "json"])
        .stdin("a.json\0")
        .run();
    assert!(!output.success(), "{output:?}");
    assert!(output.stdout.is_empty());
}

#[test]
fn disabled_settings_remain_dormant_and_effective_overrides_replace_invalid_values() {
    let root = tree_pairs(&[
        (
            "dotfmt.dotfile",
            "json { indent = bad }\nlua {\n enabled = false\n unknown = true\n}\n",
        ),
        ("sub/dotfmt.dotfile", "json { indent = 2 }\n"),
        ("sub/a.json", "{\"x\":1}"),
    ]);
    let output = dotfmt(root.path()).arg("sub").run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("sub/a.json")).unwrap(),
        "{\n  \"x\": 1\n}\n"
    );
}
