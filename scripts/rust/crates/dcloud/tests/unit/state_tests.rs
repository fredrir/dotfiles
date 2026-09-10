use super::*;

#[test]
fn durable_state_requires_a_committed_matching_run_for_occurrence() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let instant = DateTime::parse_from_rfc3339("2026-09-06T01:00:00Z")?.with_timezone(&Utc);
    let mut run = RunRecord::new("archie", "documents", "config-a");
    {
        let mut state = State::open(directory.path())?;
        state.save_run(&run)?;
        assert!(
            state
                .set_occurrence("archie", "documents", instant, &run.id)
                .is_err()
        );
        run.snapshot = Some("snapshot-1".into());
        run.state = RunState::Committed;
        state.save_run(&run)?;
        assert!(
            state
                .set_occurrence("macie", "documents", instant, &run.id)
                .is_err()
        );
        state.set_occurrence("archie", "documents", instant, &run.id)?;
        assert!(
            state
                .set_occurrence(
                    "archie",
                    "documents",
                    instant - chrono::Duration::days(7),
                    &run.id
                )
                .is_err()
        );
    }
    let state = State::open(directory.path())?;
    assert_eq!(state.occurrence("archie", "documents")?, Some(instant));
    assert_eq!(
        state.load_run(&run.id)?.unwrap().snapshot.as_deref(),
        Some("snapshot-1")
    );
    assert_eq!(state.runs()?.len(), 1);
    Ok(())
}

#[test]
fn committed_identity_and_snapshot_cannot_change() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut state = State::open(directory.path())?;
    let mut run = RunRecord::new("archie", "photos", "config-a");
    run.state = RunState::Committed;
    run.snapshot = Some("original".into());
    state.save_run(&run)?;
    let mut changed = run.clone();
    changed.state = RunState::Failed;
    assert!(state.save_run(&changed).is_err());
    changed = run.clone();
    changed.config_hash = "changed".into();
    assert!(state.save_run(&changed).is_err());
    changed = run.clone();
    changed.snapshot = Some("replacement".into());
    assert!(state.save_run(&changed).is_err());
    assert_eq!(state.load_run(&run.id)?.unwrap().snapshot, run.snapshot);
    Ok(())
}

#[test]
fn competing_connections_cannot_own_the_same_job() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let state = State::open(directory.path())?;
    let other = State::open(directory.path())?;
    let owner = state.lock("archie/documents")?;
    assert!(other.lock("archie/documents").is_err());
    let independent = other.lock("macie/documents")?;
    drop(owner);
    let takeover = other.lock("archie/documents")?;
    drop(takeover);
    drop(independent);
    Ok(())
}

#[test]
fn journal_and_locks_survive_process_termination() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let child = std::process::Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "state::tests::crash_child_fixture",
            "--nocapture",
        ])
        .env("DCLOUD_TEST_CRASH_STATE", directory.path())
        .status()?;
    assert!(child.success());
    let state = State::open(directory.path())?;
    let _recovered = state.lock("crashed-job")?;
    let runs = state.runs()?;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].state, RunState::Replicating);
    assert!(state.occurrence("archie", "documents")?.is_none());
    Ok(())
}

#[test]
fn crash_child_fixture() -> Result<()> {
    let Some(path) = std::env::var_os("DCLOUD_TEST_CRASH_STATE") else {
        return Ok(());
    };
    let mut state = State::open(Path::new(&path))?;
    let _lock = state.lock("crashed-job")?;
    let mut run = RunRecord::new("archie", "documents", "hash");
    run.state = RunState::Replicating;
    state.save_run(&run)?;
    std::process::exit(0);
}

#[test]
fn cache_and_auxiliary_records_are_rebuildable_and_persisted() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let state = State::open(directory.path())?;
    state.cache_manifest("drive", "one", &vec!["photos", "private"])?;
    state.save_value("sync", "documents", &vec!["left", "right"])?;
    drop(state);
    let state = State::open(directory.path())?;
    let manifests = state.cached_manifests::<Vec<String>>("drive")?;
    assert_eq!(manifests[0].0, "one");
    assert_eq!(manifests[0].2, vec!["photos", "private"]);
    assert_eq!(
        state.load_value::<Vec<String>>("sync", "documents")?,
        Some(vec!["left".into(), "right".into()])
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn state_refuses_symlink_database_and_lock() -> Result<()> {
    use std::os::unix::fs::symlink;
    let directory = tempfile::tempdir()?;
    let target = directory.path().join("unrelated");
    fs::write(&target, "preserved")?;
    symlink(&target, directory.path().join("state.sqlite3"))?;
    assert!(State::open(directory.path()).is_err());
    assert_eq!(fs::read_to_string(&target)?, "preserved");
    Ok(())
}
