use std::sync::{Mutex, mpsc};
use std::thread;

use arboard::{Clipboard, Error};

use crate::PEER_READ;

static PASTEBOARD: Mutex<()> = Mutex::new(());

pub fn available() -> bool {
    true
}

pub fn read() -> Result<String, String> {
    match Clipboard::new().and_then(|mut clipboard| clipboard.get_text()) {
        Ok(text) => Ok(text),
        Err(Error::ContentNotAvailable) => Ok(String::new()),
        Err(error) => Err(error.to_string()),
    }
}

pub fn write(text: &str) -> Result<(), String> {
    Clipboard::new()
        .and_then(|mut clipboard| clipboard.set_text(text))
        .map_err(|error| error.to_string())
}

pub fn read_for_peer() -> Result<String, String> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _held = PASTEBOARD.lock();
        let _ = sender.send(read());
    });
    receiver
        .recv_timeout(PEER_READ)
        .unwrap_or_else(|_| Err("clipboard timed out".into()))
}
