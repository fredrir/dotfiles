use dotfile_cli::lock::SyncLock;
use testkit::{TempDir, tree};

#[test]
fn concurrent_runs_are_refused_and_release_restores_access() {
    let directory = TempDir::new().unwrap();
    let first = SyncLock::acquire(directory.path()).unwrap();
    assert!(SyncLock::acquire(directory.path()).is_err());
    drop(first);
    assert!(SyncLock::acquire(directory.path()).is_ok());
}

#[test]
fn stale_owner_is_replaced() {
    let directory = tree(&["sync.lock=999999\n"]);
    assert!(SyncLock::acquire(directory.path()).is_ok());
}
