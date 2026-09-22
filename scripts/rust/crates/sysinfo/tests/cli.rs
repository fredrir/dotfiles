#![cfg(unix)]

use serde_json::{Value, json};
use std::process::{Command, Output};

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
        Self { root }
    }
    fn script(&self, name: &str, body: &str) {
        testkit::executable(&self.root.path().join(name), body);
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_sysinfo"));
        command
            .env("DOTFILE_ROOT", self.root.path())
            .env("SYSINFO_HOST", "fixture")
            .env(
                "SYSINFO_CONFIG",
                self.root.path().join("config/hosts.dotfile"),
            )
            .env("PATH", self.root.path())
            .env("SHELL", "/bin/sh")
            .env("NO_COLOR", "1");
        command
    }
    fn output(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
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
fn report(output: &Output) -> Value {
    let value: Value = serde_json::from_str(&stdout(output)).unwrap();
    assert_eq!(value["schema"], 1);
    assert!(
        value["hardware"]["memory"]["total"]
            .as_u64()
            .is_some_and(|total| total > 0)
    );
    assert!(value["system"]["components"].is_array());
    value
}
fn cpu_load_fact(payload: &Value) -> Option<String> {
    let components = payload["system"]["components"].as_array()?;
    let facts = components
        .iter()
        .find(|component| component["kind"] == "cpu")?["facts"]
        .as_array()?;
    facts
        .iter()
        .find(|fact| fact["label"] == "Load")
        .map(|fact| fact["value"].as_str().unwrap_or_default().to_string())
}

#[test]
fn dashboard_renders_gauges_without_subprocess_probes() {
    let fixture = Fixture::new();
    let marker = fixture.root.path().join("probe-called");
    for name in ["ps", "fastfetch", "shell"] {
        fixture.script(name, "#!/bin/sh\nprintf called >> \"$PROBE_MARKER\"\n");
    }
    let output = fixture
        .command()
        .args(["--pretty", "--timings"])
        .env("PROBE_MARKER", &marker)
        .env("SHELL", fixture.root.path().join("shell"))
        .output()
        .unwrap();
    let text = stdout(&output);
    for gauge in ["CPU", "RAM"] {
        assert!(text.contains(gauge), "{text}");
    }
    assert!(!marker.exists(), "the dashboard ran a subprocess probe");
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    assert!(
        diagnostics.contains("subprocess probes: 0"),
        "{diagnostics}"
    );
}

#[test]
fn cpu_load_is_sampled_only_by_views_that_render_it() {
    let fixture = Fixture::new();
    let summary = report(&fixture.output(&["--json"]));
    let detail = report(&fixture.output(&["--full", "--json"]));
    assert_eq!(
        cpu_load_fact(&summary),
        None,
        "the plain summary renders no load gauge"
    );
    let load = cpu_load_fact(&detail).expect("detail view samples CPU load");
    assert!(load.ends_with('%'), "{load}");
}

#[test]
fn native_summary_and_json_work_without_optional_tools() {
    let fixture = Fixture::new();
    let text = stdout(&fixture.output(&[]));
    assert!(text.starts_with("System: "));
    assert!(text.contains("CPU: "));
    assert!(text.contains("MEMORY: "));
    report(&fixture.output(&["--json"]));
}

#[test]
fn timing_diagnostics_do_not_contaminate_json() {
    let fixture = Fixture::new();
    let output = fixture.output(&["--json", "--timings"]);
    report(&output);
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    for field in ["collection:", "subprocess probes:", "total:"] {
        assert!(diagnostics.contains(field), "{diagnostics}");
    }
}

#[test]
fn full_collection_enriches_only_missing_native_modules() {
    let fixture = Fixture::new();
    report(&fixture.output(&["--full", "--json"]));
    fixture.script("fastfetch", "#!/bin/sh\n/bin/cp \"$2\" \"$CAPTURE\"\nprintf '%s' '[{\"type\":\"Host\",\"result\":{\"name\":\"Enriched Host\"}}]'\n");
    let capture = fixture.root.path().join("requested.json");
    let output = fixture
        .command()
        .args(["--full", "--json", "--timings"])
        .env("CAPTURE", &capture)
        .output()
        .unwrap();
    let payload = report(&output);
    assert!(String::from_utf8_lossy(&output.stderr).contains("fastfetch:"));
    assert!(
        payload["system"]["system_facts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|fact| fact["label"] == "Host" && fact["value"] == "Enriched Host")
    );
    let requested: Value = serde_json::from_slice(&std::fs::read(capture).unwrap()).unwrap();
    let kinds = requested["modules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|module| {
            module
                .as_str()
                .unwrap_or_else(|| module["type"].as_str().unwrap())
        })
        .collect::<Vec<_>>();
    assert!(!kinds.contains(&"CPU"));
    assert!(!kinds.contains(&"OS"));
    assert!(kinds.contains(&"Host"));
    assert!(kinds.contains(&"Display"));
}

#[test]
fn failed_or_malformed_optional_enrichment_keeps_native_inventory() {
    let fixture = Fixture::new();
    for script in ["#!/bin/sh\nexit 1\n", "#!/bin/sh\nprintf 'bad json'\n"] {
        fixture.script("fastfetch", script);
        report(&fixture.output(&["--full", "--json"]));
    }
}

#[test]
fn help_version_surface_and_completion_do_not_run_probes() {
    let fixture = Fixture::new();
    let marker = fixture.root.path().join("probe-called");
    for name in ["ps", "fastfetch", "scutil", "hostname"] {
        fixture.script(name, "#!/bin/sh\nprintf called > \"$PROBE_MARKER\"\n");
    }
    for args in [&["--help"][..], &["--version"], &["--completions", "zsh"]] {
        stdout(
            &fixture
                .command()
                .args(args)
                .env("PROBE_MARKER", &marker)
                .output()
                .unwrap(),
        );
    }
    let payload: Value = serde_json::from_str(&stdout(
        &fixture
            .command()
            .arg("--command-dump")
            .env("PROBE_MARKER", &marker)
            .output()
            .unwrap(),
    ))
    .unwrap();
    assert_eq!(payload["version"], 1);
    assert_eq!(payload["command"]["path"], json!(["sysinfo"]));
    assert!(!marker.exists());
    let help = stdout(&fixture.output(&["--help"]));
    for flag in ["--pretty", "-p", "--full", "-f", "--health", "-hh"] {
        assert!(help.contains(flag));
    }
}

#[test]
fn benchmark_commands_are_not_available() {
    let fixture = Fixture::new();
    assert!(!fixture.output(&["bench", "--help"]).status.success());
    let document: Value =
        serde_json::from_str(&stdout(&fixture.output(&["--command-dump"]))).unwrap();
    assert!(!document.to_string().contains("bench"));
}

#[test]
fn reports_do_not_read_benchmark_history_or_mutate_host_configuration() {
    let fixture = Fixture::new();
    let history = fixture.root.path().join("benchmarks");
    std::fs::create_dir(&history).unwrap();
    std::fs::write(history.join("store.json"), "{broken\n").unwrap();
    let config = fixture.root.path().join("config/hosts.dotfile");
    let before = std::fs::read(&config).unwrap();
    report(&fixture.output(&["--json", "--health"]));
    assert_eq!(std::fs::read(config).unwrap(), before);
}
