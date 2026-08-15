//! Where `dmux -` finds its way back.
//!
//! One small file of `host session` lines: for each host, the session that
//! was current the last time a con/new left it. Written before the exec —
//! there is no after — and always best-effort, because a failed state write
//! must never block an attach.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use crate::hosts::Host;

pub fn file() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(dir).join("dmux/last"));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".local/state/dmux/last"))
}

pub fn previous(host: Host) -> Option<String> {
    let text = fs::read_to_string(file()?).ok()?;
    parse(&text).remove(host.name())
}

pub fn record(host: Host, session: &str) {
    let Some(path) = file() else {
        return;
    };
    let mut entries = fs::read_to_string(&path)
        .map(|text| parse(&text))
        .unwrap_or_default();
    entries.insert(host.name().to_string(), session.to_string());
    let text: String = entries
        .iter()
        .map(|(host, session)| format!("{host} {session}\n"))
        .collect();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, text);
}

fn parse(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(host, session)| (host.to_string(), session.to_string()))
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
}
