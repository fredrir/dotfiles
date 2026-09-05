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

pub fn width(text: &str) -> usize {
    let mut count = 0;
    let mut rest = text.chars();
    while let Some(character) = rest.next() {
        if character != '\x1b' {
            count += 1;
            continue;
        }
        for escaped in rest.by_ref() {
            if escaped.is_ascii_alphabetic() {
                break;
            }
        }
    }
    count
}

pub fn fit(text: &str, limit: usize) -> String {
    crate::text::truncate_back(text, limit)
}

#[derive(Clone, Copy, Debug)]
pub struct SignalOptions {
    pub hook: Option<fn()>,
    pub reset_to_default: bool,
    pub reraise_on_drop: bool,
    pub restart_syscalls: bool,
}

impl Default for SignalOptions {
    fn default() -> Self {
        Self {
            hook: None,
            reset_to_default: false,
            reraise_on_drop: true,
            restart_syscalls: false,
        }
    }
}

#[cfg(unix)]
#[allow(unsafe_code)]
mod imp {
    use super::{Key, SignalOptions};
    use std::fs::{File, OpenOptions};
    use std::io::{self, Read, Write};
    use std::os::fd::AsRawFd;
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, Ordering};

    const TERMINATION_SIGNALS: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];

    static TERMINATION_SIGNAL: AtomicI32 = AtomicI32::new(0);
    static TERMINATION_HOOK: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
    static RESET_TO_DEFAULT: AtomicBool = AtomicBool::new(false);

    pub struct SignalGuard {
        previous: Vec<(libc::c_int, libc::sigaction)>,
        reraise_on_drop: bool,
    }

    impl SignalGuard {
        pub fn new() -> io::Result<Self> {
            Self::with_options(SignalOptions::default())
        }

        pub fn with_options(options: SignalOptions) -> io::Result<Self> {
            TERMINATION_SIGNAL.store(0, Ordering::Release);
            TERMINATION_HOOK.store(
                options
                    .hook
                    .map_or(std::ptr::null_mut(), |hook| hook as *mut ()),
                Ordering::Release,
            );
            RESET_TO_DEFAULT.store(options.reset_to_default, Ordering::Release);
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            action.sa_sigaction = terminal_signal as *const () as libc::sighandler_t;
            action.sa_flags = if options.restart_syscalls {
                libc::SA_RESTART
            } else {
                0
            };
            unsafe { libc::sigemptyset(&raw mut action.sa_mask) };
            let mut previous = Vec::with_capacity(TERMINATION_SIGNALS.len());
            for signal in TERMINATION_SIGNALS {
                let mut prior: libc::sigaction = unsafe { std::mem::zeroed() };
                if unsafe { libc::sigaction(signal, &raw const action, &raw mut prior) } != 0 {
                    for (installed, handler) in previous.into_iter().rev() {
                        unsafe {
                            libc::sigaction(installed, &raw const handler, std::ptr::null_mut())
                        };
                    }
                    return Err(io::Error::last_os_error());
                }
                previous.push((signal, prior));
            }
            Ok(Self {
                previous,
                reraise_on_drop: options.reraise_on_drop,
            })
        }
    }

    impl Drop for SignalGuard {
        fn drop(&mut self) {
            for (signal, handler) in self.previous.drain(..).rev() {
                unsafe { libc::sigaction(signal, &raw const handler, std::ptr::null_mut()) };
            }
            TERMINATION_HOOK.store(std::ptr::null_mut(), Ordering::Release);
            RESET_TO_DEFAULT.store(false, Ordering::Release);
            if !self.reraise_on_drop {
                return;
            }
            let signal = TERMINATION_SIGNAL.swap(0, Ordering::AcqRel);
            if signal != 0 {
                unsafe { libc::raise(signal) };
            }
        }
    }

    extern "C" fn terminal_signal(signal: libc::c_int) {
        TERMINATION_SIGNAL.store(signal, Ordering::Release);
        let hook = TERMINATION_HOOK.load(Ordering::Acquire);
        if !hook.is_null() {
            let hook: fn() = unsafe { std::mem::transmute(hook) };
            hook();
        }
        if RESET_TO_DEFAULT.load(Ordering::Acquire) {
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            action.sa_sigaction = libc::SIG_DFL;
            action.sa_flags = 0;
            unsafe { libc::sigemptyset(&raw mut action.sa_mask) };
            for signal in TERMINATION_SIGNALS {
                unsafe { libc::sigaction(signal, &raw const action, std::ptr::null_mut()) };
            }
        }
    }

    pub fn termination_requested() -> bool {
        TERMINATION_SIGNAL.load(Ordering::Acquire) != 0
    }

    pub fn termination_signal() -> i32 {
        TERMINATION_SIGNAL.load(Ordering::Acquire)
    }

    pub struct Screen {
        tty: File,
        saved: libc::termios,
        drawn: usize,
        _signals: SignalGuard,
    }

    impl Screen {
        pub fn open() -> io::Result<Option<Screen>> {
            let Ok(tty) = OpenOptions::new().read(true).write(true).open("/dev/tty") else {
                return Ok(None);
            };
            let fd = tty.as_raw_fd();
            // SAFETY: `termios` is a plain struct of integers and arrays, and
            // tcgetattr either fills it in and reports success or leaves it
            // alone and reports failure.
            let saved = unsafe {
                let mut saved: libc::termios = std::mem::zeroed();
                if libc::tcgetattr(fd, &mut saved) != 0 {
                    return Ok(None);
                }
                saved
            };
            let signals = SignalGuard::new()?;

            // OPOST stays on, so a newline still returns the carriage and the
            // frames below need no \r of their own. ISIG goes off so ctrl-c
            // arrives as a byte and the terminal is restored on the way out.
            let mut raw = saved;
            raw.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN);
            raw.c_iflag &= !(libc::IXON | libc::ICRNL);
            raw.c_cc[libc::VMIN] = 0;
            raw.c_cc[libc::VTIME] = 1;
            // SAFETY: `raw` is the struct tcgetattr just filled in, with only
            // flag bits and control characters changed.
            if unsafe { libc::tcsetattr(fd, libc::TCSADRAIN, &raw) } != 0 {
                return Ok(None);
            }

            let mut screen = Screen {
                tty,
                saved,
                drawn: 0,
                _signals: signals,
            };
            screen.put("\x1b[?25l")?;
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
            let fd = self.tty.as_raw_fd();
            let mut size: libc::winsize = unsafe { std::mem::zeroed() };
            let status = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &raw mut size) };
            (status == 0 && size.ws_col > 0 && size.ws_row > 0)
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
            let fd = self.tty.as_raw_fd();
            // SAFETY: `saved` is what tcgetattr returned for this descriptor
            // when the screen opened, put back unchanged.
            unsafe {
                libc::tcsetattr(fd, libc::TCSADRAIN, &self.saved);
            }
        }
    }
}

#[cfg(not(unix))]
mod imp {
    use super::{Key, SignalOptions};
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
    }
}

pub use imp::{Screen, SignalGuard, termination_requested, termination_signal};

#[cfg(test)]
#[path = "../tests/unit/screen_tests.rs"]
mod tests;
