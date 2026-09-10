use super::*;

fn local(root: &Path) -> Store {
    Store::new(
        &Destination {
            location: root.display().to_string(),
            ..Destination::default()
        },
        Path::new("rclone"),
        Duration::from_secs(10),
    )
    .unwrap()
}

#[test]
fn immutable_local_roundtrip_verifies_and_refuses_collisions() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::write(&source, b"original recovery data").unwrap();
    let store = local(&temp.path().join("objects"));
    let receipt = store
        .put_immutable("macie/uploads/id.dcloud", &source)
        .unwrap();
    assert!(receipt.verified);
    assert_eq!(receipt.bytes, 22);
    store
        .put_immutable("macie/uploads/id.dcloud", &source)
        .unwrap();
    fs::write(&source, b"changed recovery data!").unwrap();
    assert!(
        store
            .put_immutable("macie/uploads/id.dcloud", &source)
            .is_err()
    );
    let restored = temp.path().join("restored");
    store.get("macie/uploads/id.dcloud", &restored).unwrap();
    assert_eq!(fs::read(restored).unwrap(), b"original recovery data");
    let entries = store.list("macie/uploads").unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].key, "macie/uploads/id.dcloud");
    store.remove("macie/uploads/id.dcloud").unwrap();
    store.remove("macie/uploads/id.dcloud").unwrap();
    store.remove("unknown/uploads/missing.dcloud").unwrap();
    assert!(store.list("macie/uploads").unwrap().is_empty());
}

#[test]
fn malformed_keys_never_reach_storage() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("objects");
    let source = temp.path().join("source");
    fs::write(&source, "content").unwrap();
    let store = local(&root);
    for key in [
        "../outside",
        "/absolute",
        "x/../y",
        "a//b",
        "a/./b",
        "a\nb",
        "a\0b",
        "x;touch",
    ] {
        assert!(store.put_immutable(key, &source).is_err(), "{key:?}");
    }
    assert!(!root.exists());
}

#[cfg(unix)]
#[test]
fn capability_storage_refuses_symlink_escape_and_never_changes_outside_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("objects");
    let outside = temp.path().join("outside");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();
    let source = temp.path().join("source");
    fs::write(&source, "secret").unwrap();
    assert!(
        local(&root)
            .put_immutable("escape/stolen", &source)
            .is_err()
    );
    assert!(!outside.join("stolen").exists());
    assert!(local(&root).list("").is_err());
}

#[cfg(unix)]
#[test]
fn existing_symlink_object_is_not_followed() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("objects");
    fs::create_dir(&root).unwrap();
    let source = temp.path().join("source");
    fs::write(&source, "private").unwrap();
    std::os::unix::fs::symlink(&source, root.join("linked")).unwrap();
    let store = local(&root);
    assert!(store.put_immutable("linked", &source).is_err());
    assert!(store.get("linked", &temp.path().join("download")).is_err());
    assert!(store.remove("linked").is_err());
    assert_eq!(fs::read_to_string(source).unwrap(), "private");
}

#[test]
fn remote_paths_cannot_inject_shell_or_rclone_options() {
    for location in [
        "-oProxyCommand=evil:/backup",
        "host;evil:/backup",
        "host:/backup/../root",
        "host:/",
        "host:/backup\nrm",
    ] {
        assert!(sftp_location(location).is_err(), "{location}");
    }
    let script = remote_parent("/backup/it's data", "macie/uploads/id", true);
    assert!(script.contains("'/backup/it'\\''s data'"));
    for location in [":sftp,host=evil:/x", "remote:../x", "remote:x\ny"] {
        assert!(validate_remote(location).is_err());
    }
}

#[test]
fn ssh_upload_script_preserves_existing_objects_and_handles_quoted_roots() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("it's storage");
    let source = temp.path().join("source");
    fs::write(&source, "first").unwrap();
    for data in ["first", "second"] {
        fs::write(&source, data).unwrap();
        let script = upload_script(root.to_str().unwrap(), "macie/uploads/id").unwrap();
        let output = run_command(
            Command::new("sh")
                .args(["-c", &script])
                .stdin(File::open(&source).unwrap()),
            Duration::from_secs(10),
            "local SSH script test",
        )
        .unwrap();
        assert!(output.status.success());
    }
    assert_eq!(
        fs::read_to_string(root.join("macie/uploads/id")).unwrap(),
        "first"
    );
    assert_eq!(fs::read_dir(root.join("macie/uploads")).unwrap().count(), 1);
}

#[test]
fn upload_discovery_skips_repository_blob_trees_and_includes_unknown_hosts() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("objects");
    let source = temp.path().join("source");
    fs::write(&source, "archive").unwrap();
    let store = local(&root);
    for key in [
        "archie/uploads/a.manifest",
        "macie/uploads/b.manifest",
        "old-computer/uploads/c.manifest",
        "macie/Documents/data/blob",
    ] {
        store.put_immutable(key, &source).unwrap();
    }
    let entries = store.list_uploads(None).unwrap();
    assert_eq!(entries.len(), 3);
    assert!(entries.iter().all(|entry| entry.key.contains("/uploads/")));
    assert_eq!(store.list_uploads(Some("macie")).unwrap().len(), 1);
}

#[cfg(unix)]
#[test]
fn ssh_upload_script_rejects_a_symlink_parent() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("storage");
    let outside = temp.path().join("outside");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("macie")).unwrap();
    let script = upload_script(root.to_str().unwrap(), "macie/uploads/id").unwrap();
    assert!(
        run_command(
            Command::new("sh")
                .args(["-c", &script])
                .stdin(Stdio::null()),
            Duration::from_secs(2),
            "local SSH script test"
        )
        .is_err()
    );
    assert!(fs::read_dir(&outside).unwrap().next().is_none());
}
