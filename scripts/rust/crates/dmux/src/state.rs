//! Where `dmux -` finds its way back.
//!
//! One small file of `key session` lines. For each host the plain `host` key
//! holds the toggle target: on this machine, the session that was current
//! the last time a con/new left it. A peer cannot be asked what is being
//! left, so remote attaches keep a second `host:current` line — the session
//! dmux attached there last — and shift it into the toggle slot whenever a
//! later attach goes somewhere else. Written before the exec — there is no
//! after — and always best-effort, because a failed state write must never
//! block an attach.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::hosts::Host;

pub fn file() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(dir).join("dmux/last"));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".local/state/dmux/last"))
}

pub fn previous(host: Host) -> Option<String> {
    previous_at(&file()?, host)
}

pub fn record(host: Host, session: &str) {
    if let Some(path) = file() {
        record_at(&path, host.name(), session);
    }
}

/// An attach to a remote host: track what dmux attached there, and when the
/// target moves, the old current becomes the toggle target.
pub fn record_attach(host: Host, session: &str) {
    if let Some(path) = file() {
        record_attach_at(&path, host, session);
    }
}

fn previous_at(path: &Path, host: Host) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    parse(&text).remove(host.name())
}

fn record_at(path: &Path, key: &str, session: &str) {
    let mut entries = read(path);
    entries.insert(key.to_string(), session.to_string());
    write(path, &entries);
}

fn record_attach_at(path: &Path, host: Host, session: &str) {
    let mut entries = read(path);
    let current_key = format!("{}:current", host.name());
    match entries.get(&current_key) {
        Some(current) if current == session => return,
        Some(current) => {
            let current = current.clone();
            entries.insert(host.name().to_string(), current);
        }
        None => {}
    }
    entries.insert(current_key, session.to_string());
    write(path, &entries);
}

fn read(path: &Path) -> BTreeMap<String, String> {
    fs::read_to_string(path)
        .map(|text| parse(&text))
        .unwrap_or_default()
}

/// Temp file in the same directory, then rename: two dmux processes racing
/// (or one dying mid-write) leave the old file or the new one, never a torn
/// line. Still best-effort — any failure just loses one toggle record.
fn write(path: &Path, entries: &BTreeMap<String, String>) {
    let text: String = entries
        .iter()
        .map(|(key, session)| format!("{key} {session}\n"))
        .collect();
    let Some(parent) = path.parent() else { return };
    let _ = fs::create_dir_all(parent);
    let Ok(mut temp) = tempfile::NamedTempFile::new_in(parent) else {
        return;
    };
    if temp.write_all(text.as_bytes()).is_ok() {
        let _ = temp.persist(path);
    }
}

/// Keys cannot contain a space — `host`, or `host:current` — while a session
/// name (the rest of the line) can contain anything but a newline.
fn parse(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(key, session)| (key.to_string(), session.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_one_entry_per_host() {
        let entries = parse("macie main\narchie dev\n\nnoise\n");
        assert_eq!(entries.get("macie").map(String::as_str), Some("main"));
        assert_eq!(entries.get("archie").map(String::as_str), Some("dev"));
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn a_later_line_wins() {
        let entries = parse("macie old\nmacie new\n");
        assert_eq!(entries.get("macie").map(String::as_str), Some("new"));
    }

    #[test]
    fn a_remote_attach_shifts_current_into_previous() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("last");
        record_attach_at(&path, Host::Archie, "a");
        assert_eq!(previous_at(&path, Host::Archie), None);
        record_attach_at(&path, Host::Archie, "b");
        assert_eq!(previous_at(&path, Host::Archie).as_deref(), Some("a"));
        // Reattaching the current session moves nothing.
        record_attach_at(&path, Host::Archie, "b");
        assert_eq!(previous_at(&path, Host::Archie).as_deref(), Some("a"));
        // The toggle round-trip: going back makes the old current previous.
        record_attach_at(&path, Host::Archie, "a");
        assert_eq!(previous_at(&path, Host::Archie).as_deref(), Some("b"));
    }

    #[test]
    fn remote_state_leaves_the_local_slot_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("last");
        record_at(&path, Host::Macie.name(), "local");
        record_attach_at(&path, Host::Archie, "x");
        record_attach_at(&path, Host::Archie, "y");
        assert_eq!(previous_at(&path, Host::Macie).as_deref(), Some("local"));
        assert_eq!(previous_at(&path, Host::Archie).as_deref(), Some("x"));
    }
}
