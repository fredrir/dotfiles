use super::*;

fn fixture(root: &Path, name: &str) -> Restic {
    let password_file = root.join(format!("{name}.password"));
    fs::write(&password_file, "test-recovery-password-long-enough\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&password_file, fs::Permissions::from_mode(0o600)).unwrap();
    }
    Restic {
        binary: PathBuf::from("restic"),
        repository: root.join(name).display().to_string(),
        password_file,
        cache_dir: root.join(format!("{name}-cache")),
        bandwidth_kib: 0,
        read_concurrency: 2,
        timeout: Duration::from_secs(120),
        rclone: PathBuf::from("rclone"),
        rclone_config: None,
    }
}

#[test]
fn backup_copy_restore_and_pinned_retention() {
    if Command::new("restic").arg("version").output().is_err() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("important.txt"), b"durable backup fixture").unwrap();
    fs::write(source.join("excluded.tmp"), b"excluded").unwrap();
    let local = fixture(root.path(), "local");
    let remote = fixture(root.path(), "remote");
    local.init().unwrap();
    local.init().unwrap();
    remote.init().unwrap();
    assert!(
        local
            .backup(
                std::slice::from_ref(&source),
                "fixture-host",
                "documents",
                "documents",
                &[],
                &["*.tmp".into()],
                true
            )
            .unwrap()
            .is_none()
    );
    assert!(local.snapshots(None, None).unwrap().is_empty());
    let receipt = local
        .backup(
            std::slice::from_ref(&source),
            "fixture-host",
            "documents",
            "documents",
            &["personal".into()],
            &["*.tmp".into()],
            false,
        )
        .unwrap()
        .unwrap();
    let snapshots = local
        .snapshots(Some("fixture-host"), Some("documents"))
        .unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].labels(), ["personal"]);
    assert_eq!(receipt.total_files, 1);
    let copied = remote.copy_from(&local, &receipt.snapshot_id).unwrap();
    assert_eq!(
        remote.copy_from(&local, &receipt.snapshot_id).unwrap(),
        copied
    );
    remote.check(true, None).unwrap();
    let restored = root.path().join("restored");
    remote.restore(&copied, &restored, &[]).unwrap();
    let expected = restored
        .join(source.canonicalize().unwrap().strip_prefix("/").unwrap())
        .join("important.txt");
    assert_eq!(fs::read(expected).unwrap(), b"durable backup fixture");
    assert!(remote.restore(&copied, &restored, &[]).is_err());
    assert!(
        remote
            .ls(&copied)
            .unwrap()
            .iter()
            .any(|value| value.get("name").and_then(Value::as_str) == Some("important.txt"))
    );
    assert!(
        remote
            .stats(Some(&copied))
            .unwrap()
            .get("total_size")
            .is_some()
    );
    remote.pin(&copied, true).unwrap();
    let pinned = remote.snapshots(None, None).unwrap().remove(0);
    assert!(pinned.pinned());
    remote
        .set_tags(&copied, &["dcloud.label:reviewed".into()], &[])
        .unwrap();
    let relabeled = remote.snapshot(&copied).unwrap();
    assert!(relabeled.matches_id(&pinned.id));
    assert!(relabeled.pinned());
    let alias_restore = root.path().join("alias-restore");
    remote.restore(&pinned.id, &alias_restore, &[]).unwrap();
    assert!(
        remote
            .forget(std::slice::from_ref(&pinned.id), true)
            .is_err()
    );
    remote.pin(&pinned.id, false).unwrap();
    let unpinned = remote.snapshots(None, None).unwrap().remove(0);
    remote.forget(&[unpinned.id], true).unwrap();
    assert!(remote.snapshots(None, None).unwrap().is_empty());
}

#[test]
fn incremental_run_capture_is_completed_and_copies_new_content() {
    if Command::new("restic").arg("version").output().is_err() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("file.txt"), "version-one").unwrap();
    let local = fixture(root.path(), "local");
    let remote = fixture(root.path(), "remote");
    local.init().unwrap();
    remote.init().unwrap();
    let first = local
        .backup_run(
            std::slice::from_ref(&source),
            "host",
            "job",
            "category",
            &[],
            &[],
            &uuid::Uuid::new_v4().to_string(),
        )
        .unwrap()
        .unwrap();
    let snapshot = local.snapshot(&first.snapshot_id).unwrap();
    assert!(
        snapshot
            .tags
            .iter()
            .any(|tag| tag == "dcloud.capture:complete")
    );
    assert!(
        !snapshot
            .tags
            .iter()
            .any(|tag| tag == "dcloud.capture:pending")
    );
    remote.copy_from(&local, &first.snapshot_id).unwrap();
    fs::write(source.join("file.txt"), "version-two-is-different").unwrap();
    let second = local
        .backup_run(
            std::slice::from_ref(&source),
            "host",
            "job",
            "category",
            &[],
            &[],
            &uuid::Uuid::new_v4().to_string(),
        )
        .unwrap()
        .unwrap();
    assert_ne!(first.snapshot_id, second.snapshot_id);
    assert_ne!(
        local.snapshot(&first.snapshot_id).unwrap().tree,
        local.snapshot(&second.snapshot_id).unwrap().tree
    );
    let copied = remote.copy_from(&local, &second.snapshot_id).unwrap();
    let restored = root.path().join("selected");
    remote
        .restore(
            &copied,
            &restored,
            &[source.canonicalize().unwrap().join("file.txt")],
        )
        .unwrap();
    assert_eq!(
        fs::read_to_string(
            restored
                .join(source.canonicalize().unwrap().strip_prefix("/").unwrap())
                .join("file.txt")
        )
        .unwrap(),
        "version-two-is-different"
    );
    assert!(
        local
            .diff(&first.snapshot_id, &second.snapshot_id)
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item.get("modifier").is_some())
    );
}

#[cfg(unix)]
#[test]
fn incomplete_run_snapshot_remains_pending_and_cannot_be_copied() {
    use std::os::unix::fs::PermissionsExt;
    if Command::new("restic").arg("version").output().is_err() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("file"), "payload").unwrap();
    let mut local = fixture(root.path(), "local");
    let remote = fixture(root.path(), "remote");
    local.init().unwrap();
    remote.init().unwrap();
    let wrapper = root.path().join("restic-partial");
    fs::write(&wrapper, "#!/bin/sh\nrestic \"$@\"\nstatus=$?\nfor argument do\nif [ \"$argument\" = backup ]; then exit 3; fi\ndone\nexit \"$status\"\n").unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    local.binary = wrapper;
    assert!(
        local
            .backup_run(
                &[source],
                "host",
                "job",
                "category",
                &[],
                &[],
                &uuid::Uuid::new_v4().to_string()
            )
            .is_err()
    );
    let snapshot = local.snapshots(None, None).unwrap().remove(0);
    assert!(
        snapshot
            .tags
            .iter()
            .any(|tag| tag == "dcloud.capture:pending")
    );
    assert!(
        !snapshot
            .tags
            .iter()
            .any(|tag| tag == "dcloud.capture:complete")
    );
    assert!(remote.copy_from(&local, &snapshot.id).is_err());
}

#[cfg(unix)]
#[test]
fn partial_backup_exit_is_never_a_success() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let executable = root.path().join("restic-fixture");
    fs::write(&executable, "#!/bin/sh\nprintf '%s\\n' '{\"message_type\":\"summary\",\"snapshot_id\":\"aaaaaaaa\"}'\nexit 3\n").unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let mut engine = fixture(root.path(), "partial");
    engine.binary = executable;
    let result = engine.backup(
        &[root.path().to_path_buf()],
        "host",
        "job",
        "category",
        &[],
        &[],
        false,
    );
    assert!(result.unwrap_err().to_string().contains("incomplete"));
}

#[test]
fn destructive_commands_require_explicit_valid_snapshot_ids() {
    let root = tempfile::tempdir().unwrap();
    let engine = fixture(root.path(), "validation");
    assert!(engine.forget(&[], false).is_err());
    assert!(engine.forget(&["--all".into()], false).is_err());
    assert!(
        engine
            .restore("latest", &root.path().join("out"), &[])
            .is_err()
    );
    assert!(
        engine
            .backup(&[], "host", "job", "category", &[], &[], false)
            .is_err()
    );
    assert!(
        engine
            .backup(
                &[root.path().to_path_buf()],
                "host",
                "job",
                "category",
                &["one,two".into()],
                &[],
                false
            )
            .is_err()
    );
}
