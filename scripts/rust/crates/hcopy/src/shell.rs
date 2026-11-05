use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

pub const REPLY_LIMIT: usize = 64 * 1024 * 1024;
const COMPLAINT_LIMIT: u64 = 64 * 1024;
const COMPLAINT_WAIT: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Lost,
    TimedOut,
    TooLarge,
}

enum Reply {
    Done(Vec<u8>),
    TooLarge,
}

// One long-lived remote `sh`: a question costs a round trip, not an ssh process.
pub struct Shell {
    child: Child,
    stdin: ChildStdin,
    replies: Receiver<Reply>,
    complaints: Receiver<Vec<u8>>,
    marker: String,
}

impl Shell {
    pub fn open(command: &mut Command) -> io::Result<Shell> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other("shell pipes missing"));
        };
        let marker = marker();
        let ending = format!("{marker}\0").into_bytes();
        let (reply_tx, replies) = mpsc::channel();
        std::thread::spawn(move || read_replies(stdout, &ending, &reply_tx));
        let (complaint_tx, complaints) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = complaint_tx.send(drain(stderr));
        });
        Ok(Shell {
            child,
            stdin,
            replies,
            complaints,
            marker,
        })
    }

    // The whole batch is written before any reply is awaited: one round trip.
    pub fn ask(&mut self, scripts: &[String], timeout: Duration) -> Result<Vec<Vec<u8>>, Failure> {
        let mut request = String::new();
        for script in scripts {
            request.push_str(&format!(
                "( {script}\n) </dev/null 2>/dev/null; printf '\\000%s\\000' '{}'\n",
                self.marker
            ));
        }
        self.stdin
            .write_all(request.as_bytes())
            .and_then(|()| self.stdin.flush())
            .map_err(|_| Failure::Lost)?;
        let deadline = Instant::now() + timeout;
        scripts
            .iter()
            .map(|_| {
                match self
                    .replies
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                {
                    Ok(Reply::Done(bytes)) => Ok(bytes),
                    Ok(Reply::TooLarge) => Err(Failure::TooLarge),
                    Err(RecvTimeoutError::Timeout) => Err(Failure::TimedOut),
                    Err(RecvTimeoutError::Disconnected) => Err(Failure::Lost),
                }
            })
            .collect()
    }

    pub fn complaint(&self) -> Option<String> {
        let bytes = self.complaints.recv_timeout(COMPLAINT_WAIT).ok()?;
        String::from_utf8_lossy(&bytes)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_string)
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn marker() -> String {
    let state = RandomState::new();
    format!(
        "hcopy-{:016x}{:016x}",
        state.hash_one(std::process::id()),
        state.hash_one(Instant::now())
    )
}

fn read_replies(stdout: impl Read, ending: &[u8], replies: &Sender<Reply>) {
    let mut reader = BufReader::new(stdout);
    let mut reply = Vec::new();
    loop {
        let start = reply.len();
        let budget = (REPLY_LIMIT + ending.len() + 1).saturating_sub(start) as u64;
        match (&mut reader).take(budget).read_until(0, &mut reply) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if start > 0 && reply[start - 1] == 0 && &reply[start..] == ending {
            reply.truncate(start - 1);
            if replies
                .send(Reply::Done(std::mem::take(&mut reply)))
                .is_err()
            {
                return;
            }
        } else if reply.len() > REPLY_LIMIT {
            let _ = replies.send(Reply::TooLarge);
            return;
        }
    }
}

fn drain(stderr: impl Read) -> Vec<u8> {
    let mut stderr = stderr;
    let mut kept = Vec::new();
    let _ = (&mut stderr).take(COMPLAINT_LIMIT).read_to_end(&mut kept);
    let _ = io::copy(&mut stderr, &mut io::sink());
    kept
}

#[cfg(test)]
#[path = "../tests/unit/shell_tests.rs"]
mod tests;
