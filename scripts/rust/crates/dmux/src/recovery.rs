//! Guarded cold recovery for the service-owned WezTerm mux (plan §15.3).
//!
//! Automatic recovery is deliberately split across two processes:
//!
//! * this registry-only coordinator owns the common backend-instance kernel
//!   lock and the fenced `recovery:<instance>` lease for the whole restore;
//! * the `mux-startup` Lua callback performs native mux work in-process.
//!
//! They communicate through a mode-0700, epoch-scoped spool.  The
//! coordinator never inherits a Wez endpoint and never invokes `wezterm cli`;
//! Lua never writes the registry.  Every native-ID-dependent command carries
//! the current fencing token and is journaled before it is published.  A
//! reply is accepted only for the exact generation/sequence/fence tuple.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CStr, CString, OsString};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::bootstrap::{self, BootstrapJournal, BootstrapState, IssuedRequest};
use crate::locks::{LockMode, LockScope, OrderedLocks};
use crate::model::{Backend, BackendInstanceUid, Health, Lifecycle, ServerEpoch, SpaceUid};
use crate::registry::recovery::{
    BeginRecovery, RecoveryGenerationSpec, RecoveryJournalRow, RecoveryNodeSpec, RecoveryNodeState,
};
use crate::registry::{
    Lease, LeaseHolder, LeaseScope, Registry, RegistryConfig, RegistryError, TakeoverProof,
    now_rfc3339, probe_pid,
};

pub const MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const RECOVERY_PROTOCOL_VERSION: u32 = 1;
pub const RECOVERY_SUBDIR: &str = "recovery";
pub const GENERATION_ROOT_PATH: &str = "@generation";
const DEFAULT_LEASE_TTL: Duration = Duration::from_secs(30);
const DEFAULT_REPLY_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RECOVERY_MESSAGE_BYTES: u64 = 1024 * 1024;
const MAX_RECOVERY_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
pub const RECOVERY_MANIFEST_SUBDIR: &str = "recovery-manifests";

// -------------------------------------------------------------------------
// Public errors

#[derive(Debug)]
pub enum RecoveryError {
    Registry(RegistryError),
    Io(io::Error),
    Json(serde_json::Error),
    InvalidManifest(String),
    InvalidSnapshot(String),
    NonEmpty(String),
    FenceLost(String),
    Protocol(String),
    TimedOut(String),
    Failed(String),
}

impl RecoveryError {
    pub fn stable_code(&self) -> &'static str {
        match self {
            RecoveryError::Registry(RegistryError::Busy) => "registry_busy",
            RecoveryError::Registry(RegistryError::LeaseHeld { .. }) => "operation_in_progress",
            RecoveryError::Registry(_) => "recovery_registry_failed",
            RecoveryError::Io(_) => "recovery_io_failed",
            RecoveryError::Json(_) | RecoveryError::InvalidManifest(_) => {
                "recovery_manifest_invalid"
            }
            RecoveryError::InvalidSnapshot(_) | RecoveryError::NonEmpty(_) => "recovery_ineligible",
            RecoveryError::FenceLost(_) => "recovery_fence_lost",
            RecoveryError::Protocol(_) => "recovery_protocol_error",
            RecoveryError::TimedOut(_) => "recovery_timeout",
            RecoveryError::Failed(_) => "recovery_failed",
        }
    }
}

impl fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecoveryError::Registry(e) => write!(f, "recovery registry: {e}"),
            RecoveryError::Io(e) => write!(f, "recovery I/O: {e}"),
            RecoveryError::Json(e) => write!(f, "recovery JSON: {e}"),
            RecoveryError::InvalidManifest(e) => write!(f, "invalid recovery manifest: {e}"),
            RecoveryError::InvalidSnapshot(e) => write!(f, "invalid native snapshot: {e}"),
            RecoveryError::NonEmpty(e) => write!(f, "native mux is not recovery-empty: {e}"),
            RecoveryError::FenceLost(e) => write!(f, "recovery fence lost: {e}"),
            RecoveryError::Protocol(e) => write!(f, "recovery protocol: {e}"),
            RecoveryError::TimedOut(e) => write!(f, "recovery timed out: {e}"),
            RecoveryError::Failed(e) => write!(f, "recovery failed: {e}"),
        }
    }
}

impl std::error::Error for RecoveryError {}

impl From<RegistryError> for RecoveryError {
    fn from(value: RegistryError) -> Self {
        RecoveryError::Registry(value)
    }
}

impl From<io::Error> for RecoveryError {
    fn from(value: io::Error) -> Self {
        RecoveryError::Io(value)
    }
}

impl From<serde_json::Error> for RecoveryError {
    fn from(value: serde_json::Error) -> Self {
        RecoveryError::Json(value)
    }
}

pub type Result<T> = std::result::Result<T, RecoveryError>;

// -------------------------------------------------------------------------
// Durable all-Space manifest

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoveryManifest {
    pub schema_version: u32,
    pub state: String,
    pub manifest_id: String,
    pub backend_instance_uid: BackendInstanceUid,
    pub registry_revision: u64,
    pub generated_at: String,
    pub spaces: Vec<ManifestSpace>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestSpace {
    pub space_uid: SpaceUid,
    pub space_no: u64,
    pub opaque_key: String,
    pub logical_name: String,
    pub window_state: ManifestWindow,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestWindow {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub tabs: Vec<ManifestGroup>,
    #[serde(default)]
    pub size: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestGroup {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub is_active: bool,
    #[serde(default)]
    pub is_zoomed: bool,
    pub pane_tree: ManifestSplit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestSplit {
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub process: Option<Value>,
    #[serde(default)]
    pub is_active: bool,
    #[serde(default)]
    pub is_zoomed: bool,
    #[serde(default)]
    pub left: Option<u64>,
    #[serde(default)]
    pub top: Option<u64>,
    #[serde(default)]
    pub width: Option<u64>,
    #[serde(default)]
    pub height: Option<u64>,
    #[serde(default)]
    pub right: Option<Box<ManifestSplit>>,
    #[serde(default)]
    pub bottom: Option<Box<ManifestSplit>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreOperation {
    SpaceRoot,
    GroupRoot,
    Split,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitDirection {
    Right,
    Bottom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RestoreNode {
    pub manifest_node_path: String,
    pub space_uid: SpaceUid,
    pub space_no: u64,
    pub opaque_key: String,
    pub logical_name: String,
    pub group_index: usize,
    pub operation: RestoreOperation,
    pub parent_path: Option<String>,
    pub direction: Option<SplitDirection>,
    pub cwd: String,
    pub window_title: String,
    pub group_title: String,
    pub group_is_active: bool,
    pub text: Option<String>,
    pub process: Option<Value>,
    pub is_active: bool,
    pub is_zoomed: bool,
    pub width: Option<u64>,
    pub height: Option<u64>,
}

impl RecoveryManifest {
    pub fn validate(&self, expected_instance: BackendInstanceUid) -> Result<()> {
        if self.schema_version != MANIFEST_SCHEMA_VERSION {
            return Err(RecoveryError::InvalidManifest(format!(
                "schema_version {} is not {}",
                self.schema_version, MANIFEST_SCHEMA_VERSION
            )));
        }
        if self.state != "complete" {
            return Err(RecoveryError::InvalidManifest(format!(
                "manifest {} state is {:?}, not complete",
                self.manifest_id, self.state
            )));
        }
        if self.manifest_id.trim().is_empty() {
            return Err(RecoveryError::InvalidManifest("empty manifest_id".into()));
        }
        if self.backend_instance_uid != expected_instance {
            return Err(RecoveryError::InvalidManifest(format!(
                "manifest backend {} does not match {}",
                self.backend_instance_uid.0, expected_instance.0
            )));
        }
        let mut space_uids = BTreeSet::new();
        let mut opaque_keys = BTreeSet::new();
        for space in &self.spaces {
            if space.space_no == 0 {
                return Err(RecoveryError::InvalidManifest(format!(
                    "space {} has SpaceNo 0",
                    space.space_uid.0
                )));
            }
            if !space_uids.insert(space.space_uid.0) {
                return Err(RecoveryError::InvalidManifest(format!(
                    "duplicate SpaceUid {}",
                    space.space_uid.0
                )));
            }
            if space.opaque_key.is_empty() || !opaque_keys.insert(space.opaque_key.clone()) {
                return Err(RecoveryError::InvalidManifest(format!(
                    "empty or duplicate opaque key {:?}",
                    space.opaque_key
                )));
            }
            if space.window_state.tabs.is_empty() {
                return Err(RecoveryError::InvalidManifest(format!(
                    "space {} has no Groups",
                    space.space_uid.0
                )));
            }
            for (group_index, group) in space.window_state.tabs.iter().enumerate() {
                validate_split_domains(
                    &group.pane_tree,
                    &format!("space {} group {}", space.space_uid.0, group_index + 1),
                )?;
            }
        }
        Ok(())
    }

    /// Flatten one-window Space → Group(tab) → Split(pane) state into a
    /// deterministic pre-order journal.  Paths are stable across native ID
    /// allocation and never contain provider ordinals.
    pub fn restore_nodes(&self) -> Vec<RestoreNode> {
        let mut spaces: Vec<&ManifestSpace> = self.spaces.iter().collect();
        spaces.sort_by_key(|space| (space.space_no, space.space_uid.0));
        let mut nodes = Vec::new();
        for space in spaces {
            for (group_zero, group) in space.window_state.tabs.iter().enumerate() {
                let group_index = group_zero + 1;
                let path = format!(
                    "/spaces/{}/groups/{group_index}/splits/L",
                    space.space_uid.0
                );
                let operation = if group_index == 1 {
                    RestoreOperation::SpaceRoot
                } else {
                    RestoreOperation::GroupRoot
                };
                flatten_split(
                    &mut nodes,
                    space,
                    group,
                    group_index,
                    &group.pane_tree,
                    path,
                    operation,
                );
            }
        }
        nodes
    }
}

fn validate_split_domains(split: &ManifestSplit, at: &str) -> Result<()> {
    // A published owner manifest must contain local owner panes only.  The
    // resurrection fork removes imported panes before publication; reject a
    // hand-edited or stale manifest rather than turn a remote pane into a
    // local shell (acceptance 40).
    match split.domain.as_deref() {
        Some("local" | "unix" | "dmux") => {}
        Some(domain) => {
            return Err(RecoveryError::InvalidManifest(format!(
                "imported domain {domain:?} at {at}"
            )));
        }
        None => {
            return Err(RecoveryError::InvalidManifest(format!(
                "missing owner domain at {at}"
            )));
        }
    }
    if split.width.unwrap_or(0) == 0 || split.height.unwrap_or(0) == 0 {
        return Err(RecoveryError::InvalidManifest(format!(
            "missing/zero pane dimensions at {at}"
        )));
    }
    if let Some(right) = &split.right {
        validate_split_domains(right, &format!("{at}/R"))?;
    }
    if let Some(bottom) = &split.bottom {
        validate_split_domains(bottom, &format!("{at}/B"))?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn flatten_split(
    out: &mut Vec<RestoreNode>,
    space: &ManifestSpace,
    group: &ManifestGroup,
    group_index: usize,
    split: &ManifestSplit,
    path: String,
    operation: RestoreOperation,
) {
    push_restore_node(
        out,
        space,
        group,
        group_index,
        split,
        path.clone(),
        None,
        None,
        operation,
    );
    flatten_children(out, space, group, group_index, split, &path);
}

#[allow(clippy::too_many_arguments)]
fn push_restore_node(
    out: &mut Vec<RestoreNode>,
    space: &ManifestSpace,
    group: &ManifestGroup,
    group_index: usize,
    split: &ManifestSplit,
    path: String,
    parent_path: Option<String>,
    direction: Option<SplitDirection>,
    operation: RestoreOperation,
) {
    out.push(RestoreNode {
        manifest_node_path: path,
        space_uid: space.space_uid,
        space_no: space.space_no,
        opaque_key: space.opaque_key.clone(),
        logical_name: space.logical_name.clone(),
        group_index,
        operation,
        parent_path,
        direction,
        cwd: split.cwd.clone(),
        window_title: space.window_state.title.clone(),
        group_title: group.title.clone(),
        group_is_active: group.is_active,
        text: split.text.clone(),
        process: split.process.clone(),
        is_active: split.is_active,
        is_zoomed: split.is_zoomed || group.is_zoomed,
        width: split.width,
        height: split.height,
    });
}

fn flatten_children(
    out: &mut Vec<RestoreNode>,
    space: &ManifestSpace,
    group: &ManifestGroup,
    group_index: usize,
    split: &ManifestSplit,
    path: &str,
) {
    // Keep the fork's stable L/R/B vocabulary.  A child is always targeted
    // by its parent's path, never by a list ordinal or a guessed native ID.
    // When both children exist, use the resurrection fork's guillotine-cut
    // order: the subtree spanning the whole region is created first.  A
    // fixed right-then-bottom order distorts one of the two possible 2x2
    // encodings even though the node paths themselves remain stable.
    let mut children: Vec<(&ManifestSplit, String, SplitDirection)> = Vec::new();
    let right_first = split.right.as_deref().is_some_and(|right| {
        split.bottom.is_some() && split_span_height(right) > split.height.unwrap_or(0)
    });
    if right_first {
        if let Some(right) = split.right.as_deref() {
            children.push((right, format!("{path}R"), SplitDirection::Right));
        }
        if let Some(bottom) = split.bottom.as_deref() {
            children.push((bottom, format!("{path}B"), SplitDirection::Bottom));
        }
    } else {
        if let Some(bottom) = split.bottom.as_deref() {
            children.push((bottom, format!("{path}B"), SplitDirection::Bottom));
        }
        if let Some(right) = split.right.as_deref() {
            children.push((right, format!("{path}R"), SplitDirection::Right));
        }
    }

    // Both direct cuts are made before descending into either subtree,
    // matching tab_state.make_splits + pane_tree.fold.
    for (child, child_path, direction) in &children {
        push_restore_node(
            out,
            space,
            group,
            group_index,
            child,
            child_path.clone(),
            Some(path.to_string()),
            Some(*direction),
            RestoreOperation::Split,
        );
    }
    // pane_tree.fold descends the structural R branch before B regardless of
    // which direct guillotine cut had to be made first.
    if let Some(right) = split.right.as_deref() {
        flatten_children(out, space, group, group_index, right, &format!("{path}R"));
    }
    if let Some(bottom) = split.bottom.as_deref() {
        flatten_children(out, space, group, group_index, bottom, &format!("{path}B"));
    }
}

fn split_span_height(split: &ManifestSplit) -> u64 {
    split.height.unwrap_or(0) + split.bottom.as_deref().map(split_span_height).unwrap_or(0)
}

/// Fixed dmux-owned persistent root for cold-recovery manifests. Production
/// helpers must not accept a caller-selected/plugin-state directory.
pub fn production_recovery_manifest_dir() -> Result<PathBuf> {
    let database = crate::registry::production_db_path().ok_or_else(|| {
        RecoveryError::Io(io::Error::new(
            io::ErrorKind::NotFound,
            "neither XDG_DATA_HOME nor HOME is set",
        ))
    })?;
    let parent = database.parent().ok_or_else(|| {
        RecoveryError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            "production registry path has no dmux data directory",
        ))
    })?;
    Ok(parent.join(RECOVERY_MANIFEST_SUBDIR))
}

fn open_private_leaf_dir(path: &Path, create: bool) -> Result<PrivateDir> {
    match PrivateDir::open(path) {
        Ok(directory) => return Ok(directory),
        Err(error) if create && error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let parent = path.parent().ok_or_else(|| {
        RecoveryError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("private directory {} has no parent", path.display()),
        ))
    })?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            RecoveryError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("private directory {} has no UTF-8 leaf", path.display()),
            ))
        })?;
    PrivateDir::open(parent)?
        .open_child(name, create)
        .map_err(Into::into)
}

/// Load the newest valid complete manifest for one backend instance.  Bad
/// and partial candidates are reported and skipped so an older complete
/// generation remains usable.
pub fn newest_eligible_manifest(
    dir: &Path,
    instance: BackendInstanceUid,
    intentional_empty_revision: Option<u64>,
) -> Result<(Option<RecoveryManifest>, Vec<String>)> {
    let mut diagnostics = Vec::new();
    let mut candidates = Vec::new();
    let directory = match open_private_leaf_dir(dir, false) {
        Ok(directory) => directory,
        Err(RecoveryError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            return Ok((None, diagnostics));
        }
        Err(error) => return Err(error),
    };
    for os_name in directory.names()? {
        let Some(name) = os_name.to_str() else {
            diagnostics.push("manifest directory contains a non-UTF-8 entry".into());
            continue;
        };
        if !(name.ends_with(".json") || name.ends_with(".json.bak")) {
            continue;
        }
        let path = dir.join(name);
        let manifest = match directory
            .read_file(name, MAX_RECOVERY_MANIFEST_BYTES)
            .map_err(RecoveryError::from)
            .and_then(|bytes| serde_json::from_slice(&bytes).map_err(RecoveryError::from))
        {
            Ok(manifest) => manifest,
            Err(error) => {
                diagnostics.push(format!("{}: {error}", path.display()));
                continue;
            }
        };
        let manifest: RecoveryManifest = manifest;
        if let Err(error) = manifest.validate(instance) {
            diagnostics.push(format!("{}: {error}", path.display()));
            continue;
        }
        if let Some(floor) = intentional_empty_revision
            && manifest.registry_revision <= floor
        {
            diagnostics.push(format!(
                "{}: revision {} is at/below intentional-empty floor {}",
                path.display(),
                manifest.registry_revision,
                floor
            ));
            continue;
        }
        candidates.push(manifest);
    }
    candidates.sort_by(|a, b| {
        (b.registry_revision, &b.generated_at, &b.manifest_id).cmp(&(
            a.registry_revision,
            &a.generated_at,
            &a.manifest_id,
        ))
    });
    Ok((candidates.into_iter().next(), diagnostics))
}

// -------------------------------------------------------------------------
// Complete in-process native snapshots

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeSnapshot {
    pub complete: bool,
    pub server_epoch: ServerEpoch,
    #[serde(default)]
    pub windows: Vec<NativeWindow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeWindow {
    pub window_id: String,
    pub workspace: String,
    #[serde(default)]
    pub tabs: Vec<NativeTab>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeTab {
    pub tab_id: String,
    #[serde(default)]
    pub panes: Vec<NativePane>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativePane {
    pub pane_id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub domain: Option<String>,
}

/// Canonical raw-native topology carried back into an in-process mutating
/// command. Lua captures the same projection and compares it before touching
/// the mux, closing the Inspect -> mutate TOCTOU for non-cooperating writers.
/// Titles are deliberately excluded because shells may update them without a
/// topology change; domain, workspace, and every native parent tuple remain
/// part of the precondition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeTreePrecondition {
    pub server_epoch: ServerEpoch,
    pub windows: Vec<NativeTreeWindow>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NativeTreeWindow {
    pub window_id: String,
    pub workspace: String,
    pub tabs: Vec<NativeTreeTab>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NativeTreeTab {
    pub tab_id: String,
    pub panes: Vec<NativeTreePane>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NativeTreePane {
    pub pane_id: String,
    pub domain: String,
}

impl NativeSnapshot {
    pub fn validate_complete(&self, epoch: ServerEpoch) -> Result<()> {
        if !self.complete {
            return Err(RecoveryError::InvalidSnapshot(
                "provider scan is not complete".into(),
            ));
        }
        if self.server_epoch != epoch {
            return Err(RecoveryError::InvalidSnapshot(format!(
                "scan epoch {} does not match {}",
                self.server_epoch.0, epoch.0
            )));
        }
        let mut window_ids = BTreeSet::new();
        let mut tab_ids = BTreeSet::new();
        let mut pane_ids = BTreeSet::new();
        for window in &self.windows {
            if !window_ids.insert(&window.window_id) {
                return Err(RecoveryError::InvalidSnapshot(format!(
                    "duplicate window id {}",
                    window.window_id
                )));
            }
            for tab in &window.tabs {
                if !tab_ids.insert(&tab.tab_id) {
                    return Err(RecoveryError::InvalidSnapshot(format!(
                        "duplicate tab id {}",
                        tab.tab_id
                    )));
                }
                for pane in &tab.panes {
                    if !pane_ids.insert(&pane.pane_id) {
                        return Err(RecoveryError::InvalidSnapshot(format!(
                            "duplicate pane id {}",
                            pane.pane_id
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn panes(&self) -> impl Iterator<Item = (&NativeWindow, &NativeTab, &NativePane)> {
        self.windows.iter().flat_map(|window| {
            window
                .tabs
                .iter()
                .flat_map(move |tab| tab.panes.iter().map(move |pane| (window, tab, pane)))
        })
    }

    pub fn require_sentinel_only(&self, epoch: ServerEpoch) -> Result<SentinelWitness> {
        self.validate_complete(epoch)?;
        let sentinel_workspace = format!("dmux:system:{}", epoch.0);
        let sentinel: Vec<_> = self
            .panes()
            .filter(|(window, _, _)| window.workspace == sentinel_workspace)
            .collect();
        let all: Vec<_> = self.panes().collect();
        if sentinel.len() != 1 {
            return Err(RecoveryError::InvalidSnapshot(format!(
                "expected exactly one {sentinel_workspace:?} sentinel pane, found {}",
                sentinel.len()
            )));
        }
        if all.len() != 1 || self.windows.len() != 1 || self.windows[0].tabs.len() != 1 {
            return Err(RecoveryError::NonEmpty(format!(
                "sentinel plus {} user panes/windows were present",
                all.len().saturating_sub(1)
            )));
        }
        let (window, tab, pane) = sentinel[0];
        Ok(SentinelWitness {
            window_id: window.window_id.clone(),
            tab_id: tab.tab_id.clone(),
            pane_id: pane.pane_id.clone(),
        })
    }

    pub fn tree_precondition(&self) -> NativeTreePrecondition {
        let mut windows = self
            .windows
            .iter()
            .map(|window| {
                let mut tabs = window
                    .tabs
                    .iter()
                    .map(|tab| {
                        let mut panes = tab
                            .panes
                            .iter()
                            .map(|pane| NativeTreePane {
                                pane_id: pane.pane_id.clone(),
                                domain: pane.domain.clone().unwrap_or_default(),
                            })
                            .collect::<Vec<_>>();
                        panes.sort();
                        NativeTreeTab {
                            tab_id: tab.tab_id.clone(),
                            panes,
                        }
                    })
                    .collect::<Vec<_>>();
                tabs.sort();
                NativeTreeWindow {
                    window_id: window.window_id.clone(),
                    workspace: window.workspace.clone(),
                    tabs,
                }
            })
            .collect::<Vec<_>>();
        windows.sort();
        NativeTreePrecondition {
            server_epoch: self.server_epoch,
            windows,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SentinelWitness {
    pub window_id: String,
    pub tab_id: String,
    pub pane_id: String,
}

// -------------------------------------------------------------------------
// Epoch-scoped file protocol

#[derive(Debug, Clone)]
pub struct RecoverySpool {
    runtime_dir: PathBuf,
    epoch: ServerEpoch,
    directory: Arc<OnceLock<PrivateDir>>,
    pub dir: PathBuf,
    pub command: PathBuf,
    pub response: PathBuf,
    pub status: PathBuf,
    pub control: PathBuf,
    pub initial_snapshot: PathBuf,
}

impl RecoverySpool {
    pub fn new(runtime_dir: &Path, epoch: ServerEpoch) -> Self {
        let dir = runtime_dir.join(RECOVERY_SUBDIR).join(epoch.0.to_string());
        RecoverySpool {
            runtime_dir: runtime_dir.to_path_buf(),
            epoch,
            directory: Arc::new(OnceLock::new()),
            command: dir.join("command.json"),
            response: dir.join("response.json"),
            status: dir.join("status.json"),
            control: dir.join("control.json"),
            initial_snapshot: dir.join("initial-snapshot.json"),
            dir,
        }
    }

    pub fn prepare(&self) -> Result<()> {
        self.open_dir(true)?;
        Ok(())
    }

    pub fn clear_messages(&self) -> Result<()> {
        self.remove(RecoverySpoolFile::Command)?;
        self.remove(RecoverySpoolFile::Response)?;
        Ok(())
    }

    fn open_dir(&self, create: bool) -> Result<&PrivateDir> {
        if let Some(directory) = self.directory.get() {
            return Ok(directory);
        }
        let runtime = PrivateDir::open(&self.runtime_dir)?;
        let recovery = runtime.open_child(RECOVERY_SUBDIR, create)?;
        let candidate = recovery.open_child(&self.epoch.0.to_string(), create)?;
        let _ = self.directory.set(candidate);
        self.directory.get().ok_or_else(|| {
            RecoveryError::Io(io::Error::other(
                "recovery spool directory capability was not retained",
            ))
        })
    }

    fn read<T: serde::de::DeserializeOwned>(&self, kind: RecoverySpoolFile) -> Result<T> {
        let dir = self.open_dir(false)?;
        let bytes = dir.read_file(kind.name(), MAX_RECOVERY_MESSAGE_BYTES)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn write<T: Serialize>(&self, kind: RecoverySpoolFile, value: &T) -> Result<()> {
        let dir = self.open_dir(true)?;
        let mut bytes = serde_json::to_vec(value)?;
        bytes.push(b'\n');
        dir.atomic_replace(kind.name(), &bytes, MAX_RECOVERY_MESSAGE_BYTES)?;
        Ok(())
    }

    fn create_once<T: Serialize>(&self, kind: RecoverySpoolFile, value: &T) -> Result<()> {
        let dir = self.open_dir(true)?;
        let mut bytes = serde_json::to_vec(value)?;
        bytes.push(b'\n');
        dir.create_once(kind.name(), &bytes, MAX_RECOVERY_MESSAGE_BYTES)?;
        Ok(())
    }

    fn remove(&self, kind: RecoverySpoolFile) -> Result<bool> {
        let dir = self.open_dir(false)?;
        dir.remove_file(kind.name()).map_err(Into::into)
    }
}

#[derive(Debug, Clone, Copy)]
enum RecoverySpoolFile {
    Command,
    Response,
    Status,
    Control,
}

impl RecoverySpoolFile {
    const fn name(self) -> &'static str {
        match self {
            RecoverySpoolFile::Command => "command.json",
            RecoverySpoolFile::Response => "response.json",
            RecoverySpoolFile::Status => "status.json",
            RecoverySpoolFile::Control => "control.json",
        }
    }
}

#[derive(Debug)]
struct PrivateDir(File);

impl PrivateDir {
    fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        validate_private_directory(path, &file.metadata()?)?;
        Ok(Self(file))
    }

    fn open_child(&self, name: &str, create: bool) -> io::Result<Self> {
        let name = secure_component(name)?;
        if create {
            let rc = unsafe { libc::mkdirat(self.0.as_raw_fd(), name.as_ptr(), 0o700) };
            if rc != 0 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::AlreadyExists {
                    return Err(error);
                }
            }
        }
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        validate_private_directory(
            Path::new(name.to_str().unwrap_or("<entry>")),
            &file.metadata()?,
        )?;
        Ok(Self(file))
    }

    fn open_file(&self, name: &str, flags: libc::c_int, mode: libc::mode_t) -> io::Result<File> {
        let name = secure_component(name)?;
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                libc::c_uint::from(mode),
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    fn read_file(&self, name: &str, maximum: u64) -> io::Result<Vec<u8>> {
        let mut file = self.open_file(name, libc::O_RDONLY | libc::O_NONBLOCK, 0)?;
        let before = file.metadata()?;
        validate_private_file(name, &before, maximum)?;
        let capacity = usize::try_from(before.len().min(maximum)).unwrap_or(usize::MAX);
        let mut bytes = Vec::with_capacity(capacity);
        Read::by_ref(&mut file)
            .take(maximum + 1)
            .read_to_end(&mut bytes)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > maximum {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("private file {name} grew beyond {maximum} bytes while read"),
            ));
        }
        let after = file.metadata()?;
        validate_private_file(name, &after, maximum)?;
        let current = self.open_file(name, libc::O_RDONLY | libc::O_NONBLOCK, 0)?;
        let current = current.metadata()?;
        if private_file_fingerprint(&before) != private_file_fingerprint(&after)
            || before.dev() != current.dev()
            || before.ino() != current.ino()
        {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                format!("private file {name} changed while it was read"),
            ));
        }
        Ok(bytes)
    }

    fn create_once(&self, name: &str, bytes: &[u8], maximum: u64) -> io::Result<()> {
        bounded_bytes(name, bytes, maximum)?;
        let mut file =
            self.open_file(name, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL, 0o600)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        validate_private_file(name, &file.metadata()?, maximum)?;
        self.0.sync_all()?;
        let actual = self.read_file(name, maximum)?;
        if actual != bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("private file {name} differs after publication"),
            ));
        }
        Ok(())
    }

    fn atomic_replace(&self, name: &str, bytes: &[u8], maximum: u64) -> io::Result<()> {
        bounded_bytes(name, bytes, maximum)?;
        match self.open_file(name, libc::O_RDONLY | libc::O_NONBLOCK, 0) {
            Ok(existing) => validate_private_file(name, &existing.metadata()?, maximum)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let temporary = format!(".dmux-{}.tmp", Uuid::new_v4());
        let result = (|| {
            let mut file = self.open_file(
                &temporary,
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                0o600,
            )?;
            file.write_all(bytes)?;
            file.sync_all()?;
            validate_private_file(&temporary, &file.metadata()?, maximum)?;
            let old = secure_component(&temporary)?;
            let new = secure_component(name)?;
            if unsafe {
                libc::renameat(
                    self.0.as_raw_fd(),
                    old.as_ptr(),
                    self.0.as_raw_fd(),
                    new.as_ptr(),
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            self.0.sync_all()?;
            let actual = self.read_file(name, maximum)?;
            if actual != bytes {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("private file {name} differs after atomic publication"),
                ));
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = self.unlink_name(&temporary);
        }
        result
    }

    fn remove_file(&self, name: &str) -> io::Result<bool> {
        let observed = match self.open_file(name, libc::O_RDONLY | libc::O_NONBLOCK, 0) {
            Ok(file) => {
                let metadata = file.metadata()?;
                validate_private_file(name, &metadata, MAX_RECOVERY_MESSAGE_BYTES)?;
                metadata
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        // POSIX has no compare-and-unlink-by-inode. Rename the name to a
        // private consumed slot first, then verify the moved inode through
        // the held directory capability. A raced replacement is quarantined
        // and rejected, never deleted as if it were the observed message.
        let consumed = format!(".dmux-consumed-{}", Uuid::new_v4());
        let old = secure_component(name)?;
        let new = secure_component(&consumed)?;
        if unsafe {
            libc::renameat(
                self.0.as_raw_fd(),
                old.as_ptr(),
                self.0.as_raw_fd(),
                new.as_ptr(),
            )
        } != 0
        {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::NotFound {
                return Ok(false);
            }
            return Err(error);
        }
        let moved = self.open_file(&consumed, libc::O_RDONLY | libc::O_NONBLOCK, 0)?;
        let moved = moved.metadata()?;
        if observed.dev() != moved.dev() || observed.ino() != moved.ino() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                format!("private file {name} was replaced before removal"),
            ));
        }
        self.unlink_name(&consumed)?;
        self.0.sync_all()?;
        Ok(true)
    }

    fn names(&self) -> io::Result<Vec<OsString>> {
        let duplicate = unsafe { libc::dup(self.0.as_raw_fd()) };
        if duplicate < 0 {
            return Err(io::Error::last_os_error());
        }
        let directory = unsafe { libc::fdopendir(duplicate) };
        if directory.is_null() {
            let error = io::Error::last_os_error();
            unsafe {
                libc::close(duplicate);
            }
            return Err(error);
        }
        let mut names = Vec::new();
        loop {
            // POSIX requires errno to distinguish end-of-directory from an
            // error. Rust libc exposes platform errno accessors separately.
            set_errno_zero();
            let entry = unsafe { libc::readdir(directory) };
            if entry.is_null() {
                let error = io::Error::last_os_error();
                if error.raw_os_error().unwrap_or(0) != 0 {
                    unsafe {
                        libc::closedir(directory);
                    }
                    return Err(error);
                }
                break;
            }
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if name != b"." && name != b".." {
                names.push(OsString::from_vec(name.to_vec()));
            }
        }
        if unsafe { libc::closedir(directory) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(names)
    }

    fn publish_immutable(&self, name: &str, bytes: &[u8], maximum: u64) -> io::Result<()> {
        bounded_bytes(name, bytes, maximum)?;
        match self.read_file(name, maximum) {
            Ok(existing) if existing == bytes => return Ok(()),
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("immutable private file {name} already exists with different bytes"),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let temporary = format!(".dmux-{}.tmp", Uuid::new_v4());
        let result = (|| {
            let mut file = self.open_file(
                &temporary,
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                0o600,
            )?;
            file.write_all(bytes)?;
            file.sync_all()?;
            validate_private_file(&temporary, &file.metadata()?, maximum)?;
            let old = secure_component(&temporary)?;
            let new = secure_component(name)?;
            if let Err(error) = rename_noreplace(self.0.as_raw_fd(), &old, &new) {
                if error.kind() == io::ErrorKind::AlreadyExists {
                    let existing = self.read_file(name, maximum)?;
                    if existing == bytes {
                        self.unlink_name(&temporary)?;
                        self.0.sync_all()?;
                        return Ok(());
                    }
                }
                return Err(error);
            }
            self.0.sync_all()?;
            let actual = self.read_file(name, maximum)?;
            if actual != bytes {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("immutable private file {name} differs after publication"),
                ));
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = self.unlink_name(&temporary);
        }
        result
    }

    fn unlink_name(&self, name: &str) -> io::Result<()> {
        let name = secure_component(name)?;
        if unsafe { libc::unlinkat(self.0.as_raw_fd(), name.as_ptr(), 0) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn rename_noreplace(directory: RawFd, old: &CStr, new: &CStr) -> io::Result<()> {
    if unsafe {
        libc::renameatx_np(
            directory,
            old.as_ptr(),
            directory,
            new.as_ptr(),
            libc::RENAME_EXCL,
        )
    } == 0
    {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "linux")]
fn rename_noreplace(directory: RawFd, old: &CStr, new: &CStr) -> io::Result<()> {
    if unsafe {
        libc::renameat2(
            directory,
            old.as_ptr(),
            directory,
            new.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    } == 0
    {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "macos")]
fn set_errno_zero() {
    unsafe {
        *libc::__error() = 0;
    }
}

#[cfg(target_os = "linux")]
fn set_errno_zero() {
    unsafe {
        *libc::__errno_location() = 0;
    }
}

fn secure_component(name: &str) -> io::Result<CString> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsafe private path component {name:?}"),
        ));
    }
    CString::new(name).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "private path component contains NUL",
        )
    })
}

fn validate_private_directory(path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
    let euid = unsafe { libc::geteuid() };
    if !metadata.is_dir() || metadata.uid() != euid || metadata.mode() & 0o7777 != 0o700 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "private directory {} must be current-user-owned, non-symlink, and mode 0700",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn validate_private_file(name: &str, metadata: &fs::Metadata, maximum: u64) -> io::Result<()> {
    // Every caller stats a descriptor it opened by name or created itself, so
    // the name had a link at that moment.  Zero links now means the name was
    // unlinked or renamed over in between -- a concurrent republish, not a
    // hostile file.  Report the same transient signal `read_file` raises when
    // the inode moves under it, so pollers retry instead of failing the run.
    if metadata.nlink() == 0 {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            format!("private file {name} was replaced while it was open"),
        ));
    }
    let euid = unsafe { libc::geteuid() };
    if !metadata.is_file()
        || metadata.uid() != euid
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "private file {name} must be current-user-owned, single-link, non-symlink, and mode 0600"
            ),
        ));
    }
    if metadata.len() > maximum {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("private file {name} exceeds {maximum} bytes"),
        ));
    }
    Ok(())
}

fn bounded_bytes(name: &str, bytes: &[u8], maximum: u64) -> io::Result<()> {
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > maximum {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("private file {name} exceeds {maximum} bytes"),
        ));
    }
    Ok(())
}

fn private_file_fingerprint(
    metadata: &fs::Metadata,
) -> (u64, u64, u64, u32, u64, i64, i64, i64, i64) {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.nlink(),
        metadata.mode(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    )
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoveryCommand {
    pub protocol_version: u32,
    /// Identifies one registry-only coordinator process.  A crash-resume
    /// process reuses the durable generation but starts its sequence at one,
    /// so the Lua side must key replay suppression by this value as well as
    /// by `sequence`.
    pub coordinator_uid: Uuid,
    pub generation_uid: Uuid,
    pub sequence: u64,
    pub fencing_token: i64,
    #[serde(flatten)]
    pub action: RecoveryAction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)] // kept inline so the JSON wire shape stays explicit
pub enum RecoveryAction {
    Inspect,
    Prepare {
        nodes: Vec<RestoreNode>,
    },
    CompareAndRestoreNode {
        node: RestoreNode,
        request_uid: Uuid,
        bootstrap_argv: Vec<String>,
        expected_tree: NativeTreePrecondition,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_parent: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_existing: Option<CreatedNode>,
        create_if_absent: bool,
    },
    CompareAndRemoveNode {
        manifest_node_path: String,
        pane_id: String,
        tab_id: String,
        window_id: String,
        expected_tree: NativeTreePrecondition,
    },
    Verify {
        expected_nodes: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoveryResponse {
    pub protocol_version: u32,
    pub coordinator_uid: Uuid,
    pub generation_uid: Uuid,
    pub sequence: u64,
    pub fencing_token: i64,
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub snapshot: Option<NativeSnapshot>,
    #[serde(default)]
    pub created: Option<CreatedNode>,
    #[serde(default)]
    pub removed: Option<RemovedNode>,
    #[serde(default)]
    pub existing_absent: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedNode {
    pub window_id: String,
    pub tab_id: String,
    pub pane_id: String,
    /// Complete in-process scan of panes bearing this request's reserved
    /// title after the spawn.  It is the title witness in ADR 004's
    /// spawn-return = title scan = inherited-env correlation.
    pub titled_pane_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovedNodeStatus {
    Removed,
    NotFound,
    ParentMismatch,
    PostconditionFailed,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovedNode {
    pub schema_version: u32,
    pub status: RemovedNodeStatus,
    pub kind: String,
    pub requested_native_id: u64,
    #[serde(default)]
    pub actual_parent_tab_id: Option<u64>,
    #[serde(default)]
    pub actual_parent_window_id: Option<u64>,
    #[serde(default)]
    pub actual_workspace: Option<String>,
    #[serde(default)]
    pub removed_pane_ids: Vec<u64>,
    #[serde(default)]
    pub removed_tab_ids: Vec<u64>,
    #[serde(default)]
    pub removed_window_ids: Vec<u64>,
    #[serde(default)]
    pub postcondition_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryStatusState {
    Starting,
    Recovering,
    Ready,
    Failed,
    Aborted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryStatus {
    pub protocol_version: u32,
    pub state: RecoveryStatusState,
    pub backend_instance_uid: BackendInstanceUid,
    pub server_epoch: ServerEpoch,
    pub coordinator_uid: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_uid: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fencing_token: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_node: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryControlAction {
    Resume,
    Abort,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryControlRequest {
    pub protocol_version: u32,
    pub action: RecoveryControlAction,
    pub request_uid: Uuid,
    pub backend_instance_uid: BackendInstanceUid,
    pub server_epoch: ServerEpoch,
    pub requested_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryInspection {
    pub backend_instance_uid: BackendInstanceUid,
    pub server_epoch: Option<ServerEpoch>,
    pub status: Option<RecoveryStatus>,
    pub generation: Option<RecoveryGenerationSpec>,
    pub journal: Vec<RecoveryJournalRow>,
}

/// Read-only public status surface.  The runtime sidecar is accepted only
/// when it names the registry's exact current incarnation; the durable
/// journal remains authoritative when the sidecar is missing after a crash.
pub fn inspect_recovery(
    config: RegistryConfig,
    runtime_dir: &Path,
    instance: BackendInstanceUid,
) -> Result<RecoveryInspection> {
    let registry = Registry::open(config)?;
    let epoch = registry.backend_server(instance)?.server_epoch;
    let Some(epoch) = epoch else {
        return Ok(RecoveryInspection {
            backend_instance_uid: instance,
            server_epoch: None,
            status: None,
            generation: None,
            journal: Vec::new(),
        });
    };
    let spool = RecoverySpool::new(runtime_dir, epoch);
    let status = match spool.read::<RecoveryStatus>(RecoverySpoolFile::Status) {
        Ok(status) => {
            if status.protocol_version != RECOVERY_PROTOCOL_VERSION
                || status.backend_instance_uid != instance
                || status.server_epoch != epoch
            {
                return Err(RecoveryError::Protocol(
                    "recovery status does not match the current backend incarnation".into(),
                ));
            }
            Some(status)
        }
        Err(RecoveryError::Io(error)) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let unfinished = registry.unfinished_recovery_for_instance(instance)?;
    let durable = if unfinished.is_some() {
        unfinished
    } else if status
        .as_ref()
        .is_some_and(|status| status.state == RecoveryStatusState::Failed)
    {
        registry.completed_recovery(instance, epoch)?
    } else {
        None
    };
    let (generation, journal) = durable
        .map(|(spec, rows)| (Some(spec), rows))
        .unwrap_or_else(|| (None, Vec::new()));
    Ok(RecoveryInspection {
        backend_instance_uid: instance,
        server_epoch: Some(epoch),
        status,
        generation,
        journal,
    })
}

/// Ask the still-running mux owner to start an explicit failed-generation
/// resume.  This does not acquire or bypass a fence: Lua launches the normal
/// registry-only coordinator, which must win the common instance lock and
/// present `resume_failed=true` before any native step.
pub fn request_recovery_resume(
    config: RegistryConfig,
    runtime_dir: &Path,
    instance: BackendInstanceUid,
) -> Result<RecoveryControlRequest> {
    request_recovery_control(config, runtime_dir, instance, RecoveryControlAction::Resume)
}

fn request_recovery_control(
    config: RegistryConfig,
    runtime_dir: &Path,
    instance: BackendInstanceUid,
    action: RecoveryControlAction,
) -> Result<RecoveryControlRequest> {
    let registry = Registry::open(config)?;
    let epoch = registry
        .backend_server(instance)?
        .server_epoch
        .ok_or_else(|| RecoveryError::Failed("backend has no running server epoch".into()))?;
    let spool = RecoverySpool::new(runtime_dir, epoch);
    let unfinished = registry.unfinished_recovery_for_instance(instance)?;
    let (_spec, rows) = if let Some(unfinished) = unfinished {
        unfinished
    } else if action == RecoveryControlAction::Resume {
        let status: RecoveryStatus = spool.read(RecoverySpoolFile::Status).map_err(|error| {
            RecoveryError::Failed(format!(
                "backend has no unfinished recovery generation and no failed completion sidecar: {error}"
            ))
        })?;
        if status.protocol_version != RECOVERY_PROTOCOL_VERSION
            || status.backend_instance_uid != instance
            || status.server_epoch != epoch
            || status.state != RecoveryStatusState::Failed
        {
            return Err(RecoveryError::Failed(
                "backend has no unfinished failed recovery generation".into(),
            ));
        }
        let completed = registry
            .completed_recovery(instance, epoch)?
            .ok_or_else(|| {
                RecoveryError::Failed("backend has no recoverable completed generation".into())
            })?;
        if status.generation_uid != Some(completed.0.generation_uid)
            || status.manifest_id.as_deref() != Some(completed.0.manifest_id.as_str())
        {
            return Err(RecoveryError::Protocol(
                "failed completion sidecar does not name the completed journal".into(),
            ));
        }
        completed
    } else {
        return Err(RecoveryError::Failed(
            "backend has no unfinished recovery generation".into(),
        ));
    };
    let root = rows
        .iter()
        .find(|row| row.manifest_node_path == GENERATION_ROOT_PATH)
        .ok_or_else(|| RecoveryError::Protocol("unfinished generation has no root row".into()))?;
    if root.node_state != RecoveryNodeState::Failed
        && !(action == RecoveryControlAction::Resume
            && root.node_state == RecoveryNodeState::Completed)
    {
        return Err(RecoveryError::Failed(format!(
            "generation {} is {}, not failed",
            root.generation_uid,
            root.node_state.as_str()
        )));
    }
    let request = RecoveryControlRequest {
        protocol_version: RECOVERY_PROTOCOL_VERSION,
        action,
        request_uid: Uuid::new_v4(),
        backend_instance_uid: instance,
        server_epoch: epoch,
        requested_at: now_rfc3339(),
    };
    spool.prepare()?;
    match spool.read::<RecoveryControlRequest>(RecoverySpoolFile::Control) {
        Ok(existing)
            if existing.protocol_version == RECOVERY_PROTOCOL_VERSION
                && existing.action == action
                && existing.backend_instance_uid == instance
                && existing.server_epoch == epoch =>
        {
            return Ok(existing);
        }
        Ok(_) => {
            return Err(RecoveryError::Protocol(
                "a different recovery control request is already pending".into(),
            ));
        }
        Err(RecoveryError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    match spool.create_once(RecoverySpoolFile::Control, &request) {
        Ok(()) => Ok(request),
        Err(RecoveryError::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists => {
            let existing: RecoveryControlRequest = spool.read(RecoverySpoolFile::Control)?;
            if existing.protocol_version == RECOVERY_PROTOCOL_VERSION
                && existing.action == action
                && existing.backend_instance_uid == instance
                && existing.server_epoch == epoch
            {
                Ok(existing)
            } else {
                Err(RecoveryError::Protocol(
                    "a different recovery control request won the publish race".into(),
                ))
            }
        }
        Err(error) => Err(error),
    }
}

pub fn request_recovery_abort(
    config: RegistryConfig,
    runtime_dir: &Path,
    instance: BackendInstanceUid,
) -> Result<RecoveryControlRequest> {
    request_recovery_control(config, runtime_dir, instance, RecoveryControlAction::Abort)
}

/// Lua publishes each spool document by atomic rename, so a poll can catch the
/// swap: the name is momentarily absent, or the inode the reader already opened
/// is unlinked under it mid-read.  Neither is a verdict on the document -- the
/// next poll observes the settled one, and the caller's deadline still bounds
/// the wait.
fn spool_read_is_transient(error: &RecoveryError) -> bool {
    matches!(
        error,
        RecoveryError::Io(error)
            if error.kind() == io::ErrorKind::NotFound
                || error.kind() == io::ErrorKind::Interrupted
    )
}

fn wait_for_response(
    spool: &RecoverySpool,
    command: &RecoveryCommand,
    timeout: Duration,
) -> Result<RecoveryResponse> {
    let started = Instant::now();
    loop {
        match spool.read::<RecoveryResponse>(RecoverySpoolFile::Response) {
            Ok(response) => {
                // A killed coordinator may finish one in-process Lua action
                // after its OFD lock/lease has been taken over. Its atomic
                // response can therefore appear after the higher-fence
                // coordinator published a new command. Leave that stale
                // document in place for Lua's matching response to replace;
                // deleting by pathname would race and could unlink the new
                // response. The common deadline still bounds this wait.
                if response.fencing_token < command.fencing_token {
                    if started.elapsed() >= timeout {
                        return Err(RecoveryError::TimedOut(format!(
                            "only stale responses preceded sequence {}",
                            command.sequence
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                }
                if response.fencing_token > command.fencing_token {
                    return Err(RecoveryError::FenceLost(format!(
                        "response fence {} is newer than command fence {}",
                        response.fencing_token, command.fencing_token
                    )));
                }
                if response.protocol_version != RECOVERY_PROTOCOL_VERSION
                    || response.coordinator_uid != command.coordinator_uid
                    || response.generation_uid != command.generation_uid
                    || response.sequence != command.sequence
                {
                    return Err(RecoveryError::Protocol(format!(
                        "response tuple does not match command generation={} sequence={} fence={}",
                        command.generation_uid, command.sequence, command.fencing_token
                    )));
                }
                return Ok(response);
            }
            Err(e) if spool_read_is_transient(&e) => {}
            Err(e) => return Err(e),
        }
        if started.elapsed() >= timeout {
            return Err(RecoveryError::TimedOut(format!(
                "no response for sequence {}",
                command.sequence
            )));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

// The fenced coordinator and snapshot publication are below.  They use only
// the typed Registry recovery seam; no production code reaches through
// `Registry::raw_connection()`.

// -------------------------------------------------------------------------
// Common fenced backend-instance guard

struct InstanceLeaseGuard {
    registry: Registry,
    locks: OrderedLocks,
    scope: LeaseScope,
    holder: LeaseHolder,
    lease: Lease,
    ttl: Duration,
    release_on_drop: bool,
}

impl InstanceLeaseGuard {
    fn acquire(
        config: RegistryConfig,
        instance: BackendInstanceUid,
        scope: LeaseScope,
        request_uid: Uuid,
        ttl: Duration,
    ) -> Result<Self> {
        let mut registry = Registry::open(config.clone())?;
        let mut locks = OrderedLocks::new(&config.lock_dir);
        locks.acquire(LockScope::AuthorityGate, LockMode::Shared)?;
        let backend_scope = LockScope::BackendInstance(instance);
        locks.acquire(backend_scope.clone(), LockMode::Exclusive)?;
        let kernel = locks.held(&backend_scope).ok_or_else(|| {
            RecoveryError::FenceLost("backend kernel lock vanished after acquisition".into())
        })?;

        let holder = LeaseHolder::current(request_uid);
        let current = registry.current_lease(&scope)?;
        let takeover = current.as_ref().and_then(|lease| {
            if lease.holder_request_uid == request_uid {
                return None;
            }
            lease.holder_pid.map(|pid| TakeoverProof {
                prior_pid: pid,
                liveness: probe_pid(pid),
            })
        });
        let lease = registry.acquire_lease(&scope, &holder, ttl, kernel, takeover.as_ref())?;
        Ok(InstanceLeaseGuard {
            registry,
            locks,
            scope,
            holder,
            lease,
            ttl,
            release_on_drop: false,
        })
    }

    fn fence(&mut self) -> Result<i64> {
        self.lease = self
            .registry
            .renew_lease(&self.scope, self.holder.request_uid, self.ttl)?;
        self.registry.assert_lease_fence(&self.scope, &self.lease)?;
        Ok(self.lease.fencing_token)
    }

    fn release(mut self) -> Result<()> {
        self.registry
            .release_lease(&self.scope, self.holder.request_uid)?;
        self.release_on_drop = true;
        Ok(())
    }
}

impl Drop for InstanceLeaseGuard {
    fn drop(&mut self) {
        if self.release_on_drop {
            // `release` already changed the durable row; OrderedLocks then
            // drops in reverse order.  Failed recovery intentionally leaves
            // the held lease row observable for fenced resume/abort.
        }
    }
}

impl From<crate::locks::LockError> for RecoveryError {
    fn from(value: crate::locks::LockError) -> Self {
        RecoveryError::Registry(RegistryError::Lock(value))
    }
}

/// Registry-only service bootstrap.  The wrapper captures this UUID and
/// supplies it to the mux server; an empty placeholder is never accepted in
/// feature-on mode.
pub fn ensure_wez_backend_instance(
    config: RegistryConfig,
    socket: &Path,
    service_label: &str,
) -> Result<BackendInstanceUid> {
    let fixed_socket = crate::runtime::dmux_runtime_dir()?.join(crate::runtime::WEZ_SOCKET_FILE);
    if socket != fixed_socket {
        return Err(RecoveryError::FenceLost(format!(
            "managed Wez registration socket {} is not fixed service socket {}",
            socket.display(),
            fixed_socket.display()
        )));
    }
    let mut registry = Registry::open(config)?;
    let socket_text = socket.to_string_lossy().into_owned();
    let instance = registry.register_backend_instance(
        Backend::Wez,
        Some(&socket_text),
        Some(service_label),
    )?;
    let info = registry.backend_instance_info(instance)?;
    if info.backend != Backend::Wez || info.socket_path.as_deref() != Some(socket_text.as_str()) {
        return Err(RecoveryError::FenceLost(format!(
            "managed Wez instance {} is not registered to fixed socket {}",
            instance.0,
            socket.display()
        )));
    }
    Ok(instance)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotPublication {
    pub manifest_id: String,
    pub registry_revision: u64,
    pub destination: PathBuf,
}

/// Registry metadata handed to the in-process resurrection capture after the
/// snapshot helper owns the common backend fence.  It intentionally contains
/// no native IDs: Lua matches the durable opaque workspace key against its
/// own mux object graph and drops remote-only/missing windows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotSpaceDescriptor {
    pub space_uid: SpaceUid,
    pub space_no: u64,
    pub opaque_key: String,
    pub logical_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotCapturePlan {
    pub protocol_version: u32,
    pub manifest_id: String,
    pub backend_instance_uid: BackendInstanceUid,
    pub server_epoch: ServerEpoch,
    pub registry_revision: u64,
    pub generated_at: String,
    pub owner_domain: String,
    pub spaces: Vec<SnapshotSpaceDescriptor>,
}

#[derive(Debug, Clone)]
struct SnapshotNames {
    candidate: String,
    plan: String,
    published: String,
}

fn snapshot_names(candidate_id: &str, expected_epoch: ServerEpoch) -> Result<SnapshotNames> {
    let rest = candidate_id.strip_prefix(".capture-").ok_or_else(|| {
        RecoveryError::Protocol("snapshot candidate ID has no .capture- prefix".into())
    })?;
    if rest.len() < 39 || !rest.is_char_boundary(36) {
        return Err(RecoveryError::Protocol(
            "snapshot candidate ID is truncated".into(),
        ));
    }
    let epoch = &rest[..36];
    let suffix = rest[36..].strip_prefix('-').ok_or_else(|| {
        RecoveryError::Protocol("snapshot candidate ID omits timestamp separator".into())
    })?;
    let (unix, serial) = suffix
        .split_once('-')
        .ok_or_else(|| RecoveryError::Protocol("snapshot candidate ID omits serial".into()))?;
    let parsed_epoch = epoch
        .parse::<Uuid>()
        .map_err(|error| RecoveryError::Protocol(format!("snapshot candidate epoch: {error}")))?;
    let canonical_positive = |field: &str, value: &str| -> Result<u64> {
        let parsed = value.parse::<u64>().map_err(|_| {
            RecoveryError::Protocol(format!("snapshot candidate {field} is not an integer"))
        })?;
        if parsed == 0 || parsed.to_string() != value {
            return Err(RecoveryError::Protocol(format!(
                "snapshot candidate {field} is not canonical positive decimal"
            )));
        }
        Ok(parsed)
    };
    canonical_positive("timestamp", unix)?;
    canonical_positive("serial", serial)?;
    if parsed_epoch != expected_epoch.0 || parsed_epoch.to_string() != epoch {
        return Err(RecoveryError::Protocol(
            "snapshot candidate ID does not carry the exact canonical server epoch".into(),
        ));
    }
    Ok(SnapshotNames {
        candidate: candidate_id.to_string(),
        plan: format!("{candidate_id}.plan"),
        published: format!("manifest-{rest}.json"),
    })
}

/// Sidecar written only after the helper owns the snapshot fence.  The Lua
/// owner must not inspect/capture the mux before this document exists.
pub fn snapshot_capture_plan_path(candidate: &Path) -> PathBuf {
    let mut name = candidate.as_os_str().to_os_string();
    name.push(".plan");
    PathBuf::from(name)
}

/// Publish a complete candidate while holding the same backend-instance
/// kernel exclusion used by recovery and ordinary mutation.  A failed or
/// unfinished recovery remains ineligible even after its process died.
pub fn publish_snapshot_manifest(
    config: RegistryConfig,
    instance: BackendInstanceUid,
    candidate_id: &str,
    runtime_dir: &Path,
    server_epoch: ServerEpoch,
    server_pid: i64,
    server_start_token: &str,
) -> Result<SnapshotPublication> {
    let manifest_dir = production_recovery_manifest_dir()?;
    let authority = SnapshotAuthority {
        runtime_dir: runtime_dir.to_path_buf(),
        server_epoch,
        server_pid,
        server_start_token: server_start_token.to_string(),
    };
    verify_snapshot_authority(instance, &authority)?;
    publish_snapshot_manifest_inner(
        config,
        instance,
        candidate_id,
        &manifest_dir,
        server_epoch,
        Some(&authority),
    )
}

/// Explicit private-root seam for deterministic filesystem and snapshot
/// protocol tests. The production hidden command never calls this function.
#[doc(hidden)]
pub fn publish_snapshot_manifest_for_test(
    config: RegistryConfig,
    instance: BackendInstanceUid,
    candidate_id: &str,
    manifest_dir: &Path,
    server_epoch: ServerEpoch,
) -> Result<SnapshotPublication> {
    publish_snapshot_manifest_inner(
        config,
        instance,
        candidate_id,
        manifest_dir,
        server_epoch,
        None,
    )
}

#[derive(Debug, Clone)]
struct SnapshotAuthority {
    runtime_dir: PathBuf,
    server_epoch: ServerEpoch,
    server_pid: i64,
    server_start_token: String,
}

fn verify_snapshot_authority(
    instance: BackendInstanceUid,
    authority: &SnapshotAuthority,
) -> Result<()> {
    crate::runtime::verify_snapshot_service_authority(
        &authority.runtime_dir,
        instance.0,
        authority.server_epoch.0,
        authority.server_pid,
        &authority.server_start_token,
    )
    .map(|_| ())
    .map_err(|error| {
        RecoveryError::FenceLost(format!(
            "snapshot service authority was not proven: {error}"
        ))
    })
}

fn publish_snapshot_manifest_inner(
    config: RegistryConfig,
    instance: BackendInstanceUid,
    candidate_id: &str,
    manifest_dir: &Path,
    expected_epoch: ServerEpoch,
    authority: Option<&SnapshotAuthority>,
) -> Result<SnapshotPublication> {
    ensure_registry_only_environment()?;
    let names = snapshot_names(candidate_id, expected_epoch)?;
    let directory = open_private_leaf_dir(manifest_dir, true)?;
    for name in [&names.candidate, &names.plan] {
        match directory.read_file(name, MAX_RECOVERY_MANIFEST_BYTES) {
            Ok(_) => {
                return Err(RecoveryError::Protocol(format!(
                    "snapshot private entry {name} already exists before the capture fence"
                )));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let mut guard = InstanceLeaseGuard::acquire(
        config,
        instance,
        LeaseScope::Snapshot(instance),
        Uuid::new_v4(),
        DEFAULT_LEASE_TTL,
    )?;
    let result = (|| {
        guard.fence()?;
        if let Some(authority) = authority {
            verify_snapshot_authority(instance, authority)?;
        }
        let server = guard.registry.backend_server(instance)?;
        let epoch = server.server_epoch.ok_or_else(|| {
            RecoveryError::Failed(format!(
                "backend {} has no current server epoch",
                instance.0
            ))
        })?;
        if epoch != expected_epoch {
            return Err(RecoveryError::FenceLost(format!(
                "snapshot candidate epoch {} differs from registry epoch {}",
                expected_epoch.0, epoch.0
            )));
        }
        if guard
            .registry
            .unfinished_recovery_for_instance(instance)?
            .is_some()
        {
            return Err(RecoveryError::Failed(format!(
                "backend {} has unfinished recovery",
                instance.0
            )));
        }
        let head = guard.registry.authority_head()?;
        let mut spaces = Vec::new();
        for row in guard.registry.spaces()? {
            if row.backend_instance != instance
                || row.lifecycle != Lifecycle::Active
                || row.health != Health::Healthy
            {
                continue;
            }
            let Some(binding) = guard.registry.current_binding(row.space_uid)? else {
                continue;
            };
            spaces.push(SnapshotSpaceDescriptor {
                space_uid: row.space_uid,
                space_no: row.space_no.0.get(),
                opaque_key: binding.native_token,
                logical_name: row.logical_name,
            });
        }
        spaces.sort_by_key(|space| (space.space_no, space.space_uid.0));
        let plan = SnapshotCapturePlan {
            protocol_version: RECOVERY_PROTOCOL_VERSION,
            manifest_id: Uuid::new_v4().to_string(),
            backend_instance_uid: instance,
            server_epoch: epoch,
            registry_revision: head.revision,
            generated_at: now_rfc3339(),
            // The service-owned panes are local to the mux server.  Imported
            // client-domain panes are filtered before serialization.
            owner_domain: "local".into(),
            spaces,
        };
        let mut plan_bytes = serde_json::to_vec(&plan)?;
        plan_bytes.push(b'\n');
        directory.create_once(&names.plan, &plan_bytes, MAX_RECOVERY_MANIFEST_BYTES)?;

        let started = Instant::now();
        let manifest = loop {
            match directory.read_file(&names.candidate, MAX_RECOVERY_MANIFEST_BYTES) {
                Ok(bytes) => break serde_json::from_slice::<RecoveryManifest>(&bytes)?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            if started.elapsed() >= DEFAULT_REPLY_TIMEOUT {
                return Err(RecoveryError::TimedOut(format!(
                    "in-process owner did not publish snapshot candidate {}",
                    names.candidate
                )));
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        manifest.validate(instance)?;
        if manifest.manifest_id != plan.manifest_id
            || manifest.registry_revision != plan.registry_revision
        {
            return Err(RecoveryError::InvalidManifest(
                "candidate does not match its fenced capture plan".into(),
            ));
        }
        let planned = plan
            .spaces
            .iter()
            .map(|space| {
                (
                    space.space_uid,
                    space.space_no,
                    space.opaque_key.as_str(),
                    space.logical_name.as_str(),
                )
            })
            .collect::<Vec<_>>();
        let captured = manifest
            .spaces
            .iter()
            .map(|space| {
                (
                    space.space_uid,
                    space.space_no,
                    space.opaque_key.as_str(),
                    space.logical_name.as_str(),
                )
            })
            .collect::<Vec<_>>();
        if captured != planned {
            return Err(RecoveryError::InvalidManifest(
                "candidate is not the exact sorted all-Space capture plan".into(),
            ));
        }
        for space in &manifest.spaces {
            for (group_index, group) in space.window_state.tabs.iter().enumerate() {
                require_snapshot_owner_domain(
                    &group.pane_tree,
                    &plan.owner_domain,
                    &format!("space {} group {}", space.space_uid.0, group_index + 1),
                )?;
            }
        }
        guard.fence()?;
        let current = guard.registry.backend_server(instance)?;
        if current.server_epoch != Some(epoch) {
            return Err(RecoveryError::FenceLost(
                "backend server epoch changed during snapshot capture".into(),
            ));
        }
        let current_head = guard.registry.authority_head()?;
        if current_head != head {
            return Err(RecoveryError::FenceLost(
                "authority revision changed while snapshot fence was held".into(),
            ));
        }
        if manifest.registry_revision != head.revision {
            return Err(RecoveryError::InvalidManifest(format!(
                "candidate revision {} is not current authority revision {}",
                manifest.registry_revision, head.revision
            )));
        }
        let (eligible, diagnostics) = eligible_spaces(&guard.registry, &manifest)?;
        if eligible.spaces.len() != manifest.spaces.len() {
            return Err(RecoveryError::InvalidManifest(format!(
                "candidate contains ineligible Spaces: {}",
                diagnostics.join("; ")
            )));
        }
        let mut manifest_bytes = serde_json::to_vec(&manifest)?;
        manifest_bytes.push(b'\n');
        directory.publish_immutable(
            &names.published,
            &manifest_bytes,
            MAX_RECOVERY_MANIFEST_BYTES,
        )?;
        Ok(SnapshotPublication {
            manifest_id: manifest.manifest_id.clone(),
            registry_revision: manifest.registry_revision,
            destination: manifest_dir.join(&names.published),
        })
    })();
    // Snapshot failures do not create a resumable generation.  Release its
    // database scope on every exit; the kernel lock remains held until after
    // this transition.
    let cleanup_plan = directory.remove_file(&names.plan);
    let cleanup_candidate = directory.remove_file(&names.candidate);
    let release = guard.release();
    match (result, release, cleanup_plan, cleanup_candidate) {
        (Ok(value), Ok(()), Ok(_), Ok(_)) => Ok(value),
        (Err(error), _, _, _) => Err(error),
        (Ok(_), Err(error), _, _) => Err(error),
        (Ok(_), Ok(()), Err(error), _) | (Ok(_), Ok(()), Ok(_), Err(error)) => Err(error.into()),
    }
}

fn require_snapshot_owner_domain(split: &ManifestSplit, owner: &str, at: &str) -> Result<()> {
    if split.domain.as_deref() != Some(owner) {
        return Err(RecoveryError::InvalidManifest(format!(
            "snapshot pane at {at} has domain {:?}, expected {owner:?}",
            split.domain
        )));
    }
    if let Some(right) = &split.right {
        require_snapshot_owner_domain(right, owner, &format!("{at}/R"))?;
    }
    if let Some(bottom) = &split.bottom {
        require_snapshot_owner_domain(bottom, owner, &format!("{at}/B"))?;
    }
    Ok(())
}

fn ensure_registry_only_environment() -> Result<()> {
    for name in ["WEZTERM_UNIX_SOCKET", "WEZTERM_PANE", "TMUX", "TMUX_PANE"] {
        if std::env::var_os(name).is_some() {
            return Err(RecoveryError::Protocol(format!(
                "registry-only recovery helper inherited forbidden {name}"
            )));
        }
    }
    Ok(())
}

fn eligible_spaces(
    registry: &Registry,
    manifest: &RecoveryManifest,
) -> Result<(RecoveryManifest, Vec<String>)> {
    let mut eligible = manifest.clone();
    eligible.spaces.clear();
    let mut diagnostics = Vec::new();
    for space in &manifest.spaces {
        let row = match registry.space(space.space_uid) {
            Ok(row) => row,
            Err(RegistryError::NotFound { .. }) => {
                diagnostics.push(format!("{} is not registered", space.space_uid.0));
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        if row.backend_instance != manifest.backend_instance_uid
            || row.lifecycle != Lifecycle::Active
            || row.health != Health::Healthy
            || row.space_no.0.get() != space.space_no
            || row.logical_name != space.logical_name
        {
            diagnostics.push(format!(
                "{} is not an active healthy exact registry match",
                space.space_uid.0
            ));
            continue;
        }
        let Some(binding) = registry.current_binding(space.space_uid)? else {
            diagnostics.push(format!("{} has no current binding", space.space_uid.0));
            continue;
        };
        if binding.native_token != space.opaque_key {
            diagnostics.push(format!(
                "{} opaque key does not match its binding",
                space.space_uid.0
            ));
            continue;
        }
        eligible.spaces.push(space.clone());
    }
    Ok((eligible, diagnostics))
}

// -------------------------------------------------------------------------
// Long-lived recovery coordinator

#[derive(Debug, Clone)]
pub struct RecoveryCoordinatorOptions {
    pub registry: RegistryConfig,
    pub runtime_dir: PathBuf,
    pub manifest_dir: PathBuf,
    pub backend_instance: BackendInstanceUid,
    pub server_epoch: ServerEpoch,
    pub server_pid: i64,
    pub server_start_token: String,
    pub helper_bin: String,
    pub default_program: Vec<String>,
    pub request_uid: Uuid,
    pub lease_ttl: Duration,
    pub reply_timeout: Duration,
    /// `false` for automatic mux-startup.  A failed generation requires the
    /// explicit public `recovery resume` path to set this true.
    pub resume_failed: bool,
    /// Explicit public abort of a failed generation.  Native nodes are
    /// removed in reverse dependency order before the atomic journal abort.
    pub abort_failed: bool,
    /// Test-only hard-crash injection.  The service CLI never exposes this;
    /// focused recovery tests use it to model process death without running
    /// handled-error cleanup or releasing the durable lease row.
    #[doc(hidden)]
    pub crash_point: Option<RecoveryCrashPoint>,
    /// When paired with `crash_point`, publish this marker and block forever
    /// instead of unwinding.  A parent test process then sends SIGKILL, which
    /// exercises real OS lock release and durable-lease takeover semantics.
    #[doc(hidden)]
    pub hard_stop_path: Option<PathBuf>,
    /// Tests run an in-process mux under a scratch runtime and therefore
    /// cannot be the direct child of the fixed production service. Production
    /// construction is secure by default; only focused harnesses set this.
    #[doc(hidden)]
    pub skip_service_authority: bool,
    /// Deterministic test seam for a mux descriptor/socket/process witness
    /// changing after the initial child proof but before registry publish.
    /// Production construction always leaves this false.
    #[doc(hidden)]
    pub fail_service_authority_after_lock: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[doc(hidden)]
pub enum RecoveryCrashPhase {
    AfterCommandPublish,
    AfterResponseRead,
    AfterBootstrapAck,
    AfterRootCompleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[doc(hidden)]
pub struct RecoveryCrashPoint {
    pub phase: RecoveryCrashPhase,
    /// `inspect`, `prepare`, `verify`, `restore:<manifest-path>`, or
    /// `ack:<manifest-path>`.  Ignored for `AfterRootCompleted`.
    pub action: String,
}

impl RecoveryCoordinatorOptions {
    #[allow(clippy::too_many_arguments)] // service CLI fields are deliberately explicit
    pub fn new(
        registry: RegistryConfig,
        runtime_dir: PathBuf,
        manifest_dir: PathBuf,
        backend_instance: BackendInstanceUid,
        server_epoch: ServerEpoch,
        server_pid: i64,
        server_start_token: String,
        helper_bin: String,
    ) -> Self {
        RecoveryCoordinatorOptions {
            registry,
            runtime_dir,
            manifest_dir,
            backend_instance,
            server_epoch,
            server_pid,
            server_start_token,
            helper_bin,
            default_program: vec!["/bin/sh".into(), "-l".into()],
            request_uid: Uuid::new_v4(),
            lease_ttl: DEFAULT_LEASE_TTL,
            reply_timeout: DEFAULT_REPLY_TIMEOUT,
            resume_failed: false,
            abort_failed: false,
            crash_point: None,
            hard_stop_path: None,
            skip_service_authority: false,
            fail_service_authority_after_lock: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryOutcome {
    Restored,
    Resumed,
    AlreadyReady,
    Aborted,
    NoEligibleManifest,
    NoEligibleSpaces,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryRunReport {
    pub outcome: RecoveryOutcome,
    pub generation_uid: Option<Uuid>,
    pub manifest_id: Option<String>,
    pub restored_nodes: usize,
    pub diagnostics: Vec<String>,
}

struct FileDriver<'a> {
    guard: &'a mut InstanceLeaseGuard,
    spool: &'a RecoverySpool,
    generation_uid: Uuid,
    coordinator_uid: Uuid,
    sequence: u64,
    timeout: Duration,
    crash_point: Option<RecoveryCrashPoint>,
    hard_stop_path: Option<PathBuf>,
}

impl FileDriver<'_> {
    fn exchange(&mut self, action: RecoveryAction) -> Result<RecoveryResponse> {
        let action_label = recovery_action_label(&action);
        let fencing_token = self.guard.fence()?;
        self.sequence += 1;
        let command = RecoveryCommand {
            protocol_version: RECOVERY_PROTOCOL_VERSION,
            coordinator_uid: self.coordinator_uid,
            generation_uid: self.generation_uid,
            sequence: self.sequence,
            fencing_token,
            action,
        };
        self.spool.remove(RecoverySpoolFile::Response)?;
        self.spool.write(RecoverySpoolFile::Command, &command)?;
        self.crash_if(RecoveryCrashPhase::AfterCommandPublish, &action_label);
        let response = wait_for_response(self.spool, &command, self.timeout)?;
        self.crash_if(RecoveryCrashPhase::AfterResponseRead, &action_label);
        if !response.ok {
            return Err(RecoveryError::Failed(response.error.unwrap_or_else(|| {
                format!("Lua rejected sequence {}", command.sequence)
            })));
        }
        self.spool.remove(RecoverySpoolFile::Command)?;
        self.spool.remove(RecoverySpoolFile::Response)?;
        Ok(response)
    }

    fn crash_if(&self, phase: RecoveryCrashPhase, action: &str) {
        if self.crash_point.as_ref().is_some_and(|point| {
            point.phase == phase
                && (phase == RecoveryCrashPhase::AfterRootCompleted || point.action == action)
        }) {
            if let Some(path) = &self.hard_stop_path {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)
                        .expect("create hard-stop marker parent for recovery test");
                }
                let mut marker = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .mode(0o600)
                    .open(path)
                    .expect("publish recovery hard-stop marker");
                writeln!(marker, "{} {phase:?} {action}", std::process::id())
                    .expect("write recovery hard-stop marker");
                marker.sync_all().expect("sync recovery hard-stop marker");
                loop {
                    std::thread::park_timeout(Duration::from_secs(60));
                }
            }
            panic!("injected hard recovery crash at {phase:?}/{action}");
        }
    }
}

fn recovery_action_label(action: &RecoveryAction) -> String {
    match action {
        RecoveryAction::Inspect => "inspect".into(),
        RecoveryAction::Prepare { .. } => "prepare".into(),
        RecoveryAction::CompareAndRestoreNode { node, .. } => {
            format!("restore:{}", node.manifest_node_path)
        }
        RecoveryAction::CompareAndRemoveNode {
            manifest_node_path, ..
        } => format!("remove:{manifest_node_path}"),
        RecoveryAction::Verify { .. } => "verify".into(),
    }
}

fn verify_service_authority(
    options: &RecoveryCoordinatorOptions,
) -> Result<Option<crate::runtime::VerifiedWezServiceIdentity>> {
    if options.skip_service_authority {
        return Ok(None);
    }
    let fixed_manifests = production_recovery_manifest_dir()?;
    if options.manifest_dir != fixed_manifests {
        return Err(RecoveryError::FenceLost(format!(
            "recovery manifest root {} is not fixed dmux state root {}",
            options.manifest_dir.display(),
            fixed_manifests.display()
        )));
    }
    crate::runtime::verify_recovery_service_authority(
        &options.runtime_dir,
        options.backend_instance.0,
        options.server_epoch.0,
        options.server_pid,
        &options.server_start_token,
    )
    .map(Some)
    .map_err(|error| {
        RecoveryError::FenceLost(format!(
            "recovery service authority was not proven: {error}"
        ))
    })
}

pub fn run_recovery_coordinator(options: RecoveryCoordinatorOptions) -> Result<RecoveryRunReport> {
    if !options.skip_service_authority
        && (options.crash_point.is_some()
            || options.hard_stop_path.is_some()
            || options.fail_service_authority_after_lock)
    {
        return Err(RecoveryError::Protocol(
            "recovery crash/authority injection seams require the explicit test authority bypass"
                .into(),
        ));
    }
    verify_service_authority(&options)?;
    let spool = RecoverySpool::new(&options.runtime_dir, options.server_epoch);
    let result = run_recovery_coordinator_inner(options.clone(), &spool);
    if let Err(error) = &result {
        // A handled error must become observable immediately.  Hard process
        // death deliberately cannot execute this path; the stale in-progress
        // status is what makes Lua start a fenced takeover helper.
        let prior = spool.read::<RecoveryStatus>(RecoverySpoolFile::Status).ok();
        if !prior
            .as_ref()
            .is_some_and(|status| status.state == RecoveryStatusState::Failed)
        {
            let _ = spool.prepare();
            let _ = write_status(
                &spool,
                RecoveryStatus {
                    protocol_version: RECOVERY_PROTOCOL_VERSION,
                    state: RecoveryStatusState::Failed,
                    backend_instance_uid: options.backend_instance,
                    server_epoch: options.server_epoch,
                    coordinator_uid: options.request_uid,
                    generation_uid: prior.as_ref().and_then(|status| status.generation_uid),
                    manifest_id: prior.and_then(|status| status.manifest_id),
                    fencing_token: None,
                    current_node: None,
                    error: Some(error.to_string()),
                    updated_at: now_rfc3339(),
                },
            );
        }
    }
    result
}

fn run_recovery_coordinator_inner(
    options: RecoveryCoordinatorOptions,
    spool: &RecoverySpool,
) -> Result<RecoveryRunReport> {
    ensure_registry_only_environment()?;
    if options.resume_failed && options.abort_failed {
        return Err(RecoveryError::Protocol(
            "recovery resume and abort modes are mutually exclusive".into(),
        ));
    }
    if options.default_program.is_empty() {
        return Err(RecoveryError::Protocol(
            "recovery default program is empty".into(),
        ));
    }
    if !Path::new(&options.helper_bin).is_absolute() {
        return Err(RecoveryError::Protocol(
            "pane-bootstrap path must be absolute".into(),
        ));
    }
    spool.prepare()?;

    let mut guard = InstanceLeaseGuard::acquire(
        options.registry.clone(),
        options.backend_instance,
        LeaseScope::Recovery(options.backend_instance),
        options.request_uid,
        options.lease_ttl,
    )?;
    // The initial proof prevents arbitrary hidden-command invocations from
    // touching recovery state. Repeat it while holding the exact backend
    // fence: a legitimate coordinator can wait here long enough for its mux
    // parent/socket/descriptor incarnation to die and be replaced.
    let authority = if options.fail_service_authority_after_lock {
        Err(RecoveryError::FenceLost(
            "recovery service authority changed before registry publish (injected)".into(),
        ))
    } else {
        verify_service_authority(&options)
    };
    let authority = match authority {
        Ok(authority) => authority,
        Err(error) => {
            guard.release()?;
            return Err(error);
        }
    };
    publish_incarnation_if_needed(&mut guard, &options, authority.as_ref())?;

    // Two service/startup clients may race for one fresh server.  The second
    // must not delete the first coordinator's command while blocked on the
    // instance lock, nor attempt a blind restore after the first published
    // ready.  The ready sidecar is trusted only for this exact incarnation
    // and only when the registry has no unfinished generation.
    if !options.abort_failed
        && let Ok(prior) = spool.read::<RecoveryStatus>(RecoverySpoolFile::Status)
        && prior.protocol_version == RECOVERY_PROTOCOL_VERSION
        && prior.state == RecoveryStatusState::Ready
        && prior.backend_instance_uid == options.backend_instance
        && prior.server_epoch == options.server_epoch
        && guard
            .registry
            .unfinished_recovery_for_instance(options.backend_instance)?
            .is_none()
    {
        let generation_uid = prior.generation_uid;
        let manifest_id = prior.manifest_id;
        guard.release()?;
        return Ok(RecoveryRunReport {
            outcome: RecoveryOutcome::AlreadyReady,
            generation_uid,
            manifest_id,
            restored_nodes: 0,
            diagnostics: vec!["exact server incarnation was already recovery-ready".into()],
        });
    }
    spool.clear_messages()?;

    let mut unfinished = guard
        .registry
        .unfinished_recovery_for_instance(options.backend_instance)?;
    let completed = if unfinished.is_none() {
        guard
            .registry
            .completed_recovery(options.backend_instance, options.server_epoch)?
    } else {
        None
    };
    let mut generation_uid = unfinished
        .as_ref()
        .map(|(spec, _)| spec.generation_uid)
        .or_else(|| completed.as_ref().map(|(spec, _)| spec.generation_uid))
        .unwrap_or_else(Uuid::new_v4);
    let mut driver = FileDriver {
        guard: &mut guard,
        spool: &spool,
        generation_uid,
        coordinator_uid: options.request_uid,
        sequence: 0,
        timeout: options.reply_timeout,
        crash_point: options.crash_point.clone(),
        hard_stop_path: options.hard_stop_path.clone(),
    };

    write_status(
        &spool,
        RecoveryStatus {
            protocol_version: RECOVERY_PROTOCOL_VERSION,
            state: RecoveryStatusState::Starting,
            backend_instance_uid: options.backend_instance,
            server_epoch: options.server_epoch,
            coordinator_uid: options.request_uid,
            generation_uid: unfinished
                .as_ref()
                .or(completed.as_ref())
                .map(|(spec, _)| spec.generation_uid),
            manifest_id: unfinished
                .as_ref()
                .or(completed.as_ref())
                .map(|(spec, _)| spec.manifest_id.clone()),
            fencing_token: Some(driver.guard.lease.fencing_token),
            current_node: None,
            error: None,
            updated_at: now_rfc3339(),
        },
    )?;

    let mut restart_diagnostics = Vec::new();
    if unfinished
        .as_ref()
        .is_some_and(|(spec, _)| spec.server_epoch != options.server_epoch)
    {
        let (stale_spec, stale_rows) = unfinished.as_ref().cloned().ok_or_else(|| {
            RecoveryError::Protocol("stale generation disappeared before reconciliation".into())
        })?;
        let inspected = driver.exchange(RecoveryAction::Inspect)?;
        let snapshot = inspected.snapshot.ok_or_else(|| {
            RecoveryError::Protocol(
                "restart reconciliation inspect omitted complete native snapshot".into(),
            )
        })?;
        snapshot.require_sentinel_only(options.server_epoch)?;
        let root_state = stale_rows
            .iter()
            .find(|row| row.manifest_node_path == GENERATION_ROOT_PATH)
            .ok_or_else(|| RecoveryError::Protocol("stale generation root is missing".into()))?
            .node_state;
        if root_state == RecoveryNodeState::Failed
            && !options.resume_failed
            && !options.abort_failed
        {
            return Err(RecoveryError::Failed(format!(
                "stale generation {} is failed; explicit recovery resume or abort is required",
                stale_spec.generation_uid
            )));
        }
        abort_stale_bootstraps(
            &mut driver.guard.registry,
            &snapshot,
            &stale_spec,
            &stale_rows,
        )?;

        driver.guard.fence()?;
        if options.abort_failed {
            let backend_scope = LockScope::BackendInstance(options.backend_instance);
            let (registry, locks) = (&mut driver.guard.registry, &driver.guard.locks);
            let kernel = locks.held(&backend_scope).ok_or_else(|| {
                RecoveryError::FenceLost("backend kernel lock vanished during abort".into())
            })?;
            registry.abort_recovery_generation_and_record_current_empty(
                stale_spec.generation_uid,
                root_state,
                options.server_epoch,
                kernel,
                &driver.guard.lease,
            )?;
            write_aborted_status(
                &spool,
                &options,
                &stale_spec,
                driver.guard.lease.fencing_token,
            )?;
            driver.guard.release_on_drop = true;
            driver
                .guard
                .registry
                .release_lease(&driver.guard.scope, driver.guard.holder.request_uid)?;
            return Ok(RecoveryRunReport {
                outcome: RecoveryOutcome::Aborted,
                generation_uid: Some(stale_spec.generation_uid),
                manifest_id: Some(stale_spec.manifest_id),
                restored_nodes: 0,
                diagnostics: vec![
                    "stale-epoch failed recovery was proven absent and intentionally emptied"
                        .into(),
                ],
            });
        }

        driver.guard.registry.abort_stale_recovery_generation(
            stale_spec.generation_uid,
            root_state,
            options.server_epoch,
            &driver.guard.lease,
        )?;
        restart_diagnostics.push(format!(
            "retired stale recovery generation {} from server epoch {} after sentinel-only proof",
            stale_spec.generation_uid, stale_spec.server_epoch.0
        ));
        unfinished = None;
        generation_uid = Uuid::new_v4();
        driver.generation_uid = generation_uid;
        write_status(
            &spool,
            RecoveryStatus {
                protocol_version: RECOVERY_PROTOCOL_VERSION,
                state: RecoveryStatusState::Starting,
                backend_instance_uid: options.backend_instance,
                server_epoch: options.server_epoch,
                coordinator_uid: options.request_uid,
                generation_uid: None,
                manifest_id: None,
                fencing_token: Some(driver.guard.lease.fencing_token),
                current_node: None,
                error: None,
                updated_at: now_rfc3339(),
            },
        )?;
    }

    if options.abort_failed {
        return run_recovery_abort(&mut driver, &spool, &options, unfinished);
    }

    if let Some((completed_spec, completed_rows)) = completed {
        let inspected = driver.exchange(RecoveryAction::Inspect)?;
        let snapshot = inspected.snapshot.ok_or_else(|| {
            RecoveryError::Protocol(
                "completed-generation inspect omitted complete native snapshot".into(),
            )
        })?;
        let floor = driver
            .guard
            .registry
            .intentional_empty_revision(options.backend_instance)?;
        let manifest = load_eligible_manifest_by_id(
            &options.manifest_dir,
            options.backend_instance,
            floor,
            &completed_spec.manifest_id,
        )?
        .ok_or_else(|| {
            RecoveryError::InvalidManifest(format!(
                "completed generation {} exact manifest {} is missing, corrupt, or ineligible",
                completed_spec.generation_uid, completed_spec.manifest_id
            ))
        })?;
        let nodes = manifest.restore_nodes();
        verify_completed_generation_snapshot(
            &driver.guard.registry,
            &snapshot,
            &completed_spec,
            &completed_rows,
            &nodes,
        )?;
        write_ready_status(
            &spool,
            &options,
            Some(completed_spec.generation_uid),
            Some(completed_spec.manifest_id.clone()),
            driver.guard.lease.fencing_token,
        )?;
        driver.guard.release_on_drop = true;
        driver
            .guard
            .registry
            .release_lease(&driver.guard.scope, driver.guard.holder.request_uid)?;
        return Ok(RecoveryRunReport {
            outcome: RecoveryOutcome::AlreadyReady,
            generation_uid: Some(completed_spec.generation_uid),
            manifest_id: Some(completed_spec.manifest_id),
            restored_nodes: 0,
            diagnostics: vec![
                "completed generation tree was verified and readiness republished".into(),
            ],
        });
    }

    let inspected = driver.exchange(RecoveryAction::Inspect)?;
    let snapshot = inspected.snapshot.ok_or_else(|| {
        RecoveryError::Protocol("inspect response omitted complete native snapshot".into())
    })?;
    snapshot.validate_complete(options.server_epoch)?;

    let floor = driver
        .guard
        .registry
        .intentional_empty_revision(options.backend_instance)?;
    let selected = if let Some((spec, _)) = &unfinished {
        (
            load_eligible_manifest_by_id(
                &options.manifest_dir,
                options.backend_instance,
                floor,
                &spec.manifest_id,
            ),
            Vec::new(),
        )
    } else {
        let selected =
            newest_eligible_manifest(&options.manifest_dir, options.backend_instance, floor);
        match selected {
            Ok((manifest, diagnostics)) => (Ok(manifest), diagnostics),
            Err(error) => (Err(error), Vec::new()),
        }
    };
    let (manifest, mut diagnostics) = match selected {
        (Ok(manifest), diagnostics) => (manifest, diagnostics),
        (Err(error), _) => {
            if let Some((spec, _)) = &unfinished {
                return fail_generation(
                    &mut driver,
                    &spool,
                    &options,
                    spec,
                    format!("unfinished generation manifest cannot be loaded: {error}"),
                );
            }
            return Err(error);
        }
    };
    diagnostics.splice(0..0, restart_diagnostics);

    let Some(manifest) = manifest else {
        if let Some((spec, _)) = &unfinished {
            return fail_generation(
                &mut driver,
                &spool,
                &options,
                spec,
                format!(
                    "unfinished generation manifest {} is missing, corrupt, or ineligible",
                    spec.manifest_id
                ),
            );
        }
        snapshot.require_sentinel_only(options.server_epoch)?;
        write_ready_status(
            &spool,
            &options,
            None,
            None,
            driver.guard.lease.fencing_token,
        )?;
        driver.guard.release_on_drop = true;
        driver
            .guard
            .registry
            .release_lease(&driver.guard.scope, driver.guard.holder.request_uid)?;
        return Ok(RecoveryRunReport {
            outcome: RecoveryOutcome::NoEligibleManifest,
            generation_uid: None,
            manifest_id: None,
            restored_nodes: 0,
            diagnostics,
        });
    };

    let (manifest, skipped) = match eligible_spaces(&driver.guard.registry, &manifest) {
        Ok(value) => value,
        Err(error) => {
            if let Some((spec, _)) = &unfinished {
                return fail_generation(
                    &mut driver,
                    &spool,
                    &options,
                    spec,
                    format!("unfinished generation eligibility check failed: {error}"),
                );
            }
            return Err(error);
        }
    };
    diagnostics.extend(skipped);
    if manifest.spaces.is_empty() {
        if let Some((spec, _)) = &unfinished {
            return fail_generation(
                &mut driver,
                &spool,
                &options,
                spec,
                "unfinished generation has no remaining eligible Spaces".into(),
            );
        }
        snapshot.require_sentinel_only(options.server_epoch)?;
        write_ready_status(
            &spool,
            &options,
            None,
            Some(manifest.manifest_id.clone()),
            driver.guard.lease.fencing_token,
        )?;
        driver.guard.release_on_drop = true;
        driver
            .guard
            .registry
            .release_lease(&driver.guard.scope, driver.guard.holder.request_uid)?;
        return Ok(RecoveryRunReport {
            outcome: RecoveryOutcome::NoEligibleSpaces,
            generation_uid: None,
            manifest_id: Some(manifest.manifest_id),
            restored_nodes: 0,
            diagnostics,
        });
    }
    let nodes = manifest.restore_nodes();
    let node_specs = nodes
        .iter()
        .map(|node| RecoveryNodeSpec {
            space_uid: Some(node.space_uid),
            manifest_node_path: node.manifest_node_path.clone(),
        })
        .collect::<Vec<_>>();
    let spec = RecoveryGenerationSpec {
        generation_uid,
        backend_instance: options.backend_instance,
        server_epoch: options.server_epoch,
        manifest_id: manifest.manifest_id.clone(),
    };

    let resumed = if let Some((old, rows)) = unfinished {
        if old.manifest_id != spec.manifest_id {
            return fail_generation(
                &mut driver,
                &spool,
                &options,
                &spec,
                format!(
                    "unfinished generation manifest {} is not available as {}",
                    old.manifest_id, spec.manifest_id
                ),
            );
        }
        if let Err(error) =
            validate_recovery_snapshot(&driver.guard.registry, &snapshot, &spec, &rows)
        {
            return fail_generation(
                &mut driver,
                &spool,
                &options,
                &spec,
                format!("unfinished generation native reconciliation failed: {error}"),
            );
        }
        true
    } else {
        snapshot.require_sentinel_only(options.server_epoch)?;
        false
    };

    let begin = driver
        .guard
        .registry
        .begin_recovery(&spec, &node_specs, &driver.guard.lease)?;
    if resumed && !matches!(begin, BeginRecovery::Replay(_)) {
        return fail_generation(
            &mut driver,
            &spool,
            &options,
            &spec,
            "unfinished recovery did not replay its exact journal".into(),
        );
    }
    let run_result = (|| -> Result<()> {
        // From the first durable root transition onward, every handled
        // failure must leave a failed root that public resume/abort can see.
        // Keep Prepare/status/native-map setup inside this guarded region;
        // outer error reporting alone is not durable recovery state.
        advance_generation_to_restoring(&mut driver, &spec, options.resume_failed)?;
        write_recovering_status(
            &spool,
            &options,
            &spec,
            driver.guard.lease.fencing_token,
            None,
        )?;
        driver.exchange(RecoveryAction::Prepare {
            nodes: nodes.clone(),
        })?;
        let mut created = completed_native_map(&driver.guard.registry, &snapshot, &spec)?;
        for node in &nodes {
            write_recovering_status(
                &spool,
                &options,
                &spec,
                driver.guard.lease.fencing_token,
                Some(node.manifest_node_path.clone()),
            )?;
            restore_one_node(&mut driver, &options, &spec, &nodes, node, &mut created)?;
        }
        let verified = driver.exchange(RecoveryAction::Verify {
            expected_nodes: nodes.len(),
        })?;
        let final_snapshot = verified.snapshot.ok_or_else(|| {
            RecoveryError::Protocol("verify response omitted native snapshot".into())
        })?;
        verify_final_snapshot(&final_snapshot, options.server_epoch, &nodes, &created)?;
        driver.guard.fence()?;
        driver.guard.registry.transition_recovery_node(
            spec.generation_uid,
            GENERATION_ROOT_PATH,
            RecoveryNodeState::Restoring,
            RecoveryNodeState::Completed,
            None,
            &driver.guard.lease,
        )?;
        driver.crash_if(RecoveryCrashPhase::AfterRootCompleted, "root");
        Ok(())
    })();

    if let Err(error) = run_result {
        let message = error.to_string();
        let _ = mark_generation_failed(&mut driver, &spec);
        let _ = write_failed_status(&spool, &options, Some(&spec), &message);
        return Err(error);
    }

    write_ready_status(
        &spool,
        &options,
        Some(spec.generation_uid),
        Some(spec.manifest_id.clone()),
        driver.guard.lease.fencing_token,
    )?;
    driver.guard.release_on_drop = true;
    driver
        .guard
        .registry
        .release_lease(&driver.guard.scope, driver.guard.holder.request_uid)?;
    Ok(RecoveryRunReport {
        outcome: if resumed {
            RecoveryOutcome::Resumed
        } else {
            RecoveryOutcome::Restored
        },
        generation_uid: Some(spec.generation_uid),
        manifest_id: Some(spec.manifest_id),
        restored_nodes: nodes.len(),
        diagnostics,
    })
}

fn run_recovery_abort(
    driver: &mut FileDriver<'_>,
    spool: &RecoverySpool,
    options: &RecoveryCoordinatorOptions,
    unfinished: Option<(RecoveryGenerationSpec, Vec<RecoveryJournalRow>)>,
) -> Result<RecoveryRunReport> {
    let Some((spec, mut rows)) = unfinished else {
        return Err(RecoveryError::Failed(
            "backend has no unfinished recovery generation to abort".into(),
        ));
    };
    let root = rows
        .iter()
        .find(|row| row.manifest_node_path == GENERATION_ROOT_PATH)
        .ok_or_else(|| RecoveryError::Protocol("generation root row is missing".into()))?;
    if root.node_state != RecoveryNodeState::Failed {
        return Err(RecoveryError::Failed(format!(
            "generation {} is {}, not failed",
            spec.generation_uid,
            root.node_state.as_str()
        )));
    }

    write_recovering_status(
        spool,
        options,
        &spec,
        driver.guard.lease.fencing_token,
        Some("@abort".into()),
    )?;
    let inspected = driver.exchange(RecoveryAction::Inspect)?;
    let mut snapshot = inspected.snapshot.ok_or_else(|| {
        RecoveryError::Protocol("abort inspect omitted complete native snapshot".into())
    })?;
    validate_recovery_snapshot(&driver.guard.registry, &snapshot, &spec, &rows)?;

    // Children before parents; for equal depth group 2 sorts before group 1,
    // leaving the Space root until every sibling tab and descendant split is
    // gone.  This ordering needs no manifest file, so abort remains possible
    // even when the corrupt/missing manifest caused the failure.
    rows.sort_by(|left, right| {
        (right.manifest_node_path.len(), &right.manifest_node_path)
            .cmp(&(left.manifest_node_path.len(), &left.manifest_node_path))
    });
    let mut removed_nodes = 0usize;
    for row in rows
        .iter()
        .filter(|row| row.manifest_node_path != GENERATION_ROOT_PATH)
    {
        let Some(request_uid) = row.bootstrap_request_uid else {
            continue;
        };
        validate_bootstrap_identity(
            &driver.guard.registry,
            request_uid,
            &spec,
            &row.manifest_node_path,
        )?;
        let request = driver
            .guard
            .registry
            .bootstrap_request(request_uid)?
            .ok_or_else(|| RecoveryError::Protocol(format!("bootstrap {request_uid} missing")))?;
        // Each deletion is native-ID-dependent. Refresh the complete tree on
        // the current fence immediately before resolving those IDs so a
        // moved target or an out-of-band pane cannot be acted on from the
        // older initial/post-previous snapshot.
        let inspected = driver.exchange(RecoveryAction::Inspect)?;
        snapshot = inspected.snapshot.ok_or_else(|| {
            RecoveryError::Protocol("pre-remove inspect omitted complete native snapshot".into())
        })?;
        validate_recovery_snapshot(&driver.guard.registry, &snapshot, &spec, &rows)?;
        let recorded = request
            .returned_native_ids
            .as_deref()
            .map(serde_json::from_str::<CreatedNode>)
            .transpose()?;
        let live = native_for_recovery_request(&snapshot, request_uid)?;
        if let (Some(recorded), Some(live)) = (&recorded, &live)
            && (recorded.window_id != live.window_id
                || recorded.tab_id != live.tab_id
                || recorded.pane_id != live.pane_id)
        {
            return Err(RecoveryError::Protocol(format!(
                "bootstrap {request_uid} recorded IDs disagree with its live token"
            )));
        }
        let target = live.or(recorded);
        if let Some(target) = target {
            // Validate every recorded/native identifier before Lua receives
            // deletion authority.  A corrupt noncanonical ID must never be
            // coerced by Lua's `tonumber` into a different live resource.
            parse_native_id(&target.pane_id, "pane ID")?;
            parse_native_id(&target.tab_id, "tab ID")?;
            parse_native_id(&target.window_id, "window ID")?;
            let response = driver.exchange(RecoveryAction::CompareAndRemoveNode {
                manifest_node_path: row.manifest_node_path.clone(),
                pane_id: target.pane_id.clone(),
                tab_id: target.tab_id.clone(),
                window_id: target.window_id.clone(),
                expected_tree: snapshot.tree_precondition(),
            })?;
            let removed = response.removed.ok_or_else(|| {
                RecoveryError::Protocol(format!(
                    "abort response for {} omitted removal witness",
                    row.manifest_node_path
                ))
            })?;
            validate_removed_node(&removed, &target)?;
            let checked = driver.exchange(RecoveryAction::Inspect)?;
            snapshot = checked.snapshot.ok_or_else(|| {
                RecoveryError::Protocol("post-remove inspect omitted native snapshot".into())
            })?;
            validate_recovery_snapshot(&driver.guard.registry, &snapshot, &spec, &rows)?;
            if snapshot
                .panes()
                .any(|(_, _, pane)| pane.pane_id == target.pane_id)
            {
                return Err(RecoveryError::InvalidSnapshot(format!(
                    "aborted pane {} is still present",
                    target.pane_id
                )));
            }
            removed_nodes += 1;
        }
        if !crate::registry::bootstrap_is_terminal(request.state) {
            boot(
                driver
                    .guard
                    .registry
                    .bootstrap_state(request_uid, BootstrapState::Aborted),
            )?;
        }
    }

    snapshot.require_sentinel_only(options.server_epoch)?;
    driver.guard.fence()?;
    let backend_scope = LockScope::BackendInstance(options.backend_instance);
    let (registry, locks) = (&mut driver.guard.registry, &driver.guard.locks);
    let kernel = locks.held(&backend_scope).ok_or_else(|| {
        RecoveryError::FenceLost("backend kernel lock vanished during abort".into())
    })?;
    registry.abort_recovery_generation_and_record_current_empty(
        spec.generation_uid,
        RecoveryNodeState::Failed,
        options.server_epoch,
        kernel,
        &driver.guard.lease,
    )?;
    write_aborted_status(spool, options, &spec, driver.guard.lease.fencing_token)?;
    driver.guard.release_on_drop = true;
    driver
        .guard
        .registry
        .release_lease(&driver.guard.scope, driver.guard.holder.request_uid)?;
    Ok(RecoveryRunReport {
        outcome: RecoveryOutcome::Aborted,
        generation_uid: Some(spec.generation_uid),
        manifest_id: Some(spec.manifest_id),
        restored_nodes: removed_nodes,
        diagnostics: vec!["failed recovery was fenced, removed, and intentionally emptied".into()],
    })
}

fn abort_stale_bootstraps(
    registry: &mut Registry,
    snapshot: &NativeSnapshot,
    spec: &RecoveryGenerationSpec,
    rows: &[RecoveryJournalRow],
) -> Result<()> {
    for row in rows
        .iter()
        .filter(|row| row.manifest_node_path != GENERATION_ROOT_PATH)
    {
        let Some(request_uid) = row.bootstrap_request_uid else {
            continue;
        };
        validate_bootstrap_identity(registry, request_uid, spec, &row.manifest_node_path)?;
        if native_for_recovery_request(snapshot, request_uid)?.is_some() {
            return Err(RecoveryError::NonEmpty(format!(
                "stale generation bootstrap {request_uid} is still visible in the current mux"
            )));
        }
        let request = registry
            .bootstrap_request(request_uid)?
            .ok_or_else(|| RecoveryError::Protocol(format!("bootstrap {request_uid} missing")))?;
        if !crate::registry::bootstrap_is_terminal(request.state) {
            boot(registry.bootstrap_state(request_uid, BootstrapState::Aborted))?;
        }
    }
    Ok(())
}

fn native_for_recovery_request(
    snapshot: &NativeSnapshot,
    request_uid: Uuid,
) -> Result<Option<CreatedNode>> {
    let reserved = bootstrap::reserved_title(request_uid);
    let running = bootstrap::run_title(request_uid);
    let matches = snapshot
        .panes()
        .filter(|(_, _, pane)| pane.title == reserved || pane.title == running)
        .map(|(window, tab, pane)| CreatedNode {
            window_id: window.window_id.clone(),
            tab_id: tab.tab_id.clone(),
            pane_id: pane.pane_id.clone(),
            titled_pane_ids: vec![pane.pane_id.clone()],
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(one.clone())),
        many => Err(RecoveryError::Protocol(format!(
            "recovery request {request_uid} labels {} panes",
            many.len()
        ))),
    }
}

fn parse_native_id(value: &str, label: &str) -> Result<u64> {
    let parsed = value.parse::<u64>().map_err(|_| {
        RecoveryError::Protocol(format!("{label} {value:?} is not a native decimal ID"))
    })?;
    if parsed.to_string() != value {
        return Err(RecoveryError::Protocol(format!(
            "{label} {value:?} is not canonical decimal"
        )));
    }
    Ok(parsed)
}

fn validate_removed_node(removed: &RemovedNode, target: &CreatedNode) -> Result<()> {
    let pane = parse_native_id(&target.pane_id, "pane ID")?;
    let tab = parse_native_id(&target.tab_id, "tab ID")?;
    let window = parse_native_id(&target.window_id, "window ID")?;
    if removed.schema_version != 1 || removed.kind != "pane" || removed.requested_native_id != pane
    {
        return Err(RecoveryError::Protocol(
            "removal witness does not echo the exact pane request".into(),
        ));
    }
    match removed.status {
        RemovedNodeStatus::Removed => {
            if removed.actual_parent_tab_id != Some(tab)
                || removed.actual_parent_window_id != Some(window)
                || removed.removed_pane_ids != [pane]
                || !(removed.removed_tab_ids.is_empty() || removed.removed_tab_ids == [tab])
                || !(removed.removed_window_ids.is_empty()
                    || removed.removed_window_ids == [window])
                || removed.postcondition_error.is_some()
            {
                return Err(RecoveryError::Protocol(
                    "removed witness has an inexact parent/cascade postcondition".into(),
                ));
            }
        }
        RemovedNodeStatus::NotFound => {
            if !removed.removed_pane_ids.is_empty()
                || !removed.removed_tab_ids.is_empty()
                || !removed.removed_window_ids.is_empty()
                || removed.postcondition_error.is_some()
            {
                return Err(RecoveryError::Protocol(
                    "not_found removal witness reports native side effects".into(),
                ));
            }
        }
        other => {
            return Err(RecoveryError::Failed(format!(
                "recovery removal primitive returned {other:?}: {}",
                removed
                    .postcondition_error
                    .as_deref()
                    .unwrap_or("no detail")
            )));
        }
    }
    Ok(())
}

fn publish_incarnation_if_needed(
    guard: &mut InstanceLeaseGuard,
    options: &RecoveryCoordinatorOptions,
    authority: Option<&crate::runtime::VerifiedWezServiceIdentity>,
) -> Result<()> {
    if authority.is_some() {
        let info = guard
            .registry
            .backend_instance_info(options.backend_instance)?;
        let fixed_socket = options
            .runtime_dir
            .join(crate::runtime::WEZ_SOCKET_FILE)
            .to_string_lossy()
            .into_owned();
        if info.backend != Backend::Wez
            || info.socket_path.as_deref() != Some(fixed_socket.as_str())
        {
            return Err(RecoveryError::FenceLost(format!(
                "managed Wez instance {} is not bound to fixed socket {}",
                options.backend_instance.0, fixed_socket
            )));
        }
    }
    let current = guard.registry.backend_server(options.backend_instance)?;
    let (pid, start_token, socket_dev, socket_ino) = match authority {
        Some(authority) => (
            i64::from(authority.pid),
            authority.start_token.as_str(),
            Some(i64::try_from(authority.socket_dev).map_err(|_| {
                RecoveryError::Protocol("verified socket device exceeds registry integer".into())
            })?),
            Some(i64::try_from(authority.socket_ino).map_err(|_| {
                RecoveryError::Protocol("verified socket inode exceeds registry integer".into())
            })?),
        ),
        None => (
            options.server_pid,
            options.server_start_token.as_str(),
            current.socket_dev,
            current.socket_ino,
        ),
    };
    let exact = current.server_epoch == Some(options.server_epoch)
        && current.server_pid == Some(pid)
        && current.server_start_token.as_deref() == Some(start_token)
        && current.socket_dev == socket_dev
        && current.socket_ino == socket_ino;
    if !exact {
        // A previous incarnation is retired before the fresh one is
        // published, under the lease this coordinator already holds, so the
        // transition is journaled as two authority revisions instead of an
        // overwrite (ADR 012 WS-B.3). The retirement is a compare-and-set on
        // the epoch read above: a stranger that published in between is a
        // typed refusal, never silently clobbered.
        if let Some(previous) = current.server_epoch
            && previous != options.server_epoch
        {
            guard
                .registry
                .retire_backend_server(options.backend_instance, previous)?;
        }
        guard.registry.publish_backend_server(
            options.backend_instance,
            options.server_epoch,
            Some(pid),
            Some(start_token),
            socket_dev,
            socket_ino,
        )?;
    }
    Ok(())
}

fn load_eligible_manifest_by_id(
    dir: &Path,
    instance: BackendInstanceUid,
    floor: Option<u64>,
    manifest_id: &str,
) -> Result<Option<RecoveryManifest>> {
    let directory = match open_private_leaf_dir(dir, false) {
        Ok(directory) => directory,
        Err(RecoveryError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    for os_name in directory.names()? {
        let Some(name) = os_name.to_str() else {
            continue;
        };
        if !(name.ends_with(".json") || name.ends_with(".json.bak")) {
            continue;
        }
        let Ok(bytes) = directory.read_file(name, MAX_RECOVERY_MANIFEST_BYTES) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_slice::<RecoveryManifest>(&bytes) else {
            continue;
        };
        if manifest.manifest_id != manifest_id || manifest.validate(instance).is_err() {
            continue;
        }
        if floor.is_some_and(|floor| manifest.registry_revision <= floor) {
            return Ok(None);
        }
        return Ok(Some(manifest));
    }
    Ok(None)
}

fn advance_generation_to_restoring(
    driver: &mut FileDriver<'_>,
    spec: &RecoveryGenerationSpec,
    resume_failed: bool,
) -> Result<()> {
    let root = driver
        .guard
        .registry
        .recovery_rows(spec.generation_uid)?
        .into_iter()
        .find(|row| row.manifest_node_path == GENERATION_ROOT_PATH)
        .ok_or_else(|| RecoveryError::Protocol("generation root row is missing".into()))?;
    let mut state = root.node_state;
    if state == RecoveryNodeState::Failed {
        if !resume_failed {
            return Err(RecoveryError::Failed(format!(
                "generation {} is failed; use explicit recovery resume or abort",
                spec.generation_uid
            )));
        }
        driver.guard.fence()?;
        driver.guard.registry.transition_recovery_node(
            spec.generation_uid,
            GENERATION_ROOT_PATH,
            RecoveryNodeState::Failed,
            RecoveryNodeState::Preparing,
            None,
            &driver.guard.lease,
        )?;
        state = RecoveryNodeState::Preparing;
    }
    if state == RecoveryNodeState::Pending {
        driver.guard.fence()?;
        driver.guard.registry.transition_recovery_node(
            spec.generation_uid,
            GENERATION_ROOT_PATH,
            RecoveryNodeState::Pending,
            RecoveryNodeState::Preparing,
            None,
            &driver.guard.lease,
        )?;
        state = RecoveryNodeState::Preparing;
    }
    if state == RecoveryNodeState::Preparing {
        driver.guard.fence()?;
        driver.guard.registry.transition_recovery_node(
            spec.generation_uid,
            GENERATION_ROOT_PATH,
            RecoveryNodeState::Preparing,
            RecoveryNodeState::Restoring,
            None,
            &driver.guard.lease,
        )?;
        state = RecoveryNodeState::Restoring;
    }
    if state != RecoveryNodeState::Restoring {
        return Err(RecoveryError::Protocol(format!(
            "generation root is unexpectedly {}",
            state.as_str()
        )));
    }
    Ok(())
}

fn restore_one_node(
    driver: &mut FileDriver<'_>,
    options: &RecoveryCoordinatorOptions,
    spec: &RecoveryGenerationSpec,
    manifest_nodes: &[RestoreNode],
    node: &RestoreNode,
    created_map: &mut BTreeMap<String, CreatedNode>,
) -> Result<()> {
    let mut row = recovery_row(
        &driver.guard.registry,
        spec.generation_uid,
        &node.manifest_node_path,
    )?;
    if matches!(
        row.node_state,
        RecoveryNodeState::Completed | RecoveryNodeState::Skipped
    ) {
        return Ok(());
    }
    if row.node_state == RecoveryNodeState::Failed {
        if !options.resume_failed {
            return Err(RecoveryError::Failed(format!(
                "node {} is failed; explicit resume is required",
                node.manifest_node_path
            )));
        }
        driver.guard.fence()?;
        row = driver.guard.registry.transition_recovery_node(
            spec.generation_uid,
            &node.manifest_node_path,
            RecoveryNodeState::Failed,
            RecoveryNodeState::Preparing,
            row.bootstrap_request_uid,
            &driver.guard.lease,
        )?;
    }
    if row.node_state == RecoveryNodeState::Pending {
        driver.guard.fence()?;
        row = driver.guard.registry.transition_recovery_node(
            spec.generation_uid,
            &node.manifest_node_path,
            RecoveryNodeState::Pending,
            RecoveryNodeState::Preparing,
            None,
            &driver.guard.lease,
        )?;
    }

    if row.node_state == RecoveryNodeState::Restoring {
        let request_uid = row.bootstrap_request_uid.ok_or_else(|| {
            RecoveryError::Protocol(format!(
                "restoring node {} has no bootstrap request",
                node.manifest_node_path
            ))
        })?;
        let expected_parent = native_parent(node, created_map)?;
        let precondition = capture_restore_precondition(
            driver,
            options,
            spec,
            manifest_nodes,
            node,
            request_uid,
            created_map,
            true,
        )?;
        let response = driver.exchange(RecoveryAction::CompareAndRestoreNode {
            node: node.clone(),
            request_uid,
            bootstrap_argv: Vec::new(),
            expected_tree: precondition.tree,
            expected_parent,
            expected_existing: precondition.existing.clone(),
            create_if_absent: false,
        })?;
        if let Some(created) = response.created {
            let expected = precondition.existing.ok_or_else(|| {
                RecoveryError::Protocol(format!(
                    "reconcile response unexpectedly found node {}",
                    node.manifest_node_path
                ))
            })?;
            if !same_created_native_id(&expected, &created) {
                return Err(RecoveryError::Protocol(format!(
                    "reconcile response changed native IDs for {}",
                    node.manifest_node_path
                )));
            }
            complete_reconciled_bootstrap(driver, options, spec, node, request_uid, &created)?;
            created_map.insert(node.manifest_node_path.clone(), created);
            return Ok(());
        }
        if precondition.existing.is_some() || !response.existing_absent {
            return Err(RecoveryError::Protocol(format!(
                "reconcile response for {} proved neither its exact existing pane nor absence",
                node.manifest_node_path
            )));
        }
        retire_absent_bootstrap(driver, options, spec, node, request_uid)?;
        row = recovery_row(
            &driver.guard.registry,
            spec.generation_uid,
            &node.manifest_node_path,
        )?;
    }

    if row.node_state != RecoveryNodeState::Preparing {
        return Err(RecoveryError::Protocol(format!(
            "node {} cannot spawn from {}",
            node.manifest_node_path,
            row.node_state.as_str()
        )));
    }
    let request_uid = Uuid::new_v4();
    let intended_parent = native_parent(node, created_map)?;
    boot(driver.guard.registry.bootstrap_issue(&IssuedRequest {
        request_uid,
        operation_uid: None,
        space_uid: Some(node.space_uid),
        backend_instance: options.backend_instance,
        server_epoch: options.server_epoch,
        intended_parent: intended_parent.clone(),
        recovery_generation: Some(spec.generation_uid.to_string()),
        manifest_node_path: Some(node.manifest_node_path.clone()),
    }))?;
    let paths = bootstrap::prepare(&options.runtime_dir, request_uid)?;
    driver.guard.fence()?;
    driver.guard.registry.transition_recovery_node(
        spec.generation_uid,
        &node.manifest_node_path,
        RecoveryNodeState::Preparing,
        RecoveryNodeState::Restoring,
        Some(request_uid),
        &driver.guard.lease,
    )?;
    // Capture and validate the exact partial tree, then carry that canonical
    // projection into the mutating Lua callback. Lua compares a fresh raw
    // mux projection and creates without yielding; an out-of-band mutation
    // in the former Inspect -> RestoreNode gap is therefore rejected before
    // the native side effect.
    let precondition = capture_restore_precondition(
        driver,
        options,
        spec,
        manifest_nodes,
        node,
        request_uid,
        created_map,
        false,
    )?;
    let response = driver.exchange(RecoveryAction::CompareAndRestoreNode {
        node: node.clone(),
        request_uid,
        bootstrap_argv: bootstrap::helper_argv(
            &options.helper_bin,
            request_uid,
            &options.default_program,
        ),
        expected_tree: precondition.tree,
        expected_parent: intended_parent,
        expected_existing: None,
        create_if_absent: true,
    })?;
    if response.existing_absent {
        return Err(RecoveryError::Protocol(format!(
            "create response for {} reported an absent reconcile",
            node.manifest_node_path
        )));
    }
    let created = response.created.ok_or_else(|| {
        RecoveryError::Protocol(format!(
            "restore response for {} omitted created IDs",
            node.manifest_node_path
        ))
    })?;
    driver.guard.fence()?;
    finish_bootstrap(
        &mut driver.guard.registry,
        options,
        node,
        request_uid,
        &paths,
        &created,
    )?;
    driver.crash_if(
        RecoveryCrashPhase::AfterBootstrapAck,
        &format!("ack:{}", node.manifest_node_path),
    );
    driver.guard.registry.transition_recovery_node(
        spec.generation_uid,
        &node.manifest_node_path,
        RecoveryNodeState::Restoring,
        RecoveryNodeState::Completed,
        Some(request_uid),
        &driver.guard.lease,
    )?;
    created_map.insert(node.manifest_node_path.clone(), created);
    Ok(())
}

struct RestorePrecondition {
    tree: NativeTreePrecondition,
    existing: Option<CreatedNode>,
}

#[allow(clippy::too_many_arguments)]
fn capture_restore_precondition(
    driver: &mut FileDriver<'_>,
    options: &RecoveryCoordinatorOptions,
    spec: &RecoveryGenerationSpec,
    manifest_nodes: &[RestoreNode],
    node: &RestoreNode,
    request_uid: Uuid,
    expected_created: &BTreeMap<String, CreatedNode>,
    allow_existing: bool,
) -> Result<RestorePrecondition> {
    let inspected = driver.exchange(RecoveryAction::Inspect)?;
    let snapshot = inspected.snapshot.ok_or_else(|| {
        RecoveryError::Protocol("pre-restore inspect omitted complete native snapshot".into())
    })?;
    let rows = driver.guard.registry.recovery_rows(spec.generation_uid)?;
    validate_recovery_snapshot(&driver.guard.registry, &snapshot, spec, &rows)?;
    let live_created = completed_native_map(&driver.guard.registry, &snapshot, spec)?;
    if !same_created_native_ids(expected_created, &live_created) {
        return Err(RecoveryError::InvalidSnapshot(
            "pre-restore completed native IDs differ from the coordinator's exact partial tree"
                .into(),
        ));
    }
    validate_bootstrap_identity(
        &driver.guard.registry,
        request_uid,
        spec,
        &node.manifest_node_path,
    )?;
    let request = driver
        .guard
        .registry
        .bootstrap_request(request_uid)?
        .ok_or_else(|| RecoveryError::Protocol(format!("bootstrap {request_uid} missing")))?;
    let reserved = panes_with_title(&snapshot, &bootstrap::reserved_title(request_uid));
    let running = panes_with_title(&snapshot, &bootstrap::run_title(request_uid));
    if reserved.len() + running.len() > 1 {
        return Err(RecoveryError::Protocol(format!(
            "bootstrap {request_uid} has multiple native panes"
        )));
    }
    let titled = reserved.first().or_else(|| running.first()).cloned();
    let recorded = recorded_created_node(request_uid, request.returned_native_ids.as_deref())?
        .map(|recorded| live_created_node(&snapshot, request_uid, &recorded))
        .transpose()?
        .flatten();
    if let (Some(titled), Some(recorded)) = (&titled, &recorded)
        && !same_created_native_id(titled, recorded)
    {
        return Err(RecoveryError::Protocol(format!(
            "bootstrap {request_uid} title and recorded IDs disagree"
        )));
    }
    let existing = titled.or(recorded);
    if existing.is_some() && !allow_existing {
        return Err(RecoveryError::NonEmpty(format!(
            "fresh node {} already has a recovery pane",
            node.manifest_node_path
        )));
    }
    let mut exact_created = live_created;
    if let Some(existing) = &existing {
        exact_created.insert(node.manifest_node_path.clone(), existing.clone());
    }
    let completed_nodes = manifest_nodes
        .iter()
        .filter(|node| exact_created.contains_key(&node.manifest_node_path))
        .cloned()
        .collect::<Vec<_>>();
    verify_final_snapshot(
        &snapshot,
        options.server_epoch,
        &completed_nodes,
        &exact_created,
    )?;
    Ok(RestorePrecondition {
        tree: snapshot.tree_precondition(),
        existing,
    })
}

fn same_created_native_ids(
    expected: &BTreeMap<String, CreatedNode>,
    actual: &BTreeMap<String, CreatedNode>,
) -> bool {
    expected.len() == actual.len()
        && expected.iter().all(|(path, expected)| {
            actual.get(path).is_some_and(|actual| {
                actual.window_id == expected.window_id
                    && actual.tab_id == expected.tab_id
                    && actual.pane_id == expected.pane_id
            })
        })
}

fn same_created_native_id(expected: &CreatedNode, actual: &CreatedNode) -> bool {
    expected.window_id == actual.window_id
        && expected.tab_id == actual.tab_id
        && expected.pane_id == actual.pane_id
}

fn native_parent(
    node: &RestoreNode,
    created: &BTreeMap<String, CreatedNode>,
) -> Result<Option<String>> {
    match node.operation {
        RestoreOperation::SpaceRoot => Ok(None),
        RestoreOperation::GroupRoot => {
            let first = format!("/spaces/{}/groups/1/splits/L", node.space_uid.0);
            Ok(Some(
                created
                    .get(&first)
                    .ok_or_else(|| {
                        RecoveryError::Protocol(format!(
                            "Group root {} has no completed Space root",
                            node.manifest_node_path
                        ))
                    })?
                    .window_id
                    .clone(),
            ))
        }
        RestoreOperation::Split => {
            let parent = node.parent_path.as_ref().ok_or_else(|| {
                RecoveryError::InvalidManifest(format!(
                    "split {} has no parent path",
                    node.manifest_node_path
                ))
            })?;
            Ok(Some(
                created
                    .get(parent)
                    .ok_or_else(|| {
                        RecoveryError::Protocol(format!(
                            "split {} parent {parent} is not completed",
                            node.manifest_node_path
                        ))
                    })?
                    .pane_id
                    .clone(),
            ))
        }
    }
}

fn complete_reconciled_bootstrap(
    driver: &mut FileDriver<'_>,
    options: &RecoveryCoordinatorOptions,
    spec: &RecoveryGenerationSpec,
    node: &RestoreNode,
    request_uid: Uuid,
    created: &CreatedNode,
) -> Result<()> {
    validate_bootstrap_identity(
        &driver.guard.registry,
        request_uid,
        spec,
        &node.manifest_node_path,
    )?;
    let request = driver
        .guard
        .registry
        .bootstrap_request(request_uid)?
        .ok_or_else(|| RecoveryError::Protocol(format!("bootstrap {request_uid} vanished")))?;
    if matches!(
        request.state,
        BootstrapState::Correlated | BootstrapState::Acked | BootstrapState::Completed
    ) {
        complete_bootstrap_from_running(
            &mut driver.guard.registry,
            &options.runtime_dir,
            request_uid,
        )?;
    } else {
        let paths = bootstrap::BootstrapPaths::new(&options.runtime_dir, request_uid);
        finish_bootstrap(
            &mut driver.guard.registry,
            options,
            node,
            request_uid,
            &paths,
            created,
        )?;
    }
    driver.guard.fence()?;
    driver.guard.registry.transition_recovery_node(
        spec.generation_uid,
        &node.manifest_node_path,
        RecoveryNodeState::Restoring,
        RecoveryNodeState::Completed,
        Some(request_uid),
        &driver.guard.lease,
    )?;
    Ok(())
}

fn retire_absent_bootstrap(
    driver: &mut FileDriver<'_>,
    options: &RecoveryCoordinatorOptions,
    spec: &RecoveryGenerationSpec,
    node: &RestoreNode,
    request_uid: Uuid,
) -> Result<()> {
    let request = driver
        .guard
        .registry
        .bootstrap_request(request_uid)?
        .ok_or_else(|| RecoveryError::Protocol(format!("bootstrap {request_uid} vanished")))?;
    if !crate::registry::bootstrap_is_terminal(request.state) {
        boot(
            driver
                .guard
                .registry
                .bootstrap_state(request_uid, BootstrapState::Orphaned),
        )?;
    }
    driver.guard.fence()?;
    driver.guard.registry.transition_recovery_node(
        spec.generation_uid,
        &node.manifest_node_path,
        RecoveryNodeState::Restoring,
        RecoveryNodeState::Preparing,
        Some(request_uid),
        &driver.guard.lease,
    )?;
    bootstrap::cleanup(&bootstrap::BootstrapPaths::new(
        &options.runtime_dir,
        request_uid,
    ));
    Ok(())
}

fn finish_bootstrap(
    registry: &mut Registry,
    options: &RecoveryCoordinatorOptions,
    node: &RestoreNode,
    request_uid: Uuid,
    paths: &bootstrap::BootstrapPaths,
    created: &CreatedNode,
) -> Result<()> {
    let mut request = registry
        .bootstrap_request(request_uid)?
        .ok_or_else(|| RecoveryError::Protocol(format!("bootstrap {request_uid} missing")))?;
    let returned = serde_json::to_string(created)?;
    if request.state == BootstrapState::Issued {
        boot(registry.bootstrap_spawned(request_uid, &returned))?;
        request.state = BootstrapState::Spawned;
    }
    let group_ref = format!("g{}.wz-{}", options.server_epoch.0, created.tab_id);
    let split_ref = format!("p{}.wz-{}", options.server_epoch.0, created.pane_id);
    if request.state == BootstrapState::Spawned {
        let env = bootstrap::read_pane_env(paths, Duration::from_secs(10))?
            .ok_or_else(|| RecoveryError::TimedOut(format!("bootstrap {request_uid} pane-env")))?;
        if env.request_uid != request_uid {
            return Err(RecoveryError::Protocol(format!(
                "bootstrap {request_uid} pane-env UID mismatch"
            )));
        }
        match bootstrap::correlate(
            &created.titled_pane_ids,
            Some(&created.pane_id),
            env.wezterm_pane.as_deref(),
        ) {
            bootstrap::Correlation::Confirmed { .. } => {}
            other => {
                boot(registry.bootstrap_state(request_uid, BootstrapState::Conflict))?;
                return Err(RecoveryError::Protocol(format!(
                    "bootstrap {request_uid} correlation failed: {other:?}"
                )));
            }
        }
        boot(registry.bootstrap_correlated(request_uid, &group_ref, &split_ref))?;
        request.state = BootstrapState::Correlated;
    }
    if request.state == BootstrapState::Correlated {
        let identity = registry.identity()?;
        let space = registry.space(node.space_uid)?;
        let payload = bootstrap::BootstrapResult {
            request_uid,
            context: bootstrap::MarkerContext {
                host_uid: identity.host_uid,
                space_uid: node.space_uid,
                space_no: space.space_no,
                backend: Backend::Wez,
                domain: Some("dmux".into()),
                server_epoch: options.server_epoch,
                group_ref,
                split_ref,
            },
        };
        bootstrap::send_result(paths, &payload, Duration::from_secs(10)).map_err(|error| {
            RecoveryError::TimedOut(format!("bootstrap {request_uid} send: {error:?}"))
        })?;
        let ack = bootstrap::read_ack(paths, Duration::from_secs(10))?
            .ok_or_else(|| RecoveryError::TimedOut(format!("bootstrap {request_uid} ack")))?;
        if ack.request_uid != request_uid {
            return Err(RecoveryError::Protocol(format!(
                "bootstrap {request_uid} ack UID mismatch"
            )));
        }
        boot(registry.bootstrap_state(request_uid, BootstrapState::Acked))?;
        request.state = BootstrapState::Acked;
    }
    if request.state == BootstrapState::Acked {
        boot(registry.bootstrap_state(request_uid, BootstrapState::Completed))?;
        request.state = BootstrapState::Completed;
    }
    if request.state != BootstrapState::Completed {
        return Err(RecoveryError::Protocol(format!(
            "bootstrap {request_uid} stopped at {}",
            request.state.as_str()
        )));
    }
    bootstrap::cleanup(paths);
    Ok(())
}

fn complete_bootstrap_from_running(
    registry: &mut Registry,
    runtime_dir: &Path,
    request_uid: Uuid,
) -> Result<()> {
    let request = registry
        .bootstrap_request(request_uid)?
        .ok_or_else(|| RecoveryError::Protocol(format!("bootstrap {request_uid} missing")))?;
    match request.state {
        BootstrapState::Completed => Ok(()),
        BootstrapState::Acked => {
            boot(registry.bootstrap_state(request_uid, BootstrapState::Completed))?;
            Ok(())
        }
        BootstrapState::Correlated => {
            let paths = bootstrap::BootstrapPaths::new(runtime_dir, request_uid);
            let ack =
                bootstrap::read_ack(&paths, Duration::from_millis(100))?.ok_or_else(|| {
                    RecoveryError::Protocol(format!(
                        "running bootstrap {request_uid} has no durable ack"
                    ))
                })?;
            if ack.request_uid != request_uid {
                return Err(RecoveryError::Protocol("bootstrap ack UID mismatch".into()));
            }
            boot(registry.bootstrap_state(request_uid, BootstrapState::Acked))?;
            boot(registry.bootstrap_state(request_uid, BootstrapState::Completed))?;
            Ok(())
        }
        other => Err(RecoveryError::Protocol(format!(
            "running pane has bootstrap state {}",
            other.as_str()
        ))),
    }
}

fn boot<T>(value: std::result::Result<T, crate::error::TypedError>) -> Result<T> {
    value.map_err(|error| RecoveryError::Failed(error.message))
}

fn recovery_row(
    registry: &Registry,
    generation_uid: Uuid,
    path: &str,
) -> Result<RecoveryJournalRow> {
    registry
        .recovery_rows(generation_uid)?
        .into_iter()
        .find(|row| row.manifest_node_path == path)
        .ok_or_else(|| {
            RecoveryError::Protocol(format!("recovery row {generation_uid}/{path} missing"))
        })
}

fn validate_bootstrap_identity(
    registry: &Registry,
    request_uid: Uuid,
    spec: &RecoveryGenerationSpec,
    path: &str,
) -> Result<()> {
    let request = registry
        .bootstrap_request(request_uid)?
        .ok_or_else(|| RecoveryError::Protocol(format!("bootstrap {request_uid} missing")))?;
    if request.backend_instance != spec.backend_instance
        || request.server_epoch != spec.server_epoch
        || request.recovery_generation.as_deref() != Some(spec.generation_uid.to_string().as_str())
        || request.manifest_node_path.as_deref() != Some(path)
    {
        return Err(RecoveryError::Protocol(format!(
            "bootstrap {request_uid} identity does not match generation node {path}"
        )));
    }
    Ok(())
}

fn panes_with_title(snapshot: &NativeSnapshot, title: &str) -> Vec<CreatedNode> {
    let titled_pane_ids = snapshot
        .panes()
        .filter(|(_, _, pane)| pane.title == title)
        .map(|(_, _, pane)| pane.pane_id.clone())
        .collect::<Vec<_>>();
    snapshot
        .panes()
        .filter(|(_, _, pane)| pane.title == title)
        .map(|(window, tab, pane)| CreatedNode {
            window_id: window.window_id.clone(),
            tab_id: tab.tab_id.clone(),
            pane_id: pane.pane_id.clone(),
            titled_pane_ids: titled_pane_ids.clone(),
        })
        .collect()
}

fn title_request_uid(title: &str) -> Option<Uuid> {
    let token = title
        .strip_prefix(bootstrap::RESERVED_TITLE_PREFIX)
        .or_else(|| title.strip_prefix(bootstrap::RUN_TITLE_PREFIX))?;
    let uid = Uuid::parse_str(token).ok()?;
    (uid.to_string() == token).then_some(uid)
}

fn recorded_created_node(
    request_uid: Uuid,
    returned_native_ids: Option<&str>,
) -> Result<Option<CreatedNode>> {
    returned_native_ids
        .map(|value| {
            serde_json::from_str::<CreatedNode>(value).map_err(|error| {
                RecoveryError::Protocol(format!(
                    "bootstrap {request_uid} has invalid returned native IDs: {error}"
                ))
            })
        })
        .transpose()
}

fn live_created_node(
    snapshot: &NativeSnapshot,
    request_uid: Uuid,
    recorded: &CreatedNode,
) -> Result<Option<CreatedNode>> {
    let matches = snapshot
        .panes()
        .filter(|(_, _, pane)| pane.pane_id == recorded.pane_id)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [(window, tab, pane)] => {
            if window.window_id != recorded.window_id || tab.tab_id != recorded.tab_id {
                return Err(RecoveryError::Protocol(format!(
                    "bootstrap {request_uid} pane {} moved from recorded parent {}/{} to {}/{}",
                    pane.pane_id, recorded.window_id, recorded.tab_id, window.window_id, tab.tab_id
                )));
            }
            Ok(Some(CreatedNode {
                window_id: window.window_id.clone(),
                tab_id: tab.tab_id.clone(),
                pane_id: pane.pane_id.clone(),
                titled_pane_ids: if title_request_uid(&pane.title) == Some(request_uid) {
                    vec![pane.pane_id.clone()]
                } else {
                    Vec::new()
                },
            }))
        }
        many => Err(RecoveryError::Protocol(format!(
            "bootstrap {request_uid} recorded pane ID {} appears {} times",
            recorded.pane_id,
            many.len()
        ))),
    }
}

fn validate_recovery_snapshot(
    registry: &Registry,
    snapshot: &NativeSnapshot,
    spec: &RecoveryGenerationSpec,
    rows: &[RecoveryJournalRow],
) -> Result<()> {
    snapshot.validate_complete(spec.server_epoch)?;
    let sentinel_workspace = format!("dmux:system:{}", spec.server_epoch.0);
    let sentinel_count = snapshot
        .panes()
        .filter(|(window, _, _)| window.workspace == sentinel_workspace)
        .count();
    if sentinel_count != 1 {
        return Err(RecoveryError::InvalidSnapshot(format!(
            "resume requires exactly one sentinel, found {sentinel_count}"
        )));
    }
    let paths = rows
        .iter()
        .filter(|row| row.manifest_node_path != GENERATION_ROOT_PATH)
        .map(|row| row.manifest_node_path.as_str())
        .collect::<BTreeSet<_>>();
    let mut recorded_by_pane = BTreeMap::new();
    for row in rows
        .iter()
        .filter(|row| row.manifest_node_path != GENERATION_ROOT_PATH)
    {
        let Some(request_uid) = row.bootstrap_request_uid else {
            continue;
        };
        validate_bootstrap_identity(registry, request_uid, spec, &row.manifest_node_path)?;
        let request = registry
            .bootstrap_request(request_uid)?
            .ok_or_else(|| RecoveryError::Protocol(format!("bootstrap {request_uid} missing")))?;
        if let Some(recorded) =
            recorded_created_node(request_uid, request.returned_native_ids.as_deref())?
            && recorded_by_pane
                .insert(recorded.pane_id.clone(), (request_uid, recorded))
                .is_some()
        {
            return Err(RecoveryError::NonEmpty(
                "multiple recovery requests claim the same recorded pane ID".into(),
            ));
        }
    }

    let mut seen = BTreeSet::new();
    for (window, tab, pane) in snapshot.panes() {
        if window.workspace == sentinel_workspace {
            continue;
        }
        if !matches!(pane.domain.as_deref(), Some("local" | "unix" | "dmux")) {
            return Err(RecoveryError::NonEmpty(format!(
                "resume snapshot contains non-owner domain {:?}",
                pane.domain
            )));
        }
        let titled_uid = title_request_uid(&pane.title);
        let recorded = recorded_by_pane.get(&pane.pane_id);
        let request_uid = match (titled_uid, recorded) {
            (Some(titled), Some((recorded_uid, _))) if titled != *recorded_uid => {
                return Err(RecoveryError::NonEmpty(format!(
                    "pane {} title and recorded native ID name different recovery requests",
                    pane.pane_id
                )));
            }
            (Some(titled), _) => titled,
            (None, Some((recorded_uid, _))) => *recorded_uid,
            (None, None) => {
                return Err(RecoveryError::NonEmpty(format!(
                    "pane {} is not proven recovery-created by token or recorded native ID",
                    pane.pane_id
                )));
            }
        };
        if !seen.insert(request_uid) {
            return Err(RecoveryError::NonEmpty(format!(
                "recovery request {request_uid} labels multiple panes"
            )));
        }
        let request = registry.bootstrap_request(request_uid)?.ok_or_else(|| {
            RecoveryError::NonEmpty(format!("pane {} has unknown recovery token", pane.pane_id))
        })?;
        let path = request.manifest_node_path.as_deref().ok_or_else(|| {
            RecoveryError::NonEmpty(format!("recovery token {request_uid} has no node path"))
        })?;
        if request.recovery_generation.as_deref() != Some(spec.generation_uid.to_string().as_str())
            || request.backend_instance != spec.backend_instance
            || request.server_epoch != spec.server_epoch
            || !paths.contains(path)
        {
            return Err(RecoveryError::NonEmpty(format!(
                "pane {} recovery token does not belong to this generation",
                pane.pane_id
            )));
        }
        if let Some((_, recorded)) = recorded
            && (recorded.window_id != window.window_id || recorded.tab_id != tab.tab_id)
        {
            return Err(RecoveryError::NonEmpty(format!(
                "pane {} no longer has its recorded recovery parent",
                pane.pane_id
            )));
        }
    }
    Ok(())
}

fn completed_native_map(
    registry: &Registry,
    snapshot: &NativeSnapshot,
    spec: &RecoveryGenerationSpec,
) -> Result<BTreeMap<String, CreatedNode>> {
    let mut map = BTreeMap::new();
    for row in registry.recovery_rows(spec.generation_uid)? {
        if row.manifest_node_path == GENERATION_ROOT_PATH
            || row.node_state != RecoveryNodeState::Completed
        {
            continue;
        }
        let request_uid = row.bootstrap_request_uid.ok_or_else(|| {
            RecoveryError::Protocol(format!(
                "completed node {} has no bootstrap request",
                row.manifest_node_path
            ))
        })?;
        validate_bootstrap_identity(registry, request_uid, spec, &row.manifest_node_path)?;
        let request = registry
            .bootstrap_request(request_uid)?
            .ok_or_else(|| RecoveryError::Protocol(format!("bootstrap {request_uid} missing")))?;
        if request.state != BootstrapState::Completed {
            return Err(RecoveryError::Protocol(format!(
                "completed node {} bootstrap is {}",
                row.manifest_node_path,
                request.state.as_str()
            )));
        }
        let recorded = recorded_created_node(request_uid, request.returned_native_ids.as_deref())?
            .ok_or_else(|| {
                RecoveryError::Protocol(format!(
                    "completed node {} has no recorded native IDs",
                    row.manifest_node_path
                ))
            })?;
        let created = live_created_node(snapshot, request_uid, &recorded)?.ok_or_else(|| {
            RecoveryError::Protocol(format!(
                "completed node {} recorded pane {} is absent",
                row.manifest_node_path, recorded.pane_id
            ))
        })?;
        map.insert(row.manifest_node_path, created);
    }
    Ok(map)
}

fn verify_completed_generation_snapshot(
    registry: &Registry,
    snapshot: &NativeSnapshot,
    spec: &RecoveryGenerationSpec,
    rows: &[RecoveryJournalRow],
    nodes: &[RestoreNode],
) -> Result<()> {
    validate_recovery_snapshot(registry, snapshot, spec, rows)?;
    for row in rows
        .iter()
        .filter(|row| row.manifest_node_path != GENERATION_ROOT_PATH)
    {
        if !matches!(
            row.node_state,
            RecoveryNodeState::Completed | RecoveryNodeState::Skipped
        ) {
            return Err(RecoveryError::Protocol(format!(
                "completed generation {} retains nonterminal node {} ({})",
                spec.generation_uid,
                row.manifest_node_path,
                row.node_state.as_str()
            )));
        }
    }
    let completed = completed_native_map(registry, snapshot, spec)?;
    verify_final_snapshot(snapshot, spec.server_epoch, nodes, &completed)
}

fn verify_final_snapshot(
    snapshot: &NativeSnapshot,
    epoch: ServerEpoch,
    nodes: &[RestoreNode],
    created: &BTreeMap<String, CreatedNode>,
) -> Result<()> {
    snapshot.validate_complete(epoch)?;
    let sentinel_workspace = format!("dmux:system:{}", epoch.0);
    let sentinel_windows = snapshot
        .windows
        .iter()
        .filter(|window| window.workspace == sentinel_workspace)
        .collect::<Vec<_>>();
    if sentinel_windows.len() != 1
        || sentinel_windows[0].tabs.len() != 1
        || sentinel_windows[0].tabs[0].panes.len() != 1
    {
        return Err(RecoveryError::InvalidSnapshot(format!(
            "final tree sentinel topology is {}/{}/{} instead of one window/tab/pane",
            sentinel_windows.len(),
            sentinel_windows
                .iter()
                .map(|window| window.tabs.len())
                .sum::<usize>(),
            sentinel_windows
                .iter()
                .flat_map(|window| &window.tabs)
                .map(|tab| tab.panes.len())
                .sum::<usize>()
        )));
    }
    let sentinels = snapshot
        .panes()
        .filter(|(window, _, _)| window.workspace == sentinel_workspace)
        .count();
    if sentinels != 1 {
        return Err(RecoveryError::InvalidSnapshot(format!(
            "final tree has {sentinels} sentinels"
        )));
    }
    if snapshot.panes().count() != nodes.len() + 1 {
        return Err(RecoveryError::InvalidSnapshot(format!(
            "final tree has {} panes, expected sentinel + {} recovery nodes",
            snapshot.panes().count(),
            nodes.len()
        )));
    }
    let expected_paths = nodes
        .iter()
        .map(|node| node.manifest_node_path.as_str())
        .collect::<BTreeSet<_>>();
    let created_paths = created.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if created_paths != expected_paths {
        return Err(RecoveryError::InvalidSnapshot(
            "final recovery native-node paths differ from the exact manifest".into(),
        ));
    }
    for (window, _, pane) in snapshot.panes() {
        if window.workspace == sentinel_workspace {
            continue;
        }
        if !matches!(pane.domain.as_deref(), Some("local" | "unix" | "dmux")) {
            return Err(RecoveryError::InvalidSnapshot(format!(
                "final recovery tree contains non-owner domain {:?}",
                pane.domain
            )));
        }
    }
    for node in nodes {
        let native = created.get(&node.manifest_node_path).ok_or_else(|| {
            RecoveryError::InvalidSnapshot(format!(
                "node {} has no native postcondition",
                node.manifest_node_path
            ))
        })?;
        let count = snapshot
            .panes()
            .filter(|(window, tab, pane)| {
                window.window_id == native.window_id
                    && tab.tab_id == native.tab_id
                    && pane.pane_id == native.pane_id
                    && window.workspace == node.opaque_key
            })
            .count();
        if count != 1 {
            return Err(RecoveryError::InvalidSnapshot(format!(
                "node {} postcondition count is {count}",
                node.manifest_node_path
            )));
        }
    }
    let spaces = nodes
        .iter()
        .map(|node| node.opaque_key.as_str())
        .collect::<BTreeSet<_>>();
    for window in &snapshot.windows {
        if window.workspace != sentinel_workspace && !spaces.contains(window.workspace.as_str()) {
            return Err(RecoveryError::InvalidSnapshot(format!(
                "final recovery tree contains unexpected workspace {:?}",
                window.workspace
            )));
        }
    }
    for key in spaces {
        let windows = snapshot
            .windows
            .iter()
            .filter(|window| window.workspace == key)
            .collect::<Vec<_>>();
        if windows.len() != 1 {
            return Err(RecoveryError::InvalidSnapshot(format!(
                "restored Space {key:?} has {} windows",
                windows.len()
            )));
        }
        let window = windows[0];
        let space_nodes = nodes
            .iter()
            .filter(|node| node.opaque_key == key)
            .collect::<Vec<_>>();
        let roots = space_nodes
            .iter()
            .copied()
            .filter(|node| node.operation == RestoreOperation::SpaceRoot)
            .collect::<Vec<_>>();
        if roots.len() != 1 || roots[0].group_index != 1 {
            return Err(RecoveryError::InvalidSnapshot(format!(
                "restored Space {key:?} has {} Space roots",
                roots.len()
            )));
        }
        let root_native = created
            .get(&roots[0].manifest_node_path)
            .expect("created path set was checked above");
        if window.window_id != root_native.window_id {
            return Err(RecoveryError::InvalidSnapshot(format!(
                "restored Space {key:?} window differs from its Space root"
            )));
        }

        let group_indexes = space_nodes
            .iter()
            .map(|node| node.group_index)
            .collect::<BTreeSet<_>>();
        let mut expected_tabs = BTreeMap::<String, BTreeSet<String>>::new();
        for group_index in group_indexes {
            let group_nodes = space_nodes
                .iter()
                .copied()
                .filter(|node| node.group_index == group_index)
                .collect::<Vec<_>>();
            let group_roots = group_nodes
                .iter()
                .copied()
                .filter(|node| {
                    node.operation
                        == if group_index == 1 {
                            RestoreOperation::SpaceRoot
                        } else {
                            RestoreOperation::GroupRoot
                        }
                })
                .collect::<Vec<_>>();
            if group_roots.len() != 1 {
                return Err(RecoveryError::InvalidSnapshot(format!(
                    "restored Space {key:?} Group {group_index} has {} roots",
                    group_roots.len()
                )));
            }
            let group_native = created
                .get(&group_roots[0].manifest_node_path)
                .expect("created path set was checked above");
            if group_native.window_id != root_native.window_id {
                return Err(RecoveryError::InvalidSnapshot(format!(
                    "restored Space {key:?} Group {group_index} crossed windows"
                )));
            }
            let mut panes = BTreeSet::new();
            for node in group_nodes {
                let native = created
                    .get(&node.manifest_node_path)
                    .expect("created path set was checked above");
                if native.window_id != root_native.window_id
                    || native.tab_id != group_native.tab_id
                    || !panes.insert(native.pane_id.clone())
                {
                    return Err(RecoveryError::InvalidSnapshot(format!(
                        "restored Space {key:?} Group {group_index} has invalid parent topology"
                    )));
                }
            }
            if expected_tabs
                .insert(group_native.tab_id.clone(), panes)
                .is_some()
            {
                return Err(RecoveryError::InvalidSnapshot(format!(
                    "restored Space {key:?} maps multiple Groups to tab {}",
                    group_native.tab_id
                )));
            }
        }
        if window.tabs.len() != expected_tabs.len() {
            return Err(RecoveryError::InvalidSnapshot(format!(
                "restored Space {key:?} has {} tabs, expected {} Groups",
                window.tabs.len(),
                expected_tabs.len()
            )));
        }
        let actual_tabs = window
            .tabs
            .iter()
            .map(|tab| {
                (
                    tab.tab_id.clone(),
                    tab.panes
                        .iter()
                        .map(|pane| pane.pane_id.clone())
                        .collect::<BTreeSet<_>>(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        if actual_tabs != expected_tabs {
            return Err(RecoveryError::InvalidSnapshot(format!(
                "restored Space {key:?} tab/pane topology differs from the manifest"
            )));
        }
    }
    Ok(())
}

fn write_status(spool: &RecoverySpool, status: RecoveryStatus) -> Result<()> {
    spool.write(RecoverySpoolFile::Status, &status)
}

fn write_recovering_status(
    spool: &RecoverySpool,
    options: &RecoveryCoordinatorOptions,
    spec: &RecoveryGenerationSpec,
    fencing_token: i64,
    current_node: Option<String>,
) -> Result<()> {
    write_status(
        spool,
        RecoveryStatus {
            protocol_version: RECOVERY_PROTOCOL_VERSION,
            state: RecoveryStatusState::Recovering,
            backend_instance_uid: options.backend_instance,
            server_epoch: options.server_epoch,
            coordinator_uid: options.request_uid,
            generation_uid: Some(spec.generation_uid),
            manifest_id: Some(spec.manifest_id.clone()),
            fencing_token: Some(fencing_token),
            current_node,
            error: None,
            updated_at: now_rfc3339(),
        },
    )
}

fn write_ready_status(
    spool: &RecoverySpool,
    options: &RecoveryCoordinatorOptions,
    generation_uid: Option<Uuid>,
    manifest_id: Option<String>,
    fencing_token: i64,
) -> Result<()> {
    write_status(
        spool,
        RecoveryStatus {
            protocol_version: RECOVERY_PROTOCOL_VERSION,
            state: RecoveryStatusState::Ready,
            backend_instance_uid: options.backend_instance,
            server_epoch: options.server_epoch,
            coordinator_uid: options.request_uid,
            generation_uid,
            manifest_id,
            fencing_token: Some(fencing_token),
            current_node: None,
            error: None,
            updated_at: now_rfc3339(),
        },
    )
}

fn write_failed_status(
    spool: &RecoverySpool,
    options: &RecoveryCoordinatorOptions,
    spec: Option<&RecoveryGenerationSpec>,
    error: &str,
) -> Result<()> {
    write_status(
        spool,
        RecoveryStatus {
            protocol_version: RECOVERY_PROTOCOL_VERSION,
            state: RecoveryStatusState::Failed,
            backend_instance_uid: options.backend_instance,
            server_epoch: options.server_epoch,
            coordinator_uid: options.request_uid,
            generation_uid: spec.map(|spec| spec.generation_uid),
            manifest_id: spec.map(|spec| spec.manifest_id.clone()),
            fencing_token: None,
            current_node: None,
            error: Some(error.into()),
            updated_at: now_rfc3339(),
        },
    )
}

fn write_aborted_status(
    spool: &RecoverySpool,
    options: &RecoveryCoordinatorOptions,
    spec: &RecoveryGenerationSpec,
    fencing_token: i64,
) -> Result<()> {
    write_status(
        spool,
        RecoveryStatus {
            protocol_version: RECOVERY_PROTOCOL_VERSION,
            state: RecoveryStatusState::Aborted,
            backend_instance_uid: options.backend_instance,
            server_epoch: options.server_epoch,
            coordinator_uid: options.request_uid,
            generation_uid: Some(spec.generation_uid),
            manifest_id: Some(spec.manifest_id.clone()),
            fencing_token: Some(fencing_token),
            current_node: None,
            error: None,
            updated_at: now_rfc3339(),
        },
    )
}

fn mark_generation_failed(
    driver: &mut FileDriver<'_>,
    spec: &RecoveryGenerationSpec,
) -> Result<()> {
    let mut root = recovery_row(
        &driver.guard.registry,
        spec.generation_uid,
        GENERATION_ROOT_PATH,
    )?;
    if root.node_state == RecoveryNodeState::Failed {
        return Ok(());
    }
    if root.node_state == RecoveryNodeState::Pending {
        driver.guard.fence()?;
        root = driver.guard.registry.transition_recovery_node(
            spec.generation_uid,
            GENERATION_ROOT_PATH,
            RecoveryNodeState::Pending,
            RecoveryNodeState::Preparing,
            None,
            &driver.guard.lease,
        )?;
    }
    if root.node_state == RecoveryNodeState::Preparing {
        driver.guard.fence()?;
        root = driver.guard.registry.transition_recovery_node(
            spec.generation_uid,
            GENERATION_ROOT_PATH,
            RecoveryNodeState::Preparing,
            RecoveryNodeState::Restoring,
            None,
            &driver.guard.lease,
        )?;
    }
    if root.node_state == RecoveryNodeState::Restoring {
        driver.guard.fence()?;
        driver.guard.registry.transition_recovery_node(
            spec.generation_uid,
            GENERATION_ROOT_PATH,
            RecoveryNodeState::Restoring,
            RecoveryNodeState::Failed,
            None,
            &driver.guard.lease,
        )?;
        return Ok(());
    }
    Err(RecoveryError::Protocol(format!(
        "cannot mark terminal generation {} state {} failed",
        spec.generation_uid,
        root.node_state.as_str()
    )))
}

fn fail_generation(
    driver: &mut FileDriver<'_>,
    spool: &RecoverySpool,
    options: &RecoveryCoordinatorOptions,
    spec: &RecoveryGenerationSpec,
    message: String,
) -> Result<RecoveryRunReport> {
    let _ = mark_generation_failed(driver, spec);
    let _ = write_failed_status(spool, options, Some(spec), &message);
    Err(RecoveryError::Failed(message))
}

#[cfg(test)]
mod secure_incarnation_tests {
    use super::*;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    use std::os::unix::net::UnixListener;

    #[test]
    fn verified_witness_populates_fresh_registry_incarnation_for_strict_reader() {
        let scratch = tempfile::tempdir().unwrap();
        let runtime = scratch.path().join("runtime");
        fs::DirBuilder::new().mode(0o700).create(&runtime).unwrap();
        let socket = runtime.join(crate::runtime::WEZ_SOCKET_FILE);
        let _listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        let socket_metadata = fs::symlink_metadata(&socket).unwrap();
        let pid = std::process::id();
        let start_token = crate::runtime::process_start_token_for_pid(pid).unwrap();
        let boot_id = crate::runtime::current_boot_id().unwrap();
        let epoch = ServerEpoch(Uuid::new_v4());
        let config = RegistryConfig::new(
            scratch.path().join("registry.sqlite3"),
            scratch.path().join("locks"),
        );
        let mut registry = Registry::open(config.clone()).unwrap();
        let instance = registry
            .register_backend_instance(
                Backend::Wez,
                Some(socket.to_str().unwrap()),
                Some("test-service"),
            )
            .unwrap();
        assert_eq!(
            registry.backend_server(instance).unwrap(),
            crate::registry::BackendServerRecord {
                server_epoch: None,
                server_pid: None,
                server_start_token: None,
                socket_dev: None,
                socket_ino: None,
            }
        );
        drop(registry);

        let options = RecoveryCoordinatorOptions::new(
            config.clone(),
            runtime.clone(),
            scratch.path().join("manifests"),
            instance,
            epoch,
            i64::from(pid),
            start_token.clone(),
            "/test-only/pane-bootstrap".into(),
        );
        let witness = crate::runtime::VerifiedWezServiceIdentity {
            pid,
            start_token: start_token.clone(),
            boot_id: boot_id.clone(),
            socket_dev: socket_metadata.dev(),
            socket_ino: socket_metadata.ino(),
        };
        let mut guard = InstanceLeaseGuard::acquire(
            config.clone(),
            instance,
            LeaseScope::Recovery(instance),
            Uuid::new_v4(),
            DEFAULT_LEASE_TTL,
        )
        .unwrap();
        publish_incarnation_if_needed(&mut guard, &options, Some(&witness)).unwrap();
        guard.release().unwrap();

        let registry = Registry::open(config).unwrap();
        let published = registry.backend_server(instance).unwrap();
        assert_eq!(published.server_epoch, Some(epoch));
        assert_eq!(published.server_pid, Some(i64::from(pid)));
        assert_eq!(
            published.server_start_token.as_deref(),
            Some(start_token.as_str())
        );
        assert_eq!(published.socket_dev, Some(socket_metadata.dev() as i64));
        assert_eq!(published.socket_ino, Some(socket_metadata.ino() as i64));

        let descriptor_path = runtime.join(crate::runtime::WEZ_DESCRIPTOR_FILE);
        fs::write(
            &descriptor_path,
            serde_json::to_vec(&serde_json::json!({
                "descriptor_version": 1,
                "state": "ready",
                "epoch": epoch.0,
                "pid": pid,
                "socket": socket,
                "start_token": start_token,
                "boot_id": boot_id,
                "socket_dev": socket_metadata.dev(),
                "socket_ino": socket_metadata.ino(),
                "backend_instance_uid": instance.0,
                "boot_nonce": Uuid::new_v4(),
                "sentinel_window_id": 0,
                "sentinel_tab_id": 0,
                "sentinel_pane_id": 0,
                "sentinel_fallback": false,
                "written_by": "mux-startup",
                "written_at": "2026-08-17T00:00:00Z",
            }))
            .unwrap(),
        )
        .unwrap();
        fs::set_permissions(&descriptor_path, fs::Permissions::from_mode(0o600)).unwrap();
        let verified = crate::runtime::read_verified_ready_wez_descriptor_in(
            &runtime,
            Some(instance.0),
            Some(epoch.0),
        )
        .unwrap()
        .unwrap();
        assert_eq!(verified.pid, pid);
    }
}

#[cfg(test)]
mod private_spool_race_tests {
    use super::*;
    use std::os::unix::fs::DirBuilderExt;

    fn spool_dir(scratch: &Path) -> PrivateDir {
        let path = scratch.join("spool");
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        PrivateDir::open(&path).unwrap()
    }

    /// The state a loaded coordinator hits: Lua renames the next response over
    /// `response.json` while the poller already holds the previous inode open,
    /// dropping that inode to zero links.  Classifying that as a permission
    /// verdict failed whole recovery runs; it must read as retryable instead.
    #[test]
    fn response_replaced_under_an_open_reader_is_transient_not_a_permission_verdict() {
        let scratch = tempfile::tempdir().unwrap();
        let dir = spool_dir(scratch.path());
        dir.atomic_replace("response.json", b"first\n", MAX_RECOVERY_MESSAGE_BYTES)
            .unwrap();

        let held = dir
            .open_file("response.json", libc::O_RDONLY | libc::O_NONBLOCK, 0)
            .unwrap();
        validate_private_file(
            "response.json",
            &held.metadata().unwrap(),
            MAX_RECOVERY_MESSAGE_BYTES,
        )
        .expect("the published response is private before it is replaced");

        dir.atomic_replace("response.json", b"second\n", MAX_RECOVERY_MESSAGE_BYTES)
            .unwrap();

        let error = validate_private_file(
            "response.json",
            &held.metadata().unwrap(),
            MAX_RECOVERY_MESSAGE_BYTES,
        )
        .expect_err("an inode replaced under the reader must never be accepted");
        assert_eq!(error.kind(), io::ErrorKind::Interrupted, "{error}");
        assert!(
            spool_read_is_transient(&RecoveryError::Io(error)),
            "the response poller must retry a replaced inode, not fail the run"
        );
        assert_eq!(
            dir.read_file("response.json", MAX_RECOVERY_MESSAGE_BYTES)
                .unwrap()
                .as_slice(),
            b"second\n".as_slice(),
            "the retry must observe the settled document"
        );
    }

    /// The transient carve-out must not blunt the check itself: a linked file
    /// that fails it is a verdict the poller has to surface, not spin on.
    #[test]
    fn a_group_readable_response_stays_a_terminal_permission_verdict() {
        let scratch = tempfile::tempdir().unwrap();
        let dir = spool_dir(scratch.path());
        let file = dir
            .open_file(
                "response.json",
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                0o600,
            )
            .unwrap();
        // fchmod, not the open mode: umask would otherwise decide the outcome.
        assert_eq!(unsafe { libc::fchmod(file.as_raw_fd(), 0o644) }, 0);

        let error = dir
            .read_file("response.json", MAX_RECOVERY_MESSAGE_BYTES)
            .expect_err("a group-readable response must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied, "{error}");
        assert!(!spool_read_is_transient(&RecoveryError::Io(error)));
    }
}
