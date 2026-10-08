use std::cmp::Reverse;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use hostkit::process::{self, CaptureLimits};
use wl_clipboard_rs::{copy, paste};

use crate::{PEER_READ, proto};

const READY: u8 = 0;
const HOLDER: &str = "dclip-hold";

enum Display {
    Wayland,
    X11,
}

fn display() -> Option<Display> {
    let set = |name| env::var_os(name).is_some_and(|value| !value.is_empty());
    if set("WAYLAND_DISPLAY") {
        Some(Display::Wayland)
    } else if set("DISPLAY") {
        Some(Display::X11)
    } else {
        None
    }
}

pub fn available() -> bool {
    display().is_some()
}

pub fn read() -> Result<String, String> {
    match display() {
        Some(Display::Wayland) => paste_wayland(),
        Some(Display::X11) => paste_x11(),
        None => Err("no display".into()),
    }
}

pub fn write(text: &str) -> Result<(), String> {
    match display() {
        Some(Display::Wayland) => spawn_holder(text),
        Some(Display::X11) => copy_x11(text),
        None => Err("no display".into()),
    }
}

fn paste_wayland() -> Result<String, String> {
    let pipe = match paste::get_contents(
        paste::ClipboardType::Regular,
        paste::Seat::Unspecified,
        paste::MimeType::Text,
    ) {
        Ok((pipe, _)) => pipe,
        Err(paste::Error::ClipboardEmpty | paste::Error::NoMimeType) => return Ok(String::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut bytes = Vec::new();
    pipe.take(proto::LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    text(bytes)
}

fn spawn_holder(text: &str) -> Result<(), String> {
    let mut child = Command::new(executable())
        .arg0(HOLDER)
        .current_dir("/")
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("clipboard holder: {error}"))?;
    let died = || "clipboard holder died".to_string();
    let (Some(mut stdin), Some(mut stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return Err(died());
    };
    stdin.write_all(text.as_bytes()).map_err(|_| died())?;
    drop(stdin);
    let mut status = [0_u8];
    match stdout.read(&mut status) {
        Ok(1) if status[0] == READY => Ok(()),
        Ok(1) => {
            let mut message = String::new();
            let _ = stdout.read_to_string(&mut message);
            let _ = child.wait();
            Err(message)
        }
        _ => Err(died()),
    }
}

fn executable() -> PathBuf {
    env::current_exe()
        .ok()
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from("/proc/self/exe"))
}

pub fn is_holder() -> bool {
    env::args_os().next().is_some_and(|name| name == HOLDER)
}

pub fn hold() -> Result<(), String> {
    let mut text = String::new();
    io::stdin()
        .read_to_string(&mut text)
        .map_err(|_| "input is not UTF-8".to_string())?;
    let mut options = copy::Options::new();
    options
        .clipboard(copy::ClipboardType::Regular)
        .foreground(true);
    let prepared = options
        .prepare_copy(
            copy::Source::Bytes(text.into_bytes().into()),
            copy::MimeType::Text,
        )
        .map_err(|error| error.to_string());
    let mut stdout = io::stdout();
    let prepared = match prepared {
        Ok(prepared) => {
            let _ = stdout.write_all(&[READY]).and_then(|()| stdout.flush());
            prepared
        }
        Err(message) => {
            let _ = stdout.write_all(format!("\x01{message}").as_bytes());
            return Err(message);
        }
    };
    prepared.serve().map_err(|error| error.to_string())
}

fn copy_x11(text: &str) -> Result<(), String> {
    let mut child = Command::new("xclip")
        .args(["-selection", "clipboard", "-in"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("xclip: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .map_err(|error| format!("xclip: {error}"))?;
    }
    match child.wait() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("xclip: {status}")),
        Err(error) => Err(format!("xclip: {error}")),
    }
}

fn paste_x11() -> Result<String, String> {
    let mut command = Command::new("xclip");
    command.args(["-selection", "clipboard", "-out"]);
    captured(&mut command, "xclip")
}

pub fn read_for_peer() -> Result<String, String> {
    let uid = nix::unistd::getuid().as_raw();
    let runtime = hostkit::env::runtime_dir(env::var_os("XDG_RUNTIME_DIR"), uid);
    let socket = wayland_socket(
        Path::new(&runtime),
        env::var_os("WAYLAND_DISPLAY").as_deref(),
    )
    .ok_or_else(|| "no clipboard".to_string())?;
    let mut command = Command::new(executable());
    command
        .arg("-o")
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("WAYLAND_DISPLAY", socket)
        .env_remove("DISPLAY")
        .env_remove("HWIRE_SESSION")
        .env_remove("SSH_CONNECTION")
        .env_remove("SSH_TTY")
        .stdin(Stdio::null());
    captured(&mut command, "clipboard")
}

fn captured(command: &mut Command, label: &str) -> Result<String, String> {
    let limits = CaptureLimits {
        stdout: proto::LIMIT + 1,
        stderr: 4096,
    };
    let output =
        process::output(command, limits, PEER_READ).map_err(|error| match error.kind() {
            io::ErrorKind::TimedOut => format!("{label} timed out"),
            _ => format!("{label}: {error}"),
        })?;
    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if reason.is_empty() {
            format!("{label}: {}", output.status)
        } else {
            reason
        });
    }
    text(output.stdout)
}

fn text(bytes: Vec<u8>) -> Result<String, String> {
    if bytes.len() > proto::LIMIT {
        return Err("clipboard too large".into());
    }
    String::from_utf8(bytes).map_err(|_| "clipboard is not UTF-8".into())
}

fn wayland_socket(runtime: &Path, preferred: Option<&OsStr>) -> Option<OsString> {
    let preferred = preferred.filter(|name| !name.is_empty());
    if let Some(name) = preferred
        && answers(&runtime.join(name))
    {
        return Some(name.to_owned());
    }
    let mut found: Vec<_> = fs::read_dir(runtime)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("wayland-") && !name.ends_with(".lock")
        })
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            metadata.file_type().is_socket().then(|| {
                let modified = metadata.modified().ok();
                (entry.file_name(), modified)
            })
        })
        .collect();
    found.sort_by_key(|(name, modified)| (Reverse(*modified), name.clone()));
    found
        .into_iter()
        .map(|(name, _)| name)
        .find(|name| answers(&runtime.join(name)))
}

fn answers(socket: &Path) -> bool {
    UnixStream::connect(socket).is_ok()
}

#[cfg(test)]
#[path = "../../tests/unit/native/linux_tests.rs"]
mod tests;
