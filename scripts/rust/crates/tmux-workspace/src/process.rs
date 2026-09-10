use std::io::{self, Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;

use crate::Result;

#[derive(Debug)]
pub struct Output {
    pub code: i32,
    pub out: String,
    pub err: String,
}

impl Output {
    pub fn checked(self) -> Result<Self> {
        if self.code == 0 {
            Ok(self)
        } else {
            Err(if self.err.trim().is_empty() {
                format!("command exited {}: {}", self.code, self.out.trim())
            } else {
                self.err.trim().to_owned()
            }
            .into())
        }
    }
}

struct Pipes {
    out: JoinHandle<io::Result<Vec<u8>>>,
    err: JoinHandle<io::Result<Vec<u8>>>,
    writer: Option<JoinHandle<()>>,
}

impl Pipes {
    fn collect(self, status: ExitStatus) -> Result<Output> {
        if let Some(writer) = self.writer {
            let _ = writer.join();
        }
        let out = self.out.join().map_err(|_| "stdout reader failed")??;
        let err = self.err.join().map_err(|_| "stderr reader failed")??;
        Ok(Output {
            code: status.code().unwrap_or(128 + status.signal().unwrap_or(1)),
            out: String::from_utf8(out)?,
            err: String::from_utf8_lossy(&err).into_owned(),
        })
    }
}

fn spawn(cmd: &mut Command, input: Option<&[u8]>) -> Result<(Child, Pipes)> {
    cmd.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("{}: {e}", cmd.get_program().to_string_lossy()))?;
    let mut stdout = child.stdout.take().ok_or("stdout unavailable")?;
    let mut stderr = child.stderr.take().ok_or("stderr unavailable")?;
    let out = thread::spawn(move || {
        let mut v = Vec::new();
        stdout.read_to_end(&mut v).map(|_| v)
    });
    let err = thread::spawn(move || {
        let mut v = Vec::new();
        stderr.read_to_end(&mut v).map(|_| v)
    });
    let input = input.map(<[u8]>::to_vec);
    let writer = child.stdin.take().map(|mut pipe| {
        thread::spawn(move || {
            if let Some(input) = input {
                let _ = pipe.write_all(&input);
            }
        })
    });
    Ok((child, Pipes { out, err, writer }))
}

pub fn capture(
    cmd: &mut Command,
    input: Option<&[u8]>,
    timeout: Option<Duration>,
) -> Result<Output> {
    let (mut child, pipes) = spawn(cmd.process_group(0), input)?;
    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if timeout.is_some_and(|limit| started.elapsed() >= limit) {
            timed_out = true;
            terminate_group(child.id());
            let _ = child.kill();
            break child.wait()?;
        }
        thread::sleep(Duration::from_millis(5));
    };
    terminate_group(child.id());
    let output = pipes.collect(status);
    if timed_out {
        return Err(format!("{}: timed out", cmd.get_program().to_string_lossy()).into());
    }
    output
}

pub fn capture_foreground(cmd: &mut Command, input: Option<&[u8]>) -> Result<Output> {
    let (mut child, pipes) = spawn(cmd, input)?;
    let status = child.wait()?;
    pipes.collect(status)
}

pub fn run(cmd: &mut Command) -> Result<Output> {
    capture(cmd, None, Some(Duration::from_secs(15)))?.checked()
}

pub fn interactive(cmd: &mut Command) -> Result<i32> {
    let status = cmd
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    Ok(status.code().unwrap_or(128 + status.signal().unwrap_or(1)))
}

pub fn terminate_group(pid: u32) {
    let pid = Pid::from_raw(pid as i32);
    let _ = killpg(pid, Signal::SIGTERM);
    // All captured commands own their process group; tmux pane jobs live in the server's group.
    let _ = killpg(pid, Signal::SIGKILL);
}

pub fn which(program: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let executable = |p: &Path| {
        p.is_file()
            && p.metadata()
                .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    };
    if program.contains('/') {
        return executable(Path::new(program)).then(|| PathBuf::from(program));
    }
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|p| p.join(program))
        .find(|p| executable(p))
}

pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn shell(args: &[String]) -> String {
    args.iter()
        .map(|arg| quote(arg))
        .collect::<Vec<_>>()
        .join(" ")
}
