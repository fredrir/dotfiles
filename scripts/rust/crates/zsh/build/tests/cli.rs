#![forbid(unsafe_code)]

use testkit::{Bin, tree};

#[test]
fn the_zsh_completions_are_printed() {
    let ran = Bin::new(env!("CARGO_BIN_EXE_zsh-build"))
        .args(["--completions", "zsh"])
        .run();
    assert!(ran.success(), "{}", ran.stderr);
    assert!(ran.stdout.contains("#compdef zsh-build"), "{}", ran.stdout);
}

#[test]
fn a_missing_config_is_an_error() {
    let root = tree(&[]);
    let ran = Bin::new(env!("CARGO_BIN_EXE_zsh-build"))
        .arg("--root")
        .arg(root.path())
        .run();
    assert!(!ran.success());
    assert!(
        ran.stderr.contains("config/zsh/build.toml"),
        "{}",
        ran.stderr
    );
}
