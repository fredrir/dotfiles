use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(5);

// Stdout of a command, or None when it fails, prints nothing, or outlives `timeout`.
pub fn output(program: &Path, args: &[&str], cwd: &Path, timeout: Duration) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .env("FORCE_COLOR", "0")
        .env("COLUMNS", "200")
        .env("CI", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;
    let reader = thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let errors = thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(POLL),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let text = reader.join().ok()?;
    let errors = errors.join().ok().unwrap_or_default();
    // Some tools print help to stderr.
    let text = if text.trim().is_empty() { errors } else { text };
    (!text.trim().is_empty()).then_some(text)
}

pub struct Lines {
    pub lines: Vec<String>,
    // The exit code, when the command ended on its own.
    pub code: Option<i32>,
}

// Stderr lines that `keep` accepts, stopping the command after `limit` of them or at `timeout`.
pub fn stderr_lines(
    program: &Path,
    args: &[String],
    cwd: &Path,
    stdin: Option<Vec<u8>>,
    timeout: Duration,
    keep: fn(&str) -> bool,
    limit: usize,
) -> Lines {
    let mut found = Lines {
        lines: Vec::new(),
        code: None,
    };
    let spawned = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn();
    let Ok(mut child) = spawned else {
        return found;
    };
    if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
        thread::spawn(move || {
            let _ = pipe.write_all(&bytes);
        });
    }
    let Some(stderr) = child.stderr.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return found;
    };
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stderr).split(b'\n') {
            let Ok(line) = line else { break };
            let line = String::from_utf8_lossy(&line);
            if keep(&line) && sender.send(line.into_owned()).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + timeout;
    let mut ended = false;
    while found.lines.len() < limit {
        match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(line) => found.lines.push(line),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                ended = true;
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => break,
        }
    }
    while ended && Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(status)) => {
                found.code = status.code();
                return found;
            }
            Ok(None) => thread::sleep(POLL),
            Err(_) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    found
}
