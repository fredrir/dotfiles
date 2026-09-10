use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use dcloud::config::{Config, Destination, Job};

fn config() -> Config {
    Config {
        host: "archie".into(),
        password_file: "/private/recovery.key".into(),
        identity_file: "/private/identity".into(),
        destinations: BTreeMap::from([(
            "drive".into(),
            Destination {
                location: "/storage".into(),
                ..Destination::default()
            },
        )]),
        jobs: BTreeMap::from([(
            "documents".into(),
            Job {
                sources: BTreeMap::from([("archie".into(), vec![PathBuf::from("/source")])]),
                destinations: vec!["drive".into()],
                required: vec!["drive".into()],
                ..Job::default()
            },
        )]),
        ..Config::default()
    }
}

#[test]
fn operational_changes_and_unrelated_jobs_do_not_strand_pending_transfers() -> Result<()> {
    let mut config = config();
    let backup = config.backup_digest("documents")?;
    let upload = config.upload_digest(&["drive".into()])?;
    config.tools.timeout_seconds = 7;
    config.tools.restic = "/new/restic".into();
    config.jobs.get_mut("documents").unwrap().schedule.hour = 7;
    config.jobs.get_mut("documents").unwrap().retention.last = 10;
    config.jobs.get_mut("documents").unwrap().alert = vec!["alert".into()];
    config.destinations.get_mut("drive").unwrap().quota_bytes = Some(123);
    config.jobs.insert("unrelated".into(), Job::default());
    assert_eq!(config.backup_digest("documents")?, backup);
    assert_eq!(config.upload_digest(&["drive".into()])?, upload);
    config.destinations.get_mut("drive").unwrap().location = "/different-storage".into();
    assert_ne!(config.backup_digest("documents")?, backup);
    assert_ne!(config.upload_digest(&["drive".into()])?, upload);
    Ok(())
}

#[test]
fn encrypted_credentials_keep_scoped_identity_across_runtime_directories() -> Result<()> {
    let mut config = config();
    config.secrets_file = Some("/config/recovery.sops.json".into());
    config.rclone_secrets_file = Some("/config/rclone.sops.json".into());
    config.password_file = "/temporary/first/password".into();
    config.identity_file = "/temporary/first/identity".into();
    config.rclone_config_file = Some("/temporary/first/rclone".into());
    let backup = config.backup_digest("documents")?;
    let upload = config.upload_digest(&["drive".into()])?;
    config.password_file = "/temporary/second/password".into();
    config.identity_file = "/temporary/second/identity".into();
    config.rclone_config_file = Some("/temporary/second/rclone".into());
    assert_eq!(config.backup_digest("documents")?, backup);
    assert_eq!(config.upload_digest(&["drive".into()])?, upload);
    Ok(())
}

#[cfg(unix)]
#[test]
fn linked_config_resolves_secret_paths_beside_the_real_config() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let actual = directory.path().canonicalize()?.join("repository");
    fs::create_dir(&actual)?;
    let config = Config {
        host: "archie".into(),
        state_dir: directory.path().join("state"),
        password_file: "repository.key".into(),
        identity_file: "identity.key".into(),
        secrets_file: Some("recovery.sops.json".into()),
        rclone_secrets_file: Some("rclone.sops.json".into()),
        ..Config::default()
    };
    let config_path = actual.join("archie.toml");
    fs::write(&config_path, toml::to_string(&config)?)?;
    let link = directory.path().join("config.toml");
    std::os::unix::fs::symlink(&config_path, &link)?;
    let loaded = Config::load(&link)?;
    assert_eq!(loaded.secrets_file, Some(actual.join("recovery.sops.json")));
    assert_eq!(
        loaded.rclone_secrets_file,
        Some(actual.join("rclone.sops.json"))
    );
    assert_eq!(loaded.password_file, actual.join("repository.key"));
    assert_eq!(loaded.identity_file, actual.join("identity.key"));
    Ok(())
}

#[test]
fn raw_cloud_credentials_are_excluded_from_backup_sources() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let source = directory.path().canonicalize()?.join("source");
    fs::create_dir(&source)?;
    let mut config = config();
    config.state_dir = directory.path().join("state");
    config
        .jobs
        .get_mut("documents")
        .unwrap()
        .sources
        .insert("archie".into(), vec![source.clone()]);
    config.rclone_config_file = Some(source.join("rclone.conf"));
    assert!(format!("{:#}", config.validate().unwrap_err()).contains("recovery credentials"));
    Ok(())
}

#[test]
fn plain_upload_signing_key_is_part_of_transfer_identity() -> Result<()> {
    let mut config = config();
    config.destinations.get_mut("drive").unwrap().encrypted = false;
    let original = config.upload_digest(&["drive".into()])?;
    config.password_file = "/different-signing-key".into();
    assert_ne!(original, config.upload_digest(&["drive".into()])?);
    Ok(())
}
