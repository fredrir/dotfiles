use std::fs::File;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;
use tungstenite::{Message, WebSocket};

pub(super) struct Codex {
    child: Child,
    _socket_dir: TempDir,
    pub endpoint: String,
    connection: WebSocket<UnixStream>,
    next_id: u64,
    stopped: bool,
}

impl Codex {
    pub fn start(
        workspace: &Path,
        agent_home: &Path,
        log: &Path,
        trusted_snapshot: bool,
    ) -> Result<Self, String> {
        let socket_dir = tempfile::Builder::new()
            .prefix("ah-")
            .tempdir()
            .map_err(super::error)?;
        let socket = socket_dir.path().join("s");
        let endpoint = format!("unix://{}", socket.display());
        let log = File::create(log).map_err(super::error)?;
        let mut command = Command::new("codex");
        command.args(["app-server", "--listen", &endpoint]);
        if trusted_snapshot {
            command.args(["-c", &snapshot_trust(workspace)?]);
        }
        let mut child = command
            .env("CODEX_HOME", agent_home)
            .current_dir(workspace)
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(super::error)?)
            .stderr(log)
            .process_group(0)
            .spawn()
            .map_err(|e| format!("start Codex app-server: {e}"))?;
        let until = Instant::now() + Duration::from_secs(30);
        let stream = loop {
            if child.try_wait().map_err(super::error)?.is_some() {
                return Err("Codex app-server exited; inspect the run's server.log".into());
            }
            match UnixStream::connect(&socket) {
                Ok(stream) => break stream,
                Err(_) if Instant::now() < until => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => {
                    let _ = signal_group(&child, libc::SIGTERM);
                    return Err(format!("Codex control socket: {e}"));
                }
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(15)))
            .map_err(super::error)?;
        stream
            .set_write_timeout(Some(Duration::from_secs(15)))
            .map_err(super::error)?;
        let (connection, _) = match tungstenite::client("ws://localhost/", stream) {
            Ok(value) => value,
            Err(e) => {
                let _ = signal_group(&child, libc::SIGTERM);
                return Err(format!("Codex control handshake: {e}"));
            }
        };
        let mut server = Self {
            child,
            _socket_dir: socket_dir,
            endpoint,
            connection,
            next_id: 0,
            stopped: false,
        };
        server.request(
            "initialize",
            json!({"clientInfo": {"name": "agent_hop", "version": env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}),
        )?;
        server.send(json!({"method": "initialized", "params": {}}))?;
        server.request("remoteControl/disable", json!({"ephemeral":true}))?;
        let remote = server.request("remoteControl/status/read", json!({}))?;
        if remote.get("status").and_then(Value::as_str) != Some("disabled") {
            return Err("managed Codex requires its remote-control transport disabled".into());
        }
        Ok(server)
    }

    fn send(&mut self, value: Value) -> Result<(), String> {
        self.connection
            .send(Message::Text(value.to_string().into()))
            .map_err(|e| format!("Codex control: {e}"))
    }

    pub fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"id": id, "method": method, "params": params}))?;
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            if Instant::now() >= until {
                return Err(format!("Codex {method} timed out"));
            }
            let message = self
                .connection
                .read()
                .map_err(|e| format!("Codex {method}: {e}"))?;
            let Message::Text(text) = message else {
                continue;
            };
            let response: Value = serde_json::from_str(&text).map_err(super::error)?;
            if response.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = response.get("error") {
                return Err(format!("Codex {method}: {error}"));
            }
            return response
                .get("result")
                .cloned()
                .ok_or_else(|| "Codex response has no result".into());
        }
    }

    pub fn open(&mut self, workspace: &Path, resume: Option<&str>) -> Result<String, String> {
        let (method, mut params) = match resume {
            Some(id) => ("thread/resume", json!({"threadId": id, "cwd": workspace})),
            None => ("thread/start", json!({"cwd": workspace})),
        };
        params["excludeTurns"] = json!(true);
        let response = self.request(method, params)?;
        let id = response
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .ok_or("Codex returned no thread ID")?;
        crate::session::SessionId::new(id)?;
        Ok(id.into())
    }

    pub fn idle(&mut self, root: &str) -> Result<bool, String> {
        let loaded = self.request("thread/loaded/list", json!({}))?;
        let threads = loaded
            .get("data")
            .and_then(Value::as_array)
            .ok_or("Codex returned no loaded-thread list")?;
        if !threads.iter().any(|id| id.as_str() == Some(root)) {
            return Err("managed Codex thread is no longer loaded".into());
        }
        for id in threads {
            let id = id.as_str().ok_or("invalid loaded thread ID")?;
            let read = self.request("thread/read", json!({"threadId": id}))?;
            if read.pointer("/thread/status/type").and_then(Value::as_str) != Some("idle") {
                return Ok(false);
            }
            let goal = self.request("thread/goal/get", json!({"threadId": id}))?;
            if goal.pointer("/goal/status").and_then(Value::as_str) == Some("active") {
                return Ok(false);
            }
            let terminals = self.request(
                "thread/backgroundTerminals/list",
                json!({"threadId":id,"limit":1}),
            )?;
            if terminals
                .get("data")
                .and_then(Value::as_array)
                .is_none_or(|items| !items.is_empty())
                || terminals.get("nextCursor").is_some_and(|v| !v.is_null())
            {
                return Ok(false);
            }
            let queued = self.request("thread/queue/list", json!({"threadId":id,"limit":1}))?;
            if queued
                .get("data")
                .and_then(Value::as_array)
                .is_none_or(|items| !items.is_empty())
                || queued.get("nextCursor").is_some_and(|v| !v.is_null())
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn authenticated(&mut self) -> Result<(), String> {
        let result = self.request("account/read", json!({"refreshToken":false}))?;
        if result.get("account").is_none_or(Value::is_null)
            && result.get("requiresOpenaiAuth").and_then(Value::as_bool) != Some(false)
        {
            return Err(
                "destination Codex is not authenticated; run codex login on that host".into(),
            );
        }
        Ok(())
    }

    pub fn pause(&self) -> Result<(), String> {
        signal_group(&self.child, libc::SIGSTOP)
    }
    pub fn unpause(&self) -> Result<(), String> {
        signal_group(&self.child, libc::SIGCONT)
    }

    pub fn shutdown(&mut self) -> Result<(), String> {
        if self.stopped {
            return Ok(());
        }
        let owned = super::child::descendants(self.child.id())?;
        signal_group(&self.child, libc::SIGTERM)?;
        signal_group(&self.child, libc::SIGCONT)?;
        let until = Instant::now() + Duration::from_secs(15);
        while self.child.try_wait().map_err(super::error)?.is_none() {
            if Instant::now() >= until {
                return Err("source app-server did not stop; destination remains fenced".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.stopped = true;
        super::child::require_gone(&owned)
    }
}

impl Drop for Codex {
    fn drop(&mut self) {
        if !self.stopped {
            let _ = signal_group(&self.child, libc::SIGTERM);
            let _ = signal_group(&self.child, libc::SIGCONT);
        }
    }
}

#[allow(unsafe_code)]
fn signal_group(child: &Child, signal: i32) -> Result<(), String> {
    let pid = i32::try_from(child.id()).map_err(super::error)?;
    // This process group was created explicitly for this owned child; never a pane PID.
    if unsafe { libc::kill(-pid, signal) } == 0 {
        Ok(())
    } else {
        Err(super::error(std::io::Error::last_os_error()))
    }
}

pub(super) fn ui(
    endpoint: &str,
    agent_home: &Path,
    workspace: &Path,
    session: &str,
    trusted_snapshot: bool,
) -> Result<super::child::AgentChild, String> {
    let mut command = Command::new("codex");
    command.args(["--remote", endpoint]);
    if trusted_snapshot {
        // The user explicitly requested this validated private checkpoint. Scope its
        // directory trust to this UI process; never write through host config symlinks
        // or weaken the destination app-server's approvals/sandbox policy.
        command.args(["-c", &snapshot_trust(workspace)?]);
    }
    if !session.is_empty() {
        command.args(["resume", session]);
    }
    command
        .arg("-C")
        .arg(workspace)
        .env("CODEX_HOME", agent_home)
        .current_dir(workspace);
    super::child::AgentChild::spawn(&mut command).map_err(|e| format!("start Codex UI: {e}"))
}

fn snapshot_trust(workspace: &Path) -> Result<String, String> {
    Ok(format!(
        "projects={{ {} = {{trust_level=\"trusted\"}} }}",
        serde_json::to_string(workspace).map_err(super::error)?
    ))
}

pub(super) fn default_home() -> Result<PathBuf, String> {
    Ok(std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or(crate::local_home()?.join(".codex")))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires installed Codex; no model turns are submitted"]
    fn installed_codex_control_protocol_without_inference() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join(".codex");
        std::fs::create_dir(&home).unwrap();
        // No credentials, plugins, or user's runtime databases are used by this probe.
        let mut server =
            Codex::start(temp.path(), &home, &temp.path().join("server.log"), false).unwrap();
        let id = server.open(temp.path(), None).unwrap();
        assert!(server.idle(&id).unwrap());
        assert!(server.authenticated().is_err());
        server.pause().unwrap();
        server.unpause().unwrap();
        server.shutdown().unwrap();
    }
}
