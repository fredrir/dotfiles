use std::io::{self, BufReader, Read, Seek};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub(crate) struct Output<T> {
    pub status: ExitStatus,
    pub stdout: T,
    pub stderr: Vec<u8>,
}

pub(crate) fn capture(
    command: &mut Command,
    cancel: Option<&AtomicBool>,
    timeout: Option<Duration>,
) -> io::Result<Output<Vec<u8>>> {
    stream(command, cancel, timeout, |mut reader| {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        Ok(bytes)
    })
}

pub(crate) fn stream<T>(
    command: &mut Command,
    cancel: Option<&AtomicBool>,
    timeout: Option<Duration>,
    consume: impl FnOnce(BufReader<io::PipeReader>) -> io::Result<T>,
) -> io::Result<Output<T>> {
    check_cancel(cancel)?;
    let (reader, writer) = io::pipe()?;
    let mut errors = tempfile::tempfile()?;
    command.stdout(writer).stderr(errors.try_clone()?);
    let child = hostkit::process::ChildGroup::spawn(command);
    // Command retains its pipe ends after spawn; release them so readers see EOF.
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = child?;
    let finished = AtomicBool::new(false);
    let started = Instant::now();
    let (stdout, status) = std::thread::scope(|scope| {
        let finished = &finished;
        let monitor = scope.spawn(move || {
            loop {
                check_cancel(cancel)?;
                if let Some(status) = child.try_wait()? {
                    // Descendants must not keep the output pipe open after Git exits.
                    child.terminate();
                    return Ok(status);
                }
                if finished.load(Ordering::Acquire) {
                    child.terminate();
                    return Err(io::Error::other(
                        "Output reader stopped before command completion",
                    ));
                }
                if timeout.is_some_and(|timeout| started.elapsed() >= timeout) {
                    child.terminate();
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "command timed out"));
                }
                std::thread::park_timeout(Duration::from_millis(5));
            }
        });
        let stdout = consume(BufReader::with_capacity(64 * 1024, reader));
        if stdout.is_err() {
            finished.store(true, Ordering::Release);
        }
        monitor.thread().unpark();
        let status = monitor
            .join()
            .unwrap_or_else(|_| Err(io::Error::other("Command monitor panicked")));
        (stdout, status)
    });
    check_cancel(cancel)?;
    let stdout = stdout?;
    let status = status?;
    let mut stderr = Vec::new();
    if !status.success() {
        errors.rewind()?;
        errors.read_to_end(&mut stderr)?;
    }
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

pub(crate) fn check_cancel(cancel: Option<&AtomicBool>) -> io::Result<()> {
    if cancel.is_some_and(|cancel| cancel.load(Ordering::Acquire)) {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "command cancelled",
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn read_file(
    path: &std::path::Path,
    cancel: Option<&AtomicBool>,
) -> io::Result<Vec<u8>> {
    let mut file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        check_cancel(cancel)?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
}
