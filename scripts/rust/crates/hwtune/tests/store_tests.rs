#![forbid(unsafe_code)]

use hwtune::bench::{record::Run, store::Store};
use serde_json::json;
use std::fs;

fn history() -> (tempfile::TempDir, Store) {
    let temporary = tempfile::tempdir().unwrap();
    let store = Store::new(temporary.path().join("history"));
    (temporary, store)
}

fn run(id: &str) -> Run {
    Run {
        host: "archie".into(),
        run_id: id.into(),
        started: "2026-09-11T10:00:00Z".into(),
        grade: "clean".into(),
        tier: "quick".into(),
        snapshot: json!({"cpu":{"model":"Test CPU"}}),
        ..Run::default()
    }
}

#[test]
fn reading_an_absent_store_does_not_create_it() {
    let (_temporary, store) = history();
    assert!(store.known_hosts().unwrap().is_empty());
    assert!(store.list_runs(Some("archie"), &[]).unwrap().is_empty());
    assert!(store.load_baselines().unwrap().is_empty());
    assert!(store.baseline_run("archie", "epoch").unwrap().is_none());
    assert!(!store.clear_baseline("archie", "epoch").unwrap());
    assert!(!store.root.exists());
}

#[test]
fn a_run_id_cannot_overwrite_a_record_or_its_pinned_baseline() {
    let (_temporary, store) = history();
    let original = run("immutable");
    let path = store.save_run(&original).unwrap();
    let original_bytes = fs::read(&path).unwrap();
    store
        .set_baseline(&original.host, &original.epoch(), &original.run_id)
        .unwrap();
    let mut replacement = original.clone();
    replacement.note = "changed".into();
    assert!(store.save_run(&replacement).is_err());
    assert_eq!(fs::read(path).unwrap(), original_bytes);
    assert_eq!(
        store
            .baseline_run("archie", &original.epoch())
            .unwrap()
            .unwrap(),
        original
    );
}

#[test]
fn unknown_store_versions_block_reads_and_writes() {
    let (_temporary, store) = history();
    fs::create_dir_all(&store.root).unwrap();
    fs::write(store.root.join("store.json"), r#"{"schema":99}"#).unwrap();
    for error in [
        store.list_runs(None, &[]).unwrap_err(),
        store.load_baselines().unwrap_err(),
        store.save_run(&run("blocked")).unwrap_err(),
    ] {
        assert!(
            error.contains("unsupported benchmark storage schema 99"),
            "{error}"
        );
    }
    assert!(!store.root.join("hosts").exists());
}

#[test]
fn unversioned_nonempty_storage_is_rejected() {
    let (_temporary, store) = history();
    fs::create_dir_all(store.root.join("unexpected")).unwrap();
    assert!(
        store
            .list_runs(None, &[])
            .unwrap_err()
            .contains("missing store.json")
    );
    assert!(store.save_run(&run("blocked")).is_err());
    assert!(!store.root.join("store.json").exists());
}

#[test]
fn malformed_and_misidentified_records_are_reported_with_their_paths() {
    let (_temporary, store) = history();
    let original = run("bad");
    let path = store.save_run(&original).unwrap();
    for content in [
        "{broken".to_owned(),
        json!({"host":"macie","run_id":"bad"}).to_string(),
    ] {
        fs::write(&path, content).unwrap();
        let error = store.list_runs(None, &[]).unwrap_err();
        assert!(error.contains(path.to_str().unwrap()), "{error}");
    }
}

#[test]
fn baseline_pins_require_existing_runs_from_the_matching_epoch() {
    let (_temporary, store) = history();
    let original = run("pinned");
    store.save_run(&original).unwrap();
    assert!(
        store
            .set_baseline("archie", &original.epoch(), "absent")
            .is_err()
    );
    assert!(
        store
            .set_baseline("archie", "wrong", &original.run_id)
            .is_err()
    );
    store
        .set_baseline("archie", &original.epoch(), &original.run_id)
        .unwrap();
    fs::remove_file(store.run_path("archie", &original.run_id).unwrap()).unwrap();
    assert!(
        store
            .baseline_run("archie", &original.epoch())
            .unwrap_err()
            .contains("missing run")
    );
}

#[test]
fn corrupt_and_unknown_baseline_manifests_are_reported() {
    let (_temporary, store) = history();
    let original = run("pinned");
    store.save_run(&original).unwrap();
    store
        .set_baseline("archie", &original.epoch(), &original.run_id)
        .unwrap();
    let path = store.root.join("hosts/archie/baselines.json");
    for content in ["{broken", r#"{"schema":99,"pins":{}}"#] {
        fs::write(&path, content).unwrap();
        let error = store.load_baselines().unwrap_err();
        assert!(error.contains(path.to_str().unwrap()), "{error}");
    }
}

#[test]
fn host_scoped_session_paths_reject_traversal() {
    let (_temporary, store) = history();
    assert_eq!(
        store.stability_path("archie", "session").unwrap(),
        store.root.join("hosts/archie/stability/session.json")
    );
    assert_eq!(
        store.tuning_path("archie", "session").unwrap(),
        store.root.join("hosts/archie/tuning/session.json")
    );
    for invalid in ["..", "../archie", "a/b", "a\\b", ""] {
        assert!(store.stability_path(invalid, "session").is_err());
        assert!(store.tuning_path("archie", invalid).is_err());
    }
    assert!(!store.root.exists());
}

#[test]
fn records_from_the_same_second_are_sorted_newest_first() {
    let (_temporary, store) = history();
    for id in [
        "2026-09-11T10-00-00.000000001Z",
        "2026-09-11T10-00-00.000000003Z",
        "2026-09-11T10-00-00.000000002Z",
    ] {
        store.save_run(&run(id)).unwrap();
    }
    let ids = store
        .list_runs(Some("archie"), &[])
        .unwrap()
        .into_iter()
        .map(|run| run.run_id)
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        [
            "2026-09-11T10-00-00.000000003Z",
            "2026-09-11T10-00-00.000000002Z",
            "2026-09-11T10-00-00.000000001Z"
        ]
    );
}

#[cfg(unix)]
#[test]
fn host_symlinks_cannot_redirect_record_reads_or_writes() {
    let (_temporary, store) = history();
    let outside = tempfile::tempdir().unwrap();
    store.initialize().unwrap();
    fs::create_dir(store.root.join("hosts")).unwrap();
    std::os::unix::fs::symlink(outside.path(), store.root.join("hosts/archie")).unwrap();
    assert!(store.save_run(&run("escape")).is_err());
    assert!(store.list_runs(Some("archie"), &[]).is_err());
    assert!(outside.path().read_dir().unwrap().next().is_none());
}

#[test]
fn pruning_keeps_measurements_linked_to_tuning_evidence() {
    let (_temporary, store) = history();
    for day in 1..=6 {
        let mut measured = run(&format!("run-{day}"));
        measured.started = format!("2026-09-{day:02}T10:00:00Z");
        if day == 3 {
            measured.context = Some(hwtune::bench::provenance::RunContext {
                tuning_session: Some("tuning-session".into()),
                ..Default::default()
            });
        }
        store.save_run(&measured).unwrap();
    }
    let ids = store
        .prunable(Some("archie"), 2)
        .unwrap()
        .into_iter()
        .map(|run| run.run_id)
        .collect::<Vec<_>>();
    assert_eq!(ids, ["run-4", "run-2"]);
}

fn lock_process(path: &std::path::Path, mode: &str) -> std::process::Output {
    std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "measurement_lock_subprocess", "--nocapture"])
        .env("HWTUNE_MEASUREMENT_LOCK", path)
        .env("HWTUNE_LOCK_TEST_MODE", mode)
        .env("HOME", path.with_extension("other-home"))
        .env("XDG_CACHE_HOME", path.with_extension("other-cache"))
        .env("XDG_RUNTIME_DIR", path.with_extension("other-runtime"))
        .output()
        .unwrap()
}

#[test]
fn measurement_lock_subprocess() {
    let Ok(mode) = std::env::var("HWTUNE_LOCK_TEST_MODE") else {
        return;
    };
    let result = hwtune::bench::store::measurement_lock();
    match mode.as_str() {
        "acquire" => assert!(result.is_ok(), "{}", result.err().unwrap_or_default()),
        "blocked" => assert!(result.err().unwrap().contains("already running")),
        "reject" => assert!(result.is_err()),
        _ => panic!("unexpected measurement lock test mode"),
    }
}

#[test]
fn measurement_lock_is_shared_across_processes_without_mutating_existing_files() {
    use fs2::FileExt;
    use std::os::unix::fs::PermissionsExt;
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("measurement.lock");
    fs::write(&path, b"existing lock contents").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let held = fs::File::open(&path).unwrap();
    held.try_lock_exclusive().unwrap();
    let blocked = lock_process(&path, "blocked");
    assert!(blocked.status.success(), "{blocked:?}");
    drop(held);
    let acquired = lock_process(&path, "acquire");
    assert!(acquired.status.success(), "{acquired:?}");
    assert_eq!(fs::read(&path).unwrap(), b"existing lock contents");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o644
    );
    assert!(!path.with_extension("other-runtime").exists());
    assert!(!path.with_extension("other-cache").exists());
    assert!(!path.with_extension("other-home").exists());
}

#[test]
fn new_measurement_locks_are_readable_by_all_users_and_reusable() {
    use std::os::unix::fs::PermissionsExt;
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("measurement.lock");
    for _ in 0..2 {
        let acquired = lock_process(&path, "acquire");
        assert!(acquired.status.success(), "{acquired:?}");
    }
    let metadata = fs::metadata(&path).unwrap();
    assert!(metadata.is_file());
    assert_eq!(metadata.len(), 0);
    assert_eq!(metadata.permissions().mode() & 0o777, 0o444);
    assert_eq!(temporary.path().read_dir().unwrap().count(), 1);
}

#[test]
fn measurement_lock_rejects_symlinks_directories_and_fifos_without_writing() {
    let temporary = tempfile::tempdir().unwrap();
    let target = temporary.path().join("untouched");
    fs::write(&target, b"untouched contents").unwrap();
    let link = temporary.path().join("symlink.lock");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let directory = temporary.path().join("directory.lock");
    fs::create_dir(&directory).unwrap();
    let fifo = temporary.path().join("fifo.lock");
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .unwrap();
    for path in [&link, &directory, &fifo] {
        let rejected = lock_process(path, "reject");
        assert!(rejected.status.success(), "{rejected:?}");
    }
    assert_eq!(fs::read(target).unwrap(), b"untouched contents");
}
