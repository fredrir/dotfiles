#![forbid(unsafe_code)]

use std::path::Path;

use testkit::{Bin, Ran, tree};

fn zcomp(root: &Path, args: &[&str]) -> Ran {
    Bin::new(env!("CARGO_BIN_EXE_zcomp"))
        .args(args)
        .current_dir(root)
        .env("HOME", root)
        .env("PWD", root)
        .env("PATH", root.join("bin"))
        .env("ZCOMP_CACHE_DIR", root.join("cache"))
        .env("ZCOMP_OFFLINE", "1")
        .env("ZCOMP_FOREGROUND", "1")
        .run()
}

#[test]
fn the_zsh_completions_carry_the_integration() {
    let root = tree(&[]);
    let ran = zcomp(root.path(), &["--completions", "zsh"]);
    assert!(ran.status.success(), "{}", ran.stderr);
    assert!(ran.stdout.contains("#compdef zcomp"), "{}", ran.stdout);
    assert!(ran.stdout.contains("compdef _zcomp_complete \"$command\""));
    assert!(
        ran.stdout
            .contains("command zcomp complete --command=\"$service\"")
    );
}

#[test]
fn candidates_are_printed_for_the_word_under_the_cursor() {
    let root = tree(&["package.json={\"scripts\":{\"build\":\"tsc -b\"}}", "bin/"]);
    let ran = zcomp(
        root.path(),
        &[
            "complete",
            "--command=npm",
            "--current=3",
            "--prefix=",
            "--",
            "npm",
            "run",
            "",
        ],
    );
    assert!(ran.status.success(), "{}", ran.stderr);
    assert_eq!(
        ran.stdout,
        "group\tscripts\tscript\tlines\tunsorted\nitem\tbuild\tbuild  tsc -b\n"
    );
}

#[test]
fn a_prefix_that_looks_like_a_flag_is_still_a_prefix() {
    let root = tree(&["bin/"]);
    let ran = zcomp(
        root.path(),
        &[
            "complete",
            "--command=npm",
            "--current=3",
            "--prefix=--omit=",
            "--",
            "npm",
            "i",
            "--omit=",
        ],
    );
    assert!(ran.status.success(), "{}", ran.stderr);
    assert!(ran.stdout.starts_with("skip\t--omit=\n"), "{}", ran.stdout);
}

#[test]
fn warming_without_any_tool_does_nothing() {
    let root = tree(&["bin/"]);
    let ran = zcomp(root.path(), &["warm"]);
    assert!(ran.status.success(), "{}", ran.stderr);
    assert!(!root.path().join("cache").exists());
}

#[test]
fn an_unknown_refresh_source_fails() {
    let root = tree(&["bin/"]);
    let ran = zcomp(root.path(), &["refresh", "nothing"]);
    assert_eq!(ran.code(), Some(1));
    assert!(
        ran.stderr.contains("could not rebuild nothing"),
        "{}",
        ran.stderr
    );
}
