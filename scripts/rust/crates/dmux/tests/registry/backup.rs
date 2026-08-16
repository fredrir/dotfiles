//! WAL-safe online backup (plan §10.1): SQLite's online backup API after a
//! checked WAL checkpoint — never a bare file copy — including during
//! concurrent writes, with restore-and-verify.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dmux::registry::{Registry, RegistryConfig};
use uuid::Uuid;

use crate::util::{fast_busy, finalize, open, reserve, scratch, tmux_instance};

#[test]
fn quiescent_backup_checkpoints_cleanly_and_restores() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);
    for i in 0..4 {
        let r = reserve(&mut reg, &format!("s{i}"), instance);
        finalize(&mut reg, &r, &format!("${i}"));
    }

    let dest = s.dir.path().join("backup.sqlite3");
    let report = reg.backup_to(&dest).unwrap();
    // Checked checkpoint: quiescent database checkpoints without busy loops
    // and every WAL frame lands in the main file.
    assert_eq!(report.checkpoint_attempts, 0);
    assert_eq!(report.wal_frames, report.checkpointed_frames);

    // Restore-and-verify: the backup opens as a full registry with the same
    // identity and rows.
    let restored = Registry::open(RegistryConfig {
        db_path: dest,
        lock_dir: s.dir.path().join("locks-restored"),
        busy: fast_busy(),
    })
    .unwrap();
    assert_eq!(restored.identity().unwrap(), reg.identity().unwrap());
    assert_eq!(restored.spaces().unwrap(), reg.spaces().unwrap());
    assert_eq!(
        restored.authority_head().unwrap(),
        reg.authority_head().unwrap()
    );
}

#[test]
fn online_backup_during_writes_yields_a_consistent_snapshot() {
    let s = scratch();
    let mut reg = open(&s.config);
    let instance = tmux_instance(&mut reg);
    for i in 0..5 {
        let r = reserve(&mut reg, &format!("pre{i}"), instance);
        finalize(&mut reg, &r, &format!("$p{i}"));
    }
    let pre_count = reg.spaces().unwrap().len();

    // A concurrent writer keeps reserving/finalizing while we back up.
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let config = s.config.clone();
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut reg = Registry::open(config).unwrap();
            let mut made = 0u32;
            let mut i = 0u32;
            while !stop.load(Ordering::Relaxed) {
                match reg.reserve_space(&format!("w{i}"), instance, Uuid::new_v4()) {
                    Ok(r) => {
                        // Best effort under contention; busy is fine.
                        if reg
                            .finalize_create(
                                r.space_uid,
                                r.operation_uid,
                                &dmux::registry::NativeBindingSpec {
                                    native_token: format!("$w{i}"),
                                    native_kind: dmux::registry::NativeKind::TmuxSessionId,
                                    server_epoch: None,
                                },
                            )
                            .is_ok()
                        {
                            made += 1;
                        }
                    }
                    Err(dmux::registry::RegistryError::Busy) => {}
                    Err(e) => panic!("writer failed: {e}"),
                }
                i += 1;
                std::thread::sleep(Duration::from_millis(2));
            }
            made
        })
    };

    std::thread::sleep(Duration::from_millis(15));
    let dest = s.dir.path().join("backup.sqlite3");
    let report = reg.backup_to(&dest).unwrap();
    stop.store(true, Ordering::Relaxed);
    let made = writer.join().unwrap();
    assert!(
        made > 0,
        "the writer must actually have written during backup"
    );
    let _ = report;

    // The snapshot is internally consistent (integrity-checked by
    // backup_to) and opens as a registry with the same identity.
    let restored = Registry::open(RegistryConfig {
        db_path: dest,
        lock_dir: s.dir.path().join("locks-restored"),
        busy: fast_busy(),
    })
    .unwrap();
    assert_eq!(restored.identity().unwrap(), reg.identity().unwrap());

    // Consistent point-in-time: at least the pre-backup rows, unique
    // numbers, a chain head whose recorded row exists.
    let spaces = restored.spaces().unwrap();
    assert!(spaces.len() >= pre_count);
    let mut nos: Vec<u64> = spaces.iter().map(|s| s.space_no.get()).collect();
    nos.sort_unstable();
    nos.dedup();
    assert_eq!(nos.len(), spaces.len(), "duplicate SpaceNo in snapshot");
    let head = restored.authority_head().unwrap();
    if head.revision > 0 {
        let chain = restored.revision_chain().unwrap();
        assert_eq!(chain.last().unwrap().revision, head.revision);
        assert_eq!(chain.last().unwrap().head_hash, head.head_hash);
    }
}
