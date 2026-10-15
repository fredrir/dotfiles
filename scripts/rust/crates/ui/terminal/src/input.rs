use crossterm::event::Event;

/// The outcome of waiting on the terminal for a single event.
#[derive(Debug)]
pub enum Waited {
    Event(Event),
    Idle,
    HangUp,
}

#[cfg(unix)]
mod imp {
    use super::Waited;

    use std::cell::Cell;
    use std::fs::{File, OpenOptions};
    use std::io::{self, IsTerminal, Read, Write};
    use std::os::fd::{AsFd, BorrowedFd};
    use std::os::unix::net::UnixStream;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use crossterm::event::{self, Event};
    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
    use rustix::io::ioctl_fionread;
    use rustix::termios::tcgetwinsize;

    const LOST: PollFlags = PollFlags::POLLHUP
        .union(PollFlags::POLLERR)
        .union(PollFlags::POLLNVAL);

    enum Source {
        Stdin(io::Stdin),
        Device(File),
    }

    /// Wakes an input wait when background work is ready. Notifications coalesce.
    #[derive(Clone)]
    pub struct InputWake(Arc<UnixStream>);

    impl InputWake {
        pub fn wake(&self) {
            // A full socket already holds a notification; a closed one means
            // the input owner has exited. Neither should block a worker.
            let _ = (&*self.0).write(&[1]);
        }
    }

    /// Terminal input that notices its descriptor going away.
    ///
    /// crossterm polls the terminal for readability alone, and a pty whose
    /// other end has closed stays readable forever at end of file. Its reader
    /// answers that by reading zero bytes in an unbounded loop, so it never
    /// returns and the caller never regains control to notice the hangup,
    /// honour a signal, or exit.
    ///
    /// The wait is therefore ours, not crossterm's: we block on the descriptor
    /// ourselves, so a hangup wakes us instead of stranding crossterm, and we
    /// hand crossterm the descriptor only once we have established there are
    /// bytes worth reading. An idle terminal never reaches it at all, which is
    /// the case a hangup usually arrives in.
    pub struct Input {
        source: Source,
        wake_reader: UnixStream,
        waker: InputWake,
        /// Whether crossterm may still be holding events it has already
        /// parsed. It serves those from its own queue without touching the
        /// descriptor, and one read can yield several.
        queued: Cell<bool>,
        /// A resize arrives as SIGWINCH rather than as anything to read, so
        /// crossterm only reports one when something else wakes it. Measuring
        /// the terminal ourselves keeps resizes coming while it stays idle.
        measured: Cell<Option<(u16, u16)>>,
    }

    impl Input {
        pub fn new() -> io::Result<Self> {
            let stdin = io::stdin();
            if stdin.is_terminal() {
                return Self::primed(Source::Stdin(stdin));
            }
            let device = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
            Self::primed(Source::Device(device))
        }

        /// crossterm builds its reader on first use, which is slow enough that
        /// a terminal closing during it would strand the very first wait. Get
        /// that out of the way here, while the terminal is known to be alive.
        fn primed(source: Source) -> io::Result<Self> {
            let (wake_reader, wake_writer) = UnixStream::pair()?;
            wake_reader.set_nonblocking(true)?;
            wake_writer.set_nonblocking(true)?;
            let input = Self {
                source,
                wake_reader,
                waker: InputWake(Arc::new(wake_writer)),
                queued: Cell::new(false),
                measured: Cell::new(None),
            };
            input.measured.set(input.size());
            if !input.hung_up()? {
                let _ = event::poll(Duration::ZERO)?;
            }
            Ok(input)
        }

        pub fn waker(&self) -> InputWake {
            self.waker.clone()
        }

        fn descriptor(&self) -> BorrowedFd<'_> {
            match &self.source {
                Source::Stdin(stdin) => stdin.as_fd(),
                Source::Device(device) => device.as_fd(),
            }
        }

        fn revents(&self, timeout: PollTimeout, wake: bool) -> io::Result<PollFlags> {
            let mut watched = [
                PollFd::new(self.descriptor(), PollFlags::POLLIN),
                PollFd::new(self.wake_reader.as_fd(), PollFlags::POLLIN),
            ];
            let count = if wake { 2 } else { 1 };
            match poll(&mut watched[..count], timeout) {
                Ok(0) | Err(nix::errno::Errno::EINTR) => return Ok(PollFlags::empty()),
                Err(error) => return Err(error.into()),
                Ok(_) => {}
            }
            if wake
                && watched[1]
                    .revents()
                    .is_some_and(|flags| flags.contains(PollFlags::POLLIN))
            {
                let mut bytes = [0; 256];
                while (&self.wake_reader)
                    .read(&mut bytes)
                    .is_ok_and(|size| size > 0)
                {}
            }
            Ok(watched[0].revents().unwrap_or_else(PollFlags::empty))
        }

        fn size(&self) -> Option<(u16, u16)> {
            let size = tcgetwinsize(self.descriptor()).ok()?;
            (size.ws_col > 0 && size.ws_row > 0).then_some((size.ws_col, size.ws_row))
        }

        fn resized(&self) -> Option<Event> {
            let size = self.size()?;
            if self.measured.get() == Some(size) {
                return None;
            }
            self.measured.set(Some(size));
            Some(Event::Resize(size.0, size.1))
        }

        /// Whether the terminal has gone away, answered without waiting.
        pub fn hung_up(&self) -> io::Result<bool> {
            let flags = self.revents(PollTimeout::ZERO, false)?;
            if flags.intersects(LOST) {
                return Ok(true);
            }
            // Readable with nothing to read is end of file, which not every
            // platform bothers to flag as a hangup.
            Ok(flags.contains(PollFlags::POLLIN) && ioctl_fionread(self.descriptor())? == 0)
        }

        /// Waits up to `timeout` for one event, a hangup, or nothing at all.
        pub fn wait(&self, timeout: Duration) -> io::Result<Waited> {
            let deadline = Instant::now() + timeout;
            loop {
                if self.queued.get() {
                    if self.hung_up()? {
                        return Ok(Waited::HangUp);
                    }
                    if event::poll(Duration::ZERO)? {
                        return Ok(Waited::Event(event::read()?));
                    }
                    self.queued.set(false);
                }

                if let Some(resize) = self.resized() {
                    return Ok(Waited::Event(resize));
                }

                let left = deadline.saturating_duration_since(Instant::now());
                let timeout = PollTimeout::try_from(left).unwrap_or(PollTimeout::MAX);
                let ready = self.revents(timeout, true)?;
                if ready.intersects(LOST) {
                    return Ok(Waited::HangUp);
                }
                if !ready.contains(PollFlags::POLLIN) {
                    if let Some(resize) = self.resized() {
                        return Ok(Waited::Event(resize));
                    }
                    return Ok(Waited::Idle);
                }
                if ioctl_fionread(self.descriptor())? == 0 {
                    return Ok(Waited::HangUp);
                }

                if event::poll(Duration::ZERO)? {
                    self.queued.set(true);
                    return Ok(Waited::Event(event::read()?));
                }
                // Part of an escape sequence: crossterm holds the bytes until
                // the rest of them turn up.
                if deadline.saturating_duration_since(Instant::now()).is_zero() {
                    return Ok(Waited::Idle);
                }
            }
        }
    }
}

#[cfg(not(unix))]
mod imp {
    use super::Waited;
    use crossterm::event;
    use std::io;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::{Duration, Instant};

    #[derive(Clone)]
    pub struct InputWake(Arc<AtomicBool>);

    impl InputWake {
        pub fn wake(&self) {
            self.0.store(true, Ordering::Release);
        }
    }

    pub struct Input {
        wake: InputWake,
    }

    impl Input {
        pub fn new() -> io::Result<Self> {
            Ok(Self {
                wake: InputWake(Arc::new(AtomicBool::new(false))),
            })
        }
        pub fn waker(&self) -> InputWake {
            self.wake.clone()
        }
        pub fn hung_up(&self) -> io::Result<bool> {
            Ok(false)
        }
        pub fn wait(&self, timeout: Duration) -> io::Result<Waited> {
            let deadline = Instant::now() + timeout;
            loop {
                if self.wake.0.swap(false, Ordering::AcqRel) {
                    return Ok(Waited::Idle);
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                if event::poll(remaining.min(Duration::from_millis(8)))? {
                    return Ok(Waited::Event(event::read()?));
                }
                if Instant::now() >= deadline {
                    return Ok(Waited::Idle);
                }
            }
        }
    }
}

pub use imp::{Input, InputWake};
