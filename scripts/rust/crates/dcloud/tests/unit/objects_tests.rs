use super::*;
use crate::config::Destination;

fn fixture(root: &Path) -> (Config, State, PathBuf) {
    let mut config = Config {
        host: "fixture".into(),
        state_dir: root.join("state"),
        password_file: root.join("repository.key"),
        identity_file: root.join("identity.txt"),
        ..Config::default()
    };
    let (identity, recipient) = archive::generate_identity();
    fs::write(&config.identity_file, identity).unwrap();
    crate::setup::write_private_new(&config.password_file, b"0123456789abcdef0123456789abcdef\n")
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&config.identity_file, fs::Permissions::from_mode(0o600)).unwrap();
    }
    config.recipients = vec![recipient];
    config.destinations.insert(
        "plain".into(),
        Destination {
            location: root.join("plain").display().to_string(),
            encrypted: false,
            ..Destination::default()
        },
    );
    config.destinations.insert(
        "encrypted".into(),
        Destination {
            location: root.join("encrypted").display().to_string(),
            encrypted: true,
            ..Destination::default()
        },
    );
    let source = root.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("report.txt"), "original upload").unwrap();
    let state = State::open(&config.state_dir).unwrap();
    (config, state, source)
}

#[test]
fn upload_to_plain_and_encrypted_destinations_downloads_and_catalogs_same_id() {
    let root = tempfile::tempdir().unwrap();
    let (config, mut state, source) = fixture(root.path());
    let result = upload(
        &config,
        &mut state,
        &source,
        &["plain".into(), "encrypted".into()],
        "documents",
        &["personal".into()],
        None,
        false,
    )
    .unwrap();
    let id = result["id"].as_str().unwrap();
    let plain = manifests(&config, "plain", Some(&config.host)).unwrap();
    let encrypted = manifests(&config, "encrypted", Some(&config.host)).unwrap();
    assert_eq!(plain[0].id, id);
    assert_eq!(encrypted[0].id, id);
    assert!(!plain[0].encrypted);
    assert!(encrypted[0].encrypted);
    assert!(!config.state_dir.join("uploads").join(id).exists());
    for from in ["plain", "encrypted"] {
        let restored = root.path().join(format!("restored-{from}"));
        download(&config, id, &config.host, from, &restored, &[]).unwrap();
        assert_eq!(
            fs::read_to_string(restored.join("report.txt")).unwrap(),
            "original upload"
        );
    }
    fs::remove_file(&config.identity_file).unwrap();
    download(
        &config,
        id,
        &config.host,
        "plain",
        &root.path().join("plain-no-key"),
        &[],
    )
    .unwrap();
    assert!(
        download(
            &config,
            id,
            &config.host,
            "encrypted",
            &root.path().join("encrypted-no-key"),
            &[]
        )
        .is_err()
    );
}

#[test]
fn retry_keeps_original_archive_when_source_changes_after_partial_replication() {
    let root = tempfile::tempdir().unwrap();
    let (config, mut state, source) = fixture(root.path());
    let failed_root = PathBuf::from(&config.destinations["encrypted"].location);
    fs::write(&failed_root, "temporarily unavailable directory").unwrap();
    assert!(
        upload(
            &config,
            &mut state,
            &source,
            &["plain".into(), "encrypted".into()],
            "documents",
            &[],
            None,
            false
        )
        .is_err()
    );
    let before = state.runs().unwrap().remove(0);
    assert_eq!(before.replicas["plain"].state, ReplicaState::Verified);
    assert_eq!(before.state, RunState::Failed);
    fs::write(source.join("report.txt"), "new source version").unwrap();
    fs::remove_file(&failed_root).unwrap();
    fs::create_dir(&failed_root).unwrap();
    retry(&config, &mut state, Some(&before.id)).unwrap();
    let after = state.load_run(&before.id).unwrap().unwrap();
    assert_eq!(after.state, RunState::Committed);
    let restored = root.path().join("restored");
    download(
        &config,
        &before.id,
        &config.host,
        "encrypted",
        &restored,
        &[],
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(restored.join("report.txt")).unwrap(),
        "original upload"
    );
    assert_eq!(
        fs::read_to_string(source.join("report.txt")).unwrap(),
        "new source version"
    );
}

#[test]
fn spool_limit_prevents_publishing_and_persists_failure() {
    let root = tempfile::tempdir().unwrap();
    let (mut config, mut state, source) = fixture(root.path());
    config.uploads.spool_limit_bytes = 64;
    assert!(
        upload(
            &config,
            &mut state,
            &source,
            &["plain".into()],
            "documents",
            &[],
            None,
            false
        )
        .is_err()
    );
    let run = state.runs().unwrap().remove(0);
    assert_eq!(run.state, RunState::Failed);
    assert!(run.error.is_some());
    assert!(manifests(&config, "plain", None).unwrap().is_empty());
    assert_eq!(spool_size(&config.state_dir.join("uploads")).unwrap(), 0);
}

#[test]
fn encrypted_destination_rejects_plaintext_archive_on_download_and_catalog() {
    let root = tempfile::tempdir().unwrap();
    let (config, mut state, source) = fixture(root.path());
    let result = upload(
        &config,
        &mut state,
        &source,
        &["plain".into()],
        "documents",
        &[],
        None,
        false,
    )
    .unwrap();
    let id = result["id"].as_str().unwrap();
    let key = key(&config.host, id).unwrap();
    let plaintext_root = PathBuf::from(&config.destinations["plain"].location);
    let target = store(&config, "encrypted").unwrap();
    target
        .put_immutable(&key, &plaintext_root.join(&key))
        .unwrap();
    target
        .put_immutable(
            &format!("{key}.manifest"),
            &plaintext_root.join(format!("{key}.manifest")),
        )
        .unwrap();
    assert!(manifests(&config, "encrypted", None).is_err());
    let restored = root.path().join("restored");
    assert!(download(&config, id, &config.host, "encrypted", &restored, &[]).is_err());
    assert!(!restored.exists());
}

#[test]
fn download_recovers_embedded_manifest_when_only_sidecar_is_missing() {
    let root = tempfile::tempdir().unwrap();
    let (config, mut state, source) = fixture(root.path());
    let result = upload(
        &config,
        &mut state,
        &source,
        &["encrypted".into()],
        "documents",
        &[],
        None,
        false,
    )
    .unwrap();
    let id = result["id"].as_str().unwrap();
    store(&config, "encrypted")
        .unwrap()
        .remove(&format!("{}.manifest", key(&config.host, id).unwrap()))
        .unwrap();
    let restored = root.path().join("restored");
    download(&config, id, &config.host, "encrypted", &restored, &[]).unwrap();
    assert_eq!(
        fs::read_to_string(restored.join("report.txt")).unwrap(),
        "original upload"
    );
}

#[test]
fn retry_reconciles_prepared_plan_without_snapshot_record_and_accepts_new_limits() {
    let root = tempfile::tempdir().unwrap();
    let (mut config, mut state, source) = fixture(root.path());
    config.uploads.spool_limit_bytes = 64;
    assert!(
        upload(
            &config,
            &mut state,
            &source,
            &["plain".into()],
            "documents",
            &[],
            None,
            false
        )
        .is_err()
    );
    let run = state.runs().unwrap().remove(0);
    config.uploads.spool_limit_bytes = 1024 * 1024;
    config.tools.timeout_seconds += 1;
    retry(&config, &mut state, Some(&run.id)).unwrap();
    assert_eq!(
        state.load_run(&run.id).unwrap().unwrap().state,
        RunState::Committed
    );
    let mut interrupted = state.load_run(&run.id).unwrap().unwrap();
    interrupted.state = RunState::BackingUp;
    interrupted.snapshot = None;
    let mut plan: UploadPlan = state.load_value("upload-plan", &run.id).unwrap().unwrap();
    plan.prepared = true;
    let mut recovery_state = State::open(&root.path().join("recovery-state")).unwrap();
    recovery_state.save_run(&interrupted).unwrap();
    prepare(&config, &mut recovery_state, &mut interrupted, &mut plan).unwrap();
    assert_eq!(
        interrupted.snapshot.as_deref(),
        Some(interrupted.id.as_str())
    );
}

#[test]
fn expiry_resumes_after_sidecar_removal_without_orphaning_payload() {
    let root = tempfile::tempdir().unwrap();
    let (config, state, source) = fixture(root.path());
    let payload = root.path().join("expired.dcloud");
    let manifest = archive::create(
        &source,
        &payload,
        &ArchiveOptions {
            host: config.host.clone(),
            job: "_uploads".into(),
            expires_at: Some(Utc::now() - chrono::Duration::days(1)),
            ..ArchiveOptions::default()
        },
    )
    .unwrap();
    let destination = "plain".to_owned();
    let object_key = key(&config.host, &manifest.id).unwrap();
    let target = store(&config, &destination).unwrap();
    target.put_immutable(&object_key, &payload).unwrap();
    prepare_anchor(&config, &payload, &manifest, &[]).unwrap();
    target
        .put_immutable(&format!("{object_key}.anchor"), &anchor_path(&payload))
        .unwrap();
    target
        .put_immutable(
            &format!("{object_key}.manifest"),
            &archive::manifest_path(&payload),
        )
        .unwrap();
    assert_eq!(
        expire(&config, &state, false)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let intent_key = format!("{destination}:{}:{}", config.host, manifest.id);
    state
        .save_value(
            "archive-expiration",
            &intent_key,
            &ExpirationIntent {
                destination: destination.clone(),
                host: config.host.clone(),
                id: manifest.id.clone(),
                expires_at: manifest.expires_at.unwrap(),
                config_hash: config
                    .upload_digest(std::slice::from_ref(&destination))
                    .unwrap(),
                complete: false,
            },
        )
        .unwrap();
    target.remove(&format!("{object_key}.manifest")).unwrap();
    expire(&config, &state, true).unwrap();
    assert!(target.list_uploads(Some(&config.host)).unwrap().is_empty());
    assert!(
        state
            .load_value::<ExpirationIntent>("archive-expiration", &intent_key)
            .unwrap()
            .unwrap()
            .complete
    );
}

#[test]
fn relabel_preserves_payload_and_merges_authenticated_label_operations() {
    let root = tempfile::tempdir().unwrap();
    let (config, mut state, source) = fixture(root.path());
    let result = upload(
        &config,
        &mut state,
        &source,
        &["plain".into(), "encrypted".into()],
        "documents",
        &["personal".into()],
        None,
        false,
    )
    .unwrap();
    let id = result["id"].as_str().unwrap();
    for from in ["plain", "encrypted"] {
        let payload =
            PathBuf::from(&config.destinations[from].location).join(key(&config.host, id).unwrap());
        let before = archive::hash_file(&payload).unwrap();
        relabel(
            &config,
            &state,
            from,
            &config.host,
            id,
            Some("taxes"),
            &["2026".into()],
            &[],
        )
        .unwrap();
        relabel(
            &config,
            &state,
            from,
            &config.host,
            id,
            None,
            &["reviewed".into()],
            &["personal".into()],
        )
        .unwrap();
        let manifest = manifests(&config, from, Some(&config.host))
            .unwrap()
            .remove(0);
        assert_eq!(manifest.category, "taxes");
        assert_eq!(manifest.labels, vec!["2026", "reviewed"]);
        assert_eq!(archive::hash_file(&payload).unwrap(), before);
        let original = archive::read_manifest(&payload, &identities(&config).unwrap()).unwrap();
        assert_eq!(original.category, "documents");
        assert_eq!(original.labels, vec!["personal"]);
        let changes: Vec<_> = store(&config, from)
            .unwrap()
            .list_uploads(Some(&config.host))
            .unwrap()
            .into_iter()
            .filter(|item| item.key.contains(".metadata/"))
            .collect();
        assert_eq!(changes.len(), 2);
        if from == "encrypted" {
            for change in changes {
                let bytes =
                    fs::read(PathBuf::from(&config.destinations[from].location).join(change.key))
                        .unwrap();
                assert!(!String::from_utf8_lossy(&bytes).contains("reviewed"));
            }
        }
        download(
            &config,
            id,
            &config.host,
            from,
            &root.path().join(format!("restored-label-{from}")),
            &[],
        )
        .unwrap();
    }
}

#[test]
fn relabel_detects_forged_metadata_and_repository_key_replacement() {
    let root = tempfile::tempdir().unwrap();
    let (config, mut state, source) = fixture(root.path());
    let result = upload(
        &config,
        &mut state,
        &source,
        &["plain".into()],
        "documents",
        &[],
        None,
        false,
    )
    .unwrap();
    let id = result["id"].as_str().unwrap();
    relabel(
        &config,
        &state,
        "plain",
        &config.host,
        id,
        Some("taxes"),
        &[],
        &[],
    )
    .unwrap();
    let item = store(&config, "plain")
        .unwrap()
        .list_uploads(Some(&config.host))
        .unwrap()
        .into_iter()
        .find(|item| item.key.contains(".metadata/"))
        .unwrap();
    let path = PathBuf::from(&config.destinations["plain"].location).join(item.key);
    let valid = fs::read(&path).unwrap();
    let mut forged = valid.clone();
    let start = forged
        .windows(5)
        .position(|bytes| bytes == b"taxes")
        .unwrap();
    forged[start..start + 5].copy_from_slice(b"faked");
    fs::write(&path, forged).unwrap();
    let error = manifests(&config, "plain", None).unwrap_err().to_string();
    assert!(error.contains("authentication failed"), "{error}");
    fs::write(&path, valid).unwrap();
    fs::write(
        &config.password_file,
        b"different-secret-repository-password",
    )
    .unwrap();
    assert!(
        manifests(&config, "plain", None)
            .unwrap_err()
            .to_string()
            .contains("original repository recovery password")
    );
}

#[test]
fn encrypted_public_key_cannot_forge_archive_origin_or_remove_authentication() {
    let root = tempfile::tempdir().unwrap();
    let (config, mut state, source) = fixture(root.path());
    let result = upload(
        &config,
        &mut state,
        &source,
        &["encrypted".into()],
        "documents",
        &[],
        None,
        false,
    )
    .unwrap();
    let id = result["id"].as_str().unwrap();
    let original = PathBuf::from(&config.destinations["encrypted"].location)
        .join(key(&config.host, id).unwrap());
    fs::write(source.join("report.txt"), b"attacker replacement").unwrap();
    let forged = root.path().join("forged.dcloud");
    archive::create(
        &source,
        &forged,
        &ArchiveOptions {
            id: Some(id.into()),
            host: config.host.clone(),
            job: "_uploads".into(),
            category: "documents".into(),
            recipients: config.recipients.clone(),
            ..ArchiveOptions::default()
        },
    )
    .unwrap();
    fs::copy(&forged, &original).unwrap();
    fs::copy(
        archive::manifest_path(&forged),
        archive::manifest_path(&original),
    )
    .unwrap();
    let output = root.path().join("restored-forged");
    assert!(
        download(&config, id, &config.host, "encrypted", &output, &[])
            .unwrap_err()
            .to_string()
            .contains("anchor does not match")
    );
    assert!(!output.exists());
    assert!(manifests(&config, "encrypted", None).is_err());
    fs::remove_file(anchor_path(&original)).unwrap();
    assert!(
        download(&config, id, &config.host, "encrypted", &output, &[])
            .unwrap_err()
            .to_string()
            .contains("anchor is missing")
    );
    assert!(!output.exists());
}

#[test]
fn upload_refuses_to_package_runtime_oauth_credentials() {
    let root = tempfile::tempdir().unwrap();
    let (mut config, mut state, source) = fixture(root.path());
    config.rclone_config_file = Some(source.join("rclone.conf"));
    fs::write(config.rclone_config_file.as_ref().unwrap(), b"oauth-secret").unwrap();
    assert!(
        upload(
            &config,
            &mut state,
            &source,
            &["plain".into()],
            "documents",
            &[],
            None,
            false
        )
        .unwrap_err()
        .to_string()
        .contains("credentials")
    );
    assert!(state.runs().unwrap().is_empty());
}
