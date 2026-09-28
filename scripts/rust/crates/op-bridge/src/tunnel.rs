use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::Duration;

use hostkit::Host;
use hostkit::process::CaptureLimits;
use hostkit::ssh::Session;

use crate::paths::SOCKET_NAME;

const RETRY: Duration = Duration::from_secs(10);
const PREPARE_TIMEOUT: Duration = Duration::from_secs(15);

// sshd will not replace a socket left by a dropped connection, so clear it first
fn prepare_script() -> String {
    format!(
        "socket=\"${{XDG_RUNTIME_DIR:?}}/{SOCKET_NAME}\"; rm -f -- \"$socket\"; printf '%s\\n' \"$socket\""
    )
}

pub fn supervise(peer: Host, local: &Path, pid: &AtomicU32) {
    loop {
        if let Err(error) = connect(peer, local, pid) {
            eprintln!("op-bridge: {}: {error}", peer.name());
        }
        thread::sleep(RETRY);
    }
}

fn connect(peer: Host, local: &Path, pid: &AtomicU32) -> Result<(), String> {
    let remote = prepare(peer)?;
    let mut child = Command::new("ssh")
        .args(tunnel_args(peer, &remote, local))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(hostkit::ssh::command_error)?;
    pid.store(child.id(), Ordering::SeqCst);
    eprintln!("op-bridge: {}: forwarding {}", peer.name(), remote.display());
    let status = child.wait();
    pid.store(0, Ordering::SeqCst);
    match status {
        Ok(status) => Err(format!("tunnel closed: {status}")),
        Err(error) => Err(format!("tunnel: {error}")),
    }
}

fn prepare(peer: Host) -> Result<PathBuf, String> {
    let output = Session::new(peer.name())
        .batch()
        .script(&prepare_script())
        .output_bounded(CaptureLimits::default(), PREPARE_TIMEOUT)
        .map_err(hostkit::ssh::command_error)?;
    if !output.status.success() {
        return Err(hostkit::ssh::stderr_reason(
            &output.stderr,
            "could not clear the remote socket",
        ));
    }
    remote_socket(&output.stdout)
}

pub fn remote_socket(stdout: &[u8]) -> Result<PathBuf, String> {
    let path = std::str::from_utf8(stdout)
        .map_err(|_| "remote socket path is not UTF-8")?
        .trim();
    let valid = path.starts_with('/')
        && Path::new(path).file_name() == Some(SOCKET_NAME.as_ref())
        && !path.contains(':');
    if valid {
        Ok(PathBuf::from(path))
    } else {
        Err(format!("unexpected remote socket path: {path:?}"))
    }
}

pub fn tunnel_args(peer: Host, remote: &Path, local: &Path) -> Vec<OsString> {
    let mut args = ["-N", "-T"].map(OsString::from).to_vec();
    // Our own connection, so the broker can tell tunnel traffic by its pid
    for option in ["ExitOnForwardFailure=yes", "ControlMaster=no", "ControlPath=none"]
        .into_iter()
        .chain(hostkit::ssh::BATCH_OPTIONS)
        .chain(hostkit::ssh::OPTIONS)
    {
        args.extend([OsString::from("-o"), OsString::from(option)]);
    }
    let mut forward = OsString::from(remote);
    forward.push(":");
    forward.push(local);
    args.extend([OsString::from("-R"), forward]);
    args.extend([OsString::from("--"), OsString::from(peer.name())]);
    args
}

#[cfg(test)]
#[path = "../tests/unit/tunnel_tests.rs"]
mod tests;
