use std::sync::OnceLock;

use arboard::{Clipboard, Error};

use super::reader::Reader;
use crate::PEER_READ;

const QUEUE: usize = 4;

static READER: OnceLock<Reader> = OnceLock::new();

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
    READER
        .get_or_init(|| Reader::spawn(read, QUEUE))
        .ask(PEER_READ)
}
