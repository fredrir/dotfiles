//! Authenticated file-spool client for the dmux WezTerm GUI bridge.
//!
//! The GUI is a presentation plane only: requests may attach/detach a
//! verified domain, activate an existing workspace, focus a correlated
//! logical child, show a toast, or perform the guarded detach-before-quit
//! flow.  Nothing in this module can remove native resources or execute an
//! arbitrary command (plan §13.2, ADR 003).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::{CStr, CString, OsString};
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::bootstrap::MarkerContext;
use crate::model::{Backend, BackendInstanceUid, ChildKind, HostUid, ProviderHandle};
use crate::refs::parse_ref;
use crate::registry::sha256::sha256;
use crate::registry::{NetworkClass, Transport};
use crate::remote::wez_compat::{unquoted_shell_word, valid_managed_socket};

pub const BRIDGE_PROTOCOL_VERSION: u64 = 1;
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;
pub const REQUEST_TTL: Duration = Duration::from_secs(10);
pub const ACK_TIMEOUT: Duration = Duration::from_secs(5);
pub const HEARTBEAT_MAX_AGE: Duration = Duration::from_secs(2);
pub const STATUS_MAX_AGE: Duration = Duration::from_secs(2);

const MAX_JSON_INTEGER: u64 = 9_007_199_254_740_991;
const MAX_JSON_SIGNED_INTEGER: i64 = 9_007_199_254_740_991;
const MAX_GUI_INSTANCE_BYTES: usize = 160;

const BRIDGE_DIR: &str = "bridge";
const KEY_FILE: &str = "key";
const KEY_BOOT_FILE: &str = "key.boot";

/// Live per-GUI heartbeat. The GUI rebuilds `panes` from GUI-local panes and
/// their v1 user variables each poll; clients use the exact marker tuple to
/// choose one instance and never guess from a process name or static host.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeHeartbeat {
    pub protocol_version: u64,
    pub gui_instance: String,
    pub pid: u32,
    pub process_start_token: String,
    pub updated_at: u64,
    pub panes: Vec<BridgePane>,
    pub domains: BTreeMap<String, BridgeDomainState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeDomainState {
    pub state: String,
    pub has_any_panes: bool,
    /// Config-sanitized backend instance for one managed persistent domain.
    /// `local` and other nonpersistent GUI domains omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_instance_uid: Option<BackendInstanceUid>,
    pub pane_count: u32,
    pub valid_marker_pane_count: u32,
    pub system_pane_count: u32,
    /// Exact reserved GUI-local workspace observed for this domain. It is
    /// present only while the domain is attached and must encode
    /// `dmux:system:<system_epoch>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_workspace: Option<String>,
    /// Epoch parsed from the one exact system workspace above. This is GUI
    /// observation, never owner authority; Rust compares it to the freshly
    /// probed owner descriptor before authorizing detach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_epoch: Option<crate::model::ServerEpoch>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgePane {
    /// GUI-local ID. Owner-side Wez IDs are never applied to the GUI.
    pub pane_id: u64,
    pub domain: String,
    /// Attach-time exact tmux client locator. This is required for tmux
    /// markers and forbidden for Wez markers; owner-side PID/start/tty and
    /// active-child validation remains the authorization boundary.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_uuid_wire"
    )]
    pub tmux_client_uid: Option<Uuid>,
    #[serde(with = "marker_wire")]
    pub context: MarkerContext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeSelection {
    pub gui_instance: String,
    pub pid: u32,
    pub process_start_token: String,
    pub pane_id: u64,
    pub domain: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeInstanceSelection {
    pub gui_instance: String,
    pub pid: u32,
    pub process_start_token: String,
    pub domains: BTreeMap<String, BridgeDomainState>,
}

/// Action-discriminated request origin. `in_gui` is a locator that the
/// owner has revalidated against its registry and live scan. `cold_launcher`
/// carries no invented pane/Space and is bound to the broker-authenticated
/// launcher process plus the intended backend instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BridgeOrigin {
    InGui {
        gui_instance: String,
        pid: u32,
        process_start_token: String,
        pane_id: u64,
        domain: String,
        host_uid: HostUid,
        space_uid: crate::model::SpaceUid,
        space_no: crate::model::SpaceNo,
        backend: Backend,
        server_epoch: crate::model::ServerEpoch,
        group_ref: String,
        split_ref: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "optional_uuid_wire"
        )]
        tmux_client_uid: Option<Uuid>,
    },
    /// Exact resident GUI process used when no pane exists (zero-window
    /// lifecycle) or when a no-create presentation targets an already-live
    /// managed GUI. The consumer binds this to its held fork lease directly.
    ResidentGui {
        gui_instance: String,
        pid: u32,
        process_start_token: String,
    },
    ColdLauncher {
        gui_instance: String,
        uid: u64,
        pid: u64,
        start_token: String,
        launcher_request_uid: Uuid,
        domain: String,
        host_uid: HostUid,
        backend_instance_uid: BackendInstanceUid,
        server_epoch: crate::model::ServerEpoch,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        space_uid: Option<crate::model::SpaceUid>,
    },
}

/// Exact origin JSON supplied by Lua to `dmux _gui --origin-json`. This is
/// deliberately distinct from [`BridgeOrigin`]: the CLI locator carries the
/// complete marker for owner revalidation, while only the revalidated subset
/// enters a signed bridge request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuiCliOrigin {
    pub protocol_version: u64,
    pub gui_instance: String,
    pub pane_id: u64,
    pub domain: String,
    /// Attach-time locator for a tmux client. It is never authorization:
    /// the owner revalidates its private PID/start-token/tty record and the
    /// exact active session/window/pane before dispatching any GUI action.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_uuid_wire"
    )]
    pub tmux_client_uid: Option<Uuid>,
    #[serde(with = "marker_wire")]
    pub marker: MarkerContext,
}

/// Pane-free lifecycle origin emitted only by the managed GUI process.  It
/// identifies the exact held GUI lease; the consumer additionally requires
/// broker-established resident provenance before accepting any signed
/// request derived from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuiResidentCliOrigin {
    pub protocol_version: u64,
    pub kind: String,
    pub gui_instance: String,
    pub pid: u32,
    pub process_start_token: String,
}

/// Exact, short-lived status cache consumed by the Lua status renderer.
/// The marker echo is intentionally complete: Lua accepts the record only
/// when it exactly matches the current pane's marker and is at most two
/// seconds old.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuiStatusCache {
    pub schema_version: u64,
    pub gui_instance: String,
    pub pane_id: u64,
    pub validated_at: i64,
    pub ok: bool,
    #[serde(with = "marker_wire")]
    pub marker: MarkerContext,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<GuiStatusDisplay>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuiStatusDisplay {
    pub logical_ref: String,
    pub space_name: String,
    pub backend: Backend,
    pub owner_alias: String,
    pub owner_label: String,
    pub route: String,
    pub group_count: u32,
    pub split_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_name: Option<String>,
}

/// Authority-validated live Space row returned by `_gui spaces`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuiSpaceRow {
    #[serde(rename = "ref")]
    pub stable_ref: String,
    pub name: String,
    pub backend: Backend,
    pub owner_alias: String,
    pub owner_label: String,
    pub route: String,
    pub attached: bool,
    pub health: String,
}

/// Caller-supplied, authority-validated route facts used to construct the
/// GUI's dynamic remote-domain manifest. Registry access deliberately stays
/// above this presentation module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteDomainSource {
    pub name: String,
    pub remote_address: String,
    pub username: String,
    pub remote_wezterm_path: Option<String>,
    pub managed_socket: Option<String>,
    pub host_uid: HostUid,
    pub backend_instance_uid: BackendInstanceUid,
    pub route_id: i64,
    pub priority: i64,
    pub transport: Transport,
    pub network_class: NetworkClass,
    pub compatible: bool,
    pub unavailable_reason: Option<String>,
}

/// Exact serialized row consumed by `wez.remote`; alternate domains are
/// derived here from identical HostUid/backend-instance pairs and never
/// trusted from a caller or config file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuiDomainManifestRow {
    pub name: String,
    pub remote_address: String,
    pub username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_wezterm_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_proxy_command: Option<String>,
    pub host_uid: HostUid,
    pub backend_instance_uid: BackendInstanceUid,
    pub route_id: i64,
    pub priority: i64,
    pub transport: String,
    pub network_class: String,
    pub alternate_domains: Vec<String>,
    pub compatible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

/// Version-1 acknowledgement. Optional result members are validated against
/// the request action; `deny_unknown_fields` prevents a v1 producer and
/// consumer from silently assigning different meanings to an extension.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeAck {
    pub protocol_version: u64,
    pub uid: String,
    pub action: String,
    pub nonce: String,
    pub ok: bool,
    pub completed_at: u64,
    pub request_sha256: String,
    pub gui_instance: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_ids: Option<Vec<u64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detached_domains: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reattached_domains: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub already_hidden: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pong: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toasted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resident_established: Option<bool>,
}

#[derive(Debug)]
pub enum GuiError {
    Io(io::Error),
    InvalidRequest(String),
    InvalidInstance(String),
    MessageTooLarge(usize),
    BridgeUnavailable(String),
    Timeout { uid: String },
    InvalidAck(String),
    Rejected { code: String, detail: String },
}

impl fmt::Display for GuiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GuiError::Io(e) => write!(f, "GUI bridge I/O: {e}"),
            GuiError::InvalidRequest(e) => write!(f, "invalid GUI bridge request: {e}"),
            GuiError::InvalidInstance(e) => write!(f, "invalid GUI bridge instance: {e}"),
            GuiError::MessageTooLarge(n) => {
                write!(
                    f,
                    "GUI bridge message is {n} bytes (maximum {MAX_MESSAGE_BYTES})"
                )
            }
            GuiError::BridgeUnavailable(e) => write!(f, "GUI bridge unavailable: {e}"),
            GuiError::Timeout { uid } => write!(f, "GUI bridge timed out waiting for {uid}"),
            GuiError::InvalidAck(e) => write!(f, "invalid GUI bridge acknowledgement: {e}"),
            GuiError::Rejected { code, detail } => {
                write!(f, "GUI bridge rejected the request ({code}): {detail}")
            }
        }
    }
}

impl std::error::Error for GuiError {}

impl From<io::Error> for GuiError {
    fn from(value: io::Error) -> Self {
        GuiError::Io(value)
    }
}

/// Initialize the 0700 bridge root and its per-OS-boot 0600 256-bit HMAC key.
///
/// A private `key.boot` token distinguishes OS boots even if a platform's
/// runtime storage unexpectedly persists. Same-boot mux-service restarts
/// reuse the key. A boot mismatch rotates only while no live GUI heartbeat
/// exists; a fresh GUI makes reuse successful so service recovery can never
/// depend on closing the resident GUI.
pub fn ensure_bridge_key(runtime_dir: &Path) -> Result<Vec<u8>, GuiError> {
    let runtime = PrivateDir::open(runtime_dir, 0o700)?;
    let bridge = runtime.ensure_child(BRIDGE_DIR)?;
    let raw = random_key()?;
    let created = match bridge.write_new_atomic(KEY_FILE, &raw) {
        Ok(()) => true,
        Err(GuiError::Io(e)) if e.kind() == io::ErrorKind::AlreadyExists => false,
        Err(error) => return Err(error),
    };
    let boot = current_boot_token()?;
    let recorded = bridge
        .read_private_file_optional(KEY_BOOT_FILE, 256)?
        .map(|bytes| {
            String::from_utf8(bytes)
                .map_err(|_| GuiError::BridgeUnavailable("key.boot is not UTF-8".into()))
        })
        .transpose()?;
    if created {
        bridge.write_replace_atomic(KEY_BOOT_FILE, boot.as_bytes())?;
    } else if recorded.as_deref() != Some(boot.as_str()) {
        if any_fresh_heartbeat(&bridge)? {
            // A process cannot survive an OS boot; a fresh GUI proves this
            // key is the one in use in the current boot. Refresh incomplete
            // or stale metadata without invalidating that consumer.
            bridge.write_replace_atomic(KEY_BOOT_FILE, boot.as_bytes())?;
        } else {
            bridge.write_replace_atomic(KEY_FILE, &random_key()?)?;
            bridge.write_replace_atomic(KEY_BOOT_FILE, boot.as_bytes())?;
        }
    }
    read_bridge_key(runtime_dir)
}

pub fn read_bridge_key(runtime_dir: &Path) -> Result<Vec<u8>, GuiError> {
    let runtime = PrivateDir::open(runtime_dir, 0o700)?;
    let bridge = runtime.child(BRIDGE_DIR)?;
    let key = bridge.read_private_file(KEY_FILE, 32)?;
    if key.len() != 32 {
        return Err(GuiError::BridgeUnavailable(format!(
            "{}/{} contains {} key bytes, expected 32",
            bridge.path.display(),
            KEY_FILE,
            key.len()
        )));
    }
    Ok(key)
}

/// Explicitly rotate the bridge key for provisioning/tests while the GUI is idle.
///
/// Normal mux-service startup calls [`ensure_bridge_key`], which performs
/// OS-boot-aware initialization and safe reuse. This explicit operation is
/// not part of every mux epoch: it refuses while any fresh GUI heartbeat
/// exists, so it cannot invalidate a live consumer.
pub fn rotate_bridge_key_if_idle(runtime_dir: &Path) -> Result<Vec<u8>, GuiError> {
    let runtime = PrivateDir::open(runtime_dir, 0o700)?;
    let bridge = runtime.ensure_child(BRIDGE_DIR)?;
    if any_fresh_heartbeat(&bridge)? {
        return Err(GuiError::BridgeUnavailable(
            "refusing to rotate the per-boot bridge key while a live GUI heartbeat exists".into(),
        ));
    }
    if let Some(existing) = bridge.open_private_file_optional(KEY_FILE)? {
        drop(existing);
    }
    bridge.write_replace_atomic(KEY_FILE, &random_key()?)?;
    bridge.write_replace_atomic(KEY_BOOT_FILE, current_boot_token()?.as_bytes())?;
    read_bridge_key(runtime_dir)
}

/// Canonical request bytes signed by both Rust and Lua.
///
/// This is compact JSON with recursively sorted object keys, array order
/// retained, integer-only numbers, and the top-level `hmac_sha256` member
/// omitted. Nulls and fractional/exponent numbers are rejected so Lua and
/// Rust cannot disagree about their spelling.
pub fn canonical_request_bytes(request: &Value) -> Result<Vec<u8>, GuiError> {
    let object = request
        .as_object()
        .ok_or_else(|| GuiError::InvalidRequest("top level must be an object".into()))?;
    let mut unsigned = Map::new();
    for (key, value) in object {
        if key != "hmac_sha256" {
            unsigned.insert(key.clone(), value.clone());
        }
    }
    let mut out = String::new();
    write_canonical(&Value::Object(unsigned), &mut out)?;
    Ok(out.into_bytes())
}

pub fn sign_request(request: &mut Value, key: &[u8]) -> Result<String, GuiError> {
    if key.len() != 32 {
        return Err(GuiError::InvalidRequest(
            "bridge HMAC key must be exactly 32 bytes".into(),
        ));
    }
    validate_request_for_instance(request, None)?;
    let bytes = canonical_request_bytes(request)?;
    let signature = hmac_sha256_hex(key, &bytes);
    request
        .as_object_mut()
        .expect("validated object")
        .insert("hmac_sha256".into(), Value::String(signature.clone()));
    Ok(signature)
}

pub fn verify_request(request: &Value, key: &[u8]) -> Result<(), GuiError> {
    if key.len() != 32 {
        return Err(GuiError::InvalidRequest(
            "bridge HMAC key must be exactly 32 bytes".into(),
        ));
    }
    validate_request_for_instance(request, None)?;
    let signature = request
        .get("hmac_sha256")
        .and_then(Value::as_str)
        .ok_or_else(|| GuiError::InvalidRequest("hmac_sha256 is missing".into()))?;
    if signature.len() != 64
        || !signature
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(GuiError::InvalidRequest(
            "hmac_sha256 must be 64 hexadecimal characters".into(),
        ));
    }
    let expected = hmac_sha256_hex(key, &canonical_request_bytes(request)?);
    if !constant_time_eq(signature.as_bytes(), expected.as_bytes()) {
        return Err(GuiError::InvalidRequest("HMAC verification failed".into()));
    }
    Ok(())
}

/// Atomically publish one signed request to a specific live GUI instance and
/// wait boundedly for its acknowledgement. Instance selection/correlation
/// happens above this low-level transport function.
pub fn call_instance(
    runtime_dir: &Path,
    instance: &str,
    request: &mut Value,
    timeout: Duration,
) -> Result<Value, GuiError> {
    validate_instance(instance)?;
    let key = read_bridge_key(runtime_dir)?;
    sign_request(request, &key)?;
    validate_request_for_instance(request, Some(instance))?;
    validate_request_time(request, unix_seconds()?)?;
    let bytes = serde_json::to_vec(request)
        .map_err(|e| GuiError::InvalidRequest(format!("serialize: {e}")))?;
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(GuiError::MessageTooLarge(bytes.len()));
    }

    let uid = string_field(request, "uid")?.to_string();
    let digest = hex(&sha256(&canonical_request_bytes(request)?));

    let runtime = PrivateDir::open(runtime_dir, 0o700)?;
    let bridge = runtime.child(BRIDGE_DIR)?;
    let instances = bridge.child("instances")?;
    let instance_dir = instances.child(instance)?;
    let now = unix_seconds()?;
    let heartbeat = read_live_heartbeat(&instance_dir, instance, now)?;
    let origin = request
        .get("origin")
        .and_then(Value::as_object)
        .expect("validated request origin");
    if matches!(
        origin.get("kind").and_then(Value::as_str),
        Some("in_gui" | "resident_gui")
    ) {
        let signed_pid = json_uint(origin.get("pid"), "origin.pid")?;
        let signed_start = origin
            .get("process_start_token")
            .and_then(Value::as_str)
            .expect("validated in_gui process_start_token");
        if signed_pid != u64::from(heartbeat.pid) || signed_start != heartbeat.process_start_token {
            return Err(GuiError::InvalidInstance(
                "signed GUI origin process incarnation differs from the fresh target heartbeat"
                    .into(),
            ));
        }
    }
    let requests = instance_dir.child("requests")?;
    let acks = instance_dir.child("acks")?;
    let consumed = instance_dir.child("consumed")?;

    let request_name = format!("req-{uid}.json");
    let ack_name = format!("ack-{uid}.json");

    // A lost acknowledgement may lead the caller to retry the exact signed
    // request. The original ack is authoritative and remains byte-identical;
    // validate its request digest before accepting it. A different request
    // reusing the UID fails closed.
    if let Some(ack_bytes) = acks.read_private_file_optional(&ack_name, MAX_MESSAGE_BYTES)? {
        return decode_and_validate_ack(&ack_bytes, request, instance, &digest);
    }

    if let Some(prior) = consumed.read_private_file_optional(&request_name, MAX_MESSAGE_BYTES)? {
        validate_same_request(&prior, request, &key, instance, &digest)?;
    }

    match requests.write_new_atomic(&request_name, &bytes) {
        Ok(()) => {}
        Err(GuiError::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists => {
            let prior = requests.read_private_file(&request_name, MAX_MESSAGE_BYTES)?;
            validate_same_request(&prior, request, &key, instance, &digest)?;
        }
        Err(error) => return Err(error),
    }

    let started = Instant::now();
    let timeout = timeout.min(ACK_TIMEOUT);
    loop {
        if let Some(ack_bytes) = acks.read_private_file_optional(&ack_name, MAX_MESSAGE_BYTES)? {
            return decode_and_validate_ack(&ack_bytes, request, instance, &digest);
        }
        if started.elapsed() >= timeout {
            return Err(GuiError::Timeout { uid });
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub fn bridge_root(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(BRIDGE_DIR)
}

/// Read the fresh heartbeat for one already-bound GUI instance without
/// rediscovering it from pane membership. This is the post-detach seam:
/// once a domain is detached the origin pane is intentionally absent, but
/// safe quit still has to prove that the exact GUI reports that domain
/// detached with no remaining panes before it may hide or quit.
pub fn read_instance_heartbeat(
    runtime_dir: &Path,
    gui_instance: &str,
) -> Result<BridgeHeartbeat, GuiError> {
    validate_instance(gui_instance)?;
    let runtime = PrivateDir::open(runtime_dir, 0o700)?;
    let bridge = runtime
        .child(BRIDGE_DIR)
        .map_err(|e| GuiError::BridgeUnavailable(format!("no registered GUI bridge: {e}")))?;
    let instances = bridge
        .child("instances")
        .map_err(|e| GuiError::BridgeUnavailable(format!("no registered GUI instances: {e}")))?;
    let instance = instances.child(gui_instance).map_err(|e| {
        GuiError::BridgeUnavailable(format!("GUI {gui_instance} is not registered: {e}"))
    })?;
    read_live_heartbeat(&instance, gui_instance, unix_seconds()?)
}

/// Select the one live GUI instance whose current GUI-local pane carries the
/// exact revalidated marker. Zero or multiple matches fail closed.
pub fn discover_in_gui_instance(
    runtime_dir: &Path,
    marker: &MarkerContext,
) -> Result<BridgeSelection, GuiError> {
    discover_in_gui_instance_with_heartbeat(runtime_dir, marker).map(|(selection, _)| selection)
}

fn discover_in_gui_instance_with_heartbeat(
    runtime_dir: &Path,
    marker: &MarkerContext,
) -> Result<(BridgeSelection, BridgeHeartbeat), GuiError> {
    validate_marker(marker)?;
    let runtime = PrivateDir::open(runtime_dir, 0o700)?;
    let bridge = runtime
        .child(BRIDGE_DIR)
        .map_err(|e| GuiError::BridgeUnavailable(format!("no registered GUI bridge: {e}")))?;
    let instances = bridge
        .child("instances")
        .map_err(|e| GuiError::BridgeUnavailable(format!("no registered GUI instances: {e}")))?;
    let now = unix_seconds()?;
    let mut matches = Vec::new();
    for name in instances.entry_names()? {
        let Some(instance) = name.to_str() else {
            continue;
        };
        if validate_instance(instance).is_err() {
            continue;
        }
        let instance_dir = match instances.child(instance) {
            Ok(dir) => dir,
            Err(_) => continue,
        };
        let heartbeat = match read_live_heartbeat(&instance_dir, instance, now) {
            Ok(heartbeat) => heartbeat,
            Err(_) => continue,
        };
        let pane_matches: Vec<&BridgePane> = heartbeat
            .panes
            .iter()
            .filter(|pane| exact_marker(&pane.context, marker))
            .collect();
        if let [pane] = pane_matches.as_slice() {
            let selection = BridgeSelection {
                gui_instance: heartbeat.gui_instance.clone(),
                pid: heartbeat.pid,
                process_start_token: heartbeat.process_start_token.clone(),
                pane_id: pane.pane_id,
                domain: pane.domain.clone(),
            };
            matches.push((selection, heartbeat));
        } else if pane_matches.len() > 1 {
            return Err(GuiError::InvalidInstance(format!(
                "GUI {} has multiple panes claiming Split {}",
                heartbeat.gui_instance, marker.split_ref
            )));
        }
    }
    match matches.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(GuiError::BridgeUnavailable(format!(
            "no live GUI instance contains validated Split {}",
            marker.split_ref
        ))),
        many => Err(GuiError::InvalidInstance(format!(
            "{} live GUI instances contain validated Split {}; choose one explicitly",
            many.len(),
            marker.split_ref
        ))),
    }
}

/// Bind Lua's untrusted locator to the marker the owner has just revalidated
/// against registry + live inventory, then require the fresh GUI heartbeat
/// to name the same instance, pane, and imported domain.
pub fn bind_cli_origin(
    runtime_dir: &Path,
    cli: &GuiCliOrigin,
    authoritative_marker: &MarkerContext,
) -> Result<BridgeSelection, GuiError> {
    bind_cli_origin_with_heartbeat(runtime_dir, cli, authoritative_marker)
        .map(|(selection, _)| selection)
}

/// Bind Lua's untrusted locator and also return the same descriptor-read,
/// fresh heartbeat used for the binding decision. Safe disconnect/quit uses
/// the complete domain and pane snapshot for its before-state; callers must
/// still perform authority-aware owner scans to prove pane survival after a
/// detach acknowledgement.
pub fn bind_cli_origin_with_heartbeat(
    runtime_dir: &Path,
    cli: &GuiCliOrigin,
    authoritative_marker: &MarkerContext,
) -> Result<(BridgeSelection, BridgeHeartbeat), GuiError> {
    validate_cli_origin(cli)?;
    validate_marker(authoritative_marker)?;
    if !exact_marker(&cli.marker, authoritative_marker) {
        return Err(GuiError::InvalidInstance(
            "GUI marker differs from the owner-revalidated context".into(),
        ));
    }
    // The CLI-provided instance/pane pair is only a locator. Reading that
    // exact fresh private heartbeat and matching every marker/UID field is
    // stronger than marker-only global discovery and remains unambiguous
    // when two exact tmux clients in one GUI show the same session child.
    let heartbeat = read_instance_heartbeat(runtime_dir, &cli.gui_instance)?;
    let panes: Vec<&BridgePane> = heartbeat
        .panes
        .iter()
        .filter(|pane| pane.pane_id == cli.pane_id)
        .collect();
    let [pane] = panes.as_slice() else {
        return Err(GuiError::InvalidInstance(
            "fresh heartbeat does not contain exactly one GUI CLI pane id".into(),
        ));
    };
    if pane.domain != cli.domain
        || !exact_marker(&pane.context, authoritative_marker)
        || pane.tmux_client_uid != cli.tmux_client_uid
    {
        return Err(GuiError::InvalidInstance(
            "fresh heartbeat pane marker/client identity differs from the GUI CLI origin".into(),
        ));
    }
    let selection = BridgeSelection {
        gui_instance: heartbeat.gui_instance.clone(),
        pid: heartbeat.pid,
        process_start_token: heartbeat.process_start_token.clone(),
        pane_id: pane.pane_id,
        domain: pane.domain.clone(),
    };
    Ok((selection, heartbeat))
}

/// Discover the one resident live GUI without requiring a pane marker. This
/// is the zero-window `summon` seam: a fresh unique heartbeat is sufficient,
/// but zero or multiple GUI processes are never guessed between.
pub fn discover_single_live_instance(
    runtime_dir: &Path,
) -> Result<BridgeInstanceSelection, GuiError> {
    let runtime = PrivateDir::open(runtime_dir, 0o700)?;
    let bridge = runtime
        .child(BRIDGE_DIR)
        .map_err(|e| GuiError::BridgeUnavailable(format!("no registered GUI bridge: {e}")))?;
    let instances = bridge
        .child("instances")
        .map_err(|e| GuiError::BridgeUnavailable(format!("no registered GUI instances: {e}")))?;
    let now = unix_seconds()?;
    let mut live = Vec::new();
    for name in instances.entry_names()? {
        let Some(instance) = name.to_str() else {
            continue;
        };
        if validate_instance(instance).is_err() {
            continue;
        }
        let dir = match instances.child(instance) {
            Ok(dir) => dir,
            Err(_) => continue,
        };
        let heartbeat = match read_live_heartbeat(&dir, instance, now) {
            Ok(heartbeat) => heartbeat,
            Err(_) => continue,
        };
        live.push(BridgeInstanceSelection {
            gui_instance: heartbeat.gui_instance,
            pid: heartbeat.pid,
            process_start_token: heartbeat.process_start_token,
            domains: heartbeat.domains,
        });
    }
    match live.as_slice() {
        [instance] => Ok(instance.clone()),
        [] => Err(GuiError::BridgeUnavailable(
            "no live resident GUI instance is registered".into(),
        )),
        many => Err(GuiError::InvalidInstance(format!(
            "{} live GUI instances are registered; refusing to guess",
            many.len()
        ))),
    }
}

/// Build the origin object placed in a signed in-GUI request. The consumer
/// rechecks every marker field against its current pane before acting.
pub fn in_gui_origin(
    selection: &BridgeSelection,
    marker: &MarkerContext,
    tmux_client_uid: Option<Uuid>,
) -> Value {
    serde_json::to_value(BridgeOrigin::InGui {
        gui_instance: selection.gui_instance.clone(),
        pid: selection.pid,
        process_start_token: selection.process_start_token.clone(),
        pane_id: selection.pane_id,
        domain: selection.domain.clone(),
        host_uid: marker.host_uid,
        space_uid: marker.space_uid,
        space_no: marker.space_no,
        backend: marker.backend,
        server_epoch: marker.server_epoch,
        group_ref: marker.group_ref.clone(),
        split_ref: marker.split_ref.clone(),
        tmux_client_uid,
    })
    .expect("BridgeOrigin always serializes")
}

pub fn resident_gui_origin(selection: &BridgeInstanceSelection) -> Value {
    serde_json::to_value(BridgeOrigin::ResidentGui {
        gui_instance: selection.gui_instance.clone(),
        pid: selection.pid,
        process_start_token: selection.process_start_token.clone(),
    })
    .expect("BridgeOrigin always serializes")
}

pub fn parse_origin_json(input: &str) -> Result<GuiCliOrigin, GuiError> {
    if input.len() > MAX_MESSAGE_BYTES {
        return Err(GuiError::MessageTooLarge(input.len()));
    }
    let raw: Value = serde_json::from_str(input)
        .map_err(|e| GuiError::InvalidRequest(format!("origin JSON: {e}")))?;
    if contains_null(&raw) {
        return invalid("origin JSON optional fields must be omitted, never null");
    }
    let origin: GuiCliOrigin = serde_json::from_str(input)
        .map_err(|e| GuiError::InvalidRequest(format!("origin JSON: {e}")))?;
    validate_cli_origin(&origin)?;
    Ok(origin)
}

pub fn parse_resident_origin_json(input: &str) -> Result<GuiResidentCliOrigin, GuiError> {
    if input.len() > MAX_MESSAGE_BYTES {
        return Err(GuiError::MessageTooLarge(input.len()));
    }
    let raw: Value = serde_json::from_str(input)
        .map_err(|e| GuiError::InvalidRequest(format!("resident origin JSON: {e}")))?;
    if contains_null(&raw) {
        return invalid("resident origin JSON fields must never be null");
    }
    let origin: GuiResidentCliOrigin = serde_json::from_str(input)
        .map_err(|e| GuiError::InvalidRequest(format!("resident origin JSON: {e}")))?;
    if origin.protocol_version != BRIDGE_PROTOCOL_VERSION || origin.kind != "resident_gui" {
        return invalid("resident origin protocol_version/kind is invalid");
    }
    validate_instance(&origin.gui_instance)?;
    if origin.pid == 0 {
        return invalid("resident origin pid must be nonzero");
    }
    validate_string(
        &origin.process_start_token,
        "resident origin process_start_token",
        256,
    )?;
    Ok(origin)
}

pub fn parse_signed_origin_json(input: &str) -> Result<BridgeOrigin, GuiError> {
    if input.len() > MAX_MESSAGE_BYTES {
        return Err(GuiError::MessageTooLarge(input.len()));
    }
    let raw: Value = serde_json::from_str(input)
        .map_err(|e| GuiError::InvalidRequest(format!("signed origin JSON: {e}")))?;
    if contains_null(&raw) {
        return invalid("signed origin optional fields must be omitted, never null");
    }
    // Validate the original JSON before deserializing typed UUID wrappers;
    // otherwise serde could accept a noncanonical UUID spelling and erase
    // that evidence when the value is serialized again.
    validate_origin(&raw, None)?;
    let origin: BridgeOrigin = serde_json::from_str(input)
        .map_err(|e| GuiError::InvalidRequest(format!("signed origin JSON: {e}")))?;
    Ok(origin)
}

pub fn cold_launcher_origin(
    gui_instance: String,
    uid: u64,
    pid: u64,
    start_token: String,
    launcher_request_uid: Uuid,
    domain: String,
    host_uid: HostUid,
    backend_instance_uid: BackendInstanceUid,
    server_epoch: crate::model::ServerEpoch,
    space_uid: Option<crate::model::SpaceUid>,
) -> Result<Value, GuiError> {
    let value = serde_json::to_value(BridgeOrigin::ColdLauncher {
        gui_instance,
        uid,
        pid,
        start_token,
        launcher_request_uid,
        domain,
        host_uid,
        backend_instance_uid,
        server_epoch,
        space_uid,
    })
    .map_err(|e| GuiError::InvalidRequest(format!("cold origin: {e}")))?;
    validate_origin(&value, None)?;
    Ok(value)
}

pub fn request_document(action: &str, target: Value, origin: Value) -> Result<Value, GuiError> {
    if !allowed_action(action) {
        return Err(GuiError::InvalidRequest(format!(
            "action {action:?} is outside the presentation allowlist"
        )));
    }
    if !target.is_object() || !origin.is_object() {
        return Err(GuiError::InvalidRequest(
            "target and origin must be objects".into(),
        ));
    }
    let issued_at = unix_seconds()?;
    let request = serde_json::json!({
        "protocol_version": BRIDGE_PROTOCOL_VERSION,
        "uid": Uuid::new_v4().to_string(),
        "action": action,
        "target": target,
        "issued_at": issued_at,
        "expiry": issued_at + REQUEST_TTL.as_secs(),
        "nonce": format!("{}{}", simple_nonce(), simple_nonce()),
        "replay_key": simple_nonce(),
        "origin": origin,
    });
    validate_request_for_instance(&request, None)?;
    Ok(request)
}

impl GuiStatusCache {
    pub fn success(
        gui_instance: String,
        pane_id: u64,
        marker: MarkerContext,
        display: GuiStatusDisplay,
    ) -> Result<Self, GuiError> {
        let record = Self {
            schema_version: 1,
            gui_instance,
            pane_id,
            validated_at: unix_seconds()? as i64,
            ok: true,
            marker,
            display: Some(display),
            error: None,
            message: None,
        };
        validate_status_cache(&record, record.validated_at as u64)?;
        Ok(record)
    }

    pub fn failure(
        gui_instance: String,
        pane_id: u64,
        marker: MarkerContext,
        error: String,
        message: String,
    ) -> Result<Self, GuiError> {
        let record = Self {
            schema_version: 1,
            gui_instance,
            pane_id,
            validated_at: unix_seconds()? as i64,
            ok: false,
            marker,
            display: None,
            error: Some(error),
            message: Some(message),
        };
        validate_status_cache(&record, record.validated_at as u64)?;
        Ok(record)
    }
}

pub fn validate_status_cache(record: &GuiStatusCache, now: u64) -> Result<(), GuiError> {
    if record.schema_version != 1 {
        return invalid("status cache schema_version must be 1");
    }
    validate_instance(&record.gui_instance)?;
    if record.pane_id > MAX_JSON_INTEGER {
        return invalid("status cache pane_id exceeds the exact JSON integer range");
    }
    if record.validated_at < 0 || record.validated_at as u64 > MAX_JSON_INTEGER {
        return invalid("status cache validated_at is not a non-negative exact JSON integer");
    }
    let validated_at = record.validated_at as u64;
    if validated_at > now.saturating_add(2)
        || now.saturating_sub(validated_at) > STATUS_MAX_AGE.as_secs()
    {
        return invalid("status cache record is stale or from the future");
    }
    validate_marker(&record.marker)?;
    match (
        record.ok,
        record.display.as_ref(),
        record.error.as_deref(),
        record.message.as_deref(),
    ) {
        (true, Some(display), None, None) => validate_status_display(display, &record.marker),
        (false, None, Some(error), Some(message)) => {
            validate_error_token(error)?;
            validate_string(message, "status.message", 4096)
        }
        _ => invalid(
            "status cache success requires display only; failure requires error/message only",
        ),
    }
}

/// Atomically replace the exact pane cache beneath a descriptor-verified
/// private directory chain. Existing symlinks or non-0600 files are refused
/// before replacement.
pub fn write_status_cache(
    runtime_dir: &Path,
    record: &GuiStatusCache,
) -> Result<PathBuf, GuiError> {
    validate_status_cache(record, unix_seconds()?)?;
    let runtime = PrivateDir::open(runtime_dir, 0o700)?;
    let bridge = runtime.child(BRIDGE_DIR)?;
    let instances = bridge.child("instances")?;
    let instance = instances.child(&record.gui_instance)?;
    let context = instance.ensure_child("context")?;
    let name = format!("{}.json", record.pane_id);
    if let Some(file) = context.open_private_file_optional(&name)? {
        drop(file);
    }
    let bytes = serde_json::to_vec(record)
        .map_err(|e| GuiError::InvalidRequest(format!("status cache JSON: {e}")))?;
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(GuiError::MessageTooLarge(bytes.len()));
    }
    context.write_replace_atomic(&name, &bytes)?;
    Ok(context.path.join(name))
}

pub fn build_domain_manifest(
    mut sources: Vec<RemoteDomainSource>,
) -> Result<Vec<GuiDomainManifestRow>, GuiError> {
    sources.sort_by(|left, right| {
        (left.priority, left.route_id, left.name.as_str()).cmp(&(
            right.priority,
            right.route_id,
            right.name.as_str(),
        ))
    });
    let mut names = BTreeSet::new();
    let mut route_ids = BTreeSet::new();
    for source in &sources {
        validate_domain_source(source)?;
        if !names.insert(source.name.clone()) {
            return invalid(format!("duplicate GUI domain name {:?}", source.name));
        }
        if !route_ids.insert(source.route_id) {
            return invalid(format!("duplicate GUI route_id {}", source.route_id));
        }
    }

    let mut rows = Vec::with_capacity(sources.len());
    for source in &sources {
        let alternate_domains = sources
            .iter()
            .filter(|other| {
                other.name != source.name
                    && other.host_uid == source.host_uid
                    && other.backend_instance_uid == source.backend_instance_uid
                    && other.compatible
            })
            .map(|other| other.name.clone())
            .collect();
        rows.push(GuiDomainManifestRow {
            name: source.name.clone(),
            remote_address: source.remote_address.clone(),
            username: source.username.clone(),
            remote_wezterm_path: source.remote_wezterm_path.clone(),
            override_proxy_command: source
                .managed_socket
                .as_deref()
                .zip(source.remote_wezterm_path.as_deref())
                .map(|(socket, path)| managed_proxy_command(path, socket)),
            host_uid: source.host_uid,
            backend_instance_uid: source.backend_instance_uid,
            route_id: source.route_id,
            priority: source.priority,
            transport: source.transport.as_str().to_string(),
            network_class: source.network_class.as_str().to_string(),
            alternate_domains,
            compatible: source.compatible,
            unavailable_reason: source.unavailable_reason.clone(),
        });
    }
    validate_domain_manifest(&rows)?;
    Ok(rows)
}

pub fn validate_domain_manifest(rows: &[GuiDomainManifestRow]) -> Result<(), GuiError> {
    let mut names = BTreeSet::new();
    let mut routes = BTreeSet::new();
    let mut instance_by_host = HashMap::new();
    let mut host_by_instance = HashMap::new();
    for row in rows {
        validate_domain(row.name.as_str(), "domain.name")?;
        validate_control_free_string(&row.remote_address, "domain.remote_address", 1024)?;
        validate_control_free_string(&row.username, "domain.username", 256)?;
        if row.route_id <= 0
            || row.route_id > MAX_JSON_SIGNED_INTEGER
            || !routes.insert(row.route_id)
        {
            return invalid("domain manifest route_id must be positive and unique");
        }
        if !(-MAX_JSON_SIGNED_INTEGER..=MAX_JSON_SIGNED_INTEGER).contains(&row.priority) {
            return invalid("domain manifest priority exceeds the exact JSON integer range");
        }
        if !names.insert(row.name.clone()) {
            return invalid("domain manifest names must be unique");
        }
        if instance_by_host
            .insert(row.host_uid, row.backend_instance_uid)
            .is_some_and(|existing| existing != row.backend_instance_uid)
        {
            return invalid(
                "domain manifest maps one HostUid to multiple backend instance incarnations",
            );
        }
        if host_by_instance
            .insert(row.backend_instance_uid, row.host_uid)
            .is_some_and(|existing| existing != row.host_uid)
        {
            return invalid(
                "domain manifest aliases one backend instance incarnation across multiple owners",
            );
        }
        if Transport::parse(&row.transport).is_none() || row.transport == "local" {
            return invalid("domain manifest transport must be openssh or wez-ssh");
        }
        if NetworkClass::parse(&row.network_class).is_none() {
            return invalid("domain manifest network_class is invalid");
        }
        match (
            row.compatible,
            row.remote_wezterm_path.as_deref(),
            row.override_proxy_command.as_deref(),
            row.unavailable_reason.as_deref(),
        ) {
            (true, Some(path), Some(command), None) => {
                validate_absolute_path(path, "domain.remote_wezterm_path")?;
                validate_proxy_command(command, path)?
            }
            (false, path, None, Some(reason)) => {
                if let Some(path) = path {
                    validate_absolute_path(path, "domain.remote_wezterm_path")?;
                }
                validate_control_free_string(reason, "unavailable_reason", 4096)?
            }
            _ => {
                return invalid(
                    "compatible domains require an absolute remote_wezterm_path plus its pinned proxy command and omit unavailable_reason; incompatible domains require unavailable_reason, carry no proxy command, and may omit the path",
                );
            }
        }
        let mut alternates = BTreeSet::new();
        for alternate in &row.alternate_domains {
            validate_domain(alternate, "alternate_domains[]")?;
            if alternate == &row.name || !alternates.insert(alternate) {
                return invalid("alternate_domains contains self or duplicate");
            }
        }
    }
    for row in rows {
        let expected: Vec<&str> = rows
            .iter()
            .filter(|other| {
                other.name != row.name
                    && other.host_uid == row.host_uid
                    && other.backend_instance_uid == row.backend_instance_uid
                    && other.compatible
            })
            .map(|other| other.name.as_str())
            .collect();
        let actual: Vec<&str> = row.alternate_domains.iter().map(String::as_str).collect();
        if actual != expected {
            return invalid("alternate_domains is not the stable same-owner/backend route set");
        }
    }
    Ok(())
}

pub fn validate_space_rows(rows: &[GuiSpaceRow]) -> Result<(), GuiError> {
    let mut refs = BTreeSet::new();
    for row in rows {
        validate_string(&row.stable_ref, "space.ref", 512)?;
        validate_string(&row.name, "space.name", 1024)?;
        validate_string(&row.owner_alias, "space.owner_alias", 64)?;
        validate_string(&row.owner_label, "space.owner_label", 128)?;
        validate_string(&row.route, "space.route", 256)?;
        validate_error_token(&row.health)?;
        if !refs.insert(&row.stable_ref) {
            return invalid("GUI spaces contains a duplicate stable ref");
        }
    }
    Ok(())
}

pub fn hmac_sha256_hex(key: &[u8], message: &[u8]) -> String {
    const BLOCK: usize = 64;
    let normalized = if key.len() > BLOCK {
        sha256(key).to_vec()
    } else {
        key.to_vec()
    };
    let mut key_block = [0u8; BLOCK];
    key_block[..normalized.len()].copy_from_slice(&normalized);
    let mut inner = Vec::with_capacity(BLOCK + message.len());
    let mut outer = Vec::with_capacity(BLOCK + 32);
    for byte in key_block {
        inner.push(byte ^ 0x36);
        outer.push(byte ^ 0x5c);
    }
    inner.extend_from_slice(message);
    outer.extend_from_slice(&sha256(&inner));
    hex(&sha256(&outer))
}

fn validate_request_for_instance(
    request: &Value,
    expected_instance: Option<&str>,
) -> Result<(), GuiError> {
    let object = exact_object(
        request,
        &[
            "action",
            "expiry",
            "hmac_sha256",
            "issued_at",
            "nonce",
            "origin",
            "protocol_version",
            "replay_key",
            "target",
            "uid",
        ],
        &[
            "action",
            "expiry",
            "issued_at",
            "nonce",
            "origin",
            "protocol_version",
            "replay_key",
            "target",
            "uid",
        ],
        "request",
    )?;
    if object.get("protocol_version").and_then(Value::as_u64) != Some(BRIDGE_PROTOCOL_VERSION) {
        return invalid("protocol_version must be 1");
    }
    validate_uuid(string_field(request, "uid")?, "request.uid")?;
    let action = string_field(request, "action")?;
    if !allowed_action(action) {
        return invalid("action is outside the presentation allowlist");
    }
    for field in ["nonce", "replay_key"] {
        let token = string_field(request, field)?;
        if !(32..=128).contains(&token.len()) || !is_lower_hex(token) {
            return invalid(format!(
                "request.{field} must be 32-128 lowercase hexadecimal characters"
            ));
        }
    }
    let issued_at = json_uint(object.get("issued_at"), "request.issued_at")?;
    let expiry = json_uint(object.get("expiry"), "request.expiry")?;
    let ttl = expiry.saturating_sub(issued_at);
    if expiry <= issued_at || !(1..=REQUEST_TTL.as_secs()).contains(&ttl) {
        return invalid(format!(
            "request TTL must be 1..={} seconds",
            REQUEST_TTL.as_secs()
        ));
    }
    if let Some(signature) = object.get("hmac_sha256") {
        let signature = signature
            .as_str()
            .ok_or_else(|| GuiError::InvalidRequest("hmac_sha256 must be a string".into()))?;
        if signature.len() != 64 || !is_lower_hex(signature) {
            return invalid("hmac_sha256 must be 64 lowercase hexadecimal characters");
        }
    }
    validate_origin(
        object
            .get("origin")
            .ok_or_else(|| GuiError::InvalidRequest("origin is missing".into()))?,
        expected_instance,
    )?;
    validate_target(
        action,
        object
            .get("target")
            .ok_or_else(|| GuiError::InvalidRequest("target is missing".into()))?,
    )?;

    let origin = object["origin"].as_object().expect("validated origin");
    if origin.get("kind").and_then(Value::as_str) == Some("resident_gui")
        && action == "detach_domain"
    {
        return invalid("resident_gui origin cannot issue standalone detach_domain");
    }
    if origin.get("kind").and_then(Value::as_str) == Some("cold_launcher") {
        if matches!(action, "detach_domain" | "focus_pane" | "safe_quit") {
            return invalid(format!("{action} requires an in_gui origin"));
        }
        if matches!(
            action,
            "attach_domain" | "activate" | "present" | "establish_resident"
        ) {
            let target = object["target"].as_object().expect("validated target");
            if target.get("domain") != origin.get("domain")
                || target.get("host_uid") != origin.get("host_uid")
                || target.get("backend_instance_uid") != origin.get("backend_instance_uid")
                || target.get("server_epoch") != origin.get("server_epoch")
                || target.get("space_uid") != origin.get("space_uid")
            {
                return invalid(
                    "cold_launcher origin target identity differs from the request target",
                );
            }
        }
    }
    Ok(())
}

fn validate_request_time(request: &Value, now: u64) -> Result<(), GuiError> {
    let issued_at = json_uint(request.get("issued_at"), "request.issued_at")?;
    let expiry = json_uint(request.get("expiry"), "request.expiry")?;
    if now >= expiry {
        return invalid("request reached its signed expiry");
    }
    if issued_at > now.saturating_add(2) {
        return invalid("request issued_at is too far in the future");
    }
    Ok(())
}

fn validate_origin(origin: &Value, expected_instance: Option<&str>) -> Result<(), GuiError> {
    let kind = origin
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| GuiError::InvalidRequest("origin.kind is missing".into()))?;
    match kind {
        "in_gui" => {
            let object = exact_object(
                origin,
                &[
                    "backend",
                    "domain",
                    "group_ref",
                    "gui_instance",
                    "host_uid",
                    "host_uid",
                    "kind",
                    "pane_id",
                    "pid",
                    "process_start_token",
                    "server_epoch",
                    "space_no",
                    "space_uid",
                    "split_ref",
                    "tmux_client_uid",
                ],
                &[
                    "backend",
                    "domain",
                    "group_ref",
                    "gui_instance",
                    "host_uid",
                    "host_uid",
                    "kind",
                    "pane_id",
                    "pid",
                    "process_start_token",
                    "server_epoch",
                    "space_no",
                    "space_uid",
                    "split_ref",
                ],
                "origin",
            )?;
            let instance = object["gui_instance"]
                .as_str()
                .ok_or_else(|| GuiError::InvalidRequest("origin.gui_instance is invalid".into()))?;
            validate_instance(instance)?;
            if expected_instance.is_some_and(|expected| expected != instance) {
                return invalid("origin.gui_instance does not name the selected bridge consumer");
            }
            let pid = json_uint(object.get("pid"), "origin.pid")?;
            if pid == 0 || pid > u64::from(u32::MAX) {
                return invalid("origin.pid must be a nonzero process ID");
            }
            validate_string(
                value_str(object, "process_start_token", "origin.process_start_token")?,
                "origin.process_start_token",
                256,
            )?;
            json_uint(object.get("pane_id"), "origin.pane_id")?;
            validate_domain(
                value_str(object, "domain", "origin.domain")?,
                "origin.domain",
            )?;
            validate_uuid(
                value_str(object, "host_uid", "origin.host_uid")?,
                "origin.host_uid",
            )?;
            for field in ["host_uid", "space_uid", "server_epoch"] {
                validate_uuid(
                    value_str(object, field, &format!("origin.{field}"))?,
                    &format!("origin.{field}"),
                )?;
            }
            let backend = match value_str(object, "backend", "origin.backend")? {
                "wez" => Backend::Wez,
                "tmux" => Backend::Tmux,
                _ => return invalid("origin.backend must be wez or tmux"),
            };
            if json_uint(object.get("space_no"), "origin.space_no")? == 0 {
                return invalid("origin.space_no must be nonzero");
            }
            let epoch = value_str(object, "server_epoch", "origin.server_epoch")?;
            for (field, kind) in [
                ("group_ref", ChildKind::Group),
                ("split_ref", ChildKind::Split),
            ] {
                let child = value_str(object, field, &format!("origin.{field}"))?;
                validate_child_ref(child, kind, epoch, &format!("origin.{field}"))?;
                let parsed = parse_ref(&format!("1/{child}"))
                    .expect("origin child was just validated")
                    .child
                    .expect("origin child is present");
                if !matches!(
                    (parsed.handle, backend),
                    (ProviderHandle::Wz(_), Backend::Wez)
                        | (ProviderHandle::Tx(_), Backend::Tmux)
                        | (ProviderHandle::Opaque(_), _)
                ) {
                    return invalid(format!(
                        "origin.{field} provider differs from origin.backend"
                    ));
                }
            }
            match (backend, object.get("tmux_client_uid")) {
                (Backend::Tmux, Some(value)) => {
                    validate_uuid(
                        value.as_str().ok_or_else(|| {
                            GuiError::InvalidRequest(
                                "origin.tmux_client_uid must be a UUID string".into(),
                            )
                        })?,
                        "origin.tmux_client_uid",
                    )?;
                }
                (Backend::Tmux, None) => {
                    return invalid("tmux in_gui origin requires tmux_client_uid");
                }
                (Backend::Wez, None) => {}
                (Backend::Wez, Some(_)) => {
                    return invalid("Wez in_gui origin forbids tmux_client_uid");
                }
            }
        }
        "resident_gui" => {
            let object = exact_object(
                origin,
                &["gui_instance", "kind", "pid", "process_start_token"],
                &["gui_instance", "kind", "pid", "process_start_token"],
                "origin",
            )?;
            let instance = value_str(object, "gui_instance", "origin.gui_instance")?;
            validate_instance(instance)?;
            if expected_instance.is_some_and(|expected| expected != instance) {
                return invalid("origin.gui_instance does not name the selected bridge consumer");
            }
            let pid = json_uint(object.get("pid"), "origin.pid")?;
            if pid == 0 || pid > u64::from(u32::MAX) {
                return invalid("origin.pid must be a nonzero process ID");
            }
            validate_string(
                value_str(object, "process_start_token", "origin.process_start_token")?,
                "origin.process_start_token",
                256,
            )?;
        }
        "cold_launcher" => {
            let object = exact_object(
                origin,
                &[
                    "backend_instance_uid",
                    "domain",
                    "gui_instance",
                    "host_uid",
                    "kind",
                    "launcher_request_uid",
                    "pid",
                    "server_epoch",
                    "space_uid",
                    "start_token",
                    "uid",
                ],
                &[
                    "backend_instance_uid",
                    "domain",
                    "gui_instance",
                    "host_uid",
                    "kind",
                    "launcher_request_uid",
                    "pid",
                    "server_epoch",
                    "start_token",
                    "uid",
                ],
                "origin",
            )?;
            let instance = value_str(object, "gui_instance", "origin.gui_instance")?;
            validate_instance(instance)?;
            if expected_instance.is_some_and(|expected| expected != instance) {
                return invalid("origin.gui_instance does not name the selected bridge consumer");
            }
            json_uint(object.get("uid"), "origin.uid")?;
            let pid = json_uint(object.get("pid"), "origin.pid")?;
            if pid == 0 {
                return invalid("origin.pid must be nonzero");
            }
            validate_string(
                value_str(object, "start_token", "origin.start_token")?,
                "origin.start_token",
                256,
            )?;
            validate_uuid(
                value_str(
                    object,
                    "launcher_request_uid",
                    "origin.launcher_request_uid",
                )?,
                "origin.launcher_request_uid",
            )?;
            validate_domain(
                value_str(object, "domain", "origin.domain")?,
                "origin.domain",
            )?;
            validate_uuid(
                value_str(object, "host_uid", "origin.host_uid")?,
                "origin.host_uid",
            )?;
            validate_uuid(
                value_str(
                    object,
                    "backend_instance_uid",
                    "origin.backend_instance_uid",
                )?,
                "origin.backend_instance_uid",
            )?;
            for field in ["server_epoch"] {
                validate_uuid(
                    value_str(object, field, &format!("origin.{field}"))?,
                    &format!("origin.{field}"),
                )?;
            }
            if let Some(space_uid) = object.get("space_uid") {
                validate_uuid(
                    space_uid.as_str().ok_or_else(|| {
                        GuiError::InvalidRequest("origin.space_uid must be a UUID string".into())
                    })?,
                    "origin.space_uid",
                )?;
            }
        }
        _ => return invalid("origin.kind must be in_gui, resident_gui, or cold_launcher"),
    }
    Ok(())
}

fn validate_target(action: &str, target: &Value) -> Result<(), GuiError> {
    match action {
        "establish_resident" => {
            let object = exact_object(
                target,
                &[
                    "backend_instance_uid",
                    "domain",
                    "host_uid",
                    "server_epoch",
                    "space_uid",
                ],
                &["backend_instance_uid", "domain", "host_uid", "server_epoch"],
                "target",
            )?;
            validate_domain_target(object)?;
            validate_uuid(
                value_str(object, "host_uid", "target.host_uid")?,
                "target.host_uid",
            )?;
            if let Some(space_uid) = object.get("space_uid") {
                validate_uuid(
                    space_uid.as_str().ok_or_else(|| {
                        GuiError::InvalidRequest("target.space_uid must be a UUID string".into())
                    })?,
                    "target.space_uid",
                )?;
            }
        }
        "ping" => {
            exact_object(target, &[], &[], "target")?;
        }
        "toast" => {
            let object = exact_object(target, &["message"], &["message"], "target")?;
            validate_string(
                value_str(object, "message", "target.message")?,
                "target.message",
                4096,
            )?;
        }
        "attach_domain" => {
            let object = exact_object(
                target,
                &[
                    "alternate_domains",
                    "backend_instance_uid",
                    "domain",
                    "server_epoch",
                ],
                &["backend_instance_uid", "domain", "server_epoch"],
                "target",
            )?;
            validate_domain_target(object)?;
            if let Some(alternates) = object.get("alternate_domains") {
                let alternates =
                    validate_domain_array(alternates, "target.alternate_domains", false)?;
                if alternates
                    .iter()
                    .any(|value| *value == object["domain"].as_str().unwrap())
                {
                    return invalid("target.alternate_domains contains the selected domain");
                }
            }
        }
        "detach_domain" => {
            let object = exact_object(
                target,
                &["backend_instance_uid", "domain", "server_epoch"],
                &["backend_instance_uid", "domain", "server_epoch"],
                "target",
            )?;
            validate_domain_target(object)?;
        }
        "focus_pane" => {
            let object = exact_object(
                target,
                &[
                    "backend",
                    "backend_instance_uid",
                    "domain",
                    "group_ref",
                    "host_uid",
                    "pane_id",
                    "server_epoch",
                    "space_no",
                    "space_uid",
                    "split_ref",
                    "tmux_client_uid",
                ],
                &[
                    "backend",
                    "backend_instance_uid",
                    "domain",
                    "group_ref",
                    "host_uid",
                    "pane_id",
                    "server_epoch",
                    "space_no",
                    "space_uid",
                    "split_ref",
                    "tmux_client_uid",
                ],
                "target",
            )?;
            if object.get("backend").and_then(Value::as_str) != Some("tmux") {
                return invalid("target.backend must be tmux for focus_pane");
            }
            validate_domain(
                value_str(object, "domain", "target.domain")?,
                "target.domain",
            )?;
            for field in [
                "backend_instance_uid",
                "host_uid",
                "server_epoch",
                "space_uid",
                "tmux_client_uid",
            ] {
                validate_uuid(
                    value_str(object, field, &format!("target.{field}"))?,
                    &format!("target.{field}"),
                )?;
            }
            json_uint(object.get("pane_id"), "target.pane_id")?;
            if json_uint(object.get("space_no"), "target.space_no")? == 0 {
                return invalid("target.space_no must be nonzero");
            }
            let epoch = value_str(object, "server_epoch", "target.server_epoch")?;
            for (field, kind) in [
                ("group_ref", ChildKind::Group),
                ("split_ref", ChildKind::Split),
            ] {
                let child = value_str(object, field, &format!("target.{field}"))?;
                validate_child_ref(child, kind, epoch, &format!("target.{field}"))?;
                let parsed = parse_ref(&format!("1/{child}"))
                    .expect("focus_pane child was just validated")
                    .child
                    .expect("focus_pane child is present");
                if !matches!(
                    parsed.handle,
                    ProviderHandle::Tx(_) | ProviderHandle::Opaque(_)
                ) {
                    return invalid(format!(
                        "target.{field} provider differs from target.backend"
                    ));
                }
            }
        }
        "activate" | "present" => {
            let allowed = if action == "present" {
                &[
                    "alternate_domains",
                    "backend_instance_uid",
                    "domain",
                    "group_ref",
                    "host_uid",
                    "server_epoch",
                    "space_uid",
                    "split_ref",
                    "workspace",
                ][..]
            } else {
                &[
                    "backend_instance_uid",
                    "domain",
                    "group_ref",
                    "host_uid",
                    "server_epoch",
                    "space_uid",
                    "split_ref",
                    "workspace",
                ][..]
            };
            let object = exact_object(
                target,
                allowed,
                &[
                    "backend_instance_uid",
                    "domain",
                    "host_uid",
                    "server_epoch",
                    "space_uid",
                    "workspace",
                ],
                "target",
            )?;
            validate_space_target(object)?;
            if let Some(alternates) = object.get("alternate_domains") {
                let alternates =
                    validate_domain_array(alternates, "target.alternate_domains", false)?;
                if alternates
                    .iter()
                    .any(|value| *value == object["domain"].as_str().unwrap())
                {
                    return invalid("target.alternate_domains contains the selected domain");
                }
            }
        }
        "safe_quit" => {
            let phase = target
                .get("phase")
                .and_then(Value::as_str)
                .ok_or_else(|| GuiError::InvalidRequest("target.phase is missing".into()))?;
            match phase {
                "detach" => {
                    let object = exact_object(
                        target,
                        &["domains", "phase"],
                        &["domains", "phase"],
                        "target",
                    )?;
                    // The exact empty array is the authorized no-op prepare
                    // for a tmux-only GUI: no imported Wez domain is
                    // detached, but the subsequent owner-survival proof is
                    // still required before hide/quit. No other domain-array
                    // field permits emptiness.
                    validate_domain_incarnation_array(&object["domains"], "target.domains", true)?;
                }
                "rollback" => {
                    let object = exact_object(
                        target,
                        &["phase", "proof_uid"],
                        &["phase", "proof_uid"],
                        "target",
                    )?;
                    validate_uuid(
                        value_str(object, "proof_uid", "target.proof_uid")?,
                        "target.proof_uid",
                    )?;
                }
                "finish" => {
                    let object = exact_object(
                        target,
                        &["phase", "platform_action", "proof_uid"],
                        &["phase", "platform_action", "proof_uid"],
                        "target",
                    )?;
                    validate_uuid(
                        value_str(object, "proof_uid", "target.proof_uid")?,
                        "target.proof_uid",
                    )?;
                    if !matches!(object["platform_action"].as_str(), Some("hide" | "quit")) {
                        return invalid("target.platform_action must be hide or quit");
                    }
                }
                _ => return invalid("target.phase must be detach, rollback, or finish"),
            }
        }
        _ => return invalid("unsupported bridge action"),
    }
    Ok(())
}

fn validate_domain_target(object: &Map<String, Value>) -> Result<(), GuiError> {
    validate_domain(
        value_str(object, "domain", "target.domain")?,
        "target.domain",
    )?;
    validate_uuid(
        value_str(
            object,
            "backend_instance_uid",
            "target.backend_instance_uid",
        )?,
        "target.backend_instance_uid",
    )?;
    validate_uuid(
        value_str(object, "server_epoch", "target.server_epoch")?,
        "target.server_epoch",
    )?;
    Ok(())
}

fn validate_space_target(object: &Map<String, Value>) -> Result<(), GuiError> {
    validate_domain_target(object)?;
    let host = value_str(object, "host_uid", "target.host_uid")?;
    let space = value_str(object, "space_uid", "target.space_uid")?;
    let epoch = value_str(object, "server_epoch", "target.server_epoch")?;
    validate_uuid(host, "target.host_uid")?;
    validate_uuid(space, "target.space_uid")?;
    let workspace = value_str(object, "workspace", "target.workspace")?;
    validate_string(workspace, "target.workspace", 256)?;
    if workspace != format!("dmux:{host}:{space}") {
        return invalid("target.workspace does not match target HostUid/SpaceUid");
    }
    if let Some(group) = object.get("group_ref") {
        validate_child_ref(
            group.as_str().ok_or_else(|| {
                GuiError::InvalidRequest("target.group_ref must be a string".into())
            })?,
            ChildKind::Group,
            epoch,
            "target.group_ref",
        )?;
    }
    if let Some(split) = object.get("split_ref") {
        if object.get("group_ref").is_none() {
            return invalid("target.group_ref is required with split_ref");
        }
        validate_child_ref(
            split.as_str().ok_or_else(|| {
                GuiError::InvalidRequest("target.split_ref must be a string".into())
            })?,
            ChildKind::Split,
            epoch,
            "target.split_ref",
        )?;
    }
    Ok(())
}

fn validate_same_request(
    prior_bytes: &[u8],
    expected: &Value,
    key: &[u8],
    instance: &str,
    expected_digest: &str,
) -> Result<(), GuiError> {
    let prior: Value = serde_json::from_slice(prior_bytes)
        .map_err(|e| GuiError::InvalidRequest(format!("existing request is malformed: {e}")))?;
    verify_request(&prior, key)?;
    validate_request_for_instance(&prior, Some(instance))?;
    let prior_digest = hex(&sha256(&canonical_request_bytes(&prior)?));
    if !constant_time_eq(prior_digest.as_bytes(), expected_digest.as_bytes())
        || prior.get("uid") != expected.get("uid")
    {
        return invalid("request UID is already bound to different bridge content");
    }
    Ok(())
}

/// The one acknowledgement decoder. [`call_instance`] is its only caller and
/// has already validated the request for `instance` and computed `digest`
/// from the same canonical bytes it signed. Decoding works on the exact
/// bytes read from the spool, never on a re-serialized value: a duplicated
/// key is a `deny_unknown_fields` decode error here, where a `Value` round
/// trip would have silently kept the last spelling.
fn decode_and_validate_ack(
    bytes: &[u8],
    request: &Value,
    instance: &str,
    digest: &str,
) -> Result<Value, GuiError> {
    let raw: Value = serde_json::from_slice(bytes)
        .map_err(|e| GuiError::InvalidAck(format!("ack is not JSON: {e}")))?;
    if contains_null(&raw) {
        return invalid_ack("ack optional fields must be omitted, never null");
    }
    let ack: BridgeAck = serde_json::from_slice(bytes)
        .map_err(|e| GuiError::InvalidAck(format!("ack is not exact bridge-v1 JSON: {e}")))?;
    validate_ack(&ack, request, instance, digest)?;
    serde_json::to_value(ack).map_err(|e| GuiError::InvalidAck(e.to_string()))
}

pub fn request_sha256(request: &Value) -> Result<String, GuiError> {
    validate_request_for_instance(request, None)?;
    Ok(hex(&sha256(&canonical_request_bytes(request)?)))
}

fn validate_ack(
    ack: &BridgeAck,
    request: &Value,
    instance: &str,
    digest: &str,
) -> Result<(), GuiError> {
    if ack.protocol_version != BRIDGE_PROTOCOL_VERSION {
        return invalid_ack("protocol_version must be 1");
    }
    for (field, actual) in [
        ("uid", ack.uid.as_str()),
        ("action", ack.action.as_str()),
        ("nonce", ack.nonce.as_str()),
    ] {
        let expected = string_field(request, field)?;
        if actual != expected {
            return invalid_ack(format!("{field} echo is {actual:?}, expected {expected:?}"));
        }
    }
    if ack.gui_instance != instance {
        return invalid_ack("gui_instance echo differs from the selected consumer");
    }
    validate_instance(&ack.gui_instance).map_err(|e| GuiError::InvalidAck(e.to_string()))?;
    if ack.completed_at > MAX_JSON_INTEGER {
        return invalid_ack("completed_at exceeds the exact JSON integer range");
    }
    if ack.completed_at > unix_seconds()?.saturating_add(2) {
        return invalid_ack("completed_at is too far in the future");
    }
    let issued_at = json_uint(request.get("issued_at"), "request.issued_at")?;
    if ack.completed_at < issued_at {
        return invalid_ack("completed_at predates issued_at");
    }
    if ack.request_sha256.len() != 64
        || !is_lower_hex(&ack.request_sha256)
        || !constant_time_eq(ack.request_sha256.as_bytes(), digest.as_bytes())
    {
        return invalid_ack("request_sha256 does not bind this request");
    }

    if !ack.ok {
        if ack_result_present(ack) {
            return invalid_ack("a rejected acknowledgement carries success result fields");
        }
        let code = ack
            .error
            .as_deref()
            .ok_or_else(|| GuiError::InvalidAck("rejected ack has no error token".into()))?;
        let message = ack
            .message
            .as_deref()
            .ok_or_else(|| GuiError::InvalidAck("rejected ack has no message".into()))?;
        validate_error_token(code).map_err(|e| GuiError::InvalidAck(e.to_string()))?;
        validate_string(message, "ack.message", 4096)
            .map_err(|e| GuiError::InvalidAck(e.to_string()))?;
        return Err(GuiError::Rejected {
            code: code.to_string(),
            detail: message.to_string(),
        });
    }
    if ack.error.is_some() || ack.message.is_some() {
        return invalid_ack("a successful acknowledgement carries error fields");
    }

    match ack.action.as_str() {
        "establish_resident"
            if ack.resident_established == Some(true)
                && only_ack_result(ack, &["resident_established"]) => {}
        "ping" if ack.pong == Some(true) && only_ack_result(ack, &["pong"]) => {}
        "toast" if ack.toasted == Some(true) && only_ack_result(ack, &["toasted"]) => {}
        "attach_domain"
            if ack.domain.as_deref() == request["target"]["domain"].as_str()
                && ack.domain_state.as_deref() == Some("Attached")
                && only_ack_result(ack, &["domain", "domain_state"]) => {}
        "detach_domain"
            if ack.detached_domains.as_ref()
                == Some(&vec![
                    request["target"]["domain"].as_str().unwrap().to_string(),
                ])
                && only_ack_result(ack, &["detached_domains"]) => {}
        "activate" | "present" => validate_space_ack(ack, request)?,
        "focus_pane"
            if ack.domain.as_deref() == request["target"]["domain"].as_str()
                && ack.pane_id == request["target"]["pane_id"].as_u64()
                && ack.group_ref.as_deref() == request["target"]["group_ref"].as_str()
                && ack.split_ref.as_deref() == request["target"]["split_ref"].as_str()
                && only_ack_result(ack, &["domain", "pane_id", "group_ref", "split_ref"]) => {}
        "safe_quit" => validate_safe_quit_ack(ack, request)?,
        _ => return invalid_ack("success result does not match the request action"),
    }
    Ok(())
}

fn validate_space_ack(ack: &BridgeAck, request: &Value) -> Result<(), GuiError> {
    let target = &request["target"];
    if ack.domain.as_deref() != target["domain"].as_str()
        || ack.workspace.as_deref() != target["workspace"].as_str()
    {
        return invalid_ack("presentation result domain/workspace differs from the target");
    }
    let windows = ack
        .window_ids
        .as_ref()
        .ok_or_else(|| GuiError::InvalidAck("presentation result has no window_ids".into()))?;
    validate_uint_array(windows, "ack.window_ids")
        .map_err(|error| GuiError::InvalidAck(error.to_string()))?;
    let target_group = target.get("group_ref").and_then(Value::as_str);
    let target_split = target.get("split_ref").and_then(Value::as_str);
    if ack.group_ref.as_deref() != target_group {
        return invalid_ack("presentation result Group ref differs from the target");
    }
    match (target_group, target_split, ack.split_ref.as_deref()) {
        (None, None, None) => {
            if ack.pane_id.is_some() {
                return invalid_ack("Space-only presentation returned a child pane");
            }
        }
        (Some(_), Some(expected), Some(actual)) if expected == actual => {}
        (Some(_), None, Some(actual)) => {
            validate_child_ref(
                actual,
                ChildKind::Split,
                target["server_epoch"].as_str().unwrap(),
                "ack.split_ref",
            )
            .map_err(|error| GuiError::InvalidAck(error.to_string()))?;
        }
        _ => return invalid_ack("presentation result Split ref differs from correlation rules"),
    }
    if target_group.is_some() && ack.pane_id.is_none() {
        return invalid_ack("focused Group result has no pane_id");
    }
    if ack.pane_id.is_some_and(|id| id > MAX_JSON_INTEGER) {
        return invalid_ack("pane_id exceeds the exact JSON integer range");
    }
    if !only_ack_result(
        ack,
        &[
            "domain",
            "workspace",
            "window_ids",
            "pane_id",
            "group_ref",
            "split_ref",
        ],
    ) {
        return invalid_ack("presentation acknowledgement has unrelated result fields");
    }
    Ok(())
}

fn validate_safe_quit_ack(ack: &BridgeAck, request: &Value) -> Result<(), GuiError> {
    match request["target"]["phase"].as_str() {
        Some("detach") => {
            let expected: Vec<String> = request["target"]["domains"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value["name"].as_str().unwrap().to_string())
                .collect();
            if ack.detached_domains.as_ref() != Some(&expected)
                || !only_ack_result(ack, &["detached_domains"])
            {
                return invalid_ack("safe_quit detach result differs from requested domains");
            }
        }
        Some("rollback") => {
            if ack.reattached_domains.is_none() || !only_ack_result(ack, &["reattached_domains"]) {
                return invalid_ack("safe_quit rollback omitted its exact reattached domains");
            }
        }
        Some("finish") => {
            if ack.platform_action.as_deref() != request["target"]["platform_action"].as_str()
                || ack.already_hidden == Some(false)
                || !only_ack_result(ack, &["platform_action", "already_hidden"])
            {
                return invalid_ack("safe_quit finish result differs from requested action");
            }
        }
        _ => return invalid_ack("safe_quit request phase is malformed"),
    }
    Ok(())
}

fn ack_result_present(ack: &BridgeAck) -> bool {
    !ack_result_names(ack).is_empty()
}

fn only_ack_result(ack: &BridgeAck, allowed: &[&str]) -> bool {
    ack_result_names(ack)
        .iter()
        .all(|field| allowed.contains(field))
}

fn ack_result_names(ack: &BridgeAck) -> Vec<&'static str> {
    let mut fields = Vec::new();
    macro_rules! present {
        ($field:ident) => {
            if ack.$field.is_some() {
                fields.push(stringify!($field));
            }
        };
    }
    present!(domain);
    present!(domain_state);
    present!(workspace);
    present!(window_ids);
    present!(pane_id);
    present!(group_ref);
    present!(split_ref);
    present!(detached_domains);
    present!(reattached_domains);
    present!(platform_action);
    present!(already_hidden);
    present!(pong);
    present!(toasted);
    present!(resident_established);
    fields
}

fn write_canonical(value: &Value, out: &mut String) -> Result<(), GuiError> {
    match value {
        Value::Null => {
            return Err(GuiError::InvalidRequest(
                "null is not permitted in signed bridge requests".into(),
            ));
        }
        Value::Bool(v) => out.push_str(if *v { "true" } else { "false" }),
        Value::Number(v)
            if v.as_i64()
                .is_some_and(|n| (n as i128).abs() <= 9_007_199_254_740_991)
                || v.as_u64().is_some_and(|n| n <= 9_007_199_254_740_991) =>
        {
            out.push_str(&v.to_string());
        }
        Value::Number(_) => {
            return Err(GuiError::InvalidRequest(
                "signed bridge request numbers must be exactly representable integers".into(),
            ));
        }
        Value::String(v) => out.push_str(
            &serde_json::to_string(v)
                .map_err(|e| GuiError::InvalidRequest(format!("string encoding: {e}")))?,
        ),
        Value::Array(values) => {
            out.push('[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    out.push(',');
                }
                write_canonical(value, out)?;
            }
            out.push(']');
        }
        Value::Object(values) => {
            out.push('{');
            let ordered: BTreeMap<&str, &Value> =
                values.iter().map(|(k, v)| (k.as_str(), v)).collect();
            for (index, (key, value)) in ordered.into_iter().enumerate() {
                if index != 0 {
                    out.push(',');
                }
                out.push_str(
                    &serde_json::to_string(key)
                        .map_err(|e| GuiError::InvalidRequest(format!("key encoding: {e}")))?,
                );
                out.push(':');
                write_canonical(value, out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

/// An already-open, descriptor-verified private directory. Every child open
/// is relative to this descriptor and uses O_NOFOLLOW, eliminating the
/// metadata-check/path-open race from the bridge's security boundary.
#[derive(Debug)]
struct PrivateDir {
    file: File,
    path: PathBuf,
}

impl PrivateDir {
    fn open(path: &Path, mode: u32) -> Result<Self, GuiError> {
        let c_path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| GuiError::BridgeUnavailable("NUL in bridge directory path".into()))?;
        let fd = unsafe {
            libc::open(
                c_path.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error().into());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        validate_open_dir(&file, path, mode)?;
        Ok(Self {
            file,
            path: path.to_path_buf(),
        })
    }

    fn child(&self, name: &str) -> Result<Self, GuiError> {
        validate_component(name)?;
        let c_name = CString::new(name).expect("validated path component");
        let fd = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                c_name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error().into());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let path = self.path.join(name);
        validate_open_dir(&file, &path, 0o700)?;
        Ok(Self { file, path })
    }

    fn ensure_child(&self, name: &str) -> Result<Self, GuiError> {
        validate_component(name)?;
        let c_name = CString::new(name).expect("validated path component");
        let rc = unsafe { libc::mkdirat(self.file.as_raw_fd(), c_name.as_ptr(), 0o700) };
        if rc != 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::AlreadyExists {
                return Err(error.into());
            }
        }
        self.child(name)
    }

    fn open_private_file_optional(&self, name: &str) -> Result<Option<File>, GuiError> {
        validate_component(name)?;
        let c_name = CString::new(name).expect("validated path component");
        let fd = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                c_name.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::NotFound {
                return Ok(None);
            }
            return Err(error.into());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        validate_open_file(&file, &self.path.join(name))?;
        Ok(Some(file))
    }

    fn read_private_file(&self, name: &str, maximum: usize) -> Result<Vec<u8>, GuiError> {
        self.read_private_file_optional(name, maximum)?
            .ok_or_else(|| {
                GuiError::Io(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("{}/{} is absent", self.path.display(), name),
                ))
            })
    }

    fn read_private_file_optional(
        &self,
        name: &str,
        maximum: usize,
    ) -> Result<Option<Vec<u8>>, GuiError> {
        let Some(mut file) = self.open_private_file_optional(name)? else {
            return Ok(None);
        };
        let metadata = file.metadata()?;
        if metadata.len() > maximum as u64 {
            return Err(GuiError::MessageTooLarge(metadata.len() as usize));
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        std::io::Read::by_ref(&mut file)
            .take(maximum.saturating_add(1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > maximum {
            return Err(GuiError::MessageTooLarge(bytes.len()));
        }
        Ok(Some(bytes))
    }

    fn create_temp(&self, name: &str, bytes: &[u8]) -> Result<File, GuiError> {
        validate_component(name)?;
        let c_name = CString::new(name).expect("validated path component");
        let fd = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                c_name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error().into());
        }
        let mut file = unsafe { File::from_raw_fd(fd) };
        let result = (|| -> io::Result<()> {
            let rc = unsafe { libc::fchmod(file.as_raw_fd(), 0o600) };
            if rc != 0 {
                return Err(io::Error::last_os_error());
            }
            file.write_all(bytes)?;
            file.sync_all()
        })();
        if let Err(error) = result {
            let _ = self.unlink(name);
            return Err(error.into());
        }
        if let Err(error) = validate_open_file(&file, &self.path.join(name)) {
            drop(file);
            let _ = self.unlink(name);
            return Err(error);
        }
        Ok(file)
    }

    fn write_new_atomic(&self, name: &str, bytes: &[u8]) -> Result<(), GuiError> {
        validate_component(name)?;
        let temporary = format!(".tmp-{}", Uuid::new_v4());
        let file = self.create_temp(&temporary, bytes)?;
        let temp_c = CString::new(temporary.as_str()).unwrap();
        let name_c = CString::new(name).unwrap();
        let rc = unsafe {
            libc::linkat(
                self.file.as_raw_fd(),
                temp_c.as_ptr(),
                self.file.as_raw_fd(),
                name_c.as_ptr(),
                0,
            )
        };
        let result = if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error().into())
        };
        drop(file);
        let _ = self.unlink(&temporary);
        if result.is_ok() {
            sync_directory(&self.file)?;
        }
        result
    }

    fn write_replace_atomic(&self, name: &str, bytes: &[u8]) -> Result<(), GuiError> {
        validate_component(name)?;
        // Refuse an existing symlink/nonprivate entry rather than silently
        // replacing a suspicious path. Absence is the normal first write.
        if let Some(existing) = self.open_private_file_optional(name)? {
            drop(existing);
        }
        let temporary = format!(".tmp-{}", Uuid::new_v4());
        let file = self.create_temp(&temporary, bytes)?;
        let temp_c = CString::new(temporary.as_str()).unwrap();
        let name_c = CString::new(name).unwrap();
        let rc = unsafe {
            libc::renameat(
                self.file.as_raw_fd(),
                temp_c.as_ptr(),
                self.file.as_raw_fd(),
                name_c.as_ptr(),
            )
        };
        drop(file);
        if rc != 0 {
            let error = io::Error::last_os_error();
            let _ = self.unlink(&temporary);
            return Err(error.into());
        }
        sync_directory(&self.file)?;
        Ok(())
    }

    fn unlink(&self, name: &str) -> io::Result<()> {
        let c_name = CString::new(name)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in component"))?;
        let rc = unsafe { libc::unlinkat(self.file.as_raw_fd(), c_name.as_ptr(), 0) };
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn entry_names(&self) -> Result<Vec<OsString>, GuiError> {
        let duplicate = unsafe { libc::dup(self.file.as_raw_fd()) };
        if duplicate < 0 {
            return Err(io::Error::last_os_error().into());
        }
        let directory = unsafe { libc::fdopendir(duplicate) };
        if directory.is_null() {
            let error = io::Error::last_os_error();
            unsafe { libc::close(duplicate) };
            return Err(error.into());
        }
        let mut entries = Vec::new();
        loop {
            set_errno(0);
            let entry = unsafe { libc::readdir(directory) };
            if entry.is_null() {
                let errno = get_errno();
                unsafe { libc::closedir(directory) };
                if errno != 0 {
                    return Err(io::Error::from_raw_os_error(errno).into());
                }
                break;
            }
            let bytes = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if bytes != b"." && bytes != b".." {
                entries.push(OsString::from_vec(bytes.to_vec()));
            }
        }
        Ok(entries)
    }
}

fn validate_open_dir(file: &File, path: &Path, mode: u32) -> Result<(), GuiError> {
    let metadata = file.metadata()?;
    if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
        return Err(GuiError::BridgeUnavailable(format!(
            "{} is not a current-user-owned directory",
            path.display()
        )));
    }
    if metadata.mode() & 0o777 != mode {
        return Err(GuiError::BridgeUnavailable(format!(
            "{} must be mode {mode:04o}",
            path.display()
        )));
    }
    Ok(())
}

fn validate_open_file(file: &File, path: &Path) -> Result<(), GuiError> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() } || metadata.nlink() != 1
    {
        return Err(GuiError::BridgeUnavailable(format!(
            "{} is not a singly-linked current-user-owned regular file",
            path.display()
        )));
    }
    if metadata.mode() & 0o777 != 0o600 {
        return Err(GuiError::BridgeUnavailable(format!(
            "{} must be mode 0600",
            path.display()
        )));
    }
    Ok(())
}

fn validate_component(component: &str) -> Result<(), GuiError> {
    if component.is_empty()
        || component == "."
        || component == ".."
        || component.len() > 255
        || component.bytes().any(|byte| byte == b'/' || byte == 0)
    {
        return invalid("unsafe bridge path component");
    }
    Ok(())
}

fn sync_directory(file: &File) -> Result<(), GuiError> {
    let rc = unsafe { libc::fsync(file.as_raw_fd()) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error().into())
    }
}

#[cfg(target_os = "linux")]
fn errno_ptr() -> *mut libc::c_int {
    unsafe { libc::__errno_location() }
}

#[cfg(target_os = "macos")]
fn errno_ptr() -> *mut libc::c_int {
    unsafe { libc::__error() }
}

fn set_errno(value: libc::c_int) {
    unsafe { *errno_ptr() = value };
}

fn get_errno() -> libc::c_int {
    unsafe { *errno_ptr() }
}

fn read_live_heartbeat(
    instance_dir: &PrivateDir,
    expected_instance: &str,
    now: u64,
) -> Result<BridgeHeartbeat, GuiError> {
    let mut bytes = None;
    // Lua rotates between two private inodes; heartbeat.json is absent only
    // during a two-rename interval. A few short samples avoid a false bridge
    // outage without weakening the two-second liveness bound.
    for attempt in 0..3 {
        bytes = instance_dir.read_private_file_optional("heartbeat.json", MAX_MESSAGE_BYTES)?;
        if bytes.is_some() {
            break;
        }
        if attempt != 2 {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let bytes = bytes.ok_or_else(|| {
        GuiError::BridgeUnavailable(format!("GUI {expected_instance} has no heartbeat"))
    })?;
    let raw: Value = serde_json::from_slice(&bytes).map_err(|e| {
        GuiError::BridgeUnavailable(format!(
            "GUI {expected_instance} heartbeat is not JSON: {e}"
        ))
    })?;
    if contains_null(&raw) {
        return Err(GuiError::BridgeUnavailable(format!(
            "GUI {expected_instance} heartbeat contains null instead of an omitted optional field"
        )));
    }
    let heartbeat: BridgeHeartbeat = serde_json::from_slice(&bytes).map_err(|e| {
        GuiError::BridgeUnavailable(format!(
            "GUI {expected_instance} heartbeat is not exact bridge-v1 JSON: {e}"
        ))
    })?;
    validate_heartbeat(&heartbeat, expected_instance, now)?;
    Ok(heartbeat)
}

fn validate_heartbeat(
    heartbeat: &BridgeHeartbeat,
    expected_instance: &str,
    now: u64,
) -> Result<(), GuiError> {
    if heartbeat.protocol_version != BRIDGE_PROTOCOL_VERSION {
        return Err(GuiError::BridgeUnavailable(
            "GUI heartbeat protocol_version is not 1".into(),
        ));
    }
    validate_instance(&heartbeat.gui_instance)?;
    if heartbeat.gui_instance != expected_instance {
        return Err(GuiError::InvalidInstance(
            "heartbeat gui_instance differs from its directory".into(),
        ));
    }
    if heartbeat.pid == 0 {
        return Err(GuiError::InvalidInstance("heartbeat pid is zero".into()));
    }
    validate_string(
        &heartbeat.process_start_token,
        "heartbeat.process_start_token",
        256,
    )?;
    if heartbeat.updated_at > MAX_JSON_INTEGER
        || heartbeat.updated_at > now.saturating_add(2)
        || now.saturating_sub(heartbeat.updated_at) > HEARTBEAT_MAX_AGE.as_secs()
    {
        return Err(GuiError::BridgeUnavailable(format!(
            "GUI {} heartbeat is stale or from the future",
            heartbeat.gui_instance
        )));
    }
    let mut pane_ids = BTreeSet::new();
    for pane in &heartbeat.panes {
        if pane.pane_id > MAX_JSON_INTEGER || !pane_ids.insert(pane.pane_id) {
            return Err(GuiError::InvalidInstance(
                "heartbeat pane IDs must be unique exact JSON integers".into(),
            ));
        }
        validate_domain(&pane.domain, "heartbeat pane domain")?;
        validate_marker(&pane.context)?;
        match (pane.context.backend, pane.tmux_client_uid) {
            (Backend::Tmux, Some(_)) | (Backend::Wez, None) => {}
            (Backend::Tmux, None) => {
                return Err(GuiError::InvalidInstance(
                    "heartbeat tmux pane has no exact client UID".into(),
                ));
            }
            (Backend::Wez, Some(_)) => {
                return Err(GuiError::InvalidInstance(
                    "heartbeat Wez pane carries a tmux client UID".into(),
                ));
            }
        }
        if pane
            .context
            .domain
            .as_deref()
            .is_some_and(|domain| domain != pane.domain)
        {
            return Err(GuiError::InvalidInstance(
                "heartbeat pane domain differs from its marker domain".into(),
            ));
        }
    }
    let mut valid_by_domain = BTreeMap::<&str, u32>::new();
    for pane in &heartbeat.panes {
        *valid_by_domain.entry(pane.domain.as_str()).or_default() += 1;
    }
    for (domain, state) in &heartbeat.domains {
        validate_domain(domain, "heartbeat domain")?;
        validate_string(&state.state, "heartbeat domain state", 64)?;
        if !state.state.bytes().all(|byte| byte.is_ascii_alphabetic()) {
            return Err(GuiError::InvalidInstance(
                "heartbeat domain state is malformed".into(),
            ));
        }
        let classified = state
            .valid_marker_pane_count
            .checked_add(state.system_pane_count)
            .ok_or_else(|| {
                GuiError::InvalidInstance("heartbeat domain pane counts overflow".into())
            })?;
        if classified != state.pane_count
            || state.has_any_panes != (state.pane_count != 0)
            || state.valid_marker_pane_count
                != valid_by_domain.get(domain.as_str()).copied().unwrap_or(0)
        {
            return Err(GuiError::InvalidInstance(
                "heartbeat domain pane counts do not exactly cover marker/system panes".into(),
            ));
        }
        if state.state == "Detached" && state.pane_count != 0 {
            return Err(GuiError::InvalidInstance(
                "detached heartbeat domain still reports panes".into(),
            ));
        }
        if domain != "local" && state.pane_count != 0 && state.backend_instance_uid.is_none() {
            return Err(GuiError::InvalidInstance(
                "active persistent heartbeat domain omitted its backend instance".into(),
            ));
        }
        match (
            state.system_pane_count,
            state.system_workspace.as_deref(),
            state.system_epoch,
        ) {
            (0, None, None) => {}
            (1, Some(workspace), Some(epoch))
                if workspace == format!("dmux:system:{}", epoch.0) => {}
            (1, Some(_), Some(_)) => {
                return Err(GuiError::InvalidInstance(
                    "heartbeat system workspace does not bind its exact epoch".into(),
                ));
            }
            _ => {
                return Err(GuiError::InvalidInstance(
                    "heartbeat domain must report zero or one exact system workspace/pane".into(),
                ));
            }
        }
    }
    if valid_by_domain
        .keys()
        .any(|domain| !heartbeat.domains.contains_key(*domain))
    {
        return Err(GuiError::InvalidInstance(
            "heartbeat marker pane belongs to an unreported domain".into(),
        ));
    }
    Ok(())
}

fn any_fresh_heartbeat(bridge: &PrivateDir) -> Result<bool, GuiError> {
    let instances = match bridge.child("instances") {
        Ok(instances) => instances,
        Err(GuiError::Io(error)) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let now = unix_seconds()?;
    for name in instances.entry_names()? {
        let Some(instance) = name.to_str() else {
            continue;
        };
        if validate_instance(instance).is_err() {
            continue;
        }
        let dir = match instances.child(instance) {
            Ok(dir) => dir,
            Err(_) => continue,
        };
        let mut raw = None;
        for attempt in 0..3 {
            raw = dir.read_private_file_optional("heartbeat.json", MAX_MESSAGE_BYTES)?;
            if raw.is_some() {
                break;
            }
            if attempt != 2 {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let Some(raw) = raw else {
            continue;
        };
        let value: Value = serde_json::from_slice(&raw).map_err(|error| {
            GuiError::BridgeUnavailable(format!(
                "refusing key rotation: GUI {instance} heartbeat is malformed: {error}"
            ))
        })?;
        if contains_null(&value) {
            return Err(GuiError::BridgeUnavailable(format!(
                "refusing key rotation: GUI {instance} heartbeat contains null"
            )));
        }
        let heartbeat: BridgeHeartbeat = serde_json::from_slice(&raw).map_err(|error| {
            GuiError::BridgeUnavailable(format!(
                "refusing key rotation: GUI {instance} heartbeat is malformed: {error}"
            ))
        })?;
        if heartbeat.updated_at > now.saturating_add(2) {
            return Err(GuiError::BridgeUnavailable(format!(
                "refusing key rotation: GUI {instance} heartbeat is from the future"
            )));
        }
        if now.saturating_sub(heartbeat.updated_at) <= HEARTBEAT_MAX_AGE.as_secs() {
            validate_heartbeat(&heartbeat, instance, now)?;
            return Ok(true);
        }
    }
    Ok(false)
}

fn random_key() -> Result<[u8; 32], GuiError> {
    let mut raw = [0u8; 32];
    loop {
        // SAFETY: `raw` is a live, writable 32-byte array for the duration
        // of the call. `getentropy` either fills the complete buffer or
        // returns an error and never retains the pointer.
        let rc = unsafe { libc::getentropy(raw.as_mut_ptr().cast(), raw.len()) };
        if rc == 0 {
            return Ok(raw);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error.into());
        }
    }
}

#[cfg(target_os = "linux")]
fn current_boot_token() -> Result<String, GuiError> {
    let raw = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    let value = raw.trim();
    validate_uuid(value, "Linux boot_id")?;
    Ok(format!("linux:{value}"))
}

#[cfg(target_os = "macos")]
fn current_boot_token() -> Result<String, GuiError> {
    let name = CString::new("kern.boottime").unwrap();
    let mut value: libc::timeval = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of::<libc::timeval>();
    let rc = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            (&mut value as *mut libc::timeval).cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error().into());
    }
    if length != std::mem::size_of::<libc::timeval>() || value.tv_sec <= 0 {
        return Err(GuiError::BridgeUnavailable(
            "kern.boottime returned an invalid value".into(),
        ));
    }
    Ok(format!("macos:{}:{}", value.tv_sec, value.tv_usec))
}

fn exact_marker(left: &MarkerContext, right: &MarkerContext) -> bool {
    left.host_uid == right.host_uid
        && left.space_uid == right.space_uid
        && left.space_no == right.space_no
        && left.backend == right.backend
        && left.domain == right.domain
        && left.server_epoch == right.server_epoch
        && left.group_ref == right.group_ref
        && left.split_ref == right.split_ref
}

fn allowed_action(action: &str) -> bool {
    matches!(
        action,
        "present"
            | "establish_resident"
            | "attach_domain"
            | "detach_domain"
            | "activate"
            | "focus_pane"
            | "toast"
            | "safe_quit"
            | "ping"
    )
}

fn unix_seconds() -> Result<u64, GuiError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|e| GuiError::InvalidRequest(format!("system clock before Unix epoch: {e}")))
}

fn simple_nonce() -> String {
    Uuid::new_v4().simple().to_string()
}

fn string_field<'a>(value: &'a Value, field: &str) -> Result<&'a str, GuiError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| GuiError::InvalidRequest(format!("{field} must be a string")))
}

fn validate_instance(instance: &str) -> Result<(), GuiError> {
    let bytes = instance.as_bytes();
    if bytes.len() < 2
        || bytes.len() > MAX_GUI_INSTANCE_BYTES
        || !bytes[0].is_ascii_alphanumeric()
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(GuiError::InvalidInstance(
            "GUI instance must be 2-160 safe ASCII characters".into(),
        ));
    }
    Ok(())
}

fn exact_object<'a>(
    value: &'a Value,
    allowed: &[&str],
    required: &[&str],
    label: &str,
) -> Result<&'a Map<String, Value>, GuiError> {
    let object = value
        .as_object()
        .ok_or_else(|| GuiError::InvalidRequest(format!("{label} must be an object")))?;
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return invalid(format!("{label} has unknown field {key}"));
        }
    }
    for field in required {
        if !object.contains_key(*field) {
            return invalid(format!("{label}.{field} is required"));
        }
    }
    Ok(object)
}

fn value_str<'a>(
    object: &'a Map<String, Value>,
    field: &str,
    label: &str,
) -> Result<&'a str, GuiError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| GuiError::InvalidRequest(format!("{label} must be a string")))
}

fn json_uint(value: Option<&Value>, label: &str) -> Result<u64, GuiError> {
    let integer = value
        .and_then(Value::as_u64)
        .ok_or_else(|| GuiError::InvalidRequest(format!("{label} must be non-negative integer")))?;
    if integer > MAX_JSON_INTEGER {
        return invalid(format!("{label} exceeds the exact JSON integer range"));
    }
    Ok(integer)
}

fn validate_uuid(value: &str, label: &str) -> Result<Uuid, GuiError> {
    let parsed = value
        .parse::<Uuid>()
        .map_err(|_| GuiError::InvalidRequest(format!("{label} is not a UUID")))?;
    if parsed.to_string() != value {
        return invalid(format!("{label} must be a canonical lowercase UUID"));
    }
    Ok(parsed)
}

fn validate_string(value: &str, label: &str, maximum: usize) -> Result<(), GuiError> {
    if value.is_empty() || value.len() > maximum {
        return invalid(format!(
            "{label} must be a non-empty string of at most {maximum} bytes"
        ));
    }
    Ok(())
}

fn validate_control_free_string(value: &str, label: &str, maximum: usize) -> Result<(), GuiError> {
    validate_string(value, label, maximum)?;
    if value.bytes().any(|byte| byte.is_ascii_control()) {
        return invalid(format!("{label} contains a control character"));
    }
    Ok(())
}

fn validate_domain(value: &str, label: &str) -> Result<(), GuiError> {
    validate_string(value, label, 128)?;
    let mut bytes = value.bytes();
    if !bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        || !bytes
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b':' | b'-'))
    {
        return invalid(format!("{label} contains forbidden characters"));
    }
    Ok(())
}

fn validate_domain_array<'a>(
    value: &'a Value,
    label: &str,
    allow_empty: bool,
) -> Result<Vec<&'a str>, GuiError> {
    let array = value
        .as_array()
        .filter(|array| allow_empty || !array.is_empty())
        .ok_or_else(|| {
            GuiError::InvalidRequest(if allow_empty {
                format!("{label} must be an array")
            } else {
                format!("{label} must be a non-empty array")
            })
        })?;
    let mut seen = BTreeSet::new();
    let mut result = Vec::with_capacity(array.len());
    for item in array {
        let domain = item
            .as_str()
            .ok_or_else(|| GuiError::InvalidRequest(format!("{label} must contain strings")))?;
        validate_domain(domain, label)?;
        if !seen.insert(domain) {
            return invalid(format!("{label} contains a duplicate"));
        }
        result.push(domain);
    }
    Ok(result)
}

fn validate_domain_incarnation_array<'a>(
    value: &'a Value,
    label: &str,
    allow_empty: bool,
) -> Result<Vec<&'a Map<String, Value>>, GuiError> {
    let array = value
        .as_array()
        .filter(|array| allow_empty || !array.is_empty())
        .ok_or_else(|| {
            GuiError::InvalidRequest(if allow_empty {
                format!("{label} must be an array")
            } else {
                format!("{label} must be a non-empty array")
            })
        })?;
    let mut seen = BTreeSet::new();
    let mut result = Vec::with_capacity(array.len());
    for (index, item) in array.iter().enumerate() {
        let item_label = format!("{label}[{index}]");
        let object = exact_object(
            item,
            &["backend_instance_uid", "name", "server_epoch"],
            &["backend_instance_uid", "name", "server_epoch"],
            &item_label,
        )?;
        let name = value_str(object, "name", &format!("{item_label}.name"))?;
        validate_domain(name, &format!("{item_label}.name"))?;
        for field in ["backend_instance_uid", "server_epoch"] {
            validate_uuid(
                value_str(object, field, &format!("{item_label}.{field}"))?,
                &format!("{item_label}.{field}"),
            )?;
        }
        if !seen.insert(name) {
            return invalid(format!("{label} contains a duplicate domain"));
        }
        result.push(object);
    }
    Ok(result)
}

fn validate_child_ref(
    value: &str,
    kind: ChildKind,
    expected_epoch: &str,
    label: &str,
) -> Result<(), GuiError> {
    if value.len() > 180 {
        return invalid(format!("{label} is too long"));
    }
    let parsed = parse_ref(&format!("1/{value}"))
        .map_err(|_| GuiError::InvalidRequest(format!("{label} is malformed")))?;
    let child = parsed
        .child
        .ok_or_else(|| GuiError::InvalidRequest(format!("{label} has no child suffix")))?;
    if child.kind != kind
        || child.epoch.0.to_string() != expected_epoch
        || crate::refs::child_suffix(&child) != value
    {
        return invalid(format!(
            "{label} kind/epoch/canonical spelling differs from its target"
        ));
    }
    Ok(())
}

fn validate_marker(marker: &MarkerContext) -> Result<(), GuiError> {
    if let Some(domain) = marker.domain.as_deref() {
        validate_domain(domain, "marker.domain")?;
    }
    let epoch = marker.server_epoch.0.to_string();
    validate_child_ref(
        &marker.group_ref,
        ChildKind::Group,
        &epoch,
        "marker.group_ref",
    )?;
    validate_child_ref(
        &marker.split_ref,
        ChildKind::Split,
        &epoch,
        "marker.split_ref",
    )?;
    for (field, value) in [
        ("marker.group_ref", marker.group_ref.as_str()),
        ("marker.split_ref", marker.split_ref.as_str()),
    ] {
        let parsed = parse_ref(&format!("1/{value}"))
            .expect("child ref was just validated")
            .child
            .expect("child ref present");
        let provider_matches = matches!(
            (&parsed.handle, marker.backend),
            (ProviderHandle::Wz(_), Backend::Wez)
                | (ProviderHandle::Tx(_), Backend::Tmux)
                | (ProviderHandle::Opaque(_), _)
        );
        if !provider_matches {
            return invalid(format!("{field} provider differs from marker.backend"));
        }
    }
    Ok(())
}

fn validate_cli_origin(origin: &GuiCliOrigin) -> Result<(), GuiError> {
    if origin.protocol_version != BRIDGE_PROTOCOL_VERSION {
        return invalid("GUI CLI origin protocol_version must be 1");
    }
    validate_instance(&origin.gui_instance)?;
    if origin.pane_id > MAX_JSON_INTEGER {
        return invalid("GUI CLI origin pane_id exceeds the exact JSON integer range");
    }
    validate_domain(&origin.domain, "GUI CLI origin domain")?;
    validate_marker(&origin.marker)?;
    if origin
        .marker
        .domain
        .as_deref()
        .is_some_and(|domain| domain != origin.domain)
    {
        return invalid("GUI CLI origin domain differs from marker.domain");
    }
    Ok(())
}

fn validate_status_display(
    display: &GuiStatusDisplay,
    marker: &MarkerContext,
) -> Result<(), GuiError> {
    if display.backend != marker.backend {
        return invalid("status display backend differs from the validated marker");
    }
    validate_string(&display.logical_ref, "status.logical_ref", 512)?;
    validate_string(&display.space_name, "status.space_name", 1024)?;
    validate_string(&display.owner_alias, "status.owner_alias", 64)?;
    validate_string(&display.owner_label, "status.owner_label", 128)?;
    validate_string(&display.route, "status.route", 256)?;
    if display.group_count == 0 || display.split_count == 0 {
        return invalid("status Group/Split counts must be nonzero");
    }
    if let Some(group_name) = display.group_name.as_deref() {
        validate_string(group_name, "status.group_name", 1024)?;
    }
    Ok(())
}

const MANAGED_PROXY_PREFIX: &str = "env -u WEZTERM_PANE -u TMUX -u TMUX_PANE WEZTERM_UNIX_SOCKET=";
const MANAGED_PROXY_SUFFIX: &str = " cli --prefer-mux --no-auto-start proxy";

/// WezTerm's own first connect runs a bare `wezterm cli --prefer-mux
/// proxy` on the owner: no endpoint, no `--no-auto-start`, so it discovers
/// a socket of its own and auto-starts a second, unmanaged server the
/// controller then attaches to.  Every managed domain therefore carries a
/// complete proxy command naming the owner's exact socket, built here in
/// one place from ADR 001's frozen strict-endpoint template rather than
/// string-formatted by the Lua config (plan §2 decision 16, §15.1).
fn managed_proxy_command(wezterm_path: &str, socket: &str) -> String {
    format!("{MANAGED_PROXY_PREFIX}{socket} {wezterm_path}{MANAGED_PROXY_SUFFIX}")
}

/// Accept only a command this host would itself have built for the row's
/// own reported executable and a well-formed managed socket.
fn validate_proxy_command(command: &str, wezterm_path: &str) -> Result<(), GuiError> {
    validate_control_free_string(command, "domain.override_proxy_command", 1024)?;
    let Some(socket) = command
        .strip_prefix(MANAGED_PROXY_PREFIX)
        .and_then(|rest| rest.strip_suffix(MANAGED_PROXY_SUFFIX))
        .and_then(|rest| rest.strip_suffix(wezterm_path))
        .and_then(|rest| rest.strip_suffix(' '))
    else {
        return invalid(
            "domain.override_proxy_command is not the strict-endpoint proxy invocation for this domain's executable",
        );
    };
    validate_managed_socket(wezterm_path, socket)
}

/// Both facts are spliced into a command the remote login shell re-parses,
/// so both must be published owner shapes that need no quoting.
fn validate_managed_socket(wezterm_path: &str, socket: &str) -> Result<(), GuiError> {
    if !valid_managed_socket(socket) || !unquoted_shell_word(wezterm_path) {
        return invalid(
            "domain.managed_socket must be the owner's fixed dmux/wez-dmux.sock, and neither it nor the reported executable may need shell quoting",
        );
    }
    Ok(())
}

fn validate_domain_source(source: &RemoteDomainSource) -> Result<(), GuiError> {
    validate_domain(&source.name, "domain.name")?;
    validate_control_free_string(&source.remote_address, "domain.remote_address", 1024)?;
    validate_control_free_string(&source.username, "domain.username", 256)?;
    if source.route_id <= 0 || source.route_id > MAX_JSON_SIGNED_INTEGER {
        return invalid("domain route_id must be positive");
    }
    if !(-MAX_JSON_SIGNED_INTEGER..=MAX_JSON_SIGNED_INTEGER).contains(&source.priority) {
        return invalid("domain priority exceeds the exact JSON integer range");
    }
    if source.transport == Transport::Local {
        return invalid("local route cannot become a remote Wez domain");
    }
    match (
        source.compatible,
        source.remote_wezterm_path.as_deref(),
        source.managed_socket.as_deref(),
        source.unavailable_reason.as_deref(),
    ) {
        (true, Some(path), Some(socket), None) => {
            validate_absolute_path(path, "domain.remote_wezterm_path")?;
            validate_managed_socket(path, socket)
        }
        (false, path, None, Some(reason)) => {
            if let Some(path) = path {
                validate_absolute_path(path, "domain.remote_wezterm_path")?;
            }
            validate_control_free_string(reason, "unavailable_reason", 4096)
        }
        _ => invalid(
            "compatible domains require an absolute remote_wezterm_path plus the owner's managed socket and omit unavailable_reason; incompatible domains require unavailable_reason, carry no socket, and may omit the path",
        ),
    }
}

fn validate_absolute_path(value: &str, label: &str) -> Result<(), GuiError> {
    validate_control_free_string(value, label, 1024)?;
    if !Path::new(value).is_absolute() {
        return invalid(format!("{label} must be an absolute control-free path"));
    }
    Ok(())
}

fn validate_error_token(value: &str) -> Result<(), GuiError> {
    let mut bytes = value.bytes();
    if value.len() > 64
        || !bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        || !bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return invalid("error/health token is not stable snake_case");
    }
    Ok(())
}

fn validate_uint_array(values: &[u64], label: &str) -> Result<(), GuiError> {
    if values.is_empty()
        || values.iter().any(|value| *value > MAX_JSON_INTEGER)
        || values.iter().collect::<BTreeSet<_>>().len() != values.len()
    {
        return invalid(format!(
            "{label} must be a non-empty array of unique exact JSON integers"
        ));
    }
    Ok(())
}

fn is_lower_hex(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn contains_null(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Array(values) => values.iter().any(contains_null),
        Value::Object(values) => values.values().any(contains_null),
        _ => false,
    }
}

fn invalid<T>(detail: impl Into<String>) -> Result<T, GuiError> {
    Err(GuiError::InvalidRequest(detail.into()))
}

fn invalid_ack<T>(detail: impl Into<String>) -> Result<T, GuiError> {
    Err(GuiError::InvalidAck(detail.into()))
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (&a, &b) in left.iter().zip(right) {
        difference |= a ^ b;
    }
    difference == 0
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

mod marker_wire {
    use std::num::NonZeroU64;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::{Backend, GuiError, MarkerContext, validate_marker, validate_uuid};
    use crate::model::{HostUid, ServerEpoch, SpaceNo, SpaceUid};

    #[derive(Serialize)]
    struct MarkerOut<'a> {
        host_uid: HostUid,
        space_uid: SpaceUid,
        space_no: SpaceNo,
        backend: Backend,
        #[serde(skip_serializing_if = "Option::is_none")]
        domain: &'a Option<String>,
        server_epoch: ServerEpoch,
        group_ref: &'a str,
        split_ref: &'a str,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct MarkerIn {
        host_uid: String,
        space_uid: String,
        space_no: u64,
        backend: Backend,
        #[serde(default)]
        domain: Option<String>,
        server_epoch: String,
        group_ref: String,
        split_ref: String,
    }

    pub fn serialize<S>(marker: &MarkerContext, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        MarkerOut {
            host_uid: marker.host_uid,
            space_uid: marker.space_uid,
            space_no: marker.space_no,
            backend: marker.backend,
            domain: &marker.domain,
            server_epoch: marker.server_epoch,
            group_ref: &marker.group_ref,
            split_ref: &marker.split_ref,
        }
        .serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<MarkerContext, D::Error>
    where
        D: Deserializer<'de>,
    {
        let input = MarkerIn::deserialize(deserializer)?;
        let host =
            validate_uuid(&input.host_uid, "marker.host_uid").map_err(serde::de::Error::custom)?;
        let space = validate_uuid(&input.space_uid, "marker.space_uid")
            .map_err(serde::de::Error::custom)?;
        let epoch = validate_uuid(&input.server_epoch, "marker.server_epoch")
            .map_err(serde::de::Error::custom)?;
        let space_no = NonZeroU64::new(input.space_no)
            .map(SpaceNo)
            .ok_or_else(|| serde::de::Error::custom("marker.space_no must be nonzero"))?;
        let marker = MarkerContext {
            host_uid: HostUid(host),
            space_uid: SpaceUid(space),
            space_no,
            backend: input.backend,
            domain: input.domain,
            server_epoch: ServerEpoch(epoch),
            group_ref: input.group_ref,
            split_ref: input.split_ref,
        };
        validate_marker(&marker).map_err(|error: GuiError| serde::de::Error::custom(error))?;
        Ok(marker)
    }
}

mod optional_uuid_wire {
    use serde::{Deserialize, Deserializer, Serializer};
    use uuid::Uuid;

    use super::validate_uuid;

    pub fn serialize<S>(value: &Option<Uuid>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            Some(value) => serializer.serialize_some(&value.to_string()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Uuid>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Option::<String>::deserialize(deserializer)?;
        value
            .map(|value| {
                validate_uuid(&value, "GUI CLI origin tmux_client_uid")
                    .map_err(serde::de::Error::custom)
            })
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Backend, HostUid, ServerEpoch, SpaceNo, SpaceUid};
    use std::fs;
    use std::num::NonZeroU64;
    use std::os::unix::fs::PermissionsExt;

    fn private_root() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        root
    }

    fn request() -> Value {
        serde_json::json!({
            "protocol_version": 1,
            "uid": "11111111-1111-4111-8111-111111111111",
            "action": "present",
            "target": {
                "domain": "dmux-b-usb",
                "workspace": "dmux:22222222-2222-4222-8222-222222222222:33333333-3333-4333-8333-333333333333",
                "host_uid": "22222222-2222-4222-8222-222222222222",
                "space_uid": "33333333-3333-4333-8333-333333333333",
                "backend_instance_uid": "44444444-4444-4444-8444-444444444444",
                "server_epoch": "55555555-5555-4555-8555-555555555555",
                "group_ref": "g55555555-5555-4555-8555-555555555555.wz-7",
                "split_ref": "p55555555-5555-4555-8555-555555555555.wz-9",
                "alternate_domains": ["dmux-b-ts"]
            },
            "issued_at": 1800000000,
            "expiry": 1800000010,
            "nonce": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "replay_key": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "origin": {
                "kind": "in_gui",
                "gui_instance": "gui-42-cafe",
                "pid": 42,
                "process_start_token": "start-token",
                "pane_id": 91,
                "domain": "dmux-b-usb",
                "host_uid": "22222222-2222-4222-8222-222222222222",
                "space_uid": "33333333-3333-4333-8333-333333333333",
                "space_no": 7,
                "backend": "wez",
                "server_epoch": "55555555-5555-4555-8555-555555555555",
                "group_ref": "g55555555-5555-4555-8555-555555555555.wz-7",
                "split_ref": "p55555555-5555-4555-8555-555555555555.wz-9"
            }
        })
    }

    #[test]
    fn canonical_json_is_recursive_and_omits_the_signature() {
        let mut req = request();
        req["hmac_sha256"] = Value::String("ignored".into());
        let canonical = String::from_utf8(canonical_request_bytes(&req).unwrap()).unwrap();
        assert_eq!(
            canonical,
            concat!(
                "{\"action\":\"present\",\"expiry\":1800000010,\"issued_at\":1800000000,",
                "\"nonce\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"origin\":{\"backend\":\"wez\",\"domain\":\"dmux-b-usb\",",
                "\"group_ref\":\"g55555555-5555-4555-8555-555555555555.wz-7\",\"gui_instance\":\"gui-42-cafe\",",
                "\"host_uid\":\"22222222-2222-4222-8222-222222222222\",\"kind\":\"in_gui\",\"pane_id\":91,",
                "\"pid\":42,\"process_start_token\":\"start-token\",\"server_epoch\":\"55555555-5555-4555-8555-555555555555\",",
                "\"space_no\":7,\"space_uid\":\"33333333-3333-4333-8333-333333333333\",",
                "\"split_ref\":\"p55555555-5555-4555-8555-555555555555.wz-9\"},\"protocol_version\":1,",
                "\"replay_key\":\"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\",\"target\":{\"alternate_domains\":[\"dmux-b-ts\"],",
                "\"backend_instance_uid\":\"44444444-4444-4444-8444-444444444444\",\"domain\":\"dmux-b-usb\",",
                "\"group_ref\":\"g55555555-5555-4555-8555-555555555555.wz-7\",",
                "\"host_uid\":\"22222222-2222-4222-8222-222222222222\",",
                "\"server_epoch\":\"55555555-5555-4555-8555-555555555555\",",
                "\"space_uid\":\"33333333-3333-4333-8333-333333333333\",",
                "\"split_ref\":\"p55555555-5555-4555-8555-555555555555.wz-9\",",
                "\"workspace\":\"dmux:22222222-2222-4222-8222-222222222222:33333333-3333-4333-8333-333333333333\"},",
                "\"uid\":\"11111111-1111-4111-8111-111111111111\"}"
            )
        );
        assert_eq!(
            hex(&sha256(canonical.as_bytes())),
            "61f7c94cb55544583d28a47ee48684a14921a0760d5217f26cbcfad984a069df"
        );
        assert_eq!(
            hmac_sha256_hex(b"0123456789abcdef0123456789abcdef", canonical.as_bytes()),
            "293f019ae3ee7a55784dd6d03593e431f0d64117e496703ac7e39ed7ebcfce80"
        );
        assert_eq!(
            String::from_utf8(
                canonical_request_bytes(&serde_json::json!({"text":"quote\"\n\0"})).unwrap()
            )
            .unwrap(),
            r#"{"text":"quote\"\n\u0000"}"#
        );
    }

    #[test]
    fn safe_quit_detach_alone_accepts_the_exact_empty_domain_array_vector() {
        let mut empty_request: Value = serde_json::from_str(concat!(
            "{\"action\":\"safe_quit\",\"expiry\":1800000010,\"issued_at\":1800000000,",
            "\"nonce\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"origin\":{",
            "\"backend\":\"wez\",\"domain\":\"dmux-b-usb\",",
            "\"group_ref\":\"g55555555-5555-4555-8555-555555555555.wz-7\",",
            "\"gui_instance\":\"gui-42-cafe\",\"host_uid\":\"22222222-2222-4222-8222-222222222222\",",
            "\"kind\":\"in_gui\",\"pane_id\":91,\"pid\":42,\"process_start_token\":\"start-token\",",
            "\"server_epoch\":\"55555555-5555-4555-8555-555555555555\",\"space_no\":7,",
            "\"space_uid\":\"33333333-3333-4333-8333-333333333333\",",
            "\"split_ref\":\"p55555555-5555-4555-8555-555555555555.wz-9\"},",
            "\"protocol_version\":1,\"replay_key\":\"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\",",
            "\"target\":{\"domains\":[],\"phase\":\"detach\"},",
            "\"uid\":\"11111111-1111-4111-8111-111111111111\"}"
        ))
        .unwrap();
        let canonical = canonical_request_bytes(&empty_request).unwrap();
        assert_eq!(
            hex(&sha256(&canonical)),
            "9020e21a60eb17b851792de5bf1f9bd90174bbe4dd7120be0895d57d9fda5e15"
        );
        assert_eq!(
            sign_request(&mut empty_request, b"0123456789abcdef0123456789abcdef").unwrap(),
            "a188cd6cc180e07fcfc2ba88e5609885c732ac0e7748e9b451cfcc73465958b9"
        );
        verify_request(&empty_request, b"0123456789abcdef0123456789abcdef").unwrap();

        let mut present = request();
        present["target"]["alternate_domains"] = serde_json::json!([]);
        assert!(sign_request(&mut present, &[0x22; 32]).is_err());
    }

    #[test]
    fn gui_space_picker_rows_accept_both_typed_backends() {
        let mut row = GuiSpaceRow {
            stable_ref: "dmux://22222222-2222-4222-8222-222222222222/spaces/33333333-3333-4333-8333-333333333333".into(),
            name: "editor".into(),
            backend: Backend::Wez,
            owner_alias: "b".into(),
            owner_label: "archie".into(),
            route: "dmux-b-ts".into(),
            attached: true,
            health: "healthy".into(),
        };
        validate_space_rows(std::slice::from_ref(&row)).unwrap();
        row.backend = Backend::Tmux;
        validate_space_rows(&[row]).unwrap();
    }

    #[test]
    fn hmac_matches_rfc_4231_case_one() {
        assert_eq!(
            hmac_sha256_hex(&[0x0b; 20], b"Hi There"),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn signing_round_trips_and_tampering_fails() {
        let mut req = request();
        let key = [0x42; 32];
        sign_request(&mut req, &key).unwrap();
        verify_request(&req, &key).unwrap();
        req["target"]["domain"] = Value::String("other".into());
        assert!(verify_request(&req, &key).is_err());
    }

    #[test]
    fn key_is_created_once_with_private_modes() {
        let root = private_root();
        let first = ensure_bridge_key(root.path()).unwrap();
        let second = ensure_bridge_key(root.path()).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 32);
        assert_eq!(
            fs::metadata(root.path().join("bridge"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(root.path().join("bridge/key"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(root.path().join("bridge/key.boot"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn nulls_and_fractional_numbers_are_not_signable() {
        let mut null = request();
        null["target"]["x"] = Value::Null;
        assert!(canonical_request_bytes(&null).is_err());
        let mut fractional = request();
        fractional["target"]["x"] = serde_json::json!(1.5);
        assert!(canonical_request_bytes(&fractional).is_err());
    }

    fn marker() -> MarkerContext {
        MarkerContext {
            host_uid: HostUid(Uuid::from_u128(1)),
            space_uid: SpaceUid(Uuid::from_u128(2)),
            space_no: SpaceNo(NonZeroU64::new(3).unwrap()),
            backend: Backend::Wez,
            domain: None,
            server_epoch: ServerEpoch(Uuid::from_u128(4)),
            group_ref: format!("g{}.wz-5", Uuid::from_u128(4)),
            split_ref: format!("p{}.wz-6", Uuid::from_u128(4)),
        }
    }

    fn heartbeat(root: &Path, instance: &str, updated_at: u64, marker: &MarkerContext) {
        let runtime = PrivateDir::open(root, 0o700).unwrap();
        let bridge = runtime.child("bridge").unwrap();
        let instances = bridge.ensure_child("instances").unwrap();
        let instance_dir = instances.ensure_child(instance).unwrap();
        for child in ["requests", "acks", "consumed"] {
            instance_dir.ensure_child(child).unwrap();
        }
        let doc = BridgeHeartbeat {
            protocol_version: 1,
            gui_instance: instance.into(),
            pid: 123,
            process_start_token: "start-token".into(),
            updated_at,
            panes: vec![BridgePane {
                pane_id: 91,
                domain: "dmux-b-usb".into(),
                tmux_client_uid: None,
                context: marker.clone(),
            }],
            domains: BTreeMap::from([(
                "dmux-b-usb".into(),
                BridgeDomainState {
                    state: "Attached".into(),
                    has_any_panes: true,
                    backend_instance_uid: Some(BackendInstanceUid(Uuid::from_u128(12))),
                    pane_count: 1,
                    valid_marker_pane_count: 1,
                    system_pane_count: 0,
                    system_workspace: None,
                    system_epoch: None,
                },
            )]),
        };
        instance_dir
            .write_replace_atomic("heartbeat.json", &serde_json::to_vec(&doc).unwrap())
            .unwrap();
    }

    #[test]
    fn instance_discovery_requires_one_fresh_exact_marker() {
        let root = private_root();
        ensure_bridge_key(root.path()).unwrap();
        let now = unix_seconds().unwrap();
        let expected = marker();
        heartbeat(root.path(), "gui-123-a", now, &expected);
        let selected = discover_in_gui_instance(root.path(), &expected).unwrap();
        assert_eq!(selected.gui_instance, "gui-123-a");
        assert_eq!(selected.pane_id, 91);

        heartbeat(root.path(), "gui-124-b", now, &expected);
        assert!(matches!(
            discover_in_gui_instance(root.path(), &expected),
            Err(GuiError::InvalidInstance(_))
        ));
    }

    #[test]
    fn stale_or_mismatched_heartbeat_is_not_an_origin() {
        let root = private_root();
        ensure_bridge_key(root.path()).unwrap();
        let expected = marker();
        heartbeat(root.path(), "gui-stale", 1, &expected);
        assert!(matches!(
            discover_in_gui_instance(root.path(), &expected),
            Err(GuiError::BridgeUnavailable(_))
        ));
    }

    #[test]
    fn cli_origin_is_distinct_exact_and_marker_bound() {
        let marker = marker();
        let input = serde_json::to_value(GuiCliOrigin {
            protocol_version: 1,
            gui_instance: "gui-42-cafe".into(),
            pane_id: 91,
            domain: "dmux-b-usb".into(),
            tmux_client_uid: None,
            marker: marker.clone(),
        })
        .unwrap();
        let parsed = parse_origin_json(&serde_json::to_string(&input).unwrap()).unwrap();
        assert_eq!(parsed.gui_instance, "gui-42-cafe");
        assert_eq!(parsed.marker, marker);

        let mut with_client = input.clone();
        with_client["tmux_client_uid"] =
            Value::String("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".into());
        let parsed_with_client =
            parse_origin_json(&serde_json::to_string(&with_client).unwrap()).unwrap();
        assert_eq!(
            parsed_with_client.tmux_client_uid,
            Some(Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").unwrap())
        );
        with_client["tmux_client_uid"] =
            Value::String("AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA".into());
        assert!(parse_origin_json(&serde_json::to_string(&with_client).unwrap()).is_err());

        let mut signed_shape = input.clone();
        signed_shape["kind"] = Value::String("in_gui".into());
        assert!(parse_origin_json(&serde_json::to_string(&signed_shape).unwrap()).is_err());

        let mut mismatch = input;
        mismatch["marker"]["domain"] = Value::String("another-domain".into());
        assert!(parse_origin_json(&serde_json::to_string(&mismatch).unwrap()).is_err());

        let root = private_root();
        ensure_bridge_key(root.path()).unwrap();
        heartbeat(root.path(), "gui-42-cafe", unix_seconds().unwrap(), &marker);
        let exact = read_instance_heartbeat(root.path(), "gui-42-cafe").unwrap();
        assert_eq!(exact.gui_instance, "gui-42-cafe");
        assert_eq!(exact.panes[0].pane_id, 91);
        assert!(matches!(
            read_instance_heartbeat(root.path(), "gui-missing"),
            Err(GuiError::BridgeUnavailable(_))
        ));
        let (selection, bound_heartbeat) =
            bind_cli_origin_with_heartbeat(root.path(), &parsed, &marker).unwrap();
        assert_eq!(selection.pane_id, 91);
        assert_eq!(bound_heartbeat.gui_instance, "gui-42-cafe");
        assert_eq!(bound_heartbeat.panes.len(), 1);
        assert_eq!(
            bind_cli_origin(root.path(), &parsed, &marker).unwrap(),
            selection
        );
    }

    #[test]
    fn strict_request_unions_reject_unknown_stale_and_unbound_fields() {
        let key = [0x33; 32];

        let signed_origin = request()["origin"].clone();
        assert!(parse_signed_origin_json(&serde_json::to_string(&signed_origin).unwrap()).is_ok());
        let mut noncanonical_origin = signed_origin;
        noncanonical_origin["host_uid"] =
            Value::String("22222222-2222-4222-8222-22222222222A".into());
        assert!(
            parse_signed_origin_json(&serde_json::to_string(&noncanonical_origin).unwrap())
                .is_err()
        );

        let mut unknown = request();
        unknown["target"]["unexpected"] = Value::Bool(true);
        assert!(sign_request(&mut unknown, &key).is_err());

        let mut stale = request();
        stale["target"]["group_ref"] =
            Value::String("g66666666-6666-4666-8666-666666666666.wz-7".into());
        assert!(sign_request(&mut stale, &key).is_err());

        let mut leading_zero = request();
        leading_zero["target"]["group_ref"] =
            Value::String("g55555555-5555-4555-8555-555555555555.wz-07".into());
        assert!(sign_request(&mut leading_zero, &key).is_err());

        let mut cold = request();
        cold["origin"] = serde_json::json!({
            "kind": "cold_launcher",
            "gui_instance": "gui-42-cafe",
            "uid": 501,
            "pid": 42,
            "start_token": "process start token",
            "launcher_request_uid": "77777777-7777-4777-8777-777777777777",
            "domain": "some-other-domain",
            "backend_instance_uid": "44444444-4444-4444-8444-444444444444"
        });
        assert!(sign_request(&mut cold, &key).is_err());

        let mut safe = request_document(
            "safe_quit",
            serde_json::json!({
                "phase":"detach",
                "domains":[{
                    "name":"dmux-b-usb",
                    "backend_instance_uid":"44444444-4444-4444-8444-444444444444",
                    "server_epoch":"55555555-5555-4555-8555-555555555555"
                }]
            }),
            request()["origin"].clone(),
        )
        .unwrap();
        safe["target"]["proof_uid"] = Value::String("77777777-7777-4777-8777-777777777777".into());
        assert!(sign_request(&mut safe, &key).is_err());
    }

    #[test]
    fn status_cache_is_exact_atomic_and_refuses_a_symlink_target() {
        let root = private_root();
        ensure_bridge_key(root.path()).unwrap();
        let runtime = PrivateDir::open(root.path(), 0o700).unwrap();
        let bridge = runtime.child("bridge").unwrap();
        let instances = bridge.ensure_child("instances").unwrap();
        let instance = instances.ensure_child("gui-42-cafe").unwrap();
        instance.ensure_child("requests").unwrap();
        instance.ensure_child("acks").unwrap();
        instance.ensure_child("consumed").unwrap();

        let record = GuiStatusCache::success(
            "gui-42-cafe".into(),
            91,
            marker(),
            GuiStatusDisplay {
                logical_ref: "3".into(),
                space_name: "dotfiles".into(),
                backend: Backend::Wez,
                owner_alias: "a".into(),
                owner_label: "macie".into(),
                route: "local".into(),
                group_count: 2,
                split_count: 3,
                group_name: Some("editor".into()),
            },
        )
        .unwrap();
        let path = write_status_cache(root.path(), &record).unwrap();
        assert_eq!(
            path,
            root.path()
                .join("bridge/instances/gui-42-cafe/context/91.json")
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let encoded: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(encoded.get("error").is_none());
        assert!(encoded["marker"].get("domain").is_none());
        assert_eq!(encoded["display"]["group_count"], 2);

        fs::remove_file(&path).unwrap();
        let outside = root.path().join("outside");
        fs::write(&outside, b"unchanged").unwrap();
        std::os::unix::fs::symlink(&outside, &path).unwrap();
        assert!(write_status_cache(root.path(), &record).is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"unchanged");
    }

    /// Archie's real published endpoint; Macie's long
    /// `_CS_DARWIN_USER_TEMP_DIR` form is exercised below.
    const ARCHIE_SOCKET: &str = "/run/user/1000/dmux/wez-dmux.sock";

    fn domain_source(
        name: &str,
        route_id: i64,
        transport: Transport,
        compatible: bool,
    ) -> RemoteDomainSource {
        RemoteDomainSource {
            name: name.into(),
            remote_address: format!("10.77.77.{route_id}"),
            username: "fredrir".into(),
            remote_wezterm_path: compatible.then(|| "/usr/bin/wezterm".into()),
            managed_socket: compatible.then(|| ARCHIE_SOCKET.into()),
            host_uid: HostUid(Uuid::from_u128(11)),
            backend_instance_uid: BackendInstanceUid(Uuid::from_u128(12)),
            route_id,
            priority: route_id * 10,
            transport,
            network_class: if route_id == 1 {
                NetworkClass::Usb
            } else {
                NetworkClass::Tailscale
            },
            compatible,
            unavailable_reason: (!compatible).then(|| "wez_build_mismatch".into()),
        }
    }

    /// Selection among these rows is the production chooser's job
    /// (`gui_cli::choose_compatible_presentation_row`), whose tests prove
    /// the identity and compatibility refusals; this case vouches for the
    /// manifest the chooser consumes.
    #[test]
    fn domain_manifest_derives_alternates_and_refuses_unsafe_sources() {
        let rows = build_domain_manifest(vec![
            domain_source("dmux-b-ts", 2, Transport::WezSsh, false),
            domain_source("dmux-b-usb", 1, Transport::Openssh, true),
        ])
        .unwrap();
        assert_eq!(rows[0].name, "dmux-b-usb");
        assert!(rows[0].alternate_domains.is_empty());
        assert_eq!(rows[1].alternate_domains, ["dmux-b-usb"]);
        assert!(rows[0].unavailable_reason.is_none());
        let serialized = serde_json::to_value(&rows).unwrap();
        assert!(serialized[0].get("unavailable_reason").is_none());
        assert_eq!(serialized[1]["unavailable_reason"], "wez_build_mismatch");
        assert!(serialized[1].get("remote_wezterm_path").is_none());

        let mut control = domain_source("dmux-control", 3, Transport::Openssh, true);
        control.remote_address = "host\nProxyCommand=bad".into();
        assert!(build_domain_manifest(vec![control]).is_err());

        let mut imprecise = domain_source("dmux-large", 4, Transport::Openssh, true);
        imprecise.priority = MAX_JSON_SIGNED_INTEGER + 1;
        assert!(build_domain_manifest(vec![imprecise]).is_err());
    }

    #[test]
    fn managed_domain_rows_pin_the_owner_socket_into_their_proxy_command() {
        let rows = build_domain_manifest(vec![
            domain_source("dmux-b-usb", 1, Transport::Openssh, true),
            domain_source("dmux-b-ts", 2, Transport::WezSsh, false),
        ])
        .unwrap();
        assert_eq!(
            rows[0].override_proxy_command.as_deref(),
            Some(
                "env -u WEZTERM_PANE -u TMUX -u TMUX_PANE \
                 WEZTERM_UNIX_SOCKET=/run/user/1000/dmux/wez-dmux.sock \
                 /usr/bin/wezterm cli --prefer-mux --no-auto-start proxy"
            )
        );
        let serialized = serde_json::to_value(&rows).unwrap();
        assert!(serialized[1].get("override_proxy_command").is_none());

        // Macie publishes beneath `_CS_DARWIN_USER_TEMP_DIR`, so the
        // accepted shape must survive a long /var/folders prefix.
        let mut macie = domain_source("dmux-b-usb", 1, Transport::Openssh, true);
        macie.managed_socket =
            Some("/var/folders/xg/l7zk7hdd3f50h289ypshmp9m0000gn/T/dmux/wez-dmux.sock".into());
        assert!(build_domain_manifest(vec![macie]).is_ok());

        for rejected in [
            "/dmux/wez-dmux.sock".to_string(),
            "/run/user/1000/wez-dmux.sock".to_string(),
            "/run/user/1000/dmux/wez-dmux.sock.old".to_string(),
            "/run/user/my runtime/dmux/wez-dmux.sock".to_string(),
            "run/user/1000/dmux/wez-dmux.sock".to_string(),
            format!("/run/user/{}/dmux/wez-dmux.sock", "x".repeat(90)),
        ] {
            let mut source = domain_source("dmux-b-usb", 1, Transport::Openssh, true);
            source.managed_socket = Some(rejected.clone());
            assert!(
                build_domain_manifest(vec![source]).is_err(),
                "accepted {rejected:?}"
            );
        }

        let mut socketless = domain_source("dmux-b-usb", 1, Transport::Openssh, true);
        socketless.managed_socket = None;
        assert!(build_domain_manifest(vec![socketless]).is_err());

        let mut unpinned = rows.clone();
        unpinned[0].override_proxy_command = None;
        assert!(validate_domain_manifest(&unpinned).is_err());

        let mut refused = rows.clone();
        refused[1].override_proxy_command = rows[0].override_proxy_command.clone();
        assert!(validate_domain_manifest(&refused).is_err());

        let mut auto_starting = rows.clone();
        auto_starting[0].override_proxy_command = Some(
            "env -u WEZTERM_PANE -u TMUX -u TMUX_PANE \
             WEZTERM_UNIX_SOCKET=/run/user/1000/dmux/wez-dmux.sock \
             /usr/bin/wezterm cli --prefer-mux proxy"
                .into(),
        );
        assert!(validate_domain_manifest(&auto_starting).is_err());

        let mut foreign = rows.clone();
        foreign[0].remote_wezterm_path = Some("/opt/homebrew/bin/wezterm".into());
        assert!(validate_domain_manifest(&foreign).is_err());
    }

    #[test]
    fn per_boot_key_rotation_refuses_a_live_gui() {
        let root = private_root();
        let original = ensure_bridge_key(root.path()).unwrap();
        let runtime = PrivateDir::open(root.path(), 0o700).unwrap();
        let bridge = runtime.child("bridge").unwrap();
        bridge
            .write_replace_atomic(KEY_BOOT_FILE, b"macos:stale:boot")
            .unwrap();
        let rotated = ensure_bridge_key(root.path()).unwrap();
        assert_ne!(original, rotated);

        heartbeat(
            root.path(),
            "gui-42-cafe",
            unix_seconds().unwrap(),
            &marker(),
        );
        bridge
            .write_replace_atomic(KEY_BOOT_FILE, b"macos:stale:boot")
            .unwrap();
        assert_eq!(ensure_bridge_key(root.path()).unwrap(), rotated);
        assert!(matches!(
            rotate_bridge_key_if_idle(root.path()),
            Err(GuiError::BridgeUnavailable(_))
        ));
        assert_eq!(read_bridge_key(root.path()).unwrap(), rotated);
    }

    #[test]
    fn replay_reads_the_original_digest_bound_ack_without_overwrite() {
        let root = private_root();
        let key = ensure_bridge_key(root.path()).unwrap();
        let now = unix_seconds().unwrap();
        let marker = marker();
        heartbeat(root.path(), "gui-42-cafe", now, &marker);
        let selection = discover_in_gui_instance(root.path(), &marker).unwrap();
        let mut ping = request_document(
            "ping",
            serde_json::json!({}),
            in_gui_origin(&selection, &marker, None),
        )
        .unwrap();
        sign_request(&mut ping, &key).unwrap();
        let digest = request_sha256(&ping).unwrap();
        let uid = ping["uid"].as_str().unwrap().to_string();
        let completed_at = unix_seconds().unwrap();
        let ack = serde_json::json!({
            "protocol_version": 1,
            "uid": uid,
            "action": "ping",
            "nonce": ping["nonce"],
            "ok": true,
            "completed_at": completed_at,
            "request_sha256": digest,
            "gui_instance": "gui-42-cafe",
            "pong": true
        });
        let ack_bytes = serde_json::to_vec(&ack).unwrap();
        let runtime = PrivateDir::open(root.path(), 0o700).unwrap();
        let acks = runtime
            .child("bridge")
            .unwrap()
            .child("instances")
            .unwrap()
            .child("gui-42-cafe")
            .unwrap()
            .child("acks")
            .unwrap();
        let ack_name = format!("ack-{uid}.json");
        acks.write_new_atomic(&ack_name, &ack_bytes).unwrap();

        let result = call_instance(root.path(), "gui-42-cafe", &mut ping, ACK_TIMEOUT).unwrap();
        assert_eq!(result["pong"], true);
        assert_eq!(
            acks.read_private_file(&ack_name, MAX_MESSAGE_BYTES)
                .unwrap(),
            ack_bytes
        );

        let mut conflict = request_document(
            "toast",
            serde_json::json!({"message":"different content"}),
            in_gui_origin(&selection, &marker, None),
        )
        .unwrap();
        conflict["uid"] = Value::String(uid);
        assert!(matches!(
            call_instance(root.path(), "gui-42-cafe", &mut conflict, Duration::ZERO),
            Err(GuiError::InvalidAck(_))
        ));
    }

    #[test]
    fn group_only_presentation_ack_reports_the_selected_split() {
        let mut target = request()["target"].clone();
        target.as_object_mut().unwrap().remove("split_ref");
        let presentation =
            request_document("present", target, request()["origin"].clone()).unwrap();
        let digest = request_sha256(&presentation).unwrap();
        let split = "p55555555-5555-4555-8555-555555555555.wz-9";
        let ack = serde_json::json!({
            "protocol_version": 1,
            "uid": presentation["uid"],
            "action": "present",
            "nonce": presentation["nonce"],
            "ok": true,
            "completed_at": unix_seconds().unwrap(),
            "request_sha256": digest,
            "gui_instance": "gui-42-cafe",
            "domain": "dmux-b-usb",
            "workspace": presentation["target"]["workspace"],
            "window_ids": [7],
            "pane_id": 91,
            "group_ref": presentation["target"]["group_ref"],
            "split_ref": split
        });
        let validated = decode_and_validate_ack(
            &serde_json::to_vec(&ack).unwrap(),
            &presentation,
            "gui-42-cafe",
            &digest,
        )
        .unwrap();
        assert_eq!(validated["split_ref"].as_str(), Some(split));

        let mut with_null = ack.clone();
        with_null["already_hidden"] = Value::Null;
        assert!(matches!(
            decode_and_validate_ack(
                &serde_json::to_vec(&with_null).unwrap(),
                &presentation,
                "gui-42-cafe",
                &digest,
            ),
            Err(GuiError::InvalidAck(_))
        ));

        // The decoder reads the spool bytes themselves: a duplicated member
        // is refused rather than collapsed to its last spelling.
        let mut duplicated = serde_json::to_string(&ack).unwrap();
        duplicated.pop();
        duplicated.push_str(&format!(",\"split_ref\":{}}}", serde_json::json!(split)));
        assert!(matches!(
            decode_and_validate_ack(duplicated.as_bytes(), &presentation, "gui-42-cafe", &digest),
            Err(GuiError::InvalidAck(_))
        ));
    }

    /// P9's gate "invalid context always fails closed" claimed against the
    /// production bridge path, not a helper: `call_instance` is the only
    /// signer and the only ack reader, `bind_cli_origin_with_heartbeat` the
    /// only origin binder. Every refusal leaves the request spool untouched.
    #[test]
    fn production_bridge_call_fails_closed_on_invalid_origin_ack_and_context() {
        let root = private_root();
        ensure_bridge_key(root.path()).unwrap();
        let now = unix_seconds().unwrap();
        let marker = marker();
        heartbeat(root.path(), "gui-42-cafe", now, &marker);
        let selection = discover_in_gui_instance(root.path(), &marker).unwrap();
        let instance = PrivateDir::open(root.path(), 0o700)
            .unwrap()
            .child("bridge")
            .unwrap()
            .child("instances")
            .unwrap()
            .child("gui-42-cafe")
            .unwrap();
        let requests = instance.child("requests").unwrap();
        let acks = instance.child("acks").unwrap();
        let ping = || {
            request_document(
                "ping",
                serde_json::json!({}),
                in_gui_origin(&selection, &marker, None),
            )
            .unwrap()
        };
        let call = |request: &mut Value| {
            call_instance(root.path(), "gui-42-cafe", request, Duration::ZERO)
        };

        // Origin: the signed origin must name the selected consumer, spell
        // every UUID canonically, and match the fresh heartbeat's process
        // incarnation. None of these reaches the spool.
        let mut other_consumer = ping();
        other_consumer["origin"]["gui_instance"] = Value::String("gui-43-beef".into());
        assert!(matches!(
            call(&mut other_consumer),
            Err(GuiError::InvalidRequest(_))
        ));
        let mut noncanonical = ping();
        noncanonical["origin"]["host_uid"] = Value::String(marker.host_uid.0.simple().to_string());
        assert!(matches!(
            call(&mut noncanonical),
            Err(GuiError::InvalidRequest(_))
        ));
        let mut stale_process = ping();
        stale_process["origin"]["pid"] = Value::from(999u64);
        assert!(matches!(
            call(&mut stale_process),
            Err(GuiError::InvalidInstance(_))
        ));
        let mut wrong_kind = ping();
        wrong_kind["origin"] = serde_json::json!({
            "kind": "resident_gui",
            "gui_instance": "gui-42-cafe",
            "pid": 123,
            "process_start_token": "start-token",
            "pane_id": 91
        });
        assert!(matches!(
            call(&mut wrong_kind),
            Err(GuiError::InvalidRequest(_))
        ));
        assert!(requests.entry_names().unwrap().is_empty());

        // Ack: a prior ack on the spool is authoritative only when it binds
        // this exact request and consumer; each tampered shape is refused
        // before the request is ever written.
        let valid_ack = |request: &Value| {
            serde_json::json!({
                "protocol_version": 1,
                "uid": request["uid"],
                "action": "ping",
                "nonce": request["nonce"],
                "ok": true,
                "completed_at": unix_seconds().unwrap(),
                "request_sha256": request_sha256(request).unwrap(),
                "gui_instance": "gui-42-cafe",
                "pong": true
            })
        };
        let refuse_ack = |bytes: Vec<u8>, request: &mut Value| {
            let name = format!("ack-{}.json", request["uid"].as_str().unwrap());
            acks.write_new_atomic(&name, &bytes).unwrap();
            assert!(matches!(call(request), Err(GuiError::InvalidAck(_))));
            assert!(requests.entry_names().unwrap().is_empty());
        };
        let mut request = ping();
        let mut ack = valid_ack(&request);
        ack["gui_instance"] = Value::String("gui-43-beef".into());
        refuse_ack(serde_json::to_vec(&ack).unwrap(), &mut request);

        let mut request = ping();
        let mut ack = valid_ack(&request);
        ack["request_sha256"] = Value::String("0".repeat(64));
        refuse_ack(serde_json::to_vec(&ack).unwrap(), &mut request);

        let mut request = ping();
        let mut ack = valid_ack(&request);
        ack["uid"] = Value::String(Uuid::new_v4().to_string());
        refuse_ack(serde_json::to_vec(&ack).unwrap(), &mut request);

        let mut request = ping();
        let mut ack = valid_ack(&request);
        ack["action"] = Value::String("toast".into());
        refuse_ack(serde_json::to_vec(&ack).unwrap(), &mut request);

        let mut request = ping();
        let mut ack = valid_ack(&request);
        ack["already_hidden"] = Value::Null;
        refuse_ack(serde_json::to_vec(&ack).unwrap(), &mut request);

        let mut request = ping();
        let mut ack = valid_ack(&request);
        ack["ok"] = Value::Bool(false);
        refuse_ack(serde_json::to_vec(&ack).unwrap(), &mut request);

        let mut request = ping();
        let mut duplicated = serde_json::to_string(&valid_ack(&request)).unwrap();
        duplicated.pop();
        duplicated.push_str(",\"pong\":true}");
        refuse_ack(duplicated.into_bytes(), &mut request);

        // A typed GUI refusal is delivered as itself, never as success.
        let mut request = ping();
        let mut ack = valid_ack(&request);
        ack.as_object_mut().unwrap().remove("pong");
        ack["ok"] = Value::Bool(false);
        ack["error"] = Value::String("not_found".into());
        ack["message"] = Value::String("opaque workspace is not imported".into());
        let name = format!("ack-{}.json", request["uid"].as_str().unwrap());
        acks.write_new_atomic(&name, &serde_json::to_vec(&ack).unwrap())
            .unwrap();
        assert!(matches!(
            call(&mut request),
            Err(GuiError::Rejected { code, .. }) if code == "not_found"
        ));

        // Context: the CLI locator binds only when the fresh heartbeat names
        // the same pane, domain and owner-revalidated marker.
        let cli = GuiCliOrigin {
            protocol_version: BRIDGE_PROTOCOL_VERSION,
            gui_instance: "gui-42-cafe".into(),
            pane_id: 91,
            domain: "dmux-b-usb".into(),
            tmux_client_uid: None,
            marker: marker.clone(),
        };
        assert!(bind_cli_origin_with_heartbeat(root.path(), &cli, &marker).is_ok());
        let mut other_split = marker.clone();
        other_split.split_ref = format!("p{}.wz-7", Uuid::from_u128(4));
        assert!(matches!(
            bind_cli_origin_with_heartbeat(root.path(), &cli, &other_split),
            Err(GuiError::InvalidInstance(_))
        ));
        let mut other_pane = cli.clone();
        other_pane.pane_id = 92;
        assert!(matches!(
            bind_cli_origin_with_heartbeat(root.path(), &other_pane, &marker),
            Err(GuiError::InvalidInstance(_))
        ));
        let mut other_domain = cli.clone();
        other_domain.domain = "dmux-b-ts".into();
        assert!(matches!(
            bind_cli_origin_with_heartbeat(root.path(), &other_domain, &marker),
            Err(GuiError::InvalidInstance(_))
        ));

        // A stale heartbeat is bridge-down (ADR 003 §3): neither binding nor
        // a fully valid signed request proceeds, and nothing is written.
        heartbeat(root.path(), "gui-42-cafe", now.saturating_sub(10), &marker);
        assert!(matches!(
            bind_cli_origin_with_heartbeat(root.path(), &cli, &marker),
            Err(GuiError::BridgeUnavailable(_))
        ));
        let mut valid = ping();
        assert!(matches!(
            call(&mut valid),
            Err(GuiError::BridgeUnavailable(_))
        ));
        assert!(requests.entry_names().unwrap().is_empty());
    }
}
