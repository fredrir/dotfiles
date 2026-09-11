#![forbid(unsafe_code)]

use anyhow::Result;
use dcloud::config::{Config, Destination, Job, Retention};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use testkit::{Bin, stderr, stdout};

fn cli(path: &Path, args: &[&str]) -> std::process::Output {
    Bin::new(env!("CARGO_BIN_EXE_dcloud"))
        .arg("--config")
        .arg(path)
        .arg("--json")
        .args(args)
        .output()
}
fn value(path: &Path, args: &[&str]) -> Value {
    let output = cli(path, args);
    assert!(output.status.success(), "{}", stderr(&output));
    serde_json::from_str(&stdout(&output)).unwrap()
}

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    source: PathBuf,
    config: PathBuf,
}
fn fixture() -> Result<Fixture> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().canonicalize()?;
    let source = root.join("Documents");
    fs::create_dir(&source)?;
    fs::write(source.join("notes.txt"), "recovery matters")?;
    let (identity, recipient) = dcloud::archive::generate_identity();
    let password = root.join("keys/repository.key");
    let key = root.join("keys/identity.txt");
    dcloud::setup::write_private_new(&password, b"fixture-repository-password-at-least-32-bytes")?;
    dcloud::setup::write_private_new(&key, identity.as_bytes())?;
    let config = Config {
        host: "macie".into(),
        state_dir: root.join("state"),
        password_file: password,
        identity_file: key,
        recipients: vec![recipient],
        destinations: [
            (
                "plain".into(),
                Destination {
                    location: root.join("plain").display().to_string(),
                    encrypted: false,
                    ..Destination::default()
                },
            ),
            (
                "encrypted".into(),
                Destination {
                    location: root.join("encrypted").display().to_string(),
                    ..Destination::default()
                },
            ),
        ]
        .into(),
        jobs: [(
            "Documents".into(),
            Job {
                sources: [("macie".into(), vec![source.clone()])].into(),
                destinations: vec!["plain".into(), "encrypted".into()],
                required: vec!["plain".into(), "encrypted".into()],
                min_copies: 2,
                retries: 0,
                min_free_bytes: 0,
                retention: Retention {
                    auto: false,
                    ..Retention::default()
                },
                ..Job::default()
            },
        )]
        .into(),
        ..Config::default()
    };
    let path = root.join("config.toml");
    fs::write(&path, toml::to_string_pretty(&config)?)?;
    Ok(Fixture {
        _temp: temp,
        root,
        source,
        config: path,
    })
}

#[test]
fn help_completions_and_command_dump_work_without_config() {
    for args in [
        &["--help"][..],
        &["--completions", "zsh"],
        &["--command-dump"],
    ] {
        let output = Bin::new(env!("CARGO_BIN_EXE_dcloud")).args(args).output();
        assert!(output.status.success(), "{}", stderr(&output));
        assert!(stdout(&output).contains("dcloud"));
    }
}

#[test]
fn cli_archives_roundtrip_on_plain_and_encrypted_destinations() -> Result<()> {
    let fixture = fixture()?;
    let upload = value(
        &fixture.config,
        &[
            "upload",
            fixture.source.to_str().unwrap(),
            "--to",
            "plain,encrypted",
            "--category",
            "documents",
            "--label",
            "personal",
        ],
    );
    let id = upload["id"].as_str().unwrap();
    for destination in ["plain", "encrypted"] {
        let restored = fixture.root.join(format!("restore-{destination}"));
        value(
            &fixture.config,
            &[
                "download",
                id,
                "--from",
                destination,
                "--to",
                restored.to_str().unwrap(),
            ],
        );
        assert_eq!(
            fs::read_to_string(restored.join("notes.txt"))?,
            "recovery matters"
        );
        let collision = cli(
            &fixture.config,
            &[
                "download",
                id,
                "--from",
                destination,
                "--to",
                restored.to_str().unwrap(),
            ],
        );
        assert!(!collision.status.success());
        value(
            &fixture.config,
            &[
                "label-upload",
                id,
                "--from",
                destination,
                "--add",
                "reviewed",
                "--category",
                "documents",
            ],
        );
    }
    let listing = value(
        &fixture.config,
        &["browse", "--offline", "--label", "reviewed"],
    );
    assert_eq!(listing["items"].as_array().unwrap().len(), 2);
    assert!(fixture.source.join("notes.txt").exists());
    Ok(())
}

#[test]
fn cli_backup_restore_and_retention_preview_preserve_source() -> Result<()> {
    if !std::process::Command::new("restic")
        .arg("version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        return Ok(());
    }
    let fixture = fixture()?;
    value(&fixture.config, &["plan", "Documents"]);
    assert!(!fixture.root.join("state/spool").exists());
    let result = value(&fixture.config, &["backup", "Documents"]);
    let snapshot = result["results"][0]["replicas"]["encrypted"]["snapshot"]
        .as_str()
        .unwrap();
    let restored = fixture.root.join("snapshot-restore");
    value(
        &fixture.config,
        &[
            "restore",
            snapshot,
            "--job",
            "Documents",
            "--from",
            "encrypted",
            "--to",
            restored.to_str().unwrap(),
        ],
    );
    assert_eq!(
        fs::read_to_string(
            restored
                .join(fixture.source.strip_prefix("/")?)
                .join("notes.txt")
        )?,
        "recovery matters"
    );
    value(
        &fixture.config,
        &["retention", "--job", "Documents", "--from", "encrypted"],
    );
    assert!(fixture.source.exists());
    Ok(())
}

#[test]
fn invalid_configuration_fails_before_backup_side_effects() -> Result<()> {
    let fixture = fixture()?;
    let mut c = Config::load(&fixture.config)?;
    c.jobs.get_mut("Documents").unwrap().min_copies = 3;
    fs::write(&fixture.config, toml::to_string_pretty(&c)?)?;
    let output = cli(&fixture.config, &["backup", "Documents"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("min_copies"));
    assert!(!fixture.root.join("state").exists());
    Ok(())
}

#[test]
fn local_status_and_schedule_work_without_available_credentials() -> Result<()> {
    let fixture = fixture()?;
    let mut c = Config::load(&fixture.config)?;
    c.secrets_file = Some(fixture.root.join("unavailable.sops.json"));
    c.rclone_secrets_file = Some(fixture.root.join("unavailable-rclone.sops.json"));
    fs::write(&fixture.config, toml::to_string_pretty(&c)?)?;
    value(&fixture.config, &["status"]);
    value(&fixture.config, &["schedule"]);
    let listing = value(&fixture.config, &["browse", "--offline"]);
    assert!(listing["items"].as_array().unwrap().is_empty());
    Ok(())
}

#[test]
fn local_status_does_not_bootstrap_state_and_bare_status_reports_unreachable_owners() -> Result<()>
{
    let fixture = fixture()?;
    let mut config = Config::load(&fixture.config)?;
    config.secrets_file = Some(fixture.root.join("unavailable.sops.json"));
    config
        .jobs
        .get_mut("Documents")
        .unwrap()
        .sources
        .insert("archie".into(), vec!["/Documents".into()]);
    config
        .hosts
        .insert("archie".into(), dcloud::config::HostConfig::default());
    fs::write(&fixture.config, toml::to_string_pretty(&config)?)?;
    let local = value(&fixture.config, &["status", "--local"]);
    assert_eq!(local["local_only"], true);
    assert!(
        local["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["host"] == "macie")
    );
    assert!(!fixture.root.join("state").exists());
    let bare = cli(&fixture.config, &[]);
    assert_eq!(bare.status.code(), Some(2));
    let all: Value = serde_json::from_str(&stdout(&bare))?;
    assert_eq!(all["local_only"], false);
    assert_eq!(all["hosts"][1]["status"], "unreachable");
    let remote = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["host"] == "archie")
        .unwrap();
    assert!(remote["last_verified"].is_null());
    assert_eq!(remote["status"], "unknown; source unreachable");
    assert!(!fixture.root.join("state").exists());
    Ok(())
}
