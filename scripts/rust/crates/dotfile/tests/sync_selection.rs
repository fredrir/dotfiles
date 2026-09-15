#![forbid(unsafe_code)]

use std::ffi::OsString;

use dotfile_cli::sync::selection::normalize;

fn normalized(arguments: &[&str]) -> Vec<String> {
    normalize(arguments.iter().map(OsString::from).collect())
        .into_iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn a_profile_spelled_as_a_flag_becomes_the_positional() {
    assert_eq!(normalized(&["--macos"]), ["macos"]);
    assert_eq!(
        normalized(&["--arch-linux/hyprland", "-n"]),
        ["arch-linux/hyprland", "-n"]
    );
}

#[test]
fn every_real_sync_flag_survives() {
    let flags = [
        "--dry-run",
        "--override",
        "--force",
        "--resolve",
        "--push",
        "--to",
        "--verbose",
        "--commands-only",
        "--native-only",
        "--rebuild",
        "--help",
        "--version",
    ];
    for flag in flags {
        assert_eq!(normalized(&[flag]), [flag], "{flag} was read as a profile");
    }
}

#[test]
fn values_and_short_flags_are_untouched() {
    assert_eq!(
        normalized(&["--override", "linux/hyprland=none", "-n", "-v"]),
        ["--override", "linux/hyprland=none", "-n", "-v"]
    );
    assert_eq!(normalized(&["--override=a=b"]), ["--override=a=b"]);
}

#[test]
fn the_old_separator_is_accepted_and_dropped() {
    assert_eq!(normalized(&["--macos", "--", "-n"]), ["macos", "-n"]);
}
