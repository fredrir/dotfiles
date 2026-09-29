use std::ffi::OsString;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode};
use std::time::Duration;

use crate::paths;
use crate::protocol::{self, Request, Response};

#[cfg(target_os = "macos")]
const REAL_OP: &str = "/opt/homebrew/bin/op";
#[cfg(not(target_os = "macos"))]
const REAL_OP: &str = "/usr/bin/op";
// Long enough for a Touch ID prompt; pi gives up after 10 s on its own
const ANSWER_TIMEOUT: Duration = Duration::from_secs(60);
// Nothing is cached without a daemon, so callers may treat this as nothing to clear
pub const UNREACHABLE: u8 = 3;

#[derive(Debug, PartialEq, Eq)]
pub struct Read {
    pub reference: String,
    pub newline: bool,
}

impl Read {
    pub fn parse(args: &[OsString]) -> Option<Read> {
        let (command, rest) = args.split_first()?;
        if command != "read" {
            return None;
        }
        let mut newline = true;
        let mut reference = None;
        for arg in rest {
            match arg.to_str()? {
                "-n" | "--no-newline" => newline = false,
                flag if flag.starts_with('-') => return None,
                value if reference.is_none() => reference = Some(value.to_string()),
                _ => return None,
            }
        }
        Some(Read {
            reference: reference?,
            newline,
        })
    }
}

pub fn run(args: Vec<OsString>) -> Result<ExitCode, String> {
    if let Some(read) = Read::parse(&args)
        && let Ok(stream) = UnixStream::connect(paths::client()?)
    {
        match ask(&stream, &Request::Read(read.reference))? {
            Response::Value(value) => {
                let newline: &[u8] = if read.newline { b"\n" } else { b"" };
                let mut stdout = std::io::stdout().lock();
                stdout
                    .write_all(value.as_bytes())
                    .and_then(|()| stdout.write_all(newline))
                    .and_then(|()| stdout.flush())
                    .map_err(|error| format!("stdout: {error}"))?;
                return Ok(ExitCode::SUCCESS);
            }
            Response::Denied(reason) => return Err(reason),
            Response::Refused(reason) => eprintln!("op-bridge: {reason}; using local op"),
            Response::Reloaded { .. } => return Err("unexpected answer to a read".to_string()),
        }
    }
    let real = real_op();
    let error = Command::new(&real).args(args).exec();
    Err(format!("{}: {error}", real.to_string_lossy()))
}

pub fn reload() -> Result<ExitCode, String> {
    let socket = paths::client()?;
    let stream = match UnixStream::connect(&socket) {
        Ok(stream) => stream,
        Err(error) => {
            eprintln!("op-bridge: daemon unreachable at {}: {error}", socket.display());
            return Ok(ExitCode::from(UNREACHABLE));
        }
    };
    match ask(&stream, &Request::Reload)? {
        Response::Reloaded { refilled, known } if refilled == known => {
            println!("reloaded {refilled}/{known}");
            Ok(ExitCode::SUCCESS)
        }
        Response::Reloaded { refilled, known } => Err(format!(
            "reloaded {refilled}/{known}; see ~/Library/Logs/op-bridge.log on macie"
        )),
        Response::Refused(reason) | Response::Denied(reason) => Err(reason),
        Response::Value(_) => Err("unexpected answer to a reload".to_string()),
    }
}

fn ask(stream: &UnixStream, request: &Request) -> Result<Response, String> {
    stream
        .set_read_timeout(Some(ANSWER_TIMEOUT))
        .map_err(|error| error.to_string())?;
    protocol::send(stream, request)?;
    protocol::receive(stream).map_err(|error| format!("macie did not answer: {error}"))
}

fn real_op() -> OsString {
    std::env::var_os("OP_BRIDGE_OP")
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| OsString::from(REAL_OP))
}

#[cfg(test)]
#[path = "../tests/unit/client_tests.rs"]
mod tests;
