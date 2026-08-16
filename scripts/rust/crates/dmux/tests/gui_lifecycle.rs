use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use dmux::backend::{
    InventoryOutcome, InventoryScope, NativeGroupRow, NativeInventory, NativeSpaceRow,
    NativeSplitRow,
};
use dmux::error::{ErrorCode, TypedError};
use dmux::gui::BridgeInstanceSelection;
use dmux::gui_lifecycle::{
    CommandExit, DescriptorSource, FixedServicePlatform, GuiLaunchDeps, HeartbeatSource,
    LINUX_SERVICE_LABEL, LifecycleChild, LifecycleClock, LifecycleCommand, LifecycleCommandRunner,
    MACOS_SERVICE_LABEL, ReadyWezService, ServiceEnsureDeps, WezServiceInventory,
    ensure_ready_wez_service_with, launch_attach_only_gui_with,
};
use dmux::model::{Backend, BackendInstanceUid, ProviderHandle, ServerEpoch};
use dmux::registry::{Registry, RegistryConfig};
use dmux::runtime::WezMuxDescriptor;
use tempfile::TempDir;
use uuid::Uuid;

const SOCKET: &str = "/run/user/1000/dmux/wez-dmux.sock";
const GUI_CFG: &str = "/cfg/wezterm.lua";

#[derive(Debug, Clone)]
enum RunResult {
    Exit(CommandExit),
    Error(io::ErrorKind, &'static str),
}

#[derive(Debug)]
struct FakeCommands {
    run: RunResult,
    spawn: Result<u32, (io::ErrorKind, &'static str)>,
    cleanups: Rc<Cell<u32>>,
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
            cleanups: Rc::new(Cell::new(0)),
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
                cleanups: Rc::clone(&self.cleanups),
            })),
            Err((kind, detail)) => Err(io::Error::new(kind, detail)),
        }
    }
}

struct FakeChild {
    pid: u32,
    cleanups: Rc<Cell<u32>>,
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
        Ok(())
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
struct FakeDescriptors(RefCell<VecDeque<DescriptorStep>>);

impl FakeDescriptors {
    fn new(steps: impl IntoIterator<Item = DescriptorStep>) -> Self {
        Self(RefCell::new(steps.into_iter().collect()))
    }
}

impl DescriptorSource for FakeDescriptors {
    fn read(&self, _runtime_dir: &Path) -> io::Result<Option<WezMuxDescriptor>> {
        let mut steps = self.0.borrow_mut();
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
        .publish_backend_server(instance, epoch, Some(4242), Some("4242-start"), None, None)
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
        start_token: "4242-start".into(),
        boot_nonce: Some("boot".into()),
        backend_instance_uid: Some(fixture.instance.0.to_string()),
        recovery_generation: None,
        sentinel_window_id: Some(1),
        sentinel_tab_id: Some(2),
        sentinel_pane_id: Some(3),
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
    let calls = inventory.calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0],
        (
            InventoryScope {
                backend: Backend::Wez,
                endpoint: SOCKET.into(),
                expected_epoch: Some(fixture.epoch),
            },
            4242,
            "4242-start".into(),
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
fn stale_descriptor_epoch_never_retargets_and_times_out_typed() {
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
    assert!(error.message.contains("timed out"), "{}", error.message);
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
    let clock = FakeClock::default();
    let launched = launch_attach_only_gui_with(
        Path::new("/run/user/1000/dmux"),
        &ready(&fixture),
        "/opt/wezterm/bin/wezterm",
        Path::new(GUI_CFG),
        request,
        GuiLaunchDeps {
            command: &commands,
            heartbeats: &heartbeats,
            clock: &clock,
        },
        Duration::from_millis(100),
    )
    .unwrap();
    assert_eq!(launched.instance, fresh);
    assert_eq!(launched.class, format!("dmux-{}", request.simple()));
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
            "--class",
            &format!("dmux-{}", request.simple()),
            "--always-new-process",
            "--domain",
            "dmux",
            "--attach",
        ]
        .map(OsString::from)
    );
    assert_eq!(
        command.env_remove,
        ["WEZTERM_PANE", "TMUX", "TMUX_PANE"].map(OsString::from)
    );
    assert_eq!(
        command.env_set.get(&OsString::from("WEZTERM_UNIX_SOCKET")),
        Some(&OsString::from(SOCKET))
    );
    assert_eq!(
        command.env_set.get(&OsString::from("DMUX_RUNTIME_DIR")),
        Some(&OsString::from("/run/user/1000/dmux"))
    );
    assert_eq!(
        command.env_set.get(&OsString::from("DMUX_GUI_INSTANCE")),
        Some(&OsString::from(requested))
    );
    assert_eq!(
        command.env_set.get(&OsString::from("DMUX_WEZ_FIRST")),
        Some(&OsString::from("1"))
    );
    assert!(
        !command
            .args
            .windows(2)
            .any(|pair| pair == ["start", "--domain"])
    );
}

#[test]
fn attach_timeout_never_reuses_an_old_heartbeat() {
    let fixture = registry_fixture(FixedServicePlatform::Linux);
    let commands = FakeCommands::successful(7001);
    let old = heartbeat("gui-old", 6001, "old-token");
    let heartbeats = FakeHeartbeats::new([Ok(vec![old])]);
    let clock = FakeClock::default();
    let error = launch_attach_only_gui_with(
        Path::new("/run/user/1000/dmux"),
        &ready(&fixture),
        "/opt/wezterm/bin/wezterm",
        Path::new(GUI_CFG),
        Uuid::from_u128(0x44444444_4444_4444_8444_444444444444),
        GuiLaunchDeps {
            command: &commands,
            heartbeats: &heartbeats,
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
fn attach_rejects_wrong_pid_and_multiple_new_process_instances() {
    let fixture = registry_fixture(FixedServicePlatform::Linux);
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
        GuiLaunchDeps {
            command: &commands,
            heartbeats: &wrong_pid,
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
        GuiLaunchDeps {
            command: &commands,
            heartbeats: &wrong_token,
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
        GuiLaunchDeps {
            command: &commands,
            heartbeats: &multiple,
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
    let commands = FakeCommands::successful(7100);
    let heartbeats = FakeHeartbeats::new([
        Ok(vec![]),
        Err(TypedError::new(
            ErrorCode::BridgeUnavailable,
            "private heartbeat directory changed",
        )),
    ]);
    let clock = FakeClock::default();
    let error = launch_attach_only_gui_with(
        Path::new("/run/user/1000/dmux"),
        &ready(&fixture),
        "/opt/wezterm/bin/wezterm",
        Path::new(GUI_CFG),
        Uuid::from_u128(0x77777777_7777_4777_8777_777777777777),
        GuiLaunchDeps {
            command: &commands,
            heartbeats: &heartbeats,
            clock: &clock,
        },
        Duration::from_millis(50),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::BridgeUnavailable);
    assert_eq!(commands.cleanups.get(), 1);
}
