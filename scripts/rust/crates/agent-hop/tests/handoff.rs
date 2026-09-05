#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use hostkit::shell::quote;
use serde_json::Value;

struct Fixture {
    _temp: tempfile::TempDir,
    source: PathBuf,
    destination: PathBuf,
    workspace: PathBuf,
    source_socket: String,
    destination_socket: String,
    tmux: PathBuf,
    source_id: String,
    destination_id: Option<String>,
}

impl Fixture {
    fn new(fail_auth: bool, lose_ack: bool) -> Option<Self> {
        Self::new_for("claude", fail_auth, lose_ack)
    }
    fn new_for(agent: &str, fail_auth: bool, lose_ack: bool) -> Option<Self> {
        let tmux = Command::new("sh")
            .args(["-c", "command -v tmux"])
            .output()
            .ok()?;
        if !tmux.status.success() {
            return None;
        }
        let tmux = PathBuf::from(String::from_utf8(tmux.stdout).unwrap().trim());
        let temp = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(temp.path()).unwrap();
        let source = base.join("source");
        let destination = base.join("destination");
        let workspace = source.join("project");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&destination).unwrap();
        let suffix = temp.path().file_name().unwrap().to_str().unwrap();
        let source_socket = format!("ah-test-src-{suffix}");
        let destination_socket = format!("ah-test-dst-{suffix}");
        for (home, socket) in [
            (&source, &source_socket),
            (&destination, &destination_socket),
        ] {
            let bin = home.join(".local/bin");
            fs::create_dir_all(&bin).unwrap();
            symlink(env!("CARGO_BIN_EXE_agent-hop"), bin.join("agent-hop")).unwrap();
            if agent.starts_with("codex") {
                let native = Command::new("sh")
                    .args(["-c", "command -v codex"])
                    .output()
                    .unwrap();
                assert!(
                    native.status.success(),
                    "native Codex required for this ignored probe"
                );
                symlink(
                    String::from_utf8(native.stdout).unwrap().trim(),
                    bin.join("codex"),
                )
                .unwrap();
                fs::create_dir_all(home.join(".codex/sessions")).unwrap();
                fs::write(home.join(".codex/config.toml"), format!("model = \"fixture-no-inference\"\nmodel_provider = \"fixture\"\n[model_providers.fixture]\nname = \"Isolated no-inference fixture\"\nbase_url = \"http://127.0.0.1:9/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = false\n[projects.{}]\ntrust_level = \"trusted\"\n",serde_json::to_string(&workspace).unwrap())).unwrap();
                if agent == "codex-resume" && home == &source {
                    let session = "01999999-1111-7222-8333-444444444444";
                    let path = home.join(".codex/sessions/2026/09/05");
                    fs::create_dir_all(&path).unwrap();
                    let metadata = serde_json::json!({"timestamp":"2026-09-05T01:00:00Z","type":"session_meta","payload":{"id":session,"timestamp":"2026-09-05T01:00:00Z","cwd":workspace,"originator":"codex_cli_rs","cli_version":"0.153.2","source":"cli","model_provider":"fixture"}});
                    let message = serde_json::json!({"timestamp":"2026-09-05T01:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Synthetic saved history for a no-inference readiness test."}]}});
                    fs::write(
                        path.join(format!("rollout-2026-09-05T01-00-00-{session}.jsonl")),
                        format!("{metadata}\n{message}\n"),
                    )
                    .unwrap();
                }
            }
            executable(
                &bin.join("claude"),
                include_str!("fixtures/handoff_claude.py"),
            );
            executable(
                &bin.join("tmux"),
                &format!(
                    "#!/bin/sh\nexec {} -L {} \"$@\"\n",
                    quote(tmux.to_str().unwrap()),
                    quote(socket)
                ),
            );
        }
        let remote_script = format!(
            "#!/bin/sh\nfor arg do script=$arg; done\nexport HOME={}\nexport PATH={}:/usr/bin:/bin\nexport AH_FAIL_AUTH={}\n/usr/bin/python3 -c 'import subprocess,sys;sys.exit(subprocess.call([\"/bin/sh\",\"-c\",sys.argv[1]],close_fds=True))' \"$script\"\nresult=$?\ncase \"$script\" in *activate*) if [ {} = 1 ]; then exit 255; fi;; esac\nexit $result\n",
            quote(destination.to_str().unwrap()),
            quote(destination.join(".local/bin").to_str().unwrap()),
            if fail_auth { "1" } else { "0" },
            if lose_ack { "1" } else { "0" }
        );
        executable(&source.join(".local/bin/ssh"), &remote_script);
        git(&workspace, &["init", "-b", "main"]);
        git(&workspace, &["config", "user.name", "Test"]);
        git(
            &workspace,
            &["config", "user.email", "test@example.invalid"],
        );
        fs::write(workspace.join("file"), "base\n").unwrap();
        git(&workspace, &["add", "file"]);
        git(&workspace, &["commit", "-m", "fixture"]);
        fs::write(workspace.join("file"), "dirty\n").unwrap();
        fs::write(workspace.join("untracked"), "new\n").unwrap();
        let launch = format!(
            "exec env HOME={} CODEX_HOME={} PATH={}:/usr/bin:/bin {} run {}",
            quote(source.to_str().unwrap()),
            quote(source.join(".codex").to_str().unwrap()),
            quote(source.join(".local/bin").to_str().unwrap()),
            quote(env!("CARGO_BIN_EXE_agent-hop")),
            if agent == "codex-resume" {
                "codex --resume 01999999-1111-7222-8333-444444444444"
            } else {
                agent
            },
        );
        let output = Command::new(&tmux)
            .args([
                "-L",
                &source_socket,
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                "test",
                "-c",
            ])
            .arg(&workspace)
            .arg(launch)
            .env_remove("TMUX")
            .output()
            .unwrap();
        assert_ok(output);
        let mut fixture = Self {
            _temp: temp,
            source,
            destination,
            workspace,
            source_socket,
            destination_socket,
            tmux,
            source_id: String::new(),
            destination_id: None,
        };
        fixture.source_id = wait_until(|| {
            let output = Command::new(&fixture.tmux)
                .args([
                    "-L",
                    &fixture.source_socket,
                    "show-option",
                    "-p",
                    "-v",
                    "-t",
                    "%0",
                    "@agent_hop_run",
                ])
                .output()
                .unwrap();
            if output.status.success() {
                Some(String::from_utf8(output.stdout).unwrap().trim().into())
            } else {
                None
            }
        });
        let startup_until = Instant::now() + Duration::from_secs(20);
        wait_until(|| {
            let state = fixture
                .state(&fixture.source, &fixture.source_id)
                .filter(|s| s["phase"] == "running" || s["phase"] == "failed");
            if let Some(state) = &state
                && state["phase"] == "failed"
            {
                let pane = Command::new(&fixture.tmux)
                    .args([
                        "-L",
                        &fixture.source_socket,
                        "capture-pane",
                        "-p",
                        "-t",
                        "%0",
                    ])
                    .output()
                    .unwrap();
                panic!(
                    "native startup failed: {state}\n{}",
                    String::from_utf8_lossy(&pane.stdout)
                );
            }
            if Instant::now() > startup_until {
                let pane = Command::new(&fixture.tmux)
                    .args([
                        "-L",
                        &fixture.source_socket,
                        "capture-pane",
                        "-p",
                        "-t",
                        "%0",
                    ])
                    .output()
                    .unwrap();
                panic!(
                    "native startup did not become ready: {}",
                    String::from_utf8_lossy(&pane.stdout)
                );
            }
            state
        });
        Some(fixture)
    }

    fn state(&self, home: &Path, id: &str) -> Option<Value> {
        fs::read(
            home.join(".local/state/agent-hop/runs")
                .join(id)
                .join("state.json"),
        )
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    }
    fn source_state(&self) -> Value {
        self.state(&self.source, &self.source_id).unwrap()
    }
    fn command(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_agent-hop"))
            .args(args)
            .current_dir(&self.workspace)
            .env("HOME", &self.source)
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.source.join(".local/bin").display()),
            )
            .env_remove("TMUX")
            .output()
            .unwrap()
    }
    fn queue(&mut self) {
        assert_ok(self.command(&[
            "move",
            "--pane",
            "%0",
            "--to",
            hostkit::Host::this().unwrap().peer().name(),
        ]));
        wait_until(|| {
            let state = self.source_state();
            if state["phase"] == "queued" {
                state["destination_run"].as_str().map(str::to_owned)
            } else {
                None
            }
        });
        self.destination_id = self.source_state()["destination_run"]
            .as_str()
            .map(str::to_owned);
    }
    fn finish(&self) {
        fs::write(self.source.join("finish-turn"), "yes").unwrap();
    }
}

#[test]
#[ignore = "requires installed native Codex; creates an empty isolated thread without inference"]
fn installed_codex_native_ui_attaches_through_readiness_proxy_without_inference() {
    let fixture = Fixture::new_for("codex", false, false).expect("tmux required");
    assert_eq!(fixture.source_state()["phase"], "running");
    assert_ok(
        Command::new(&fixture.tmux)
            .args(["-L", &fixture.source_socket, "send-keys", "-t", "%0", "C-d"])
            .output()
            .unwrap(),
    );
    wait_until(|| {
        let state = fixture.source_state();
        (state["phase"] == "closed").then_some(state)
    });
}

#[test]
#[ignore = "requires installed native Codex; resumes synthetic history without inference"]
fn installed_codex_native_resume_moves_to_an_independent_private_workspace_without_inference() {
    let mut fixture = Fixture::new_for("codex-resume", false, false).expect("tmux required");
    let host_config = fs::read(fixture.destination.join(".codex/config.toml")).unwrap();
    assert_ok(fixture.command(&[
        "move",
        "--pane",
        "%0",
        "--to",
        hostkit::Host::this().unwrap().peer().name(),
    ]));
    let diagnostic_deadline = Instant::now() + Duration::from_secs(20);
    let state = wait_until(|| {
        let state = fixture.source_state();
        if state["phase"] == "running" && !state["error"].is_null() {
            panic!("native handoff refused: {state}");
        }
        if Instant::now() > diagnostic_deadline {
            let pane = Command::new(&fixture.tmux)
                .args([
                    "-L",
                    &fixture.destination_socket,
                    "capture-pane",
                    "-p",
                    "-t",
                    "%0",
                ])
                .output()
                .unwrap();
            panic!(
                "native handoff waiting: {state}\n{}",
                String::from_utf8_lossy(&pane.stdout)
            );
        }
        (state["phase"] == "moved").then_some(state)
    });
    fixture.destination_id = state["destination_run"].as_str().map(str::to_owned);
    let target = fixture
        .state(
            &fixture.destination,
            fixture.destination_id.as_deref().unwrap(),
        )
        .unwrap();
    assert_eq!(target["phase"], "running");
    assert_eq!(
        fs::read(fixture.destination.join(".codex/config.toml")).unwrap(),
        host_config
    );
    assert_ok(
        Command::new(&fixture.tmux)
            .args(["-L", &fixture.source_socket, "kill-server"])
            .output()
            .unwrap(),
    );
    assert_ok(
        Command::new(&fixture.tmux)
            .args([
                "-L",
                &fixture.destination_socket,
                "send-keys",
                "-t",
                target["pane"].as_str().unwrap(),
                "C-d",
            ])
            .output()
            .unwrap(),
    );
    wait_until(|| {
        fixture
            .state(
                &fixture.destination,
                fixture.destination_id.as_deref().unwrap(),
            )
            .filter(|state| state["phase"] == "closed")
    });
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.source.join(".codex/sessions").exists() {
            let _ = Command::new(&self.tmux)
                .args(["-L", &self.source_socket, "send-keys", "-t", "%0", "C-d"])
                .output();
            std::thread::sleep(Duration::from_millis(500));
        }
        for socket in [&self.source_socket, &self.destination_socket] {
            let _ = Command::new(&self.tmux)
                .args(["-L", socket, "kill-server"])
                .output();
        }
    }
}

fn executable(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}
fn assert_ok(output: Output) {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
fn git(root: &Path, args: &[&str]) {
    assert_ok(
        Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap(),
    );
}
fn wait_until<T>(mut test: impl FnMut() -> Option<T>) -> T {
    let until = Instant::now() + Duration::from_secs(25);
    loop {
        if let Some(value) = test() {
            return value;
        }
        assert!(Instant::now() < until, "handoff condition timed out");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn active_session_moves_only_after_turn_end_and_survives_source_tmux_shutdown() {
    let Some(mut fixture) = Fixture::new(false, false) else {
        return;
    };
    let cancelled = fixture.command(&["cancel", "--pane", "%0"]);
    assert!(!cancelled.status.success());
    assert!(
        !fixture
            .source
            .join(".local/state/agent-hop/runs")
            .join(&fixture.source_id)
            .join("cancel.json")
            .exists()
    );
    fixture.queue();
    assert!(!fixture.source.join("agent.stopped").exists());
    assert!(!fixture.destination.join("agent.pid").exists());
    fixture.finish();
    wait_until(|| {
        let state = fixture.source_state();
        if state["phase"] == "moved" {
            Some(state)
        } else {
            None
        }
    });
    assert!(fixture.source.join("agent.stopped").exists());
    let target = fixture
        .state(
            &fixture.destination,
            fixture.destination_id.as_deref().unwrap(),
        )
        .unwrap();
    assert_eq!(target["phase"], "running");
    let workspace = PathBuf::from(target["workspace"].as_str().unwrap());
    assert_eq!(
        fs::read_to_string(workspace.join("file")).unwrap(),
        "dirty\n"
    );
    assert!(workspace.join("untracked").exists());
    assert_ok(
        Command::new(&fixture.tmux)
            .args(["-L", &fixture.source_socket, "kill-server"])
            .output()
            .unwrap(),
    );
    assert_ok(
        Command::new(&fixture.tmux)
            .args([
                "-L",
                &fixture.destination_socket,
                "has-session",
                "-t",
                &format!("ah-{}", fixture.destination_id.as_deref().unwrap()),
            ])
            .output()
            .unwrap(),
    );
    assert!(!fixture.destination.join("agent.stopped").exists());
}

#[test]
fn destination_auth_failure_preserves_source_execution() {
    let Some(mut fixture) = Fixture::new(true, false) else {
        return;
    };
    fixture.queue();
    fixture.finish();
    let state = wait_until(|| {
        let state = fixture.source_state();
        if state["phase"] == "running" && !state["error"].is_null() {
            Some(state)
        } else {
            None
        }
    });
    assert!(
        state["error"]
            .as_str()
            .unwrap()
            .contains("not authenticated")
    );
    assert!(!fixture.source.join("agent.stopped").exists());
    assert!(!fixture.destination.join("agent.pid").exists());
}

#[test]
fn lost_activation_reply_does_not_resume_a_second_owner() {
    let Some(mut fixture) = Fixture::new(false, true) else {
        return;
    };
    fixture.queue();
    fixture.finish();
    wait_until(|| {
        let state = fixture.source_state();
        if state["phase"] == "commit-uncertain" && !state["error"].is_null() {
            Some(state)
        } else {
            None
        }
    });
    assert!(fixture.source.join("agent.stopped").exists());
    wait_until(|| {
        fixture
            .state(
                &fixture.destination,
                fixture.destination_id.as_deref().unwrap(),
            )
            .filter(|s| s["phase"] == "running")
    });
    let recovery = fixture.command(&["recover", "--run", &fixture.source_id]);
    assert!(!recovery.status.success());
    assert!(String::from_utf8_lossy(&recovery.stderr).contains("destination owns execution"));
}

#[test]
fn failed_destination_never_returns_ownership_to_a_stale_source() {
    let Some(mut fixture) = Fixture::new(false, false) else {
        return;
    };
    fixture.queue();
    fixture.finish();
    wait_until(|| (fixture.source_state()["phase"] == "moved").then_some(()));
    fs::write(fixture.destination.join("stop-owner"), "yes").unwrap();
    let id = fixture.destination_id.as_deref().unwrap();
    let mut state = wait_until(|| {
        fixture
            .state(&fixture.destination, id)
            .filter(|s| s["phase"] == "closed")
    });
    assert_eq!(state["ownership_committed"], true);
    // Simulate a later failure after activation-marker archival during recovery.
    let directory = fixture
        .destination
        .join(".local/state/agent-hop/runs")
        .join(id);
    fs::rename(
        directory.join("activate.json"),
        directory.join("recovered-activate.json"),
    )
    .unwrap();
    state["phase"] = serde_json::json!("failed");
    fs::write(
        directory.join("state.json"),
        serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();
    let output = fixture.command(&["recover", "--run", &fixture.source_id]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("destination owns execution"));
    assert!(fixture.source.join("agent.stopped").exists());
}
