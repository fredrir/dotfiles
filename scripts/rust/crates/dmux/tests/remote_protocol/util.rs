//! Shared scaffolding for the remote_protocol suite: scratch owner
//! environments (tempdir registry + lock dirs), optional scratch tmux
//! servers with the epoch bootstrap, and direct-argv `_agent` invocation
//! against the REAL built binary.

use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use dmux::model::HostUid;
use dmux::operations::OperationEnv;
use dmux::registry::{Registry, RegistryConfig, RegistryIdentity};
use dmux::remote::client::request_envelope;
use dmux::remote::protocol::Envelope;
use serde_json::Value;
use uuid::Uuid;

pub const DMUX_BIN: &str = env!("CARGO_BIN_EXE_dmux");

pub struct Scratch {
    pub data: tempfile::TempDir,
    pub locks: tempfile::TempDir,
    pub ns: Option<String>,
}

impl Scratch {
    /// Owner environment only — no tmux server.
    pub fn new(_tag: &str) -> Scratch {
        Scratch {
            data: tempfile::tempdir().unwrap(),
            locks: tempfile::tempdir().unwrap(),
            ns: None,
        }
    }

    /// Owner environment with a scratch tmux server (`-L` namespace) whose
    /// environment carries DMUX_RUNTIME_DIR so bootstrap-helper panes reach
    /// the broker FIFOs, epoch-bootstrapped through the real hidden
    /// subcommand.
    pub fn with_tmux(tag: &str) -> Scratch {
        let mut scratch = Scratch::new(tag);
        let ns = format!("dmux-p7-{tag}-{}", std::process::id());
        let out = Command::new("tmux")
            .args([
                "-L",
                &ns,
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                "seed",
            ])
            .env("DMUX_RUNTIME_DIR", scratch.locks.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "scratch tmux server: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        scratch.ns = Some(ns);
        scratch.bootstrap_tmux();
        scratch
    }

    pub fn bootstrap_tmux(&self) {
        let out = Command::new(DMUX_BIN)
            .args([
                "_tmux-bootstrap",
                "--namespace",
                self.ns.as_deref().unwrap(),
                "--data-dir",
                self.data.path().to_str().unwrap(),
                "--lock-dir",
                self.locks.path().to_str().unwrap(),
            ])
            .env_remove("TMUX")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "_tmux-bootstrap: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    pub fn env(&self) -> OperationEnv {
        OperationEnv {
            db_path: self.data.path().join("registry.sqlite3"),
            lock_dir: self.locks.path().to_path_buf(),
        }
    }

    pub fn registry(&self) -> Registry {
        Registry::open(RegistryConfig::new(
            self.data.path().join("registry.sqlite3"),
            self.locks.path(),
        ))
        .unwrap()
    }

    pub fn tmux(&self, args: &[&str]) -> Output {
        Command::new("tmux")
            .args(["-L", self.ns.as_deref().expect("tmux scratch")])
            .args(args)
            .output()
            .unwrap()
    }

    pub fn session_names(&self) -> Vec<String> {
        String::from_utf8_lossy(
            &self
                .tmux(&["list-sessions", "-F", "#{session_name}"])
                .stdout,
        )
        .lines()
        .map(str::to_string)
        .collect()
    }

    /// Invoke the REAL binary's `_agent` with this scratch's seams, writing
    /// `body` (already-serialized request document) to stdin.
    pub fn agent_raw(&self, protocol: u32, method: &str, body: &str) -> Output {
        let mut child = Command::new(DMUX_BIN)
            .args([
                "_agent",
                "--protocol",
                &protocol.to_string(),
                method,
                "--data-dir",
                self.data.path().to_str().unwrap(),
                "--lock-dir",
                self.locks.path().to_str().unwrap(),
            ])
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(body.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    /// Invoke `_agent` with a typed envelope; returns (exit, response).
    pub fn agent(&self, request: &Envelope) -> (i32, Envelope) {
        let out = self.agent_raw(
            request.protocol_version,
            &request.method,
            &serde_json::to_string(request).unwrap(),
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        let response: Envelope = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
            panic!(
                "agent stdout must be one envelope: {e}\nstdout: {stdout}\nstderr: {}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
        (out.status.code().unwrap_or(-1), response)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if self.ns.is_some() {
            let _ = self.tmux(&["kill-server"]);
        }
    }
}

/// A synthetic CLIENT identity for request envelopes (the request carries
/// the caller's authority fields; the agent validates digest/protocol, not
/// the caller's identity).
pub fn client_identity() -> RegistryIdentity {
    RegistryIdentity {
        host_uid: HostUid(Uuid::from_u128(0xC11E17)),
        registry_uid: dmux::model::RegistryUid(Uuid::from_u128(0xC11E18)),
        schema_version: 2,
        created_at: "2026-08-16T00:00:00Z".to_string(),
    }
}

pub fn envelope(method: &str, request_uid: Uuid, payload: Value) -> Envelope {
    request_envelope(
        &client_identity(),
        &dmux::registry::AuthorityHead {
            revision: 0,
            head_hash: "sha256:client".to_string(),
        },
        method,
        request_uid,
        payload,
    )
}

/// Poll until `probe` returns true or the deadline passes.
pub fn wait_for(what: &str, deadline: Duration, mut probe: impl FnMut() -> bool) {
    let end = Instant::now() + deadline;
    while !probe() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The typed error inside a response envelope.
pub fn error_code(envelope: &Envelope) -> String {
    let error = envelope
        .error
        .as_ref()
        .unwrap_or_else(|| panic!("expected error envelope, got {envelope:?}"));
    serde_json::to_value(error.code)
        .unwrap()
        .as_str()
        .unwrap()
        .to_string()
}
