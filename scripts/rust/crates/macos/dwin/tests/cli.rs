#![forbid(unsafe_code)]
#![cfg(target_os = "macos")]

use testkit::{Bin, stderr};

fn dwin() -> Bin {
    Bin::new(env!("CARGO_BIN_EXE_dwin")).plain()
}

#[test]
fn an_unknown_window_is_not_found() {
    let output = dwin().arg("0").output();
    assert!(!output.status.success());
    assert_eq!(stderr(&output).trim(), "dwin: window not found");
}

#[test]
fn a_window_id_must_be_numeric() {
    let output = dwin().arg("zen").output();
    assert!(!output.status.success());
}
