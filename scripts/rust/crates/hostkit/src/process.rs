use std::io;
use std::process::{Command, ExitStatus};
use std::time::Duration;

#[cfg(unix)]
pub use unix::ChildGroup;

#[derive(Clone, Copy, Debug)]
pub struct CaptureLimits {
    pub stdout: usize,
    pub stderr: usize,
}

impl Default for CaptureLimits {
    fn default() -> Self {
        Self {
            stdout: 1024 * 1024,
            stderr: 64 * 1024,
        }
    }
}

#[derive(Debug)]
pub struct CapturedOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

#[cfg(unix)]
pub fn output(
    command: &mut Command,
    limits: CaptureLimits,
    timeout: Duration,
) -> io::Result<CapturedOutput> {
    unix::output(command, limits, timeout, None)
}

#[cfg(unix)]
pub fn output_to_file(
    command: &mut Command,
    destination: &std::fs::File,
    stderr_limit: usize,
    timeout: Duration,
) -> io::Result<CapturedOutput> {
    unix::output(
        command,
        CaptureLimits {
            stdout: 0,
            stderr: stderr_limit,
        },
        timeout,
        Some((destination, None)),
    )
}

#[cfg(unix)]
pub fn output_to_file_limited(
    command: &mut Command,
    destination: &std::fs::File,
    stderr_limit: usize,
    max_bytes: u64,
    timeout: Duration,
) -> io::Result<CapturedOutput> {
    unix::output(
        command,
        CaptureLimits {
            stdout: 0,
            stderr: stderr_limit,
        },
        timeout,
        Some((destination, Some(max_bytes))),
    )
}

#[cfg(not(unix))]
pub fn output(
    _command: &mut Command,
    _limits: CaptureLimits,
    _timeout: Duration,
) -> io::Result<CapturedOutput> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "bounded subprocess execution requires Unix",
    ))
}

#[cfg(unix)]
mod unix {
    use std::io::{self, Read, Write};
    use std::os::fd::AsFd;
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    use nix::fcntl::{FcntlArg, OFlag, fcntl};
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;

    use super::{CaptureLimits, CapturedOutput};

    pub struct ChildGroup {
        child: Child,
        terminated: bool,
    }

    impl ChildGroup {
        pub fn spawn(command: &mut Command) -> io::Result<Self> {
            Ok(Self {
                child: command.process_group(0).spawn()?,
                terminated: false,
            })
        }

        #[allow(unsafe_code)]
        pub fn spawn_detached(command: &mut Command) -> io::Result<Self> {
            unsafe {
                command.pre_exec(|| nix::unistd::setsid().map(|_| ()).map_err(io::Error::from));
            }
            Ok(Self {
                child: command.spawn()?,
                terminated: false,
            })
        }

        pub fn try_wait(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
            self.child.try_wait()
        }

        pub fn signal(&self, signal: i32) -> io::Result<()> {
            let signal = Signal::try_from(signal).map_err(io::Error::from)?;
            let pid = i32::try_from(self.child.id()).map_err(io::Error::other)?;
            killpg(Pid::from_raw(pid), signal).map_err(io::Error::from)
        }

        pub fn terminate(&mut self) {
            if !self.terminated {
                if let Ok(pid) = i32::try_from(self.child.id()) {
                    let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
                }
                self.terminated = true;
            }
        }
    }

    impl Drop for ChildGroup {
        fn drop(&mut self) {
            self.terminate();
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    struct Capture<R> {
        pipe: R,
        bytes: Vec<u8>,
        limit: usize,
        truncated: bool,
        ended: bool,
        sink: Option<std::fs::File>,
        written: u64,
        max_bytes: Option<u64>,
    }

    impl<R: Read + AsFd> Capture<R> {
        fn new(pipe: R, limit: usize) -> io::Result<Self> {
            let flags = fcntl(&pipe, FcntlArg::F_GETFL).map_err(io::Error::from)?;
            fcntl(
                &pipe,
                FcntlArg::F_SETFL(OFlag::from_bits_truncate(flags) | OFlag::O_NONBLOCK),
            )
            .map_err(io::Error::from)?;
            Ok(Self {
                pipe,
                bytes: Vec::with_capacity(limit.min(16 * 1024)),
                limit,
                truncated: false,
                ended: false,
                sink: None,
                written: 0,
                max_bytes: None,
            })
        }

        fn drain(&mut self) -> io::Result<bool> {
            let mut buffer = [0_u8; 8192];
            let mut progress = false;
            for _ in 0..64 {
                match self.pipe.read(&mut buffer) {
                    Ok(0) => {
                        self.ended = true;
                        return Ok(progress);
                    }
                    Ok(count) => {
                        progress = true;
                        if let Some(sink) = &mut self.sink {
                            self.written = self.written.saturating_add(count as u64);
                            if self.max_bytes.is_some_and(|limit| self.written > limit) {
                                return Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "command output exceeds maximum file size",
                                ));
                            }
                            sink.write_all(&buffer[..count])?;
                        } else {
                            let retained = count.min(self.limit.saturating_sub(self.bytes.len()));
                            self.bytes.extend_from_slice(&buffer[..retained]);
                            self.truncated |= retained < count;
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(progress),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error),
                }
            }
            Ok(progress)
        }
    }

    pub(super) fn output(
        command: &mut Command,
        limits: CaptureLimits,
        timeout: Duration,
        destination: Option<(&std::fs::File, Option<u64>)>,
    ) -> io::Result<CapturedOutput> {
        if timeout.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "command timed out"));
        }
        let started = Instant::now();
        command.stderr(Stdio::piped());
        command.stdout(Stdio::piped());
        let mut owned = ChildGroup::spawn(command)?;
        let stdout = owned.child.stdout.take();
        let stderr = owned
            .child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("stderr pipe missing"))?;
        let mut stdout = stdout
            .map(|pipe| Capture::new(pipe, limits.stdout))
            .transpose()?;
        if let Some((destination, limit)) = destination
            && let Some(stdout) = &mut stdout
        {
            stdout.sink = Some(destination.try_clone()?);
            stdout.max_bytes = limit;
        }
        let mut stderr = Capture::new(stderr, limits.stderr)?;
        let mut status = None;
        loop {
            let stdout_progress = if let Some(stdout) = &mut stdout {
                stdout.drain()?
            } else {
                false
            };
            let stderr_progress = stderr.drain()?;
            if status.is_none() {
                status = owned.child.try_wait()?;
                if status.is_some() {
                    owned.terminate();
                }
            }
            if let Some(status) = status
                && stdout.as_ref().is_none_or(|capture| capture.ended)
                && stderr.ended
            {
                return Ok(CapturedOutput {
                    status,
                    stdout_truncated: stdout.as_ref().is_some_and(|capture| capture.truncated),
                    stdout: stdout.map_or_else(Vec::new, |capture| capture.bytes),
                    stderr: stderr.bytes,
                    stderr_truncated: stderr.truncated,
                });
            }
            if started.elapsed() >= timeout {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "command timed out"));
            }
            if !stdout_progress && !stderr_progress {
                thread::sleep(
                    Duration::from_millis(5).min(timeout.saturating_sub(started.elapsed())),
                );
            }
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/process_tests.rs"]
mod tests;
