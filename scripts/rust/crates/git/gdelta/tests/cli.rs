#![forbid(unsafe_code)]

use std::env;
use std::path::Path;
use std::process::Output;

use testkit::{Bin, TempDir, executable, stdout};

const FAKE_DELTA: &str =
    "#!/bin/sh\nprintf '<delta %s>\\n' \"$*\"\ncat\nexit \"${FAKE_DELTA_EXIT:-0}\"\n";

const ADDED: &str = "diff --git a/new b/new\n@@ -0,0 +1 @@\n+new\n";
const CHANGED: &str = "diff --git a/a b/a\n@@ -1 +1 @@\n-before\n+after\n";
const ALSO_CHANGED: &str = "diff --git a/b b/b\n@@ -1 +1 @@\n-old\n+new\n";
const RENAMED: &str = "diff --git a/x b/y\nrename from x\nrename to y\n";
const REMOVED: &str = "diff --git a/old b/old\n@@ -1 +0,0 @@\n-old\n";

fn gdelta(fakes: &Path, input: &str, exit: &str) -> Output {
    let path = env::join_paths(
        [fakes.to_path_buf()]
            .into_iter()
            .chain(env::split_paths(&env::var_os("PATH").unwrap_or_default())),
    )
    .unwrap();
    Bin::new(env!("CARGO_BIN_EXE_gdelta"))
        .args(["--dark", "--width=80"])
        .env("PATH", path)
        .env("FAKE_DELTA_EXIT", exit)
        .stdin(input)
        .output()
}

fn fakes() -> TempDir {
    let dir = TempDir::new().unwrap();
    executable(&dir.path().join("delta"), FAKE_DELTA);
    dir
}

#[test]
fn neighbouring_files_with_the_same_layout_share_a_delta_run() {
    let fakes = fakes();
    let input = [
        "commit abc\n\n",
        ADDED,
        CHANGED,
        RENAMED,
        ALSO_CHANGED,
        REMOVED,
    ]
    .concat();

    let output = gdelta(fakes.path(), &input, "0");

    assert!(output.status.success());
    assert_eq!(
        stdout(&output),
        [
            "<delta --dark --width=80>\n",
            "commit abc\n\n",
            ADDED,
            "<delta --dark --width=80 --features=split>\n",
            CHANGED,
            RENAMED,
            ALSO_CHANGED,
            "<delta --dark --width=80>\n",
            REMOVED,
        ]
        .concat()
    );
}

#[test]
fn text_without_changes_still_goes_through_delta() {
    let fakes = fakes();

    let output = gdelta(fakes.path(), "commit abc\n", "0");

    assert_eq!(stdout(&output), "<delta --dark --width=80>\ncommit abc\n");
}

#[test]
fn delta_failing_fails_the_run() {
    let fakes = fakes();

    let output = gdelta(fakes.path(), CHANGED, "3");

    assert_eq!(output.status.code(), Some(3));
}
