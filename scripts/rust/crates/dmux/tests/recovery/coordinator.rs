use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{self, Child, Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use dmux::backend::wez::{
    ProbeOutcome, RunError, RunOutput, WezInvocation, WezProvider, WezRunner,
};
use dmux::backend::{InventoryOutcome, InventoryScope, Provider};
use dmux::bootstrap::{self, BootstrapResult, BootstrapState, HelperAck, PaneEnvRecord};
use dmux::locks::{self, LockMode, LockScope};
use dmux::model::{Backend, BackendInstanceUid, Lifecycle, ServerEpoch, SpaceUid};
use dmux::operations::{OperationEnv, remove_space};
use dmux::recovery::{
    CreatedNode, MANIFEST_SCHEMA_VERSION, ManifestGroup, ManifestSpace, ManifestSplit,
    ManifestWindow, NativePane, NativeSnapshot, NativeTab, NativeWindow, RecoveryAction,
    RecoveryCommand, RecoveryCoordinatorOptions, RecoveryCrashPhase, RecoveryCrashPoint,
    RecoveryManifest, RecoveryOutcome, RecoveryResponse, RecoveryRunReport, RecoverySpool,
    RecoveryStatus, RecoveryStatusState, RemovedNode, RemovedNodeStatus, SnapshotCapturePlan,
    inspect_recovery, publish_snapshot_manifest_for_test, request_recovery_abort,
    request_recovery_resume, run_recovery_coordinator, snapshot_capture_plan_path,
};
use dmux::registry::recovery::RecoveryNodeState;
use dmux::registry::{
    BusyPolicy, LeaseScope, NativeBindingSpec, NativeKind, Registry, RegistryConfig,
};
use uuid::Uuid;

fn write_private(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) {
    let path = path.as_ref();
    let temporary = path.with_extension(format!("private-{}", Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)
        .unwrap();
    file.write_all(bytes.as_ref()).unwrap();
    file.sync_all().unwrap();
    fs::rename(temporary, path).unwrap();
}

struct World {
    _dir: tempfile::TempDir,
    config: RegistryConfig,
    runtime: PathBuf,
    manifests: PathBuf,
    instance: BackendInstanceUid,
    epoch: ServerEpoch,
    manifest: RecoveryManifest,
    pid: i64,
    start_token: String,
}

impl World {
    fn new(write_manifest: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = RegistryConfig {
            db_path: dir.path().join("registry.sqlite3"),
            lock_dir: dir.path().join("locks"),
            busy: BusyPolicy {
                busy_timeout: Duration::from_millis(500),
                attempts: 5,
                retry_base: Duration::from_millis(2),
            },
        };
        let runtime = dir.path().join("runtime");
        let manifests = dir.path().join("manifests");
        fs::DirBuilder::new().mode(0o700).create(&runtime).unwrap();
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&manifests)
            .unwrap();

        let mut registry = Registry::open(config.clone()).unwrap();
        let instance = registry
            .register_backend_instance(Backend::Wez, Some("/tmp/recovery-test.sock"), Some("test"))
            .unwrap();
        let epoch = ServerEpoch(Uuid::new_v4());
        let pid = process::id() as i64;
        let start_token = format!("test-{pid}-{}", Uuid::new_v4());
        registry
            .publish_backend_server(instance, epoch, Some(pid), Some(&start_token), None, None)
            .unwrap();
        let reservation = registry
            .reserve_space("dotfiles", instance, Uuid::new_v4())
            .unwrap();
        registry
            .finalize_create(
                reservation.space_uid,
                reservation.operation_uid,
                &NativeBindingSpec {
                    native_token: "dmux-space-opaque".into(),
                    native_kind: NativeKind::WezWorkspaceKey,
                    server_epoch: Some(epoch),
                },
            )
            .unwrap();
        let revision = registry.authority_head().unwrap().revision;
        let manifest = test_manifest(
            instance,
            reservation.space_uid,
            reservation.space_no.get(),
            revision,
        );
        if write_manifest {
            write_private(
                manifests.join("manifest.json"),
                serde_json::to_vec(&manifest).unwrap(),
            );
        }
        World {
            _dir: dir,
            config,
            runtime,
            manifests,
            instance,
            epoch,
            manifest,
            pid,
            start_token,
        }
    }

    fn options(&self, reply_timeout: Duration) -> RecoveryCoordinatorOptions {
        let mut options = RecoveryCoordinatorOptions::new(
            self.config.clone(),
            self.runtime.clone(),
            self.manifests.clone(),
            self.instance,
            self.epoch,
            self.pid,
            self.start_token.clone(),
            "/test-only/pane-bootstrap".into(),
        );
        options.default_program = vec!["/usr/bin/true".into()];
        options.reply_timeout = reply_timeout;
        options.lease_ttl = Duration::from_secs(5);
        options.skip_service_authority = true;
        options
    }
}

fn pane(cwd: &str) -> ManifestSplit {
    ManifestSplit {
        cwd: cwd.into(),
        domain: Some("local".into()),
        text: Some(format!("scrollback {cwd}")),
        process: Some(serde_json::json!({"name":"zsh","argv":["zsh","-l"]})),
        is_active: false,
        is_zoomed: false,
        left: Some(0),
        top: Some(0),
        width: Some(80),
        height: Some(24),
        right: None,
        bottom: None,
    }
}

fn test_manifest(
    instance: BackendInstanceUid,
    space_uid: SpaceUid,
    space_no: u64,
    revision: u64,
) -> RecoveryManifest {
    let mut root = pane("/root");
    root.right = Some(Box::new(pane("/right")));
    root.bottom = Some(Box::new(pane("/bottom")));
    RecoveryManifest {
        schema_version: MANIFEST_SCHEMA_VERSION,
        state: "complete".into(),
        manifest_id: Uuid::new_v4().to_string(),
        backend_instance_uid: instance,
        registry_revision: revision,
        generated_at: "2026-08-16T18:00:00Z".into(),
        spaces: vec![ManifestSpace {
            space_uid,
            space_no,
            opaque_key: "dmux-space-opaque".into(),
            logical_name: "dotfiles".into(),
            window_state: ManifestWindow {
                title: "dotfiles".into(),
                size: None,
                tabs: vec![
                    ManifestGroup {
                        title: "editor".into(),
                        is_active: true,
                        is_zoomed: false,
                        pane_tree: root,
                    },
                    ManifestGroup {
                        title: "shell".into(),
                        is_active: false,
                        is_zoomed: false,
                        pane_tree: pane("/shell"),
                    },
                ],
            },
        }],
    }
}

fn sentinel(epoch: ServerEpoch) -> NativeSnapshot {
    NativeSnapshot {
        complete: true,
        server_epoch: epoch,
        windows: vec![NativeWindow {
            window_id: "1".into(),
            workspace: format!("dmux:system:{}", epoch.0),
            tabs: vec![NativeTab {
                tab_id: "2".into(),
                panes: vec![NativePane {
                    pane_id: "3".into(),
                    title: "sentinel".into(),
                    domain: Some("local".into()),
                }],
            }],
        }],
    }
}

/// Stateful exact-CLI runner for one managed Wez Space.  The acceptance
/// regression uses the real `WezProvider` and `operations::remove_space`;
/// only the external mux process is replaced by this deterministic wire.
struct FinalWezRunner {
    epoch: ServerEpoch,
    endpoint: String,
    native_token: String,
    live: Cell<bool>,
    remove_calls: Cell<usize>,
    list_calls: Cell<usize>,
}

impl FinalWezRunner {
    fn new(world: &World) -> Self {
        FinalWezRunner {
            epoch: world.epoch,
            endpoint: "/tmp/recovery-test.sock".into(),
            native_token: world.manifest.spaces[0].opaque_key.clone(),
            live: Cell::new(true),
            remove_calls: Cell::new(0),
            list_calls: Cell::new(0),
        }
    }

    fn scope(&self) -> InventoryScope {
        InventoryScope {
            backend: Backend::Wez,
            endpoint: self.endpoint.clone(),
            expected_epoch: Some(self.epoch),
        }
    }
}

impl WezRunner for &FinalWezRunner {
    fn probe(&self, socket_path: &str, expected_server_pid: Option<u32>) -> ProbeOutcome {
        assert_eq!(socket_path, self.endpoint);
        assert_eq!(expected_server_pid, None);
        ProbeOutcome::Connectable
    }

    fn run(&self, invocation: &WezInvocation, _: Duration) -> Result<RunOutput, RunError> {
        let command = invocation
            .argv
            .iter()
            .position(|arg| arg == "list" || arg == "kill-pane")
            .map(|index| (index, invocation.argv[index].as_str()))
            .unwrap_or_else(|| panic!("unexpected Wez invocation: {:?}", invocation.argv));
        let stdout = match command {
            (_, "list") => {
                self.list_calls.set(self.list_calls.get() + 1);
                let mut rows = vec![serde_json::json!({
                    "window_id": 1,
                    "tab_id": 2,
                    "pane_id": 3,
                    "workspace": format!("dmux:system:{}", self.epoch.0),
                })];
                if self.live.get() {
                    rows.push(serde_json::json!({
                        "window_id": 20,
                        "tab_id": 30,
                        "pane_id": 40,
                        "workspace": self.native_token,
                        "tab_title": "dotfiles",
                        "title": "shell",
                        "cwd": "file:///root",
                    }));
                }
                serde_json::to_vec(&rows).unwrap()
            }
            (index, "kill-pane") => {
                assert_eq!(
                    &invocation.argv[index..],
                    &["kill-pane", "--pane-id", "40"],
                    "whole-Space removal must target the exact listed pane"
                );
                assert!(self.live.replace(false), "native Space removed twice");
                self.remove_calls.set(self.remove_calls.get() + 1);
                Vec::new()
            }
            _ => unreachable!(),
        };
        Ok(RunOutput {
            status: 0,
            stdout,
            stderr: Vec::new(),
        })
    }
}

struct PendingHelper {
    request_uid: Uuid,
    pane_id: String,
    handle: JoinHandle<()>,
}

struct InProcessMux {
    runtime: PathBuf,
    spool: RecoverySpool,
    snapshot: NativeSnapshot,
    objects: BTreeMap<String, CreatedNode>,
    helpers: Vec<PendingHelper>,
    seen: Option<(Uuid, u64)>,
    next_id: u64,
    restore_counts: BTreeMap<String, usize>,
    remove_counts: BTreeMap<String, usize>,
    remove_effect_counts: BTreeMap<String, usize>,
    drop_response_for: Option<String>,
    drop_remove_response_for: Option<String>,
    drop_verify_response: bool,
    reject_prepare: bool,
    dropped: bool,
    collapse_group_tabs: bool,
    inject_stale_response_once: bool,
    stale_response_emitted: bool,
    pending_response: Option<RecoveryResponse>,
    compare_restore_count: usize,
    inject_unmanaged_before_compare_restore_count: Option<usize>,
    compare_remove_count: usize,
    inject_unmanaged_before_compare_remove_count: Option<usize>,
    unmanaged_injected: bool,
}

impl InProcessMux {
    fn new(world: &World) -> Self {
        InProcessMux {
            runtime: world.runtime.clone(),
            spool: RecoverySpool::new(&world.runtime, world.epoch),
            snapshot: sentinel(world.epoch),
            objects: BTreeMap::new(),
            helpers: Vec::new(),
            seen: None,
            next_id: 10,
            restore_counts: BTreeMap::new(),
            remove_counts: BTreeMap::new(),
            remove_effect_counts: BTreeMap::new(),
            drop_response_for: None,
            drop_remove_response_for: None,
            drop_verify_response: false,
            reject_prepare: false,
            dropped: false,
            collapse_group_tabs: false,
            inject_stale_response_once: false,
            stale_response_emitted: false,
            pending_response: None,
            compare_restore_count: 0,
            inject_unmanaged_before_compare_restore_count: None,
            compare_remove_count: 0,
            inject_unmanaged_before_compare_remove_count: None,
            unmanaged_injected: false,
        }
    }

    fn poll_helpers(&mut self) {
        let mut index = 0;
        while index < self.helpers.len() {
            if !self.helpers[index].handle.is_finished() {
                index += 1;
                continue;
            }
            let helper = self.helpers.remove(index);
            helper.handle.join().unwrap();
            let title = bootstrap::run_title(helper.request_uid);
            for window in &mut self.snapshot.windows {
                for tab in &mut window.tabs {
                    for pane in &mut tab.panes {
                        if pane.pane_id == helper.pane_id {
                            pane.title = title.clone();
                        }
                    }
                }
            }
        }
    }

    fn tick(&mut self) {
        self.poll_helpers();
        if let Some(response) = self.pending_response.take() {
            write_response(&self.spool.response, &response);
            return;
        }
        let command = match fs::read(&self.spool.command)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<RecoveryCommand>(&bytes).ok())
        {
            Some(command) => command,
            None => return,
        };
        let tuple = (command.coordinator_uid, command.sequence);
        if self.seen == Some(tuple) {
            return;
        }
        self.seen = Some(tuple);
        let mut response = RecoveryResponse {
            protocol_version: command.protocol_version,
            coordinator_uid: command.coordinator_uid,
            generation_uid: command.generation_uid,
            sequence: command.sequence,
            fencing_token: command.fencing_token,
            ok: true,
            error: None,
            snapshot: None,
            created: None,
            removed: None,
            existing_absent: false,
        };
        match command.action {
            RecoveryAction::Inspect => {
                response.snapshot = Some(self.snapshot.clone());
            }
            RecoveryAction::Verify { .. } => {
                response.snapshot = Some(self.snapshot.clone());
                if self.drop_verify_response && !self.dropped {
                    self.dropped = true;
                    return;
                }
            }
            RecoveryAction::Prepare { .. } => {
                if self.reject_prepare {
                    response.ok = false;
                    response.error = Some("injected Prepare rejection".into());
                }
            }
            RecoveryAction::CompareAndRestoreNode {
                node,
                request_uid,
                bootstrap_argv,
                expected_tree,
                expected_parent,
                expected_existing,
                create_if_absent,
            } => {
                self.compare_restore_count += 1;
                if self.inject_unmanaged_before_compare_restore_count
                    == Some(self.compare_restore_count)
                    && !self.unmanaged_injected
                {
                    self.inject_unmanaged_pane();
                }
                let path = node.manifest_node_path.clone();
                if expected_tree != self.snapshot.tree_precondition() {
                    response.ok = false;
                    response.error = Some("native tree precondition changed".into());
                } else if expected_parent != self.native_parent(&node) {
                    response.ok = false;
                    response.error = Some("native parent precondition changed".into());
                } else if let Some(expected) = expected_existing {
                    if create_if_absent {
                        response.ok = false;
                        response.error = Some("existing reconcile requested creation".into());
                    } else if self
                        .objects
                        .get(&path)
                        .is_some_and(|actual| same_native_ids(actual, &expected))
                    {
                        response.created = self.objects.get(&path).cloned();
                    } else {
                        response.ok = false;
                        response.error = Some("expected recovery object is not exact".into());
                    }
                } else if create_if_absent {
                    assert_eq!(bootstrap_argv[1], request_uid.to_string());
                    if self.objects.contains_key(&path) {
                        response.ok = false;
                        response.error = Some("unexpected prepared recovery object".into());
                    } else {
                        *self.restore_counts.entry(path.clone()).or_default() += 1;
                        let created = self.create_node(&node, request_uid);
                        self.objects.insert(path.clone(), created.clone());
                        response.created = Some(created);
                    }
                } else {
                    self.objects.remove(&path);
                    response.existing_absent = true;
                }
                if self.drop_response_for.as_deref() == Some(path.as_str()) && !self.dropped {
                    self.dropped = true;
                    return;
                }
            }
            RecoveryAction::CompareAndRemoveNode {
                manifest_node_path,
                pane_id,
                tab_id,
                window_id,
                expected_tree,
            } => {
                self.compare_remove_count += 1;
                if self.inject_unmanaged_before_compare_remove_count
                    == Some(self.compare_remove_count)
                    && !self.unmanaged_injected
                {
                    self.inject_unmanaged_pane();
                }
                if expected_tree != self.snapshot.tree_precondition() {
                    response.ok = false;
                    response.error = Some("native tree precondition changed".into());
                } else {
                    *self
                        .remove_counts
                        .entry(manifest_node_path.clone())
                        .or_default() += 1;
                    response.removed =
                        Some(self.remove_node(&manifest_node_path, &pane_id, &tab_id, &window_id));
                }
                if self.drop_remove_response_for.as_deref() == Some(manifest_node_path.as_str())
                    && !self.dropped
                {
                    self.dropped = true;
                    return;
                }
            }
        }
        if self.inject_stale_response_once && !self.stale_response_emitted {
            self.stale_response_emitted = true;
            self.pending_response = Some(response.clone());
            let mut stale = response;
            stale.coordinator_uid = Uuid::new_v4();
            stale.generation_uid = Uuid::new_v4();
            stale.sequence = stale.sequence.saturating_add(73);
            stale.fencing_token = stale.fencing_token.saturating_sub(1);
            write_response(&self.spool.response, &stale);
        } else {
            write_response(&self.spool.response, &response);
        }
    }

    fn inject_unmanaged_pane(&mut self) {
        self.unmanaged_injected = true;
        self.snapshot.windows.push(NativeWindow {
            window_id: "900000".into(),
            workspace: "out-of-band".into(),
            tabs: vec![NativeTab {
                tab_id: "900001".into(),
                panes: vec![NativePane {
                    pane_id: "900002".into(),
                    title: "out-of-band mutation".into(),
                    domain: Some("local".into()),
                }],
            }],
        });
    }

    fn native_parent(&self, node: &dmux::recovery::RestoreNode) -> Option<String> {
        match node.operation {
            dmux::recovery::RestoreOperation::SpaceRoot => None,
            dmux::recovery::RestoreOperation::GroupRoot => {
                let first = format!("/spaces/{}/groups/1/splits/L", node.space_uid.0);
                self.objects
                    .get(&first)
                    .map(|object| object.window_id.clone())
            }
            dmux::recovery::RestoreOperation::Split => node
                .parent_path
                .as_ref()
                .and_then(|parent| self.objects.get(parent))
                .map(|object| object.pane_id.clone()),
        }
    }

    fn remove_node(
        &mut self,
        path: &str,
        pane_id: &str,
        tab_id: &str,
        window_id: &str,
    ) -> RemovedNode {
        let pane = pane_id.parse::<u64>().unwrap();
        let tab = tab_id.parse::<u64>().unwrap();
        let window = window_id.parse::<u64>().unwrap();
        let mut removed_pane_ids = Vec::new();
        let mut removed_tab_ids = Vec::new();
        let mut removed_window_ids = Vec::new();
        let mut status = RemovedNodeStatus::NotFound;
        if let Some(window_index) = self
            .snapshot
            .windows
            .iter()
            .position(|candidate| candidate.window_id == window_id)
            && let Some(tab_index) = self.snapshot.windows[window_index]
                .tabs
                .iter()
                .position(|candidate| candidate.tab_id == tab_id)
            && let Some(pane_index) = self.snapshot.windows[window_index].tabs[tab_index]
                .panes
                .iter()
                .position(|candidate| candidate.pane_id == pane_id)
        {
            self.snapshot.windows[window_index].tabs[tab_index]
                .panes
                .remove(pane_index);
            removed_pane_ids.push(pane);
            status = RemovedNodeStatus::Removed;
            if self.snapshot.windows[window_index].tabs[tab_index]
                .panes
                .is_empty()
            {
                self.snapshot.windows[window_index].tabs.remove(tab_index);
                removed_tab_ids.push(tab);
                if self.snapshot.windows[window_index].tabs.is_empty() {
                    self.snapshot.windows.remove(window_index);
                    removed_window_ids.push(window);
                }
            }
            *self
                .remove_effect_counts
                .entry(path.to_string())
                .or_default() += 1;
        }
        self.objects.remove(path);
        RemovedNode {
            schema_version: 1,
            status,
            kind: "pane".into(),
            requested_native_id: pane,
            actual_parent_tab_id: (status == RemovedNodeStatus::Removed).then_some(tab),
            actual_parent_window_id: (status == RemovedNodeStatus::Removed).then_some(window),
            actual_workspace: None,
            removed_pane_ids,
            removed_tab_ids,
            removed_window_ids,
            postcondition_error: None,
        }
    }

    fn create_node(
        &mut self,
        node: &dmux::recovery::RestoreNode,
        request_uid: Uuid,
    ) -> CreatedNode {
        let pane_id = self.allocate_id();
        let (window_id, tab_id) = match node.operation {
            dmux::recovery::RestoreOperation::SpaceRoot => {
                let window_id = self.allocate_id();
                let tab_id = self.allocate_id();
                self.snapshot.windows.push(NativeWindow {
                    window_id: window_id.clone(),
                    workspace: node.opaque_key.clone(),
                    tabs: vec![NativeTab {
                        tab_id: tab_id.clone(),
                        panes: Vec::new(),
                    }],
                });
                (window_id, tab_id)
            }
            dmux::recovery::RestoreOperation::GroupRoot => {
                let first = format!("/spaces/{}/groups/1/splits/L", node.space_uid.0);
                let first = self.objects.get(&first).unwrap();
                let window_id = first.window_id.clone();
                if self.collapse_group_tabs {
                    (window_id, first.tab_id.clone())
                } else {
                    let tab_id = self.allocate_id();
                    let window = self
                        .snapshot
                        .windows
                        .iter_mut()
                        .find(|window| window.window_id == window_id)
                        .unwrap();
                    window.tabs.push(NativeTab {
                        tab_id: tab_id.clone(),
                        panes: Vec::new(),
                    });
                    (window_id, tab_id)
                }
            }
            dmux::recovery::RestoreOperation::Split => {
                let parent = self
                    .objects
                    .get(node.parent_path.as_ref().unwrap())
                    .unwrap();
                (parent.window_id.clone(), parent.tab_id.clone())
            }
        };
        let title = bootstrap::reserved_title(request_uid);
        let tab = self
            .snapshot
            .windows
            .iter_mut()
            .find(|window| window.window_id == window_id)
            .unwrap()
            .tabs
            .iter_mut()
            .find(|tab| tab.tab_id == tab_id)
            .unwrap();
        tab.panes.push(NativePane {
            pane_id: pane_id.clone(),
            title,
            domain: Some("local".into()),
        });
        self.helpers.push(PendingHelper {
            request_uid,
            pane_id: pane_id.clone(),
            handle: fake_bootstrap_helper(self.runtime.clone(), request_uid, pane_id.clone()),
        });
        CreatedNode {
            window_id,
            tab_id,
            pane_id: pane_id.clone(),
            titled_pane_ids: vec![pane_id],
        }
    }

    fn allocate_id(&mut self) -> String {
        let id = self.next_id;
        self.next_id += 1;
        id.to_string()
    }

    fn drive_until(&mut self, done: impl Fn() -> bool) {
        let started = Instant::now();
        while !done() {
            self.tick();
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "recovery test driver timed out"
            );
            thread::sleep(Duration::from_millis(2));
        }
        for _ in 0..100 {
            self.poll_helpers();
            if self.helpers.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
    }
}

fn fake_bootstrap_helper(runtime: PathBuf, request_uid: Uuid, pane_id: String) -> JoinHandle<()> {
    thread::spawn(move || {
        let paths = bootstrap::BootstrapPaths::new(&runtime, request_uid);
        let env = PaneEnvRecord {
            request_uid,
            wezterm_pane: Some(pane_id),
            tmux_pane: None,
            helper_pid: process::id(),
        };
        fs::write(&paths.pane_env, serde_json::to_vec(&env).unwrap()).unwrap();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&paths.fifo)
            .unwrap();
        let mut line = String::new();
        std::io::BufReader::new(file).read_line(&mut line).unwrap();
        let result: BootstrapResult = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(result.request_uid, request_uid);
        fs::write(
            &paths.ack,
            serde_json::to_vec(&HelperAck { request_uid }).unwrap(),
        )
        .unwrap();
    })
}

fn write_response(path: &Path, response: &RecoveryResponse) {
    let tmp = path.with_extension(format!("tmp-{}", Uuid::new_v4()));
    fs::write(&tmp, serde_json::to_vec(response).unwrap()).unwrap();
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600)).unwrap();
    fs::rename(tmp, path).unwrap();
}

fn same_native_ids(left: &CreatedNode, right: &CreatedNode) -> bool {
    left.window_id == right.window_id
        && left.tab_id == right.tab_id
        && left.pane_id == right.pane_id
}

fn collapse_completed_second_group_into_first_tab(
    world: &World,
    mux: &mut InProcessMux,
    generation_uid: Uuid,
) {
    let nodes = world.manifest.restore_nodes();
    let first_root = nodes
        .iter()
        .find(|node| node.group_index == 1 && node.parent_path.is_none())
        .unwrap();
    let first = mux.objects[&first_root.manifest_node_path].clone();
    let second_paths = nodes
        .iter()
        .filter(|node| node.group_index == 2)
        .map(|node| node.manifest_node_path.clone())
        .collect::<Vec<_>>();
    let second_tabs = second_paths
        .iter()
        .map(|path| mux.objects[path].tab_id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(!second_tabs.contains(&first.tab_id));

    let window = mux
        .snapshot
        .windows
        .iter_mut()
        .find(|window| window.window_id == first.window_id)
        .unwrap();
    let mut moved = Vec::new();
    for tab_id in &second_tabs {
        let index = window
            .tabs
            .iter()
            .position(|tab| &tab.tab_id == tab_id)
            .unwrap();
        moved.extend(window.tabs.remove(index).panes);
    }
    window
        .tabs
        .iter_mut()
        .find(|tab| tab.tab_id == first.tab_id)
        .unwrap()
        .panes
        .extend(moved);

    let registry = Registry::open(world.config.clone()).unwrap();
    let rows = registry.recovery_rows(generation_uid).unwrap();
    for path in second_paths {
        let object = mux.objects.get_mut(&path).unwrap();
        object.tab_id.clone_from(&first.tab_id);
        let request_uid = rows
            .iter()
            .find(|row| row.manifest_node_path == path)
            .and_then(|row| row.bootstrap_request_uid)
            .unwrap();
        registry
            .raw_connection()
            .execute(
                "UPDATE bootstrap_requests SET returned_native_ids = ?1 WHERE request_uid = ?2",
                rusqlite::params![
                    serde_json::to_string(object).unwrap(),
                    request_uid.to_string()
                ],
            )
            .unwrap();
    }
}

fn spawn_coordinator(
    options: RecoveryCoordinatorOptions,
) -> JoinHandle<dmux::recovery::Result<RecoveryRunReport>> {
    thread::spawn(move || run_recovery_coordinator(options))
}

#[test]
#[ignore = "entry point used only by the real-process recovery crash harness"]
fn recovery_subprocess_entry() {
    let Some(db_path) = std::env::var_os("DMUX_RECOVERY_TEST_DB") else {
        return;
    };
    let phase = match std::env::var("DMUX_RECOVERY_TEST_PHASE").unwrap().as_str() {
        "command" => RecoveryCrashPhase::AfterCommandPublish,
        "response" => RecoveryCrashPhase::AfterResponseRead,
        "ack" => RecoveryCrashPhase::AfterBootstrapAck,
        "root" => RecoveryCrashPhase::AfterRootCompleted,
        other => panic!("unknown subprocess crash phase {other}"),
    };
    let config = RegistryConfig {
        db_path: PathBuf::from(db_path),
        lock_dir: PathBuf::from(std::env::var_os("DMUX_RECOVERY_TEST_LOCKS").unwrap()),
        busy: BusyPolicy {
            busy_timeout: Duration::from_millis(500),
            attempts: 5,
            retry_base: Duration::from_millis(2),
        },
    };
    let mut options = RecoveryCoordinatorOptions::new(
        config,
        PathBuf::from(std::env::var_os("DMUX_RECOVERY_TEST_RUNTIME").unwrap()),
        PathBuf::from(std::env::var_os("DMUX_RECOVERY_TEST_MANIFESTS").unwrap()),
        BackendInstanceUid(
            Uuid::parse_str(&std::env::var("DMUX_RECOVERY_TEST_INSTANCE").unwrap()).unwrap(),
        ),
        ServerEpoch(Uuid::parse_str(&std::env::var("DMUX_RECOVERY_TEST_EPOCH").unwrap()).unwrap()),
        std::env::var("DMUX_RECOVERY_TEST_SERVER_PID")
            .unwrap()
            .parse()
            .unwrap(),
        std::env::var("DMUX_RECOVERY_TEST_START_TOKEN").unwrap(),
        "/test-only/pane-bootstrap".into(),
    );
    options.default_program = vec!["/usr/bin/true".into()];
    options.reply_timeout = Duration::from_secs(10);
    options.lease_ttl = Duration::from_secs(30);
    options.skip_service_authority = true;
    options.crash_point = Some(RecoveryCrashPoint {
        phase,
        action: std::env::var("DMUX_RECOVERY_TEST_ACTION").unwrap(),
    });
    options.hard_stop_path = Some(PathBuf::from(
        std::env::var_os("DMUX_RECOVERY_TEST_MARKER").unwrap(),
    ));
    let result = run_recovery_coordinator(options);
    panic!("subprocess coordinator returned before SIGKILL: {result:?}");
}

fn spawn_recovery_subprocess(world: &World, phase: &str, action: &str, marker: &Path) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "coordinator::recovery_subprocess_entry",
            "--ignored",
            "--nocapture",
        ])
        .env("DMUX_RECOVERY_TEST_DB", &world.config.db_path)
        .env("DMUX_RECOVERY_TEST_LOCKS", &world.config.lock_dir)
        .env("DMUX_RECOVERY_TEST_RUNTIME", &world.runtime)
        .env("DMUX_RECOVERY_TEST_MANIFESTS", &world.manifests)
        .env("DMUX_RECOVERY_TEST_INSTANCE", world.instance.0.to_string())
        .env("DMUX_RECOVERY_TEST_EPOCH", world.epoch.0.to_string())
        .env("DMUX_RECOVERY_TEST_SERVER_PID", world.pid.to_string())
        .env("DMUX_RECOVERY_TEST_START_TOKEN", &world.start_token)
        .env("DMUX_RECOVERY_TEST_PHASE", phase)
        .env("DMUX_RECOVERY_TEST_ACTION", action)
        .env("DMUX_RECOVERY_TEST_MARKER", marker)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn test_action_label(action: &RecoveryAction) -> String {
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

fn kill_at_real_process_marker(
    child: &mut Child,
    mux: &mut InProcessMux,
    marker: &Path,
    hold_before_action: Option<&str>,
) {
    let started = Instant::now();
    while !marker.exists() {
        let held = hold_before_action.is_some_and(|wanted| {
            fs::read(&mux.spool.command)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<RecoveryCommand>(&bytes).ok())
                .is_some_and(|command| test_action_label(&command.action) == wanted)
        });
        if !held {
            mux.tick();
        }
        assert!(
            child.try_wait().unwrap().is_none(),
            "coordinator subprocess exited before its hard-stop marker"
        );
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "coordinator subprocess hard-stop marker timed out"
        );
        thread::sleep(Duration::from_millis(2));
    }
    child.kill().unwrap();
    let status = child.wait().unwrap();
    assert!(!status.success());
    for _ in 0..200 {
        mux.poll_helpers();
        if mux.helpers.is_empty() {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn simultaneous_startup_clients_restore_once_and_the_second_observes_ready() {
    let world = World::new(true);
    let mut mux = InProcessMux::new(&world);
    let first = spawn_coordinator(world.options(Duration::from_secs(2)));
    let second = spawn_coordinator(world.options(Duration::from_secs(2)));
    mux.drive_until(|| first.is_finished() && second.is_finished());
    let reports = [
        first.join().unwrap().unwrap(),
        second.join().unwrap().unwrap(),
    ];
    assert!(
        reports
            .iter()
            .any(|report| report.outcome == RecoveryOutcome::Restored)
    );
    assert!(
        reports
            .iter()
            .any(|report| report.outcome == RecoveryOutcome::AlreadyReady)
    );
    assert_eq!(
        mux.snapshot.panes().count(),
        world.manifest.restore_nodes().len() + 1
    );
    assert_eq!(
        mux.restore_counts.len(),
        world.manifest.restore_nodes().len()
    );
    assert!(mux.restore_counts.values().all(|count| *count == 1));

    let status: RecoveryStatus =
        serde_json::from_slice(&fs::read(&mux.spool.status).unwrap()).unwrap();
    assert_eq!(status.state, RecoveryStatusState::Ready);
    let registry = Registry::open(world.config.clone()).unwrap();
    let generation = reports
        .iter()
        .find_map(|report| report.generation_uid)
        .unwrap();
    assert!(
        registry
            .recovery_rows(generation)
            .unwrap()
            .iter()
            .all(|row| row.node_state == RecoveryNodeState::Completed)
    );
}

#[test]
fn arbitrary_hidden_coordinator_invocation_cannot_mutate_registry_or_status() {
    let world = World::new(true);
    let registry = Registry::open(world.config.clone()).unwrap();
    let before_server = registry.backend_server(world.instance).unwrap();
    let before_head = registry.authority_head().unwrap();
    drop(registry);

    let mut forged = world.options(Duration::from_millis(100));
    forged.skip_service_authority = false;
    forged.server_epoch = ServerEpoch(Uuid::new_v4());
    forged.server_pid = i64::from(process::id()) + 10_000;
    forged.server_start_token = "macos:1:0".into();
    let forged_spool = RecoverySpool::new(&forged.runtime_dir, forged.server_epoch);
    let error = run_recovery_coordinator(forged).unwrap_err();

    assert!(
        error.to_string().contains("recovery fence lost"),
        "unexpected authority failure: {error}"
    );
    let registry = Registry::open(world.config.clone()).unwrap();
    assert_eq!(
        registry.backend_server(world.instance).unwrap(),
        before_server
    );
    assert_eq!(registry.authority_head().unwrap(), before_head);
    assert!(
        registry
            .current_lease(&LeaseScope::Recovery(world.instance))
            .unwrap()
            .is_none()
    );
    assert!(!forged_spool.status.exists());
    assert!(!forged_spool.command.exists());
}

#[test]
fn crash_injection_seams_require_the_explicit_test_authority_bypass() {
    let world = World::new(true);
    let mut forged = world.options(Duration::from_millis(100));
    forged.skip_service_authority = false;
    forged.crash_point = Some(RecoveryCrashPoint {
        phase: RecoveryCrashPhase::AfterCommandPublish,
        action: "inspect".into(),
    });
    forged.hard_stop_path = Some(world.runtime.join("must-not-exist"));
    let error = run_recovery_coordinator(forged).unwrap_err();
    assert!(
        error.to_string().contains("explicit test authority bypass"),
        "unexpected test-seam refusal: {error}"
    );
    assert!(!world.runtime.join("must-not-exist").exists());
    assert!(
        Registry::open(world.config.clone())
            .unwrap()
            .current_lease(&LeaseScope::Recovery(world.instance))
            .unwrap()
            .is_none()
    );
}

#[test]
fn service_authority_drift_under_fence_cannot_publish_stale_incarnation() {
    let world = World::new(true);
    let registry = Registry::open(world.config.clone()).unwrap();
    let before_server = registry.backend_server(world.instance).unwrap();
    drop(registry);

    let mut stale_child = world.options(Duration::from_millis(100));
    stale_child.server_epoch = ServerEpoch(Uuid::new_v4());
    stale_child.server_pid += 10_000;
    stale_child.server_start_token = "stale-native-process-token".into();
    stale_child.fail_service_authority_after_lock = true;
    let stale_spool = RecoverySpool::new(&stale_child.runtime_dir, stale_child.server_epoch);
    let error = run_recovery_coordinator(stale_child).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("authority changed before registry publish"),
        "unexpected under-fence authority failure: {error}"
    );
    let registry = Registry::open(world.config.clone()).unwrap();
    assert_eq!(
        registry.backend_server(world.instance).unwrap(),
        before_server,
        "a coordinator whose mux witness drifted must not overwrite server identity"
    );
    assert!(
        registry
            .current_lease(&LeaseScope::Recovery(world.instance))
            .unwrap()
            .is_none(),
        "authority drift before recovery begins must release its provisional lease"
    );
    assert!(!stale_spool.command.exists());
}

#[test]
fn recovery_spool_rejects_symlinked_dirs_and_hostile_message_types() {
    let world = World::new(false);
    let victim_dir = world._dir.path().join("victim-dir");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&victim_dir)
        .unwrap();
    let victim = victim_dir.join("victim");
    write_private(&victim, b"victim-must-survive");
    symlink(&victim_dir, world.runtime.join("recovery")).unwrap();

    let spool = RecoverySpool::new(&world.runtime, world.epoch);
    assert!(spool.prepare().is_err());
    assert_eq!(fs::read(&victim).unwrap(), b"victim-must-survive");

    fs::remove_file(world.runtime.join("recovery")).unwrap();
    let spool = RecoverySpool::new(&world.runtime, world.epoch);
    spool.prepare().unwrap();
    symlink(&victim, &spool.response).unwrap();
    assert!(spool.clear_messages().is_err());
    assert_eq!(fs::read(&victim).unwrap(), b"victim-must-survive");
    fs::remove_file(&spool.response).unwrap();

    let fifo_c = std::ffi::CString::new(spool.response.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
    fs::set_permissions(&spool.response, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(spool.clear_messages().is_err());
    fs::remove_file(&spool.response).unwrap();

    fs::write(&spool.response, b"{}").unwrap();
    fs::set_permissions(&spool.response, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(spool.clear_messages().is_err());
    fs::remove_file(&spool.response).unwrap();

    let oversized = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&spool.response)
        .unwrap();
    oversized.set_len(1024 * 1024 + 1).unwrap();
    assert!(spool.clear_messages().is_err());
    assert_eq!(fs::read(victim).unwrap(), b"victim-must-survive");
}

#[test]
fn recovery_spool_retains_one_epoch_directory_across_path_replacement() {
    let world = World::new(false);
    let spool = RecoverySpool::new(&world.runtime, world.epoch);
    spool.prepare().unwrap();
    write_private(&spool.response, b"held-response");

    let held = world.runtime.join("recovery/held-epoch");
    fs::rename(&spool.dir, &held).unwrap();
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&spool.dir)
        .unwrap();
    write_private(&spool.response, b"replacement-response");

    spool.clear_messages().unwrap();
    assert!(!held.join("response.json").exists());
    assert_eq!(fs::read(&spool.response).unwrap(), b"replacement-response");
}

#[test]
fn lower_fence_response_is_ignored_until_the_matching_response_arrives() {
    let world = World::new(true);
    let mut mux = InProcessMux::new(&world);
    mux.inject_stale_response_once = true;

    let coordinator = spawn_coordinator(world.options(Duration::from_secs(2)));
    mux.drive_until(|| coordinator.is_finished());
    let report = coordinator.join().unwrap().unwrap();

    assert_eq!(report.outcome, RecoveryOutcome::Restored);
    assert!(mux.stale_response_emitted);
    assert!(mux.pending_response.is_none());
    assert_eq!(
        mux.snapshot.panes().count(),
        world.manifest.restore_nodes().len() + 1
    );
    assert!(mux.restore_counts.values().all(|count| *count == 1));
}

#[test]
fn drift_after_precheck_is_atomically_rejected_before_the_first_native_create() {
    let world = World::new(true);
    let mut mux = InProcessMux::new(&world);
    mux.inject_unmanaged_before_compare_restore_count = Some(1);

    let coordinator = spawn_coordinator(world.options(Duration::from_secs(2)));
    mux.drive_until(|| coordinator.is_finished());
    let error = coordinator.join().unwrap().unwrap_err();

    assert!(mux.unmanaged_injected);
    assert!(
        mux.restore_counts.is_empty(),
        "the in-callback comparison must fail before native create: {:?}",
        mux.restore_counts
    );
    assert!(mux.objects.is_empty());
    assert!(
        error
            .to_string()
            .contains("native tree precondition changed"),
        "unexpected drift failure: {error}"
    );
    let status: RecoveryStatus =
        serde_json::from_slice(&fs::read(&mux.spool.status).unwrap()).unwrap();
    assert_eq!(status.state, RecoveryStatusState::Failed);
}

#[test]
fn drift_after_the_next_precheck_is_atomically_rejected_before_that_native_create() {
    let world = World::new(true);
    let mut mux = InProcessMux::new(&world);
    mux.inject_unmanaged_before_compare_restore_count = Some(2);

    let coordinator = spawn_coordinator(world.options(Duration::from_secs(2)));
    mux.drive_until(|| coordinator.is_finished());
    let error = coordinator.join().unwrap().unwrap_err();

    assert!(mux.unmanaged_injected);
    assert_eq!(
        mux.restore_counts.values().sum::<usize>(),
        1,
        "the second in-callback comparison must prevent its native create"
    );
    assert_eq!(mux.objects.len(), 1);
    assert!(
        error
            .to_string()
            .contains("native tree precondition changed"),
        "unexpected drift failure: {error}"
    );
    let status: RecoveryStatus =
        serde_json::from_slice(&fs::read(&mux.spool.status).unwrap()).unwrap();
    assert_eq!(status.state, RecoveryStatusState::Failed);
}

#[test]
fn drift_after_resume_precheck_is_rejected_before_existing_token_reconciliation() {
    let world = World::new(true);
    let first_path = world.manifest.restore_nodes()[0].manifest_node_path.clone();
    let mut mux = InProcessMux::new(&world);
    mux.drop_response_for = Some(first_path.clone());
    let first_options = world.options(Duration::from_millis(120));
    let holder_uid = first_options.request_uid;
    let first = spawn_coordinator(first_options);
    mux.drive_until(|| first.is_finished());
    assert!(first.join().unwrap().is_err());
    assert_eq!(mux.restore_counts[&first_path], 1);

    let registry = Registry::open(world.config.clone()).unwrap();
    let (_, rows) = registry
        .unfinished_recovery_for_instance(world.instance)
        .unwrap()
        .unwrap();
    let request_uid = rows
        .iter()
        .find(|row| row.manifest_node_path == first_path)
        .and_then(|row| row.bootstrap_request_uid)
        .unwrap();
    let before = registry
        .bootstrap_request(request_uid)
        .unwrap()
        .unwrap()
        .state;
    assert_eq!(before, BootstrapState::Issued);
    drop(registry);

    mux.drop_response_for = None;
    mux.dropped = false;
    mux.seen = None;
    let _ = fs::remove_file(&mux.spool.command);
    let _ = fs::remove_file(&mux.spool.response);
    mux.inject_unmanaged_before_compare_restore_count = Some(mux.compare_restore_count + 1);
    let mut resume = world.options(Duration::from_secs(2));
    resume.resume_failed = true;
    resume.request_uid = holder_uid;
    let resumed = spawn_coordinator(resume);
    mux.drive_until(|| resumed.is_finished());
    let error = resumed.join().unwrap().unwrap_err();

    assert!(mux.unmanaged_injected);
    assert_eq!(mux.restore_counts[&first_path], 1);
    assert!(
        error
            .to_string()
            .contains("native tree precondition changed"),
        "unexpected reconcile drift failure: {error}"
    );
    let registry = Registry::open(world.config.clone()).unwrap();
    assert_eq!(
        registry
            .bootstrap_request(request_uid)
            .unwrap()
            .unwrap()
            .state,
        before,
        "the rejected combined callback must not advance bootstrap reconciliation"
    );
    let (_, rows) = registry
        .unfinished_recovery_for_instance(world.instance)
        .unwrap()
        .unwrap();
    assert_eq!(
        rows.iter()
            .find(|row| row.manifest_node_path == first_path)
            .unwrap()
            .node_state,
        RecoveryNodeState::Restoring
    );
}

#[test]
fn final_snapshot_rejects_two_manifest_groups_collapsed_into_one_tab() {
    let world = World::new(true);
    let mut mux = InProcessMux::new(&world);
    mux.collapse_group_tabs = true;

    let coordinator = spawn_coordinator(world.options(Duration::from_secs(2)));
    mux.drive_until(|| coordinator.is_finished());
    let error = coordinator.join().unwrap().unwrap_err();

    assert!(
        error.to_string().contains("maps multiple Groups"),
        "unexpected topology failure: {error}"
    );
    let status: RecoveryStatus =
        serde_json::from_slice(&fs::read(&mux.spool.status).unwrap()).unwrap();
    assert_eq!(status.state, RecoveryStatusState::Failed);
    let registry = Registry::open(world.config.clone()).unwrap();
    let (_, rows) = registry
        .unfinished_recovery_for_instance(world.instance)
        .unwrap()
        .expect("invalid topology must retain a failed durable generation");
    assert_eq!(
        rows.iter()
            .find(|row| row.manifest_node_path == dmux::registry::recovery::RECOVERY_GENERATION_PATH)
            .unwrap()
            .node_state,
        RecoveryNodeState::Failed
    );
}

#[test]
fn no_manifest_is_ready_with_only_the_sentinel_and_nonempty_fails_immediately() {
    let world = World::new(false);
    let mut mux = InProcessMux::new(&world);
    let ready = spawn_coordinator(world.options(Duration::from_secs(2)));
    mux.drive_until(|| ready.is_finished());
    let report = ready.join().unwrap().unwrap();
    assert_eq!(report.outcome, RecoveryOutcome::NoEligibleManifest);
    assert_eq!(mux.snapshot.panes().count(), 1);
    let status: RecoveryStatus =
        serde_json::from_slice(&fs::read(&mux.spool.status).unwrap()).unwrap();
    assert_eq!(status.state, RecoveryStatusState::Ready);

    let other = World::new(true);
    let mut nonempty = InProcessMux::new(&other);
    nonempty.snapshot.windows.push(NativeWindow {
        window_id: "98".into(),
        workspace: "default".into(),
        tabs: vec![NativeTab {
            tab_id: "99".into(),
            panes: vec![NativePane {
                pane_id: "100".into(),
                title: "unmanaged default shell".into(),
                domain: Some("local".into()),
            }],
        }],
    });
    let failed = spawn_coordinator(other.options(Duration::from_secs(2)));
    nonempty.drive_until(|| failed.is_finished());
    let error = failed.join().unwrap().unwrap_err();
    assert_eq!(error.stable_code(), "recovery_ineligible");
    assert!(nonempty.restore_counts.is_empty());
    let status: RecoveryStatus =
        serde_json::from_slice(&fs::read(&nonempty.spool.status).unwrap()).unwrap();
    assert_eq!(status.state, RecoveryStatusState::Failed);
    assert!(status.error.unwrap().contains("not recovery-empty"));
}

#[test]
fn explicit_final_wez_remove_records_empty_floor_and_blocks_old_manifest_in_fresh_epoch() {
    let world = World::new(true);
    let runner = FinalWezRunner::new(&world);
    let provider =
        WezProvider::with_runner("/test-only/wezterm", "/test-only/dmux-mux.lua", &runner);
    let scope = runner.scope();
    let space_uid = world.manifest.spaces[0].space_uid;
    let operation_env = OperationEnv {
        db_path: world.config.db_path.clone(),
        lock_dir: world.config.lock_dir.clone(),
    };

    let before = provider.inventory(&scope);
    assert!(
        matches!(before, InventoryOutcome::Complete(ref inventory) if inventory.rows.len() == 1),
        "the production removal seam must begin with one real native Wez Space: {before:?}"
    );
    remove_space(
        &operation_env,
        &provider,
        &scope,
        Backend::Wez,
        space_uid,
        Uuid::new_v4(),
    )
    .unwrap();
    assert_eq!(runner.remove_calls.get(), 1);
    assert_eq!(runner.list_calls.get(), 5);
    assert!(!runner.live.get());

    let registry = Registry::open(world.config.clone()).unwrap();
    assert_eq!(
        registry.space(space_uid).unwrap().lifecycle,
        Lifecycle::Deleted
    );
    let floor = registry
        .intentional_empty_revision(world.instance)
        .unwrap()
        .expect("removing the final Wez Space must publish an intentional-empty floor");
    assert_eq!(floor, registry.authority_head().unwrap().revision);
    assert!(
        world.manifest.registry_revision <= floor,
        "the pre-removal manifest must be at or below the empty floor"
    );
    drop(registry);

    // Model a cold mux-server restart: a new epoch has only its reserved
    // sentinel, while the old complete manifest remains on disk.  The
    // coordinator must reject that manifest before publishing readiness.
    let fresh_epoch = ServerEpoch(Uuid::new_v4());
    let mut mux = InProcessMux::new(&world);
    mux.snapshot = sentinel(fresh_epoch);
    mux.spool = RecoverySpool::new(&world.runtime, fresh_epoch);
    let mut options = world.options(Duration::from_secs(2));
    options.server_epoch = fresh_epoch;
    options.server_start_token = format!("restart-{}", Uuid::new_v4());
    let coordinator = spawn_coordinator(options);
    mux.drive_until(|| coordinator.is_finished());
    let report = coordinator.join().unwrap().unwrap();

    assert_eq!(report.outcome, RecoveryOutcome::NoEligibleManifest);
    assert_eq!(report.restored_nodes, 0);
    assert_eq!(report.generation_uid, None);
    assert!(mux.restore_counts.is_empty());
    assert!(mux.objects.is_empty());
    assert_eq!(mux.snapshot, sentinel(fresh_epoch));
    assert_eq!(mux.snapshot.panes().count(), 1);
    let status: RecoveryStatus =
        serde_json::from_slice(&fs::read(&mux.spool.status).unwrap()).unwrap();
    assert_eq!(status.state, RecoveryStatusState::Ready);
    assert_eq!(status.manifest_id, None);
}

#[test]
fn crash_after_each_native_create_resumes_without_duplicate_nodes() {
    let node_count = World::new(true).manifest.restore_nodes().len();
    for crash_index in 0..node_count {
        let world = World::new(true);
        let crash_path = world.manifest.restore_nodes()[crash_index]
            .manifest_node_path
            .clone();
        let mut mux = InProcessMux::new(&world);
        mux.drop_response_for = Some(crash_path.clone());
        let first_options = world.options(Duration::from_millis(120));
        let request_uid = first_options.request_uid;
        let crashed = spawn_coordinator(first_options);
        mux.drive_until(|| crashed.is_finished());
        assert!(crashed.join().unwrap().is_err(), "{crash_path}");
        let status: RecoveryStatus =
            serde_json::from_slice(&fs::read(&mux.spool.status).unwrap()).unwrap();
        assert_eq!(status.state, RecoveryStatusState::Failed, "{crash_path}");
        let inspection =
            inspect_recovery(world.config.clone(), &world.runtime, world.instance).unwrap();
        assert_eq!(
            inspection.status.as_ref().unwrap().state,
            RecoveryStatusState::Failed
        );
        assert!(inspection.generation.is_some());
        assert!(!inspection.journal.is_empty());
        let control =
            request_recovery_resume(world.config.clone(), &world.runtime, world.instance).unwrap();
        assert_eq!(control.server_epoch, world.epoch);
        let _ = fs::remove_file(&mux.spool.control);

        mux.drop_response_for = None;
        mux.seen = None;
        let _ = fs::remove_file(&mux.spool.command);
        let _ = fs::remove_file(&mux.spool.response);
        let mut resume = world.options(Duration::from_secs(2));
        resume.resume_failed = true;
        // Threads share a process PID, so model the same-holder replay seam;
        // production takeover instead presents a fresh request after the
        // crashed coordinator PID is dead.
        resume.request_uid = request_uid;
        let resumed = spawn_coordinator(resume);
        mux.drive_until(|| resumed.is_finished());
        let report = resumed.join().unwrap().unwrap();
        assert_eq!(report.outcome, RecoveryOutcome::Resumed, "{crash_path}");
        assert_eq!(
            mux.snapshot.panes().count(),
            world.manifest.restore_nodes().len() + 1
        );
        assert!(
            mux.restore_counts.values().all(|count| *count == 1),
            "native restore duplicated after {crash_path}: {:?}",
            mux.restore_counts
        );
    }
}

#[test]
fn abort_crash_after_each_native_remove_retries_without_broad_or_duplicate_deletion() {
    let node_count = World::new(true).manifest.restore_nodes().len();
    for crash_index in 0..node_count {
        let world = World::new(true);
        let crash_path = world.manifest.restore_nodes()[crash_index]
            .manifest_node_path
            .clone();
        let mut mux = InProcessMux::new(&world);
        mux.drop_verify_response = true;
        let failed_options = world.options(Duration::from_millis(120));
        let request_uid = failed_options.request_uid;
        let failed = spawn_coordinator(failed_options);
        mux.drive_until(|| failed.is_finished());
        assert!(failed.join().unwrap().is_err(), "{crash_path}");
        assert_eq!(
            mux.snapshot.panes().count(),
            world.manifest.restore_nodes().len() + 1,
            "the injected verify crash occurs after every native node exists"
        );
        let target_pane_id = mux.objects[&crash_path].pane_id.clone();

        let control =
            request_recovery_abort(world.config.clone(), &world.runtime, world.instance).unwrap();
        assert_eq!(control.action, dmux::recovery::RecoveryControlAction::Abort);
        let _ = fs::remove_file(&mux.spool.control);

        mux.drop_verify_response = false;
        mux.drop_remove_response_for = Some(crash_path.clone());
        mux.dropped = false;
        mux.seen = None;
        let _ = fs::remove_file(&mux.spool.command);
        let _ = fs::remove_file(&mux.spool.response);
        let mut abort = world.options(Duration::from_millis(120));
        abort.abort_failed = true;
        abort.request_uid = request_uid;
        let interrupted = spawn_coordinator(abort.clone());
        mux.drive_until(|| interrupted.is_finished());
        assert!(interrupted.join().unwrap().is_err(), "{crash_path}");
        assert!(
            mux.snapshot
                .panes()
                .all(|(_, _, pane)| pane.pane_id != target_pane_id),
            "the target remove happened before its acknowledgement was lost"
        );

        mux.drop_remove_response_for = None;
        mux.dropped = false;
        mux.seen = None;
        let _ = fs::remove_file(&mux.spool.command);
        let _ = fs::remove_file(&mux.spool.response);
        let retried = spawn_coordinator(abort);
        mux.drive_until(|| retried.is_finished());
        let report = retried.join().unwrap().unwrap();
        assert_eq!(report.outcome, RecoveryOutcome::Aborted, "{crash_path}");
        assert_eq!(mux.snapshot, sentinel(world.epoch), "{crash_path}");
        assert_eq!(mux.remove_counts[&crash_path], 2, "{crash_path}");
        assert!(mux.remove_counts.values().all(|count| *count <= 2));
        assert!(
            mux.remove_effect_counts.values().all(|count| *count == 1),
            "no native pane may be removed twice or by a broad prune: {:?}",
            mux.remove_effect_counts
        );
        assert_eq!(
            mux.remove_effect_counts.len(),
            world.manifest.restore_nodes().len()
        );

        let registry = Registry::open(world.config.clone()).unwrap();
        assert!(
            registry
                .unfinished_recovery_for_instance(world.instance)
                .unwrap()
                .is_none()
        );
        assert!(
            registry
                .intentional_empty_revision(world.instance)
                .unwrap()
                .is_some_and(|floor| floor >= world.manifest.registry_revision)
        );

        // The aborted manifest is now below the intentional-empty floor and
        // cannot resurrect on a later startup.
        mux.seen = None;
        let _ = fs::remove_file(&mux.spool.command);
        let _ = fs::remove_file(&mux.spool.response);
        let later = spawn_coordinator(world.options(Duration::from_secs(2)));
        mux.drive_until(|| later.is_finished());
        assert_eq!(
            later.join().unwrap().unwrap().outcome,
            RecoveryOutcome::NoEligibleManifest
        );
        assert_eq!(mux.snapshot, sentinel(world.epoch));
    }
}

#[test]
fn drift_after_abort_precheck_is_atomically_rejected_before_exact_id_remove() {
    let world = World::new(true);
    let mut mux = InProcessMux::new(&world);
    mux.drop_verify_response = true;
    let failed_options = world.options(Duration::from_millis(120));
    let holder_uid = failed_options.request_uid;
    let failed = spawn_coordinator(failed_options);
    mux.drive_until(|| failed.is_finished());
    assert!(failed.join().unwrap().is_err());
    let pane_count = mux.snapshot.panes().count();

    mux.drop_verify_response = false;
    mux.dropped = false;
    mux.seen = None;
    let _ = fs::remove_file(&mux.spool.command);
    let _ = fs::remove_file(&mux.spool.response);
    mux.inject_unmanaged_before_compare_remove_count = Some(1);
    let mut abort = world.options(Duration::from_secs(2));
    abort.abort_failed = true;
    abort.request_uid = holder_uid;
    let aborting = spawn_coordinator(abort);
    mux.drive_until(|| aborting.is_finished());
    let error = aborting.join().unwrap().unwrap_err();

    assert!(mux.unmanaged_injected);
    assert!(mux.remove_counts.is_empty());
    assert!(mux.remove_effect_counts.is_empty());
    assert_eq!(mux.snapshot.panes().count(), pane_count + 1);
    assert!(
        error
            .to_string()
            .contains("native tree precondition changed"),
        "unexpected abort drift failure: {error}"
    );
}

fn simulate_recovery_process_death(config: &RegistryConfig) -> i64 {
    let registry = Registry::open(config.clone()).unwrap();
    let prior_fence: i64 = registry
        .raw_connection()
        .query_row(
            "SELECT fencing_token FROM leases WHERE state = 'held' AND scope LIKE 'recovery:%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    registry
        .raw_connection()
        .execute(
            "UPDATE leases SET holder_pid = 2147483647 \
             WHERE state = 'held' AND scope LIKE 'recovery:%'",
            [],
        )
        .unwrap();
    prior_fence
}

#[test]
fn hard_process_death_at_command_native_and_ack_boundaries_resumes_under_a_new_fence() {
    for (phase, prefix) in [
        (RecoveryCrashPhase::AfterCommandPublish, "restore:"),
        (RecoveryCrashPhase::AfterResponseRead, "restore:"),
        (RecoveryCrashPhase::AfterBootstrapAck, "ack:"),
    ] {
        let world = World::new(true);
        let first_path = world.manifest.restore_nodes()[0].manifest_node_path.clone();
        let mut mux = InProcessMux::new(&world);
        let mut crashing = world.options(Duration::from_secs(2));
        crashing.crash_point = Some(RecoveryCrashPoint {
            phase,
            action: format!("{prefix}{first_path}"),
        });
        let crashed = spawn_coordinator(crashing);
        mux.drive_until(|| crashed.is_finished());
        assert!(crashed.join().is_err(), "{phase:?}");
        let prior_fence = simulate_recovery_process_death(&world.config);

        let resumed = spawn_coordinator(world.options(Duration::from_secs(2)));
        mux.drive_until(|| resumed.is_finished());
        let report = resumed.join().unwrap().unwrap();
        assert_eq!(report.outcome, RecoveryOutcome::Resumed, "{phase:?}");
        assert_eq!(
            mux.snapshot.panes().count(),
            world.manifest.restore_nodes().len() + 1
        );
        assert!(
            mux.restore_counts.values().all(|count| *count == 1),
            "hard-crash recovery duplicated a native node at {phase:?}: {:?}",
            mux.restore_counts
        );
        let status: RecoveryStatus =
            serde_json::from_slice(&fs::read(&mux.spool.status).unwrap()).unwrap();
        assert_eq!(status.state, RecoveryStatusState::Ready);
        assert!(status.fencing_token.unwrap() > prior_fence);
    }
}

#[test]
fn sigkill_subprocess_releases_kernel_lock_and_resumes_every_durable_boundary() {
    for (phase, phase_token, prefix, hold_command, expected) in [
        (
            RecoveryCrashPhase::AfterCommandPublish,
            "command",
            "restore:",
            true,
            RecoveryOutcome::Resumed,
        ),
        (
            RecoveryCrashPhase::AfterResponseRead,
            "response",
            "restore:",
            false,
            RecoveryOutcome::Resumed,
        ),
        (
            RecoveryCrashPhase::AfterBootstrapAck,
            "ack",
            "ack:",
            false,
            RecoveryOutcome::Resumed,
        ),
        (
            RecoveryCrashPhase::AfterRootCompleted,
            "root",
            "",
            false,
            RecoveryOutcome::AlreadyReady,
        ),
    ] {
        let world = World::new(true);
        let first_path = world.manifest.restore_nodes()[0].manifest_node_path.clone();
        let action = if phase == RecoveryCrashPhase::AfterRootCompleted {
            String::new()
        } else {
            format!("{prefix}{first_path}")
        };
        let marker = world.runtime.join(format!("sigkill-{phase_token}.marker"));
        let mut mux = InProcessMux::new(&world);
        let mut child =
            spawn_recovery_subprocess(&world, phase_token, action.as_str(), marker.as_path());
        let child_pid = child.id();
        kill_at_real_process_marker(
            &mut child,
            &mut mux,
            &marker,
            hold_command.then_some(action.as_str()),
        );

        let registry = Registry::open(world.config.clone()).unwrap();
        let killed_lease = registry
            .current_lease(&LeaseScope::Recovery(world.instance))
            .unwrap()
            .expect("SIGKILL deliberately leaves the durable held lease row");
        assert_eq!(killed_lease.holder_pid, Some(child_pid as i32));
        let prior_fence = killed_lease.fencing_token;
        let kernel = locks::try_acquire(
            &world.config.lock_dir,
            LockScope::BackendInstance(world.instance),
            LockMode::Exclusive,
        )
        .unwrap()
        .expect("the OS must release the child coordinator's kernel lock on SIGKILL");
        drop(kernel);

        let takeover = spawn_coordinator(world.options(Duration::from_secs(3)));
        mux.drive_until(|| takeover.is_finished());
        let report = takeover.join().unwrap().unwrap();
        assert_eq!(report.outcome, expected, "{phase:?}");
        assert_eq!(
            mux.snapshot.panes().count(),
            world.manifest.restore_nodes().len() + 1,
            "{phase:?}"
        );
        assert!(
            mux.restore_counts.values().all(|count| *count == 1),
            "SIGKILL boundary duplicated a native node at {phase:?}: {:?}",
            mux.restore_counts
        );
        let status: RecoveryStatus =
            serde_json::from_slice(&fs::read(&mux.spool.status).unwrap()).unwrap();
        assert_eq!(status.state, RecoveryStatusState::Ready);
        assert!(status.fencing_token.unwrap() > prior_fence);
    }
}

#[test]
fn hard_death_after_completed_root_republishes_ready_without_restoring_again() {
    let world = World::new(true);
    let mut mux = InProcessMux::new(&world);
    let mut crashing = world.options(Duration::from_secs(2));
    crashing.crash_point = Some(RecoveryCrashPoint {
        phase: RecoveryCrashPhase::AfterRootCompleted,
        action: String::new(),
    });
    let crashed = spawn_coordinator(crashing);
    mux.drive_until(|| crashed.is_finished());
    assert!(crashed.join().is_err());
    assert!(mux.restore_counts.values().all(|count| *count == 1));
    let mut failed_sidecar: RecoveryStatus =
        serde_json::from_slice(&fs::read(&mux.spool.status).unwrap()).unwrap();
    failed_sidecar.state = RecoveryStatusState::Failed;
    failed_sidecar.error = Some("injected final ready publication failure".into());
    fs::write(
        &mux.spool.status,
        serde_json::to_vec(&failed_sidecar).unwrap(),
    )
    .unwrap();
    let inspection =
        inspect_recovery(world.config.clone(), &world.runtime, world.instance).unwrap();
    assert_eq!(
        inspection
            .generation
            .as_ref()
            .expect("failed sidecar must surface its completed journal")
            .generation_uid,
        failed_sidecar.generation_uid.unwrap()
    );
    let control =
        request_recovery_resume(world.config.clone(), &world.runtime, world.instance).unwrap();
    assert_eq!(
        control.action,
        dmux::recovery::RecoveryControlAction::Resume
    );
    let _ = fs::remove_file(&mux.spool.control);
    let prior_fence = simulate_recovery_process_death(&world.config);

    let mut resume = world.options(Duration::from_secs(2));
    resume.resume_failed = true;
    let takeover = spawn_coordinator(resume);
    mux.drive_until(|| takeover.is_finished());
    let report = takeover.join().unwrap().unwrap();
    assert_eq!(report.outcome, RecoveryOutcome::AlreadyReady);
    assert!(mux.restore_counts.values().all(|count| *count == 1));
    let status: RecoveryStatus =
        serde_json::from_slice(&fs::read(&mux.spool.status).unwrap()).unwrap();
    assert_eq!(status.state, RecoveryStatusState::Ready);
    assert!(status.fencing_token.unwrap() > prior_fence);
}

#[test]
fn completed_root_takeover_rejects_two_manifest_groups_collapsed_into_one_tab() {
    let world = World::new(true);
    let mut mux = InProcessMux::new(&world);
    let mut crashing = world.options(Duration::from_secs(2));
    crashing.crash_point = Some(RecoveryCrashPoint {
        phase: RecoveryCrashPhase::AfterRootCompleted,
        action: String::new(),
    });
    let crashed = spawn_coordinator(crashing);
    mux.drive_until(|| crashed.is_finished());
    assert!(crashed.join().is_err());

    let registry = Registry::open(world.config.clone()).unwrap();
    let (spec, _) = registry
        .completed_recovery(world.instance, world.epoch)
        .unwrap()
        .expect("the injected crash follows the completed-root transaction");
    drop(registry);
    collapse_completed_second_group_into_first_tab(&world, &mut mux, spec.generation_uid);
    simulate_recovery_process_death(&world.config);

    let takeover = spawn_coordinator(world.options(Duration::from_secs(2)));
    mux.drive_until(|| takeover.is_finished());
    let error = takeover.join().unwrap().unwrap_err();
    assert!(
        error.to_string().contains("maps multiple Groups"),
        "unexpected completed-takeover topology failure: {error}"
    );
    assert!(mux.restore_counts.values().all(|count| *count == 1));
    let status: RecoveryStatus =
        serde_json::from_slice(&fs::read(&mux.spool.status).unwrap()).unwrap();
    assert_eq!(status.state, RecoveryStatusState::Failed);
}

#[test]
fn mux_restart_retires_visible_old_epoch_generation_then_restores_fresh() {
    let world = World::new(true);
    let first_path = world.manifest.restore_nodes()[0].manifest_node_path.clone();
    let mut mux = InProcessMux::new(&world);
    let mut crashing = world.options(Duration::from_secs(2));
    crashing.crash_point = Some(RecoveryCrashPoint {
        phase: RecoveryCrashPhase::AfterResponseRead,
        action: format!("restore:{first_path}"),
    });
    let crashed = spawn_coordinator(crashing);
    mux.drive_until(|| crashed.is_finished());
    assert!(crashed.join().is_err());
    simulate_recovery_process_death(&world.config);

    // A mux-server restart destroys every old native object and starts with
    // one fresh-epoch sentinel.  The durable old generation remains visible
    // instance-wide and must be retired under the new fence before begin.
    let new_epoch = ServerEpoch(Uuid::new_v4());
    mux.snapshot = sentinel(new_epoch);
    mux.objects.clear();
    mux.helpers.clear();
    mux.seen = None;
    mux.spool = RecoverySpool::new(&world.runtime, new_epoch);
    let mut restarted = world.options(Duration::from_secs(2));
    restarted.server_epoch = new_epoch;
    restarted.server_start_token = format!("restart-{}", Uuid::new_v4());
    let takeover = spawn_coordinator(restarted);
    mux.drive_until(|| takeover.is_finished());
    let report = takeover.join().unwrap().unwrap();
    assert_eq!(report.outcome, RecoveryOutcome::Restored);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|line| line.contains("retired stale recovery generation"))
    );
    assert_eq!(
        mux.snapshot.panes().count(),
        world.manifest.restore_nodes().len() + 1
    );
    let registry = Registry::open(world.config.clone()).unwrap();
    assert!(
        registry
            .unfinished_recovery_for_instance(world.instance)
            .unwrap()
            .is_none()
    );
    assert!(
        registry
            .recovery_rows(report.generation_uid.unwrap())
            .unwrap()
            .iter()
            .all(|row| row.node_state == RecoveryNodeState::Completed)
    );
    assert_eq!(
        registry.intentional_empty_revision(world.instance).unwrap(),
        None
    );
}

#[test]
fn failed_old_epoch_stays_blocked_until_explicit_resume_or_abort() {
    let world = World::new(true);
    let crash_path = world.manifest.restore_nodes()[0].manifest_node_path.clone();
    let mut mux = InProcessMux::new(&world);
    mux.drop_response_for = Some(crash_path);
    let failed = spawn_coordinator(world.options(Duration::from_millis(120)));
    mux.drive_until(|| failed.is_finished());
    assert!(failed.join().unwrap().is_err());
    simulate_recovery_process_death(&world.config);

    let new_epoch = ServerEpoch(Uuid::new_v4());
    mux.snapshot = sentinel(new_epoch);
    mux.objects.clear();
    mux.helpers.clear();
    mux.seen = None;
    mux.drop_response_for = None;
    mux.dropped = false;
    mux.spool = RecoverySpool::new(&world.runtime, new_epoch);
    let mut automatic = world.options(Duration::from_secs(2));
    automatic.server_epoch = new_epoch;
    automatic.server_start_token = format!("restart-{}", Uuid::new_v4());
    let takeover_uid = automatic.request_uid;
    let blocked = spawn_coordinator(automatic.clone());
    mux.drive_until(|| blocked.is_finished());
    let error = blocked.join().unwrap().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("explicit recovery resume or abort")
    );
    let registry = Registry::open(world.config.clone()).unwrap();
    let (stale, rows) = registry
        .unfinished_recovery_for_instance(world.instance)
        .unwrap()
        .expect("automatic startup must retain the failed old generation");
    assert_ne!(stale.server_epoch, new_epoch);
    assert_eq!(
        rows.iter()
            .find(|row| row.manifest_node_path == dmux::registry::recovery::RECOVERY_GENERATION_PATH)
            .unwrap()
            .node_state,
        RecoveryNodeState::Failed
    );

    let mut resume = automatic;
    resume.resume_failed = true;
    resume.request_uid = takeover_uid;
    mux.seen = None;
    let _ = fs::remove_file(&mux.spool.command);
    let _ = fs::remove_file(&mux.spool.response);
    let resumed = spawn_coordinator(resume);
    mux.drive_until(|| resumed.is_finished());
    assert_eq!(
        resumed.join().unwrap().unwrap().outcome,
        RecoveryOutcome::Restored
    );
    assert_eq!(
        mux.snapshot.panes().count(),
        world.manifest.restore_nodes().len() + 1
    );

    // A separate failed old epoch takes the explicit abort path: sentinel
    // proof plus one atomic empty-floor/journal transaction, with no restore.
    let abort_world = World::new(true);
    let abort_path = abort_world.manifest.restore_nodes()[0]
        .manifest_node_path
        .clone();
    let mut abort_mux = InProcessMux::new(&abort_world);
    abort_mux.drop_response_for = Some(abort_path);
    let failed = spawn_coordinator(abort_world.options(Duration::from_millis(120)));
    abort_mux.drive_until(|| failed.is_finished());
    assert!(failed.join().unwrap().is_err());
    simulate_recovery_process_death(&abort_world.config);
    let abort_epoch = ServerEpoch(Uuid::new_v4());
    abort_mux.snapshot = sentinel(abort_epoch);
    abort_mux.objects.clear();
    abort_mux.helpers.clear();
    abort_mux.seen = None;
    abort_mux.spool = RecoverySpool::new(&abort_world.runtime, abort_epoch);
    let mut abort = abort_world.options(Duration::from_secs(2));
    abort.server_epoch = abort_epoch;
    abort.server_start_token = format!("restart-{}", Uuid::new_v4());
    abort.abort_failed = true;
    let aborted = spawn_coordinator(abort);
    abort_mux.drive_until(|| aborted.is_finished());
    assert_eq!(
        aborted.join().unwrap().unwrap().outcome,
        RecoveryOutcome::Aborted
    );
    let registry = Registry::open(abort_world.config.clone()).unwrap();
    assert!(
        registry
            .unfinished_recovery_for_instance(abort_world.instance)
            .unwrap()
            .is_none()
    );
    assert!(
        registry
            .intentional_empty_revision(abort_world.instance)
            .unwrap()
            .is_some()
    );
    assert_eq!(abort_mux.snapshot, sentinel(abort_epoch));
}

#[test]
fn handled_prepare_failure_and_missing_unfinished_manifest_stay_durably_failed() {
    let world = World::new(true);
    let mut mux = InProcessMux::new(&world);
    mux.reject_prepare = true;
    let first_options = world.options(Duration::from_secs(2));
    let request_uid = first_options.request_uid;
    let failed = spawn_coordinator(first_options);
    mux.drive_until(|| failed.is_finished());
    assert!(failed.join().unwrap().is_err());
    let inspection =
        inspect_recovery(world.config.clone(), &world.runtime, world.instance).unwrap();
    assert_eq!(
        inspection
            .generation
            .as_ref()
            .expect("Prepare follows durable begin")
            .generation_uid,
        inspection.journal[0].generation_uid
    );
    assert_eq!(
        inspection
            .journal
            .iter()
            .find(|row| row.manifest_node_path == dmux::registry::recovery::RECOVERY_GENERATION_PATH)
            .unwrap()
            .node_state,
        RecoveryNodeState::Failed
    );

    fs::remove_file(world.manifests.join("manifest.json")).unwrap();
    mux.reject_prepare = false;
    mux.seen = None;
    let _ = fs::remove_file(&mux.spool.command);
    let _ = fs::remove_file(&mux.spool.response);
    let mut resume = world.options(Duration::from_secs(2));
    resume.resume_failed = true;
    resume.request_uid = request_uid;
    let missing = spawn_coordinator(resume);
    mux.drive_until(|| missing.is_finished());
    let error = missing.join().unwrap().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("missing, corrupt, or ineligible")
    );
    let status: RecoveryStatus =
        serde_json::from_slice(&fs::read(&mux.spool.status).unwrap()).unwrap();
    assert_eq!(status.state, RecoveryStatusState::Failed);
    let registry = Registry::open(world.config.clone()).unwrap();
    let (_, rows) = registry
        .unfinished_recovery_for_instance(world.instance)
        .unwrap()
        .expect("missing manifest must not publish Ready over a durable blocker");
    assert_eq!(
        rows.iter()
            .find(|row| row.manifest_node_path == dmux::registry::recovery::RECOVERY_GENERATION_PATH)
            .unwrap()
            .node_state,
        RecoveryNodeState::Failed
    );
}

fn wait_for_plan(path: &Path) -> SnapshotCapturePlan {
    let started = Instant::now();
    loop {
        if let Ok(bytes) = fs::read(path)
            && let Ok(plan) = serde_json::from_slice(&bytes)
        {
            return plan;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "snapshot plan timed out"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn snapshot_candidate_ids_and_preplanted_plan_links_fail_before_the_fence() {
    let world = World::new(false);
    let traversal = publish_snapshot_manifest_for_test(
        world.config.clone(),
        world.instance,
        "../escape",
        &world.manifests,
        world.epoch,
    )
    .unwrap_err();
    assert!(traversal.to_string().contains("candidate ID"));

    let candidate_id = format!(".capture-{}-3-1", world.epoch.0);
    let victim = world.manifests.join("victim");
    write_private(&victim, b"victim-must-survive");
    symlink(
        &victim,
        world.manifests.join(format!("{candidate_id}.plan")),
    )
    .unwrap();
    let error = publish_snapshot_manifest_for_test(
        world.config.clone(),
        world.instance,
        &candidate_id,
        &world.manifests,
        world.epoch,
    )
    .unwrap_err();
    assert!(error.to_string().contains("recovery I/O"), "{error}");
    assert_eq!(fs::read(victim).unwrap(), b"victim-must-survive");
    assert!(
        Registry::open(world.config.clone())
            .unwrap()
            .current_lease(&LeaseScope::Snapshot(world.instance))
            .unwrap()
            .is_none(),
        "a hostile plan entry must fail before snapshot lease acquisition"
    );
}

#[test]
fn snapshot_capture_holds_common_lock_and_rejects_an_omitted_planned_space() {
    let world = World::new(false);
    let candidate_id = format!(".capture-{}-1-1", world.epoch.0);
    let candidate = world.manifests.join(&candidate_id);
    let destination = world
        .manifests
        .join(format!("manifest-{}-1-1.json", world.epoch.0));
    let plan_path = snapshot_capture_plan_path(&candidate);
    let config = world.config.clone();
    let instance = world.instance;
    let manifests = world.manifests.clone();
    let epoch = world.epoch;
    let publisher = thread::spawn(move || {
        publish_snapshot_manifest_for_test(config, instance, &candidate_id, &manifests, epoch)
    });
    let plan = wait_for_plan(&plan_path);
    assert_eq!(plan.spaces.len(), 1);
    assert!(
        locks::try_acquire(
            &world.config.lock_dir,
            LockScope::BackendInstance(world.instance),
            LockMode::Exclusive,
        )
        .unwrap()
        .is_none(),
        "snapshot capture must hold the common backend lock"
    );
    let mut omitted = world.manifest.clone();
    omitted.manifest_id = plan.manifest_id;
    omitted.registry_revision = plan.registry_revision;
    omitted.generated_at = plan.generated_at;
    omitted.spaces.clear();
    write_private(&candidate, serde_json::to_vec(&omitted).unwrap());
    let error = publisher.join().unwrap().unwrap_err();
    assert!(
        error.to_string().contains("exact sorted all-Space"),
        "{error}"
    );
    assert!(!destination.exists());
    assert!(!candidate.exists());
    assert!(!plan_path.exists());
}

#[test]
fn snapshot_capture_publishes_the_exact_fenced_plan_atomically() {
    let world = World::new(false);
    let candidate_id = format!(".capture-{}-2-1", world.epoch.0);
    let candidate = world.manifests.join(&candidate_id);
    let destination = world
        .manifests
        .join(format!("manifest-{}-2-1.json", world.epoch.0));
    let plan_path = snapshot_capture_plan_path(&candidate);
    let config = world.config.clone();
    let instance = world.instance;
    let manifests = world.manifests.clone();
    let epoch = world.epoch;
    let publisher = thread::spawn(move || {
        publish_snapshot_manifest_for_test(config, instance, &candidate_id, &manifests, epoch)
    });
    let plan = wait_for_plan(&plan_path);
    let mut manifest = world.manifest.clone();
    manifest.manifest_id = plan.manifest_id.clone();
    manifest.registry_revision = plan.registry_revision;
    manifest.generated_at = plan.generated_at;
    write_private(&candidate, serde_json::to_vec(&manifest).unwrap());
    let report = publisher.join().unwrap().unwrap();
    assert_eq!(report.manifest_id, plan.manifest_id);
    let published: RecoveryManifest =
        serde_json::from_slice(&fs::read(&destination).unwrap()).unwrap();
    assert_eq!(published, manifest);
    let metadata = fs::metadata(&destination).unwrap();
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    assert_eq!(metadata.nlink(), 1);
}
