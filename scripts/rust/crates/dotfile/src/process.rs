use std::io;
use std::process::Command;
use std::time::Duration;

pub use hostkit::process::{CaptureLimits, CapturedOutput};

pub fn output(
    command: &mut Command,
    limits: CaptureLimits,
    timeout: Duration,
) -> io::Result<CapturedOutput> {
    hostkit::process::output_cancellable(command, limits, timeout, &crate::cancel::requested)
}

/// Run an interactive tool in the foreground, restoring terminal ownership on every exit.
pub fn status(command: &mut Command) -> io::Result<std::process::ExitStatus> {
    #[cfg(unix)]
    {
        use nix::sys::signal::{Signal, killpg};
        use nix::unistd::{Pid, getpgrp, tcgetpgrp};
        use std::os::unix::process::{CommandExt, ExitStatusExt};
        struct Child {
            child: std::process::Child,
            terminal: Option<(std::fs::File, Pid)>,
        }
        impl Drop for Child {
            fn drop(&mut self) {
                if let Ok(pid) = i32::try_from(self.child.id()) {
                    let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
                }
                let _ = self.child.wait();
                if let Some((terminal, previous)) = &self.terminal {
                    let _ = foreground(terminal, *previous);
                }
            }
        }
        let terminal = std::fs::File::options()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .ok()
            .and_then(|file| {
                tcgetpgrp(&file)
                    .ok()
                    .filter(|p| *p == getpgrp())
                    .map(|p| (file, p))
            });
        let mut child = Child {
            child: command.process_group(0).spawn()?,
            terminal,
        };
        let group = Pid::from_raw(i32::try_from(child.child.id()).map_err(io::Error::other)?);
        if let Some((terminal, _)) = &child.terminal {
            foreground(terminal, group)?;
            let _ = killpg(group, Signal::SIGCONT);
        }
        loop {
            if let Some(status) = child.child.try_wait()? {
                if let Some(signal) = status.signal()
                    && matches!(
                        signal,
                        libc::SIGHUP | libc::SIGINT | libc::SIGQUIT | libc::SIGTERM
                    )
                {
                    crate::cancel::request_signal(signal);
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "command interrupted",
                    ));
                }
                return Ok(status);
            }
            if crate::cancel::requested() {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "command cancelled",
                ));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[cfg(not(unix))]
    command.status()
}

#[cfg(unix)]
fn foreground(terminal: &std::fs::File, group: nix::unistd::Pid) -> io::Result<()> {
    use nix::sys::signal::{SigSet, SigmaskHow, Signal, pthread_sigmask};
    let mut previous = SigSet::empty();
    pthread_sigmask(
        SigmaskHow::SIG_BLOCK,
        Some(&SigSet::from(Signal::SIGTTOU)),
        Some(&mut previous),
    )
    .map_err(io::Error::from)?;
    struct Restore(SigSet);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = self.0.thread_set_mask();
        }
    }
    let _restore = Restore(previous);
    nix::unistd::tcsetpgrp(terminal, group).map_err(io::Error::from)
}
