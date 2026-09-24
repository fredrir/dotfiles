use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use hostkit::process::CaptureLimits;
use hostkit::{Host, Route};

use crate::forward::Forward;

// Unresolvable, so a session whose socket is gone fails instead of dialing another host
const CONTROL_HOST: &str = "hport.invalid";

const READY: Duration = Duration::from_secs(20);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Master {
    child: Child,
    socket: PathBuf,
    pub route: Option<Route>,
}

impl Master {
    pub fn connect(peer: Host, socket: &Path) -> Result<Master, String> {
        clear(socket);
        let route = hostkit::ssh::resolved(peer.name());
        let child = Command::new("ssh")
            .args(master_args(socket, peer))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(hostkit::ssh::command_error)?;
        let mut master = Master {
            child,
            socket: socket.to_path_buf(),
            route,
        };
        let deadline = Instant::now() + READY;
        loop {
            if let Ok(Some(status)) = master.child.try_wait() {
                return Err(format!("{}: ssh exited with {status}", peer.name()));
            }
            if master.control("check", None).is_ok() {
                return Ok(master);
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "{}: no connection after {}s",
                    peer.name(),
                    READY.as_secs()
                ));
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn forward(&self, forward: &Forward) -> Result<(), String> {
        self.control("forward", Some(forward))
    }

    pub fn cancel(&self, forward: &Forward) -> Result<(), String> {
        self.control("cancel", Some(forward))
    }

    pub fn session(&self, script: &str) -> Command {
        let mut command = Command::new("ssh");
        command.args(session_args(&self.socket, script));
        command
    }

    fn control(&self, operation: &str, forward: Option<&Forward>) -> Result<(), String> {
        control(&self.socket, operation, forward)
    }
}

impl Drop for Master {
    fn drop(&mut self) {
        let _ = self.control("exit", None);
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.socket);
    }
}

fn control(socket: &Path, operation: &str, forward: Option<&Forward>) -> Result<(), String> {
    let mut command = Command::new("ssh");
    command
        .args(control_args(socket, operation, forward))
        .stdin(Stdio::null());
    let output = hostkit::process::output(&mut command, CaptureLimits::default(), CONTROL_TIMEOUT)
        .map_err(hostkit::ssh::command_error)?;
    if output.status.success() {
        return Ok(());
    }
    Err(hostkit::ssh::stderr_reason(
        &output.stderr,
        &format!("ssh -O {operation} failed"),
    ))
}

// A daemon killed without unwinding leaves its master holding every forward.
fn clear(socket: &Path) {
    if socket.exists() {
        let _ = control(socket, "exit", None);
        let _ = std::fs::remove_file(socket);
    }
}

pub fn master_args(socket: &Path, peer: Host) -> Vec<OsString> {
    let mut args = ["-M", "-N", "-T", "-S"].map(OsString::from).to_vec();
    args.push(socket.into());
    for option in [
        "ControlPersist=no",
        "ExitOnForwardFailure=no",
        "BatchMode=yes",
        "ConnectionAttempts=1",
    ]
    .into_iter()
    .chain(hostkit::ssh::OPTIONS)
    {
        args.extend([OsString::from("-o"), OsString::from(option)]);
    }
    args.extend([OsString::from("--"), OsString::from(peer.name())]);
    args
}

pub fn control_args(socket: &Path, operation: &str, forward: Option<&Forward>) -> Vec<OsString> {
    let mut args = ["-F", "none", "-S"].map(OsString::from).to_vec();
    args.push(socket.into());
    args.extend(["-O", operation].map(OsString::from));
    if let Some(forward) = forward {
        args.extend([OsString::from("-L"), OsString::from(forward.spec())]);
    }
    args.push(OsString::from(CONTROL_HOST));
    args
}

pub fn session_args(socket: &Path, script: &str) -> Vec<OsString> {
    let mut args = ["-F", "none", "-T", "-o", "LogLevel=ERROR", "-S"]
        .map(OsString::from)
        .to_vec();
    args.push(socket.into());
    args.extend([CONTROL_HOST, script].map(OsString::from));
    args
}

#[cfg(test)]
#[path = "../tests/unit/master_tests.rs"]
mod tests;
