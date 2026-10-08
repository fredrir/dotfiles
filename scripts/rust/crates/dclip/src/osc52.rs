use std::fs::OpenOptions;
use std::io::Write;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

pub fn sequence(text: &str) -> String {
    format!("\x1b]52;c;{}\x1b\\", STANDARD.encode(text))
}

pub fn copy(text: &str) -> Result<(), String> {
    let mut tty = OpenOptions::new()
        .write(true)
        .open("/dev/tty")
        .map_err(|_| "no clipboard and no tty".to_string())?;
    tty.write_all(sequence(text).as_bytes())
        .and_then(|()| tty.flush())
        .map_err(|error| format!("/dev/tty: {error}"))
}

#[cfg(test)]
#[path = "../tests/unit/osc52_tests.rs"]
mod tests;
