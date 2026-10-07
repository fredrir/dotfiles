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

#[test]
fn formatting_bare_braces_preserves_the_dotfile_readers_entries() {
    let original =
        "packages {\nlib{foo,bar}\n${HOME}/foo\nlib{}\njson{foo,bar}\nlib{ foo,bar}\n}\n";
    let root = tree_pairs(&[
        ("dotfmt.dotfile", "conf {}\n"),
        ("packages.dotfile", original),
    ]);
    let output = dotfmt(root.path()).arg("packages.dotfile").run();
    assert!(output.success(), "{output:?}");
    let formatted = fs::read_to_string(root.path().join("packages.dotfile")).unwrap();
    assert_eq!(
        workstation::blocks::parse(&formatted).unwrap(),
        workstation::blocks::parse(original).unwrap()
    );
    assert_eq!(
        formatted,
        "packages {\n  lib{foo,bar}\n  ${HOME}/foo\n  lib{}\n  json{foo,bar}\n  lib{ foo,bar}\n}"
    );
}

#[test]
fn self_formatting_compact_configuration_preserves_effective_settings() {
    use dotfmt_core::config::{Language, Resolver};
    let original = "conf{}\njson{indent=4}\nlua{include{ *.custom }}\nmarkdown{width=80\n}\n";
    let root = tree_pairs(&[("dotfmt.dotfile", original)]);
    let before = Resolver::with_paths(root.path().to_path_buf(), None)
        .for_directory(root.path())
        .unwrap();
    let output = dotfmt(root.path()).arg("dotfmt.dotfile").run();
    assert!(output.success(), "{output:?}");
    let after = Resolver::with_paths(root.path().to_path_buf(), None)
        .for_directory(root.path())
        .unwrap();
    for language in Language::ALL {
        assert_eq!(
            before.languages[&language].enabled,
            after.languages[&language].enabled
        );
        let values = |config: &dotfmt_core::config::Effective| {
            config
                .settings(language)
                .iter()
                .map(|(key, value)| (key.clone(), value.value.clone()))
                .collect::<std::collections::BTreeMap<_, _>>()
        };
        assert_eq!(values(&before), values(&after));
    }
    for path in ["a.custom", "a.json", "dotfmt.dotfile", "a.lua"] {
        assert_eq!(
            before.select(Path::new(path), None).unwrap(),
            after.select(Path::new(path), None).unwrap()
        );
    }
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

#[test]
fn top_level_compact_brace_tokens_remain_literal() {
    let original = "json{foo,bar}\nlua{width=120}\ninclude{foo,bar}\n";
    let root = tree_pairs(&[
        ("dotfmt.dotfile", "conf {}\n"),
        ("ordinary.dotfile", original),
    ]);
    let output = dotfmt(root.path()).arg("ordinary.dotfile").run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("ordinary.dotfile")).unwrap(),
        original.trim_end()
    );
}

#[test]
fn compact_configuration_preserves_escaped_pattern_spaces_and_ownership() {
    use dotfmt_core::config::{Language, Resolver};
    for original in [
        "conf{}\njson{indent=4}\nlua{include {\n name\\ \n}\n}\n",
        "conf{}\njson{indent=4}\nlua{include { name\\ \n}\n}\n",
    ] {
        let root = tree_pairs(&[("dotfmt.dotfile", original)]);
        let before = Resolver::with_paths(root.path().to_path_buf(), None)
            .for_directory(root.path())
            .unwrap();
        let owned_before = dotfmt(root.path())
            .arg("--owns")
            .stdin("name \0name\0")
            .run();
        assert!(owned_before.success(), "{owned_before:?}");
        assert_eq!(owned_before.stdout, "name \0");
        let output = dotfmt(root.path()).arg("dotfmt.dotfile").run();
        assert!(output.success(), "{output:?}");
        let formatted = fs::read_to_string(root.path().join("dotfmt.dotfile")).unwrap();
        assert!(formatted.contains("name\\ \n"), "{formatted:?}");
        let after = Resolver::with_paths(root.path().to_path_buf(), None)
            .for_directory(root.path())
            .unwrap();
        for language in Language::ALL {
            assert_eq!(
                before.languages[&language].enabled,
                after.languages[&language].enabled
            );
            let values = |config: &dotfmt_core::config::Effective| {
                config
                    .settings(language)
                    .iter()
                    .map(|(key, setting)| (key.clone(), setting.value.clone()))
                    .collect::<std::collections::BTreeMap<_, _>>()
            };
            assert_eq!(values(&before), values(&after));
        }
        let owned_after = dotfmt(root.path())
            .arg("--owns")
            .stdin("name \0name\0")
            .run();
        assert!(owned_after.success(), "{owned_after:?}");
        assert_eq!(owned_after.stdout, owned_before.stdout);
        let check = dotfmt(root.path())
            .args(["--check", "dotfmt.dotfile"])
            .run();
        assert!(check.success(), "{check:?}");
    }
}
