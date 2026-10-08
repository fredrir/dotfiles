use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cache;
use crate::context::{Context, fingerprint, secs};
use crate::pi::resources::Settings;
use crate::pi::spec::Pi;

const FIRST_MESSAGE_SCAN: usize = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub cwd: String,
    pub name: Option<String>,
    pub first_message: Option<String>,
    pub modified: u64,
}

// The directory pi lists for this working directory, and whether it holds other projects too.
pub fn directory(ctx: &Context, pi: &Pi, flag: Option<&str>) -> (PathBuf, bool) {
    let custom = flag
        .map(|dir| ctx.expand(dir))
        .or_else(|| ctx.var_path("PI_CODING_AGENT_SESSION_DIR"))
        .or_else(|| {
            Settings::load(ctx, pi)
                .session_dir
                .map(|dir| ctx.expand(&dir))
        });
    let default = default_directory(&pi.agent_dir, &ctx.cwd);
    match custom {
        Some(dir) => {
            let shared = dir != default;
            (dir, shared)
        }
        None => (default, false),
    }
}

pub fn default_directory(agent_dir: &Path, cwd: &Path) -> PathBuf {
    let cwd = cwd.to_string_lossy();
    let encoded: String = cwd
        .trim_start_matches(['/', '\\'])
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') {
                '-'
            } else {
                c
            }
        })
        .collect();
    agent_dir.join("sessions").join(format!("--{encoded}--"))
}

// Sessions for this working directory, newest first.
pub fn list(ctx: &Context, dir: &Path, filter_cwd: bool) -> Vec<Session> {
    let key = format!("pi-sessions-{}", dir.display());
    let mut known: HashMap<String, (String, Session)> = cache::peek(ctx, &key).unwrap_or_default();
    let mut changed = false;
    let mut sessions = Vec::new();
    let mut present = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return sessions;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .is_none_or(|extension| extension != "jsonl")
        {
            continue;
        }
        let file = path.to_string_lossy().to_string();
        let stamp = fingerprint(&path);
        present.push(file.clone());
        let session = match known.get(&file) {
            Some((known_stamp, session)) if *known_stamp == stamp => Some(session.clone()),
            _ => {
                let session = summarize(&path);
                if let Some(session) = &session {
                    known.insert(file, (stamp, session.clone()));
                    changed = true;
                }
                session
            }
        };
        if let Some(session) = session
            && (!filter_cwd || Path::new(&session.cwd) == ctx.cwd)
        {
            sessions.push(session);
        }
    }
    let before = known.len();
    known.retain(|file, _| present.contains(file));
    if changed || known.len() != before {
        cache::store(ctx, &key, &known);
    }
    sessions.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.id.cmp(&b.id)));
    sessions
}

pub fn summarize(path: &Path) -> Option<Session> {
    let bytes = fs::read(path).ok()?;
    let modified = fs::metadata(path)
        .and_then(|meta| meta.modified())
        .map(secs)
        .unwrap_or(0);
    let header_end = memchr::memchr(b'\n', &bytes).unwrap_or(bytes.len());
    let header: Value = serde_json::from_slice(&bytes[..header_end]).ok()?;
    if header.get("type").and_then(Value::as_str) != Some("session") {
        return None;
    }
    let text = |value: &Value, key: &str| value.get(key).and_then(Value::as_str).map(String::from);
    Some(Session {
        id: text(&header, "id")?,
        cwd: text(&header, "cwd").unwrap_or_default(),
        name: latest_name(&bytes),
        first_message: first_user_message(&bytes[..bytes.len().min(FIRST_MESSAGE_SCAN)]),
        modified,
    })
}

fn latest_name(bytes: &[u8]) -> Option<String> {
    let mut name = None;
    for at in memchr::memmem::find_iter(bytes, b"\"session_info\"") {
        let start = memchr::memrchr(b'\n', &bytes[..at]).map_or(0, |newline| newline + 1);
        let end = memchr::memchr(b'\n', &bytes[at..]).map_or(bytes.len(), |newline| at + newline);
        let Ok(entry) = serde_json::from_slice::<Value>(&bytes[start..end]) else {
            continue;
        };
        if entry.get("type").and_then(Value::as_str) == Some("session_info") {
            name = entry
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(String::from);
        }
    }
    name
}

fn first_user_message(bytes: &[u8]) -> Option<String> {
    for line in bytes.split(|byte| *byte == b'\n') {
        if memchr::memmem::find(line, b"\"role\":\"user\"").is_none() {
            continue;
        }
        let Ok(entry) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        let Some(content) = entry.pointer("/message/content") else {
            continue;
        };
        let text = match content {
            Value::String(text) => text.clone(),
            Value::Array(parts) => parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(" "),
            _ => continue,
        };
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if !text.is_empty() {
            return Some(text);
        }
    }
    None
}

#[cfg(test)]
#[path = "../../tests/unit/pi/sessions_tests.rs"]
mod tests;
