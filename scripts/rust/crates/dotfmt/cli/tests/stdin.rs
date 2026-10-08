#![forbid(unsafe_code)]

mod common;

use common::dotfmt;
use std::fs;
use testkit::tree_pairs;

const CONFIG: &str =
    "{\n indent = 2\n final_newline = false\n}\njson {}\nlua {}\nmarkdown {}\nconf {}\n";

#[test]
fn stdin_aliases_and_extensionless_explicit_languages_work() {
    let root = tree_pairs(&[("dotfmt.dotfile", CONFIG), ("body", "{\"x\":1}")]);
    for language in ["md", "markdown"] {
        let output = dotfmt(root.path())
            .args(["--editor", "-l", language])
            .stdin("#   Title\n")
            .run();
        assert!(output.success(), "{output:?}");
        assert_eq!(output.stdout, "#   Title");
        assert!(output.stderr.is_empty(), "{output:?}");
    }
    let output = dotfmt(root.path()).args(["-l", "json", "body"]).run();
    assert!(output.success(), "{output:?}");
    assert!(
        fs::read_to_string(root.path().join("body"))
            .unwrap()
            .contains("\"x\": 1")
    );
    let output = dotfmt(root.path())
        .args(["--stdin", "new.lua"])
        .stdin("local x=1")
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(output.stdout, "local x = 1");
    assert!(!root.path().join("new.lua").exists());
}

#[test]
fn editor_json_repairs_are_reported_and_invalid_lua_has_no_output() {
    let root = tree_pairs(&[("dotfmt.dotfile", CONFIG)]);
    let output = dotfmt(root.path())
        .args(["--editor", "--stdin", "a.json"])
        .stdin("{key:'value',}")
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(output.stdout, "{\n  \"key\": \"value\"\n}");
    assert!(output.stderr.contains("fixed"), "{output:?}");
    let output = dotfmt(root.path())
        .args(["--editor", "--stdin", "a.lua"])
        .stdin("local =")
        .run();
    assert!(!output.success(), "{output:?}");
    assert!(output.stdout.is_empty());
}

#[test]
fn dialect_is_inferred_from_files_and_cli_overrides_it() {
    let root = tree_pairs(&[("dotfmt.dotfile", CONFIG)]);
    let output = dotfmt(root.path())
        .args(["--stdin", "a.luau"])
        .stdin("local x: number=1")
        .run();
    assert!(output.success(), "{output:?}");
    assert!(output.stdout.contains("local x: number = 1"));
    let output = dotfmt(root.path())
        .args(["--stdin", "a.lua", "--dialect", "luau"])
        .stdin("local x: number=1")
        .run();
    assert!(output.success(), "{output:?}");
    let output = dotfmt(root.path())
        .args(["--stdin", "a.jsonc"])
        .stdin("// note\n{\"x\":1,}")
        .run();
    assert!(output.success(), "{output:?}");
    assert!(output.stdout.contains("// note"));
}

#[test]
fn deep_lua_runs_safely_in_stdin_single_file_and_parallel_modes() {
    let root = tree_pairs(&[("dotfmt.dotfile", "lua {}\n")]);
    let input = format!("local x = {}1{}", "{".repeat(150), "}".repeat(150));
    let output = dotfmt(root.path())
        .args(["-l", "lua", "-"])
        .stdin(&input)
        .run();
    assert!(output.success(), "{output:?}");
    fs::write(root.path().join("a.lua"), &input).unwrap();
    let output = dotfmt(root.path()).arg("a.lua").run();
    assert!(output.success(), "{output:?}");
    fs::write(root.path().join("b.lua"), &input).unwrap();
    let output = dotfmt(root.path())
        .env("RAYON_NUM_THREADS", "1")
        .args(["a.lua", "b.lua"])
        .run();
    assert!(output.success(), "{output:?}");
}

#[test]
fn streamed_and_file_targets_share_one_language_without_summary_noise() {
    let root = tree_pairs(&[("dotfmt.dotfile", CONFIG), ("a.json", "{\"a\":1}")]);
    let output = dotfmt(root.path())
        .args(["-l", "json", "-", "a.json"])
        .stdin("{\"b\":2}")
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(output.stdout, "{\n  \"b\": 2\n}");
    assert!(output.stderr.is_empty(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("a.json")).unwrap(),
        "{\n  \"a\": 1\n}"
    );
}

#[test]
fn configured_json_dialect_and_explicit_override_both_apply() {
    let root = tree_pairs(&[("dotfmt.dotfile", "json {\n dialect = jsonc\n}\n")]);
    let input = "// comment\n{\"a\":1}";
    let output = dotfmt(root.path()).args(["-l", "json"]).stdin(input).run();
    assert!(output.success(), "{output:?}");
    assert!(output.stdout.contains("// comment"));
    let output = dotfmt(root.path())
        .args(["-l", "json", "--dialect", "json"])
        .stdin(input)
        .run();
    assert!(!output.success(), "{output:?}");
    assert!(output.stdout.is_empty());
}

#[test]
fn stream_configuration_errors_prevent_writes_to_valid_file_targets() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", "json { indent = bad }\n"),
        ("sub/dotfmt.dotfile", "json { indent = 2 }\n"),
        ("sub/a.json", "{\"x\":1}"),
    ]);
    let output = dotfmt(root.path())
        .args(["-l", "json", "-", "sub/a.json"])
        .stdin("{}")
        .run();
    assert!(!output.success(), "{output:?}");
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read_to_string(root.path().join("sub/a.json")).unwrap(),
        "{\"x\":1}"
    );
}
