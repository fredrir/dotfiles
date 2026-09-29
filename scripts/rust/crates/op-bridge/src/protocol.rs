use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

const MAX_LINE: u64 = 64 * 1024;
const MAX_REFERENCE: usize = 512;

#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub read: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Response {
    Value(Zeroizing<String>),
    // Touch ID was declined; the caller must not retry elsewhere
    Denied(String),
    // Outside the allowlist or op failed on macie; the caller falls back to the real op
    Refused(String),
}

pub fn vault(reference: &str) -> Option<&str> {
    let (vault, path) = reference.strip_prefix("op://")?.split_once('/')?;
    (!vault.is_empty() && !path.is_empty()).then_some(vault)
}

pub fn validate(reference: &str) -> Result<&str, String> {
    if reference.len() > MAX_REFERENCE || reference.chars().any(char::is_control) {
        return Err("malformed secret reference".to_string());
    }
    vault(reference).ok_or_else(|| format!("not a secret reference: {reference}"))
}

pub fn send<T: Serialize>(mut stream: &UnixStream, message: &T) -> Result<(), String> {
    let mut line = Zeroizing::new(serde_json::to_vec(message).map_err(|error| error.to_string())?);
    line.push(b'\n');
    stream
        .write_all(&line)
        .map_err(|error| format!("send: {error}"))
}

pub fn receive<T: DeserializeOwned>(stream: &UnixStream) -> Result<T, String> {
    let mut line = Zeroizing::new(Vec::new());
    BufReader::new(stream.take(MAX_LINE))
        .read_until(b'\n', &mut line)
        .map_err(|error| format!("receive: {error}"))?;
    if line.last() != Some(&b'\n') {
        return Err("receive: connection closed mid-message".to_string());
    }
    serde_json::from_slice(&line).map_err(|error| format!("receive: {error}"))
}

#[cfg(test)]
#[path = "../tests/unit/protocol_tests.rs"]
mod tests;
