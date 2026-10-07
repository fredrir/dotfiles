#![forbid(unsafe_code)]

use std::fs;
use std::path::Path;

use testkit::{Bin, tree_pairs};

const CONFIG: &str =
    "{\n indent = 2\n final_newline = false\n}\njson {}\nlua {}\nmarkdown {}\nconf {}\n";

fn dotfmt(root: &Path) -> Bin {
    Bin::new(env!("CARGO_BIN_EXE_dotfmt"))
        .plain()
        .env("PATH", "")
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join(".config"))
        .env("DOTFILE_ROOT", "/no/checkout/needed")
        .current_dir(root)
}

#[test]
fn help_completions_and_metadata_describe_the_unified_cli() {
    let root = tree_pairs(&[]);
    let output = dotfmt(root.path()).arg("--help").run();
    assert!(output.success(), "{output:?}");
    for flag in [
        "--check",
        "--add",
        "--sync",
        "--dialect",
        "--lang",
        "--editor",
        "--stdin",
        "--owns",
        "--verbose",
        "--quiet",
        "--completions",
    ] {
        assert!(output.stdout.contains(flag), "missing {flag}: {output:?}");
    }
    assert!(!output.stdout.contains("placeholder"));
    let output = dotfmt(root.path()).args(["--completions", "zsh"]).run();
    assert!(output.success(), "{output:?}");
    assert!(output.stdout.contains("#compdef dotfmt"));
    assert!(output.stdout.contains("--lang"));
    let output = dotfmt(root.path()).arg("--command-dump").run();
    assert!(output.success(), "{output:?}");
    assert!(output.stdout.contains("\"--lang\""));
    assert!(
        output
            .stdout
            .contains("\"choices\":[\"conf\",\"json\",\"lua\",\"md\",\"markdown\"]"),
        "{output:?}"
    );
}

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
fn add_prompts_and_sync_only_updates_an_existing_unified_config() {
    let root = tree_pairs(&[]);
    let output = dotfmt(root.path()).arg("--sync").run();
    assert!(!output.success(), "{output:?}");
    assert!(!root.path().join("dotfmt.dotfile").exists());
    let output = dotfmt(root.path()).arg("--add").stdin("n\n").run();
    assert!(output.success(), "{output:?}");
    assert!(!root.path().join("dotfmt.dotfile").exists());
    let output = dotfmt(root.path()).arg("--add").stdin("y\n").run();
    assert!(output.success(), "{output:?}");
    let bundled = fs::read_to_string(root.path().join("dotfmt.dotfile")).unwrap();
    for language in ["conf {", "markdown {", "json {", "lua {"] {
        assert!(bundled.contains(language));
    }
    fs::write(root.path().join("dotfmt.dotfile"), "json {}\n").unwrap();
    let output = dotfmt(root.path()).arg("--sync").run();
    assert!(output.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("dotfmt.dotfile")).unwrap(),
        bundled
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
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
fn verbose_output_keeps_heading_and_language_rows() {
    let root = tree_pairs(&[("dotfmt.dotfile", "json {}\n"), ("a.json", "{}\n")]);
    let output = dotfmt(root.path()).args(["-v", "--check", "."]).run();
    assert!(output.success(), "{output:?}");
    assert!(
        output.stderr.starts_with("\n  dotfmt  .  check\n\n"),
        "{output:?}"
    );
    assert!(output.stderr.contains("json"));
    assert!(output.stderr.contains("1 file checked"));
}

#[test]
fn conflicting_flags_remain_usage_errors() {
    let root = tree_pairs(&[]);
    for args in [
        ["--check", "--add"],
        ["--check", "--sync"],
        ["--add", "--sync"],
        ["--verbose", "--quiet"],
    ] {
        assert_eq!(dotfmt(root.path()).args(args).run().code(), Some(2));
    }
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
