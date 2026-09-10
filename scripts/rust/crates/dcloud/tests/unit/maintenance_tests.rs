use super::*;
use crate::config::Destination;
use crate::policy::{QuarantineState, QuarantineTicket};
use crate::state::RunState;
use std::fs;

#[test]
fn scheduled_maintenance_reaps_opted_in_quarantine_once_daily_after_full_restore() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let source = root.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("file.txt"), "retained verified copy").unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 86400);
    for path in [source.join("file.txt"), source.clone()] {
        fs::File::open(path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(old))
            .unwrap();
    }
    let config = Config {
        host: "fixture".into(),
        state_dir: root.join("state"),
        password_file: root.join("repository.key"),
        identity_file: root.join("unused.key"),
        destinations: [(
            "local".into(),
            Destination {
                location: root.join("storage").display().to_string(),
                encrypted: false,
                ..Destination::default()
            },
        )]
        .into(),
        ..Config::default()
    };
    crate::setup::write_private_new(&config.password_file, b"0123456789abcdef0123456789abcdef")
        .unwrap();
    let mut state = State::open(&config.state_dir).unwrap();
    let upload = objects::upload(
        &config,
        &mut state,
        &source,
        &["local".into()],
        "",
        &[],
        None,
        true,
    )
    .unwrap();
    let run_id = upload["id"].as_str().unwrap();
    let mut ticket = state
        .list_values::<QuarantineTicket>("quarantine")
        .unwrap()
        .remove(0)
        .1;
    assert_eq!(ticket.state, QuarantineState::Held);
    ticket.delete_after = Utc::now() - chrono::Duration::days(1);
    state.save_value("quarantine", &ticket.id, &ticket).unwrap();
    let first = run_due(&config, &mut state).unwrap();
    assert_eq!(first["attempted"], true);
    assert_eq!(first["result"]["warnings"], json!([]), "{first}");
    assert_eq!(first["result"]["quarantine"][0]["deleted"], true);
    assert!(!ticket.held.exists());
    assert_eq!(
        state.load_run(run_id).unwrap().unwrap().state,
        RunState::Committed
    );
    assert!(
        root.join("storage")
            .join(objects::key("fixture", run_id).unwrap())
            .exists()
    );
    let after = state
        .load_value::<QuarantineTicket>("quarantine", &ticket.id)
        .unwrap()
        .unwrap();
    assert_eq!(after.state, QuarantineState::Deleted);
    let second = run_due(&config, &mut state).unwrap();
    assert_eq!(second["attempted"], false);
    let recorded: DateTime<Utc> = serde_json::from_value(first["at"].clone()).unwrap();
    assert_eq!(
        run_due_at(
            &config,
            &mut state,
            recorded + chrono::Duration::minutes(16)
        )
        .unwrap()["attempted"],
        false
    );
    assert_eq!(
        run_due_at(&config, &mut state, recorded + chrono::Duration::days(1)).unwrap()["attempted"],
        true
    );
    assert!(
        state
            .load_value::<Value>("maintenance-result", "fixture")
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_failed_automatic_maintenance_attempt_is_visible_and_not_repeated_every_tick() {
    let temp = tempfile::tempdir().unwrap();
    let unavailable = temp.path().join("storage");
    fs::write(&unavailable, "not a directory").unwrap();
    let config = Config {
        host: "fixture".into(),
        state_dir: temp.path().join("state"),
        destinations: [(
            "local".into(),
            Destination {
                location: unavailable.display().to_string(),
                encrypted: false,
                ..Destination::default()
            },
        )]
        .into(),
        ..Config::default()
    };
    let mut state = State::open(&config.state_dir).unwrap();
    let first = run_due(&config, &mut state).unwrap();
    assert_eq!(first["attempted"], true);
    assert_eq!(first["result"]["warnings"].as_array().unwrap().len(), 1);
    assert_eq!(run_due(&config, &mut state).unwrap()["attempted"], false);
    let manual = cleanup(&config, &mut state, false).unwrap();
    assert_eq!(manual["errors"].as_array().unwrap().len(), 1);
    let recorded: DateTime<Utc> = serde_json::from_value(first["at"].clone()).unwrap();
    assert_eq!(
        run_due_at(
            &config,
            &mut state,
            recorded + chrono::Duration::minutes(14)
        )
        .unwrap()["attempted"],
        false
    );
    let retry = run_due_at(
        &config,
        &mut state,
        recorded + chrono::Duration::minutes(15),
    )
    .unwrap();
    assert_eq!(retry["attempted"], true);
    assert_eq!(retry["succeeded"], false);
}

#[test]
fn successful_manual_cleanup_replaces_warning_but_preview_preserves_history() {
    let temp = tempfile::tempdir().unwrap();
    let storage = temp.path().join("storage");
    fs::write(&storage, "unavailable").unwrap();
    let config = Config {
        host: "fixture".into(),
        state_dir: temp.path().join("state"),
        destinations: [(
            "local".into(),
            Destination {
                location: storage.display().to_string(),
                encrypted: false,
                ..Destination::default()
            },
        )]
        .into(),
        ..Config::default()
    };
    let mut state = State::open(&config.state_dir).unwrap();
    let failed = run_due(&config, &mut state).unwrap();
    let previous: DateTime<Utc> = state
        .load_value("maintenance-attempt", "fixture")
        .unwrap()
        .unwrap();
    assert_eq!(failed["succeeded"], false);
    fs::remove_file(&storage).unwrap();
    fs::create_dir(&storage).unwrap();
    assert_eq!(
        cleanup(&config, &mut state, false).unwrap()["errors"],
        json!([])
    );
    assert_eq!(
        state
            .load_value::<Value>("maintenance-result", "fixture")
            .unwrap()
            .unwrap(),
        failed
    );
    assert_eq!(
        state
            .load_value::<DateTime<Utc>>("maintenance-attempt", "fixture")
            .unwrap()
            .unwrap(),
        previous
    );
    assert_eq!(
        cleanup(&config, &mut state, true).unwrap()["errors"],
        json!([])
    );
    let successful: Value = state
        .load_value("maintenance-result", "fixture")
        .unwrap()
        .unwrap();
    let at: DateTime<Utc> = state
        .load_value("maintenance-attempt", "fixture")
        .unwrap()
        .unwrap();
    assert_eq!(successful["manual"], true);
    assert_eq!(successful["succeeded"], true);
    assert_eq!(successful["result"]["warnings"], json!([]));
    assert_eq!(successful["at"], json!(at));
    assert_eq!(
        run_due_at(&config, &mut state, at + chrono::Duration::minutes(16)).unwrap()["attempted"],
        false
    );
}

#[test]
fn manual_and_automatic_attempts_share_the_same_lock() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config {
        host: "fixture".into(),
        state_dir: temp.path().join("state"),
        ..Config::default()
    };
    let mut state = State::open(&config.state_dir).unwrap();
    let lock = state.lock("maintenance:fixture").unwrap();
    assert!(cleanup(&config, &mut state, true).is_err());
    assert!(run_due(&config, &mut state).is_err());
    assert!(
        state
            .load_value::<Value>("maintenance-attempt", "fixture")
            .unwrap()
            .is_none()
    );
    assert!(cleanup(&config, &mut state, false).is_ok());
    drop(lock);
    assert!(cleanup(&config, &mut state, true).is_ok());
}

#[test]
fn failed_manual_attempt_records_failure_and_retries_instead_of_reusing_old_success() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config {
        host: "fixture".into(),
        state_dir: temp.path().join("state"),
        ..Config::default()
    };
    let mut state = State::open(&config.state_dir).unwrap();
    cleanup(&config, &mut state, true).unwrap();
    let held = state.lock("quarantine-cleanup").unwrap();
    assert!(cleanup(&config, &mut state, true).is_err());
    let failure: Value = state
        .load_value("maintenance-result", "fixture")
        .unwrap()
        .unwrap();
    assert_eq!(failure["manual"], true);
    assert_eq!(failure["succeeded"], false);
    assert!(
        failure["warning"]
            .as_str()
            .unwrap()
            .contains("quarantine-cleanup")
    );
    let at: DateTime<Utc> = state
        .load_value("maintenance-attempt", "fixture")
        .unwrap()
        .unwrap();
    drop(held);
    let retry = run_due_at(&config, &mut state, at + chrono::Duration::minutes(15)).unwrap();
    assert_eq!(retry["attempted"], true);
    assert_eq!(retry["succeeded"], true);
}

#[test]
fn pending_source_cleanup_and_interrupted_attempts_retry_after_fifteen_minutes() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config {
        host: "fixture".into(),
        state_dir: temp.path().join("state"),
        ..Config::default()
    };
    let mut state = State::open(&config.state_dir).unwrap();
    let progress = backups::BackupCleanup {
        run_id: "pending-run".into(),
        host: "fixture".into(),
        job: "documents".into(),
        tickets: vec![],
        pending: [(temp.path().join("source"), "recently modified".into())].into(),
        directory_modes: Default::default(),
    };
    state
        .save_value("backup-cleanup", &progress.run_id, &progress)
        .unwrap();
    let now = Utc::now();
    let pending = run_due_at(&config, &mut state, now).unwrap();
    assert_eq!(pending["succeeded"], false);
    assert_eq!(pending["result"]["warnings"], json!([]));
    cleanup(&config, &mut state, true).unwrap();
    assert_eq!(
        state
            .load_value::<Value>("maintenance-result", "fixture")
            .unwrap()
            .unwrap()["succeeded"],
        false
    );
    let manual_at: DateTime<Utc> = state
        .load_value("maintenance-attempt", "fixture")
        .unwrap()
        .unwrap();
    assert_eq!(
        run_due_at(
            &config,
            &mut state,
            manual_at + chrono::Duration::minutes(15)
        )
        .unwrap()["attempted"],
        true
    );
    let success = json!({"attempted":true,"at":now,"result":{"warnings":[]}});
    state
        .save_value("maintenance-result", "fixture", &success)
        .unwrap();
    let interrupted = now + chrono::Duration::hours(1);
    state
        .save_value("maintenance-attempt", "fixture", &interrupted)
        .unwrap();
    assert_eq!(
        run_due_at(
            &config,
            &mut state,
            interrupted + chrono::Duration::minutes(14)
        )
        .unwrap()["attempted"],
        false
    );
    assert_eq!(
        run_due_at(
            &config,
            &mut state,
            interrupted + chrono::Duration::minutes(15)
        )
        .unwrap()["attempted"],
        true
    );
}
