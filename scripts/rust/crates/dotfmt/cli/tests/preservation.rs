#![forbid(unsafe_code)]

mod common;

use common::dotfmt;
use std::fs;
use std::path::Path;
use testkit::tree_pairs;

#[test]
fn lua_bom_and_backticks_in_literals_are_preserved() {
    let root = tree_pairs(&[("dotfmt.dotfile", "lua {}\n")]);
    let output = dotfmt(root.path())
        .args(["-l", "lua"])
        .stdin("\u{feff}local x='`' -- `\n")
        .run();
    assert!(output.success(), "{output:?}");
    assert!(output.stdout.starts_with('\u{feff}'));
    assert!(output.stdout.contains('`'));
    fs::write(root.path().join("a.lua"), "\u{feff}local x=1").unwrap();
    let output = dotfmt(root.path()).arg("a.lua").run();
    assert!(output.success(), "{output:?}");
    assert!(
        fs::read_to_string(root.path().join("a.lua"))
            .unwrap()
            .starts_with('\u{feff}')
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
    use dotfmt_core::config::Resolver;
    use dotfmt_core::language::Language;
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
    use dotfmt_core::config::Resolver;
    use dotfmt_core::language::Language;
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
