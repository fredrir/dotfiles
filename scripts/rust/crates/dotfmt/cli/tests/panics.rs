#![forbid(unsafe_code)]

mod common;

use common::dotfmt;
use std::fs;
use testkit::{Ran, tree_pairs};

// An unterminated long comment made full_moon panic. The fixtures stay valid
// for the failure case even if that parser bug is fixed upstream.
const BAD_LUA: &str = "#!/usr/bin/env lua\n--[==[\n";
const GOOD_JSON: &str = "{\"a\":1}";
const FORMATTED_JSON: &str = "{\n  \"a\": 1\n}\n";

fn tree() -> tempfile::TempDir {
    tree_pairs(&[("a.json", GOOD_JSON), ("bad.lua", BAD_LUA)])
}

/// A panic that escapes containment aborts with 101, so a documented status is
/// enough to prove the job panic was converted into a diagnostic.
fn failed_without_aborting(output: &Ran, arguments: &[&str]) -> bool {
    let code = output.code();
    assert_ne!(code, Some(101), "{arguments:?} aborted: {output:?}");
    assert!(
        matches!(code, Some(0) | Some(1)),
        "{arguments:?}: {output:?}"
    );
    code == Some(1)
}

#[test]
fn a_panicking_engine_fails_one_file_without_aborting_the_directory() {
    let root = tree();

    let output = dotfmt(root.path()).arg(".").run();

    assert!(failed_without_aborting(&output, &["."]), "{output:?}");
    assert!(output.stderr.contains("bad.lua"), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("a.json")).unwrap(),
        FORMATTED_JSON
    );
}

#[test]
fn a_panicking_engine_survives_check_fix_and_diagnose() {
    for arguments in [["--check", "."], ["--diagnose", "."], ["--fix", "."]] {
        let root = tree();

        let output = dotfmt(root.path()).args(arguments).run();

        assert!(failed_without_aborting(&output, &arguments), "{output:?}");
        assert!(output.stderr.contains("bad.lua"), "{output:?}");
        let expected = if arguments[0] == "--fix" {
            FORMATTED_JSON
        } else {
            GOOD_JSON
        };
        assert_eq!(
            fs::read_to_string(root.path().join("a.json")).unwrap(),
            expected,
            "{arguments:?}"
        );
    }
}

#[test]
fn a_panicking_engine_survives_the_stdin_buffer() {
    let root = tree();

    let output = dotfmt(root.path())
        .args(["--stdin", "bad.lua"])
        .stdin(BAD_LUA)
        .run();

    assert!(failed_without_aborting(&output, &["--stdin"]), "{output:?}");
    assert!(output.stderr.contains("bad.lua"), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
}

#[test]
fn a_panicking_buffer_keeps_editor_input_intact() {
    let root = tree();

    let output = dotfmt(root.path())
        .args(["--editor", "bad.lua"])
        .stdin(BAD_LUA)
        .run();

    let failed = failed_without_aborting(&output, &["--editor"]);
    assert_eq!(output.stdout, BAD_LUA, "{output:?}");
    if failed {
        assert!(output.stderr.contains("bad.lua"), "{output:?}");
    }
}
