#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Decision {
    Accept,
    Abort,
}

#[cfg(unix)]
pub(super) use terminal::Session;

#[cfg(unix)]
mod terminal {
    use super::{Decision, escaped};
    use nix::sys::select::{FD_SETSIZE, FdSet, select};
    use nix::sys::signal::{SigSet, SigmaskHow, Signal, pthread_sigmask};
    use nix::sys::termios::{
        self, InputFlags, LocalFlags, SetArg, SpecialCharacterIndices, Termios,
    };
    use nix::sys::time::TimeVal;
    use nix::unistd::{getpgrp, tcgetpgrp};
    use std::fs::{File, OpenOptions};
    use std::io::{self, IsTerminal, Read, Write};
    use std::os::fd::{AsFd, AsRawFd};
    use std::os::unix::fs::OpenOptionsExt;

    pub(in super::super) struct Session {
        terminal: File,
        original: Termios,
    }

    impl Session {
        pub(in super::super) fn open() -> Result<Option<Self>, String> {
            if std::env::var_os("CI").is_some() || !io::stderr().is_terminal() {
                return Ok(None);
            }
            let Ok(terminal) = OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
                .open("/dev/tty")
            else {
                return Ok(None);
            };
            if !usize::try_from(terminal.as_raw_fd()).is_ok_and(|fd| fd < FD_SETSIZE)
                || tcgetpgrp(&terminal).ok() != Some(getpgrp())
            {
                return Ok(None);
            }
            let Ok(original) = termios::tcgetattr(&terminal) else {
                return Ok(None);
            };
            let mut raw = original.clone();
            raw.local_flags.remove(
                LocalFlags::ICANON | LocalFlags::ECHO | LocalFlags::ECHONL | LocalFlags::IEXTEN,
            );
            raw.local_flags.insert(LocalFlags::ISIG);
            raw.input_flags
                .remove(InputFlags::IXON | InputFlags::ICRNL | InputFlags::IGNCR);
            raw.control_chars[SpecialCharacterIndices::VMIN as usize] = 1;
            raw.control_chars[SpecialCharacterIndices::VTIME as usize] = 0;
            raw.control_chars[SpecialCharacterIndices::VINTR as usize] = 3;
            raw.control_chars[SpecialCharacterIndices::VQUIT as usize] = libc::_POSIX_VDISABLE;
            raw.control_chars[SpecialCharacterIndices::VSUSP as usize] = libc::_POSIX_VDISABLE;
            let session = Self { terminal, original };
            if set_attributes(&session.terminal, &raw).is_err() {
                return Ok(None);
            }
            Ok(Some(session))
        }

        /// Position is one-based; inspection supplies only redacted context.
        pub(in super::super) fn choose(
            &mut self,
            title: &str,
            position: usize,
            total: usize,
            can_accept: bool,
            mut inspection: impl FnMut() -> Result<String, String>,
        ) -> Result<Decision, String> {
            self.put(&format!(
                "\nSecret scan {position}/{total}\n{}\n",
                escaped(title)
            ))?;
            if !can_accept {
                self.put("This finding must be fixed.\n")?;
            }
            loop {
                self.put(if can_accept {
                    "[i] Inspect  [a] Accept this content  [q] Abort (default): "
                } else {
                    "[i] Inspect  [q] Abort (default): "
                })?;
                match self.key()?.map(|key| key.to_ascii_lowercase()) {
                    Some(b'i') => {
                        self.put("i\n\nInspection\n")?;
                        let context = inspection()?;
                        self.put(&escaped(&context))?;
                        self.put("\n\n")?;
                    }
                    Some(b'a') if can_accept => {
                        self.put("a\n")?;
                        return Ok(Decision::Accept);
                    }
                    Some(3) => {
                        let _ = self.terminal.write_all(b"^C\n");
                        crate::cancel::request_signal(libc::SIGINT);
                        return Ok(Decision::Abort);
                    }
                    Some(b'q' | b'\r' | b'\n' | 4 | 27) => {
                        self.put("\n")?;
                        return Ok(Decision::Abort);
                    }
                    None => return Ok(Decision::Abort),
                    _ => self.put("\n")?,
                }
            }
        }

        fn ready(&self, writing: bool) -> Result<bool, String> {
            loop {
                if crate::cancel::requested() || tcgetpgrp(&self.terminal).ok() != Some(getpgrp()) {
                    return Ok(false);
                }
                let mut readable = FdSet::new();
                let mut writable = FdSet::new();
                if writing {
                    writable.insert(self.terminal.as_fd());
                } else {
                    readable.insert(self.terminal.as_fd());
                }
                let mut timeout = TimeVal::new(0, 100_000);
                match select(None, &mut readable, &mut writable, None, &mut timeout) {
                    Ok(count) if count > 0 => return Ok(true),
                    Ok(_) => {}
                    Err(nix::errno::Errno::EINTR) => {}
                    Err(error) => return Err(format!("review terminal: {error}")),
                }
            }
        }

        fn key(&mut self) -> Result<Option<u8>, String> {
            let mut byte = [0];
            while self.ready(false)? {
                match self.terminal.read(&mut byte) {
                    Ok(0) => return Ok(None),
                    Ok(_) => return Ok(Some(byte[0])),
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                        ) => {}
                    Err(error) if error.raw_os_error() == Some(libc::EIO) => return Ok(None),
                    Err(error) => return Err(format!("read review terminal: {error}")),
                }
            }
            Ok(None)
        }

        fn put(&mut self, text: &str) -> Result<(), String> {
            let mut bytes = text.as_bytes();
            while !bytes.is_empty() {
                if !self.ready(true)? {
                    return Err("review terminal unavailable".into());
                }
                match self.terminal.write(bytes) {
                    Ok(0) => return Err("review terminal closed".into()),
                    Ok(count) => bytes = &bytes[count..],
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                        ) => {}
                    Err(error) => return Err(format!("write review terminal: {error}")),
                }
            }
            Ok(())
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            let _ = set_attributes(&self.terminal, &self.original);
        }
    }

    fn set_attributes(terminal: &File, attributes: &Termios) -> Result<(), nix::errno::Errno> {
        let mut previous = SigSet::empty();
        pthread_sigmask(
            SigmaskHow::SIG_BLOCK,
            Some(&SigSet::from(Signal::SIGTTOU)),
            Some(&mut previous),
        )?;
        struct Restore(SigSet);
        impl Drop for Restore {
            fn drop(&mut self) {
                let _ = self.0.thread_set_mask();
            }
        }
        let _restore = Restore(previous);
        termios::tcsetattr(terminal, SetArg::TCSANOW, attributes)
    }
}

#[cfg(not(unix))]
pub(super) struct Session;

#[cfg(not(unix))]
impl Session {
    pub(super) fn open() -> Result<Option<Self>, String> {
        Ok(None)
    }
    pub(super) fn choose(
        &mut self,
        _title: &str,
        _position: usize,
        _total: usize,
        _can_accept: bool,
        _inspection: impl FnMut() -> Result<String, String>,
    ) -> Result<Decision, String> {
        Ok(Decision::Abort)
    }
}

fn escaped(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for character in text.chars() {
        if (character.is_control() && character != '\n')
            || matches!(character, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            output.extend(character.escape_default());
        } else {
            output.push(character);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::escaped;

    #[test]
    fn terminal_text_preserves_lines_and_escapes_control_sequences() {
        assert_eq!(
            escaped("path\n\x1b[2J\u{202e}name"),
            "path\n\\u{1b}[2J\\u{202e}name"
        );
        assert_eq!(
            escaped("one\ntwo\tcolumn\rreturn"),
            "one\ntwo\\tcolumn\\rreturn"
        );
        assert_eq!(escaped("Norsk æøå 日本語"), "Norsk æøå 日本語");
    }
}
