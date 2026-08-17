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
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::bootstrap::MarkerContext;
use crate::model::{BackendInstanceUid, ChildKind, HostUid, SpaceUid};

pub const HISTORY_FILE: &str = "history-v1.json";
const HISTORY_LOCK_FILE: &str = "history-v1.lock";
pub const HISTORY_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct HostSlots {
    #[serde(skip_serializing_if = "Option::is_none")]
    current: Option<SpaceUid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous: Option<SpaceUid>,
}

/// The most recently presented Space across all owners. GUI summon needs a
/// single stable identity; per-host previous/current slots cannot establish
/// cross-host recency without guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuiHistoryTarget {
    pub host_uid: HostUid,
    pub space_uid: SpaceUid,
}

/// Short-lived source/destination proof staged before a terminal `exec`.
/// It does not change GUI current/previous until the destination marker is
/// observed in the exact same GUI process/pane with `tmux_client_uid`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingGuiTransition {
    pub tmux_client_uid: Uuid,
    pub source: GuiHistoryTarget,
    pub destination: GuiHistoryTarget,
    pub destination_backend_instance_uid: BackendInstanceUid,
    /// Full owner-resolved destination marker, including SpaceNo and exact
    /// identity. `destination_child_kind` controls the historical subset
    /// semantics: a Split compares Group+Split, a Group compares Group and
    /// accepts its owner-validated active Split, and no child compares only
    /// the Space identity.
    pub destination_marker: MarkerContext,
    pub destination_child_kind: Option<ChildKind>,
    pub gui_instance: String,
    pub gui_pid: u32,
    pub gui_process_start_token: String,
    pub gui_pane_id: u64,
    pub gui_domain: String,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileFormat {
    version: u32,
    /// Updated only after an acknowledged GUI presentation. Older v1 files
    /// omit it and remain valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    gui_current: Option<GuiHistoryTarget>,
    /// Previous distinct globally acknowledged GUI presentation. Added
    /// compatibly within v1; older files omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    gui_previous: Option<GuiHistoryTarget>,
    /// Pending exec transitions keyed by canonical tmux client UUID. Older
    /// v1 files omit this compatible best-effort field.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pending_gui: BTreeMap<String, PendingGuiTransition>,
    /// Keyed by owner HostUid (lowercase hyphenated).
    hosts: BTreeMap<String, HostSlots>,
}

impl Default for FileFormat {
    fn default() -> Self {
        FileFormat {
            version: HISTORY_VERSION,
            gui_current: None,
            gui_previous: None,
            pending_gui: BTreeMap::new(),
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

    /// Serialize every read-modify-write transaction.  The file is opened
    /// without following a final symlink and its inode is validated after
    /// open, before `flock`, so an attacker cannot redirect or weaken the
    /// coordination primitive.
    fn mutation_lock(&self) -> io::Result<MutationLock> {
        fs::create_dir_all(&self.dir)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(self.dir.join(HISTORY_LOCK_FILE))?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "history lock must be a private regular file owned by the current user",
            ));
        }
        loop {
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
                break;
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
        Ok(MutationLock { file })
    }

    /// The toggle target for `dmux -` on `host`.
    pub fn previous(&self, host: HostUid) -> Option<SpaceUid> {
        self.read().hosts.get(&host.0.to_string())?.previous
    }

    /// The Space last attached on `host`.
    pub fn current(&self, host: HostUid) -> Option<SpaceUid> {
        self.read().hosts.get(&host.0.to_string())?.current
    }

    /// Last Space whose GUI presentation received a valid acknowledgement,
    /// across every owner. The caller must still authority/live revalidate
    /// this locator before using it.
    pub fn last_gui_presented(&self) -> Option<GuiHistoryTarget> {
        self.read().gui_current
    }

    /// Previous distinct acknowledged GUI presentation across all owners.
    pub fn previous_gui_presented(&self) -> Option<GuiHistoryTarget> {
        self.read().gui_previous
    }

    /// Record an attach: when the target differs from the recorded current,
    /// the old current becomes the toggle target. Reattaching the current
    /// Space moves nothing (same semantics the legacy state file had, but
    /// identity-stable).
    pub fn record_attach(&self, host: HostUid, space: SpaceUid) -> io::Result<()> {
        let _lock = self.mutation_lock()?;
        let mut format = self.read();
        if !update_host_slots(&mut format, host, space) {
            return Ok(());
        }
        self.write(&format)
    }

    /// Record an acknowledged GUI presentation and its per-owner toggle
    /// ordering in one atomic file replacement.
    pub fn record_gui_present(&self, host: HostUid, space: SpaceUid) -> io::Result<()> {
        let _lock = self.mutation_lock()?;
        let mut format = self.read();
        let host_changed = update_host_slots(&mut format, host, space);
        let target = GuiHistoryTarget {
            host_uid: host,
            space_uid: space,
        };
        let gui_changed = format.gui_current != Some(target);
        if gui_changed {
            format.gui_previous = format.gui_current;
            format.gui_current = Some(target);
        }
        if host_changed || gui_changed {
            self.write(&format)?;
        }
        Ok(())
    }

    /// Record one exact GUI-to-GUI transition in a single file replacement.
    ///
    /// This is used immediately before a managed Wez pane is replaced by a
    /// correlated tmux client.  The fresh GUI proof is authoritative for the
    /// visible source, so it becomes `gui_previous` even when best-effort
    /// history was empty or stale.  A same-target transition is idempotent.
    pub fn record_gui_transition(
        &self,
        source: GuiHistoryTarget,
        destination: GuiHistoryTarget,
    ) -> io::Result<()> {
        let _lock = self.mutation_lock()?;
        let mut format = self.read();
        let source_host_changed = update_host_slots(&mut format, source.host_uid, source.space_uid);
        let destination_host_changed =
            update_host_slots(&mut format, destination.host_uid, destination.space_uid);
        let transition_changed = source != destination
            && (format.gui_current != Some(destination) || format.gui_previous != Some(source));
        let same_target_changed = source == destination && format.gui_current != Some(destination);
        if transition_changed {
            format.gui_previous = Some(source);
            format.gui_current = Some(destination);
        } else if same_target_changed {
            format.gui_current = Some(destination);
        }
        if source_host_changed
            || destination_host_changed
            || transition_changed
            || same_target_changed
        {
            self.write(&format)?;
        }
        Ok(())
    }

    /// Stage a transition without claiming that its tmux destination is
    /// visible. Reusing a client UID with different content is corruption.
    pub fn stage_gui_transition(&self, pending: PendingGuiTransition) -> io::Result<()> {
        let _lock = self.mutation_lock()?;
        let mut format = self.read();
        let key = pending.tmux_client_uid.to_string();
        if let Some(existing) = format.pending_gui.get(&key) {
            if existing == &pending {
                return Ok(());
            }
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| io::Error::other(format!("system clock: {error}")))?
                .as_secs();
            if existing.expires_at > now {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "tmux client UID already stages a different live GUI transition",
                ));
            }
        }
        format.pending_gui.insert(key, pending);
        self.write(&format)
    }

    pub fn pending_gui_transition(&self, uid: Uuid) -> Option<PendingGuiTransition> {
        self.read().pending_gui.get(&uid.to_string()).cloned()
    }

    /// Complete the exact still-pending transition and rotate source/current
    /// in one replacement. A raced cancellation/replacement returns false.
    pub fn complete_gui_transition(&self, expected: &PendingGuiTransition) -> io::Result<bool> {
        let _lock = self.mutation_lock()?;
        let mut format = self.read();
        let key = expected.tmux_client_uid.to_string();
        if format.pending_gui.get(&key) != Some(expected) {
            return Ok(false);
        }
        format.pending_gui.remove(&key);
        update_host_slots(
            &mut format,
            expected.source.host_uid,
            expected.source.space_uid,
        );
        update_host_slots(
            &mut format,
            expected.destination.host_uid,
            expected.destination.space_uid,
        );
        if expected.source != expected.destination {
            format.gui_previous = Some(expected.source);
        }
        format.gui_current = Some(expected.destination);
        self.write(&format)?;
        Ok(true)
    }

    pub fn cancel_gui_transition(&self, uid: Uuid) -> io::Result<bool> {
        let _lock = self.mutation_lock()?;
        let mut format = self.read();
        if format.pending_gui.remove(&uid.to_string()).is_none() {
            return Ok(false);
        }
        self.write(&format)?;
        Ok(true)
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

struct MutationLock {
    file: File,
}

impl Drop for MutationLock {
    fn drop(&mut self) {
        // Closing releases an advisory flock too; explicitly unlock so the
        // critical-section boundary is obvious and independent of drop order.
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn update_host_slots(format: &mut FileFormat, host: HostUid, space: SpaceUid) -> bool {
    let slots = format.hosts.entry(host.0.to_string()).or_default();
    match slots.current {
        Some(current) if current == space => false,
        Some(current) => {
            slots.previous = Some(current);
            slots.current = Some(space);
            true
        }
        None => {
            slots.current = Some(space);
            true
        }
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
