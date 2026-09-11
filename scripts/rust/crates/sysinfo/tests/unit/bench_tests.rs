use super::{
    capture, compare, conditions, health, menu,
    record::{self, Metric, Run},
    runner,
    select::Selector,
    store::Store,
    suites,
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

fn archived(name: &str) -> Run {
    serde_json::from_str(match name {
        "archie" => include_str!("../fixtures/bench/archie-schema1.json"),
        _ => include_str!("../fixtures/bench/macie-schema1.json"),
    })
    .unwrap()
}
fn metric(values: &[f64]) -> Metric {
    Metric {
        key: "cpu.multi".into(),
        method: "cpu.multi/1.0.0".into(),
        tool: "7z".into(),
        tool_version: "26.02".into(),
        scale: "MIPS".into(),
        samples: values.into(),
        comparable: "world".into(),
        ..Metric::default()
    }
}
fn sample(day: usize) -> Run {
    Run {
        run_id: format!("2026-08-{day:02}T09-00-00Z-10db7d1f"),
        started: format!("2026-08-{day:02}T09:00:00Z"),
        metrics: vec![metric(&[100.0, 100.0, 100.0])],
        ..archived("archie")
    }
}
fn store() -> (tempfile::TempDir, Store) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().join("benchmarks"));
    (root, store)
}

#[test]
fn existing_schema_one_histories_keep_the_exact_python_blake2s_epochs() {
    for (host, epoch) in [("archie", "10db7d1f"), ("macie", "31c0c4e0")] {
        let run = archived(host);
        assert_eq!(run.epoch(), epoch);
        let restored: Run = serde_json::from_value(run.to_json()).unwrap();
        assert_eq!(restored, run);
        assert_eq!(restored.schema, 1);
        assert!(!restored.metrics.is_empty());
    }
}
#[test]
fn identity_survives_root_permissions_enumeration_order_and_capacity_rounding() {
    let run = archived("archie");
    let mut snapshot = run.snapshot.clone();
    snapshot["memory"]["modules"] = json!(0);
    snapshot["gpu"].as_array_mut().unwrap().reverse();
    snapshot["disks"].as_array_mut().unwrap().reverse();
    assert_eq!(record::epoch_of(&snapshot), run.epoch());
    for gpu in snapshot["gpu"].as_array_mut().unwrap() {
        if gpu["memory_total"].as_f64().is_some_and(|n| n > 16e9) {
            gpu["memory_total"] = json!(16648896512.0);
        }
    }
    assert_eq!(record::epoch_of(&snapshot), run.epoch());
    snapshot["memory"]["total"] = json!(68719476736_u64);
    assert_ne!(record::epoch_of(&snapshot), run.epoch());
}
#[test]
fn metric_uses_sample_standard_deviation_and_median_absolute_deviation() {
    let metric = metric(&[90.0, 100.0, 110.0]);
    assert_eq!(metric.median(), Some(100.0));
    assert_eq!(metric.mad(), Some(10.0));
    assert_eq!(metric.rsd_pct(), 10.0);
    assert_eq!(record::median(&[1.0, 2.0]), Some(1.5));
    assert_eq!(Metric::default().median(), None);
}
#[test]
fn singleton_noise_floor_does_not_create_false_disk_regressions() {
    assert_eq!(
        compare::noise_band(&metric(&[1000.0]), &metric(&[979.0])),
        8.0
    );
    assert_eq!(
        compare::noise_band(&metric(&[100.0; 3]), &metric(&[100.0; 3])),
        2.0
    );
}
#[test]
fn comparison_obeys_direction_method_tool_scope_and_dirty_configuration() {
    let mut left = sample(1);
    let mut right = sample(2);
    right.metrics[0].samples = vec![50.0; 3];
    assert_eq!(
        compare::compare_runs(&left, &right).deltas[0].verdict,
        "worse"
    );
    right.metrics[0].method = "cpu.multi/1.1.0".into();
    assert_eq!(
        compare::compare_runs(&left, &right).deltas[0].verdict,
        "blocked"
    );
    right.metrics[0].method = "cpu.multi/1.0.9".into();
    assert_eq!(
        compare::compare_runs(&left, &right).deltas[0].verdict,
        "worse"
    );
    right.metrics[0].tool_version = "27".into();
    assert_eq!(
        compare::compare_runs(&left, &right).deltas[0].verdict,
        "blocked"
    );
    right.metrics[0].tool_version = left.metrics[0].tool_version.clone();
    left.metrics[0].comparable = "host".into();
    right.host = "macie".into();
    assert_eq!(
        compare::compare_runs(&left, &right).deltas[0].verdict,
        "blocked"
    );
    right.host = left.host.clone();
    left.metrics[0].key = "workload.nvim".into();
    right.metrics[0].key = "workload.nvim".into();
    left.dotfiles_sha = "abc-dirty".into();
    right.dotfiles_sha = left.dotfiles_sha.clone();
    assert!(
        compare::compare_runs(&left, &right).deltas[0]
            .reason
            .contains("uncommitted")
    );
}
#[test]
fn lower_is_better_regressions_say_above_baseline() {
    let mut left = sample(1);
    let mut right = sample(2);
    left.metrics[0].proportion = "LIB".into();
    left.metrics[0].samples = vec![20.0];
    right.metrics[0].samples = vec![30.0];
    let delta = &compare::compare_runs(&left, &right).deltas[0];
    assert_eq!(delta.verdict, "worse");
    assert!(
        health::regression_issue(delta, &left, &right)
            .title
            .contains("50% above")
    );
}
#[test]
fn store_loads_real_schema_one_pins_without_orphaning_baselines() {
    let (_root, store) = store();
    let run = archived("archie");
    let _lock = store.exclusive().unwrap();
    store.save_run(&run).unwrap();
    store
        .set_baseline(&run.host, "10db7d1f", &run.run_id)
        .unwrap();
    assert_eq!(
        store.baseline_run("archie", "10db7d1f").unwrap().unwrap(),
        run
    );
    assert!(store.load_baselines().unwrap().contains_key("archie"));
}
#[test]
fn store_ignores_corrupt_files_and_cannot_escape_history_root() {
    let (_root, store) = store();
    fs::create_dir_all(store.root.join("archie")).unwrap();
    for (index, content) in [
        b"null".as_slice(),
        b"[1,2]",
        b"{broken",
        b"{\"host\":\"archi\xe9\"}",
    ]
    .iter()
    .enumerate()
    {
        fs::write(
            store.root.join("archie").join(format!("bad{index}.json")),
            content,
        )
        .unwrap();
    }
    assert!(store.list_runs(None, &[]).unwrap().is_empty());
    assert!(store.run_path("../outside", "run").is_err());
    assert!(store.run_path("archie", "../../outside").is_err());
}
#[test]
fn benchmark_lock_is_exclusive_and_reusable() {
    let (_root, store) = store();
    let first = store.exclusive().unwrap();
    assert!(store.exclusive().is_err());
    drop(first);
    let _again = store.exclusive().unwrap();
    assert!(store.exclusive().is_err());
}
#[test]
fn atomic_updates_leave_no_partial_files_and_retain_other_hosts_pins() {
    let (_root, store) = store();
    let _lock = store.exclusive().unwrap();
    store.set_baseline("archie", "1234", "run-a").unwrap();
    store.set_baseline("macie", "5678", "run-b").unwrap();
    store.set_baseline("archie", "abcd", "run-c").unwrap();
    let pins = store.load_baselines().unwrap();
    assert_eq!(pins["macie"]["5678"], "run-b");
    assert_eq!(pins["archie"].len(), 2);
    assert!(store.clear_baseline("archie", "1234").unwrap());
    assert!(store.root.read_dir().unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("partial")
    }));
}
#[test]
fn prune_retains_newest_oldest_and_pinned_reference_per_epoch() {
    let (_root, store) = store();
    for day in 1..=6 {
        store.save_run(&sample(day)).unwrap();
    }
    let baseline = sample(3);
    store
        .set_baseline(&baseline.host, &baseline.epoch(), &baseline.run_id)
        .unwrap();
    let dropped = store
        .prunable(Some("archie"), 2)
        .unwrap()
        .into_iter()
        .map(|run| run.run_id)
        .collect::<Vec<_>>();
    assert_eq!(dropped, vec![sample(4).run_id, sample(2).run_id]);
}
#[test]
fn selector_prefers_clean_then_falls_back_to_noisy_and_supports_all_components() {
    let (_root, store) = store();
    let left = sample(1);
    let mut right = sample(2);
    right.grade = "noisy".into();
    store.save_run(&left).unwrap();
    store.save_run(&right).unwrap();
    assert_eq!(
        Selector::parse("archie")
            .resolve(&store)
            .unwrap()
            .unwrap()
            .run_id,
        left.run_id
    );
    let selector = Selector::parse(&format!(
        "archie/{}@{}:{}",
        right.os_id(),
        right.epoch(),
        right.run_id
    ));
    assert!(selector.matches(&right));
    assert_eq!(
        selector.resolve(&store).unwrap().unwrap().run_id,
        right.run_id
    );
}
#[test]
fn gates_preserve_battery_load_disk_and_throttle_meaning() {
    let c = json!({"on_battery":true,"loadavg_1":8,"cpu_count":16,"throttled_at_start":true,"free_disk_ratio":0.1,"filesystem":{"fstype":"tmpfs"}});
    assert_eq!(conditions::gate_reasons(&c, true).len(), 5);
    assert_eq!(conditions::gate_reasons(&c, false).len(), 3);
    assert_eq!(conditions::grade_for(&[], 0, &[]), "aborted");
    assert_eq!(conditions::grade_for(&[], 1, &["failed".into()]), "noisy");
}
#[test]
fn battery_charging_and_adapter_override_discharge_reports() {
    let mut s = crate::model::Snapshot::default();
    s.modules
        .insert("Battery".into(), json!([{"status":"Discharging"}]));
    assert!(conditions::on_battery(&s));
    s.modules.insert("PowerAdapter".into(), json!([{}]));
    assert!(!conditions::on_battery(&s));
}
#[test]
fn capture_removes_serials_virtual_disks_and_removable_media() {
    let mut s = crate::model::Snapshot::default();
    s.modules
        .insert("CPU".into(), json!({"cpu":"CPU","serial":"PRIVATE"}));
    s.modules.insert("PhysicalDisk".into(),json!([{"name":"ATA Real SSD","size":2000000000000_u64,"serial":"PRIVATE"},{"name":"Disk Image","size":500000},{"name":"USB","removable":true}]));
    let result = capture::describe_hardware(&s);
    assert!(!result.to_string().contains("PRIVATE"));
    assert_eq!(result["disks"].as_array().unwrap().len(), 1);
    assert_eq!(result["disks"][0]["name"], "Real SSD");
    assert!(result.get("virtualized").is_none());
}
fn counting_job(values: Vec<f64>, repeat: bool) -> (suites::Job, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let mut job = suites::job(
        "test",
        "fake",
        "1",
        "test/1.0.0",
        vec![suites::output("test.value", "units", "HIB", "world")],
        json!({}),
        move || {
            let index = counter.fetch_add(1, Ordering::SeqCst);
            Ok(suites::scalar(
                "test.value",
                values[index.min(values.len() - 1)],
            ))
        },
    );
    job.repeat = repeat;
    (job, calls)
}
#[test]
fn steady_measurements_stop_at_three_and_noisy_at_six() {
    let (mut steady, calls) = counting_job(vec![100.0], true);
    assert_eq!(
        runner::measure_job(&mut steady).unwrap()["test.value"].len(),
        3
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    let (mut noisy, calls) = counting_job(vec![50.0, 150.0, 60.0, 140.0, 70.0, 130.0], true);
    runner::measure_job(&mut noisy).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 6);
    let (mut once, calls) = counting_job(vec![1.0], false);
    runner::measure_job(&mut once).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
#[test]
fn nonfinite_measurements_are_rejected_and_empty_measurements_never_converge() {
    let (mut job, _) = counting_job(vec![f64::INFINITY], true);
    assert!(runner::measure_job(&mut job).is_err());
    assert!(!runner::converged(&BTreeMap::new()));
}
#[test]
fn fio_writes_are_size_bounded_and_expected_writes_cover_layout() {
    let writes = BTreeMap::from([
        ("seq-write".into(), "6g".into()),
        ("rand-write".into(), "2g".into()),
    ]);
    let spec = suites::disk::job_file("1g", std::path::Path::new("/tmp"), "psync", &writes);
    let write = spec
        .split("[seq-write]")
        .nth(1)
        .unwrap()
        .split("[rand-read]")
        .next()
        .unwrap();
    assert!(write.contains("io_size=6g"));
    assert!(!write.contains("time_based"));
    assert!(spec.contains("runtime=20"));
    assert_eq!(
        suites::disk::predicted_writes("1g", &writes),
        12 * 1024_u64.pow(3)
    );
    assert_eq!(spec.contains("disk_util=0"), !cfg!(target_os = "macos"));
}
#[test]
fn fio_parser_extracts_throughput_iops_latency_and_actual_writes() {
    let payload = json!({"jobs":[{"jobname":"seq-read","read":{"bw_bytes":2000000000_u64,"clat_ns":{"percentile":{"99.000000":4000}}}},{"jobname":"rand-write","write":{"iops":45000,"io_bytes":1000}}]});
    let result = suites::disk::parse(&payload).unwrap();
    assert_eq!(result.values["disk.seq_read"], vec![2000.0]);
    assert_eq!(result.values["disk.seq_read_p99"], vec![4.0]);
    assert_eq!(result.values["disk.rand_write"], vec![45000.0]);
    assert_eq!(result.values[suites::WRITTEN], vec![1000.0]);
    assert!(suites::disk::parse(&json!({"jobs":[]})).is_err());
}
#[test]
fn workload_recording_does_not_publish_repository_absolute_path() {
    let root = std::path::Path::new("/home/someone/dotfiles");
    let command = ["git", "-C", "/home/someone/dotfiles", "status"].map(String::from);
    assert_eq!(
        suites::workload::displayed(&command, root),
        "git -C . status"
    );
}
#[test]
fn menu_handles_wide_unicode_and_small_terminals_without_overflow() {
    let columns = vec![menu::Column {
        kind: "run".into(),
        title: "choose".into(),
        options: vec![("界面 configuration".into(), "some details".into()); 30],
        index: 29,
    }];
    let frame = menu::frame("bench", &columns, 20, 10);
    assert!(
        frame
            .iter()
            .all(|line| unicode_width::UnicodeWidthStr::width(line.as_str()) <= 19)
    );
    assert!(frame.len() <= 10);
}

#[test]
fn battery_status_lists_use_the_same_normalization_as_health() {
    let mut snapshot = crate::model::Snapshot::default();
    for status in [
        json!("AC Connected"),
        json!(["AC Connected"]),
        json!(["Charging"]),
    ] {
        snapshot
            .modules
            .insert("Battery".into(), json!([{"status":status}]));
        assert!(!conditions::on_battery(&snapshot));
    }
}

#[test]
fn fio_retry_is_allowed_only_before_layout_or_written_bytes() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("bench.fio"), "").unwrap();
    assert!(suites::disk::retry_before_writes(directory.path(), b""));
    assert!(!suites::disk::retry_before_writes(
        directory.path(),
        br#"{"jobs":[{"write":{"io_bytes":10}}]}"#
    ));
    fs::write(directory.path().join("seq-read.0.0"), "").unwrap();
    assert!(!suites::disk::retry_before_writes(directory.path(), b""));
}

#[test]
fn graphics_dependency_selection_matches_the_display_backend() {
    let wayland = BTreeMap::from([("WAYLAND_DISPLAY".into(), "wayland-0".into())]);
    let x11 = BTreeMap::from([("DISPLAY".into(), ":0".into())]);
    assert_eq!(
        suites::gpu::graphics_tools(&wayland),
        &["glmark2-wayland", "glmark2-es2-wayland"]
    );
    assert_eq!(
        suites::gpu::graphics_tools(&x11),
        &["glmark2", "glmark2-es2"]
    );
    assert!(suites::gpu::graphics_tools(&BTreeMap::new()).is_empty());
}

#[cfg(unix)]
#[test]
fn metal_uses_valid_cache_without_a_compiler_and_preserves_it_on_failed_rebuild() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let binary = directory.path().join("gpu_bench");
    let source = directory.path().join("gpu_bench.swift");
    fs::write(&binary, "cached").unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        &source,
        include_str!("../../src/bench/suites/metal/gpu_bench.swift"),
    )
    .unwrap();
    assert_eq!(
        suites::gpu::metal_binary_at(directory.path(), None).unwrap(),
        Some(binary.clone())
    );
    fs::write(&source, "outdated").unwrap();
    assert!(
        suites::gpu::metal_binary_at(directory.path(), None)
            .unwrap()
            .is_none()
    );
    let compiler = directory.path().join("swiftc");
    fs::write(&compiler, "#!/bin/sh\nexit 42\n").unwrap();
    fs::set_permissions(&compiler, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(suites::gpu::metal_binary_at(directory.path(), Some(&compiler)).is_err());
    assert_eq!(fs::read_to_string(&binary).unwrap(), "cached");
    assert_eq!(fs::read_to_string(&source).unwrap(), "outdated");
}
#[test]
fn stored_summaries_recompute_statistics_instead_of_trusting_derived_json() {
    let mut payload = sample(1).to_json();
    payload["epoch"] = json!("bogus");
    payload["metrics"][0]["median"] = json!(999999);
    let run: Run = serde_json::from_value(payload).unwrap();
    assert_eq!(run.epoch(), "10db7d1f");
    assert_eq!(run.metrics[0].median(), Some(100.0));
}
