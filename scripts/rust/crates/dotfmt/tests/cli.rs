use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn dotfmt(root: &Path, args: &[&str], body: &str) -> Output {
    dotfmt_from(root, root, args, body)
}

fn dotfmt_from(root: &Path, home: &Path, args: &[&str], body: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dotfmt"))
        .args(args)
        .current_dir(root)
        .env("NO_COLOR", "1")
        .env("COLUMNS", "80")
        .env("HOME", home)
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

fn tree(lines: &[(&str, &str)]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    for (path, contents) in lines {
        let at = root.path().join(path);
        fs::create_dir_all(at.parent().unwrap()).unwrap();
        fs::write(&at, contents).unwrap();
    }
    root
}

const RAGGED: &str = "host {\n  a = 1\n  longer = 2\n}\n";
const LAID_OUT: &str = "host {\n  a       = 1\n  longer  = 2\n}";

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
        ("project/dotfmt.dotfile", "dotfmt {\n  indent = 4\n}\n"),
        ("project/hosts.dotfile", ""),
    ]);
    let output = dotfmt(
        root.path(),
        &["--stdin", "project/hosts.dotfile"],
        "host {\n  a = 1\n}\n",
    );

    assert_eq!(code(&output), 0);
    assert_eq!(stdout(&output), "host {\n    a  = 1\n}");
}

#[test]
fn the_config_in_the_home_directory_is_the_next_place_looked() {
    let root = tree(&[
        (
            "empty-config/dotfmt/dotfmt.dotfile",
            "dotfmt {\n  align = false\n}\n",
        ),
        ("a.dotfile", ""),
    ]);
    let output = dotfmt(root.path(), &["--stdin", "a.dotfile"], RAGGED);

    assert_eq!(code(&output), 0);
    assert_eq!(stdout(&output), "host {\n  a = 1\n  longer = 2\n}");
}

#[test]
fn the_home_directory_itself_is_the_place_looked_after_that() {
    let root = tree(&[
        (
            "empty-config/dotfmt/dotfmt.dotfile",
            "dotfmt {\n  indent = 4\n}\n",
        ),
        ("home/dotfmt.dotfile", "dotfmt {\n  indent = 6\n}\n"),
        ("a.dotfile", ""),
    ]);
    let home = root.path().join("home");

    let under_config = dotfmt_from(root.path(), &home, &["--stdin", "a.dotfile"], RAGGED);
    assert_eq!(
        stdout(&under_config),
        "host {\n    a       = 1\n    longer  = 2\n}"
    );

    fs::remove_file(root.path().join("empty-config/dotfmt/dotfmt.dotfile")).unwrap();
    let at_home = dotfmt_from(root.path(), &home, &["--stdin", "a.dotfile"], RAGGED);
    assert_eq!(
        stdout(&at_home),
        "host {\n      a       = 1\n      longer  = 2\n}"
    );
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

#[cfg(unix)]
#[test]
fn a_directory_that_cannot_be_read_is_named_rather_than_passed_over_in_silence() {
    use std::os::unix::fs::PermissionsExt;

    let root = tree(&[("dotfmt.dotfile", ""), ("a.dotfile", RAGGED)]);
    let shut = root.path().join("shut");
    fs::create_dir(&shut).unwrap();
    fs::set_permissions(&shut, fs::Permissions::from_mode(0o000)).unwrap();

    let output = dotfmt(root.path(), &["."], "");
    let quiet = dotfmt(root.path(), &["-q", "."], "");
    fs::set_permissions(&shut, fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(code(&output), 0);
    let said = stderr(&output);
    assert!(
        said.contains("dotfmt: 1 directory could not be read"),
        "{said}"
    );
    assert_eq!(stderr(&quiet), "");
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
    let root = tree(&[
        ("dotfmt.dotfile", "include {\n  .conf\n}\n"),
        ("kitty/kitty.conf", "font_size  12\n"),
    ]);
    let output = dotfmt(root.path(), &["-v", "."], "");

    assert_eq!(code(&output), 0);
    let said = stderr(&output);
    assert!(said.contains("config "), "{said}");
    assert!(said.contains("dotfmt.dotfile"), "{said}");
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
        ("dotfmt.dotfile", "dotfmt {\n  indnet = 2\n}\n"),
        ("a.dotfile", RAGGED),
    ]);
    let output = dotfmt(root.path(), &["."], "");

    assert_eq!(code(&output), 1);
    assert!(
        stderr(&output).contains("dotfmt.dotfile:2: unknown setting: indnet"),
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

#[test]
fn the_include_block_decides_which_files_a_walk_picks_up() {
    let root = tree(&[
        (
            "dotfmt.dotfile",
            "include {\n  .conf\n}\n\nexclude {\n  kitty\n}",
        ),
        ("a.conf", "x  =  1\n\n\n"),
        ("kitty/b.conf", "x  =  1\n\n\n"),
    ]);
    let output = dotfmt(root.path(), &["."], "");

    assert_eq!(code(&output), 0);
    assert_eq!(stderr(&output), "  format a.conf\nformatted 1 of 2 files\n");
    assert_eq!(
        fs::read_to_string(root.path().join("a.conf")).unwrap(),
        "x  =  1\n"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("kitty/b.conf")).unwrap(),
        "x  =  1\n\n\n"
    );
}

#[test]
fn a_conf_file_is_left_alone_until_a_config_opts_it_in() {
    // The default that made this rework necessary. `.dotfile` is dotfmt's own
    // format and is on; `.conf` is somebody else's and is not.
    let root = tree(&[("a.conf", "x  =  1\n\n\n"), ("b.dotfile", RAGGED)]);
    let output = dotfmt(root.path(), &["."], "");

    assert_eq!(code(&output), 0);
    assert_eq!(
        stderr(&output),
        "  format b.dotfile\nformatted 1 of 1 file\n"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("a.conf")).unwrap(),
        "x  =  1\n\n\n"
    );
}

#[test]
fn a_named_file_the_config_leaves_alone_is_a_failure_rather_than_a_silent_skip() {
    let root = tree(&[("a.conf", "x=1\n")]);
    let output = dotfmt(root.path(), &["a.conf"], "");

    assert_eq!(code(&output), 1);
    assert!(
        stderr(&output).contains("not selected by this config: a.conf"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn owns_answers_on_stdout_with_the_paths_it_would_format() {
    let root = tree(&[
        ("dotfmt.dotfile", "include {\n  .conf\n  **ssh/_empty_\n}\n"),
        ("a.conf", ""),
        ("a.dotfile", ""),
        ("a.py", ""),
        ("LICENSE", ""),
        ("ssh/config.d/40-cabled", ""),
    ]);
    // `deep/absent.conf` is not there at all: the answer is about paths, and
    // the caller is asking which ones dotfmt would take rather than which it
    // can open.
    let asked = "a.conf\0a.dotfile\0a.py\0LICENSE\0ssh/config.d/40-cabled\0deep/absent.conf\0";
    let output = dotfmt(root.path(), &["--owns"], asked);

    assert_eq!(code(&output), 0);
    assert_eq!(
        stdout(&output),
        "a.conf\0a.dotfile\0ssh/config.d/40-cabled\0deep/absent.conf\0"
    );
    assert_eq!(stderr(&output), "");
}

#[test]
fn owns_reads_the_config_of_each_path_rather_than_one_for_all_of_them() {
    let root = tree(&[
        ("one/dotfmt.dotfile", "include {\n  .conf\n}\n"),
        ("one/a.conf", ""),
        ("two/a.conf", ""),
    ]);
    let output = dotfmt(root.path(), &["--owns"], "one/a.conf\0two/a.conf\0");

    assert_eq!(code(&output), 0);
    assert_eq!(stdout(&output), "one/a.conf\0");
}

#[test]
fn owns_writes_nothing_at_all_when_a_config_will_not_parse() {
    // A half-answered question about ownership is worse than an unanswered
    // one: the caller cannot tell the files dotfmt declined from the ones it
    // never managed to consider.
    let root = tree(&[
        ("dotfmt.dotfile", "dotfmt {\n  indnet = 2\n}\n"),
        ("a.dotfile", ""),
    ]);
    let output = dotfmt(root.path(), &["--owns"], "a.dotfile\0");

    assert_eq!(code(&output), 1);
    assert_eq!(stdout(&output), "");
    assert!(
        stderr(&output).contains("unknown setting: indnet"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn owns_answers_nothing_for_nothing_rather_than_reading_the_tree() {
    let root = tree(&[("a.dotfile", RAGGED)]);
    let output = dotfmt(root.path(), &["--owns"], "");

    assert_eq!(code(&output), 0);
    assert_eq!(stdout(&output), "");
    assert_eq!(
        fs::read_to_string(root.path().join("a.dotfile")).unwrap(),
        RAGGED
    );
}

#[test]
fn owns_refuses_to_be_given_a_target_as_well() {
    let root = tempfile::tempdir().unwrap();
    let output = dotfmt(root.path(), &["--owns", "a.dotfile"], "");

    assert_eq!(code(&output), 2);
    assert_eq!(stdout(&output), "");
}

#[test]
fn a_pattern_holding_an_equals_is_refused_rather_than_rewritten() {
    // The trap this rejection exists for: `block.rs` reads `a=b` as an entry,
    // and laying this very file out would write it back as `a  = b`. A run
    // must never edit a pattern into a different pattern.
    let config = "include {\n  a=b\n}\n";
    let root = tree(&[("dotfmt.dotfile", config), ("b.dotfile", RAGGED)]);
    let output = dotfmt(root.path(), &["."], "");

    assert_eq!(code(&output), 1);
    assert!(
        stderr(&output).contains("a pattern cannot hold an ="),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        fs::read_to_string(root.path().join("dotfmt.dotfile")).unwrap(),
        config
    );
    assert_eq!(
        fs::read_to_string(root.path().join("b.dotfile")).unwrap(),
        RAGGED
    );
}
