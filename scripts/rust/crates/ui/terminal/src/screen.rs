#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Escape,
    Backspace,
    Tab,
    Char(char),
    Interrupt,
    Kill,
    WordBack,
    Home,
    End,
    PageUp,
    PageDown,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Key(Key),
    Resize { width: usize, height: usize },
}

#[derive(Clone, Copy, Debug)]
pub struct SignalOptions {
    pub cancellation: Option<&'static std::sync::atomic::AtomicBool>,
    pub reset_to_default: bool,
    pub reraise_on_drop: bool,
    pub restart_syscalls: bool,
}

impl Default for SignalOptions {
    fn default() -> Self {
        Self {
            cancellation: None,
            reset_to_default: false,
            reraise_on_drop: true,
            restart_syscalls: false,
        }
    }
}

#[cfg(unix)]
mod imp {
    use super::{Event, Key, SignalOptions};
    use nix::sys::select::{FdSet, select};
    use nix::sys::signal::{self, SaFlags, SigAction, SigHandler, SigSet, Signal};
    use nix::sys::time::{TimeVal, TimeValLike};
    use rustix::termios::{
        self, InputModes, LocalModes, OptionalActions, SpecialCodeIndex, Termios,
    };
    use std::fs::{File, OpenOptions};
    use std::io::{self, Read, Write};
    use std::os::fd::AsFd;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, Ordering};
    use std::time::{Duration, Instant};

    const TERMINATION_SIGNALS: [Signal; 3] = [Signal::SIGINT, Signal::SIGTERM, Signal::SIGHUP];

    static TERMINATION_SIGNAL: AtomicI32 = AtomicI32::new(0);
    static CANCELLATION: AtomicPtr<AtomicBool> = AtomicPtr::new(std::ptr::null_mut());
    static RESET_TO_DEFAULT: AtomicBool = AtomicBool::new(false);
    static GUARDS: Mutex<Guards> = Mutex::new(Guards {
        next_id: 0,
        active: Vec::new(),
        previous: Vec::new(),
    });

    struct Guards {
        next_id: u64,
        active: Vec<(u64, SignalOptions)>,
        previous: Vec<(Signal, SigAction)>,
    }

    pub struct SignalGuard {
        id: u64,
        reraise_on_drop: bool,
    }

    impl SignalGuard {
        pub fn new() -> io::Result<Self> {
            Self::with_options(SignalOptions::default())
        }

        pub fn with_options(options: SignalOptions) -> io::Result<Self> {
            let mut guards = GUARDS.lock().unwrap_or_else(|error| error.into_inner());
            let id = guards.next_id;
            guards.next_id = id
                .checked_add(1)
                .ok_or_else(|| io::Error::other("signal guard limit"))?;
            if guards.active.is_empty() {
                TERMINATION_SIGNAL.store(0, Ordering::Release);
            }
            publish(options);
            let previous = match install(options) {
                Ok(previous) => previous,
                Err(error) => {
                    publish(
                        guards
                            .active
                            .last()
                            .map_or_else(SignalOptions::default, |(_, options)| *options),
                    );
                    return Err(error.into());
                }
            };
            if guards.active.is_empty() {
                guards.previous = previous;
            }
            guards.active.push((id, options));
            Ok(Self {
                id,
                reraise_on_drop: options.reraise_on_drop,
            })
        }
    }

    impl Drop for SignalGuard {
        fn drop(&mut self) {
            let mut guards = GUARDS.lock().unwrap_or_else(|error| error.into_inner());
            let was_active = guards.active.last().is_some_and(|(id, _)| *id == self.id);
            guards.active.retain(|(id, _)| *id != self.id);
            if was_active {
                if let Some((_, options)) = guards.active.last() {
                    publish(*options);
                    let _ = install(*options);
                } else {
                    publish(SignalOptions::default());
                    for (signal, handler) in guards.previous.drain(..).rev() {
                        let _ = set_action(signal, &handler);
                    }
                }
            }
            drop(guards);
            if !was_active || !self.reraise_on_drop {
                return;
            }
            let signal = TERMINATION_SIGNAL.swap(0, Ordering::AcqRel);
            if let Ok(signal) = Signal::try_from(signal) {
                let _ = signal::raise(signal);
            }
        }
    }

    fn publish(options: SignalOptions) {
        CANCELLATION.store(
            options.cancellation.map_or(std::ptr::null_mut(), |flag| {
                std::ptr::from_ref(flag).cast_mut()
            }),
            Ordering::Release,
        );
        RESET_TO_DEFAULT.store(options.reset_to_default, Ordering::Release);
    }

    fn install(options: SignalOptions) -> nix::Result<Vec<(Signal, SigAction)>> {
        let flags = if options.restart_syscalls {
            SaFlags::SA_RESTART
        } else {
            SaFlags::empty()
        };
        let action = SigAction::new(SigHandler::Handler(terminal_signal), flags, SigSet::empty());
        let mut previous = Vec::with_capacity(TERMINATION_SIGNALS.len());
        for signal in TERMINATION_SIGNALS {
            match set_action(signal, &action) {
                Ok(prior) => previous.push((signal, prior)),
                Err(error) => {
                    for (installed, handler) in previous.into_iter().rev() {
                        let _ = set_action(installed, &handler);
                    }
                    return Err(error);
                }
            }
        }
        Ok(previous)
    }

    #[allow(unsafe_code)]
    fn set_action(signal: Signal, action: &SigAction) -> nix::Result<SigAction> {
        // SAFETY: Private callers use our handler, SIG_DFL, or a disposition
        // returned by the OS. Our handler only uses atomics and signal-safe
        // sigaction/sigemptyset calls; it never locks or allocates.
        unsafe { signal::sigaction(signal, action) }
    }

    #[allow(unsafe_code)]
    extern "C" fn terminal_signal(signal: i32) {
        let cancellation = CANCELLATION.load(Ordering::Acquire);
        // SAFETY: publish only stores pointers from shared 'static references.
        // Replacing a guard cannot invalidate a flag already loaded here.
        if let Some(flag) = unsafe { cancellation.as_ref() } {
            flag.store(true, Ordering::Release);
        }
        if RESET_TO_DEFAULT.load(Ordering::Acquire) {
            let action = SigAction::new(SigHandler::SigDfl, SaFlags::empty(), SigSet::empty());
            for signal in TERMINATION_SIGNALS {
                let _ = set_action(signal, &action);
            }
        }
        TERMINATION_SIGNAL.store(signal, Ordering::Release);
    }

    pub fn termination_requested() -> bool {
        TERMINATION_SIGNAL.load(Ordering::Acquire) != 0
    }

    pub fn termination_signal() -> i32 {
        TERMINATION_SIGNAL.load(Ordering::Acquire)
    }

    pub struct Screen {
        tty: File,
        saved: Termios,
        drawn: usize,
        last_size: Option<(usize, usize)>,
        _signals: SignalGuard,
    }

    impl Screen {
        pub fn open() -> io::Result<Option<Screen>> {
            let Ok(tty) = OpenOptions::new().read(true).write(true).open("/dev/tty") else {
                return Ok(None);
            };
            let Ok(saved) = termios::tcgetattr(&tty) else {
                return Ok(None);
            };
            let signals = SignalGuard::new()?;

            // OPOST stays on, so a newline still returns the carriage and the
            // frames below need no \r of their own. ISIG goes off so ctrl-c
            // arrives as a byte and the terminal is restored on the way out.
            let mut raw = saved.clone();
            raw.local_modes.remove(
                LocalModes::ICANON | LocalModes::ECHO | LocalModes::ISIG | LocalModes::IEXTEN,
            );
            raw.input_modes.remove(InputModes::IXON | InputModes::ICRNL);
            raw.special_codes[SpecialCodeIndex::VMIN] = 0;
            raw.special_codes[SpecialCodeIndex::VTIME] = 1;
            if termios::tcsetattr(&tty, OptionalActions::Drain, &raw).is_err() {
                return Ok(None);
            }

            let mut screen = Screen {
                tty,
                saved,
                drawn: 0,
                last_size: None,
                _signals: signals,
            };
            screen.put("\x1b[?25l")?;
            screen.last_size = screen.size();
            Ok(Some(screen))
        }

        pub fn draw(&mut self, lines: &[String]) -> io::Result<()> {
            let mut frame = String::new();
            if self.drawn > 0 {
                frame.push_str(&format!("\x1b[{}F", self.drawn));
            }
            for line in lines {
                frame.push_str("\x1b[2K");
                frame.push_str(line);
                frame.push('\n');
            }
            // Anything left over from a taller frame goes with it.
            frame.push_str("\x1b[0J");
            self.drawn = lines.len();
            self.put(&frame)
        }

        pub fn size(&self) -> Option<(usize, usize)> {
            let size = termios::tcgetwinsize(&self.tty).ok()?;
            (size.ws_col > 0 && size.ws_row > 0)
                .then_some((size.ws_col as usize, size.ws_row as usize))
        }

        pub fn clear(&mut self) -> io::Result<()> {
            if self.drawn == 0 {
                return Ok(());
            }
            let frame = format!("\x1b[{}F\x1b[0J", self.drawn);
            self.put(&frame)?;
            self.drawn = 0;
            Ok(())
        }

        pub fn key(&mut self) -> io::Result<Key> {
            let Some(first) = self.byte()? else {
                return Ok(Key::Interrupt);
            };
            Ok(match first {
                0x1b => self.escape()?,
                b'\r' | b'\n' => Key::Enter,
                0x7f | 0x08 => Key::Backspace,
                b'\t' => Key::Tab,
                0x03 | 0x04 => Key::Interrupt,
                0x15 => Key::Kill,
                0x17 => Key::WordBack,
                byte if byte < 0x20 => Key::Unknown,
                byte => self.utf8(byte)?,
            })
        }

        pub fn poll_event(&mut self, timeout: Duration) -> io::Result<Option<Event>> {
            let deadline = Instant::now() + timeout;
            loop {
                if termination_requested() {
                    return Ok(Some(Event::Key(Key::Interrupt)));
                }
                let size = self.size();
                if size != self.last_size {
                    self.last_size = size;
                    if let Some((width, height)) = size {
                        return Ok(Some(Event::Resize { width, height }));
                    }
                }
                let wait = deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(100));
                let mut readable = FdSet::new();
                readable.insert(self.tty.as_fd());
                let mut timeout =
                    TimeVal::microseconds(i64::try_from(wait.as_micros()).unwrap_or(i64::MAX));
                let ready = match select(None, Some(&mut readable), None, None, Some(&mut timeout))
                {
                    Ok(ready) => ready,
                    Err(nix::errno::Errno::EINTR) => continue,
                    Err(error) => return Err(error.into()),
                };
                if ready > 0 {
                    return self.key().map(|key| Some(Event::Key(key)));
                }
                if Instant::now() >= deadline {
                    return Ok(None);
                }
            }
        }

        pub fn event(&mut self) -> io::Result<Event> {
            loop {
                if let Some(event) = self.poll_event(Duration::from_millis(100))? {
                    return Ok(event);
                }
            }
        }

        fn escape(&mut self) -> io::Result<Key> {
            let Some(second) = self.waited()? else {
                return Ok(Key::Escape);
            };
            if second != b'[' && second != b'O' {
                return Ok(Key::Escape);
            }
            let Some(third) = self.waited()? else {
                return Ok(Key::Escape);
            };
            Ok(match third {
                b'A' => Key::Up,
                b'B' => Key::Down,
                b'C' => Key::Right,
                b'D' => Key::Left,
                b'H' => Key::Home,
                b'F' => Key::End,
                b'0'..=b'9' => self.tilde(u32::from(third - b'0'))?,
                _ => Key::Unknown,
            })
        }

        fn tilde(&mut self, first: u32) -> io::Result<Key> {
            let mut code = first;
            while let Some(byte) = self.waited()? {
                match byte {
                    b'0'..=b'9' => code = code * 10 + u32::from(byte - b'0'),
                    _ => break,
                }
            }
            Ok(match code {
                1 | 7 => Key::Home,
                4 | 8 => Key::End,
                5 => Key::PageUp,
                6 => Key::PageDown,
                _ => Key::Unknown,
            })
        }

        fn utf8(&mut self, first: u8) -> io::Result<Key> {
            let extra = match first {
                0x00..=0x7f => 0,
                0xc2..=0xdf => 1,
                0xe0..=0xef => 2,
                0xf0..=0xf4 => 3,
                _ => return Ok(Key::Unknown),
            };
            let mut bytes = vec![first];
            for _ in 0..extra {
                match self.byte()? {
                    Some(byte) => bytes.push(byte),
                    None => return Ok(Key::Unknown),
                }
            }
            Ok(std::str::from_utf8(&bytes)
                .ok()
                .and_then(|text| text.chars().next())
                .map_or(Key::Unknown, Key::Char))
        }

        fn byte(&mut self) -> io::Result<Option<u8>> {
            self.read_byte(false)
        }

        fn read_byte(&mut self, brief: bool) -> io::Result<Option<u8>> {
            let mut buffer = [0u8; 1];
            loop {
                if termination_requested() {
                    return Ok(None);
                }
                return match self.tty.read(&mut buffer) {
                    Ok(0) if brief => Ok(None),
                    Ok(0) => continue,
                    Ok(_) => Ok(Some(buffer[0])),
                    Err(error)
                        if error.kind() == io::ErrorKind::Interrupted
                            && termination_requested() =>
                    {
                        Ok(None)
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => Err(error),
                };
            }
        }

        // An escape byte is either a key on its own or the start of a
        // sequence, and only the pause after it tells the two apart.
        fn waited(&mut self) -> io::Result<Option<u8>> {
            self.read_byte(true)
        }

        fn put(&mut self, text: &str) -> io::Result<()> {
            self.tty.write_all(text.as_bytes())?;
            self.tty.flush()
        }
    }

    impl Drop for Screen {
        fn drop(&mut self) {
            let _ = self.clear();
            let _ = self.put("\x1b[?25h");
            let _ = termios::tcsetattr(&self.tty, OptionalActions::Drain, &self.saved);
        }
    }
}

#[cfg(not(unix))]
mod imp {
    use super::{Event, Key, SignalOptions};
    use std::io;

    pub struct SignalGuard;

    impl SignalGuard {
        pub fn new() -> io::Result<Self> {
            Ok(Self)
        }

        pub fn with_options(_options: SignalOptions) -> io::Result<Self> {
            Ok(Self)
        }
    }

    pub fn termination_requested() -> bool {
        false
    }

    pub fn termination_signal() -> i32 {
        0
    }

    pub struct Screen;

    impl Screen {
        pub fn open() -> io::Result<Option<Screen>> {
            Ok(None)
        }
        pub fn draw(&mut self, _lines: &[String]) -> io::Result<()> {
            Ok(())
        }
        pub fn size(&self) -> Option<(usize, usize)> {
            None
        }
        pub fn clear(&mut self) -> io::Result<()> {
            Ok(())
        }
        pub fn key(&mut self) -> io::Result<Key> {
            Ok(Key::Interrupt)
        }
        pub fn event(&mut self) -> io::Result<Event> {
            Ok(Event::Key(Key::Interrupt))
        }
        pub fn poll_event(&mut self, _timeout: std::time::Duration) -> io::Result<Option<Event>> {
            self.event().map(Some)
        }
    }
}

pub use imp::{Screen, SignalGuard, termination_requested, termination_signal};
