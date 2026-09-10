use super::*;
use crate::config::{Cleanup, Destination, DestinationKind, Retention, Tools};

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    source: PathBuf,
    config: Config,
    state: State,
}

fn fixture() -> Result<Fixture> {
    let temporary = tempfile::tempdir()?;
    let root = temporary.path().canonicalize()?;
    let source = root.join("documents");
    fs::create_dir(&source)?;
    fs::write(source.join("important.txt"), "original")?;
    let password = root.join("repository.key");
    fs::write(&password, "a-long-fixture-only-recovery-password\n")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&password, fs::Permissions::from_mode(0o600))?;
    }
    let job = Job {
        sources: BTreeMap::from([("archie".into(), vec![source.clone()])]),
        destinations: vec!["local".into()],
        required: vec!["local".into()],
        min_copies: 1,
        category: "documents".into(),
        retention: Retention {
            last: 1,
            weekly: 0,
            monthly: 0,
            yearly: 0,
            auto: false,
        },
        min_free_bytes: 0,
        spool_limit_bytes: 1024 * 1024 * 1024,
        retries: 0,
        retry_delay_seconds: 0,
        ..Job::default()
    };
    let config = Config {
        host: "archie".into(),
        state_dir: root.join("state"),
        password_file: password,
        identity_file: root.join("identity"),
        tools: Tools {
            timeout_seconds: 120,
            ..Tools::default()
        },
        jobs: BTreeMap::from([("documents".into(), job)]),
        destinations: BTreeMap::from([(
            "local".into(),
            Destination {
                kind: DestinationKind::Local,
                location: root.join("remote").display().to_string(),
                ..Destination::default()
            },
        )]),
        ..Config::default()
    };
    let state = State::open(&config.state_dir)?;
    Ok(Fixture {
        _temporary: temporary,
        root,
        source,
        config,
        state,
    })
}

fn has_restic() -> bool {
    Command::new("restic")
        .arg("version")
        .output()
        .is_ok_and(|output| output.status.success())
}

#[test]
fn remote_maintenance_cannot_expire_source_cleanup_donors() -> Result<()> {
    let mut fixture = fixture()?;
    fixture.config.host = "macie".into();
    fixture.config.jobs.get_mut("documents").unwrap().cleanup = Some(Cleanup::default());
    fixture
        .config
        .destinations
        .get_mut("local")
        .unwrap()
        .maintenance_owner = Some("macie".into());
    let error = retention(
        &fixture.config,
        &mut fixture.state,
        "archie",
        "documents",
        "local",
        true,
        true,
    )
    .unwrap_err();
    assert!(error.to_string().contains("source owner archie"));
    assert!(!fixture.root.join("remote").exists());
    Ok(())
}

#[test]
fn immutable_snapshot_retry_keeps_original_source_version() -> Result<()> {
    if !has_restic() {
        return Ok(());
    }
    let mut fixture = fixture()?;
    let blocker = fixture.root.join("offline");
    fs::write(&blocker, "offline destination")?;
    fixture.config.destinations.insert(
        "later".into(),
        Destination {
            location: blocker.join("repository").display().to_string(),
            ..Destination::default()
        },
    );
    fixture
        .config
        .jobs
        .get_mut("documents")
        .unwrap()
        .destinations
        .push("later".into());
    let first = backup(&fixture.config, &mut fixture.state, "documents", false)?;
    assert_eq!(first["state"], "degraded");
    let run_id = first["id"].as_str().unwrap();
    let original_snapshot = first["snapshot"].as_str().unwrap().to_owned();
    fs::write(fixture.source.join("important.txt"), "newer source version")?;
    fs::remove_file(&blocker)?;
    fs::create_dir(&blocker)?;
    retry(&fixture.config, &mut fixture.state, Some(run_id))?;
    let run = fixture.state.load_run(run_id)?.unwrap();
    assert_eq!(run.state, RunState::Committed);
    assert_eq!(run.snapshot.as_deref(), Some(original_snapshot.as_str()));
    let destination = repository(&fixture.config, "archie", "documents", "later")?;
    let restored = fixture.root.join("restored");
    destination.restore(
        run.replicas["later"].snapshot.as_deref().unwrap(),
        &restored,
        &[],
    )?;
    assert_eq!(
        fs::read_to_string(
            restored
                .join(fixture.source.strip_prefix("/")?)
                .join("important.txt")
        )?,
        "original"
    );
    assert_eq!(
        repository(&fixture.config, "archie", "documents", "spool")?
            .snapshots(None, None)?
            .len(),
        1
    );
    Ok(())
}

#[test]
fn capture_journal_recovers_snapshot_committed_before_run_record() -> Result<()> {
    if !has_restic() {
        return Ok(());
    }
    let mut fixture = fixture()?;
    let job = fixture.config.jobs["documents"].clone();
    let mut run = RunRecord::new(
        "archie",
        "documents",
        &fixture.config.backup_digest("documents")?,
    );
    run.state = RunState::BackingUp;
    fixture.state.save_run(&run)?;
    let details = BackupDetails {
        tree: String::new(),
        time: run.started,
        sources: vec![fixture.source.clone()],
        fingerprints: BTreeMap::new(),
        total_bytes: 8,
        cleanup_error: None,
    };
    fixture
        .state
        .save_value("backup_details", &run.id, &details)?;
    let spool = repository(&fixture.config, "archie", "documents", "spool")?;
    spool.init()?;
    let captured = spool
        .backup_run(
            &[fixture.source.clone()],
            "archie",
            "documents",
            "documents",
            &[],
            &[],
            &run.id,
        )?
        .unwrap();
    fs::write(fixture.source.join("important.txt"), "later")?;
    assert!(recover_capture(
        &fixture.config,
        &mut fixture.state,
        &mut run
    )?);
    assert_eq!(run.snapshot.as_deref(), Some(captured.snapshot_id.as_str()));
    replicate(
        &fixture.config,
        &mut fixture.state,
        "documents",
        &job,
        &mut run,
    )?;
    assert_eq!(spool.snapshots(None, None)?.len(), 1);
    Ok(())
}

#[test]
fn pending_partial_capture_is_never_promoted_on_recovery() -> Result<()> {
    if !has_restic() {
        return Ok(());
    }
    let mut fixture = fixture()?;
    let mut run = RunRecord::new(
        "archie",
        "documents",
        &fixture.config.backup_digest("documents")?,
    );
    run.state = RunState::BackingUp;
    fixture.state.save_run(&run)?;
    let details = BackupDetails {
        tree: String::new(),
        time: run.started,
        sources: vec![fixture.source.clone()],
        fingerprints: BTreeMap::new(),
        total_bytes: 8,
        cleanup_error: None,
    };
    fixture
        .state
        .save_value("backup_details", &run.id, &details)?;
    let spool = repository(&fixture.config, "archie", "documents", "spool")?;
    spool.init()?;
    let partial = spool
        .backup(
            &[fixture.source.clone()],
            "archie",
            "documents",
            "documents",
            &[],
            &[],
            false,
        )?
        .unwrap();
    spool.set_tags(
        &partial.snapshot_id,
        &[
            format!("dcloud.run:{}", run.id),
            "dcloud.capture:pending".into(),
        ],
        &[],
    )?;
    assert!(!recover_capture(
        &fixture.config,
        &mut fixture.state,
        &mut run
    )?);
    assert!(run.snapshot.is_none());
    let snapshot = spool.snapshots(None, None)?.remove(0);
    let entries = retention_entries(&[snapshot], &[run], &[], "spool");
    assert!(!entries[0].complete);
    assert!(!entries[0].verified);
    Ok(())
}

#[test]
fn retention_expires_history_but_keeps_latest_and_quarantine_donors() -> Result<()> {
    if !has_restic() {
        return Ok(());
    }
    let mut fixture = fixture()?;
    for value in ["one", "two", "three"] {
        fs::write(fixture.source.join("important.txt"), value)?;
        backup(&fixture.config, &mut fixture.state, "documents", false)?;
    }
    let preview = retention(
        &fixture.config,
        &mut fixture.state,
        "archie",
        "documents",
        "local",
        false,
        false,
    )?;
    assert_eq!(preview["plan"]["delete"].as_array().unwrap().len(), 2);
    retention(
        &fixture.config,
        &mut fixture.state,
        "archie",
        "documents",
        "local",
        true,
        true,
    )?;
    let snapshots =
        repository(&fixture.config, "archie", "documents", "local")?.snapshots(None, None)?;
    assert_eq!(snapshots.len(), 1);
    let result = restore_test(
        &fixture.config,
        &mut fixture.state,
        "archie",
        "documents",
        "local",
        Some(&snapshots[0].id),
    )?;
    assert_eq!(result["full_restore"], true);
    let latest = fixture.state.runs()?.remove(0);
    assert!(latest.replicas["local"].full_verified_at.is_some());
    Ok(())
}

#[test]
fn required_failure_never_completes_scheduled_occurrence() -> Result<()> {
    if !has_restic() {
        return Ok(());
    }
    let mut fixture = fixture()?;
    let blocker = fixture.root.join("remote");
    fs::write(&blocker, "not a directory")?;
    assert!(run_due(&fixture.config, &mut fixture.state).is_err());
    assert!(fixture.state.occurrence("archie", "documents")?.is_none());
    let run = fixture.state.runs()?.remove(0);
    assert_eq!(run.state, RunState::Failed);
    let snapshot = run.snapshot;
    fs::remove_file(&blocker)?;
    fs::create_dir(&blocker)?;
    run_due(&fixture.config, &mut fixture.state)?;
    assert!(fixture.state.occurrence("archie", "documents")?.is_some());
    assert_eq!(fixture.state.runs()?.len(), 1);
    assert_eq!(fixture.state.runs()?.remove(0).snapshot, snapshot);
    Ok(())
}

#[test]
fn complete_readback_is_required_before_automatic_source_quarantine() -> Result<()> {
    if !has_restic() {
        return Ok(());
    }
    let mut fixture = fixture()?;
    let old = filetime::FileTime::from_unix_time(
        (Utc::now() - chrono::Duration::days(30)).timestamp(),
        0,
    );
    filetime::set_file_mtime(fixture.source.join("important.txt"), old)?;
    filetime::set_file_mtime(&fixture.source, old)?;
    fixture.config.jobs.get_mut("documents").unwrap().cleanup = Some(Cleanup {
        min_age_days: 1,
        quarantine_days: 1,
        min_verified_copies: 1,
    });
    let result = backup(&fixture.config, &mut fixture.state, "documents", false)?;
    assert!(fixture.source.is_dir());
    assert_eq!(fs::read_dir(&fixture.source)?.count(), 0);
    assert_eq!(result["cleanup_pending"], false, "{result:#}");
    let held = result["cleanup"][0]["held"].as_str().unwrap();
    assert_eq!(
        fs::read_to_string(Path::new(held).join("important.txt"))?,
        "original"
    );
    assert!(result["replicas"]["local"]["full_verified_at"].is_string());
    let tickets = fixture.state.values::<QuarantineTicket>("quarantine")?;
    assert_eq!(tickets.len(), 1);
    assert_eq!(tickets[0].1.state, QuarantineState::Held);
    Ok(())
}

#[test]
fn dry_run_does_not_create_repository_or_run_record() -> Result<()> {
    let mut fixture = fixture()?;
    let plan = backup(&fixture.config, &mut fixture.state, "documents", true)?;
    assert_eq!(plan["dry_run"], true);
    assert!(fixture.state.runs()?.is_empty());
    assert!(!fixture.config.state_dir.join("spool").exists());
    assert!(!fixture.root.join("remote").exists());
    Ok(())
}

#[test]
fn post_hook_export_growth_is_checked_before_capture_and_release_runs() -> Result<()> {
    let mut fixture = fixture()?;
    let release = fixture.root.join("released");
    let job = fixture.config.jobs.get_mut("documents").unwrap();
    job.spool_limit_bytes = 64 * 1024 * 1024 + 64;
    job.before = vec![vec![
        "/bin/sh".into(),
        "-c".into(),
        "printf '%0256d' 0 > \"$1\"".into(),
        "export".into(),
        fixture.source.join("important.txt").display().to_string(),
    ]];
    job.after = vec![vec![
        "/bin/sh".into(),
        "-c".into(),
        ": > \"$1\"".into(),
        "release".into(),
        release.display().to_string(),
    ]];
    let error = backup(&fixture.config, &mut fixture.state, "documents", false).unwrap_err();
    assert!(format!("{error:#}").contains("pending spool would exceed"));
    assert_eq!(
        fs::metadata(fixture.source.join("important.txt"))?.len(),
        256
    );
    assert!(release.is_file());
    assert!(!fixture.config.state_dir.join("spool").exists());
    Ok(())
}

#[test]
fn partial_cleanup_is_durable_and_does_not_fail_verified_backup() -> Result<()> {
    if !has_restic() {
        return Ok(());
    }
    let mut fixture = fixture()?;
    let old = filetime::FileTime::from_unix_time(
        (Utc::now() - chrono::Duration::days(30)).timestamp(),
        0,
    );
    filetime::set_file_mtime(fixture.source.join("important.txt"), old)?;
    filetime::set_file_mtime(&fixture.source, old)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fixture.source, fs::Permissions::from_mode(0o750))?;
    }
    let recent = fixture.root.join("recent");
    fs::create_dir(&recent)?;
    fs::write(recent.join("active.txt"), "still active")?;
    let job = fixture.config.jobs.get_mut("documents").unwrap();
    job.sources.get_mut("archie").unwrap().push(recent.clone());
    job.cleanup = Some(Cleanup {
        min_age_days: 1,
        quarantine_days: 1,
        min_verified_copies: 1,
    });
    let result = backup(&fixture.config, &mut fixture.state, "documents", false)?;
    assert_eq!(result["state"], "committed");
    assert_eq!(result["cleanup_pending"], true);
    assert!(fixture.source.is_dir());
    assert_eq!(fs::read_dir(&fixture.source)?.count(), 0);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&fixture.source)?.permissions().mode() & 0o777,
            0o750
        );
    }
    fs::write(fixture.source.join("new.txt"), "created after quarantine")?;
    let retry = retry_cleanup(&fixture.config, &mut fixture.state)?;
    assert_eq!(retry["cleanup_pending"], true);
    assert_eq!(
        fs::read_to_string(fixture.source.join("new.txt"))?,
        "created after quarantine"
    );
    assert_eq!(
        fs::read_to_string(recent.join("active.txt"))?,
        "still active"
    );
    let run_id = result["id"].as_str().unwrap();
    let progress: BackupCleanup = fixture.state.load_value("backup-cleanup", run_id)?.unwrap();
    assert_eq!(progress.tickets.len(), 1);
    assert_eq!(progress.pending.len(), 1);
    assert!(progress.pending.contains_key(&recent));
    assert_eq!(
        fixture.state.load_run(run_id)?.unwrap().state,
        RunState::Committed
    );
    Ok(())
}

#[test]
fn source_evidence_mismatch_blocks_cleanup_without_rejecting_verified_snapshot() -> Result<()> {
    if !has_restic() {
        return Ok(());
    }
    let mut fixture = fixture()?;
    fixture.config.jobs.get_mut("documents").unwrap().cleanup = Some(Cleanup {
        min_age_days: 1,
        quarantine_days: 1,
        min_verified_copies: 1,
    });
    let job = fixture.config.jobs["documents"].clone();
    let mut run = RunRecord::new(
        "archie",
        "documents",
        &fixture.config.backup_digest("documents")?,
    );
    fixture.state.save_run(&run)?;
    capture(
        &fixture.config,
        &mut fixture.state,
        "documents",
        &job,
        &[fixture.source.clone()],
        &mut run,
    )?;
    let mut details: BackupDetails = fixture
        .state
        .load_value("backup_details", &run.id)?
        .unwrap();
    details.fingerprints.insert(
        fixture.source.clone(),
        "a-different-pre-capture-version".into(),
    );
    fixture
        .state
        .save_value("backup_details", &run.id, &details)?;
    replicate(
        &fixture.config,
        &mut fixture.state,
        "documents",
        &job,
        &mut run,
    )?;
    assert_eq!(run.state, RunState::Committed);
    assert!(run.replicas["local"].full_verified_at.is_some());
    let mut value = json!({});
    attach_cleanup(&fixture.config, &mut fixture.state, &job, &run, &mut value)?;
    assert_eq!(value["cleanup_pending"], true);
    assert!(
        value["cleanup_warnings"]
            .to_string()
            .contains("pre-capture fingerprint")
    );
    assert_eq!(
        fs::read_to_string(fixture.source.join("important.txt"))?,
        "original"
    );
    assert!(
        fixture
            .state
            .values::<QuarantineTicket>("quarantine")?
            .is_empty()
    );
    Ok(())
}
