#![cfg(unix)]
#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const EXPORT: &str = "[2026/09/11 15:50:20]\r\nAi Overclock Tuner [EXPO I]\r\nMemory Frequency [DDR5-6000MHz]\r\nPrecision Boost Overdrive [Enabled]\r\nCPU Boost Clock Override [Enabled (Positive)]\r\nMax CPU Boost Clock Override(+) [Auto]\r\nPlatform Thermal Throttle Limit [Manual]\r\nPlatform Thermal Throttle Limit [85]\r\nSVM Mode [Enabled]\r\nResize BAR Support [Enabled]\r\nCore Performance Boost [Enabled]\r\nCPU Fan Point1 Temperature [30]\r\nCPU Fan Point1 Duty Cycle (%) [15]\r\nCPU Fan Point2 Temperature [40]\r\nCPU Fan Point2 Duty Cycle (%) [15]\r\nCPU Fan Point3 Temperature [50]\r\nCPU Fan Point3 Duty Cycle (%) [15]\r\nCPU Fan Point4 Temperature [60]\r\nCPU Fan Point4 Duty Cycle (%) [20]\r\nCore Performance Boost [Auto]\r\n\r\n";

const SPEC: &str = "live {\n  base_boost_mhz = 5200\n  memory_mts     = 6000\n  gpu            = 0000:01:00.0\n}\n\nmemory {\n  Ai Overclock Tuner = EXPO I\n}\n\npbo {\n  Platform Thermal Throttle Limit#1  = Manual\n  Platform Thermal Throttle Limit#2  = 85\n  CPU Boost Clock Override           = Enabled (Positive)\n  Max CPU Boost Clock Override(+)    = Auto\n  Core Performance Boost#2           = Auto\n}\n\nplatform {\n  SVM Mode           = Enabled\n  Resize BAR Support = Enabled\n}\n\nq-fan-cpu {\n  CPU Fan Point1 Temperature    = 30\n  CPU Fan Point1 Duty Cycle (%) = 15\n  CPU Fan Point2 Temperature    = 40\n  CPU Fan Point2 Duty Cycle (%) = 15\n  CPU Fan Point3 Temperature    = 50\n  CPU Fan Point3 Duty Cycle (%) = 15\n  CPU Fan Point4 Temperature    = 60\n  CPU Fan Point4 Duty Cycle (%) = 20\n}";

fn utf16le(text: &str) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        bytes.extend(unit.to_le_bytes());
    }
    bytes
}

fn write(path: &Path, contents: impl AsRef<[u8]>) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let fixture = Self { root };
        let base = fixture.root.path();
        write(&base.join("config/targets.dotfile"), "");
        write(&base.join("config/bios/fixture.dotfile"), SPEC);
        write(&base.join("stick/Archie_BIOS_setting.txt"), utf16le(EXPORT));
        write(&base.join("lact.yaml"), "version: 7\n");
        fixture.sysfs();
        fixture.stubs();
        fixture
    }

    fn sys(&self) -> PathBuf {
        self.root.path().join("sys")
    }

    fn sysfs(&self) {
        let sys = self.sys();
        let chip = sys.join("class/hwmon/hwmon0");
        write(&chip.join("name"), "nct6799\n");
        for (channel, pwm, rpm) in [
            (1u8, 24u32, 596u32),
            (2, 37, 575),
            (3, 45, 426),
            (7, 75, 1496),
        ] {
            write(&chip.join(format!("pwm{channel}")), format!("{pwm}\n"));
            write(
                &chip.join(format!("fan{channel}_input")),
                format!("{rpm}\n"),
            );
        }
        write(&chip.join("temp7_input"), "23000\n");
        for (channel, duties) in [
            (2u8, [38u8, 38, 38, 51]),
            (1, [26, 26, 26, 64]),
            (3, [51, 51, 51, 64]),
            (7, [77, 77, 77, 89]),
        ] {
            for (index, (temp, pwm)) in [30u32, 40, 50, 60].iter().zip(duties.iter()).enumerate() {
                let point = index + 1;
                write(
                    &chip.join(format!("pwm{channel}_auto_point{point}_temp")),
                    format!("{}\n", temp * 1000),
                );
                write(
                    &chip.join(format!("pwm{channel}_auto_point{point}_pwm")),
                    format!("{pwm}\n"),
                );
            }
            write(
                &chip.join(format!("pwm{channel}_auto_point5_temp")),
                "125000\n",
            );
            write(&chip.join(format!("pwm{channel}_auto_point5_pwm")), "255\n");
        }
        let cpu_sensor = sys.join("class/hwmon/hwmon1");
        write(&cpu_sensor.join("name"), "k10temp\n");
        write(&cpu_sensor.join("temp1_input"), "35000\n");
        let cpus = sys.join("devices/system/cpu");
        write(&cpus.join("cpufreq/boost"), "1\n");
        for (id, siblings) in [(0, "0,2"), (1, "1,3"), (2, "0,2"), (3, "1,3")] {
            let cpu = cpus.join(format!("cpu{id}"));
            write(
                &cpu.join("topology/thread_siblings_list"),
                format!("{siblings}\n"),
            );
            write(&cpu.join("cpufreq/cpuinfo_max_freq"), "5455945\n");
            write(&cpu.join("cpufreq/scaling_governor"), "powersave\n");
            write(
                &cpu.join("cpufreq/energy_performance_preference"),
                "balance_performance\n",
            );
            write(&cpu.join("cpufreq/amd_pstate_prefcore_ranking"), "196\n");
        }
        write(&sys.join("class/dmi/id/bios_version"), "1681\n");
        write(
            &sys.join("bus/pci/devices/0000:01:00.0/resource"),
            "0x00000000fb000000 0x00000000fbffffff 0x0000000000040200\n0x0000006000000000 0x00000063ffffffff 0x000000000014220c\n",
        );
        let mut dmi = vec![0u8; 0x60];
        dmi[0x0D] = 0x40;
        dmi[0x15..0x17].copy_from_slice(&6000u16.to_le_bytes());
        dmi[0x20..0x22].copy_from_slice(&6000u16.to_le_bytes());
        write(&sys.join("firmware/dmi/entries/17-0/raw"), &dmi);
        let mut spd = vec![0u8; 1024];
        spd[512] = 0x02;
        spd[513] = 0x9E;
        spd[521..539].copy_from_slice(b"CMK32GX5M2B6000Z30");
        spd[552] = 0x80;
        spd[553] = 0xAD;
        write(&sys.join("bus/i2c/devices/4-0051/name"), "spd5118\n");
        write(&sys.join("bus/i2c/devices/4-0051/eeprom"), &spd);
        write(&self.root.path().join("dev/kvm"), "");
    }

    fn stub(&self, name: &str, body: &str) {
        let bin = self.root.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        testkit::executable(&bin.join(name), &format!("#!/bin/sh\n{body}\n"));
    }

    fn stubs(&self) {
        self.stub("stress-ng", "exit 0");
        self.stub("journalctl", "echo 'kernel initialized'");
        self.stub(
            "systemctl",
            "case \"$1\" in is-active) echo active;; is-enabled) echo enabled;; esac",
        );
        self.stub("nvidia-smi", "echo '24, 11.96, 300.00, 345, 405, 0'");
        self.stub("sudo", "exit 1");
    }

    fn command(&self) -> Command {
        let base = self.root.path();
        let mut command = Command::new(env!("CARGO_BIN_EXE_hwtune"));
        command
            .env("DOTFILE_ROOT", base)
            .env("HWTUNE_HOST", "fixture")
            .env("HWTUNE_SYSFS_ROOT", self.sys())
            .env("HWTUNE_DEV_ROOT", base.join("dev"))
            .env("HWTUNE_LACT_CONFIG", base.join("lact.yaml"))
            .env("HWTUNE_BENCHMARKS", base.join("benchmarks"))
            .env("HWTUNE_MEASUREMENT_LOCK", base.join("measurement.lock"))
            .env("HWTUNE_BOOT_ID", "boot-a")
            .env("XDG_STATE_HOME", base.join("state"))
            .env("XDG_CACHE_HOME", base.join("cache"))
            .env("HOME", base)
            .env("CAPTURE", base.join("captured.txt"))
            .env("PATH", base.join("bin"))
            .env("NO_COLOR", "1");
        command
    }

    fn output(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    fn run(&self, args: &[&str]) -> String {
        let output = self.output(args);
        assert!(output.status.success(), "{:?}\n{}", args, text(&output));
        text(&output)
    }

    fn export(&self) -> PathBuf {
        self.root
            .path()
            .join("config/bios/exports/fixture-1681-20260911.txt")
    }

    fn import(&self) -> String {
        self.run(&[
            "bios",
            "import",
            &self
                .root
                .path()
                .join("stick/Archie_BIOS_setting.txt")
                .display()
                .to_string(),
        ])
    }
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn help_completions_and_command_dump_work() {
    let fixture = Fixture::new();
    assert!(
        fixture
            .run(&["--help"])
            .starts_with("Hardware tuning, benchmarks, and stability tests")
    );
    assert!(
        fixture
            .run(&["--completions", "zsh"])
            .contains("#compdef hwtune")
    );
    let dump = fixture.run(&["--command-dump"]);
    assert!(serde_json::from_str::<serde_json::Value>(&dump).is_ok());
    assert!(fixture.run(&[]).contains("Usage:"));
}

#[test]
fn import_normalizes_and_reports_changes() {
    let fixture = Fixture::new();
    let first = fixture.import();
    assert!(first.contains("wrote"), "{first}");
    let stored = fs::read_to_string(fixture.export()).unwrap();
    assert!(stored.starts_with("[2026/09/11 15:50:20]\nAi Overclock Tuner [EXPO I]\n"));
    assert!(!stored.contains('\r'));
    assert!(stored.ends_with("Core Performance Boost [Auto]\n"));
    assert!(fixture.import().contains("unchanged"));
    let list = fixture.run(&["bios", "list"]);
    assert!(list.contains("fixture-1681-20260911.txt"));
    assert!(list.contains("20"));

    let changed = EXPORT
        .replace("[2026/09/11 15:50:20]", "[2026/09/12 10:00:00]")
        .replace("Magnitude [25]", "Magnitude [30]")
        .replace("SVM Mode [Enabled]", "SVM Mode [Disabled]");
    write(
        &fixture.root.path().join("stick/second.txt"),
        utf16le(&changed),
    );
    let second = fixture.run(&[
        "bios",
        "import",
        &fixture
            .root
            .path()
            .join("stick/second.txt")
            .display()
            .to_string(),
    ]);
    assert!(second.contains("1 changed"), "{second}");
    assert!(second.contains("SVM Mode"));
    let diff = fixture.run(&["bios", "diff"]);
    assert!(diff.contains("-SVM Mode [Enabled]"));
    assert!(diff.contains("+SVM Mode [Disabled]"));
}

#[test]
fn check_compares_spec_export_and_live_system() {
    let fixture = Fixture::new();
    fixture.import();
    let report = fixture.run(&["bios", "check"]);
    for label in [
        "Platform Thermal Throttle Limit#2",
        "Core Performance Boost#2",
        "boost",
        "svm",
        "memory",
        "rebar",
        "q-fan cpu fan",
        "dimms",
    ] {
        assert!(report.contains(label), "missing {label}:\n{report}");
    }
    assert!(report.contains("ceiling 5455 MHz"));
    assert!(report.contains("6000 MT/s"));
    assert!(report.contains("CMK32GX5M2B6000Z30"));
    assert!(!report.contains("bad "), "{report}");
    let quick = fixture.run(&["bios", "check", "--no-live"]);
    assert!(!quick.contains("boost"));
}

#[test]
fn check_fails_on_a_spec_mismatch() {
    let fixture = Fixture::new();
    fixture.import();
    write(
        &fixture.root.path().join("config/bios/fixture.dotfile"),
        "memory {\n  Ai Overclock Tuner = XMP\n}",
    );
    let output = fixture.output(&["bios", "check", "--no-live"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output).contains("bad   Ai Overclock Tuner  spec says XMP"));
}

#[test]
fn status_shows_every_section() {
    let fixture = Fixture::new();
    fixture.import();
    let status = fixture.run(&["status"]);
    for needle in [
        "fan2go active/enabled",
        "ceiling 5455 MHz",
        "prefcore 0:196",
        "radiator",
        "tctl 35.0°C",
        "300 W",
        "0 mce, 0 hw, 0 xid",
        "CMK32GX5M2B6000Z30",
        "fixture-1681-20260911.txt",
    ] {
        assert!(status.contains(needle), "missing {needle}:\n{status}");
    }
}

#[test]
fn per_core_stress_logs_each_core() {
    let fixture = Fixture::new();
    fixture.import();
    let report = fixture.run(&[
        "stress",
        "cpu",
        "--profile",
        "per-core",
        "--minutes",
        "0",
        "--cores",
        "0-1",
        "--offset",
        "-25",
    ]);
    assert!(report.contains("pass"), "{report}");
    let log = fs::read_to_string(
        fixture
            .root
            .path()
            .join("config/bios/fixture-stability.dotfile"),
    )
    .unwrap();
    assert!(log.contains("profile"), "{log}");
    assert!(log.contains("= per-core"));
    assert!(log.contains("= -25"));
    assert!(log.contains("core0"));
    assert!(log.contains("core1"));
    assert!(log.contains("= pass"));
    assert!(log.contains("peak_tctl"));
    let csv = fs::read_dir(fixture.root.path().join("cache/hwtune"))
        .unwrap()
        .flatten()
        .find(|entry| entry.path().extension().is_some_and(|ext| ext == "csv"))
        .unwrap();
    assert!(
        fs::read_to_string(csv.path())
            .unwrap()
            .starts_with("t_iso,elapsed_s")
    );
    assert!(
        !fixture
            .root
            .path()
            .join("state/hwtune/percore.json")
            .exists()
    );
}

#[test]
fn a_rebooted_core_is_recorded_as_failed() {
    let fixture = Fixture::new();
    write(
        &fixture.root.path().join("state/hwtune/percore.json"),
        r#"{"session":"old","core":0,"started":"2026-09-13T20:44:10","offset":-30,"boot_id":"boot-b","pid":1}"#,
    );
    let output = fixture.output(&[
        "stress",
        "cpu",
        "--profile",
        "per-core",
        "--minutes",
        "0",
        "--cores",
        "0-1",
    ]);
    assert_eq!(output.status.code(), Some(1));
    let report = text(&output);
    assert!(report.contains("rebooted"), "{report}");
    let log = fs::read_to_string(
        fixture
            .root
            .path()
            .join("config/bios/fixture-stability.dotfile"),
    )
    .unwrap();
    assert!(log.contains("fail (rebooted)"));
    assert!(log.contains("= fail\n"));
}

#[test]
fn a_failing_stressor_fails_the_run() {
    let fixture = Fixture::new();
    fixture.stub("stress-ng", "exit 2");
    let output = fixture.output(&[
        "stress",
        "cpu",
        "--profile",
        "all-core",
        "--minutes",
        "0",
        "--no-log",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output).contains("exit 2"));
    assert!(
        !fixture
            .root
            .path()
            .join("config/bios/fixture-stability.dotfile")
            .exists()
    );
}

#[test]
fn sample_prints_peaks() {
    let fixture = Fixture::new();
    let report = fixture.run(&["sample", "--minutes", "0"]);
    assert!(report.contains("tctl"));
    assert!(report.contains("35.0"));
}

#[test]
fn benchmark_commands_are_owned_by_hwtune() {
    let fixture = Fixture::new();
    let help = fixture.run(&["bench", "--help"]);
    for command in [
        "run", "plan", "show", "list", "health", "compare", "trend", "baseline", "prune", "report",
    ] {
        assert!(help.contains(command), "{help}");
    }
    assert!(!fixture.output(&["report"]).status.success());
    assert!(
        !fixture
            .output(&["bench", "--note", "why", "--", "--tier", "quick"])
            .status
            .success()
    );
}

#[test]
fn missing_spec_or_export_is_reported() {
    let fixture = Fixture::new();
    let output = fixture.output(&["bios", "check"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output).contains("no exports imported"));
    let output = fixture.output(&["--host", "nowhere", "bios", "list"]);
    assert!(text(&output).contains("no exports imported for nowhere"));
}

#[test]
fn bios_root_discovery_works_outside_checkout_and_honors_explicit_override() {
    let fixture = Fixture::new();
    fixture.import();
    let elsewhere = tempfile::tempdir().unwrap();
    let explicit = fixture
        .command()
        .current_dir(elsewhere.path())
        .args(["bios", "list"])
        .output()
        .unwrap();
    assert!(explicit.status.success(), "{}", text(&explicit));
    assert!(text(&explicit).contains("fixture-1681-20260911.txt"));
    let discovered = fixture
        .command()
        .env_remove("DOTFILE_ROOT")
        .current_dir(elsewhere.path())
        .args(["--host", "outside-cwd-fixture", "bios", "list"])
        .output()
        .unwrap();
    assert!(discovered.status.success(), "{}", text(&discovered));
    assert!(!text(&discovered).contains("repository root not found"));
}

#[test]
fn missing_journal_evidence_records_unknown_stability_without_a_pass() {
    let fixture = Fixture::new();
    fixture.stub("journalctl", "exit 1");
    let output = fixture.output(&["stress", "cpu", "--profile", "all-core", "--minutes", "0"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output).contains("unknown"), "{}", text(&output));
    let directory = fixture
        .root
        .path()
        .join("benchmarks/hosts/fixture/stability");
    let path = fs::read_dir(directory)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let receipt: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(receipt["result"], "unknown");
    assert_eq!(receipt["evidence_known"], false);
}

#[test]
fn stress_refuses_to_start_while_measurement_lock_is_held() {
    let fixture = Fixture::new();
    for tool in ["stress-ng", "vkmark", "glmark2"] {
        fixture.stub(tool, "printf started > \"$CAPTURE\"; exit 99");
    }
    let lock = fs::File::create(fixture.root.path().join("measurement.lock")).unwrap();
    fs2::FileExt::lock_exclusive(&lock).unwrap();
    for args in [
        &["stress", "cpu", "--profile", "all-core", "--minutes", "0"][..],
        &["stress", "mem", "--minutes", "0"],
        &["stress", "gpu", "--minutes", "0"],
    ] {
        let output = fixture.output(args);
        assert_eq!(output.status.code(), Some(1), "{}", text(&output));
        assert!(
            text(&output).contains("already running"),
            "{}",
            text(&output)
        );
    }
    assert!(!fixture.root.path().join("captured.txt").exists());
}

#[test]
fn scoped_run_refuses_without_root_before_touching_controls() {
    let fixture = Fixture::new();
    fixture.stub("true", "printf started > \"$CAPTURE\"; exit 0");
    let output = fixture.output(&["run", "--profile", "performance", "--", "true"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("run under sudo"),
        "{}",
        text(&output)
    );
    assert!(!fixture.root.path().join("captured.txt").exists());
    assert_eq!(
        fs::read_to_string(
            fixture
                .sys()
                .join("devices/system/cpu/cpu0/cpufreq/scaling_governor")
        )
        .unwrap(),
        "powersave\n"
    );
}

#[test]
fn gpu_sweep_fails_cleanly_without_lact_and_restores_nothing() {
    let fixture = Fixture::new();
    let output = fixture.output(&["gpu", "sweep", "--caps", "300"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(text(&output).contains("lact"), "{}", text(&output));
    assert!(!fixture.root.path().join("benchmarks").exists());
    assert!(
        !fixture
            .output(&["gpu", "sweep", "--caps", "300", "--only", "nope"])
            .status
            .success()
    );
}

#[test]
fn curve_status_reads_offsets_from_exports_and_suggests_the_next_step() {
    let fixture = Fixture::new();
    let stick = fixture.root.path().join("stick/curve.txt");
    write(
        &stick,
        utf16le(
            "[2026/09/12 10:00:00]\r\nCurve Optimizer [All Cores]\r\nAll Core Curve Optimizer Sign [Negative]\r\nAll Core Curve Optimizer Magnitude [25]\r\nCurve Optimizer [Disable]\r\n",
        ),
    );
    fixture.run(&["bios", "import", &stick.display().to_string()]);
    let stored = fs::read_to_string(
        fixture
            .root
            .path()
            .join("config/bios/exports/fixture-1681-20260912.txt"),
    )
    .unwrap();
    let sha = hwtune::bios::export::sha8(&stored);
    let before = fixture.run(&["curve", "status"]);
    assert!(before.contains("curve optimizer  All Cores"), "{before}");
    assert!(before.contains("stress -25 first"), "{before}");
    assert!(
        before
            .contains("hwtune stress cpu --profile per-core --cores 0,1 --offset -25 --minutes 10"),
        "{before}"
    );
    let session = serde_json::json!({
        "schema": 1,
        "host": "fixture",
        "session": "20260912-per-core-1-0",
        "completed": "2026-09-12T11:00:00",
        "profile": "per-core",
        "result": "pass",
        "context": {"observed": {}},
        "context_unchanged": true,
        "evidence_known": true,
        "details": {
            "profile": "per-core",
            "minutes": "10",
            "bios": sha,
            "result": "pass",
            "core0": "pass",
            "core1": "fail (exit 1)"
        },
        "samples_path": ""
    });
    write(
        &fixture
            .root
            .path()
            .join("benchmarks/hosts/fixture/stability/20260912-per-core-1-0.json"),
        serde_json::to_vec(&session).unwrap(),
    );
    let after = fixture.run(&["curve", "status"]);
    assert!(after.contains("try -30"), "{after}");
    assert!(after.contains("back off to -20"), "{after}");
    assert!(after.contains("bios    core 0 -30  core 1 -20"), "{after}");
    let json: serde_json::Value =
        serde_json::from_str(&fixture.run(&["curve", "status", "--json"])).unwrap();
    assert_eq!(json["cores"][0]["next"]["action"], "try");
    assert_eq!(json["cores"][1]["shallowest_failed"], -25);
    assert_eq!(json["ryzen_smu"], serde_json::Value::Null);
}

#[test]
fn curve_bench_fails_without_the_native_worker_and_writes_nothing() {
    let fixture = Fixture::new();
    fixture.stub("taskset", "exit 0");
    let output = fixture.output(&["curve", "bench", "--iterations", "1"]);
    assert!(!output.status.success(), "{}", text(&output));
    assert!(
        !fixture
            .root
            .path()
            .join("benchmarks/hosts/fixture/curve")
            .exists()
    );
    let output = fixture.output(&["curve", "bench", "--iterations", "0"]);
    assert!(text(&output).contains("--iterations must be at least 1"));
}
