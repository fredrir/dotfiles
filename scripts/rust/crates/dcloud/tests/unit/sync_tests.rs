use super::*;

fn pair(root: &Path) -> SyncPair {
    SyncPair {
        owner: "macie".into(),
        left: root.join("left").display().to_string(),
        right: root.join("right").display().to_string(),
        backup_left: root.join("left-backup").display().to_string(),
        backup_right: root.join("right-backup").display().to_string(),
        ..SyncPair::default()
    }
}

#[test]
fn nested_sync_or_backup_roots_are_refused() {
    let temp = tempfile::tempdir().unwrap();
    let original = pair(temp.path());
    assert!(normalized(&original).is_ok());
    let mut overlapping = original.clone();
    overlapping.right = format!("{}/nested", overlapping.left);
    assert!(normalized(&overlapping).is_err());
    overlapping = original;
    overlapping.backup_left = format!("{}/backup", overlapping.left);
    assert!(normalized(&overlapping).is_err());
}

#[test]
fn topology_changes_require_explicit_initialization() {
    let temp = tempfile::tempdir().unwrap();
    let mut configuration = pair(temp.path());
    let initial = fingerprint(&configuration).unwrap();
    configuration.exclude.push("*.tmp".into());
    assert_ne!(initial, fingerprint(&configuration).unwrap());
    configuration.exclude.clear();
    configuration.owner = "archie".into();
    assert_ne!(initial, fingerprint(&configuration).unwrap());
}

#[test]
fn normal_sync_never_forces_or_resyncs_and_preserves_both_conflicts() {
    let temp = tempfile::tempdir().unwrap();
    let arguments = arguments(
        &pair(temp.path()),
        temp.path(),
        &temp.path().join("filters"),
        "ACCESS",
        "/backup-left/run",
        "/backup-right/run",
        false,
        true,
    );
    let arguments: Vec<_> = arguments.iter().map(|s| s.to_string_lossy()).collect();
    for flag in [
        "--check-access",
        "--recover",
        "--backup-dir1",
        "--backup-dir2",
    ] {
        assert!(arguments.iter().any(|arg| arg == flag));
    }
    for flag in [
        "--force",
        "--resync",
        "--resync-mode",
        "--max-lock",
        "--delete-excluded",
    ] {
        assert!(!arguments.iter().any(|arg| arg == flag));
    }
    assert!(
        arguments
            .windows(2)
            .any(|args| args == ["--conflict-resolve", "none"])
    );
    assert!(
        arguments
            .windows(2)
            .any(|args| args == ["--conflict-loser", "num"])
    );
}

#[test]
fn sync_lock_prevents_overlapping_invocations() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("lock");
    let locked = SyncLock::acquire(&path).unwrap();
    assert!(SyncLock::acquire(&path).is_err());
    drop(locked);
    assert!(SyncLock::acquire(&path).is_ok());
}

#[cfg(unix)]
#[test]
fn direct_ssh_remote_requires_encryption_when_vps_flag_is_omitted() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("rclone");
    fs::write(
        &executable,
        "#!/bin/sh\nprintf '%s\\n' '[{\"name\":\"plain\",\"type\":\"sftp\"}]'\n",
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = Config::default();
    config.tools.rclone = executable;
    let error =
        validate_backend(&config, "plain:directory", false, Duration::from_secs(10)).unwrap_err();
    assert!(error.to_string().contains("encrypted crypt"));
}

#[cfg(unix)]
#[test]
fn wrapper_remotes_cannot_hide_an_unencrypted_ssh_backend() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("rclone");
    let mut config = Config::default();
    config.tools.rclone = executable.clone();
    for kind in ["alias", "union", "combine", "chunker", "webdav"] {
        fs::write(
            &executable,
            format!("#!/bin/sh\nprintf '%s\\n' '[{{\"name\":\"wrapped\",\"type\":\"{kind}\"}}]'\n"),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let error = validate_backend(&config, "wrapped:directory", false, Duration::from_secs(10))
            .unwrap_err();
        assert!(error.to_string().contains("encrypted crypt"));
    }
    for kind in ["drive", "local", "crypt"] {
        fs::write(
            &executable,
            format!("#!/bin/sh\nprintf '%s\\n' '[{{\"name\":\"allowed\",\"type\":\"{kind}\"}}]'\n"),
        )
        .unwrap();
        validate_backend(&config, "allowed:directory", false, Duration::from_secs(10)).unwrap();
    }
}

#[test]
fn crypt_sync_cannot_disable_content_encryption_or_accept_corrupted_blocks() {
    if Command::new("rclone").arg("version").output().is_err() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let left = temp.path().join("left");
    let encrypted = temp.path().join("encrypted");
    fs::create_dir(&left).unwrap();
    fs::create_dir(&encrypted).unwrap();
    fs::write(left.join("file.txt"), "private fixture contents").unwrap();
    let password = Command::new("rclone")
        .args(["obscure", "fixture encryption key"])
        .output()
        .unwrap();
    assert!(password.status.success());
    let rclone_config = temp.path().join("rclone.conf");
    fs::write(&rclone_config, format!("[protected]\ntype = crypt\nremote = {}\nfilename_encryption = off\npassword = {}\nno_data_encryption = true\npass_bad_blocks = true\n", encrypted.display(), String::from_utf8(password.stdout).unwrap().trim())).unwrap();
    let pair = SyncPair {
        owner: "macie".into(),
        left: left.display().to_string(),
        right: "protected:sync".into(),
        right_vps: true,
        backup_left: temp.path().join("backup").display().to_string(),
        backup_right: "protected:backups".into(),
        ..SyncPair::default()
    };
    let config = Config {
        host: "macie".into(),
        state_dir: temp.path().join("state"),
        rclone_config_file: Some(rclone_config),
        sync: [("crypt".into(), pair)].into(),
        ..Config::default()
    };
    run(&config, "crypt", true, true).unwrap();
    let data = fs::read(encrypted.join("sync/file.txt.bin")).unwrap();
    assert!(data.starts_with(b"RCLONE"));
    assert!(
        !data
            .windows(b"private fixture contents".len())
            .any(|window| window == b"private fixture contents")
    );
    let command = rclone_command(&Config::default());
    let args: Vec<_> = command
        .get_args()
        .map(|value| value.to_string_lossy())
        .collect();
    assert!(
        args.iter()
            .any(|argument| argument == "--crypt-no-data-encryption=false")
    );
    assert!(
        args.iter()
            .any(|argument| argument == "--crypt-pass-bad-blocks=false")
    );
}

#[test]
fn actual_local_bisync_previews_initializes_and_syncs_both_directions() {
    if Command::new("rclone").arg("version").output().is_err() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let pair = pair(temp.path());
    fs::create_dir(&pair.left).unwrap();
    fs::create_dir(&pair.right).unwrap();
    fs::write(Path::new(&pair.left).join("left.txt"), "left original").unwrap();
    fs::write(Path::new(&pair.right).join("right.txt"), "right original").unwrap();
    let mut config = Config {
        host: "macie".into(),
        state_dir: temp.path().join("state"),
        ..Config::default()
    };
    config.sync.insert("test".into(), pair.clone());
    run(&config, "test", true, false).unwrap();
    assert!(!Path::new(&pair.right).join("left.txt").exists());
    assert!(!Path::new(&pair.left).join("right.txt").exists());
    run(&config, "test", true, true).unwrap();
    assert_eq!(
        fs::read_to_string(Path::new(&pair.right).join("left.txt")).unwrap(),
        "left original"
    );
    fs::write(Path::new(&pair.left).join("new-left.txt"), "new left").unwrap();
    fs::write(Path::new(&pair.right).join("new-right.txt"), "new right").unwrap();
    run(&config, "test", false, true).unwrap();
    assert_eq!(
        fs::read_to_string(Path::new(&pair.right).join("new-left.txt")).unwrap(),
        "new left"
    );
    assert_eq!(
        fs::read_to_string(Path::new(&pair.left).join("new-right.txt")).unwrap(),
        "new right"
    );
    fs::write(
        Path::new(&pair.left).join("left.txt"),
        "left concurrent edit",
    )
    .unwrap();
    fs::write(
        Path::new(&pair.right).join("left.txt"),
        "right concurrent edit",
    )
    .unwrap();
    run(&config, "test", false, true).unwrap();
    for side in [&pair.left, &pair.right] {
        let contents: Vec<_> = fs::read_dir(side)
            .unwrap()
            .map(|entry| fs::read_to_string(entry.unwrap().path()).unwrap())
            .collect();
        assert!(
            contents
                .iter()
                .any(|content| content == "left concurrent edit")
        );
        assert!(
            contents
                .iter()
                .any(|content| content == "right concurrent edit")
        );
    }
    let marker = fs::read_dir(&pair.left)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".dcloud-access-")
        })
        .unwrap();
    fs::remove_file(marker).unwrap();
    assert!(run(&config, "test", false, true).is_err());
    assert!(Path::new(&pair.right).join("right.txt").exists());
}

#[test]
fn actual_sync_preserves_deleted_files_and_refuses_mass_deletion() {
    if Command::new("rclone").arg("version").output().is_err() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let pair = pair(temp.path());
    fs::create_dir(&pair.left).unwrap();
    fs::create_dir(&pair.right).unwrap();
    for index in 0..20 {
        fs::write(
            Path::new(&pair.left).join(format!("file-{index}.txt")),
            format!("original {index}"),
        )
        .unwrap();
    }
    let mut config = Config {
        host: "macie".into(),
        state_dir: temp.path().join("state"),
        ..Config::default()
    };
    config.sync.insert("deletes".into(), pair.clone());
    run(&config, "deletes", true, true).unwrap();
    fs::remove_file(Path::new(&pair.left).join("file-0.txt")).unwrap();
    let result = run(&config, "deletes", false, true).unwrap();
    assert!(!Path::new(&pair.right).join("file-0.txt").exists());
    let saved = Path::new(result["backup_right"].as_str().unwrap()).join("file-0.txt");
    assert_eq!(fs::read_to_string(saved).unwrap(), "original 0");
    for index in 1..12 {
        fs::remove_file(Path::new(&pair.left).join(format!("file-{index}.txt"))).unwrap();
    }
    assert!(run(&config, "deletes", false, true).is_err());
    for index in 1..12 {
        assert!(
            Path::new(&pair.right)
                .join(format!("file-{index}.txt"))
                .exists()
        );
    }
}

#[test]
fn scheduled_sync_does_not_relock_or_repeat_a_completed_occurrence() {
    if Command::new("rclone").arg("version").output().is_err() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let pair = pair(temp.path());
    fs::create_dir(&pair.left).unwrap();
    fs::create_dir(&pair.right).unwrap();
    fs::write(Path::new(&pair.left).join("file.txt"), "test").unwrap();
    let mut config = Config {
        host: "macie".into(),
        state_dir: temp.path().join("state"),
        ..Config::default()
    };
    config.sync.insert("scheduled".into(), pair.clone());
    run(&config, "scheduled", true, true).unwrap();
    let mut state = crate::state::State::open(&config.state_dir).unwrap();
    let first = run_due(&config, &mut state).unwrap();
    assert!(first["errors"].as_array().unwrap().is_empty(), "{first}");
    assert_eq!(first["pairs"].as_array().unwrap().len(), 1);
    let second = run_due(&config, &mut state).unwrap();
    assert!(second["errors"].as_array().unwrap().is_empty());
    assert!(second["pairs"].as_array().unwrap().is_empty());
}
