use std::fs;
use std::process::Command;

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
fn unrelated_commands_exec_the_private_backend() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let backend = directory.path().join("dotfile-py");
    fs::write(&backend, "#!/bin/sh\nprintf '%s' \"$*\"\nexit 7\n").unwrap();
    fs::set_permissions(&backend, fs::Permissions::from_mode(0o755)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dotfile"))
        .args(["secret", "status", "--all"])
        .env("DOTFILE_PYTHON", &backend)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "secret status --all"
    );
}
