use std::fs;

use dotfile_cli::lock::SyncLock;

#[test]
fn concurrent_runs_are_refused_and_release_restores_access() {
    let directory = tempfile::tempdir().unwrap();
    let first = SyncLock::acquire(directory.path()).unwrap();
    assert!(SyncLock::acquire(directory.path()).is_err());
    drop(first);
    assert!(SyncLock::acquire(directory.path()).is_ok());
}

#[test]
fn stale_owner_is_replaced() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("sync.lock"), "999999\n").unwrap();
    assert!(SyncLock::acquire(directory.path()).is_ok());
}
