#![forbid(unsafe_code)]

mod common;

use common::dotfmt;
use std::fs;
#[cfg(unix)]
use std::path::PathBuf;
use testkit::tree_pairs;

const CONFIG: &str = "json {}\n";

#[test]
fn bun_dependency_caches_are_not_discovered_or_formatted() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", CONFIG),
        (".bun/install/cache/pkg/package.json", "{\"bun\":1}"),
        (".bun-cache/pkg/package.json", "{\"cache\":1}"),
        ("keep.json", "{\n  \"keep\": 1\n}\n"),
    ]);

    let output = dotfmt(root.path()).args(["--check", "."]).run();
    assert!(output.success(), "{output:?}");

    let output = dotfmt(root.path()).arg(".").run();
    assert!(output.success(), "{output:?}");
    for path in [
        ".bun/install/cache/pkg/package.json",
        ".bun-cache/pkg/package.json",
    ] {
        assert_eq!(
            fs::read_to_string(root.path().join(path)).unwrap(),
            if path.starts_with(".bun/") {
                "{\"bun\":1}"
            } else {
                "{\"cache\":1}"
            },
            "{path}"
        );
    }
}

#[test]
fn owns_matches_discovery_for_skipped_directories_and_prettier_ignore() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", CONFIG),
        (".prettierignore", "ignored/\nsrc/generated/\n"),
        ("keep.json", "{}\n"),
        ("ignored/skip.json", "{}\n"),
        ("node_modules/pkg/package.json", "{}"),
        ("dist/out.json", "{}"),
        (".bun/pkg/package.json", "{}"),
        (".bun-cache/pkg/package.json", "{}"),
        ("src/keep.json", "{}\n"),
        ("src/generated/g.json", "{}"),
    ]);
    let controlled = [
        "keep.json",
        "ignored/skip.json",
        "node_modules/pkg/package.json",
        "dist/out.json",
        ".bun/pkg/package.json",
        ".bun-cache/pkg/package.json",
        "src/generated/g.json",
        "src/keep.json",
    ];
    let mut input = String::new();
    for path in controlled {
        input.push_str(path);
        input.push('\0');
    }

    let output = dotfmt(root.path()).arg("--owns").stdin(&input).run();
    assert!(output.success(), "{output:?}");
    assert_eq!(output.stdout, "keep.json\0src/keep.json\0", "{output:?}");
}

#[test]
fn prettierignore_directory_pattern_applies_to_check_explain_and_owns() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", CONFIG),
        (".prettierignore", "src/generated/\n"),
        ("src/generated/g.json", "{\"generated\":1}"),
        ("src/keep.json", "{\n  \"keep\": 1\n}\n"),
    ]);

    let output = dotfmt(root.path()).args(["--check", "."]).run();
    assert!(output.success(), "{output:?}");

    let output = dotfmt(root.path())
        .args(["--explain", "src/generated/g.json"])
        .run();
    assert!(output.success(), "{output:?}");
    assert!(output.stdout.contains("skipped  excluded"), "{output:?}");

    let output = dotfmt(root.path())
        .arg("--owns")
        .stdin("src/generated/g.json\0src/keep.json\0")
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(output.stdout, "src/keep.json\0", "{output:?}");

    let output = dotfmt(root.path()).arg(".").run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("src/generated/g.json")).unwrap(),
        "{\"generated\":1}"
    );
}

#[test]
fn a_nested_prettierignore_only_applies_to_its_own_directory() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", CONFIG),
        ("nested/.prettierignore", "generated/\n"),
        ("nested/generated/g.json", "{\"generated\":1}"),
        ("nested/keep.json", "{\n  \"keep\": 1\n}\n"),
        ("keep.json", "{\n  \"keep\": 1\n}\n"),
    ]);

    let output = dotfmt(root.path()).args(["--check", "."]).run();
    assert!(output.success(), "{output:?}");

    let output = dotfmt(root.path())
        .arg("--owns")
        .stdin("nested/generated/g.json\0nested/keep.json\0keep.json\0")
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(output.stdout, "nested/keep.json\0keep.json\0", "{output:?}");
}

#[cfg(unix)]
#[test]
fn owns_accepts_a_path_spelled_through_a_symlinked_cwd() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("build").join("checkout");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("dotfmt.dotfile"), CONFIG).unwrap();
    fs::write(root.join("a.json"), "{\"a\":1}").unwrap();
    let alias = temp.path().join("alias");
    std::os::unix::fs::symlink(&root, &alias).unwrap();

    let paths = [
        PathBuf::from("a.json"),
        alias.join("a.json"),
        root.join("a.json"),
    ];
    let input = paths
        .iter()
        .map(|path| format!("{}\0", path.display()))
        .collect::<String>();

    let output = dotfmt(&root)
        .current_dir(&alias)
        .arg("--owns")
        .stdin(&input)
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(output.stdout, input, "{output:?}");
}

#[test]
fn explicit_targets_still_format_inside_skipped_directories() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", CONFIG),
        (".bun-cache/pkg/package.json", "{\"cache\":1}"),
    ]);

    let output = dotfmt(root.path()).arg(".bun-cache/pkg/package.json").run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join(".bun-cache/pkg/package.json")).unwrap(),
        "{\n  \"cache\": 1\n}\n"
    );
}
