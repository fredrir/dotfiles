use std::io;

use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

pub struct RawMode {
    enabled: bool,
}

impl RawMode {
    pub fn enable() -> io::Result<Self> {
        enable_raw_mode()?;
        Ok(Self { enabled: true })
    }

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

#[cfg(test)]
#[path = "../tests/unit/raw_tests.rs"]
mod tests;
