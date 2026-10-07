#![forbid(unsafe_code)]

use std::fs;
use std::path::Path;

use testkit::{Bin, tree_pairs};

fn dotfmt(root: &Path) -> Bin {
    Bin::new(env!("CARGO_BIN_EXE_dotfmt"))
        .plain()
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join(".config"))
        .current_dir(root)
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
fn sibling_directory_target_spelled_with_parent_components_is_not_skipped() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", "json {}\n"),
        ("a/first.json", "{\"first\":1}"),
        ("b/second.json", "{\"second\":2}"),
    ]);
    let output = dotfmt(root.path()).args(["a", "a/../b"]).run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("a/first.json")).unwrap(),
        "{\n  \"first\": 1\n}\n"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("b/second.json")).unwrap(),
        "{\n  \"second\": 2\n}\n"
    );
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
