#![forbid(unsafe_code)]

use clap::Parser;
use dotfile_cli::cli::{Resolution, SyncCli};

#[test]
fn sync_flags_compose() {
    let cli = SyncCli::try_parse_from([
        "dotfile sync",
        "macos",
        "-n",
        "-p",
        "-v",
        "--override",
        "shared=none",
        "--resolve",
        "live",
        "--to",
        "archie",
    ])
    .unwrap();
    assert_eq!(cli.profile.as_deref(), Some("macos"));
    assert!(cli.dry_run);
    assert!(cli.push);
    assert!(cli.verbose);
    assert_eq!(cli.overrides, ["shared=none"]);
    assert_eq!(cli.resolve, Resolution::Live);
    assert_eq!(cli.to.as_deref(), Some("archie"));
}

#[test]
fn force_and_resolution_are_exclusive() {
    let parsed = SyncCli::try_parse_from(["dotfile sync", "--force", "--resolve", "live"]);
    assert!(parsed.is_err());
}

#[cfg(unix)]
#[test]
fn command_help_needs_no_external_programs() {
    use testkit::Bin;
    let ran = Bin::new(env!("CARGO_BIN_EXE_dotfile"))
        .args(["secret", "--help"])
        .env("PATH", "")
        .run();
    assert!(ran.success(), "{}", ran.stderr);
    assert!(ran.stdout.contains("rekey"));
}

#[cfg(unix)]
#[test]
fn extension_dispatch_preserves_arguments_and_exit_status() {
    use testkit::{Bin, TempDir, executable};
    let directory = TempDir::new().unwrap();
    executable(
        &directory.path().join("dotfile-example"),
        "#!/bin/sh\nprintf '%s\\n' \"$@\"\nexit 7\n",
    );
    let ran = Bin::new(env!("CARGO_BIN_EXE_dotfile"))
        .args(["example", "two words", "--flag"])
        .env("PATH", directory.path())
        .run();
    assert_eq!(ran.code(), Some(7));
    assert_eq!(ran.stdout, "two words\n--flag\n");
}
