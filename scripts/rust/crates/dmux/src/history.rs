//! Stable previous/current Space references for `dmux -` (plan §9.2, §17).
//!
//! History is keyed by SpaceUid — stable identity — never by mutable names
//! or row positions, so rename and re-list cannot retarget the toggle.
//! Client-side history lives under an explicit state dir
//! (`$XDG_STATE_HOME/dmux` in production, injected in tests). Writes are
//! atomic (same-directory temp file + rename) and, like the legacy
//! `state.rs` they replace, deliberately small: one JSON document.
//!
//! Legacy name-based entries convert to SpaceUids only when unambiguous;
//! ambiguous or missing names warn and drop (plan §17 step 11).

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::{HostUid, SpaceUid};

pub const HISTORY_FILE: &str = "history-v1.json";
pub const HISTORY_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct HostSlots {
    #[serde(skip_serializing_if = "Option::is_none")]
    current: Option<SpaceUid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous: Option<SpaceUid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileFormat {
    version: u32,
    /// Keyed by owner HostUid (lowercase hyphenated).
    hosts: BTreeMap<String, HostSlots>,
}

impl Default for FileFormat {
    fn default() -> Self {
        FileFormat {
            version: HISTORY_VERSION,
            hosts: BTreeMap::new(),
        }
    }
}

/// Previous/current Space history under one explicit state directory.
#[derive(Debug, Clone)]
pub struct History {
    dir: PathBuf,
}

impl History {
    /// `dir` is the state directory (the file lives at `dir/history-v1.json`).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        History { dir: dir.into() }
    }

    /// Production state dir: `$XDG_STATE_HOME/dmux`, falling back to
    /// `~/.local/state/dmux`.
    pub fn default_dir() -> Option<PathBuf> {
        if let Some(dir) = std::env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(dir).join("dmux"));
        }
        Some(PathBuf::from(std::env::var_os("HOME")?).join(".local/state/dmux"))
    }

    fn path(&self) -> PathBuf {
        self.dir.join(HISTORY_FILE)
    }

    fn read(&self) -> FileFormat {
        read_format(&self.path())
    }

    /// The toggle target for `dmux -` on `host`.
    pub fn previous(&self, host: HostUid) -> Option<SpaceUid> {
        self.read().hosts.get(&host.0.to_string())?.previous
    }

    /// The Space last attached on `host`.
    pub fn current(&self, host: HostUid) -> Option<SpaceUid> {
        self.read().hosts.get(&host.0.to_string())?.current
    }

    /// Record an attach: when the target differs from the recorded current,
    /// the old current becomes the toggle target. Reattaching the current
    /// Space moves nothing (same semantics the legacy state file had, but
    /// identity-stable).
    pub fn record_attach(&self, host: HostUid, space: SpaceUid) -> io::Result<()> {
        let mut format = self.read();
        let slots = format.hosts.entry(host.0.to_string()).or_default();
        match slots.current {
            Some(current) if current == space => return Ok(()),
            Some(current) => {
                slots.previous = Some(current);
                slots.current = Some(space);
            }
            None => slots.current = Some(space),
        }
        self.write(&format)
    }

    /// Same-directory temp file then rename: concurrent writers or a crash
    /// leave the old document or the new one, never a torn file.
    fn write(&self, format: &FileFormat) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let mut temp = tempfile::NamedTempFile::new_in(&self.dir)?;
        serde_json::to_writer_pretty(&mut temp, format)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        temp.write_all(b"\n")?;
        temp.persist(self.path()).map_err(|e| e.error)?;
        Ok(())
    }
}

fn read_format(path: &Path) -> FileFormat {
    let Ok(text) = fs::read_to_string(path) else {
        return FileFormat::default();
    };
    match serde_json::from_str::<FileFormat>(&text) {
        Ok(format) if format.version == HISTORY_VERSION => format,
        // Unknown version or corrupt content: history is best-effort state,
        // never authority — start fresh rather than misread.
        _ => FileFormat::default(),
    }
}

// ---------------------------------------------------------------------------
// Legacy conversion (plan §17 step 11)

/// One legacy `key session-name` line from the old `dmux/last` state file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyEntry {
    /// The legacy key (`host` or `host:current`).
    pub key: String,
    /// The session name the line pointed at.
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvertDropReason {
    /// No Space with that exact name exists on the host.
    Missing,
    /// More than one Space matches the name (cross-backend duplicate);
    /// converting would guess identity.
    Ambiguous { candidates: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvertWarning {
    pub key: String,
    pub name: String,
    pub reason: ConvertDropReason,
}

/// Pure conversion of legacy name-based history entries to SpaceUids.
///
/// `lookup` maps an exact session name to `(SpaceUid, match_count)` where
/// `match_count` is how many Spaces carry that name on the relevant host
/// (cross-backend duplicates make it > 1). Unambiguous entries convert;
/// ambiguous or missing entries are dropped with a returned warning —
/// never a guessed identity.
pub fn convert_legacy_entries(
    entries: &[LegacyEntry],
    lookup: impl Fn(&str) -> Option<(SpaceUid, u32)>,
) -> (Vec<(String, SpaceUid)>, Vec<ConvertWarning>) {
    let mut converted = Vec::new();
    let mut warnings = Vec::new();
    for entry in entries {
        match lookup(&entry.name) {
            Some((space, 1)) => converted.push((entry.key.clone(), space)),
            Some((_, candidates)) if candidates > 1 => warnings.push(ConvertWarning {
                key: entry.key.clone(),
                name: entry.name.clone(),
                reason: ConvertDropReason::Ambiguous { candidates },
            }),
            _ => warnings.push(ConvertWarning {
                key: entry.key.clone(),
                name: entry.name.clone(),
                reason: ConvertDropReason::Missing,
            }),
        }
    }
    (converted, warnings)
}
