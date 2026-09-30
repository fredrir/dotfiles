#![forbid(unsafe_code)]
#![cfg(unix)]

use std::fs;
use std::path::Path;

use testkit::{Bin, Ran, executable, tree};

fn prepare(root: &Path, picker: &str) {
    let shim = Bin::new(env!("CARGO_BIN_EXE_zcomp"))
        .args(["--completions", "zsh"])
        .run();
    assert!(shim.success(), "{shim:?}");
    fs::write(root.join("completion.zsh"), shim.stdout).unwrap();
    executable(&root.join("bin/fzf"), picker);
}

fn pick(root: &Path, input: &str, query: &str) -> Ran {
    let fzf = Bin::new("zsh")
        .args(["-dfc", "print -r -- $commands[fzf]"])
        .run();
    assert!(fzf.success() && !fzf.stdout.trim().is_empty(), "{fzf:?}");
    let mut paths = vec![root.join("bin")];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    Bin::new("zsh")
        .args([
            "-dfc",
            r#"
compdef() {}
defer() {}
source "$ZCOMP_TEST_ROOT/completion.zsh"
_zcomp_fzf --ansi --delimiter='\x00' --nth=2,3 --no-sort --filter="$ZCOMP_TEST_QUERY"
"#,
        ])
        .env("ZCOMP_TEST_ROOT", root)
        .env("ZCOMP_TEST_QUERY", query)
        .env("ZCOMP_TEST_FZF", fzf.stdout.trim())
        .env("PATH", std::env::join_paths(paths).unwrap())
        .stdin(input)
        .run()
}

#[test]
fn colored_selections_return_the_original_rows_for_fzf_tab_to_insert() {
    let root = tree(&["bin/"]);
    prepare(
        root.path(),
        "#!/bin/sh\nprintf '%s\\n\\n' \"$ZCOMP_TEST_QUERY\"\nexec \"$ZCOMP_TEST_FZF\" \"$@\"\n",
    );
    let editor = "\x1b[32m\0\x1b[36mcod-editor\x1b[0m  \x1b[34m116/mo\x1b[0m  editor [tools]\0\n";
    let scoped = "\x1b[33m\0\x1b[92m@\x1b[0m\x1b[34macme\x1b[0m/\x1b[36mcod-tools\x1b[0m  工具\0\n";
    let other = "\x1b[32m\0other  another package\0\n";
    let input = format!("{editor}{scoped}{other}");
    for (query, rows) in [
        ("cod-editor", editor.to_string()),
        ("cod", format!("{editor}{scoped}")),
    ] {
        let ran = pick(root.path(), &input, query);
        assert!(ran.success(), "{ran:?}");
        assert_eq!(ran.stdout, format!("{query}\n\n{rows}"));
    }
}

#[test]
fn plain_selections_and_the_query_and_expect_key_are_preserved() {
    let root = tree(&["bin/"]);
    prepare(
        root.path(),
        "#!/bin/sh\nprintf '%s\\n/\\n' \"$ZCOMP_TEST_QUERY\"\nexec \"$ZCOMP_TEST_FZF\" \"$@\"\n",
    );
    let row = "\0cod-editor  116/mo  editor\0\n";
    let ran = pick(root.path(), row, "cod-editor");
    assert!(ran.success(), "{ran:?}");
    assert_eq!(ran.stdout, format!("cod-editor\n/\n{row}"));
}

#[test]
fn accepting_a_query_without_a_selection_preserves_the_query() {
    let root = tree(&["bin/"]);
    prepare(
        root.path(),
        "#!/bin/sh\nprintf '%s\\n' \"$ZCOMP_TEST_QUERY\"\n",
    );
    let ran = pick(root.path(), "\0cod-editor\0\n", "new-package");
    assert!(ran.success(), "{ran:?}");
    assert_eq!(ran.stdout, "new-package\n");
}

#[test]
fn cancelling_the_picker_returns_no_selection_and_keeps_the_exit_status() {
    let root = tree(&["bin/"]);
    prepare(root.path(), "#!/bin/sh\nexit 130\n");
    let ran = pick(root.path(), "\0cod-editor\0\n", "cod");
    assert_eq!(ran.code(), Some(130), "{ran:?}");
    assert_eq!(ran.stdout, "");
}
