use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsString;
use std::fs;
use std::io;
use std::num::NonZeroU64;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dmux::backend::{
    InventoryOutcome, InventoryScope, NativeGroupRow, NativeInventory, NativeSpaceRow,
    NativeSplitRow,
};
use dmux::connect_cli::{FrozenBinding, FrozenConnectTarget};
use dmux::error::{ErrorCode, TypedError};
use dmux::gui::{BridgeHeartbeat, BridgeInstanceSelection};
use dmux::gui_lifecycle::{
    ColdLaunchIntent, CommandExit, DescriptorSource, FixedServicePlatform,
    GUI_BACKEND_INSTANCE_ENV, GUI_INSTANCE_ENV, GUI_LAUNCHER_PID_ENV, GUI_LAUNCHER_REQUEST_UID_ENV,
    GUI_LAUNCHER_START_TOKEN_ENV, GUI_TARGET_BACKEND_INSTANCE_ENV, GUI_TARGET_DOMAIN_ENV,
    GUI_TARGET_HOST_UID_ENV, GUI_TARGET_SERVER_EPOCH_ENV, GUI_TARGET_SPACE_UID_ENV, GuiLaunchDeps,
    HeartbeatSource, LINUX_SERVICE_LABEL, LauncherProcessWitness, LauncherWitnessSource,
    LifecycleChild, LifecycleClock, LifecycleCommand, LifecycleCommandRunner, MACOS_SERVICE_LABEL,
    ReadyWezService, RuntimeHeartbeatSource, ServiceEnsureDeps, WezServiceInventory,
    ensure_ready_wez_service_with, launch_attach_only_gui_with,
};
use dmux::model::{
    Backend, BackendInstanceUid, HostUid, ProviderHandle, ServerEpoch, SpaceNo, SpaceUid,
};
use dmux::new_cli::{NewPresentationMode, WezPresentationPreflight};
use dmux::registry::{Registry, RegistryConfig};
use dmux::runtime::WezMuxDescriptor;
use tempfile::TempDir;
use uuid::Uuid;

const SOCKET: &str = "/run/user/1000/dmux/wez-dmux.sock";
const GUI_CFG: &str = "/cfg/wezterm.lua";
const LAUNCHER_UID: u64 = 1000;
const LAUNCHER_PID: u32 = 5151;
const TARGET_DOMAIN: &str = "dmux-b-ts";

#[cfg(target_os = "linux")]
const LAUNCHER_START_TOKEN: &str = "linux:123456";
#[cfg(target_os = "macos")]
const LAUNCHER_START_TOKEN: &str = "macos:1700000000:123456";

#[cfg(target_os = "linux")]
const SERVER_START_TOKEN: &str = "linux:424242";
#[cfg(target_os = "macos")]
const SERVER_START_TOKEN: &str = "macos:1700000001:424242";

#[cfg(target_os = "linux")]
const BOOT_ID: &str = "linux:bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
#[cfg(target_os = "macos")]
const BOOT_ID: &str = "macos:1699999999:999999";

#[derive(Debug, Clone)]
enum RunResult {
    Exit(CommandExit),
    Error(io::ErrorKind, &'static str),
}

#[derive(Debug)]
struct FakeCommands {
    run: RunResult,
    spawn: Result<u32, (io::ErrorKind, &'static str)>,
    cleanup_error: Option<(io::ErrorKind, &'static str)>,
    cleanups: Rc<Cell<u32>>,
    cleanup_pids: Rc<RefCell<Vec<u32>>>,
    run_commands: RefCell<Vec<(LifecycleCommand, Duration)>>,
    spawn_commands: RefCell<Vec<LifecycleCommand>>,
}

impl FakeCommands {
    fn successful(pid: u32) -> Self {
        Self {
            run: RunResult::Exit(CommandExit {
                success: true,
                code: Some(0),
            }),
            spawn: Ok(pid),
            cleanup_error: None,
            cleanups: Rc::new(Cell::new(0)),
            cleanup_pids: Rc::new(RefCell::new(Vec::new())),
            run_commands: RefCell::new(Vec::new()),
            spawn_commands: RefCell::new(Vec::new()),
        }
    }
}

impl LifecycleCommandRunner for FakeCommands {
    fn run_bounded(
        &self,
        command: &LifecycleCommand,
        timeout: Duration,
    ) -> io::Result<CommandExit> {
        self.run_commands
            .borrow_mut()
            .push((command.clone(), timeout));
        match self.run {
            RunResult::Exit(exit) => Ok(exit),
            RunResult::Error(kind, detail) => Err(io::Error::new(kind, detail)),
        }
    }

    fn spawn(&self, command: &LifecycleCommand) -> io::Result<Box<dyn LifecycleChild>> {
        self.spawn_commands.borrow_mut().push(command.clone());
        match self.spawn {
            Ok(pid) => Ok(Box::new(FakeChild {
                pid,
                cleanup_error: self.cleanup_error,
                cleanups: Rc::clone(&self.cleanups),
                cleanup_pids: Rc::clone(&self.cleanup_pids),
            })),
            Err((kind, detail)) => Err(io::Error::new(kind, detail)),
        }
    }
}

struct FakeChild {
    pid: u32,
    cleanup_error: Option<(io::ErrorKind, &'static str)>,
    cleanups: Rc<Cell<u32>>,
    cleanup_pids: Rc<RefCell<Vec<u32>>>,
}

impl LifecycleChild for FakeChild {
    fn pid(&self) -> u32 {
        self.pid
    }

    fn process_start_token(&self) -> &str {
        "spawn-token"
    }

    fn terminate_and_reap(&mut self) -> io::Result<()> {
        self.cleanups.set(self.cleanups.get() + 1);
        self.cleanup_pids.borrow_mut().push(self.pid);
        match self.cleanup_error {
            Some((kind, detail)) => Err(io::Error::new(kind, detail)),
            None => Ok(()),
        }
    }
}

#[derive(Debug, Clone)]
enum LauncherResult {
    Witness(LauncherProcessWitness),
    Error(io::ErrorKind, &'static str),
}

#[derive(Debug, Clone)]
struct FakeLauncher(LauncherResult);

impl FakeLauncher {
    fn successful() -> Self {
        Self(LauncherResult::Witness(LauncherProcessWitness {
            uid: LAUNCHER_UID,
            pid: LAUNCHER_PID,
            start_token: LAUNCHER_START_TOKEN.into(),
        }))
    }
}

impl LauncherWitnessSource for FakeLauncher {
    fn current(&self) -> io::Result<LauncherProcessWitness> {
        match &self.0 {
            LauncherResult::Witness(witness) => Ok(witness.clone()),
            LauncherResult::Error(kind, detail) => Err(io::Error::new(*kind, *detail)),
        }
    }
}

#[derive(Debug, Default)]
struct FakeClock(Cell<Duration>);

impl LifecycleClock for FakeClock {
    fn now(&self) -> Duration {
        self.0.get()
    }

    fn sleep(&self, duration: Duration) {
        self.0.set(self.0.get().saturating_add(duration));
    }
}

#[derive(Debug, Clone)]
enum DescriptorStep {
    Missing,
    Document(WezMuxDescriptor),
    Error(io::ErrorKind, &'static str),
}

#[derive(Debug)]
struct FakeDescriptors {
    steps: RefCell<VecDeque<DescriptorStep>>,
    verified_reads: Cell<u32>,
}

impl FakeDescriptors {
    fn new(steps: impl IntoIterator<Item = DescriptorStep>) -> Self {
        Self {
            steps: RefCell::new(steps.into_iter().collect()),
            verified_reads: Cell::new(0),
        }
    }
}

impl DescriptorSource for FakeDescriptors {
    fn read(&self, _runtime_dir: &Path) -> io::Result<Option<WezMuxDescriptor>> {
        let mut steps = self.steps.borrow_mut();
        let step = if steps.len() > 1 {
            steps.pop_front().expect("nonempty descriptor script")
        } else {
            steps.front().expect("nonempty descriptor script").clone()
        };
        match step {
            DescriptorStep::Missing => Ok(None),
            DescriptorStep::Document(document) => Ok(Some(document)),
            DescriptorStep::Error(kind, detail) => Err(io::Error::new(kind, detail)),
        }
    }

    fn read_verified_ready(
        &self,
        runtime_dir: &Path,
        expected_instance: Uuid,
        expected_epoch: Uuid,
    ) -> io::Result<Option<WezMuxDescriptor>> {
        self.verified_reads.set(self.verified_reads.get() + 1);
        let descriptor = self.read(runtime_dir)?;
        if let Some(descriptor) = &descriptor
            && (descriptor.backend_instance_uid.as_deref()
                != Some(expected_instance.to_string().as_str())
                || descriptor.epoch != expected_epoch.to_string())
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "verified descriptor identity changed",
            ));
        }
        Ok(descriptor)
    }
}

#[derive(Debug)]
struct FakeInventory {
    outcomes: RefCell<VecDeque<InventoryOutcome>>,
    calls: RefCell<Vec<(InventoryScope, u32, String)>>,
}

impl FakeInventory {
    fn new(outcomes: impl IntoIterator<Item = InventoryOutcome>) -> Self {
        Self {
            outcomes: RefCell::new(outcomes.into_iter().collect()),
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl WezServiceInventory for FakeInventory {
    fn inventory(&self, scope: &InventoryScope, descriptor: &WezMuxDescriptor) -> InventoryOutcome {
        self.calls.borrow_mut().push((
            scope.clone(),
            descriptor.pid,
            descriptor.start_token.clone(),
        ));
        let mut outcomes = self.outcomes.borrow_mut();
        if outcomes.len() > 1 {
            outcomes.pop_front().expect("nonempty inventory script")
        } else {
            outcomes.front().expect("nonempty inventory script").clone()
        }
    }
}

#[derive(Debug)]
struct FakeHeartbeats(RefCell<VecDeque<Result<Vec<BridgeInstanceSelection>, TypedError>>>);

impl FakeHeartbeats {
    fn new(
        steps: impl IntoIterator<Item = Result<Vec<BridgeInstanceSelection>, TypedError>>,
    ) -> Self {
        Self(RefCell::new(steps.into_iter().collect()))
    }
}

impl HeartbeatSource for FakeHeartbeats {
    fn live_instances(
        &self,
        _runtime_dir: &Path,
    ) -> Result<Vec<BridgeInstanceSelection>, TypedError> {
        let mut steps = self.0.borrow_mut();
        if steps.len() > 1 {
            steps.pop_front().expect("nonempty heartbeat script")
        } else {
            steps.front().expect("nonempty heartbeat script").clone()
        }
    }
}

struct RegistryFixture {
    _dir: TempDir,
    registry: Registry,
    instance: BackendInstanceUid,
    epoch: ServerEpoch,
    platform: FixedServicePlatform,
}

fn registry_fixture(platform: FixedServicePlatform) -> RegistryFixture {
    let dir = tempfile::tempdir().unwrap();
    let mut registry = Registry::open(RegistryConfig::new(
        dir.path().join("registry.sqlite3"),
        dir.path().join("locks"),
    ))
    .unwrap();
    let instance = registry
        .register_backend_instance(Backend::Wez, Some(SOCKET), Some(platform.service_label()))
        .unwrap();
    let epoch = ServerEpoch(Uuid::from_u128(0x11111111_1111_4111_8111_111111111111));
    registry
        .publish_backend_server(
            instance,
            epoch,
            Some(4242),
            Some(SERVER_START_TOKEN),
            Some(42),
            Some(84),
        )
        .unwrap();
    RegistryFixture {
        _dir: dir,
        registry,
        instance,
        epoch,
        platform,
    }
}

fn descriptor(fixture: &RegistryFixture) -> WezMuxDescriptor {
    WezMuxDescriptor {
        descriptor_version: 1,
        state: "ready".into(),
        epoch: fixture.epoch.0.to_string(),
        pid: 4242,
        socket: SOCKET.into(),
        start_token: SERVER_START_TOKEN.into(),
        boot_id: Some(BOOT_ID.into()),
        socket_dev: Some(42),
        socket_ino: Some(84),
        boot_nonce: Some("cccccccc-cccc-4ccc-8ccc-cccccccccccc".into()),
        backend_instance_uid: Some(fixture.instance.0.to_string()),
        recovery_generation: None,
        sentinel_window_id: Some(0),
        sentinel_tab_id: Some(0),
        sentinel_pane_id: Some(0),
        sentinel_fallback: Some(false),
        recovery_manifest_id: None,
        written_by: Some("mux-startup".into()),
        written_at: Some("2026-08-17T12:34:56Z".into()),
        error: None,
    }
}

fn complete(epoch: ServerEpoch, user_rows: bool) -> InventoryOutcome {
    let rows = if user_rows {
        vec![NativeSpaceRow {
            native_token: "dmux:host:space".into(),
            native_name: "dmux:host:space".into(),
            groups: vec![NativeGroupRow {
                handle: ProviderHandle::Wz(20),
                title: Some("group".into()),
                splits: vec![NativeSplitRow {
                    handle: ProviderHandle::Wz(21),
                    title: Some("split".into()),
                    cwd: Some("/work".into()),
                }],
            }],
            multi_window: false,
        }]
    } else {
        Vec::new()
    };
    InventoryOutcome::Complete(NativeInventory {
        server_epoch: Some(epoch),
        rows,
    })
}

fn service_deps<'a>(
    commands: &'a FakeCommands,
    descriptors: &'a FakeDescriptors,
    inventory: &'a FakeInventory,
    clock: &'a FakeClock,
) -> ServiceEnsureDeps<'a> {
    ServiceEnsureDeps {
        command: commands,
        descriptor: descriptors,
        inventory,
        clock,
    }
}

fn ready(fixture: &RegistryFixture) -> ReadyWezService {
    ReadyWezService {
        socket: SOCKET.into(),
        backend_instance_uid: fixture.instance,
        server_epoch: fixture.epoch,
    }
}

fn target_preflight(launcher_request_uid: Uuid) -> WezPresentationPreflight {
    WezPresentationPreflight {
        owner: HostUid(Uuid::from_u128(0x22222222_2222_4222_8222_222222222222)),
        backend_instance_uid: BackendInstanceUid(Uuid::from_u128(
            0xaaaaaaaa_aaaa_4aaa_8aaa_aaaaaaaaaaaa,
        )),
        server_epoch: ServerEpoch(Uuid::from_u128(0xbbbbbbbb_bbbb_4bbb_8bbb_bbbbbbbbbbbb)),
        gui_instance: format!("gui-{}", launcher_request_uid.simple()),
        domain: TARGET_DOMAIN.into(),
        alternate_domains: vec!["dmux-b-usb".into()],
        mode: NewPresentationMode::Cold,
    }
}

fn frozen_target(preflight: &WezPresentationPreflight) -> FrozenConnectTarget {
    FrozenConnectTarget {
        owner: preflight.owner,
        space_uid: SpaceUid(Uuid::from_u128(0xcccccccc_cccc_4ccc_8ccc_cccccccccccc)),
        space_no: SpaceNo(NonZeroU64::new(7).unwrap()),
        logical_name: "remote-space".into(),
        backend: Backend::Wez,
        backend_instance_uid: preflight.backend_instance_uid,
        server_epoch: preflight.server_epoch,
        binding: FrozenBinding {
            native_token: "dmux:remote:space".into(),
            endpoint: "/run/user/1000/dmux/remote.sock".into(),
        },
        child: None,
    }
}

fn existing_cold_intent(launcher_request_uid: Uuid) -> ColdLaunchIntent {
    let preflight = target_preflight(launcher_request_uid);
    ColdLaunchIntent::from_existing_target(
        &preflight,
        &frozen_target(&preflight),
        launcher_request_uid,
    )
    .unwrap()
}

fn new_cold_intent(launcher_request_uid: Uuid) -> ColdLaunchIntent {
    ColdLaunchIntent::from_new_preflight(
        &target_preflight(launcher_request_uid),
        launcher_request_uid,
    )
    .unwrap()
}

fn local_cold_intent(fixture: &RegistryFixture, launcher_request_uid: Uuid) -> ColdLaunchIntent {
    let preflight = WezPresentationPreflight {
        owner: HostUid(Uuid::from_u128(0x11111111_1111_4111_8111_111111111111)),
        backend_instance_uid: fixture.instance,
        server_epoch: fixture.epoch,
        gui_instance: format!("gui-{}", launcher_request_uid.simple()),
        domain: "dmux".into(),
        alternate_domains: Vec::new(),
        mode: NewPresentationMode::Cold,
    };
    ColdLaunchIntent::from_existing_target(
        &preflight,
        &frozen_target(&preflight),
        launcher_request_uid,
    )
    .unwrap()
}

fn heartbeat(
    name: impl Into<String>,
    pid: u32,
    token: impl Into<String>,
) -> BridgeInstanceSelection {
    BridgeInstanceSelection {
        gui_instance: name.into(),
        pid,
        process_start_token: token.into(),
        domains: BTreeMap::new(),
    }
}

#[test]
fn fixed_service_commands_accept_no_arbitrary_label() {
    let mac = FixedServicePlatform::MacOs { uid: 501 }.start_command();
    assert_eq!(mac.program, "/bin/launchctl");
    assert_eq!(
        mac.args,
        [
            OsString::from("kickstart"),
            OsString::from(format!("gui/501/{MACOS_SERVICE_LABEL}")),
        ]
    );
    assert!(mac.env_remove.is_empty());
    assert!(mac.env_set.is_empty());

    let linux = FixedServicePlatform::Linux.start_command();
    assert_eq!(linux.program, "/usr/bin/systemctl");
    assert_eq!(
        linux.args,
        [
            OsString::from("--user"),
            OsString::from("start"),
            OsString::from(LINUX_SERVICE_LABEL),
        ]
    );
}

#[test]
fn service_start_failure_prevents_descriptor_or_provider_use() {
    let fixture = registry_fixture(FixedServicePlatform::Linux);
    let commands = FakeCommands {
        run: RunResult::Exit(CommandExit {
            success: false,
            code: Some(5),
        }),
        ..FakeCommands::successful(99)
    };
    let descriptors = FakeDescriptors::new([DescriptorStep::Document(descriptor(&fixture))]);
    let inventory = FakeInventory::new([complete(fixture.epoch, false)]);
    let clock = FakeClock::default();
    let error = ensure_ready_wez_service_with(
        &fixture.registry,
        fixture._dir.path(),
        fixture.platform,
        service_deps(&commands, &descriptors, &inventory, &clock),
        Duration::from_millis(50),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::ProviderUnavailable);
    assert!(error.message.contains("exit 5"), "{}", error.message);
    assert!(inventory.calls.borrow().is_empty());
}

#[test]
fn service_command_io_failure_is_typed_and_bounded() {
    let fixture = registry_fixture(FixedServicePlatform::Linux);
    let commands = FakeCommands {
        run: RunResult::Error(io::ErrorKind::TimedOut, "manager hung"),
        ..FakeCommands::successful(99)
    };
    let descriptors = FakeDescriptors::new([DescriptorStep::Missing]);
    let inventory = FakeInventory::new([complete(fixture.epoch, false)]);
    let clock = FakeClock::default();
    let error = ensure_ready_wez_service_with(
        &fixture.registry,
        fixture._dir.path(),
        fixture.platform,
        service_deps(&commands, &descriptors, &inventory, &clock),
        Duration::from_millis(50),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::ProviderUnavailable);
    assert!(error.message.contains("manager hung"), "{}", error.message);
    let calls = commands.run_commands.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, FixedServicePlatform::Linux.start_command());
    assert_eq!(calls[0].1, Duration::from_secs(10));
}

#[test]
fn starting_descriptor_polls_then_accepts_nonempty_sentinel_verified_inventory() {
    let fixture = registry_fixture(FixedServicePlatform::Linux);
    let commands = FakeCommands::successful(99);
    let mut starting = descriptor(&fixture);
    starting.state = "starting".into();
    let descriptors = FakeDescriptors::new([
        DescriptorStep::Missing,
        DescriptorStep::Document(starting),
        DescriptorStep::Document(descriptor(&fixture)),
    ]);
    let inventory = FakeInventory::new([complete(fixture.epoch, true)]);
    let clock = FakeClock::default();
    let result = ensure_ready_wez_service_with(
        &fixture.registry,
        fixture._dir.path(),
        fixture.platform,
        service_deps(&commands, &descriptors, &inventory, &clock),
        Duration::from_millis(100),
    )
    .unwrap();
    assert_eq!(result, ready(&fixture));
    assert_eq!(
        descriptors.verified_reads.get(),
        1,
        "raw ready descriptor must be re-read through the native verifier"
    );
    let calls = inventory.calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0],
        (
            InventoryScope::managed(Backend::Wez, SOCKET, fixture.epoch),
            4242,
            SERVER_START_TOKEN.into(),
        )
    );
}

#[test]
fn wrong_descriptor_socket_fails_before_inventory() {
    let fixture = registry_fixture(FixedServicePlatform::Linux);
    let commands = FakeCommands::successful(99);
    let mut wrong = descriptor(&fixture);
    wrong.socket = "/run/user/1000/dmux/imposter.sock".into();
    let descriptors = FakeDescriptors::new([DescriptorStep::Document(wrong)]);
    let inventory = FakeInventory::new([complete(fixture.epoch, false)]);
    let clock = FakeClock::default();
    let error = ensure_ready_wez_service_with(
        &fixture.registry,
        fixture._dir.path(),
        fixture.platform,
        service_deps(&commands, &descriptors, &inventory, &clock),
        Duration::from_millis(50),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::WrongBackendInstance);
    assert!(inventory.calls.borrow().is_empty());
}

#[test]
fn ready_descriptor_identity_change_fails_before_inventory() {
    let fixture = registry_fixture(FixedServicePlatform::Linux);
    let commands = FakeCommands::successful(99);
    let raw = descriptor(&fixture);
    let mut replaced = raw.clone();
    replaced.backend_instance_uid =
        Some(Uuid::from_u128(0xeeeeeeee_eeee_4eee_8eee_eeeeeeeeeeee).to_string());
    let descriptors = FakeDescriptors::new([
        DescriptorStep::Document(raw),
        DescriptorStep::Document(replaced),
    ]);
    let inventory = FakeInventory::new([complete(fixture.epoch, false)]);
    let clock = FakeClock::default();
    let error = ensure_ready_wez_service_with(
        &fixture.registry,
        fixture._dir.path(),
        fixture.platform,
        service_deps(&commands, &descriptors, &inventory, &clock),
        Duration::from_millis(50),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BridgeUnavailable);
    assert!(
        error.message.contains("identity changed"),
        "{}",
        error.message
    );
    assert_eq!(descriptors.verified_reads.get(), 1);
    assert!(inventory.calls.borrow().is_empty());
}

#[test]
fn registry_and_inventory_use_the_verified_reread_not_the_raw_ready_claim() {
    let fixture = registry_fixture(FixedServicePlatform::Linux);
    let commands = FakeCommands::successful(99);
    let raw = descriptor(&fixture);
    let mut verified = raw.clone();
    verified.pid = 4243;
    let descriptors = FakeDescriptors::new([
        DescriptorStep::Document(raw),
        DescriptorStep::Document(verified),
    ]);
    let inventory = FakeInventory::new([complete(fixture.epoch, false)]);
    let clock = FakeClock::default();
    let error = ensure_ready_wez_service_with(
        &fixture.registry,
        fixture._dir.path(),
        fixture.platform,
        service_deps(&commands, &descriptors, &inventory, &clock),
        Duration::from_millis(50),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::WrongBackendInstance);
    assert!(error.message.contains("process/socket witness"));
    assert_eq!(descriptors.verified_reads.get(), 1);
    assert!(inventory.calls.borrow().is_empty());
}

#[test]
fn registry_socket_inode_must_match_the_native_verified_descriptor() {
    let mut fixture = registry_fixture(FixedServicePlatform::Linux);
    fixture
        .registry
        .publish_backend_server(
            fixture.instance,
            fixture.epoch,
            Some(4242),
            Some(SERVER_START_TOKEN),
            Some(43),
            Some(84),
        )
        .unwrap();
    let commands = FakeCommands::successful(99);
    let descriptors = FakeDescriptors::new([DescriptorStep::Document(descriptor(&fixture))]);
    let inventory = FakeInventory::new([complete(fixture.epoch, false)]);
    let clock = FakeClock::default();
    let error = ensure_ready_wez_service_with(
        &fixture.registry,
        fixture._dir.path(),
        fixture.platform,
        service_deps(&commands, &descriptors, &inventory, &clock),
        Duration::from_millis(50),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::WrongBackendInstance);
    assert!(
        error.message.contains("process/socket witness"),
        "{}",
        error.message
    );
    assert_eq!(descriptors.verified_reads.get(), 1);
    assert!(inventory.calls.borrow().is_empty());
}

/// A descriptor that is already `ready` and disagrees with the registry is
/// settled, not pending: the validator holds `&Registry` and nothing on this
/// path republishes, so every later poll compares the same two values and
/// reaches the same verdict. It must never retarget onto the descriptor's
/// epoch — and it must not spend the whole deadline proving that, which is
/// what a `BackendEpochChanged` classified as retryable used to do.
#[test]
fn stale_descriptor_epoch_never_retargets_and_fails_fast() {
    let fixture = registry_fixture(FixedServicePlatform::Linux);
    let commands = FakeCommands::successful(99);
    let mut stale = descriptor(&fixture);
    stale.epoch = Uuid::from_u128(0x22222222_2222_4222_8222_222222222222).to_string();
    let descriptors = FakeDescriptors::new([DescriptorStep::Document(stale)]);
    let inventory = FakeInventory::new([complete(fixture.epoch, false)]);
    let clock = FakeClock::default();
    let error = ensure_ready_wez_service_with(
        &fixture.registry,
        fixture._dir.path(),
        fixture.platform,
        service_deps(&commands, &descriptors, &inventory, &clock),
        Duration::from_millis(50),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BackendEpochChanged);
    assert!(
        !error.message.contains("timed out"),
        "waiting cannot change this answer, so no deadline is spent on it: {}",
        error.message
    );
    assert!(
        error.message.contains("registry records") && error.message.contains("Restart"),
        "the operator is told which side is stale and what fixes it: {}",
        error.message
    );
    assert_eq!(
        descriptors.verified_reads.get(),
        1,
        "the settled descriptor is read once, not polled"
    );
    assert_eq!(
        clock.0.get(),
        Duration::ZERO,
        "and no part of the readiness deadline is burned on it"
    );
    assert!(inventory.calls.borrow().is_empty());
}

#[test]
fn missing_sentinel_outcome_is_rejected() {
    let fixture = registry_fixture(FixedServicePlatform::Linux);
    let commands = FakeCommands::successful(99);
    let descriptors = FakeDescriptors::new([DescriptorStep::Document(descriptor(&fixture))]);
    let inventory = FakeInventory::new([InventoryOutcome::Malformed {
        detail: "sentinel missing".into(),
    }]);
    let clock = FakeClock::default();
    let error = ensure_ready_wez_service_with(
        &fixture.registry,
        fixture._dir.path(),
        fixture.platform,
        service_deps(&commands, &descriptors, &inventory, &clock),
        Duration::from_millis(50),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::PostconditionFailed);
    assert!(
        error.message.contains("sentinel missing"),
        "{}",
        error.message
    );
}

#[test]
fn private_descriptor_error_fails_closed() {
    let fixture = registry_fixture(FixedServicePlatform::Linux);
    let commands = FakeCommands::successful(99);
    let descriptors = FakeDescriptors::new([DescriptorStep::Error(
        io::ErrorKind::PermissionDenied,
        "mode 0644",
    )]);
    let inventory = FakeInventory::new([complete(fixture.epoch, false)]);
    let clock = FakeClock::default();
    let error = ensure_ready_wez_service_with(
        &fixture.registry,
        fixture._dir.path(),
        fixture.platform,
        service_deps(&commands, &descriptors, &inventory, &clock),
        Duration::from_millis(50),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BridgeUnavailable);
}

#[test]
fn unsupported_descriptor_version_fails_before_inventory() {
    let fixture = registry_fixture(FixedServicePlatform::Linux);
    let commands = FakeCommands::successful(99);
    let mut incompatible = descriptor(&fixture);
    incompatible.descriptor_version = 2;
    let descriptors = FakeDescriptors::new([DescriptorStep::Document(incompatible)]);
    let inventory = FakeInventory::new([complete(fixture.epoch, false)]);
    let clock = FakeClock::default();
    let error = ensure_ready_wez_service_with(
        &fixture.registry,
        fixture._dir.path(),
        fixture.platform,
        service_deps(&commands, &descriptors, &inventory, &clock),
        Duration::from_millis(50),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::ProtocolMismatch);
    assert!(inventory.calls.borrow().is_empty());
    assert_eq!(descriptors.verified_reads.get(), 0);
}

#[test]
fn attach_launcher_uses_only_the_frozen_argv_and_sanitized_endpoint_env() {
    let fixture = registry_fixture(FixedServicePlatform::Linux);
    let request = Uuid::from_u128(0x33333333_3333_4333_8333_333333333333);
    let requested = format!("gui-{}", request.simple());
    let commands = FakeCommands::successful(7001);
    let old = heartbeat("gui-old", 6001, "old-token");
    let fresh = heartbeat(&requested, 7001, "spawn-token");
    let heartbeats = FakeHeartbeats::new([
        Ok(vec![old.clone()]),
        Ok(vec![old]),
        Ok(vec![fresh.clone()]),
    ]);
    let launcher = FakeLauncher::successful();
    let intent = existing_cold_intent(request);
    let clock = FakeClock::default();
    let launched = launch_attach_only_gui_with(
        Path::new("/run/user/1000/dmux"),
        &ready(&fixture),
        "/opt/wezterm/bin/wezterm",
        Path::new(GUI_CFG),
        request,
        &intent,
        GuiLaunchDeps {
            command: &commands,
            heartbeats: &heartbeats,
            launcher: &launcher,
            clock: &clock,
        },
        Duration::from_millis(100),
    )
    .unwrap();
    assert_eq!(launched.instance(), &fresh);
    assert_eq!(launched.class(), format!("dmux-{}", request.simple()));
    assert_eq!(
        launched.launcher_witness().gui_instance(),
        format!("gui-{}", request.simple())
    );
    assert_eq!(launched.launcher_witness().launcher_request_uid(), request);
    assert_eq!(
        launched.launcher_witness().process(),
        &LauncherProcessWitness {
            uid: LAUNCHER_UID,
            pid: LAUNCHER_PID,
            start_token: LAUNCHER_START_TOKEN.into(),
        }
    );
    assert_eq!(launched.launcher_witness().intent(), &intent);
    assert_eq!(commands.cleanups.get(), 0, "successful GUI stays running");

    let calls = commands.spawn_commands.borrow();
    assert_eq!(calls.len(), 1);
    let command = &calls[0];
    assert_eq!(command.program, "/opt/wezterm/bin/wezterm");
    assert_eq!(
        command.args,
        [
            "--config-file",
            GUI_CFG,
            "start",
            "--no-auto-connect",
            "--class",
            &format!("dmux-{}", request.simple()),
            "--always-new-process",
            "--domain",
            "dmux",
            "--attach",
            "--dmux-resident-broker",
        ]
        .map(OsString::from)
    );
    assert_eq!(
        command.env_remove,
        [
            "WEZTERM_PANE",
            "TMUX",
            "TMUX_PANE",
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
        ]
        .map(OsString::from)
    );
    assert_eq!(
        &command.env_set,
        &BTreeMap::from([
            (
                OsString::from("DMUX_RUNTIME_DIR"),
                OsString::from("/run/user/1000/dmux")
            ),
            (
                OsString::from(GUI_BACKEND_INSTANCE_ENV),
                OsString::from(fixture.instance.0.to_string()),
            ),
            (OsString::from(GUI_INSTANCE_ENV), OsString::from(requested)),
            (
                OsString::from(GUI_LAUNCHER_PID_ENV),
                OsString::from(LAUNCHER_PID.to_string()),
            ),
            (
                OsString::from(GUI_LAUNCHER_REQUEST_UID_ENV),
                OsString::from(request.to_string()),
            ),
            (
                OsString::from(GUI_LAUNCHER_START_TOKEN_ENV),
                OsString::from(LAUNCHER_START_TOKEN),
            ),
            (
                OsString::from(GUI_TARGET_BACKEND_INSTANCE_ENV),
                OsString::from(intent.backend_instance_uid().0.to_string()),
            ),
            (
                OsString::from(GUI_TARGET_DOMAIN_ENV),
                OsString::from(intent.domain()),
            ),
            (
                OsString::from(GUI_TARGET_HOST_UID_ENV),
                OsString::from(intent.owner().0.to_string()),
            ),
            (
                OsString::from(GUI_TARGET_SERVER_EPOCH_ENV),
                OsString::from(intent.server_epoch().0.to_string()),
            ),
            (
                OsString::from(GUI_TARGET_SPACE_UID_ENV),
                OsString::from(intent.space_uid().unwrap().0.to_string()),
            ),
            (OsString::from("DMUX_WEZ_FIRST"), OsString::from("1")),
            (
                OsString::from("WEZTERM_UNIX_SOCKET"),
                OsString::from(SOCKET)
            ),
        ])
    );
    assert_ne!(
        command.env_set[&OsString::from(GUI_BACKEND_INSTANCE_ENV)],
        command.env_set[&OsString::from(GUI_TARGET_BACKEND_INSTANCE_ENV)],
        "local transport identity must not be replaced by the remote target incarnation"
    );
    assert!(
        !command
            .args
            .windows(2)
            .any(|pair| pair == ["start", "--domain"])
    );
    let committed = launched.commit();
    assert_eq!(committed.instance, fresh);
    assert_eq!(committed.launcher_witness.intent(), &intent);
    assert_eq!(commands.cleanups.get(), 0, "commit disarms cleanup");
    assert!(commands.cleanup_pids.borrow().is_empty());
}

#[test]
fn local_cold_target_is_still_explicitly_bound_to_its_owner_and_incarnation() {
    let fixture = registry_fixture(FixedServicePlatform::Linux);
    let request = Uuid::from_u128(0x37373737_3737_4737_8737_373737373737);
    let requested = format!("gui-{}", request.simple());
    let intent = local_cold_intent(&fixture, request);
    let commands = FakeCommands::successful(7020);
    let heartbeats = FakeHeartbeats::new([
        Ok(vec![]),
        Ok(vec![heartbeat(&requested, 7020, "spawn-token")]),
    ]);
    let launcher = FakeLauncher::successful();
    let clock = FakeClock::default();
    let launched = launch_attach_only_gui_with(
        Path::new("/run/user/1000/dmux"),
        &ready(&fixture),
        "/opt/wezterm/bin/wezterm",
        Path::new(GUI_CFG),
        request,
        &intent,
        GuiLaunchDeps {
            command: &commands,
            heartbeats: &heartbeats,
            launcher: &launcher,
            clock: &clock,
        },
        Duration::from_millis(50),
    )
    .unwrap();
    let _committed = launched.commit();

    let calls = commands.spawn_commands.borrow();
    let env = &calls[0].env_set;
    assert_eq!(
        env.get(&OsString::from(GUI_TARGET_DOMAIN_ENV)),
        Some(&OsString::from("dmux"))
    );
    assert_eq!(
        env.get(&OsString::from(GUI_TARGET_HOST_UID_ENV)),
        Some(&OsString::from(intent.owner().0.to_string()))
    );
    assert_eq!(
        env.get(&OsString::from(GUI_TARGET_BACKEND_INSTANCE_ENV)),
        env.get(&OsString::from(GUI_BACKEND_INSTANCE_ENV))
    );
    assert_eq!(
        env.get(&OsString::from(GUI_TARGET_SERVER_EPOCH_ENV)),
        Some(&OsString::from(fixture.epoch.0.to_string()))
    );
    assert!(env.contains_key(&OsString::from(GUI_TARGET_SPACE_UID_ENV)));
}

#[test]
fn cold_launch_rejects_local_transport_and_target_cross_mix_before_spawn() {
    let fixture = registry_fixture(FixedServicePlatform::Linux);
    let commands = FakeCommands::successful(7021);
    let heartbeats = FakeHeartbeats::new([Ok(vec![])]);
    let launcher = FakeLauncher::successful();
    let clock = FakeClock::default();

    let request = Uuid::from_u128(0x38383838_3838_4838_8838_383838383838);
    let mut remote_backend_on_local_domain = target_preflight(request);
    remote_backend_on_local_domain.domain = "dmux".into();
    let target = frozen_target(&remote_backend_on_local_domain);
    let intent =
        ColdLaunchIntent::from_existing_target(&remote_backend_on_local_domain, &target, request)
            .unwrap();
    let error = launch_attach_only_gui_with(
        Path::new("/run/user/1000/dmux"),
        &ready(&fixture),
        "/opt/wezterm/bin/wezterm",
        Path::new(GUI_CFG),
        request,
        &intent,
        GuiLaunchDeps {
            command: &commands,
            heartbeats: &heartbeats,
            launcher: &launcher,
            clock: &clock,
        },
        Duration::from_millis(50),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::IdentityConflict);
    assert!(error.message.contains("local dmux transport incarnation"));

    let request = Uuid::from_u128(0x39393939_3939_4939_8939_393939393939);
    let local_backend_on_remote_domain = WezPresentationPreflight {
        owner: HostUid(Uuid::from_u128(0x11111111_1111_4111_8111_111111111111)),
        backend_instance_uid: fixture.instance,
        server_epoch: fixture.epoch,
        gui_instance: format!("gui-{}", request.simple()),
        domain: TARGET_DOMAIN.into(),
        alternate_domains: Vec::new(),
        mode: NewPresentationMode::Cold,
    };
    let target = frozen_target(&local_backend_on_remote_domain);
    let intent =
        ColdLaunchIntent::from_existing_target(&local_backend_on_remote_domain, &target, request)
            .unwrap();
    let error = launch_attach_only_gui_with(
        Path::new("/run/user/1000/dmux"),
        &ready(&fixture),
        "/opt/wezterm/bin/wezterm",
        Path::new(GUI_CFG),
        request,
        &intent,
        GuiLaunchDeps {
            command: &commands,
            heartbeats: &heartbeats,
            launcher: &launcher,
            clock: &clock,
        },
        Duration::from_millis(50),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::IdentityConflict);
    assert!(commands.spawn_commands.borrow().is_empty());
}

#[test]
fn cold_launch_intent_requires_one_matching_owner_validated_preflight() {
    let request = Uuid::from_u128(0x36363636_3636_4636_8636_363636363636);
    let preflight = target_preflight(request);
    let mut mismatched = frozen_target(&preflight);
    mismatched.backend_instance_uid =
        BackendInstanceUid(Uuid::from_u128(0xdddddddd_dddd_4ddd_8ddd_dddddddddddd));
    let error =
        ColdLaunchIntent::from_existing_target(&preflight, &mismatched, request).unwrap_err();
    assert_eq!(error.code, ErrorCode::IdentityConflict);

    let mut ambient = preflight.clone();
    ambient.mode = NewPresentationMode::Ambient;
    let error = ColdLaunchIntent::from_new_preflight(&ambient, request).unwrap_err();
    assert_eq!(error.code, ErrorCode::IdentityConflict);

    let mut malformed = preflight;
    malformed.domain = "dmux-b-ts\nforged".into();
    let error = ColdLaunchIntent::from_new_preflight(&malformed, request).unwrap_err();
    assert_eq!(error.code, ErrorCode::ProtocolMismatch);

    let mut wrong_instance = target_preflight(request);
    wrong_instance.gui_instance = "gui-for-another-request".into();
    let error = ColdLaunchIntent::from_new_preflight(&wrong_instance, request).unwrap_err();
    assert_eq!(error.code, ErrorCode::IdentityConflict);
}

#[test]
fn new_cold_launch_omits_only_the_not_yet_allocated_space_uid() {
    let fixture = registry_fixture(FixedServicePlatform::Linux);
    let request = Uuid::from_u128(0x35353535_3535_4535_8535_353535353535);
    let requested = format!("gui-{}", request.simple());
    let commands = FakeCommands::successful(7010);
    let heartbeats = FakeHeartbeats::new([
        Ok(vec![]),
        Ok(vec![heartbeat(&requested, 7010, "spawn-token")]),
    ]);
    let launcher = FakeLauncher::successful();
    let intent = new_cold_intent(request);
    let clock = FakeClock::default();
    let launched = launch_attach_only_gui_with(
        Path::new("/run/user/1000/dmux"),
        &ready(&fixture),
        "/opt/wezterm/bin/wezterm",
        Path::new(GUI_CFG),
        request,
        &intent,
        GuiLaunchDeps {
            command: &commands,
            heartbeats: &heartbeats,
            launcher: &launcher,
            clock: &clock,
        },
        Duration::from_millis(50),
    )
    .unwrap();
    assert_eq!(launched.launcher_witness().intent().space_uid(), None);

    let calls = commands.spawn_commands.borrow();
    let env = &calls[0].env_set;
    assert!(!env.contains_key(&OsString::from(GUI_TARGET_SPACE_UID_ENV)));
    assert!(
        calls[0]
            .env_remove
            .contains(&OsString::from(GUI_TARGET_SPACE_UID_ENV)),
        "a stale inherited target Space UID must be removed for new"
    );
    for required in [
        GUI_INSTANCE_ENV,
        GUI_LAUNCHER_REQUEST_UID_ENV,
        GUI_LAUNCHER_PID_ENV,
        GUI_LAUNCHER_START_TOKEN_ENV,
        GUI_BACKEND_INSTANCE_ENV,
        GUI_TARGET_HOST_UID_ENV,
        GUI_TARGET_DOMAIN_ENV,
        GUI_TARGET_BACKEND_INSTANCE_ENV,
        GUI_TARGET_SERVER_EPOCH_ENV,
    ] {
        assert!(
            env.contains_key(&OsString::from(required)),
            "missing {required}"
        );
    }
    let committed = launched.commit();
    assert_eq!(committed.launcher_witness.intent().space_uid(), None);
    assert_eq!(commands.cleanups.get(), 0, "commit disarms cleanup");
}

#[test]
fn dropping_uncommitted_launch_guard_terminates_and_reaps_the_exact_child() {
    let fixture = registry_fixture(FixedServicePlatform::Linux);
    let request = Uuid::from_u128(0x35353535_3535_4535_8535_353535353536);
    let requested = format!("gui-{}", request.simple());
    let commands = FakeCommands::successful(7030);
    let heartbeats = FakeHeartbeats::new([
        Ok(vec![]),
        Ok(vec![heartbeat(&requested, 7030, "spawn-token")]),
    ]);
    let launcher = FakeLauncher::successful();
    let intent = existing_cold_intent(request);
    let clock = FakeClock::default();
    let launched = launch_attach_only_gui_with(
        Path::new("/run/user/1000/dmux"),
        &ready(&fixture),
        "/opt/wezterm/bin/wezterm",
        Path::new(GUI_CFG),
        request,
        &intent,
        GuiLaunchDeps {
            command: &commands,
            heartbeats: &heartbeats,
            launcher: &launcher,
            clock: &clock,
        },
        Duration::from_millis(50),
    )
    .unwrap();
    assert_eq!(launched.instance().pid, 7030);
    assert_eq!(commands.cleanups.get(), 0);

    drop(launched);
    assert_eq!(commands.cleanups.get(), 1);
    assert_eq!(&*commands.cleanup_pids.borrow(), &[7030]);
}

#[test]
fn attach_fails_closed_before_spawn_without_a_canonical_launcher_witness() {
    let fixture = registry_fixture(FixedServicePlatform::Linux);
    let request = Uuid::from_u128(0x34343434_3434_4434_8434_343434343434);

    let commands = FakeCommands::successful(7000);
    let heartbeats = FakeHeartbeats::new([Ok(vec![])]);
    let launcher = FakeLauncher::successful();
    let error = launch_attach_only_gui_with(
        Path::new("/run/user/1000/dmux"),
        &ready(&fixture),
        "/opt/wezterm/bin/wezterm",
        Path::new(GUI_CFG),
        Uuid::from_u128(0x34343434_3434_4434_8434_343434343435),
        &existing_cold_intent(request),
        GuiLaunchDeps {
            command: &commands,
            heartbeats: &heartbeats,
            launcher: &launcher,
            clock: &FakeClock::default(),
        },
        Duration::from_millis(50),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::IdentityConflict);
    assert!(error.message.contains("deterministic launcher request"));
    assert!(commands.spawn_commands.borrow().is_empty());

    let commands = FakeCommands::successful(7001);
    let heartbeats = FakeHeartbeats::new([Ok(vec![])]);
    let unavailable = FakeLauncher(LauncherResult::Error(
        io::ErrorKind::PermissionDenied,
        "parent identity unavailable",
    ));
    let clock = FakeClock::default();
    let error = launch_attach_only_gui_with(
        Path::new("/run/user/1000/dmux"),
        &ready(&fixture),
        "/opt/wezterm/bin/wezterm",
        Path::new(GUI_CFG),
        request,
        &existing_cold_intent(request),
        GuiLaunchDeps {
            command: &commands,
            heartbeats: &heartbeats,
            launcher: &unavailable,
            clock: &clock,
        },
        Duration::from_millis(50),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BridgeUnavailable);
    assert!(
        error.message.contains("parent identity unavailable"),
        "{}",
        error.message
    );
    assert!(commands.spawn_commands.borrow().is_empty());

    let commands = FakeCommands::successful(7002);
    let noncanonical = FakeLauncher(LauncherResult::Witness(LauncherProcessWitness {
        uid: LAUNCHER_UID,
        pid: LAUNCHER_PID,
        start_token: "not-a-native-start-token".into(),
    }));
    let error = launch_attach_only_gui_with(
        Path::new("/run/user/1000/dmux"),
        &ready(&fixture),
        "/opt/wezterm/bin/wezterm",
        Path::new(GUI_CFG),
        request,
        &existing_cold_intent(request),
        GuiLaunchDeps {
            command: &commands,
            heartbeats: &heartbeats,
            launcher: &noncanonical,
            clock: &clock,
        },
        Duration::from_millis(50),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BridgeUnavailable);
    assert!(
        error
            .message
            .contains("canonical OS-native process witness")
    );
    assert!(commands.spawn_commands.borrow().is_empty());
}

#[test]
fn attach_timeout_never_reuses_an_old_heartbeat() {
    let fixture = registry_fixture(FixedServicePlatform::Linux);
    let request = Uuid::from_u128(0x44444444_4444_4444_8444_444444444444);
    let commands = FakeCommands::successful(7001);
    let old = heartbeat("gui-old", 6001, "old-token");
    let heartbeats = FakeHeartbeats::new([Ok(vec![old])]);
    let launcher = FakeLauncher::successful();
    let clock = FakeClock::default();
    let error = launch_attach_only_gui_with(
        Path::new("/run/user/1000/dmux"),
        &ready(&fixture),
        "/opt/wezterm/bin/wezterm",
        Path::new(GUI_CFG),
        request,
        &existing_cold_intent(request),
        GuiLaunchDeps {
            command: &commands,
            heartbeats: &heartbeats,
            launcher: &launcher,
            clock: &clock,
        },
        Duration::from_millis(50),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BridgeUnavailable);
    assert!(error.message.contains("timed out"), "{}", error.message);
    assert_eq!(commands.cleanups.get(), 1, "timed-out GUI is killed/reaped");
}

#[test]
fn zero_pid_child_is_terminated_and_reaped_before_correlation() {
    let fixture = registry_fixture(FixedServicePlatform::Linux);
    let request = Uuid::from_u128(0x43434343_4343_4343_8343_434343434343);
    let commands = FakeCommands::successful(0);
    let heartbeats = FakeHeartbeats::new([Ok(vec![])]);
    let launcher = FakeLauncher::successful();
    let clock = FakeClock::default();
    let error = launch_attach_only_gui_with(
        Path::new("/run/user/1000/dmux"),
        &ready(&fixture),
        "/opt/wezterm/bin/wezterm",
        Path::new(GUI_CFG),
        request,
        &existing_cold_intent(request),
        GuiLaunchDeps {
            command: &commands,
            heartbeats: &heartbeats,
            launcher: &launcher,
            clock: &clock,
        },
        Duration::from_millis(50),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BridgeUnavailable);
    assert!(
        error.message.contains("process id zero"),
        "{}",
        error.message
    );
    assert_eq!(commands.cleanups.get(), 1);
}

#[test]
fn attach_failure_preserves_terminate_and_reap_error_detail() {
    let fixture = registry_fixture(FixedServicePlatform::Linux);
    let request = Uuid::from_u128(0x45454545_4545_4545_8545_454545454545);
    let commands = FakeCommands {
        cleanup_error: Some((io::ErrorKind::PermissionDenied, "kill denied")),
        ..FakeCommands::successful(7001)
    };
    let heartbeats = FakeHeartbeats::new([Ok(vec![])]);
    let launcher = FakeLauncher::successful();
    let clock = FakeClock::default();
    let error = launch_attach_only_gui_with(
        Path::new("/run/user/1000/dmux"),
        &ready(&fixture),
        "/opt/wezterm/bin/wezterm",
        Path::new(GUI_CFG),
        request,
        &existing_cold_intent(request),
        GuiLaunchDeps {
            command: &commands,
            heartbeats: &heartbeats,
            launcher: &launcher,
            clock: &clock,
        },
        Duration::ZERO,
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BridgeUnavailable);
    assert!(error.message.contains("timed out"), "{}", error.message);
    assert!(
        error
            .message
            .contains("additionally failed to terminate/reap launched GUI pid 7001: kill denied"),
        "{}",
        error.message
    );
    assert_eq!(commands.cleanups.get(), 1);
}

#[test]
fn attach_rejects_wrong_pid_and_multiple_new_process_instances() {
    let fixture = registry_fixture(FixedServicePlatform::Linux);
    let launcher = FakeLauncher::successful();
    let request = Uuid::from_u128(0x55555555_5555_4555_8555_555555555555);
    let requested = format!("gui-{}", request.simple());
    let commands = FakeCommands::successful(7001);
    let wrong_pid = FakeHeartbeats::new([
        Ok(vec![]),
        Ok(vec![heartbeat(&requested, 7002, "wrong-pid")]),
    ]);
    let clock = FakeClock::default();
    let error = launch_attach_only_gui_with(
        Path::new("/run/user/1000/dmux"),
        &ready(&fixture),
        "/opt/wezterm/bin/wezterm",
        Path::new(GUI_CFG),
        request,
        &existing_cold_intent(request),
        GuiLaunchDeps {
            command: &commands,
            heartbeats: &wrong_pid,
            launcher: &launcher,
            clock: &clock,
        },
        Duration::from_millis(50),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::IdentityConflict);
    assert!(error.message.contains("pid 7002"), "{}", error.message);
    assert_eq!(commands.cleanups.get(), 1, "wrong-pid GUI is killed/reaped");

    let request = Uuid::from_u128(0x56565656_5656_4656_8656_565656565656);
    let requested = format!("gui-{}", request.simple());
    let commands = FakeCommands::successful(7050);
    let wrong_token = FakeHeartbeats::new([
        Ok(vec![]),
        Ok(vec![heartbeat(&requested, 7050, "different-start")]),
    ]);
    let clock = FakeClock::default();
    let error = launch_attach_only_gui_with(
        Path::new("/run/user/1000/dmux"),
        &ready(&fixture),
        "/opt/wezterm/bin/wezterm",
        Path::new(GUI_CFG),
        request,
        &existing_cold_intent(request),
        GuiLaunchDeps {
            command: &commands,
            heartbeats: &wrong_token,
            launcher: &launcher,
            clock: &clock,
        },
        Duration::from_millis(50),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::IdentityConflict);
    assert_eq!(
        commands.cleanups.get(),
        1,
        "wrong-start-token GUI is killed/reaped"
    );

    let request = Uuid::from_u128(0x66666666_6666_4666_8666_666666666666);
    let requested = format!("gui-{}", request.simple());
    let commands = FakeCommands::successful(8001);
    let multiple = FakeHeartbeats::new([
        Ok(vec![]),
        Ok(vec![
            heartbeat(requested, 8001, "spawn-token"),
            heartbeat("gui-concurrent", 9001, "other"),
        ]),
    ]);
    let clock = FakeClock::default();
    let error = launch_attach_only_gui_with(
        Path::new("/run/user/1000/dmux"),
        &ready(&fixture),
        "/opt/wezterm/bin/wezterm",
        Path::new(GUI_CFG),
        request,
        &existing_cold_intent(request),
        GuiLaunchDeps {
            command: &commands,
            heartbeats: &multiple,
            launcher: &launcher,
            clock: &clock,
        },
        Duration::from_millis(50),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::IdentityConflict);
    assert!(error.message.contains("2 new GUI"), "{}", error.message);
    assert_eq!(commands.cleanups.get(), 1, "ambiguous GUI is killed/reaped");
}

#[test]
fn heartbeat_read_failure_cleans_up_the_launched_process() {
    let fixture = registry_fixture(FixedServicePlatform::Linux);
    let request = Uuid::from_u128(0x77777777_7777_4777_8777_777777777777);
    let commands = FakeCommands::successful(7100);
    let heartbeats = FakeHeartbeats::new([
        Ok(vec![]),
        Err(TypedError::new(
            ErrorCode::BridgeUnavailable,
            "private heartbeat directory changed",
        )),
    ]);
    let launcher = FakeLauncher::successful();
    let clock = FakeClock::default();
    let error = launch_attach_only_gui_with(
        Path::new("/run/user/1000/dmux"),
        &ready(&fixture),
        "/opt/wezterm/bin/wezterm",
        Path::new(GUI_CFG),
        request,
        &existing_cold_intent(request),
        GuiLaunchDeps {
            command: &commands,
            heartbeats: &heartbeats,
            launcher: &launcher,
            clock: &clock,
        },
        Duration::from_millis(50),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BridgeUnavailable);
    assert_eq!(commands.cleanups.get(), 1);
}

fn private_dir(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn write_heartbeat(root: &Path, name: &str, updated_at: u64, dir_mode: u32) {
    let instance = root.join("bridge").join("instances").join(name);
    private_dir(&instance);
    fs::set_permissions(&instance, fs::Permissions::from_mode(dir_mode)).unwrap();
    let heartbeat = BridgeHeartbeat {
        protocol_version: 1,
        gui_instance: name.into(),
        pid: 4242,
        process_start_token: "gui-start-token".into(),
        updated_at,
        panes: Vec::new(),
        domains: BTreeMap::new(),
    };
    let path = instance.join("heartbeat.json");
    fs::write(&path, serde_json::to_vec(&heartbeat).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
}

/// ADR 003 §3: a stale heartbeat means bridge-down. The production reader
/// behind `summon`, cold launch and the frozen presentation paths lists a
/// GUI as live only from a fresh, private, well-formed heartbeat; stale,
/// future-dated, unprivate, malformed and absent ones are not live instances
/// (report 06 row 11, closed here against `RuntimeHeartbeatSource`).
#[test]
fn stale_or_unprivate_heartbeats_are_not_live_instances() {
    let root = TempDir::new().unwrap();
    private_dir(root.path());
    private_dir(&root.path().join("bridge"));
    let instances = root.path().join("bridge").join("instances");
    private_dir(&instances);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    write_heartbeat(root.path(), "gui-fresh", now, 0o700);
    write_heartbeat(root.path(), "gui-stale", now - 10, 0o700);
    write_heartbeat(root.path(), "gui-future", now + 60, 0o700);
    write_heartbeat(root.path(), "gui-shared", now, 0o755);
    let broken = instances.join("gui-broken");
    private_dir(&broken);
    fs::write(broken.join("heartbeat.json"), b"{").unwrap();
    fs::set_permissions(
        broken.join("heartbeat.json"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    private_dir(&instances.join("gui-silent"));

    let live = RuntimeHeartbeatSource.live_instances(root.path()).unwrap();
    assert_eq!(
        live,
        vec![BridgeInstanceSelection {
            gui_instance: "gui-fresh".into(),
            pid: 4242,
            process_start_token: "gui-start-token".into(),
            domains: BTreeMap::new(),
        }]
    );

    // The one live GUI ages past HEARTBEAT_MAX_AGE: nothing is live, and the
    // callers' `[]` arm (cold launch) is what follows, never a reuse.
    write_heartbeat(root.path(), "gui-fresh", now - 3, 0o700);
    assert!(
        RuntimeHeartbeatSource
            .live_instances(root.path())
            .unwrap()
            .is_empty()
    );

    // No instance directory yet (no GUI has ever registered) is an empty
    // listing, not an error.
    let bare = TempDir::new().unwrap();
    private_dir(bare.path());
    assert!(
        RuntimeHeartbeatSource
            .live_instances(bare.path())
            .unwrap()
            .is_empty()
    );
}
