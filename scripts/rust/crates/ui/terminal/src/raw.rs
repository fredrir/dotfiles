use std::io;
use std::io::Write;

use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

pub struct RawMode {
    enabled: bool,
}

impl RawMode {
    pub fn enable() -> io::Result<Self> {
        enable_raw_mode()?;
        Ok(Self { enabled: true })
    }

    #[cfg(any(feature = "ratatui", test))]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn disable(&mut self) -> io::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        self.enabled = false;
        disable_raw_mode()
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = self.disable();
    }
}

pub struct RawSession {
    raw: RawMode,
    _signals: crate::SignalGuard,
}

impl RawSession {
    pub fn new() -> io::Result<Self> {
        let signals = crate::SignalGuard::new()?;
        let raw = RawMode::enable()?;
        let session = Self {
            raw,
            _signals: signals,
        };
        let mut out = io::stdout();
        crossterm::execute!(out, crossterm::cursor::Hide)?;
        out.flush()?;
        Ok(session)
    }
}

impl Drop for RawSession {
    fn drop(&mut self) {
        let mut out = io::stdout();
        let _ = crossterm::execute!(out, crossterm::cursor::Show);
        let _ = out.flush();
        let _ = self.raw.disable();
    }
}

#[cfg(test)]
#[path = "../tests/unit/raw_tests.rs"]
mod tests;
