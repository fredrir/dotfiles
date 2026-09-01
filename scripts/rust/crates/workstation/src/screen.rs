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

// Plain text only: a row is truncated before any colour is put on it, so the
// escapes can never be cut in half.
pub fn fit(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    if limit <= 1 {
        return "…".repeat(limit);
    }
    let kept: String = text.chars().take(limit - 1).collect();
    format!("{kept}…")
}

#[cfg(unix)]
mod imp {
    use super::Key;
    use std::fs::{File, OpenOptions};
    use std::io::{self, Read, Write};
    use std::os::fd::AsRawFd;

    pub struct Screen {
        tty: File,
        saved: libc::termios,
        drawn: usize,
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

            // OPOST stays on, so a newline still returns the carriage and the
            // frames below need no \r of their own. ISIG goes off so ctrl-c
            // arrives as a byte and the terminal is restored on the way out.
            let mut raw = saved;
            raw.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN);
            raw.c_iflag &= !(libc::IXON | libc::ICRNL);
            raw.c_cc[libc::VMIN] = 1;
            raw.c_cc[libc::VTIME] = 0;
            // SAFETY: `raw` is the struct tcgetattr just filled in, with only
            // flag bits and control characters changed.
            if unsafe { libc::tcsetattr(fd, libc::TCSADRAIN, &raw) } != 0 {
                return Ok(None);
            }

            let mut screen = Screen {
                tty,
                saved,
                drawn: 0,
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

        pub fn clear(&mut self) -> io::Result<()> {
            if self.drawn == 0 {
                return Ok(());
            }
            let frame = format!("\x1b[{}F\x1b[0J", self.drawn);
            self.drawn = 0;
            self.put(&frame)
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
            let mut buffer = [0u8; 1];
            loop {
                return match self.tty.read(&mut buffer) {
                    Ok(0) => Ok(None),
                    Ok(_) => Ok(Some(buffer[0])),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => Err(error),
                };
            }
        }

        // An escape byte is either a key on its own or the start of a
        // sequence, and only the pause after it tells the two apart.
        fn waited(&mut self) -> io::Result<Option<u8>> {
            self.timeout(true);
            let byte = self.byte();
            self.timeout(false);
            byte
        }

        fn timeout(&mut self, brief: bool) {
            let fd = self.tty.as_raw_fd();
            // SAFETY: both calls take a descriptor this struct owns and a
            // termios they fill in or read, and both report their own failure.
            unsafe {
                let mut current: libc::termios = std::mem::zeroed();
                if libc::tcgetattr(fd, &mut current) != 0 {
                    return;
                }
                current.c_cc[libc::VMIN] = u8::from(!brief);
                current.c_cc[libc::VTIME] = u8::from(brief);
                libc::tcsetattr(fd, libc::TCSANOW, &current);
            }
        }

        fn put(&mut self, text: &str) -> io::Result<()> {
            self.tty.write_all(text.as_bytes())?;
            self.tty.flush()
        }
    }

    impl Drop for Screen {
        fn drop(&mut self) {
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
    use super::Key;
    use std::io;

    pub struct Screen;

    impl Screen {
        pub fn open() -> io::Result<Option<Screen>> {
            Ok(None)
        }
        pub fn draw(&mut self, _lines: &[String]) -> io::Result<()> {
            Ok(())
        }
        pub fn clear(&mut self) -> io::Result<()> {
            Ok(())
        }
        pub fn key(&mut self) -> io::Result<Key> {
            Ok(Key::Interrupt)
        }
    }
}

pub use imp::Screen;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_counts_what_is_printed_rather_than_what_is_sent() {
        assert_eq!(width("my-app"), 6);
        assert_eq!(width("\x1b[1mmy-app\x1b[0m"), 6);
        assert_eq!(width(""), 0);
        assert_eq!(width("\x1b[0m"), 0);
    }

    #[test]
    fn fit_keeps_short_text_whole() {
        assert_eq!(fit("my-app", 10), "my-app");
        assert_eq!(fit("my-app", 6), "my-app");
    }

    #[test]
    fn fit_marks_the_text_it_had_to_cut() {
        assert_eq!(fit("my-application", 6), "my-ap…");
        assert_eq!(fit("my-app", 1), "…");
        assert_eq!(fit("my-app", 0), "");
    }

    #[test]
    fn fit_measures_characters_rather_than_bytes() {
        assert_eq!(fit("émigré", 6), "émigré");
        assert_eq!(fit("émigré", 3), "ém…");
    }
}
