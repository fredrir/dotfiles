//! Versioned conformance fixture records and the lex/parse/bootstrap
//! fixture runner.
//!
//! The record format is frozen by `contracts/dotfile/v1/fixtures.json`.
//! Pure syntax fixtures carry empty repository and machine state; the runner
//! decodes `input_bytes`, executes the named operation, and compares the
//! golden channels byte-for-byte or as exact JSON.

use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};

use dotfile_source::{RepoPath, SourceText, read_bootstrap};
use dotfile_syntax::{dump_tokens, lex, parse};
use serde_json::Value;

use crate::contract_directory;

/// One loaded fixture record.
#[derive(Clone, Debug)]
pub struct Fixture {
    pub id: String,
    pub status: String,
    pub domain: String,
    pub operation: String,
    pub input: Vec<u8>,
    pub expected: Value,
}

#[derive(Debug)]
pub enum FixtureError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Record { id: String, message: String },
}

impl Display for FixtureError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => Display::fmt(error, formatter),
            Self::Json(error) => Display::fmt(error, formatter),
            Self::Record { id, message } => write!(formatter, "fixture {id}: {message}"),
        }
    }
}

impl Error for FixtureError {}

impl From<std::io::Error> for FixtureError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for FixtureError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

fn record_error(id: &str, message: impl Into<String>) -> FixtureError {
    FixtureError::Record {
        id: id.to_owned(),
        message: message.into(),
    }
}

/// The directory holding the versioned fixture records.
pub fn fixture_directory() -> PathBuf {
    contract_directory().join("fixtures")
}

/// Loads every fixture record, sorted by unsigned UTF-8 bytes of id.
pub fn load_fixtures() -> Result<Vec<Fixture>, FixtureError> {
    let directory = fixture_directory();
    let paths: Vec<PathBuf> = fs::read_dir(&directory).map(|entries| {
        entries
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                if path
                    .extension()
                    .is_some_and(|extension| extension == "json")
                {
                    Some(path)
                } else {
                    None
                }
            })
            .collect()
    })?;
    let mut fixtures = paths
        .iter()
        .map(|path| load_fixture(path))
        .collect::<Result<Vec<_>, _>>()?;
    fixtures.sort_by(|left, right| left.id.as_bytes().cmp(right.id.as_bytes()));
    Ok(fixtures)
}

/// Loads and validates one fixture record.
pub fn load_fixture(path: &Path) -> Result<Fixture, FixtureError> {
    let bytes = fs::read(path)?;
    let value: Value = serde_json::from_slice(&bytes)?;
    let id = value["id"]
        .as_str()
        .ok_or_else(|| record_error("<unknown>", "missing id"))?
        .to_owned();
    let object = value
        .as_object()
        .ok_or_else(|| record_error(&id, "record is not an object"))?;
    const TOP_LEVEL: &[&str] = &[
        "id",
        "format_version",
        "status",
        "normative_references",
        "domain",
        "operation",
        "input_bytes",
        "repository_state",
        "machine_state",
        "expected",
    ];
    for key in object.keys() {
        if !TOP_LEVEL.contains(&key.as_str()) {
            return Err(record_error(&id, format!("unknown top-level field {key}")));
        }
    }
    for required in TOP_LEVEL {
        if !object.contains_key(*required) {
            return Err(record_error(&id, format!("missing field {required}")));
        }
    }
    if value["format_version"] != "1" {
        return Err(record_error(&id, "format_version is not \"1\""));
    }
    let status = value["status"].as_str().unwrap_or("").to_owned();
    if !["planned", "implemented", "passing", "failing", "blocked"].contains(&status.as_str()) {
        return Err(record_error(&id, format!("invalid status {status}")));
    }
    let domain = value["domain"]
        .as_str()
        .ok_or_else(|| record_error(&id, "domain is not a string"))?
        .to_owned();
    let operation = value["operation"]
        .as_str()
        .ok_or_else(|| record_error(&id, "operation is not a string"))?
        .to_owned();
    let input = decode_byte_string(&value["input_bytes"])
        .ok_or_else(|| record_error(&id, "invalid input_bytes"))?;
    let expected = value["expected"].clone();
    let expected_object = expected
        .as_object()
        .ok_or_else(|| record_error(&id, "expected is not an object"))?;
    const CHANNELS: &[&str] = &[
        "tokens",
        "cst",
        "diagnostics",
        "ir",
        "lock_text",
        "lock_jcs",
        "edits",
        "cli",
        "lsp",
        "tree_sitter",
        "filesystem_mutations",
    ];
    for key in expected_object.keys() {
        if !CHANNELS.contains(&key.as_str()) {
            return Err(record_error(&id, format!("unknown expected channel {key}")));
        }
    }
    for channel in CHANNELS {
        let record = expected_object
            .get(*channel)
            .ok_or_else(|| record_error(&id, format!("missing expected channel {channel}")))?;
        let record = record
            .as_object()
            .ok_or_else(|| record_error(&id, format!("channel {channel} is not an object")))?;
        if record.len() != 3
            || !record.contains_key("state")
            || !record.contains_key("comparison")
            || !record.contains_key("payload")
        {
            return Err(record_error(
                &id,
                format!("channel {channel} must have exactly state, comparison, payload"),
            ));
        }
    }
    Ok(Fixture {
        id,
        status,
        domain,
        operation,
        input,
        expected,
    })
}

/// Decodes a contract byte string: `{ "encoding": "utf8" | "base64" | "hex",
/// "value": ... }` decoded exactly, without newline or Unicode
/// normalization.
pub fn decode_byte_string(record: &Value) -> Option<Vec<u8>> {
    let object = record.as_object()?;
    if object.len() != 2 {
        return None;
    }
    let encoding = object.get("encoding")?.as_str()?;
    let value = object.get("value")?.as_str()?;
    match encoding {
        "utf8" => Some(value.as_bytes().to_vec()),
        "hex" => decode_hex(value),
        "base64" => decode_base64(value),
        _ => None,
    }
}

/// Encodes a byte string using the readable-when-possible representation:
/// valid UTF-8 text uses `utf8`, anything else uses `base64`.
pub fn encode_byte_string(bytes: &[u8]) -> Value {
    if let Ok(text) = std::str::from_utf8(bytes) {
        serde_json::json!({ "encoding": "utf8", "value": text })
    } else {
        serde_json::json!({ "encoding": "base64", "value": encode_base64(bytes) })
    }
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(value.get(index..index + 2)?, 16).ok())
        .collect()
}

fn decode_base64(value: &str) -> Option<Vec<u8>> {
    let bytes = value.as_bytes();
    if bytes.len() % 4 != 0 {
        return None;
    }
    let mut output = Vec::with_capacity(bytes.len() / 4 * 3);
    let mut padded = 0;
    for (index, chunk) in bytes.chunks(4).enumerate() {
        let mut accumulator = 0u32;
        for (position, byte) in chunk.iter().enumerate() {
            let digit = match byte {
                b'A'..=b'Z' => u32::from(byte - b'A'),
                b'a'..=b'z' => u32::from(byte - b'a') + 26,
                b'0'..=b'9' => u32::from(byte - b'0') + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' => {
                    if index == bytes.len() / 4 - 1 && position >= 2 {
                        padded += 1;
                        0
                    } else {
                        return None;
                    }
                }
                _ => return None,
            };
            accumulator = (accumulator << 6) | digit;
        }
        output.push((accumulator >> 16) as u8);
        if padded < 2 {
            output.push((accumulator >> 8) as u8);
        }
        if padded < 1 {
            output.push(accumulator as u8);
        }
    }
    if padded > 2 {
        return None;
    }
    Some(output)
}

fn encode_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::new();
    for chunk in bytes.chunks(3) {
        let mut accumulator = 0u32;
        for (index, byte) in chunk.iter().enumerate() {
            accumulator |= u32::from(*byte) << (16 - index * 8);
        }
        for position in 0..4 {
            let beyond = position > chunk.len();
            if beyond {
                output.push('=');
            } else {
                output.push(ALPHABET[(accumulator >> (18 - position * 6)) as usize & 0x3f] as char);
            }
        }
    }
    output
}

/// One channel mismatch or fixture-level failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixtureFailure {
    pub id: String,
    pub channel: String,
    pub message: String,
}

impl Display for FixtureFailure {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "fixture {} channel {}: {}",
            self.id, self.channel, self.message
        )
    }
}

/// Runs one fixture and returns every channel mismatch.
pub fn run_fixture(fixture: &Fixture) -> Vec<FixtureFailure> {
    let path = fixture_path(&fixture.domain);
    let source = SourceText::from_bytes(fixture.input.clone());
    let mut failures = Vec::new();
    match fixture.operation.as_str() {
        "lex" => {
            let lexed = lex(&path, &source);
            compare_bytes(
                fixture,
                &mut failures,
                "tokens",
                lexed.dump(source.as_bytes()).as_bytes(),
            );
            compare_diagnostics(fixture, &mut failures, &lexed.diagnostics);
        }
        "parse" => {
            let result = parse(&path, &source);
            compare_bytes(
                fixture,
                &mut failures,
                "tokens",
                dump_tokens(
                    &result.cst.tokens,
                    &result.cst.gaps,
                    &result.cst.strings,
                    source.as_bytes(),
                )
                .as_bytes(),
            );
            compare_bytes(
                fixture,
                &mut failures,
                "cst",
                result.cst.dump(source.as_bytes()).as_bytes(),
            );
            compare_diagnostics(fixture, &mut failures, &result.diagnostics);
        }
        "bootstrap" => {
            let diagnostics = match read_bootstrap(&path, &source) {
                Ok(_) => Vec::new(),
                Err(errors) => errors,
            };
            compare_diagnostics(fixture, &mut failures, &diagnostics);
        }
        other => failures.push(FixtureFailure {
            id: fixture.id.clone(),
            channel: "operation".to_owned(),
            message: format!("unknown operation {other}"),
        }),
    }
    failures
}

/// The canonical path a fixture domain parses as: the profiles domain uses
/// the bootstrap path; every other domain uses one neutral repository path.
fn fixture_path(domain: &str) -> RepoPath {
    let path = if domain == "profiles" {
        dotfile_source::PROFILES_PATH
    } else {
        "fixture.dotfile"
    };
    RepoPath::new(path).unwrap()
}

fn channel<'a>(fixture: &'a Fixture, name: &str) -> &'a Value {
    &fixture.expected[name]
}

fn compare_bytes(fixture: &Fixture, failures: &mut Vec<FixtureFailure>, name: &str, actual: &[u8]) {
    let channel = channel(fixture, name);
    match channel["state"].as_str() {
        Some("golden") => {
            let Some(expected) = decode_byte_string(&channel["payload"]) else {
                failures.push(FixtureFailure {
                    id: fixture.id.clone(),
                    channel: name.to_owned(),
                    message: "golden payload is not a valid byte string".to_owned(),
                });
                return;
            };
            if expected != actual {
                failures.push(FixtureFailure {
                    id: fixture.id.clone(),
                    channel: name.to_owned(),
                    message: format!(
                        "golden mismatch\n--- expected ---\n{}\n--- actual ---\n{}",
                        String::from_utf8_lossy(&expected),
                        String::from_utf8_lossy(actual),
                    ),
                });
            }
        }
        Some("empty") => {
            if !actual.is_empty() {
                failures.push(FixtureFailure {
                    id: fixture.id.clone(),
                    channel: name.to_owned(),
                    message: "expected an empty result".to_owned(),
                });
            }
        }
        Some("not_applicable") => {}
        _ => {}
    }
}

fn compare_diagnostics(
    fixture: &Fixture,
    failures: &mut Vec<FixtureFailure>,
    actual: &[dotfile_source::Diagnostic],
) {
    let channel = channel(fixture, "diagnostics");
    let actual_json = serde_json::to_value(actual).unwrap_or(Value::Null);
    match channel["state"].as_str() {
        Some("golden") => {
            if channel["payload"] != actual_json {
                failures.push(FixtureFailure {
                    id: fixture.id.clone(),
                    channel: "diagnostics".to_owned(),
                    message: format!(
                        "golden mismatch\n--- expected ---\n{}\n--- actual ---\n{}",
                        pretty(&channel["payload"]),
                        pretty(&actual_json),
                    ),
                });
            }
        }
        Some("empty") => {
            if !actual.is_empty() {
                failures.push(FixtureFailure {
                    id: fixture.id.clone(),
                    channel: "diagnostics".to_owned(),
                    message: format!("expected no diagnostics, got {}", pretty(&actual_json)),
                });
            }
        }
        Some("not_applicable") => {}
        _ => {}
    }
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_string_codecs() {
        assert_eq!(decode_hex("00ff10"), Some(vec![0x00, 0xff, 0x10]));
        assert_eq!(decode_hex("0"), None);
        assert_eq!(decode_hex("zz"), None);

        assert_eq!(decode_base64("aGVsbG8="), Some(b"hello".to_vec()));
        assert_eq!(decode_base64("aGVsbG8"), None);
        assert_eq!(decode_base64("AAAA"), Some(vec![0, 0, 0]));
        assert_eq!(decode_base64("A==="), None);
        for case in [
            &b""[..],
            b"f",
            b"fo",
            b"foo",
            b"foob",
            b"fooba",
            b"foobar",
            &[0xff, 0xfe, 0xfd],
        ] {
            assert_eq!(decode_base64(&encode_base64(case)), Some(case.to_vec()));
        }

        let encoded = encode_byte_string(b"plain text");
        assert_eq!(encoded["encoding"], "utf8");
        let encoded = encode_byte_string(&[0xff, 0x00]);
        assert_eq!(encoded["encoding"], "base64");
        assert_eq!(decode_byte_string(&encoded), Some(vec![0xff, 0x00]));
    }
}
