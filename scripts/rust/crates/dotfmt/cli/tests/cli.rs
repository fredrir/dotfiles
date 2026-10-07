#![forbid(unsafe_code)]

use std::fs;

use testkit::{Bin, tree_pairs};

fn dotfmt() -> Bin {
    Bin::new(env!("CARGO_BIN_EXE_dotfmt"))
        .plain()
        .env("PATH", "")
        .env("DOTFILE_ROOT", "/no/checkout/needed")
}

#[test]
fn no_arguments_and_help_show_the_shell_and_all_public_flags() {
    for args in [vec![], vec!["--help"]] {
        let output = dotfmt().args(args).run();
        assert!(output.success(), "{}", output.stderr);
        assert!(output.stdout.contains("not implemented yet"));
        for flag in [
            "--check",
            "--add",
            "--sync",
            "--dialect",
            "--editor",
            "--stdin",
            "--owns",
            "--verbose",
            "--quiet",
            "--completions",
            "--version",
        ] {
            assert!(
                output.stdout.contains(flag),
                "missing {flag}: {}",
                output.stdout
            );
        }
    }
}

#[test]
fn version_completions_and_command_metadata_still_work() {
    let version = dotfmt().args(["--version"]).run();
    assert!(version.success());
    assert!(version.stdout.starts_with("dotfmt "));

    let completions = dotfmt().args(["--completions", "zsh"]).run();
    assert!(completions.success(), "{}", completions.stderr);
    assert!(completions.stdout.contains("#compdef dotfmt"));
    for flag in [
        "--add",
        "--sync",
        "--dialect",
        "--editor",
        "--stdin",
        "--owns",
    ] {
        assert!(completions.stdout.contains(flag), "missing {flag}");
    }

    let metadata = dotfmt().args(["--command-dump"]).run();
    assert!(metadata.success(), "{}", metadata.stderr);
    assert!(metadata.stdout.contains("\"dotfmt\""));
    assert!(metadata.stdout.contains("\"--dialect\""));
}

#[test]
fn placeholder_actions_leave_files_and_configuration_untouched() {
    let root = tree_pairs(&[("a.py", "x=1\n"), ("ruff.toml", "existing configuration\n")]);
    for args in [
        vec!["."],
        vec!["--check", "."],
        vec!["--add", "."],
        vec!["--sync", "."],
        vec!["--dialect", "future-dialect", "."],
        vec!["--editor"],
        vec!["--stdin", "input.lua"],
        vec!["--owns"],
        vec!["-q", "."],
    ] {
        let output = dotfmt()
            .args(args)
            .current_dir(root.path())
            .stdin("input")
            .run();
        assert_eq!(output.code(), Some(1), "{}", output.stderr);
        assert!(output.stdout.is_empty());
        assert!(
            output.stderr.contains("not implemented yet"),
            "{}",
            output.stderr
        );
        assert_eq!(
            fs::read_to_string(root.path().join("a.py")).unwrap(),
            "x=1\n"
        );
        assert_eq!(
            fs::read_to_string(root.path().join("ruff.toml")).unwrap(),
            "existing configuration\n"
        );
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
    }
}

#[test]
fn placeholders_do_not_resolve_the_target_or_read_stdin() {
    let output = dotfmt().args(["--check", "/no/such/target"]).run();
    assert_eq!(output.code(), Some(1));
    assert!(output.stderr.contains("not implemented yet"));
    assert!(!output.stderr.contains("no such file"));

    let output = dotfmt()
        .args(["--stdin", "/no/such/input.md"])
        .stdin("unchanged input")
        .run();
    assert_eq!(output.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.contains("not implemented yet"));
}

#[test]
fn verbose_retains_the_heading_without_claiming_to_have_formatted_files() {
    let output = dotfmt().args(["-v", "--check", "project"]).run();
    assert_eq!(output.code(), Some(1));
    assert!(
        output.stderr.starts_with("\n  dotfmt  project  check\n\n"),
        "{}",
        output.stderr
    );
    assert!(output.stderr.contains("not implemented yet"));
    assert!(!output.stderr.contains("files formatted"));
}

#[test]
fn conflicting_flags_remain_usage_errors() {
    for args in [
        ["--check", "--add"],
        ["--check", "--sync"],
        ["--add", "--sync"],
        ["--verbose", "--quiet"],
    ] {
        assert_eq!(dotfmt().args(args).run().code(), Some(2));
    }
}
