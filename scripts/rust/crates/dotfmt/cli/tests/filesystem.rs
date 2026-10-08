#![forbid(unsafe_code)]

mod common;

use common::dotfmt;
use std::fs;
use testkit::tree_pairs;

const CONFIG: &str =
    "{\n indent = 2\n final_newline = false\n}\njson {}\nlua {}\nmarkdown {}\nconf {}\n";

#[test]
fn one_run_formats_all_languages_without_external_executables() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", CONFIG),
        ("data.json", "{\"a\":1}"),
        ("init.lua", "local x=1"),
        ("note.md", "#   Title\n"),
        ("app.conf", "a=1\n"),
    ]);
    let output = dotfmt(root.path()).arg(".").run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("data.json")).unwrap(),
        "{\n  \"a\": 1\n}"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("init.lua")).unwrap(),
        "local x = 1"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("note.md")).unwrap(),
        "#   Title"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("app.conf")).unwrap(),
        "a=1"
    );
    let output = dotfmt(root.path()).args(["--check", "."]).run();
    assert!(output.success(), "{output:?}");
}

#[test]
fn check_does_not_write_and_language_filter_does_not_touch_other_engines() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", CONFIG),
        ("a.json", "{\"a\":1}"),
        ("b.lua", "local x=1"),
    ]);
    let path = root.path().join("a.json");
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    let output = dotfmt(root.path())
        .args(["--check", "-l", "json", "."])
        .run();
    assert_eq!(output.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty());
    assert_eq!(fs::read_to_string(&path).unwrap(), "{\"a\":1}");
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
    let output = dotfmt(root.path()).args(["-l", "json", "."]).run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("b.lua")).unwrap(),
        "local x=1"
    );
    let unchanged = fs::metadata(&path).unwrap().modified().unwrap();
    assert!(
        dotfmt(root.path())
            .args(["-l", "json", "."])
            .run()
            .success()
    );
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), unchanged);
}

#[cfg(unix)]
#[test]
fn symlink_targets_keep_their_links_and_use_configuration_beside_the_link() {
    use std::os::unix::fs::symlink;
    let root = tree_pairs(&[
        ("dotfmt.dotfile", "json {}\n"),
        ("real/a.json", "{\"x\":1}"),
        (
            "links/dotfmt.dotfile",
            "json {\n indent = 4\n final_newline = false\n}\n",
        ),
    ]);
    symlink(
        root.path().join("real/a.json"),
        root.path().join("links/a.json"),
    )
    .unwrap();
    let output = dotfmt(root.path()).arg("links/a.json").run();
    assert!(output.success(), "{output:?}");
    assert!(
        fs::symlink_metadata(root.path().join("links/a.json"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_to_string(root.path().join("real/a.json")).unwrap(),
        "{\n    \"x\": 1\n}"
    );
}

#[test]
fn invalid_lua_does_not_overwrite_it_and_other_files_still_format() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", "lua {}\n"),
        ("a.lua", "local x=`hi`"),
        ("b.lua", "local x=1"),
    ]);
    let output = dotfmt(root.path()).arg(".").run();
    assert!(!output.success(), "{output:?}");
    assert!(output.stderr.contains("backtick"), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("a.lua")).unwrap(),
        "local x=`hi`"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("b.lua")).unwrap(),
        "local x = 1"
    );
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
fn explicitly_named_skipped_directories_are_walked_beside_their_ancestor() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", "json {}\n"),
        ("a.json", "{\"x\":1}"),
        ("vendor/a.json", "{\"x\":1}"),
        ("target/sub/a.json", "{\"x\":1}"),
    ]);
    let output = dotfmt(root.path())
        .args(["-v", ".", "vendor", "vendor", "target/sub"])
        .run();
    assert!(output.success(), "{output:?}");
    assert!(output.stderr.contains("3 files formatted"), "{output:?}");
    for file in ["a.json", "vendor/a.json", "target/sub/a.json"] {
        assert_eq!(
            fs::read_to_string(root.path().join(file)).unwrap(),
            "{\n  \"x\": 1\n}\n"
        );
    }
}

#[cfg(unix)]
#[test]
fn explicitly_named_symlink_directories_are_walked_beside_their_ancestor() {
    let root = tree_pairs(&[("dotfmt.dotfile", "json {}\n"), ("a.json", "{\"x\":1}")]);
    let external = tree_pairs(&[("a.json", "{\"x\":1}")]);
    std::os::unix::fs::symlink(external.path(), root.path().join("linked")).unwrap();
    let output = dotfmt(root.path())
        .args(["-v", ".", "linked", "linked"])
        .run();
    assert!(output.success(), "{output:?}");
    assert!(output.stderr.contains("2 files formatted"), "{output:?}");
    assert_eq!(
        fs::read_to_string(external.path().join("a.json")).unwrap(),
        "{\n  \"x\": 1\n}\n"
    );
}

#[cfg(unix)]
#[test]
fn readable_explicit_descendant_is_walked_when_ancestor_walk_is_incomplete() {
    use std::os::unix::fs::PermissionsExt;
    let root = tree_pairs(&[
        ("dotfmt.dotfile", "json {}\n"),
        ("private/child/a.json", "{\"x\":1}"),
    ]);
    let private = root.path().join("private");
    let original_permissions = fs::metadata(&private).unwrap().permissions();
    fs::set_permissions(&private, fs::Permissions::from_mode(0o111)).unwrap();
    if fs::read_dir(&private).is_ok() {
        fs::set_permissions(&private, original_permissions).unwrap();
        return;
    }
    let output = dotfmt(root.path()).args([".", "private/child"]).run();
    fs::set_permissions(&private, original_permissions).unwrap();
    assert!(!output.success(), "{output:?}");
    assert!(output.stderr.contains("unreadable"), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("private/child/a.json")).unwrap(),
        "{\n  \"x\": 1\n}\n"
    );
}
