#![cfg(unix)]

use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

struct Fixture {
    root: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("config")).unwrap();
        std::fs::write(
            root.path().join("config/hosts.dotfile"),
            "fixture {\n role = server\n}\n",
        )
        .unwrap();
        let fixture = Self { root };
        fixture.script(
            "collector",
            "#!/bin/sh\nprintf '%s\\n' \"$SYSINFO_TEST_NATIVE\"\n",
        );
        fixture
    }
    fn script(&self, name: &str, body: &str) {
        let path = self.root.path().join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sysinfo"));
        command.env("DOTFILE_ROOT",self.root.path()).env("SYSINFO_CONFIG",self.root.path().join("config/hosts.dotfile")).env("SYSINFO_HOST","fixture").env("SYSINFO_BENCHMARKS",self.root.path().join("benchmarks")).env("SYSINFO_COLLECTOR",self.root.path().join("collector")).env("SYSINFO_TEST_NATIVE",json!([{"type":"CPU","result":{"cpu":"AMD Test CPU","vendor":"AMD","cores":{"physical":4,"logical":8}}},{"type":"OS","result":{"id":"test","prettyName":"Test OS"}},{"type":"PhysicalDisk","result":[{"name":"Real Disk","size":1000000000u64,"serial":"PRIVATE-SERIAL"}]}]).to_string()).env("PATH",self.root.path()).env("SHELL","/bin/sh").env("NO_COLOR","1");
        command
    }
    fn output(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }
    fn fastfetch(&self) {
        self.script("fastfetch","#!/bin/sh\n/bin/cp \"$2\" \"$SYSINFO_TEST_CONFIG_CAPTURE\"\nprintf '%s\\n' \"$SYSINFO_TEST_ENRICH\"\n");
    }
}
fn stdout(output: &Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

#[test]
fn default_summary_uses_native_fixture_without_fastfetch_or_python() {
    let fixture = Fixture::new();
    let text = stdout(&fixture.output(&[]));
    assert!(text.contains("System: Test OS"));
    assert!(text.contains("CPU: AMD Test CPU"));
    assert!(!text.contains("PRIVATE"));
    assert!(!text.contains("Traceback"));
}
#[test]
fn empty_collector_override_uses_in_process_collection() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .arg("--json")
        .env("SYSINFO_COLLECTOR", "")
        .output()
        .unwrap();
    let payload: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(payload["schema"], 1);
    assert!(
        payload["hardware"]["memory"]["total"]
            .as_u64()
            .is_some_and(|total| total > 0)
    );
}
#[test]
fn normalized_json_and_timing_diagnostics_use_separate_streams() {
    let fixture = Fixture::new();
    let output = fixture.output(&["--json", "--timings"]);
    let text = stdout(&output);
    let payload: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(payload["schema"], 1);
    assert_eq!(payload["hardware"]["cpu"]["cores_physical"], 4);
    assert_eq!(payload["installation"]["os"], "test");
    assert!(!text.contains("PRIVATE"));
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    assert!(diagnostics.contains("collection:"));
    // Explicit host identity skips hostname probes: only collector and terminal ps.
    assert!(
        diagnostics.contains("subprocess probes: 2\n"),
        "{diagnostics}"
    );
    assert!(diagnostics.contains("total:"));
}
#[test]
fn full_collection_requests_missing_modules_and_survives_optional_enrichment_absence() {
    let fixture = Fixture::new();
    let first = fixture.output(&["--full", "--json"]);
    assert_eq!(
        serde_json::from_str::<Value>(&stdout(&first)).unwrap()["hardware"]["cpu"]["model"],
        "AMD Test CPU"
    );
    fixture.fastfetch();
    let capture = fixture.root.path().join("requested.json");
    let output = fixture
        .command()
        .args(["--full", "--json", "--timings"])
        .env("SYSINFO_TEST_CONFIG_CAPTURE", &capture)
        .env(
            "SYSINFO_TEST_ENRICH",
            json!([{"type":"Host","result":{"name":"Enriched Host"}}]).to_string(),
        )
        .output()
        .unwrap();
    let payload: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert!(String::from_utf8_lossy(&output.stderr).contains("fastfetch:"));
    assert!(
        payload["system"]["system_facts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["label"] == "Host" && f["value"] == "Enriched Host")
    );
    let requested: Value = serde_json::from_slice(&std::fs::read(capture).unwrap()).unwrap();
    let kinds = requested["modules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap_or_else(|| m["type"].as_str().unwrap()))
        .collect::<Vec<_>>();
    assert!(!kinds.contains(&"CPU"));
    assert!(!kinds.contains(&"OS"));
    assert!(kinds.contains(&"Host"));
    assert!(kinds.contains(&"Display"));
}
#[test]
fn failed_missing_and_malformed_collector_fall_back_to_fastfetch() {
    for script in ["#!/bin/sh\nexit 1\n", "#!/bin/sh\nprintf 'bad json'\n"] {
        let fixture = Fixture::new();
        fixture.script("collector", script);
        fixture.fastfetch();
        let output = fixture
            .command()
            .env(
                "SYSINFO_TEST_CONFIG_CAPTURE",
                fixture.root.path().join("requested.json"),
            )
            .env(
                "SYSINFO_TEST_ENRICH",
                json!([{"type":"OS","result":{"prettyName":"Fallback OS"}}]).to_string(),
            )
            .output()
            .unwrap();
        assert!(stdout(&output).contains("System: Fallback OS"));
    }
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .env("SYSINFO_COLLECTOR", fixture.root.path().join("missing"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("could not collect"));
}
#[test]
fn help_version_surface_and_completion_do_not_probe_the_machine() {
    let fixture = Fixture::new();
    fixture.script("collector", "#!/bin/sh\nexit 99\n");
    for args in [
        &["--help"][..],
        &["--version"],
        &["--completions", "zsh"],
        &["bench", "--help"],
    ] {
        stdout(&fixture.output(args));
    }
    let payload: Value =
        serde_json::from_str(&stdout(&fixture.output(&["--command-dump"]))).unwrap();
    assert_eq!(payload["version"], 1);
    assert_eq!(payload["command"]["path"], json!(["sysinfo"]));
    assert!(
        payload["command"]["children"]
            .as_array()
            .unwrap()
            .iter()
            .any(|child| child["path"] == json!(["sysinfo", "bench"]))
    );
    let help = stdout(&fixture.output(&["--help"]));
    for flag in ["--pretty", "-p", "--full", "-f", "--health", "-hh"] {
        assert!(help.contains(flag));
    }
}
#[test]
fn completion_failures_are_quiet_and_config_host_completion_is_available() {
    let fixture = Fixture::new();
    assert_eq!(
        stdout(&fixture.output(&["__complete", "known-hosts"])),
        "fixture:server\n"
    );
    std::fs::write(
        fixture.root.path().join("config/hosts.dotfile"),
        "malformed {\n",
    )
    .unwrap();
    let result = fixture.output(&["__complete", "known-hosts"]);
    assert_eq!(stdout(&result), "");
    assert!(result.stderr.is_empty());
}
#[test]
fn bounded_probe_kills_timed_out_child_and_reports_failure() {
    let started = Instant::now();
    let result = workstation_sysinfo::collect::probe(
        Command::new("/bin/sh").args(["-c", "exec /bin/sleep 30"]),
        Duration::from_millis(40),
    );
    assert!(result.is_err());
    assert!(started.elapsed() < Duration::from_secs(3));
}
