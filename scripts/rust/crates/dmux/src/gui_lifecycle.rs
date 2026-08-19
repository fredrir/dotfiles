//! Fixed-service and zero-window GUI lifecycle for the managed Wez backend.
//!
//! This module is deliberately narrower than presentation.  It may start the
//! one platform service, prove that service's descriptor/registry/inventory
//! identity, and launch ADR 003's attach-only GUI.  It never selects a Space,
//! signs a bridge action, or mutates an owner resource.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use uuid::Uuid;

use crate::backend::wez::{
    IdentityExpectation, SCRUBBED_ENV, SOCKET_ENV, SystemRunner as WezSystemRunner, WezProvider,
};
use crate::backend::{InventoryOutcome, InventoryScope, Provider};
use crate::connect_cli::FrozenConnectTarget;
use crate::error::{ErrorCode, TypedError};
use crate::gui::{self, BridgeInstanceSelection};
use crate::model::{Backend, BackendInstanceUid, HostUid, ServerEpoch, SpaceUid};
use crate::new_cli::{NewPresentationMode, WezPresentationPreflight};
use crate::registry::Registry;
use crate::runtime::{self, WezMuxDescriptor};

pub const SERVICE_READY_TIMEOUT: Duration = Duration::from_secs(15);
pub const GUI_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(10);
pub const POLL_INTERVAL: Duration = Duration::from_millis(25);
pub const SERVICE_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

pub const MACOS_SERVICE_LABEL: &str = "com.fredrir.wezterm-mux";
pub const LINUX_SERVICE_LABEL: &str = "wezterm-mux.service";
pub const GUI_INSTANCE_ENV: &str = "DMUX_GUI_INSTANCE";
pub const GUI_LAUNCHER_REQUEST_UID_ENV: &str = "DMUX_GUI_LAUNCHER_REQUEST_UID";
pub const GUI_LAUNCHER_PID_ENV: &str = "DMUX_GUI_LAUNCHER_PID";
pub const GUI_LAUNCHER_START_TOKEN_ENV: &str = "DMUX_GUI_LAUNCHER_START_TOKEN";
pub const GUI_BACKEND_INSTANCE_ENV: &str = "DMUX_GUI_BACKEND_INSTANCE";
pub const GUI_TARGET_HOST_UID_ENV: &str = "DMUX_GUI_TARGET_HOST_UID";
pub const GUI_TARGET_DOMAIN_ENV: &str = "DMUX_GUI_TARGET_DOMAIN";
pub const GUI_TARGET_BACKEND_INSTANCE_ENV: &str = "DMUX_GUI_TARGET_BACKEND_INSTANCE";
pub const GUI_TARGET_SERVER_EPOCH_ENV: &str = "DMUX_GUI_TARGET_SERVER_EPOCH";
pub const GUI_TARGET_SPACE_UID_ENV: &str = "DMUX_GUI_TARGET_SPACE_UID";

const COLD_LAUNCH_WITNESS_ENV: [&str; 10] = [
    GUI_INSTANCE_ENV,
    GUI_LAUNCHER_REQUEST_UID_ENV,
    GUI_LAUNCHER_PID_ENV,
    GUI_LAUNCHER_START_TOKEN_ENV,
    GUI_BACKEND_INSTANCE_ENV,
    GUI_TARGET_HOST_UID_ENV,
    GUI_TARGET_DOMAIN_ENV,
    GUI_TARGET_BACKEND_INSTANCE_ENV,
    GUI_TARGET_SERVER_EPOCH_ENV,
    GUI_TARGET_SPACE_UID_ENV,
];

/// Identity returned only after the private descriptor, durable authority,
/// and one complete sentinel-verified exact-socket inventory all agree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyWezService {
    pub socket: String,
    pub backend_instance_uid: BackendInstanceUid,
    pub server_epoch: ServerEpoch,
}

/// Armed launch guard for a GUI correlated to its exact fresh heartbeat.
/// Until resident establishment succeeds and [`Self::commit`] is called,
/// dropping this value terminates and reaps the exact spawned process.
pub struct LaunchedGui {
    committed: Option<CommittedGui>,
    child: Option<Box<dyn LifecycleChild>>,
}

/// Stable launch evidence returned after the bridge has established resident
/// provenance and explicitly disarmed the launch guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommittedGui {
    pub instance: BridgeInstanceSelection,
    pub launcher_request_uid: Uuid,
    pub class: String,
    pub launcher_witness: ColdLauncherWitness,
}

impl LaunchedGui {
    fn committed(&self) -> &CommittedGui {
        self.committed
            .as_ref()
            .expect("uncommitted GUI launch guard always retains stable evidence")
    }

    pub fn instance(&self) -> &BridgeInstanceSelection {
        &self.committed().instance
    }

    pub fn launcher_request_uid(&self) -> Uuid {
        self.committed().launcher_request_uid
    }

    pub fn class(&self) -> &str {
        &self.committed().class
    }

    pub fn launcher_witness(&self) -> &ColdLauncherWitness {
        &self.committed().launcher_witness
    }

    /// Disarm automatic cleanup only after the one-use resident bridge
    /// establishment has succeeded. Dropping the retained child handle does
    /// not terminate the now-committed GUI process.
    pub fn commit(mut self) -> CommittedGui {
        drop(self.child.take());
        self.committed
            .take()
            .expect("GUI launch guard can be committed only once")
    }
}

impl fmt::Debug for LaunchedGui {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LaunchedGui")
            .field("committed", &self.committed)
            .field("cleanup_armed", &self.child.is_some())
            .finish()
    }
}

impl Drop for LaunchedGui {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.terminate_and_reap();
        }
    }
}

/// Owner-validated presentation intent frozen before the GUI child exists.
/// The fields are private so lifecycle callers cannot independently mix a
/// domain from one preflight with the incarnation of another target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColdLaunchIntent {
    launcher_request_uid: Uuid,
    gui_instance: String,
    owner: HostUid,
    domain: String,
    backend_instance_uid: BackendInstanceUid,
    server_epoch: ServerEpoch,
    space_uid: Option<SpaceUid>,
}

impl ColdLaunchIntent {
    /// Freeze `new` presentation authority before a Space identity exists.
    /// This is the only constructor that can produce an intent without a
    /// target Space UID.
    pub fn from_new_preflight(
        preflight: &WezPresentationPreflight,
        launcher_request_uid: Uuid,
    ) -> Result<Self, TypedError> {
        Self::from_verified_preflight(preflight, None, launcher_request_uid)
    }

    /// Freeze an already-existing Space target.  The independently frozen
    /// target and GUI route preflight must agree on owner/backend/epoch.
    pub fn from_existing_target(
        preflight: &WezPresentationPreflight,
        existing_target: &FrozenConnectTarget,
        launcher_request_uid: Uuid,
    ) -> Result<Self, TypedError> {
        Self::from_verified_preflight(preflight, Some(existing_target), launcher_request_uid)
    }

    fn from_verified_preflight(
        preflight: &WezPresentationPreflight,
        existing_target: Option<&FrozenConnectTarget>,
        launcher_request_uid: Uuid,
    ) -> Result<Self, TypedError> {
        if launcher_request_uid.is_nil() {
            return Err(TypedError::new(
                ErrorCode::ProtocolMismatch,
                "cold GUI launch has a nil launcher request UID",
            ));
        }
        let expected_instance = format!("gui-{}", launcher_request_uid.simple());
        if preflight.mode != NewPresentationMode::Cold {
            return Err(TypedError::new(
                ErrorCode::IdentityConflict,
                "cold GUI launch requires a cold presentation preflight",
            ));
        }
        if preflight.gui_instance != expected_instance {
            return Err(TypedError::new(
                ErrorCode::IdentityConflict,
                "cold GUI launch preflight names a different deterministic GUI instance",
            ));
        }
        if preflight.owner.0.is_nil()
            || preflight.backend_instance_uid.0.is_nil()
            || preflight.server_epoch.0.is_nil()
            || !valid_domain(&preflight.domain)
        {
            return Err(TypedError::new(
                ErrorCode::ProtocolMismatch,
                "cold GUI launch preflight has malformed owner/domain/backend identity",
            ));
        }

        let space_uid = match existing_target {
            Some(target)
                if target.backend == Backend::Wez
                    && target.owner == preflight.owner
                    && target.backend_instance_uid == preflight.backend_instance_uid
                    && target.server_epoch == preflight.server_epoch
                    && !target.space_uid.0.is_nil() =>
            {
                Some(target.space_uid)
            }
            Some(_) => {
                return Err(TypedError::new(
                    ErrorCode::IdentityConflict,
                    "cold GUI launch target differs from its owner-validated presentation preflight",
                ));
            }
            None => None,
        };

        Ok(Self {
            launcher_request_uid,
            gui_instance: expected_instance,
            owner: preflight.owner,
            domain: preflight.domain.clone(),
            backend_instance_uid: preflight.backend_instance_uid,
            server_epoch: preflight.server_epoch,
            space_uid,
        })
    }

    pub fn launcher_request_uid(&self) -> Uuid {
        self.launcher_request_uid
    }

    pub fn gui_instance(&self) -> &str {
        &self.gui_instance
    }

    pub fn owner(&self) -> HostUid {
        self.owner
    }

    pub fn domain(&self) -> &str {
        &self.domain
    }

    pub fn backend_instance_uid(&self) -> BackendInstanceUid {
        self.backend_instance_uid
    }

    pub fn server_epoch(&self) -> ServerEpoch {
        self.server_epoch
    }

    pub fn space_uid(&self) -> Option<SpaceUid> {
        self.space_uid
    }
}

/// Exact process witness and immutable intent actually inherited by a newly
/// launched GUI.  The bridge origin must use this returned snapshot rather
/// than recomputing or substituting a registry token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColdLauncherWitness {
    gui_instance: String,
    launcher_request_uid: Uuid,
    process: LauncherProcessWitness,
    intent: ColdLaunchIntent,
}

impl ColdLauncherWitness {
    pub fn gui_instance(&self) -> &str {
        &self.gui_instance
    }

    pub fn launcher_request_uid(&self) -> Uuid {
        self.launcher_request_uid
    }

    pub fn process(&self) -> &LauncherProcessWitness {
        &self.process
    }

    pub fn intent(&self) -> &ColdLaunchIntent {
        &self.intent
    }
}

/// Pure command description used by both the real runner and deterministic
/// tests.  `env_remove` and `env_set` are deltas: ADR 003 requires removal of
/// ambient mux selectors while retaining the user's normal GUI environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleCommand {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub env_remove: Vec<OsString>,
    pub env_set: BTreeMap<OsString, OsString>,
}

impl LifecycleCommand {
    fn new(program: impl Into<OsString>, args: impl IntoIterator<Item = OsString>) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().collect(),
            env_remove: Vec::new(),
            env_set: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandExit {
    pub success: bool,
    pub code: Option<i32>,
}

/// Retained child handle for a cold GUI launch.  A failed correlation must
/// terminate and reap this exact process instead of stranding an extra GUI.
pub trait LifecycleChild {
    fn pid(&self) -> u32;
    fn process_start_token(&self) -> &str;
    fn terminate_and_reap(&mut self) -> io::Result<()>;
}

/// Injectable command seam.  The production implementation never invokes a
/// shell and bounds the service-manager child separately from readiness.
pub trait LifecycleCommandRunner {
    fn run_bounded(&self, command: &LifecycleCommand, timeout: Duration)
    -> io::Result<CommandExit>;

    fn spawn(&self, command: &LifecycleCommand) -> io::Result<Box<dyn LifecycleChild>>;
}

/// Immutable identity of the dmux process that launches a cold GUI.  The
/// maintained fork consumes this witness once and proves that it is still the
/// GUI's live same-user parent before accepting the first bridge request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherProcessWitness {
    pub uid: u64,
    pub pid: u32,
    pub start_token: String,
}

/// Injectable parent-process identity seam for deterministic cold-launch
/// tests.  Production always uses [`SystemLauncherWitnessSource`].
pub trait LauncherWitnessSource {
    fn current(&self) -> io::Result<LauncherProcessWitness>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemLauncherWitnessSource;

impl LauncherWitnessSource for SystemLauncherWitnessSource {
    fn current(&self) -> io::Result<LauncherProcessWitness> {
        let pid = std::process::id();
        Ok(LauncherProcessWitness {
            uid: u64::from(unsafe { libc::geteuid() }),
            pid,
            start_token: current_process_start_token()?,
        })
    }
}

/// Return the OS-native start witness for this exact dmux process.  The cold
/// bridge request uses the same stable token that the launcher places in the
/// GUI child's environment.
pub fn current_process_start_token() -> io::Result<String> {
    runtime::process_start_token(std::process::id())
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemCommandRunner;

impl LifecycleCommandRunner for SystemCommandRunner {
    fn run_bounded(
        &self,
        command: &LifecycleCommand,
        timeout: Duration,
    ) -> io::Result<CommandExit> {
        let mut child = system_command(command).spawn()?;
        let started = Instant::now();
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok(CommandExit {
                    success: status.success(),
                    code: status.code(),
                });
            }
            if started.elapsed() >= timeout {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "{} did not finish within {} ms",
                        Path::new(&command.program).display(),
                        timeout.as_millis()
                    ),
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn spawn(&self, command: &LifecycleCommand) -> io::Result<Box<dyn LifecycleChild>> {
        let mut child = system_command(command).spawn()?;
        let pid = child.id();
        let process_start_token = match gui_heartbeat_start_token(pid) {
            Ok(token) => token,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        Ok(Box::new(SystemLifecycleChild {
            child: Some(child),
            process_start_token,
        }))
    }
}

struct SystemLifecycleChild {
    child: Option<Child>,
    process_start_token: String,
}

impl LifecycleChild for SystemLifecycleChild {
    fn pid(&self) -> u32 {
        self.child.as_ref().map(Child::id).unwrap_or(0)
    }

    fn process_start_token(&self) -> &str {
        &self.process_start_token
    }

    fn terminate_and_reap(&mut self) -> io::Result<()> {
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };
        if child.try_wait()?.is_none() {
            if let Err(error) = child.kill()
                && error.kind() != io::ErrorKind::InvalidInput
            {
                return Err(error);
            }
            let _ = child.wait()?;
        }
        self.child = None;
        Ok(())
    }
}

/// Match the bridge consumer's process-instance token byte-for-byte.  The
/// Lua side uses this same fixed `/bin/ps` query under `LC_ALL=C`.
fn gui_heartbeat_start_token(pid: u32) -> io::Result<String> {
    let output = Command::new("/bin/ps")
        .env_clear()
        .env("LC_ALL", "C")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .stdin(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "/bin/ps could not identify launched GUI pid {pid}"
        )));
    }
    let token = std::str::from_utf8(&output.stdout)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "/bin/ps output is not UTF-8"))?
        .trim()
        .to_string();
    if token.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("launched GUI pid {pid} has no process start token"),
        ));
    }
    Ok(token)
}

fn system_command(spec: &LifecycleCommand) -> Command {
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for key in &spec.env_remove {
        command.env_remove(key);
    }
    for (key, value) in &spec.env_set {
        command.env(key, value);
    }
    command
}

/// Monotonic clock seam.  Synchronous polling is intentional: dmux is a
/// short-lived CLI and owns no async runtime; tests advance this clock
/// without sleeping.
pub trait LifecycleClock {
    fn now(&self) -> Duration;
    fn sleep(&self, duration: Duration);
}

#[derive(Debug)]
pub struct SystemClock {
    started: Instant,
}

impl Default for SystemClock {
    fn default() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}

impl LifecycleClock for SystemClock {
    fn now(&self) -> Duration {
        self.started.elapsed()
    }

    fn sleep(&self, duration: Duration) {
        thread::sleep(duration);
    }
}

pub trait DescriptorSource {
    fn read(&self, runtime_dir: &Path) -> io::Result<Option<WezMuxDescriptor>>;

    fn read_verified_ready(
        &self,
        runtime_dir: &Path,
        expected_instance: Uuid,
        expected_epoch: Uuid,
    ) -> io::Result<Option<WezMuxDescriptor>>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RuntimeDescriptorSource;

impl DescriptorSource for RuntimeDescriptorSource {
    fn read(&self, runtime_dir: &Path) -> io::Result<Option<WezMuxDescriptor>> {
        runtime::read_wez_descriptor_in(runtime_dir)
    }

    fn read_verified_ready(
        &self,
        runtime_dir: &Path,
        expected_instance: Uuid,
        expected_epoch: Uuid,
    ) -> io::Result<Option<WezMuxDescriptor>> {
        runtime::read_verified_ready_wez_descriptor_in(
            runtime_dir,
            Some(expected_instance),
            Some(expected_epoch),
        )
    }
}

/// Inventory seam whose production implementation constructs a Wez provider
/// with the descriptor's exact process witness.  A `Complete` result from
/// this seam therefore includes the provider's sentinel-in-list handshake;
/// the sentinel itself is intentionally filtered from returned user rows.
pub trait WezServiceInventory {
    fn inventory(&self, scope: &InventoryScope, descriptor: &WezMuxDescriptor) -> InventoryOutcome;
}

#[derive(Debug, Clone)]
pub struct SystemWezServiceInventory {
    wezterm_bin: String,
    mux_config: String,
}

impl SystemWezServiceInventory {
    pub fn new(wezterm_bin: impl Into<String>, mux_config: impl Into<String>) -> Self {
        Self {
            wezterm_bin: wezterm_bin.into(),
            mux_config: mux_config.into(),
        }
    }
}

impl WezServiceInventory for SystemWezServiceInventory {
    fn inventory(&self, scope: &InventoryScope, descriptor: &WezMuxDescriptor) -> InventoryOutcome {
        let provider: WezProvider<WezSystemRunner> =
            WezProvider::new(&self.wezterm_bin, &self.mux_config).with_identity(
                IdentityExpectation {
                    server_pid: Some(descriptor.pid),
                    start_token: Some(descriptor.start_token.clone()),
                },
            );
        provider.inventory(scope)
    }
}

pub trait HeartbeatSource {
    fn live_instances(
        &self,
        runtime_dir: &Path,
    ) -> Result<Vec<BridgeInstanceSelection>, TypedError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RuntimeHeartbeatSource;

impl HeartbeatSource for RuntimeHeartbeatSource {
    fn live_instances(
        &self,
        runtime_dir: &Path,
    ) -> Result<Vec<BridgeInstanceSelection>, TypedError> {
        let instances = gui::bridge_root(runtime_dir).join("instances");
        let metadata = match fs::symlink_metadata(&instances) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(bridge_io("reading GUI instance directory", error)),
        };
        let euid = unsafe { libc::geteuid() };
        if !metadata.is_dir() || metadata.uid() != euid || metadata.mode() & 0o777 != 0o700 {
            return Err(TypedError::new(
                ErrorCode::BridgeUnavailable,
                format!(
                    "GUI instance directory {} must be a current-user-owned non-symlink mode-0700 directory",
                    instances.display()
                ),
            ));
        }

        let entries = fs::read_dir(&instances)
            .map_err(|error| bridge_io("enumerating GUI instances", error))?;
        let mut live = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| bridge_io("enumerating GUI instances", error))?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            // Stale or malformed instance directories are not live.  The
            // public reader performs private-dir, schema, timestamp, and
            // process-instance validation before yielding a heartbeat.
            if let Ok(heartbeat) = gui::read_instance_heartbeat(runtime_dir, &name) {
                live.push(BridgeInstanceSelection {
                    gui_instance: heartbeat.gui_instance,
                    pid: heartbeat.pid,
                    process_start_token: heartbeat.process_start_token,
                    domains: heartbeat.domains,
                });
            }
        }
        live.sort_by_key(instance_key);
        Ok(live)
    }
}

fn bridge_io(context: &str, error: io::Error) -> TypedError {
    TypedError::new(ErrorCode::BridgeUnavailable, format!("{context}: {error}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixedServicePlatform {
    MacOs { uid: u32 },
    Linux,
}

impl FixedServicePlatform {
    pub fn current() -> Result<Self, TypedError> {
        #[cfg(target_os = "macos")]
        {
            return Ok(Self::MacOs {
                uid: unsafe { libc::geteuid() },
            });
        }
        #[cfg(target_os = "linux")]
        {
            return Ok(Self::Linux);
        }
        #[allow(unreachable_code)]
        Err(TypedError::new(
            ErrorCode::ProviderUnavailable,
            "managed Wez service startup is supported only on macOS and Linux",
        ))
    }

    pub fn service_label(self) -> &'static str {
        match self {
            Self::MacOs { .. } => MACOS_SERVICE_LABEL,
            Self::Linux => LINUX_SERVICE_LABEL,
        }
    }

    pub fn start_command(self) -> LifecycleCommand {
        match self {
            Self::MacOs { uid } => LifecycleCommand::new(
                "/bin/launchctl",
                [
                    OsString::from("kickstart"),
                    OsString::from(format!("gui/{uid}/{MACOS_SERVICE_LABEL}")),
                ],
            ),
            Self::Linux => LifecycleCommand::new(
                "/usr/bin/systemctl",
                [
                    OsString::from("--user"),
                    OsString::from("start"),
                    OsString::from(LINUX_SERVICE_LABEL),
                ],
            ),
        }
    }
}

pub struct ServiceEnsureDeps<'a> {
    pub command: &'a dyn LifecycleCommandRunner,
    pub descriptor: &'a dyn DescriptorSource,
    pub inventory: &'a dyn WezServiceInventory,
    pub clock: &'a dyn LifecycleClock,
}

pub struct GuiLaunchDeps<'a> {
    pub command: &'a dyn LifecycleCommandRunner,
    pub heartbeats: &'a dyn HeartbeatSource,
    pub launcher: &'a dyn LauncherWitnessSource,
    pub clock: &'a dyn LifecycleClock,
}

/// Production fixed-service ensure.  The caller supplies only executable and
/// config paths; no service label or arbitrary command is accepted.
pub fn ensure_ready_wez_service(
    registry: &Registry,
    runtime_dir: &Path,
    wezterm_bin: &str,
    mux_config: &str,
) -> Result<ReadyWezService, TypedError> {
    if !runtime_dir.is_absolute()
        || !Path::new(wezterm_bin).is_absolute()
        || !Path::new(mux_config).is_absolute()
    {
        return Err(TypedError::new(
            ErrorCode::Usage,
            "managed Wez ensure requires absolute runtime, wezterm binary, and mux-config paths",
        ));
    }
    let platform = FixedServicePlatform::current()?;
    let command = SystemCommandRunner;
    let descriptor = RuntimeDescriptorSource;
    let inventory = SystemWezServiceInventory::new(wezterm_bin, mux_config);
    let clock = SystemClock::default();
    ensure_ready_wez_service_with(
        registry,
        runtime_dir,
        platform,
        ServiceEnsureDeps {
            command: &command,
            descriptor: &descriptor,
            inventory: &inventory,
            clock: &clock,
        },
        SERVICE_READY_TIMEOUT,
    )
}

/// Injectable form used by focused fault tests.  It still constructs the
/// fixed service command itself, so an injected runner cannot turn this API
/// into an arbitrary service starter.
pub fn ensure_ready_wez_service_with(
    registry: &Registry,
    runtime_dir: &Path,
    platform: FixedServicePlatform,
    deps: ServiceEnsureDeps<'_>,
    timeout: Duration,
) -> Result<ReadyWezService, TypedError> {
    let start = platform.start_command();
    let exit = deps
        .command
        .run_bounded(&start, SERVICE_COMMAND_TIMEOUT)
        .map_err(|error| {
            TypedError::new(
                ErrorCode::ProviderUnavailable,
                format!("could not start fixed Wez service: {error}"),
            )
        })?;
    if !exit.success {
        return Err(TypedError::new(
            ErrorCode::ProviderUnavailable,
            format!(
                "fixed Wez service start failed{}",
                exit.code
                    .map(|code| format!(" with exit {code}"))
                    .unwrap_or_else(|| " after a signal".to_string())
            ),
        ));
    }

    let deadline = deadline(deps.clock.now(), timeout);
    loop {
        let not_ready = match deps.descriptor.read(runtime_dir) {
            Ok(None) => TypedError::new(
                ErrorCode::ProviderUnavailable,
                "fixed Wez service has not published a private descriptor",
            ),
            Err(error) => return Err(bridge_descriptor_error(error)),
            Ok(Some(descriptor)) => match descriptor.state.as_str() {
                "starting" | "recovering" => TypedError::new(
                    ErrorCode::ProviderUnavailable,
                    format!("managed Wez service is {}", descriptor.state),
                ),
                "failed" => {
                    return Err(TypedError::new(
                        ErrorCode::ProviderUnavailable,
                        format!(
                            "managed Wez service failed{}",
                            descriptor
                                .error
                                .as_deref()
                                .map(|detail| format!(": {detail}"))
                                .unwrap_or_default()
                        ),
                    ));
                }
                "ready" => {
                    let (expected_instance, expected_epoch) = ready_descriptor_uuids(&descriptor)?;
                    let verified = match deps.descriptor.read_verified_ready(
                        runtime_dir,
                        expected_instance,
                        expected_epoch,
                    ) {
                        Ok(Some(verified)) => verified,
                        Ok(None) => {
                            return Err(TypedError::new(
                                ErrorCode::ProviderUnavailable,
                                "managed Wez ready descriptor disappeared before native verification",
                            ));
                        }
                        Err(error) => return Err(bridge_descriptor_error(error)),
                    };
                    match validate_ready_descriptor(registry, platform, deps.inventory, &verified) {
                        Ok(ready) => return Ok(ready),
                        Err(ReadyRejection::Retry(error)) => error,
                        Err(ReadyRejection::Fatal(error)) => return Err(error),
                    }
                }
                other => {
                    return Err(TypedError::new(
                        ErrorCode::ProtocolMismatch,
                        format!("managed Wez descriptor has unknown state {other:?}"),
                    ));
                }
            },
        };

        if deps.clock.now() >= deadline {
            return Err(TypedError::new(
                not_ready.code,
                format!(
                    "timed out after {} ms waiting for managed Wez service: {detail}",
                    timeout.as_millis(),
                    detail = not_ready.message
                ),
            ));
        }
        deps.clock
            .sleep(next_sleep(deps.clock.now(), deadline, POLL_INTERVAL));
    }
}

/// Why one poll of a published descriptor did not produce a ready service.
///
/// The distinction is whether *waiting* can change the answer. A service that
/// is still coming up will publish a better descriptor, and the next read
/// picks it up. A descriptor that is already `ready` and disagrees with the
/// registry cannot: [`validate_ready_descriptor`] takes `&Registry`, so
/// nothing on this path republishes, and no later read of the same two values
/// can differ. Polling that to the deadline burns the whole timeout to reach
/// the same verdict, so it is reported at once instead.
enum ReadyRejection {
    /// Not ready *yet*: poll again until the deadline.
    Retry(TypedError),
    /// Ready and wrong: nothing this call path does will change it.
    Fatal(TypedError),
}

impl ReadyRejection {
    /// Classify by code, for the rejections whose convergence depends on the
    /// backend rather than on the registry.
    fn classify(error: TypedError) -> ReadyRejection {
        if retryable_ready_error(error.code) {
            ReadyRejection::Retry(error)
        } else {
            ReadyRejection::Fatal(error)
        }
    }
}

fn retryable_ready_error(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::ProviderUnavailable | ErrorCode::BackendEpochChanged
    )
}

fn validate_ready_descriptor(
    registry: &Registry,
    platform: FixedServicePlatform,
    inventory: &dyn WezServiceInventory,
    descriptor: &WezMuxDescriptor,
) -> Result<ReadyWezService, ReadyRejection> {
    // Every registry comparison in this function is fatal by construction:
    // it reads a ready descriptor against a `&Registry` nothing on this path
    // writes, so re-reading the same two settled values cannot answer
    // differently.  Only the backend comparisons at the end can converge.
    let fatal = ReadyRejection::Fatal;
    let (descriptor_instance, descriptor_epoch) =
        ready_descriptor_uuids(descriptor).map_err(fatal)?;
    let descriptor_instance = BackendInstanceUid(descriptor_instance);
    let descriptor_epoch = ServerEpoch(descriptor_epoch);

    let registry_instance = registry
        .backend_instance_for_backend(Backend::Wez)
        .map_err(|error| fatal(registry_error(error)))?
        .ok_or_else(|| {
            fatal(TypedError::new(
                ErrorCode::WrongBackendInstance,
                "registry has no managed Wez backend instance",
            ))
        })?;
    if registry_instance != descriptor_instance {
        return Err(fatal(TypedError::new(
            ErrorCode::WrongBackendInstance,
            format!(
                "descriptor backend instance {} differs from registry {}",
                descriptor_instance.0, registry_instance.0
            ),
        )));
    }

    let info = registry
        .backend_instance_info(registry_instance)
        .map_err(|error| fatal(registry_error(error)))?;
    if info.backend != Backend::Wez
        || info.socket_path.as_deref() != Some(descriptor.socket.as_str())
        || info.service_label.as_deref() != Some(platform.service_label())
    {
        return Err(fatal(TypedError::new(
            ErrorCode::WrongBackendInstance,
            format!(
                "descriptor socket/service does not match registered Wez instance {}",
                registry_instance.0
            ),
        )));
    }

    let server = registry
        .backend_server(registry_instance)
        .map_err(|error| fatal(registry_error(error)))?;
    // The descriptor is `ready`, so its epoch is final; the registry's is
    // whatever the last republication left.  Waiting cannot reconcile them:
    // the only republisher is the mux's own recovery coordinator, which no
    // read path — least of all this one — may stand in for.  Say so now and
    // name the remedy, rather than spending the deadline re-reading two
    // values that are already settled.
    if server.server_epoch != Some(descriptor_epoch) {
        return Err(fatal(TypedError::new(
            ErrorCode::BackendEpochChanged,
            format!(
                "managed Wez service is ready at epoch {} but the registry records {}; \
                 the registry's server incarnation is stale and waiting cannot refresh it. \
                 Restart the managed Wez service so it republishes, then re-run `dmux doctor`",
                descriptor_epoch.0,
                server
                    .server_epoch
                    .map(|epoch| epoch.0.to_string())
                    .unwrap_or_else(|| "<unpublished>".to_string())
            ),
        )));
    }
    if server.server_pid != Some(i64::from(descriptor.pid))
        || server.server_start_token.as_deref() != Some(descriptor.start_token.as_str())
        || server.socket_dev
            != descriptor
                .socket_dev
                .and_then(|value| i64::try_from(value).ok())
        || server.socket_ino
            != descriptor
                .socket_ino
                .and_then(|value| i64::try_from(value).ok())
    {
        return Err(fatal(TypedError::new(
            ErrorCode::WrongBackendInstance,
            "descriptor process/socket witness differs from the registry-published server incarnation",
        )));
    }

    let scope = InventoryScope {
        backend: Backend::Wez,
        endpoint: descriptor.socket.clone(),
        expected_epoch: Some(descriptor_epoch),
    };
    match inventory.inventory(&scope, descriptor) {
        InventoryOutcome::Complete(complete) => {
            // Retryable, unlike the registry comparison above: this one is
            // against the live server, which can still replace itself and
            // publish a descriptor the next read agrees with.
            if complete.server_epoch != Some(descriptor_epoch) {
                return Err(ReadyRejection::Retry(TypedError::new(
                    ErrorCode::BackendEpochChanged,
                    "sentinel-verified inventory epoch differs from descriptor epoch",
                )));
            }
            // Nonempty user rows are valid here.  The provider has already
            // excluded exactly one sentinel and this lifecycle seam must not
            // mistake existing Spaces for a failed readiness check.
        }
        other => return Err(ReadyRejection::classify(inventory_error(other))),
    }

    Ok(ReadyWezService {
        socket: descriptor.socket.clone(),
        backend_instance_uid: registry_instance,
        server_epoch: descriptor_epoch,
    })
}

fn ready_descriptor_uuids(descriptor: &WezMuxDescriptor) -> Result<(Uuid, Uuid), TypedError> {
    if descriptor.descriptor_version != 1 {
        return Err(TypedError::new(
            ErrorCode::ProtocolMismatch,
            format!(
                "managed Wez descriptor version {} is unsupported",
                descriptor.descriptor_version
            ),
        ));
    }
    descriptor
        .require_ready()
        .map_err(bridge_descriptor_error)?;
    let descriptor_instance = descriptor
        .backend_instance_uid
        .as_deref()
        .expect("require_ready checked backend_instance_uid")
        .parse::<Uuid>()
        .map_err(|error| {
            TypedError::new(
                ErrorCode::ProtocolMismatch,
                format!("managed Wez descriptor backend instance is invalid: {error}"),
            )
        })?;
    let descriptor_epoch = descriptor.epoch.parse::<Uuid>().map_err(|error| {
        TypedError::new(
            ErrorCode::ProtocolMismatch,
            format!("managed Wez descriptor epoch is invalid: {error}"),
        )
    })?;

    Ok((descriptor_instance, descriptor_epoch))
}

fn registry_error(error: crate::registry::RegistryError) -> TypedError {
    TypedError::new(error.error_code(), error.to_string())
}

fn bridge_descriptor_error(error: io::Error) -> TypedError {
    let code = match error.kind() {
        io::ErrorKind::PermissionDenied => ErrorCode::BridgeUnavailable,
        io::ErrorKind::InvalidData | io::ErrorKind::InvalidInput => ErrorCode::ProtocolMismatch,
        _ => ErrorCode::ProviderUnavailable,
    };
    TypedError::new(code, format!("managed Wez descriptor: {error}"))
}

fn inventory_error(outcome: InventoryOutcome) -> TypedError {
    let (code, detail) = match outcome {
        InventoryOutcome::Complete(_) => unreachable!("handled by caller"),
        InventoryOutcome::ServerStopped { detail }
        | InventoryOutcome::Unreachable { detail }
        | InventoryOutcome::CommandMissing { detail }
        | InventoryOutcome::Timeout { detail }
        | InventoryOutcome::PermissionFailure { detail } => {
            (ErrorCode::ProviderUnavailable, detail)
        }
        InventoryOutcome::AuthFailed { detail } => (ErrorCode::AuthFailed, detail),
        InventoryOutcome::HostKeyIdentityFailed { detail } => {
            (ErrorCode::HostIdentityChanged, detail)
        }
        InventoryOutcome::VersionMismatch { detail } => (ErrorCode::VersionMismatch, detail),
        InventoryOutcome::ProtocolMismatch { detail } => (ErrorCode::ProtocolMismatch, detail),
        InventoryOutcome::Malformed { detail } => (ErrorCode::PostconditionFailed, detail),
    };
    TypedError::new(
        code,
        format!("managed Wez readiness inventory failed: {detail}"),
    )
}

/// Production ADR-003 cold GUI launcher.  It attaches only to the already
/// verified `dmux` domain and returns only after correlating a unique new
/// heartbeat to the spawned process.
pub fn launch_attach_only_gui(
    runtime_dir: &Path,
    ready: &ReadyWezService,
    wezterm_bin: &str,
    gui_config: &Path,
    launcher_request_uid: Uuid,
    intent: &ColdLaunchIntent,
) -> Result<LaunchedGui, TypedError> {
    let command = SystemCommandRunner;
    let heartbeats = RuntimeHeartbeatSource;
    let launcher = SystemLauncherWitnessSource;
    let clock = SystemClock::default();
    launch_attach_only_gui_with(
        runtime_dir,
        ready,
        wezterm_bin,
        gui_config,
        launcher_request_uid,
        intent,
        GuiLaunchDeps {
            command: &command,
            heartbeats: &heartbeats,
            launcher: &launcher,
            clock: &clock,
        },
        GUI_HEARTBEAT_TIMEOUT,
    )
}

pub fn launch_attach_only_gui_with(
    runtime_dir: &Path,
    ready: &ReadyWezService,
    wezterm_bin: &str,
    gui_config: &Path,
    launcher_request_uid: Uuid,
    intent: &ColdLaunchIntent,
    deps: GuiLaunchDeps<'_>,
    timeout: Duration,
) -> Result<LaunchedGui, TypedError> {
    validate_launch_inputs(
        runtime_dir,
        ready,
        wezterm_bin,
        gui_config,
        launcher_request_uid,
    )?;
    let class = format!("dmux-{}", launcher_request_uid.simple());
    let requested_instance = format!("gui-{}", launcher_request_uid.simple());
    if intent.launcher_request_uid() != launcher_request_uid
        || intent.gui_instance() != requested_instance
    {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "cold GUI launch intent differs from the deterministic launcher request instance",
        ));
    }
    validate_transport_target_relation(ready, intent)?;
    let launcher = deps.launcher.current().map_err(|error| {
        TypedError::new(
            ErrorCode::BridgeUnavailable,
            format!("could not authenticate cold GUI launcher process: {error}"),
        )
    })?;
    validate_launcher_witness(&launcher)?;
    let baseline = deps.heartbeats.live_instances(runtime_dir)?;
    if baseline
        .iter()
        .any(|instance| instance.gui_instance == requested_instance)
    {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            format!("requested GUI instance {requested_instance} is already live"),
        ));
    }
    let baseline_keys: BTreeSet<_> = baseline.iter().map(instance_key).collect();

    let launch = attach_only_command(
        runtime_dir,
        ready,
        wezterm_bin,
        gui_config,
        &class,
        &requested_instance,
        launcher_request_uid,
        &launcher,
        intent,
    );
    let spawned = deps.command.spawn(&launch).map_err(|error| {
        TypedError::new(
            ErrorCode::BridgeUnavailable,
            format!("could not launch attach-only Wez GUI: {error}"),
        )
    })?;
    if spawned.pid() == 0 {
        return Err(cleanup_launched(
            spawned,
            TypedError::new(
                ErrorCode::BridgeUnavailable,
                "attach-only GUI launcher returned process id zero",
            ),
        ));
    }
    let spawned_pid = spawned.pid();
    let spawned_start_token = spawned.process_start_token().to_string();

    let deadline = deadline(deps.clock.now(), timeout);
    loop {
        let current = match deps.heartbeats.live_instances(runtime_dir) {
            Ok(current) => current,
            Err(error) => return Err(cleanup_launched(spawned, error)),
        };
        let newly_live: Vec<_> = current
            .into_iter()
            .filter(|instance| !baseline_keys.contains(&instance_key(instance)))
            .collect();
        match newly_live.as_slice() {
            [] => {}
            [instance]
                if instance.gui_instance == requested_instance
                    && instance.pid == spawned_pid
                    && instance.process_start_token == spawned_start_token =>
            {
                return Ok(LaunchedGui {
                    committed: Some(CommittedGui {
                        instance: instance.clone(),
                        launcher_request_uid,
                        class,
                        launcher_witness: ColdLauncherWitness {
                            gui_instance: requested_instance,
                            launcher_request_uid,
                            process: launcher,
                            intent: intent.clone(),
                        },
                    }),
                    child: Some(spawned),
                });
            }
            [instance] => {
                return Err(cleanup_launched(
                    spawned,
                    TypedError::new(
                        ErrorCode::IdentityConflict,
                        format!(
                            "new GUI heartbeat {} pid {}/start-token does not match requested instance {} pid {}/start-token",
                            instance.gui_instance, instance.pid, requested_instance, spawned_pid
                        ),
                    ),
                ));
            }
            many => {
                return Err(cleanup_launched(
                    spawned,
                    TypedError::new(
                        ErrorCode::IdentityConflict,
                        format!(
                            "{} new GUI process instances appeared; refusing to guess which launch to use",
                            many.len()
                        ),
                    ),
                ));
            }
        }

        if deps.clock.now() >= deadline {
            return Err(cleanup_launched(
                spawned,
                TypedError::new(
                    ErrorCode::BridgeUnavailable,
                    format!(
                        "timed out after {} ms waiting for fresh GUI instance {requested_instance}",
                        timeout.as_millis()
                    ),
                ),
            ));
        }
        deps.clock
            .sleep(next_sleep(deps.clock.now(), deadline, POLL_INTERVAL));
    }
}

fn cleanup_launched(mut child: Box<dyn LifecycleChild>, mut error: TypedError) -> TypedError {
    let pid = child.pid();
    if let Err(cleanup) = child.terminate_and_reap() {
        error.message.push_str(&format!(
            "; additionally failed to terminate/reap launched GUI pid {}: {cleanup}",
            pid
        ));
    }
    error
}

fn validate_launch_inputs(
    runtime_dir: &Path,
    ready: &ReadyWezService,
    wezterm_bin: &str,
    gui_config: &Path,
    request_uid: Uuid,
) -> Result<(), TypedError> {
    if request_uid.is_nil()
        || !Path::new(wezterm_bin).is_absolute()
        || !runtime_dir.is_absolute()
        || !gui_config.is_absolute()
        || ready.socket.is_empty()
        || !Path::new(&ready.socket).is_absolute()
    {
        return Err(TypedError::new(
            ErrorCode::Usage,
            "attach-only GUI launch requires a non-nil request UID and absolute runtime/wezterm/config/socket paths",
        ));
    }
    Ok(())
}

fn validate_launcher_witness(witness: &LauncherProcessWitness) -> Result<(), TypedError> {
    if witness.pid == 0 || !is_canonical_native_start_token(&witness.start_token) {
        return Err(TypedError::new(
            ErrorCode::BridgeUnavailable,
            "cold GUI launcher has no canonical OS-native process witness",
        ));
    }
    Ok(())
}

fn validate_transport_target_relation(
    ready: &ReadyWezService,
    intent: &ColdLaunchIntent,
) -> Result<(), TypedError> {
    let local_domain = intent.domain() == "dmux";
    let local_backend = intent.backend_instance_uid() == ready.backend_instance_uid;
    if local_domain != local_backend
        || (local_domain && intent.server_epoch() != ready.server_epoch)
    {
        return Err(TypedError::new(
            ErrorCode::IdentityConflict,
            "cold GUI target contradicts the verified local dmux transport incarnation",
        ));
    }
    Ok(())
}

fn valid_domain(value: &str) -> bool {
    let mut bytes = value.bytes();
    !value.is_empty()
        && value.len() <= 128
        && bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && bytes
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b':' | b'-'))
}

#[cfg(target_os = "linux")]
fn is_canonical_native_start_token(value: &str) -> bool {
    value
        .strip_prefix("linux:")
        .and_then(|ticks| ticks.parse::<u64>().ok().map(|parsed| (ticks, parsed)))
        .is_some_and(|(ticks, parsed)| parsed > 0 && parsed.to_string() == ticks)
}

#[cfg(target_os = "macos")]
fn is_canonical_native_start_token(value: &str) -> bool {
    value.strip_prefix("macos:").is_some_and(|value| {
        let Some((seconds, micros)) = value.split_once(':') else {
            return false;
        };
        let Ok(seconds_value) = seconds.parse::<u64>() else {
            return false;
        };
        let Ok(micros_value) = micros.parse::<u32>() else {
            return false;
        };
        seconds_value > 0
            && micros_value <= 999_999
            && seconds_value.to_string() == seconds
            && micros_value.to_string() == micros
    })
}

fn attach_only_command(
    runtime_dir: &Path,
    ready: &ReadyWezService,
    wezterm_bin: &str,
    gui_config: &Path,
    class: &str,
    requested_instance: &str,
    launcher_request_uid: Uuid,
    launcher: &LauncherProcessWitness,
    intent: &ColdLaunchIntent,
) -> LifecycleCommand {
    let mut command = LifecycleCommand::new(
        wezterm_bin,
        [
            OsString::from("--config-file"),
            gui_config.as_os_str().to_owned(),
            OsString::from("start"),
            OsString::from("--no-auto-connect"),
            OsString::from("--class"),
            OsString::from(class),
            OsString::from("--always-new-process"),
            OsString::from("--domain"),
            OsString::from("dmux"),
            OsString::from("--attach"),
            OsString::from("--dmux-resident-broker"),
        ],
    );
    command.env_remove = SCRUBBED_ENV
        .iter()
        .chain(COLD_LAUNCH_WITNESS_ENV.iter())
        .map(OsString::from)
        .collect();
    command
        .env_set
        .insert(OsString::from(SOCKET_ENV), OsString::from(&ready.socket));
    command.env_set.insert(
        OsString::from("DMUX_RUNTIME_DIR"),
        runtime_dir.as_os_str().to_owned(),
    );
    command.env_set.insert(
        OsString::from(GUI_INSTANCE_ENV),
        OsString::from(requested_instance),
    );
    command.env_set.insert(
        OsString::from(GUI_LAUNCHER_REQUEST_UID_ENV),
        OsString::from(launcher_request_uid.to_string()),
    );
    command.env_set.insert(
        OsString::from(GUI_LAUNCHER_PID_ENV),
        OsString::from(launcher.pid.to_string()),
    );
    command.env_set.insert(
        OsString::from(GUI_LAUNCHER_START_TOKEN_ENV),
        OsString::from(&launcher.start_token),
    );
    command.env_set.insert(
        OsString::from(GUI_BACKEND_INSTANCE_ENV),
        OsString::from(ready.backend_instance_uid.0.to_string()),
    );
    command.env_set.insert(
        OsString::from(GUI_TARGET_HOST_UID_ENV),
        OsString::from(intent.owner().0.to_string()),
    );
    command.env_set.insert(
        OsString::from(GUI_TARGET_DOMAIN_ENV),
        OsString::from(intent.domain()),
    );
    command.env_set.insert(
        OsString::from(GUI_TARGET_BACKEND_INSTANCE_ENV),
        OsString::from(intent.backend_instance_uid().0.to_string()),
    );
    command.env_set.insert(
        OsString::from(GUI_TARGET_SERVER_EPOCH_ENV),
        OsString::from(intent.server_epoch().0.to_string()),
    );
    if let Some(space_uid) = intent.space_uid() {
        command.env_set.insert(
            OsString::from(GUI_TARGET_SPACE_UID_ENV),
            OsString::from(space_uid.0.to_string()),
        );
    }
    command
        .env_set
        .insert(OsString::from("DMUX_WEZ_FIRST"), OsString::from("1"));
    command
}

fn instance_key(instance: &BridgeInstanceSelection) -> (String, u32, String) {
    (
        instance.gui_instance.clone(),
        instance.pid,
        instance.process_start_token.clone(),
    )
}

fn deadline(now: Duration, timeout: Duration) -> Duration {
    now.checked_add(timeout).unwrap_or(Duration::MAX)
}

fn next_sleep(now: Duration, deadline: Duration, interval: Duration) -> Duration {
    deadline.saturating_sub(now).min(interval)
}
