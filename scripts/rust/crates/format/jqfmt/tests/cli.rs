#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use testkit::{Bin, Ran, tree_pairs};

fn jqfmt(root: &Path, args: &[&str], body: &str) -> Ran {
    Bin::new(env!("CARGO_BIN_EXE_jqfmt"))
        .args(args)
        .current_dir(root)
        .plain()
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("empty-config"))
        .stdin(body)
        .run()
}

fn code(output: &Ran) -> i32 {
    output.code().expect("jqfmt exits rather than signals")
}

const LAID_OUT: &str = "{\n  \"a\": 1\n}\n";
const RAGGED: &str = "{\"a\":1}";

// ------------------------------------------------------------------- stdin

#[test]
fn stdin_puts_only_the_laid_out_body_on_stdout_and_says_nothing() {
    let root = tree_pairs(&[]);
    let output = jqfmt(root.path(), &[], RAGGED);

    assert_eq!(code(&output), 0);
    assert_eq!(output.stdout, LAID_OUT);
    assert_eq!(output.stderr, "");
}

#[test]
fn stdin_leaves_an_empty_body_empty_rather_than_failing() {
    let root = tree_pairs(&[]);
    let output = jqfmt(root.path(), &[], "");

    assert_eq!(code(&output), 0);
    assert_eq!(output.stdout, "");
    assert_eq!(output.stderr, "");
}

#[test]
fn stdin_writes_nothing_at_all_when_the_body_will_not_read() {
    // The buffer's safety rests on both halves of this: a non-zero status so
    // conform throws the result away, and an empty stdout so there is nothing
    // to throw away in the first place.
    let root = tree_pairs(&[]);
    let output = jqfmt(root.path(), &[], "{\"a\":1,}");

    assert_eq!(code(&output), 1);
    assert_eq!(output.stdout, "");
    assert_eq!(
        output.stderr,
        "jqfmt: stdin:1:8: stray comma; --editor fixes this\n"
    );
}

#[test]
fn a_body_of_comments_is_refused_rather_than_emptied() {
    let root = tree_pairs(&[]);
    let output = jqfmt(root.path(), &[], "// a note\n");

    assert_eq!(code(&output), 1);
    assert_eq!(output.stdout, "");
    assert_eq!(
        output.stderr,
        "jqfmt: stdin:1:1: comments are not JSON; --editor fixes this\n"
    );
}

#[test]
fn a_dash_says_the_same_thing_as_naming_nothing() {
    let root = tree_pairs(&[]);
    let output = jqfmt(root.path(), &["-"], RAGGED);

    assert_eq!(code(&output), 0);
    assert_eq!(output.stdout, LAID_OUT);
}

// ------------------------------------------------------------------ editor

#[test]
fn editor_takes_the_mistakes_and_says_what_it_fixed() {
    let root = tree_pairs(&[]);
    let output = jqfmt(root.path(), &["--editor"], "{\n  'a': 1, // why\n}\n");

    assert_eq!(code(&output), 0);
    assert_eq!(output.stdout, LAID_OUT);
    assert_eq!(
        output.stderr,
        "jqfmt: stdin: fixed 1 stray comma, 1 comment, 1 single quote\n"
    );
}

#[test]
fn editor_reads_the_file_the_editor_this_repository_uses_cannot_parse() {
    // `shared/vscode/settings.json` carries a trailing comma, which is why a
    // JSONC buffer goes through the flag rather than around it.
    let root = tree_pairs(&[]);
    let output = jqfmt(root.path(), &["-e"], "{\n  \"a\": 1,\n}\n");

    assert_eq!(code(&output), 0);
    assert_eq!(output.stdout, LAID_OUT);
}

#[test]
fn editor_still_writes_nothing_when_the_body_cannot_be_read_at_all() {
    let root = tree_pairs(&[]);
    let output = jqfmt(root.path(), &["--editor"], "{\"a\" 1}");

    assert_eq!(code(&output), 1);
    assert_eq!(output.stdout, "");
    assert!(output.stderr.contains("expected : after a key"), "{}", output.stderr);
}

#[test]
fn editor_leaves_the_body_alone_when_there_is_nothing_to_fix() {
    let root = tree_pairs(&[]);
    let output = jqfmt(root.path(), &["--editor"], LAID_OUT);

    assert_eq!(code(&output), 0);
    assert_eq!(output.stdout, LAID_OUT);
    assert_eq!(output.stderr, "");
}

// ------------------------------------------------------------------- files

#[test]
fn a_directory_is_walked_and_its_files_are_written_in_place() {
    let root = tree_pairs(&[("a.json", RAGGED), ("deep/b.json", "[1,2]"), ("c.txt", "{}")]);
    let output = jqfmt(root.path(), &["."], "");

    assert_eq!(code(&output), 0);
    assert_eq!(read(root.path(), "a.json"), LAID_OUT);
    assert_eq!(read(root.path(), "deep/b.json"), "[\n  1,\n  2\n]\n");
    assert_eq!(read(root.path(), "c.txt"), "{}");
    assert_eq!(output.stdout, "");
    assert!(output.stderr.contains("formatted 2 of 2 files"), "{}", output.stderr);
}

#[test]
fn check_reports_what_is_unformatted_and_writes_nothing() {
    let root = tree_pairs(&[("a.json", RAGGED), ("b.json", LAID_OUT)]);
    let output = jqfmt(root.path(), &["--check", "."], "");

    assert_eq!(code(&output), 1);
    assert_eq!(read(root.path(), "a.json"), RAGGED);
    assert!(output.stderr.contains("needs format a.json"), "{}", output.stderr);
    assert!(output.stderr.contains("1 of 2 files need formatting"), "{}", output.stderr);
}

#[test]
fn check_succeeds_quietly_when_everything_is_already_formatted() {
    let root = tree_pairs(&[("a.json", LAID_OUT)]);
    let output = jqfmt(root.path(), &["--check", "."], "");

    assert_eq!(code(&output), 0);
    assert!(output.stderr.contains("1 file already formatted"), "{}", output.stderr);
}

#[test]
fn check_with_editor_fails_on_a_file_that_had_to_be_repaired() {
    // The gate for a hook: a JSONC file is a file the strict reader cannot
    // read, and here that is a finding rather than a quiet rewrite.
    let root = tree_pairs(&[("a.json", "{\n  \"a\": 1,\n}\n")]);
    let output = jqfmt(root.path(), &["--editor", "--check", "."], "");

    assert_eq!(code(&output), 1);
    assert_eq!(read(root.path(), "a.json"), "{\n  \"a\": 1,\n}\n");
    assert!(output.stderr.contains("would fix 1 stray comma"), "{}", output.stderr);
}

#[test]
fn a_file_that_will_not_read_is_reported_by_name_and_line() {
    let root = tree_pairs(&[("a.json", "{\n  \"a\": 1,\n  \"b\" 2\n}")]);
    let output = jqfmt(root.path(), &["."], "");

    assert_eq!(code(&output), 1);
    assert_eq!(
        output.stderr.lines().next().unwrap(),
        "jqfmt: a.json:3:7: expected : after a key"
    );
}

#[test]
fn a_file_named_on_the_command_line_needs_no_extension() {
    let root = tree_pairs(&[(".prettierrc", RAGGED)]);
    let output = jqfmt(root.path(), &[".prettierrc"], "");

    assert_eq!(code(&output), 0);
    assert_eq!(read(root.path(), ".prettierrc"), LAID_OUT);
}

#[test]
fn a_config_beside_the_files_governs_them() {
    let root = tree_pairs(&[
        ("jqfmt.dotfile", "jqfmt {\n  indent = 4\n  final_newline = false\n}\n"),
        ("a.json", RAGGED),
    ]);
    let output = jqfmt(root.path(), &["a.json"], "");

    assert_eq!(code(&output), 0);
    assert_eq!(read(root.path(), "a.json"), "{\n    \"a\": 1\n}");
}

#[test]
fn a_setting_that_cannot_be_honoured_is_named_on_stderr() {
    let root = tree_pairs(&[
        ("jqfmt.dotfile", "jqfmt {\n  align = true\n}\n"),
        ("a.json", LAID_OUT),
    ]);
    let output = jqfmt(root.path(), &["a.json"], "");

    assert_eq!(code(&output), 0);
    assert!(
        output.stderr.contains("warning: jq does not support align"),
        "{}",
        output.stderr
    );
}

#[test]
fn a_mistake_in_the_config_is_reported_at_its_line() {
    let root = tree_pairs(&[
        ("jqfmt.dotfile", "jqfmt {\n  indent = 9\n}\n"),
        ("a.json", LAID_OUT),
    ]);
    let output = jqfmt(root.path(), &["a.json"], "");

    assert_eq!(code(&output), 1);
    assert!(
        output.stderr.contains("indent must be -1 for a tab, or between 0 and 7"),
        "{}",
        output.stderr
    );
}

#[test]
fn a_target_that_is_not_there_is_a_failure() {
    let root = tree_pairs(&[]);
    let output = jqfmt(root.path(), &["absent.json"], "");

    assert_eq!(code(&output), 1);
    assert!(output.stderr.contains("absent.json"), "{}", output.stderr);
}

#[test]
fn verbose_names_the_config_that_was_read() {
    let root = tree_pairs(&[("jqfmt.dotfile", "jqfmt {\n  indent = 2\n}\n"), ("a.json", RAGGED)]);
    let output = jqfmt(root.path(), &["--verbose", "a.json"], "");

    assert_eq!(code(&output), 0);
    assert!(output.stderr.contains("config"), "{}", output.stderr);
    assert!(output.stderr.contains("jqfmt.dotfile"), "{}", output.stderr);
}

// ------------------------------------------------------------------ the rest

#[test]
fn help_names_the_flag_this_exists_for() {
    let root = tree_pairs(&[]);
    let output = jqfmt(root.path(), &["--help"], "");

    assert_eq!(code(&output), 0);
    assert!(output.stdout.contains("--editor"), "{}", output.stdout);
}

#[test]
fn an_unknown_option_is_a_usage_error_rather_than_a_failure_to_format() {
    let root = tree_pairs(&[]);
    let output = jqfmt(root.path(), &["--nope"], "");

    assert_eq!(code(&output), 2);
    assert_eq!(output.stdout, "");
}

#[test]
fn completions_and_the_command_dump_are_data_on_stdout() {
    let root = tree_pairs(&[]);
    for (args, expected) in [
        (["--completions", "zsh"], "#compdef jqfmt"),
        (["--command-dump", ""], "\"version\":1"),
    ] {
        let args: Vec<&str> = args.iter().copied().filter(|arg| !arg.is_empty()).collect();
        let output = jqfmt(root.path(), &args, "");
        assert_eq!(code(&output), 0);
        assert!(output.stdout.contains(expected), "{}", output.stdout);
        assert_eq!(output.stderr, "");
    }
}

#[test]
fn every_json_file_this_repository_holds_is_laid_out_exactly_as_jq_lays_it_out() {
    // The contract the rest of the repository rests on: a format run has to be
    // a no-op on files jq has already formatted, and a no-op means these bytes.
    let Some(files) = repository_json_files() else {
        return;
    };
    assert!(
        files.len() > 20,
        "expected the repository's files, found {}",
        files.len()
    );
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_jqfmt"));
    let roomy = tree_pairs(&[]);
    assert!(jq(&[], &["--version"]).is_some(), "jq is not installed");
    let mut compared = 0;
    for path in files {
        let raw = fs::read(&path).unwrap();
        let Some(theirs) = jq(&raw, &["--indent", "2", "."]) else {
            // A body jq cannot read is one of the JSONC files this repository
            // holds, and `--editor` is the flag that is about those.
            let (status, _, _) = run(&binary, &["-"], &raw, roomy.path());
            assert_ne!(
                status, 0,
                "{} reads for jq but not here",
                path.display()
            );
            continue;
        };
        let (status, ours, erred) = run(&binary, &["-"], &raw, roomy.path());
        assert_eq!(status, 0, "{}: {}", path.display(), String::from_utf8_lossy(&erred));
        assert_eq!(ours, theirs, "{} differs from jq", path.display());
        compared += 1;
    }
    assert!(compared > 20, "compared {compared} files");
}

#[test]
fn one_line_reads_the_way_jq_compact_reads() {
    // `indent = 0` is jq's `--indent 0`, and the layout rules are the same
    // rules: no line breaks, no space after a colon, no space after a comma.
    let Some(files) = repository_json_files() else {
        return;
    };
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_jqfmt"));
    let compact = tree_pairs(&[("jqfmt.dotfile", "jqfmt {\n  indent = 0\n}\n")]);
    let mut compared = 0;
    for path in files.iter().take(12) {
        let raw = fs::read(path).unwrap();
        let Some(theirs) = jq(&raw, &["-c", "."]) else {
            continue;
        };
        let (status, ours, erred) = run(&binary, &["-"], &raw, compact.path());
        assert_eq!(status, 0, "{}: {}", path.display(), String::from_utf8_lossy(&erred));
        assert_eq!(ours, theirs, "{} differs from jq -c", path.display());
        compared += 1;
    }
    assert!(compared > 5, "compared {compared} files");
}

#[test]
fn a_body_that_is_already_laid_out_comes_back_unchanged() {
    let root = tree_pairs(&[]);
    let output = jqfmt(root.path(), &["-"], LAID_OUT);

    assert_eq!(code(&output), 0);
    assert_eq!(output.stdout, LAID_OUT);
}

fn read(root: &Path, name: &str) -> String {
    fs::read_to_string(root.join(name)).unwrap()
}

fn run(binary: &Path, args: &[&str], input: &[u8], here: &Path) -> (i32, Vec<u8>, Vec<u8>) {
    let mut child = Command::new(binary)
        .args(args)
        .current_dir(here)
        .env("HOME", here)
        .env("XDG_CONFIG_HOME", here.join("empty-config"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("jqfmt runs");
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().expect("jqfmt reads stdin");
        stdin.write_all(input).expect("the body fits in a pipe");
    }
    let output = child.wait_with_output().expect("jqfmt exits");
    (
        output.status.code().unwrap_or(-1),
        output.stdout,
        output.stderr,
    )
}

fn jq(input: &[u8], args: &[&str]) -> Option<Vec<u8>> {
    let mut child = Command::new("jq")
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    {
        use std::io::Write;
        let mut stdin = child.stdin.take()?;
        stdin.write_all(input).ok()?;
    }
    let output = child.wait_with_output().ok()?;
    output.status.success().then_some(output.stdout)
}

/// Every `.json` below the repository root, or nothing when the tests are not
/// running from a checkout of it.
fn repository_json_files() -> Option<Vec<PathBuf>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../..");
    let root = root.canonicalize().ok()?;
    if !root.join("config/targets.dotfile").is_file() {
        return None;
    }
    let mut found = Vec::new();
    collect(&root, &mut found);
    found.sort();
    Some(found)
}

fn collect(directory: &Path, found: &mut Vec<PathBuf>) {
    const SKIP: [&str; 13] = [
        ".git",
        "node_modules",
        "target",
        ".bin",
        ".cache",
        ".venv",
        "dist",
        "build",
        "out",
        "vendor",
        "__pycache__",
        ".ruff_cache",
        ".uv",
    ];
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if path.is_dir() {
            if SKIP.contains(&name.as_str()) {
                continue;
            }
            collect(&path, found);
        } else if path.extension().is_some_and(|kind| kind == "json") {
            found.push(path);
        }
    }
}

#[test]
fn dialects_and_aliases_preserve_comments_on_stdin() {
    let root = tree_pairs(&[]);
    for dialect in ["jsonc", "hujson", "jwcc", "json-with-comments"] {
        let output = jqfmt(root.path(), &["--dialect", dialect], "{\"a\":1,// why\n}");
        assert_eq!(code(&output), 0, "{}", output.stderr);
        assert_eq!(output.stdout, "{\n  \"a\": 1, // why\n}\n");
        assert_eq!(output.stderr, "");
    }
}

#[test]
fn mixed_tree_detects_dialects_and_check_is_read_only() {
    let body = "{\"a\":1,/* why */}";
    let expected = "{\n  \"a\": 1, /* why */\n}\n";
    let root = tree_pairs(&[("a.json", RAGGED), ("b.jsonc", body), ("c.HUJSON", body), ("d.jwcc", body), ("ignored.json5", body)]);
    let check = jqfmt(root.path(), &["--check", "."], "");
    assert_eq!(code(&check), 1);
    assert_eq!(read(root.path(), "b.jsonc"), body);
    let output = jqfmt(root.path(), &["."], "");
    assert_eq!(code(&output), 0, "{}", output.stderr);
    assert_eq!(read(root.path(), "a.json"), LAID_OUT);
    for name in ["b.jsonc", "c.HUJSON", "d.jwcc"] {
        assert_eq!(read(root.path(), name), expected);
    }
    assert_eq!(read(root.path(), "ignored.json5"), body);
    assert_eq!(code(&jqfmt(root.path(), &["--check", "."], "")), 0);
}

#[test]
fn explicit_dialect_overrides_extensions() {
    let body = "{\"a\":1,// why\n}";
    let root = tree_pairs(&[("settings.json", body), ("settings.jsonc", body)]);
    assert_eq!(code(&jqfmt(root.path(), &["settings.json"], "")), 1);
    assert_eq!(code(&jqfmt(root.path(), &["--dialect", "jsonc", "settings.json"], "")), 0);
    assert!(read(root.path(), "settings.json").contains("// why"));
    assert_eq!(code(&jqfmt(root.path(), &["--dialect", "json", "settings.jsonc"], "")), 1);
    assert_eq!(read(root.path(), "settings.jsonc"), body);
}

#[test]
fn invalid_dialect_input_never_overwrites_a_file() {
    let body = "{\"a\":1, // why\n\"b\" 2}";
    let root = tree_pairs(&[("a.hujson", body)]);
    let output = jqfmt(root.path(), &["a.hujson"], "");
    assert_eq!(code(&output), 1);
    assert_eq!(read(root.path(), "a.hujson"), body);
    assert!(output.stderr.contains("a.hujson:2:5:"), "{}", output.stderr);
}

#[test]
fn editor_explicitly_converts_dialect_files_to_json() {
    let root = tree_pairs(&[("a.jsonc", "{\"a\":1,/* why */}")]);
    let output = jqfmt(root.path(), &["--editor", "a.jsonc"], "");
    assert_eq!(code(&output), 0, "{}", output.stderr);
    assert_eq!(read(root.path(), "a.jsonc"), LAID_OUT);
    assert!(output.stderr.contains("1 stray comma, 1 comment"), "{}", output.stderr);
}
