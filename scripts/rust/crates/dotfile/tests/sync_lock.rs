#![forbid(unsafe_code)]

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

#[cfg(unix)]
#[test]
fn invalid_owner_pids_do_not_probe_process_groups() {
    for owner in ["0", "4294967295"] {
        let directory = tree(&[&format!("sync.lock={owner}\n")]);
        assert!(SyncLock::acquire(directory.path()).is_ok());
    }
}
