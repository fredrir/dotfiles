use serde_json::{Value, json};
use std::fs;
use testkit::Bin;
use workstation_sysinfo::bench::{record::Run, store::Store};
fn archived() -> Run {
    serde_json::from_str(include_str!("fixtures/bench/archie-schema1.json")).unwrap()
}
fn binary(store: &Store) -> Bin {
    Bin::new(env!("CARGO_BIN_EXE_sysinfo"))
        .arg("bench")
        .env("SYSINFO_BENCHMARKS", &store.root)
        .plain()
}
#[test]
fn historical_show_and_compare_emit_parseable_schema_one_json() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().join("history"));
    let run = archived();
    store.save_run(&run).unwrap();
    let result = binary(&store).args(["show", "archie", "--json"]).run();
    assert!(result.success(), "{result:?}");
    let value: Value = serde_json::from_str(&result.stdout).unwrap();
    assert_eq!(value["epoch"], "10db7d1f");
    assert_eq!(value["schema"], 1);
    let result = binary(&store)
        .args(["compare", "archie", "archie", "--json"])
        .run();
    assert!(result.success(), "{result:?}");
    let value: Value = serde_json::from_str(&result.stdout).unwrap();
    assert!(
        value["deltas"]
            .as_array()
            .unwrap()
            .iter()
            .all(|delta| delta["verdict"] == "noise" || delta["verdict"] == "blocked")
    );
}

#[test]
fn completions_preserve_hardware_selectors_and_escape_exact_run_colons() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path().join("history"));
    let run = archived();
    store.save_run(&run).unwrap();
    let mut noisy = run.clone();
    noisy.run_id = "2026-08-19T03-57-50Z-10db7d1f".into();
    noisy.grade = "noisy".into();
    noisy.metrics[0].key = "noisy.only".into();
    store.save_run(&noisy).unwrap();
    let complete = |source| {
        Bin::new(env!("CARGO_BIN_EXE_sysinfo"))
            .args(["__complete", source])
            .env("SYSINFO_BENCHMARKS", &store.root)
            .plain()
            .run()
    };
    let result = complete("runs");
    assert!(result.success(), "{result:?}");
    assert!(result.stdout.contains("archie:2 stored runs"));
    assert!(
        result
            .stdout
            .contains("archie@10db7d1f:2 runs on this hardware")
    );
    assert!(result.stdout.contains(&format!("archie\\:{}:", run.run_id)));
    let metrics = complete("metrics");
    assert!(!metrics.stdout.contains("noisy.only"));
    assert!(metrics.stdout.contains(&run.metrics[0].key));
}
#[test]
fn missing_selectors_fail_explicitly_without_a_terminal() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().into());
    for args in [
        vec!["show"],
        vec!["compare"],
        vec!["trend", "archie"],
        vec!["baseline", "set"],
    ] {
        let result = binary(&store).args(args).run();
        assert!(!result.success());
        assert!(result.stderr.contains("needs a terminal"), "{result:?}");
    }
}
#[test]
fn plan_does_not_spawn_benchmark_tools_or_create_directories() {
    let root = tempfile::tempdir().unwrap();
    let tools = root.path().join("bin");
    fs::create_dir(&tools).unwrap();
    for tool in [
        "fio",
        "7z",
        "sysbench",
        "hyperfine",
        "stress-ng",
        "swiftc",
        "openssl",
    ] {
        testkit::executable(
            &tools.join(tool),
            "#!/bin/sh\necho unexpectedly-ran >&2\nexit 99\n",
        );
    }
    let store = Store::new(root.path().join("absent-history"));
    let work = root.path().join("absent-work");
    let result = binary(&store)
        .args(["plan", "--tier", "heavy", "--json", "--workdir"])
        .arg(&work)
        .env("PATH", &tools)
        .run();
    assert!(result.success(), "{result:?}");
    assert!(result.stderr.is_empty());
    let plan: Value = serde_json::from_str(&result.stdout).unwrap();
    assert_eq!(plan["expected_bytes_written"], 58 * 1024_u64.pow(3));
    assert!(!store.root.exists());
    assert!(!work.exists());
}
#[test]
fn pruning_requires_yes_and_retains_oldest_newest_and_baseline() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().join("history"));
    for day in 1..=6 {
        let mut run = archived();
        run.run_id = format!("2026-08-{day:02}T09-00-00Z-10db7d1f");
        run.started = format!("2026-08-{day:02}T09:00:00Z");
        store.save_run(&run).unwrap();
        if day == 3 {
            store
                .set_baseline(&run.host, &run.epoch(), &run.run_id)
                .unwrap();
        }
    }
    let result = binary(&store).args(["prune", "--keep", "2"]).run();
    assert!(!result.success());
    assert_eq!(store.list_runs(None, &[]).unwrap().len(), 6);
    assert!(
        binary(&store)
            .args(["prune", "--keep", "2", "--dry-run"])
            .run()
            .success()
    );
    assert_eq!(store.list_runs(None, &[]).unwrap().len(), 6);
    assert!(
        binary(&store)
            .args(["prune", "--keep", "2", "--yes"])
            .run()
            .success()
    );
    assert_eq!(store.list_runs(None, &[]).unwrap().len(), 4);
}
#[test]
fn baseline_mutations_respect_the_benchmark_lock() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().join("history"));
    store.save_run(&archived()).unwrap();
    let held = store.exclusive().unwrap();
    let result = binary(&store).args(["baseline", "set", "archie"]).run();
    assert!(!result.success());
    assert!(result.stderr.contains("already running"));
    drop(held);
    assert!(
        binary(&store)
            .args(["baseline", "set", "archie"])
            .run()
            .success()
    );
}
#[test]
fn benchmark_run_uses_only_native_tools_and_writes_history_under_lock() {
    let root = tempfile::tempdir().unwrap();
    let tools = root.path().join("bin");
    fs::create_dir(&tools).unwrap();
    let config = root.path().join("hosts.dotfile");
    fs::write(&config, "archie {\n  role = hyprland\n}\n").unwrap();
    let collector = tools.join("collector");
    let modules = json!([{"type":"CPU","result":{"cpu":"Fixture CPU","cores":{"physical":2,"logical":4}}},{"type":"Memory","result":{"total":34359738368_u64,"used":1073741824_u64}},{"type":"OS","result":{"id":"fixture"}}]);
    testkit::executable(
        &collector,
        &format!("#!/bin/sh\nprintf '%s' '{}'\n", modules),
    );
    let workload = tools.join("bench-workloads");
    testkit::executable(
        &workload,
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'bench-workloads 0.1.0'; else echo '{\"value\":123.4}'; fi\n",
    );
    let store = Store::new(root.path().join("history"));
    let result = binary(&store)
        .args([
            "run",
            "--host",
            "archie",
            "--only",
            "mem",
            "--force",
            "--json",
            "--baseline",
            "--workdir",
        ])
        .arg(root.path().join("work"))
        .env("PATH", &tools)
        .env("SYSINFO_CONFIG", &config)
        .env("SYSINFO_COLLECTOR", &collector)
        .env("SYSINFO_BENCH_WORKLOADS", &workload)
        .env("DOTFILE_ROOT", root.path())
        .run();
    assert!(result.success(), "{result:?}");
    let run: Run = serde_json::from_str(&result.stdout).unwrap();
    assert_eq!(run.metrics.len(), 2);
    assert!(
        run.metrics
            .iter()
            .all(|metric| metric.tool == "bench-workloads" && metric.samples.len() == 3)
    );
    assert!(
        store
            .baseline_run("archie", &run.epoch())
            .unwrap()
            .is_some()
    );
}

fn fixture_measurement(root: &std::path::Path, store: &Store, family: &str) -> Bin {
    let tools = root.join("bin");
    fs::create_dir_all(&tools).unwrap();
    let config = root.join("hosts.dotfile");
    fs::write(&config, "archie {\n  role = hyprland\n}\n").unwrap();
    let collector = tools.join("collector");
    testkit::executable(
        &collector,
        "#!/bin/sh\necho '[{\"type\":\"CPU\",\"result\":{\"cpu\":\"Fixture CPU\"}},{\"type\":\"Memory\",\"result\":{\"total\":34359738368,\"used\":1073741824}}]'\n",
    );
    binary(store)
        .args([
            "run",
            "--host",
            "archie",
            "--only",
            family,
            "--force",
            "--json",
            "--tier",
            "standard",
            "--workdir",
        ])
        .arg(root.join("work"))
        .env("PATH", &tools)
        .env("SYSINFO_CONFIG", &config)
        .env("SYSINFO_COLLECTOR", &collector)
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("DOTFILE_ROOT", root)
}

#[test]
fn fio_failure_after_layout_is_not_retried_and_still_consumes_the_write_budget() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().join("history"));
    let command = fixture_measurement(root.path(), &store, "mem,disk");
    let attempts = root.path().join("attempts");
    let workload = root.path().join("bin/bench-workloads");
    testkit::executable(
        &workload,
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'bench-workloads 0.1.0'; else echo '{\"value\":123.4}'; fi\n",
    );
    testkit::executable(
        &root.path().join("bin/fio"),
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo fio-3.33; exit 0; fi\necho attempt >> \"$ATTEMPTS\"\nwhile IFS= read -r line; do case \"$line\" in directory=*) directory=${line#directory=};; esac; done < \"$2\"\n: > \"$directory/seq-read.0.0\"\necho engine-failed-after-layout >&2\nexit 1\n",
    );
    let result = command
        .env("SYSINFO_BENCH_WORKLOADS", workload)
        .env("ATTEMPTS", &attempts)
        .run();
    assert!(result.success(), "{result:?}");
    let run: Run = serde_json::from_str(&result.stdout).unwrap();
    assert_eq!(fs::read_to_string(attempts).unwrap(), "attempt\n");
    assert_eq!(run.bytes_written, 12 * 1024_u64.pow(3));
    assert_eq!(run.grade, "noisy");
    assert!(
        run.gate_reasons
            .iter()
            .any(|reason| reason.contains("not retrying after fio started disk writes"))
    );
    assert_eq!(fs::read_dir(root.path().join("work")).unwrap().count(), 0);
}

#[cfg(target_os = "macos")]
#[test]
fn metal_compiler_failure_keeps_available_graphics_measurements() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().join("history"));
    let command = fixture_measurement(root.path(), &store, "gpu");
    testkit::executable(
        &root.path().join("bin/glmark2"),
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo glmark2-2023.01; else echo 'Score: 123'; fi\n",
    );
    testkit::executable(&root.path().join("bin/swiftc"), "#!/bin/sh\nexit 1\n");
    let result = command
        .env("DISPLAY", ":99")
        .env("WAYLAND_DISPLAY", "")
        .run();
    assert!(result.success(), "{result:?}");
    let run: Run = serde_json::from_str(&result.stdout).unwrap();
    assert_eq!(run.metrics.len(), 1);
    assert_eq!(run.metrics[0].key, "gpu.graphics");
    assert!(result.stderr.contains("gpu.compute skipped"));
}

#[test]
fn plan_reports_only_graphics_tools_for_the_current_display_backend() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path().join("history"));
    let tools = root.path().join("bin");
    fs::create_dir(&tools).unwrap();
    testkit::executable(&tools.join("glmark2"), "#!/bin/sh\nexit 99\n");
    let result = binary(&store)
        .args(["plan", "--only", "gpu", "--json"])
        .env("PATH", &tools)
        .env("WAYLAND_DISPLAY", "wayland-0")
        .env("DISPLAY", "")
        .run();
    assert!(result.success(), "{result:?}");
    let plan: Value = serde_json::from_str(&result.stdout).unwrap();
    let graphics = plan["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|job| job["name"] == "gpu.graphics")
        .unwrap();
    assert_eq!(graphics["available"], false);
    assert_eq!(
        graphics["tools"],
        json!(["glmark2-wayland", "glmark2-es2-wayland"])
    );
    assert!(!store.root.exists());
}
