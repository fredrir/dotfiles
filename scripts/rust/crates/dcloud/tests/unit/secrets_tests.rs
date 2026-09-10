use super::*;

#[cfg(unix)]
#[test]
fn runtime_secret_directory_is_private_and_removed_with_its_guard() {
    use std::os::unix::fs::PermissionsExt;
    let repository = tempfile::tempdir().unwrap();
    fs::write(repository.path().join(".sops.yaml"), "creation_rules: []").unwrap();
    let config = Config {
        host: "fixture".into(),
        rclone_secrets_file: Some(repository.path().join("config/dcloud/rclone.sops.json")),
        ..Config::default()
    };
    let guard = materialize(&config).unwrap();
    let directory = guard.runtime_directory().unwrap().to_path_buf();
    assert_eq!(
        fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(directory.join("rclone.conf"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(!directory.starts_with(repository.path()));
    drop(guard);
    assert!(!directory.exists());
}

#[test]
fn google_oauth_client_secret_is_stored_literally_in_private_runtime_config() {
    let mut config = Ini::new_cs();
    config.set("drive", "type", Some("drive".into()));
    config.set(
        "drive",
        "client_id",
        Some("fixture.apps.googleusercontent.com".into()),
    );
    config.set(
        "drive",
        "client_secret",
        Some("previous-obscured-fixture-value".into()),
    );
    config.set("drive", "token", Some("existing-fixture-token".into()));
    let original = "GOCSPX-dcloud_fixture-secret_123";
    configure_google_remote(
        &mut config,
        "drive",
        "fixture.apps.googleusercontent.com",
        original,
    )
    .unwrap();
    let mut reloaded = Ini::new_cs();
    reloaded.read(config.writes()).unwrap();
    assert_eq!(
        reloaded.get("drive", "client_secret").as_deref(),
        Some(original)
    );
    assert_eq!(
        reloaded.get("drive", "scope").as_deref(),
        Some("drive.file")
    );
    assert_eq!(
        reloaded.get("drive", "token").as_deref(),
        Some("existing-fixture-token")
    );
}

#[test]
fn oauth_failure_hints_expose_only_known_classes_and_never_raw_credentials() {
    let hint = oauth_error_hint(
        b"callback?code=private-code-canary",
        b"oauth2: invalid_client client_secret=private-secret-canary access_token=private-token-canary",
    );
    assert!(hint.starts_with("invalid_client:"));
    assert!(!hint.contains("canary"));
    let unknown = oauth_error_hint(
        b"",
        b"server echoed private-secret-canary in an unexpected failure",
    );
    assert!(!unknown.contains("canary"));
    assert!(oauth_error_hint(b"", b"OAuth error: ACCESS_DENIED").starts_with("access_denied:"));
}

#[test]
fn existing_repository_password_spaces_survive_sops_migration() {
    let temp = tempfile::tempdir().unwrap();
    let key = temp.path().join("repository.key");
    let password = format!("  {}  ", "recovery".repeat(8));
    fs::write(&key, format!("{password}\r\n")).unwrap();
    assert_eq!(read_repository_password(&key).unwrap(), password);
}

#[test]
fn unconfigured_secrets_preserve_config_and_have_no_runtime_files() {
    let config = Config {
        host: "macie".into(),
        ..Config::default()
    };
    let digest = config.digest().unwrap();
    let runtime = materialize(&config).unwrap();
    assert_eq!(runtime.config.password_file, config.password_file);
    assert_eq!(runtime.config.digest().unwrap(), digest);
    assert!(runtime.runtime_directory().is_none());
}

#[test]
fn private_files_never_overwrite_existing_keys() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("key");
    write_private_new(&path, b"original").unwrap();
    assert!(write_private_new(&path, b"replacement").is_err());
    assert_eq!(fs::read(&path).unwrap(), b"original");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn secret_input_size_and_symlinks_are_checked() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("large");
    File::create(&path)
        .unwrap()
        .set_len(MAX_SECRET_BYTES as u64 + 1)
        .unwrap();
    assert!(read_private(&path).is_err());
    #[cfg(unix)]
    {
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(read_private(&link).is_err());
    }
}

#[test]
fn recovery_requires_valid_independent_credentials() {
    let identity = age::x25519::Identity::generate();
    let valid = RecoveryBundle {
        version: 1,
        repository_password: "a".repeat(64),
        archive_identity: identity.to_string().expose_secret().into(),
    };
    assert!(validate_recovery(&valid).is_ok());
    assert_eq!(
        recipient(&valid.archive_identity).unwrap(),
        identity.to_public().to_string()
    );
    let invalid = RecoveryBundle {
        version: 1,
        repository_password: "short".into(),
        archive_identity: valid.archive_identity.clone(),
    };
    assert!(validate_recovery(&invalid).is_err());
}

#[test]
fn actual_sops_encrypts_to_existing_recipient_rules_without_plaintext_artifacts() {
    if Command::new("sops").arg("--version").output().is_err() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let identity = age::x25519::Identity::generate();
    fs::write(
        temp.path().join(".sops.yaml"),
        format!("creation_rules:\n  - age: {}\n", identity.to_public()),
    )
    .unwrap();
    let target = temp.path().join("config/dcloud/recovery.sops.json");
    let secret = b"{\"example\":\"dcloud-secret-canary\"}";
    seal(&target, secret, None).unwrap();
    let encrypted = fs::read_to_string(&target).unwrap();
    assert!(!encrypted.contains("dcloud-secret-canary"));
    assert!(encrypted.contains("ENC[AES256_GCM"));
    assert_eq!(
        fs::read_dir(target.parent().unwrap())
            .unwrap()
            .filter(|entry| !entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".lock"))
            .count(),
        1
    );
    assert!(seal(&target, secret, None).is_err());
    let key = NamedTempFile::new().unwrap();
    fs::write(key.path(), identity.to_string().expose_secret()).unwrap();
    let mut output = process::output(
        Command::new("sops")
            .args(["decrypt", "--output-type", "json"])
            .arg(&target)
            .env("SOPS_AGE_KEY_FILE", key.path())
            .stdin(Stdio::null()),
        CaptureLimits::default(),
        Duration::from_secs(10),
    )
    .unwrap();
    assert!(output.status.success());
    let decoded: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(decoded["example"], "dcloud-secret-canary");
    output.stdout.zeroize();
}

#[test]
fn google_client_import_requires_external_plaintext_and_seals_without_copying_it() {
    if Command::new("sops").arg("--version").output().is_err() {
        return;
    }
    let repository = tempfile::tempdir().unwrap();
    let identity = age::x25519::Identity::generate();
    fs::write(
        repository.path().join(".sops.yaml"),
        format!("creation_rules:\n  - age: {}\n", identity.to_public()),
    )
    .unwrap();
    let target = repository
        .path()
        .join("config/dcloud/google-client.sops.json");
    let in_repository = repository.path().join("client.json");
    let client = b"{\"installed\":{\"client_id\":\"example.apps.googleusercontent.com\",\"client_secret\":\"dcloud-oauth-secret-canary\"}}";
    fs::write(&in_repository, client).unwrap();
    assert!(seal_google_client(&in_repository, &target).is_err());
    assert!(!target.exists());
    let downloaded = tempfile::NamedTempFile::new().unwrap();
    fs::write(downloaded.path(), client).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(downloaded.path(), fs::Permissions::from_mode(0o644)).unwrap();
    }
    seal_google_client(downloaded.path(), &target).unwrap();
    assert!(
        !fs::read_to_string(&target)
            .unwrap()
            .contains("dcloud-oauth-secret-canary")
    );
    assert_eq!(
        fs::read_dir(target.parent().unwrap())
            .unwrap()
            .filter(|entry| !entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".lock"))
            .count(),
        1
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(downloaded.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

fn oauth_fixture(expiry: &str, access: &str, refresh: &str) -> String {
    let mut ini = Ini::new_cs();
    ini.set("drive", "type", Some("drive".into()));
    ini.set(
        "drive",
        "client_id",
        Some("fixture.apps.googleusercontent.com".into()),
    );
    ini.set(
        "drive",
        "client_secret",
        Some("fixture-client-secret-canary".into()),
    );
    ini.set("drive", "scope", Some("drive.file".into()));
    ini.set("drive", "token", Some(serde_json::json!({"access_token":access,"token_type":"Bearer","refresh_token":refresh,"expiry":expiry}).to_string()));
    ini.writes()
}

#[test]
fn oauth_merge_preserves_newer_tokens_and_rejects_changed_credentials() {
    let initial = oauth_fixture("2026-09-10T21:00:00Z", "access-old", "refresh-stable");
    let fresh = oauth_fixture("2026-09-10T22:00:00Z", "access-new", "refresh-stable");
    let merged = merge_tokens(&initial, &fresh).unwrap();
    assert!(merged.contains("access-new"));
    assert_eq!(
        merge_tokens(&merged, &initial).unwrap().as_str(),
        merged.as_str()
    );
    for candidate in [
        fresh.replace("drive.file", "drive"),
        fresh.replace("fixture.apps", "different.apps"),
        fresh.replace("fixture-client-secret", "different-client-secret"),
        fresh.replace("refresh-stable", "refresh-rotated"),
    ] {
        assert!(merge_tokens(&initial, &candidate).is_err());
    }
}

#[test]
fn oauth_cache_refuses_repositories_and_symlink_directories() {
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("repository");
    fs::create_dir(&repository).unwrap();
    fs::write(repository.join(".sops.yaml"), "creation_rules: []").unwrap();
    let seed = repository.join("rclone.sops.json");
    fs::write(&seed, "fixture").unwrap();
    let mut config = Config {
        host: "fixture".into(),
        rclone_secrets_file: Some(seed),
        state_dir: repository.join("state"),
        ..Config::default()
    };
    assert!(cache_path(&config, "fixturehash", true).is_err());
    #[cfg(unix)]
    {
        config.state_dir = root.path().join("state");
        fs::create_dir(&config.state_dir).unwrap();
        std::os::unix::fs::symlink(&repository, config.state_dir.join("credentials")).unwrap();
        assert!(cache_path(&config, "fixturehash", false).is_err());
    }
}

#[test]
fn actual_sops_cache_refresh_preserves_git_seed_and_portable_recovery() {
    if Command::new("sops").arg("--version").output().is_err() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let repository = root.path().join("repository");
    fs::create_dir(&repository).unwrap();
    let identity = age::x25519::Identity::generate();
    fs::write(
        repository.join(".sops.yaml"),
        format!("creation_rules:\n  - age: {}\n", identity.to_public()),
    )
    .unwrap();
    let key = root.path().join("age-key");
    write_private_new(&key, identity.to_string().expose_secret().as_bytes()).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "secrets::tests::oauth_cache_subprocess_fixture",
            "--nocapture",
        ])
        .env("DCLOUD_CACHE_TEST_ROOT", root.path())
        .env("SOPS_AGE_KEY_FILE", key)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "cache subprocess failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn oauth_cache_subprocess_fixture() {
    let Some(root) = std::env::var_os("DCLOUD_CACHE_TEST_ROOT").map(PathBuf::from) else {
        return;
    };
    let repository = root.join("repository");
    let seed = repository.join("config/dcloud/rclone.sops.json");
    let bundle = RcloneBundle {
        version: 1,
        config: oauth_fixture(
            "2026-09-10T20:00:00Z",
            "initial-access-canary",
            "stable-refresh-canary",
        ),
        seed_sha256: None,
    };
    seal(&seed, &serde_json::to_vec(&bundle).unwrap(), None).unwrap();
    let original = fs::read(&seed).unwrap();
    let modified = fs::metadata(&seed).unwrap().modified().unwrap();
    let config = Config {
        host: "fixture".into(),
        state_dir: root.join("state"),
        password_file: root.join("repository.key"),
        identity_file: root.join("age-key"),
        rclone_secrets_file: Some(seed.clone()),
        ..Config::default()
    };
    write_private_new(
        &config.password_file,
        b"fixture-recovery-password-012345678901234567890123456789",
    )
    .unwrap();
    let first = materialize(&config).unwrap();
    let second = materialize(&config).unwrap();
    let config_digest = config.digest().unwrap();
    assert_eq!(first.config.digest().unwrap(), config_digest);
    let older = oauth_fixture(
        "2026-09-10T21:00:00Z",
        "older-access-canary",
        "stable-refresh-canary",
    );
    let newer = oauth_fixture(
        "2026-09-10T22:00:00Z",
        "newer-access-canary",
        "stable-refresh-canary",
    );
    fs::write(first.config.rclone_config_file.as_ref().unwrap(), &newer).unwrap();
    fs::write(second.config.rclone_config_file.as_ref().unwrap(), &older).unwrap();
    std::thread::scope(|scope| {
        let a = scope.spawn(|| first.persist());
        let b = scope.spawn(|| second.persist());
        a.join().unwrap().unwrap();
        b.join().unwrap().unwrap();
    });
    let cache = effective_rclone_secrets(&config).unwrap().unwrap();
    assert!(cache.starts_with(fs::canonicalize(config.state_dir.join("credentials")).unwrap()));
    assert_ne!(cache, seed);
    let ciphertext = fs::read(&cache).unwrap();
    let text = String::from_utf8_lossy(&ciphertext);
    assert!(text.contains("ENC[AES256_GCM"));
    assert!(!text.contains("canary"));
    let current = materialize(&config).unwrap();
    assert!(
        fs::read_to_string(current.config.rclone_config_file.as_ref().unwrap())
            .unwrap()
            .contains("newer-access-canary")
    );
    first.persist().unwrap();
    assert_eq!(fs::read(&cache).unwrap(), ciphertext);
    assert_eq!(fs::read(&seed).unwrap(), original);
    assert_eq!(fs::metadata(&seed).unwrap().modified().unwrap(), modified);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&cache).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(cache.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    let imported = RcloneBundle {
        version: 1,
        config: oauth_fixture(
            "2026-09-10T23:00:00Z",
            "migrated-access-canary",
            "stable-refresh-canary",
        ),
        seed_sha256: None,
    };
    let candidate = repository.join("preserved.sops.json");
    seal(&candidate, &serde_json::to_vec(&imported).unwrap(), None).unwrap();
    assert_eq!(seed_runtime_cache(&config, &candidate).unwrap(), cache);
    let effective = materialize(&config).unwrap();
    assert!(
        fs::read_to_string(effective.config.rclone_config_file.as_ref().unwrap())
            .unwrap()
            .contains("migrated-access-canary")
    );
    let export = root.join("export");
    crate::setup::export(&config, &export).unwrap();
    assert_eq!(
        fs::read(export.join("rclone.sops.json")).unwrap(),
        fs::read(&cache).unwrap()
    );
    let portable = Config {
        rclone_secrets_file: Some(export.join("rclone.sops.json")),
        state_dir: root.join("relocated-state"),
        ..config.clone()
    };
    let moved = materialize(&portable).unwrap();
    assert!(
        fs::read_to_string(moved.config.rclone_config_file.as_ref().unwrap())
            .unwrap()
            .contains("migrated-access-canary")
    );
    fs::write(
        moved.config.rclone_config_file.as_ref().unwrap(),
        oauth_fixture(
            "2026-09-11T01:00:00Z",
            "relocated-access-canary",
            "stable-refresh-canary",
        ),
    )
    .unwrap();
    moved.persist().unwrap();
    let wrong = RcloneBundle {
        version: 1,
        config: imported.config.clone(),
        seed_sha256: Some("wrong-binding".into()),
    };
    let wrong_bytes = encrypt_with_policy(&seed, &serde_json::to_vec(&wrong).unwrap()).unwrap();
    let (_, observed_hash) = cached_bundle(&cache, &digest(&original), &bundle)
        .unwrap()
        .unwrap();
    commit_ciphertext(&cache, &wrong_bytes, Some(&observed_hash)).unwrap();
    assert!(commit_ciphertext(&cache, &ciphertext, Some(&observed_hash)).is_err());
    assert_eq!(fs::read(&cache).unwrap(), wrong_bytes);
    assert!(materialize(&config).is_err());
    fs::remove_file(&cache).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&seed, &cache).unwrap();
        assert!(materialize(&config).is_err());
        assert!(effective_rclone_secrets(&config).is_err());
        fs::remove_file(&cache).unwrap();
    }
    seal(
        &seed,
        &serde_json::to_vec(&bundle).unwrap(),
        Some(&digest(&original)),
    )
    .unwrap();
    assert!(first.persist().is_err());
    assert_eq!(effective_rclone_secrets(&config).unwrap().unwrap(), seed);
    assert_eq!(config.digest().unwrap(), config_digest);
}
