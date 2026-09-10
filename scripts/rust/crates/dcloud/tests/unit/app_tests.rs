use super::*;
use crate::config::Job;
use crate::state::{ReplicaReceipt, ReplicaState, RunRecord, RunState};

#[test]
fn offline_metadata_and_encrypted_recovery_export_do_not_decrypt_credentials() {
    assert!(!needs_credentials(&Command::Status {
        overdue: false,
        local: true
    }));
    assert!(!needs_credentials(&Command::Browse {
        filter: Filter::default(),
        offline: true,
        tui: false,
        snapshot: None
    }));
    assert!(!needs_credentials(&Command::RecoveryExport {
        to: "/recovery".into()
    }));
    assert!(needs_credentials(&Command::Doctor { remote: false }));
}

#[test]
fn nested_maintenance_failures_produce_a_failing_exit_status() {
    assert!(failed(
        &json!({"results":[{"maintenance":[{"error":"retention failed"}]}]})
    ));
    assert!(failed(
        &json!({"backups":{"errors":["offline"]},"uploads":{"verified":true}})
    ));
    assert!(!failed(
        &json!({"results":[{"success":true,"error":null}],"errors":[]})
    ));
}

#[test]
fn pending_source_cleanup_is_visible_without_reclassifying_or_mutating_backups() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = State::open(temp.path()).unwrap();
    let config = Config {
        host: "macie".into(),
        state_dir: temp.path().to_path_buf(),
        ..Config::default()
    };
    let progress = backups::BackupCleanup {
        run_id: "captured".into(),
        host: "macie".into(),
        job: "Documents".into(),
        tickets: Vec::new(),
        pending: [(
            "/Documents".into(),
            "source was edited after capture".into(),
        )]
        .into(),
        directory_modes: Default::default(),
    };
    state
        .save_value("backup-cleanup", "captured", &progress)
        .unwrap();
    let before = serde_json::to_value(&progress).unwrap();
    let status = crate::status::local(&config, Some(&state)).unwrap();
    assert_eq!(status["pending_cleanup"][0]["backup"], "verified");
    assert!(!failed(&status));
    let preview = cleanup(&config, &mut state, false).unwrap();
    assert_eq!(preview["source_cleanup"]["applied"], false);
    assert_eq!(
        preview["source_cleanup"]["pending"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(!failed(&preview));
    let after = state
        .load_value::<Value>("backup-cleanup", "captured")
        .unwrap()
        .unwrap();
    assert_eq!(before, after);
}

#[test]
fn status_uses_verification_time_and_keeps_restore_history() {
    let temp = tempfile::tempdir().unwrap();
    let mut state = State::open(temp.path()).unwrap();
    let config = Config {
        host: "macie".into(),
        jobs: [(
            "Documents".into(),
            Job {
                sources: [
                    ("macie".into(), vec!["/Documents".into()]),
                    ("archie".into(), vec!["/Documents".into()]),
                ]
                .into(),
                destinations: vec!["drive".into()],
                ..Job::default()
            },
        )]
        .into(),
        ..Config::default()
    };
    let now = Utc::now();
    let mut run = RunRecord::new("macie", "Documents", "digest");
    run.started = now - chrono::Duration::days(30);
    run.state = RunState::Committed;
    run.snapshot = Some("capture".into());
    let full = now - chrono::Duration::days(2);
    run.replicas.insert(
        "drive".into(),
        ReplicaReceipt {
            destination: "drive".into(),
            snapshot: Some("old".into()),
            offsite: true,
            state: ReplicaState::Verified,
            verified_at: Some(full),
            full_verified_at: Some(full),
            error: None,
        },
    );
    state.save_run(&run).unwrap();
    run.id = "new-run".into();
    run.started = now - chrono::Duration::days(20);
    let receipt = run.replicas.get_mut("drive").unwrap();
    receipt.verified_at = Some(now);
    receipt.full_verified_at = None;
    receipt.snapshot = Some("latest".into());
    state.save_run(&run).unwrap();
    let result = crate::status::local(&config, Some(&state)).unwrap();
    let rows = result["items"].as_array().unwrap();
    let local = rows.iter().find(|row| row["host"] == "macie").unwrap();
    assert_eq!(local["overdue"], false);
    assert_eq!(local["snapshot"], "latest");
    assert_eq!(local["last_full_restore"], json!(full));
    assert!(rows.iter().all(|row| row["host"] == "macie"));
}
