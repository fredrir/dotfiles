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
}
