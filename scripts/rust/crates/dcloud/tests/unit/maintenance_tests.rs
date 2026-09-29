use super::*;
use crate::config::Destination;
use crate::policy::{QuarantineState, QuarantineTicket};
use crate::state::RunState;
use std::fs;

#[test]
fn manual_cleanup_reaps_opted_in_quarantine_after_full_restore() {
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
    let first = cleanup(&config, &mut state, true).unwrap();
    assert_eq!(first["errors"], json!([]), "{first}");
    assert_eq!(first["quarantine"][0]["deleted"], true);
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
    assert!(
        state
            .load_value::<Value>("maintenance-result", "fixture")
            .unwrap()
            .is_some()
    );
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
    let failed = cleanup(&config, &mut state, true).unwrap();
    assert_eq!(failed["errors"].as_array().unwrap().len(), 1);
    let recorded: Value = state
        .load_value("maintenance-result", "fixture")
        .unwrap()
        .unwrap();
    assert_eq!(recorded["succeeded"], false);
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
        recorded
    );
    assert_eq!(
        cleanup(&config, &mut state, true).unwrap()["errors"],
        json!([])
    );
    let successful: Value = state
        .load_value("maintenance-result", "fixture")
        .unwrap()
        .unwrap();
    assert_eq!(successful["manual"], true);
    assert_eq!(successful["succeeded"], true);
    assert_eq!(successful["result"]["warnings"], json!([]));
}

#[test]
fn manual_cleanup_respects_the_maintenance_lock() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config {
        host: "fixture".into(),
        state_dir: temp.path().join("state"),
        ..Config::default()
    };
    let mut state = State::open(&config.state_dir).unwrap();
    let lock = state.lock("maintenance:fixture").unwrap();
    assert!(cleanup(&config, &mut state, true).is_err());
    assert!(
        state
            .load_value::<Value>("maintenance-result", "fixture")
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
    drop(held);
    cleanup(&config, &mut state, true).unwrap();
    let retry: Value = state
        .load_value("maintenance-result", "fixture")
        .unwrap()
        .unwrap();
    assert_eq!(retry["succeeded"], true);
}

#[test]
fn pending_source_cleanup_marks_the_manual_attempt_unsuccessful() {
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
    cleanup(&config, &mut state, true).unwrap();
    assert_eq!(
        state
            .load_value::<Value>("maintenance-result", "fixture")
            .unwrap()
            .unwrap()["succeeded"],
        false
    );
}
