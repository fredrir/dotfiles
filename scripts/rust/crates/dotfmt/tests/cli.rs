//! Black-box checks on the flags, the streams and the exit codes callers
//! depend on.
//!
//! Mostly the streams. conform.nvim replaces a buffer with whatever stdout
//! carried and only throws it away when the status is non-zero, so "stdout is
//! empty" and "the status is 1" are not stylistic preferences here — they are
//! the difference between a saved file and a mangled one.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

/// A run with `HOME` and `XDG_CONFIG_HOME` pointed at an empty directory, so
/// the machine's own `~/.config/dotfmt/dotfile.dotfile` cannot decide a test.
fn dotfmt(root: &Path, args: &[&str], body: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dotfmt"))
        .args(args)
        .current_dir(root)
        .env("NO_COLOR", "1")
        .env("COLUMNS", "80")
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("empty-config"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("dotfmt runs");
    child
        .stdin
        .take()
        .expect("stdin is a pipe")
        .write_all(body.as_bytes())
        .expect("the body is read");
    child.wait_with_output().expect("dotfmt finishes")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn code(output: &Output) -> i32 {
    output
        .status
        .code()
        .expect("dotfmt exits rather than signals")
}

/// A tree from `path=contents` lines.
fn tree(lines: &[(&str, &str)]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for (path, contents) in lines {
        let at = root.path().join(path);
        fs::create_dir_all(at.parent().unwrap()).unwrap();
        fs::write(&at, contents).unwrap();
    }
    root
}

/// A `.dotfile` whose entries are not yet in a column.
const RAGGED: &str = "host {\n  a = 1\n  longer = 2\n}\n";
const LAID_OUT: &str = "host {\n  a       = 1\n  longer  = 2\n}\n";

#[test]
fn no_arguments_at_all_prints_the_help_on_stdout_and_stops() {
    let root = tempfile::tempdir().unwrap();
    let output = dotfmt(root.path(), &[], "");

    assert_eq!(code(&output), 0);
    assert!(
        stdout(&output).contains("Usage: dotfmt"),
        "{}",
        stdout(&output)
    );
    assert_eq!(stderr(&output), "");
}

#[test]
fn help_shows_the_tools_own_about_rather_than_a_flattened_structs() {
    let root = tempfile::tempdir().unwrap();
    let output = dotfmt(root.path(), &["--help"], "");

    assert_eq!(code(&output), 0);
    let shown = stdout(&output);
    assert!(
        shown.starts_with("Format .conf, .config and .dotfile files"),
        "{shown}"
    );
}

#[test]
fn completions_and_the_command_dump_are_data_on_stdout() {
    let root = tempfile::tempdir().unwrap();
    for (args, expected) in [
        (["--completions", "zsh"], "#compdef dotfmt"),
        (["--command-dump", ""], "C\tdotfmt\t"),
    ] {
        let args: Vec<&str> = args.iter().copied().filter(|arg| !arg.is_empty()).collect();
        let output = dotfmt(root.path(), &args, "");
        assert_eq!(code(&output), 0);
        assert!(stdout(&output).contains(expected), "{}", stdout(&output));
        assert_eq!(stderr(&output), "");
    }
}

#[test]
fn stdin_puts_only_the_formatted_body_on_stdout_and_says_nothing() {
    let root = tempfile::tempdir().unwrap();
    let output = dotfmt(root.path(), &["--stdin", "hosts.dotfile"], RAGGED);

    assert_eq!(code(&output), 0);
    assert_eq!(stdout(&output), LAID_OUT);
    assert_eq!(stderr(&output), "");
}

#[test]
fn stdin_on_a_conf_file_formats_it_as_a_conf_file() {
    let root = tempfile::tempdir().unwrap();
    let output = dotfmt(
        root.path(),
        &["--stdin", "/home/x/.config/hypr/hyprland.conf"],
        "general{\ngaps_in=5\n}\n",
    );

    assert_eq!(code(&output), 0);
    assert_eq!(stdout(&output), "general{\n    gaps_in = 5\n}\n");
    assert_eq!(stderr(&output), "");
}

#[test]
fn stdin_writes_nothing_at_all_when_the_body_will_not_parse() {
    // The buffer's safety rests on both halves of this: a non-zero status so
    // conform throws the result away, and an empty stdout so there is nothing
    // to throw away in the first place.
    let root = tempfile::tempdir().unwrap();
    let output = dotfmt(
        root.path(),
        &["--stdin", "hosts.dotfile"],
        "a {\n  x = 1\nb {\n  y = 2\n}\n",
    );

    assert_eq!(code(&output), 1);
    assert_eq!(stdout(&output), "");
    assert_eq!(stderr(&output), "dotfmt: hosts.dotfile:3: nested block\n");
}

#[test]
fn stdin_echoes_a_file_dotfmt_does_not_own() {
    let root = tempfile::tempdir().unwrap();
    let body = "x  =  1\n\n\n";
    let output = dotfmt(root.path(), &["--stdin", "script.py"], body);

    assert_eq!(code(&output), 0);
    assert_eq!(stdout(&output), body);
    assert_eq!(stderr(&output), "");
}

#[test]
fn stdin_refuses_to_be_given_a_target_as_well() {
    let root = tempfile::tempdir().unwrap();
    let output = dotfmt(root.path(), &["--stdin", "a.dotfile", "b.dotfile"], "");

    assert_eq!(code(&output), 2);
    assert_eq!(stdout(&output), "");
}

#[test]
fn stdin_reads_the_settings_that_govern_the_name_it_was_given() {
    let root = tree(&[
        ("project/dotfile.dotfile", "dotfmt {\n  indent = 4\n}\n"),
        ("project/hosts.dotfile", ""),
    ]);
    let output = dotfmt(
        root.path(),
        &["--stdin", "project/hosts.dotfile"],
        "host {\n  a = 1\n}\n",
    );

    assert_eq!(code(&output), 0);
    assert_eq!(stdout(&output), "host {\n    a  = 1\n}\n");
}

#[test]
fn the_config_in_the_home_directory_is_the_next_place_looked() {
    let root = tree(&[
        (
            "empty-config/dotfmt/dotfile.dotfile",
            "dotfmt {\n  align = false\n}\n",
        ),
        ("a.dotfile", ""),
    ]);
    let output = dotfmt(root.path(), &["--stdin", "a.dotfile"], RAGGED);

    assert_eq!(code(&output), 0);
    assert_eq!(stdout(&output), RAGGED);
}

#[test]
fn check_writes_nothing_to_stdout_and_nothing_to_the_tree() {
    let root = tree(&[("a.dotfile", RAGGED)]);
    let output = dotfmt(root.path(), &["--check", "."], "");

    assert_eq!(code(&output), 1);
    assert_eq!(stdout(&output), "");
    assert!(
        stderr(&output).contains("needs format a.dotfile"),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        fs::read_to_string(root.path().join("a.dotfile")).unwrap(),
        RAGGED
    );
}

#[test]
fn check_over_a_formatted_tree_says_so_and_succeeds() {
    let root = tree(&[("a.dotfile", LAID_OUT)]);
    let output = dotfmt(root.path(), &["--check", "."], "");

    assert_eq!(code(&output), 0);
    assert_eq!(stdout(&output), "");
    assert_eq!(stderr(&output), "1 file already formatted\n");
}

#[test]
fn the_check_summary_never_claims_to_have_formatted_anything() {
    // A check run writes nothing, so its summary must not use the verb a
    // writing run uses. "2 files formatted" after `--check` reads as though
    // the tree had just been rewritten, which is the one thing it was asked
    // not to do.
    let clean = tree(&[("a.dotfile", LAID_OUT), ("b.dotfile", LAID_OUT)]);
    let clean = dotfmt(clean.path(), &["--check", "."], "");
    assert_eq!(code(&clean), 0);
    assert_eq!(stderr(&clean), "2 files already formatted\n");

    let drifted = tree(&[("a.dotfile", RAGGED), ("b.dotfile", LAID_OUT)]);
    let drifted = dotfmt(drifted.path(), &["--check", "."], "");
    assert_eq!(code(&drifted), 1);
    assert_eq!(
        stderr(&drifted),
        "  needs format a.dotfile\n1 of 2 files need formatting\n"
    );

    // And the writing run is the only one that gets to say "formatted".
    let written = tree(&[("a.dotfile", RAGGED), ("b.dotfile", LAID_OUT)]);
    let written = dotfmt(written.path(), &["."], "");
    assert_eq!(code(&written), 0);
    assert_eq!(
        stderr(&written),
        "  format a.dotfile\nformatted 1 of 2 files\n"
    );
}

#[test]
fn check_with_no_target_at_all_looks_at_the_current_directory() {
    let root = tree(&[("deep/a.dotfile", RAGGED)]);
    let output = dotfmt(root.path(), &["--check"], "");

    assert_eq!(code(&output), 1);
    assert!(
        stderr(&output).contains("deep/a.dotfile"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_run_formats_the_tree_and_names_what_it_changed() {
    let root = tree(&[("a.dotfile", RAGGED), ("b.toml", "untouched")]);
    let output = dotfmt(root.path(), &["."], "");

    assert_eq!(code(&output), 0);
    assert_eq!(stdout(&output), "");
    assert_eq!(
        stderr(&output),
        "  format a.dotfile\nformatted 1 of 1 file\n"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("a.dotfile")).unwrap(),
        LAID_OUT
    );
    assert_eq!(
        fs::read_to_string(root.path().join("b.toml")).unwrap(),
        "untouched"
    );
}

#[test]
fn formatting_twice_changes_nothing_the_second_time() {
    let root = tree(&[("a.dotfile", RAGGED)]);
    dotfmt(root.path(), &["."], "");
    let output = dotfmt(root.path(), &["."], "");

    assert_eq!(code(&output), 0);
    assert_eq!(stderr(&output), "formatted 0 of 1 file\n");
    assert_eq!(
        fs::read_to_string(root.path().join("a.dotfile")).unwrap(),
        LAID_OUT
    );
}

#[test]
fn quiet_says_nothing_anywhere_about_a_run_that_changed_a_file() {
    let root = tree(&[("a.dotfile", RAGGED)]);
    let output = dotfmt(root.path(), &["-q", "."], "");

    assert_eq!(code(&output), 0);
    assert_eq!(stdout(&output), "");
    assert_eq!(stderr(&output), "");
    assert_eq!(
        fs::read_to_string(root.path().join("a.dotfile")).unwrap(),
        LAID_OUT
    );
}

#[test]
fn verbose_names_every_file_and_which_mode_a_conf_was_read_in() {
    let root = tree(&[("kitty/kitty.conf", "font_size  12\n")]);
    let output = dotfmt(root.path(), &["-v", "."], "");

    assert_eq!(code(&output), 0);
    let said = stderr(&output);
    assert!(said.contains("config built-in defaults"), "{said}");
    assert!(said.contains("ok kitty/kitty.conf  kitty"), "{said}");
}

#[test]
fn one_bad_file_is_reported_and_the_rest_of_the_run_still_happens() {
    // `format.py` aborts the batch on the first odd path, so one bad argument
    // hides every other file's answer.
    let root = tree(&[
        ("a.dotfile", RAGGED),
        ("broken.dotfile", "a {\nb {\n}\n"),
        ("c.dotfile", RAGGED),
    ]);
    let output = dotfmt(root.path(), &["."], "");

    assert_eq!(code(&output), 1);
    let said = stderr(&output);
    assert!(
        said.contains("dotfmt: broken.dotfile:2: nested block"),
        "{said}"
    );
    assert!(said.contains("formatted 2 of 3 files"), "{said}");
    assert_eq!(
        fs::read_to_string(root.path().join("a.dotfile")).unwrap(),
        LAID_OUT
    );
    assert_eq!(
        fs::read_to_string(root.path().join("c.dotfile")).unwrap(),
        LAID_OUT
    );
    assert_eq!(
        fs::read_to_string(root.path().join("broken.dotfile")).unwrap(),
        "a {\nb {\n}\n"
    );
}

#[test]
fn a_target_that_is_not_there_is_a_failure() {
    let root = tempfile::tempdir().unwrap();
    let output = dotfmt(root.path(), &["absent"], "");

    assert_eq!(code(&output), 1);
    assert_eq!(stdout(&output), "");
    assert!(
        stderr(&output).starts_with("dotfmt: absent:"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_named_file_dotfmt_does_not_own_is_a_failure_rather_than_a_silent_skip() {
    let root = tree(&[("a.py", "x = 1\n")]);
    let output = dotfmt(root.path(), &["a.py"], "");

    assert_eq!(code(&output), 1);
    assert!(
        stderr(&output).contains("not a .conf, .config or .dotfile file: a.py"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_mistake_in_the_config_stops_the_run_and_names_the_line() {
    let root = tree(&[
        ("dotfile.dotfile", "dotfmt {\n  indnet = 2\n}\n"),
        ("a.dotfile", RAGGED),
    ]);
    let output = dotfmt(root.path(), &["."], "");

    assert_eq!(code(&output), 1);
    assert!(
        stderr(&output).contains("dotfile.dotfile:2: unknown setting: indnet"),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        fs::read_to_string(root.path().join("a.dotfile")).unwrap(),
        RAGGED
    );
}

#[test]
fn a_bad_flag_is_claps_own_failure() {
    let root = tempfile::tempdir().unwrap();
    let output = dotfmt(root.path(), &["--nonsense"], "");

    assert_eq!(code(&output), 2);
    assert_eq!(stdout(&output), "");
}
