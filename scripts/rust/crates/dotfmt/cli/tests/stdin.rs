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
fn editor_repairs_are_quiet_unless_verbose() {
    let root = tree_pairs(&[("dotfmt.dotfile", CONFIG)]);
    let output = dotfmt(root.path())
        .args(["--editor", "--stdin", "a.json"])
        .stdin("{key:'value',}")
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(output.stdout, "{\n  \"key\": \"value\"\n}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let verbose = dotfmt(root.path())
        .args(["-e", "a.json", "--verbose"])
        .stdin("{key:'value',}")
        .run();
    assert!(verbose.success(), "{verbose:?}");
    assert_eq!(verbose.stdout, output.stdout);
    assert!(verbose.stderr.contains("fixed"), "{verbose:?}");
}

#[test]
fn editor_formatting_errors_succeed_silently_and_preserve_the_buffer() {
    let root = tree_pairs(&[("dotfmt.dotfile", CONFIG), ("a.lua", "disk contents")]);
    for args in [
        vec!["-e", "a.lua"],
        vec!["--editor", "--stdin", "a.lua"],
        vec!["-l", "lua", "-eq", "--stdin", "a.lua"],
    ] {
        let input = "\u{feff}-- unfinished buffer\r\nlocal =  \r\n";
        let output = dotfmt(root.path()).args(args).stdin(input).run();
        assert!(output.success(), "{output:?}");
        assert_eq!(output.stdout, input);
        assert!(output.stderr.is_empty(), "{output:?}");
    }
    assert_eq!(
        fs::read_to_string(root.path().join("a.lua")).unwrap(),
        "disk contents"
    );
}

#[test]
fn editor_configuration_errors_succeed_silently_and_preserve_the_buffer() {
    for config in ["", "lua {", "lua { indent = bad }\n"] {
        let root = tree_pairs(&[("dotfmt.dotfile", config)]);
        let input = "local x=1\r\n";
        let output = dotfmt(root.path())
            .args(["-l", "lua", "-eq", "--stdin", "a.lua"])
            .stdin(input)
            .run();
        assert!(output.success(), "{config}: {output:?}");
        assert_eq!(output.stdout, input);
        assert!(output.stderr.is_empty(), "{output:?}");
    }
}

#[test]
fn verbose_editor_check_and_non_editor_modes_still_report_errors() {
    for (config, input, diagnostic) in [
        (CONFIG, "local =", "a.lua"),
        ("lua { indent = bad }\n", "local x=1", "indent"),
    ] {
        let root = tree_pairs(&[("dotfmt.dotfile", config)]);
        for args in [
            vec!["-e", "a.lua", "--verbose"],
            vec!["-e", "a.lua", "--check"],
            vec!["-q", "--stdin", "a.lua"],
        ] {
            let output = dotfmt(root.path()).args(args).stdin(input).run();
            assert_eq!(output.code(), Some(1), "{output:?}");
            assert!(output.stdout.is_empty());
            assert!(output.stderr.contains(diagnostic), "{output:?}");
        }
    }
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

#[test]
fn editor_filenames_infer_languages_and_dialects_without_reading_or_writing_files() {
    let root = tree_pairs(&[("dotfmt.dotfile", CONFIG)]);
    for (name, input, expected) in [
        ("a.json", "{key:'value',}", "{\n  \"key\": \"value\"\n}"),
        (
            "a.jsonc",
            "// note\n{\"x\":1,}",
            "// note\n{\n  \"x\": 1,\n}",
        ),
        ("a.lua", "local x=1", "local x = 1"),
        ("a.luau", "local x: number=1", "local x: number = 1"),
        ("a.md", "__Strong__", "**Strong**"),
        ("a.conf", "x=1  \n", "x=1"),
        ("a.dotfile", "host{\nx=1\n}", "host {\n  x  = 1\n}"),
    ] {
        let path = root.path().join(name);
        let output = dotfmt(root.path()).args(["-e", name]).stdin(input).run();
        assert!(output.success(), "{name}: {output:?}");
        assert_eq!(output.stdout, expected, "{name}");
        assert!(output.stderr.is_empty(), "{name}: {output:?}");
        assert!(!path.exists(), "editor created {name}");
        fs::write(&path, "disk contents must stay untouched").unwrap();
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        let existing = dotfmt(root.path()).args(["-e", name]).stdin(input).run();
        assert!(existing.success(), "{name}: {existing:?}");
        assert_eq!(existing.stdout, expected, "{name}");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "disk contents must stay untouched"
        );
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
    }
}

#[test]
fn editor_filename_uses_local_settings_custom_mappings_and_exclusions() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", CONFIG),
        (
            "nested/dotfmt.dotfile",
            "json {\n indent = 4\n include { payload }\n}\nexcluded_files { skip.json }\n",
        ),
    ]);
    for name in ["nested/a.json", "nested/payload"] {
        let output = dotfmt(root.path())
            .args(["-e", name])
            .stdin("{\"x\":1}")
            .run();
        assert!(output.success(), "{name}: {output:?}");
        assert_eq!(output.stdout, "{\n    \"x\": 1\n}");
        assert!(output.stderr.is_empty(), "{output:?}");
        assert!(!root.path().join(name).exists());
    }
    let output = dotfmt(root.path())
        .args(["-e", "nested/skip.json"])
        .stdin("unformatted input")
        .run();
    assert!(output.success(), "{output:?}");
    assert_eq!(output.stdout, "unformatted input");
    assert!(output.stderr.is_empty(), "{output:?}");
    assert!(!root.path().join("nested/skip.json").exists());
}

#[test]
fn editor_rejects_multiple_filenames_and_missing_filename_context() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", CONFIG),
        ("a.json", "{}"),
        ("b.json", "{}"),
    ]);
    for args in [vec!["-e", "a.json", "b.json"], vec!["-e", "-", "a.json"]] {
        let output = dotfmt(root.path()).args(args).stdin("{\"x\":1}").run();
        assert!(!output.success(), "{output:?}");
        assert!(output.stdout.is_empty());
        assert!(output.stderr.contains("one filename"), "{output:?}");
    }
    for args in [vec!["-e"], vec!["-e", "-"]] {
        let output = dotfmt(root.path()).args(args).stdin("{\"x\":1}").run();
        assert!(!output.success(), "{output:?}");
        assert!(output.stdout.is_empty());
        assert!(output.stderr.contains("--editor FILENAME"), "{output:?}");
    }
    for name in ["a.json", "b.json"] {
        assert_eq!(fs::read_to_string(root.path().join(name)).unwrap(), "{}");
    }
}

#[test]
fn editor_check_emits_no_output_and_legacy_stdin_flags_remain_compatible() {
    let root = tree_pairs(&[("dotfmt.dotfile", CONFIG), ("a.json", "disk contents")]);
    for (input, code) in [("{\"x\":1}", 1), ("{\n  \"x\": 1\n}", 0)] {
        let output = dotfmt(root.path())
            .args(["--check", "-e", "a.json"])
            .stdin(input)
            .run();
        assert_eq!(output.code(), Some(code), "{output:?}");
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty(), "{output:?}");
    }
    let legacy = dotfmt(root.path())
        .args(["-eq", "--stdin", "a.json"])
        .stdin("{key:'value',}")
        .run();
    let editor = dotfmt(root.path())
        .args(["-e", "a.json"])
        .stdin("{key:'value',}")
        .run();
    assert!(legacy.success(), "{legacy:?}");
    assert!(editor.success(), "{editor:?}");
    assert_eq!(legacy.stdout, editor.stdout);
    assert!(legacy.stderr.is_empty(), "{legacy:?}");
    assert_eq!(
        fs::read_to_string(root.path().join("a.json")).unwrap(),
        "disk contents"
    );
}
