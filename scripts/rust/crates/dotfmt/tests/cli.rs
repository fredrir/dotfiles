use std::fs;
use std::path::Path;

use testkit::{Bin, Ran, tree_pairs};

fn dotfmt(root: &Path, args: &[&str], body: &str) -> Ran {
    dotfmt_from(root, root, args, body)
}

fn dotfmt_from(root: &Path, home: &Path, args: &[&str], body: &str) -> Ran {
    Bin::new(env!("CARGO_BIN_EXE_dotfmt"))
        .args(args)
        .current_dir(root)
        .plain()
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", root.join("empty-config"))
        .stdin(body)
        .run()
}

const RAGGED: &str = "host {\n  a = 1\n  longer = 2\n}\n";
const LAID_OUT: &str = "host {\n  a       = 1\n  longer  = 2\n}";

#[test]
fn no_arguments_at_all_prints_the_help_on_stdout_and_stops() {
    let root = tree_pairs(&[]);
    let output = dotfmt(root.path(), &[], "");

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 0);
    assert!(output.stdout.contains("Usage: dotfmt"), "{}", output.stdout);
    assert_eq!(output.stderr, "");
}

#[test]
fn help_shows_the_tools_own_about_rather_than_a_flattened_structs() {
    let root = tree_pairs(&[]);
    let output = dotfmt(root.path(), &["--help"], "");

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 0);
    let shown = output.stdout;
    assert!(
        shown.starts_with("Format .conf, .config and .dotfile files"),
        "{shown}"
    );
}

#[test]
fn completions_and_the_command_dump_are_data_on_stdout() {
    let root = tree_pairs(&[]);
    for (args, expected) in [
        (["--completions", "zsh"], "#compdef dotfmt"),
        (["--command-dump", ""], "C\tdotfmt\t"),
    ] {
        let args: Vec<&str> = args.iter().copied().filter(|arg| !arg.is_empty()).collect();
        let output = dotfmt(root.path(), &args, "");
        assert_eq!(output.code().expect("dotfmt exits rather than signals"), 0);
        assert!(output.stdout.contains(expected), "{}", output.stdout);
        assert_eq!(output.stderr, "");
    }
}

#[test]
fn stdin_puts_only_the_formatted_body_on_stdout_and_says_nothing() {
    let root = tree_pairs(&[]);
    let output = dotfmt(root.path(), &["--stdin", "hosts.dotfile"], RAGGED);

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 0);
    assert_eq!(output.stdout, LAID_OUT);
    assert_eq!(output.stderr, "");
}

#[test]
fn stdin_on_a_conf_file_formats_it_as_a_conf_file() {
    let root = tree_pairs(&[]);
    let output = dotfmt(
        root.path(),
        &["--stdin", "/home/x/.config/hypr/hyprland.conf"],
        "general{\ngaps_in=5\n}\n",
    );

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 0);
    assert_eq!(output.stdout, "general{\n    gaps_in = 5\n}");
    assert_eq!(output.stderr, "");
}

#[test]
fn stdin_writes_nothing_at_all_when_the_body_will_not_parse() {
    // The buffer's safety rests on both halves of this: a non-zero status so
    // conform throws the result away, and an empty stdout so there is nothing
    // to throw away in the first place.
    let root = tree_pairs(&[]);
    let output = dotfmt(
        root.path(),
        &["--stdin", "hosts.dotfile"],
        "a {\n  x = 1\nb {\n  y = 2\n}\n",
    );

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 1);
    assert_eq!(output.stdout, "");
    assert_eq!(output.stderr, "dotfmt: hosts.dotfile:3: nested block\n");
}

#[test]
fn stdin_echoes_a_file_dotfmt_does_not_own() {
    let root = tree_pairs(&[]);
    let body = "x  =  1\n\n\n";
    let output = dotfmt(root.path(), &["--stdin", "script.py"], body);

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 0);
    assert_eq!(output.stdout, body);
    assert_eq!(output.stderr, "");
}

#[test]
fn stdin_refuses_to_be_given_a_target_as_well() {
    let root = tree_pairs(&[]);
    let output = dotfmt(root.path(), &["--stdin", "a.dotfile", "b.dotfile"], "");

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 2);
    assert_eq!(output.stdout, "");
}

#[test]
fn stdin_reads_the_settings_that_govern_the_name_it_was_given() {
    let root = tree_pairs(&[
        ("project/dotfmt.dotfile", "dotfmt {\n  indent = 4\n}\n"),
        ("project/hosts.dotfile", ""),
    ]);
    let output = dotfmt(
        root.path(),
        &["--stdin", "project/hosts.dotfile"],
        "host {\n  a = 1\n}\n",
    );

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 0);
    assert_eq!(output.stdout, "host {\n    a  = 1\n}");
}

#[test]
fn the_config_in_the_home_directory_is_the_next_place_looked() {
    let root = tree_pairs(&[
        (
            "empty-config/dotfmt/dotfmt.dotfile",
            "dotfmt {\n  align = false\n}\n",
        ),
        ("a.dotfile", ""),
    ]);
    let output = dotfmt(root.path(), &["--stdin", "a.dotfile"], RAGGED);

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 0);
    assert_eq!(output.stdout, "host {\n  a = 1\n  longer = 2\n}");
}

#[test]
fn the_home_directory_itself_is_the_place_looked_after_that() {
    let root = tree_pairs(&[
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
        under_config.stdout,
        "host {\n    a       = 1\n    longer  = 2\n}"
    );

    fs::remove_file(root.path().join("empty-config/dotfmt/dotfmt.dotfile")).unwrap();
    let at_home = dotfmt_from(root.path(), &home, &["--stdin", "a.dotfile"], RAGGED);
    assert_eq!(
        at_home.stdout,
        "host {\n      a       = 1\n      longer  = 2\n}"
    );
}

#[test]
fn check_writes_nothing_to_stdout_and_nothing_to_the_tree() {
    let root = tree_pairs(&[("a.dotfile", RAGGED)]);
    let output = dotfmt(root.path(), &["--check", "."], "");

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 1);
    assert_eq!(output.stdout, "");
    assert!(
        output.stderr.contains("needs format a.dotfile"),
        "{}",
        output.stderr
    );
    assert_eq!(
        fs::read_to_string(root.path().join("a.dotfile")).unwrap(),
        RAGGED
    );
}

#[test]
fn check_over_a_formatted_tree_says_so_and_succeeds() {
    let root = tree_pairs(&[("a.dotfile", LAID_OUT)]);
    let output = dotfmt(root.path(), &["--check", "."], "");

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 0);
    assert_eq!(output.stdout, "");
    assert_eq!(output.stderr, "1 file already formatted\n");
}

#[test]
fn the_check_summary_never_claims_to_have_formatted_anything() {
    // A check run writes nothing, so its summary must not use the verb a
    // writing run uses. "2 files formatted" after `--check` reads as though
    // the tree had just been rewritten, which is the one thing it was asked
    // not to do.
    let clean = tree_pairs(&[("a.dotfile", LAID_OUT), ("b.dotfile", LAID_OUT)]);
    let clean = dotfmt(clean.path(), &["--check", "."], "");
    assert_eq!(clean.code().expect("dotfmt exits rather than signals"), 0);
    assert_eq!(clean.stderr, "2 files already formatted\n");

    let drifted = tree_pairs(&[("a.dotfile", RAGGED), ("b.dotfile", LAID_OUT)]);
    let drifted = dotfmt(drifted.path(), &["--check", "."], "");
    assert_eq!(drifted.code().expect("dotfmt exits rather than signals"), 1);
    assert_eq!(
        drifted.stderr,
        "  needs format a.dotfile\n1 of 2 files need formatting\n"
    );

    // And the writing run is the only one that gets to say "formatted".
    let written = tree_pairs(&[("a.dotfile", RAGGED), ("b.dotfile", LAID_OUT)]);
    let written = dotfmt(written.path(), &["."], "");
    assert_eq!(written.code().expect("dotfmt exits rather than signals"), 0);
    assert_eq!(
        written.stderr,
        "  format a.dotfile\nformatted 1 of 2 files\n"
    );
}

#[test]
fn check_with_no_target_at_all_looks_at_the_current_directory() {
    let root = tree_pairs(&[("deep/a.dotfile", RAGGED)]);
    let output = dotfmt(root.path(), &["--check"], "");

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 1);
    assert!(
        output.stderr.contains("deep/a.dotfile"),
        "{}",
        output.stderr
    );
}

#[test]
fn a_run_formats_the_tree_and_names_what_it_changed() {
    let root = tree_pairs(&[("a.dotfile", RAGGED), ("b.toml", "untouched")]);
    let output = dotfmt(root.path(), &["."], "");

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 0);
    assert_eq!(output.stdout, "");
    assert_eq!(output.stderr, "  format a.dotfile\nformatted 1 of 1 file\n");
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

    let root = tree_pairs(&[("dotfmt.dotfile", ""), ("a.dotfile", RAGGED)]);
    let shut = root.path().join("shut");
    fs::create_dir(&shut).unwrap();
    fs::set_permissions(&shut, fs::Permissions::from_mode(0o000)).unwrap();

    let output = dotfmt(root.path(), &["."], "");
    let quiet = dotfmt(root.path(), &["-q", "."], "");
    fs::set_permissions(&shut, fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 0);
    let said = output.stderr;
    assert!(
        said.contains("dotfmt: 1 directory could not be read"),
        "{said}"
    );
    assert_eq!(quiet.stderr, "");
}

#[test]
fn formatting_twice_changes_nothing_the_second_time() {
    let root = tree_pairs(&[("a.dotfile", RAGGED)]);
    dotfmt(root.path(), &["."], "");
    let output = dotfmt(root.path(), &["."], "");

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 0);
    assert_eq!(output.stderr, "formatted 0 of 1 file\n");
    assert_eq!(
        fs::read_to_string(root.path().join("a.dotfile")).unwrap(),
        LAID_OUT
    );
}

#[test]
fn quiet_says_nothing_anywhere_about_a_run_that_changed_a_file() {
    let root = tree_pairs(&[("a.dotfile", RAGGED)]);
    let output = dotfmt(root.path(), &["-q", "."], "");

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 0);
    assert_eq!(output.stdout, "");
    assert_eq!(output.stderr, "");
    assert_eq!(
        fs::read_to_string(root.path().join("a.dotfile")).unwrap(),
        LAID_OUT
    );
}

#[test]
fn verbose_names_every_file_and_which_mode_a_conf_was_read_in() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", "include {\n  .conf\n}\n"),
        ("kitty/kitty.conf", "font_size  12"),
    ]);
    let output = dotfmt(root.path(), &["-v", "."], "");

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 0);
    let said = output.stderr;
    assert!(said.contains("config "), "{said}");
    assert!(said.contains("dotfmt.dotfile"), "{said}");
    assert!(said.contains("ok kitty/kitty.conf  kitty"), "{said}");
}

#[test]
fn one_bad_file_is_reported_and_the_rest_of_the_run_still_happens() {
    // `format.py` aborts the batch on the first odd path, so one bad argument
    // hides every other file's answer.
    let root = tree_pairs(&[
        ("a.dotfile", RAGGED),
        ("broken.dotfile", "a {\nb {\n}\n"),
        ("c.dotfile", RAGGED),
    ]);
    let output = dotfmt(root.path(), &["."], "");

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 1);
    let said = output.stderr;
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
    let root = tree_pairs(&[]);
    let output = dotfmt(root.path(), &["absent"], "");

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 1);
    assert_eq!(output.stdout, "");
    assert!(
        output.stderr.starts_with("dotfmt: absent:"),
        "{}",
        output.stderr
    );
}

#[test]
fn a_named_file_dotfmt_does_not_own_is_a_failure_rather_than_a_silent_skip() {
    let root = tree_pairs(&[("a.py", "x = 1\n")]);
    let output = dotfmt(root.path(), &["a.py"], "");

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 1);
    assert!(
        output
            .stderr
            .contains("not a .conf, .config or .dotfile file: a.py"),
        "{}",
        output.stderr
    );
}

#[test]
fn a_mistake_in_the_config_stops_the_run_and_names_the_line() {
    let root = tree_pairs(&[
        ("dotfmt.dotfile", "dotfmt {\n  indnet = 2\n}\n"),
        ("a.dotfile", RAGGED),
    ]);
    let output = dotfmt(root.path(), &["."], "");

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 1);
    assert!(
        output
            .stderr
            .contains("dotfmt.dotfile:2: unknown setting: indnet"),
        "{}",
        output.stderr
    );
    assert_eq!(
        fs::read_to_string(root.path().join("a.dotfile")).unwrap(),
        RAGGED
    );
}

#[test]
fn a_bad_flag_is_claps_own_failure() {
    let root = tree_pairs(&[]);
    let output = dotfmt(root.path(), &["--nonsense"], "");

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 2);
    assert_eq!(output.stdout, "");
}

#[test]
fn the_include_block_decides_which_files_a_walk_picks_up() {
    let root = tree_pairs(&[
        (
            "dotfmt.dotfile",
            "include {\n  .conf\n}\n\nexclude {\n  kitty\n}",
        ),
        ("a.conf", "x  =  1\n\n\n"),
        ("kitty/b.conf", "x  =  1\n\n\n"),
    ]);
    let output = dotfmt(root.path(), &["."], "");

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 0);
    assert_eq!(output.stderr, "  format a.conf\nformatted 1 of 2 files\n");
    assert_eq!(
        fs::read_to_string(root.path().join("a.conf")).unwrap(),
        "x  =  1"
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
    let root = tree_pairs(&[("a.conf", "x  =  1\n\n\n"), ("b.dotfile", RAGGED)]);
    let output = dotfmt(root.path(), &["."], "");

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 0);
    assert_eq!(output.stderr, "  format b.dotfile\nformatted 1 of 1 file\n");
    assert_eq!(
        fs::read_to_string(root.path().join("a.conf")).unwrap(),
        "x  =  1\n\n\n"
    );
}

#[test]
fn final_newline_governs_a_conf_file_as_well_as_a_dotfile() {
    for (setting, expected) in [("true", "x  =  1\n"), ("false", "x  =  1")] {
        let config =
            format!("dotfmt {{\n  final_newline = {setting}\n}}\n\ninclude {{\n  .conf\n}}\n");
        let root = tree_pairs(&[
            ("dotfmt.dotfile", config.as_str()),
            ("a.conf", "x  =  1\n\n\n"),
        ]);
        let output = dotfmt(root.path(), &["."], "");

        assert_eq!(output.code().expect("dotfmt exits rather than signals"), 0);
        assert_eq!(
            fs::read_to_string(root.path().join("a.conf")).unwrap(),
            expected,
            "final_newline = {setting}"
        );
    }
}

#[test]
fn a_named_file_the_config_leaves_alone_is_a_failure_rather_than_a_silent_skip() {
    let root = tree_pairs(&[("a.conf", "x=1\n")]);
    let output = dotfmt(root.path(), &["a.conf"], "");

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 1);
    assert!(
        output
            .stderr
            .contains("not selected by this config: a.conf"),
        "{}",
        output.stderr
    );
}

#[test]
fn owns_answers_on_stdout_with_the_paths_it_would_format() {
    let root = tree_pairs(&[
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

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 0);
    assert_eq!(
        output.stdout,
        "a.conf\0a.dotfile\0ssh/config.d/40-cabled\0deep/absent.conf\0"
    );
    assert_eq!(output.stderr, "");
}

#[test]
fn owns_reads_the_config_of_each_path_rather_than_one_for_all_of_them() {
    let root = tree_pairs(&[
        ("one/dotfmt.dotfile", "include {\n  .conf\n}\n"),
        ("one/a.conf", ""),
        ("two/a.conf", ""),
    ]);
    let output = dotfmt(root.path(), &["--owns"], "one/a.conf\0two/a.conf\0");

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 0);
    assert_eq!(output.stdout, "one/a.conf\0");
}

#[test]
fn owns_writes_nothing_at_all_when_a_config_will_not_parse() {
    // A half-answered question about ownership is worse than an unanswered
    // one: the caller cannot tell the files dotfmt declined from the ones it
    // never managed to consider.
    let root = tree_pairs(&[
        ("dotfmt.dotfile", "dotfmt {\n  indnet = 2\n}\n"),
        ("a.dotfile", ""),
    ]);
    let output = dotfmt(root.path(), &["--owns"], "a.dotfile\0");

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 1);
    assert_eq!(output.stdout, "");
    assert!(
        output.stderr.contains("unknown setting: indnet"),
        "{}",
        output.stderr
    );
}

#[test]
fn owns_answers_nothing_for_nothing_rather_than_reading_the_tree() {
    let root = tree_pairs(&[("a.dotfile", RAGGED)]);
    let output = dotfmt(root.path(), &["--owns"], "");

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 0);
    assert_eq!(output.stdout, "");
    assert_eq!(
        fs::read_to_string(root.path().join("a.dotfile")).unwrap(),
        RAGGED
    );
}

#[test]
fn owns_refuses_to_be_given_a_target_as_well() {
    let root = tree_pairs(&[]);
    let output = dotfmt(root.path(), &["--owns", "a.dotfile"], "");

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 2);
    assert_eq!(output.stdout, "");
}

#[test]
fn a_pattern_holding_an_equals_is_refused_rather_than_rewritten() {
    // The trap this rejection exists for: `block.rs` reads `a=b` as an entry,
    // and laying this very file out would write it back as `a  = b`. A run
    // must never edit a pattern into a different pattern.
    let config = "include {\n  a=b\n}\n";
    let root = tree_pairs(&[("dotfmt.dotfile", config), ("b.dotfile", RAGGED)]);
    let output = dotfmt(root.path(), &["."], "");

    assert_eq!(output.code().expect("dotfmt exits rather than signals"), 1);
    assert!(
        output.stderr.contains("a pattern cannot hold an ="),
        "{}",
        output.stderr
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
