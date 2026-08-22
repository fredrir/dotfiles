//! P6 gate: fenced Space operations end to end on a real scratch tmux
//! server, driving the REAL pane-bootstrap helper through the broker
//! protocol. Root-owned. Covers: scratch one-window create with marker
//! export, acknowledgement-loss replay, rename, remove with tombstone
//! non-reuse, and failure injection (unmanaged name conflict, wrong epoch
//! abort with consumed number).

use std::process::Command;
use std::time::{Duration, Instant};

use dmux::backend::tmux::TmuxProvider;
use dmux::backend::{InventoryScope, SplitDirection};
use dmux::bootstrap::MarkerContext;
use dmux::model::{Backend, ServerEpoch};
use dmux::operations::{
    CreateRequest, GroupNewRequest, OpError, OperationEnv, OwnerCreateTarget, SplitNewRequest,
    context_read, create_space, create_space_owner_fenced, group_activate_exact, group_new,
    hierarchy, remove_space, rename_space, resume_remove_space, split_direction, split_new,
    split_resize, split_zoom, tmux_bootstrap, validate_marker_context,
};
use dmux::refs::{ChildRefShape, parse_ref};
use dmux::registry::{Registry, RegistryConfig};
use uuid::Uuid;

struct Scratch {
    ns: String,
    data: tempfile::TempDir,
    locks: tempfile::TempDir,
}

impl Scratch {
    /// Starts the scratch server with DMUX_RUNTIME_DIR in its environment,
    /// so helper panes resolve the broker's FIFO directory.
    fn new(tag: &str) -> Scratch {
        let s = Scratch {
            ns: format!("dmux-p6-{tag}-{}", std::process::id()),
            data: tempfile::tempdir().unwrap(),
            locks: tempfile::tempdir().unwrap(),
        };
        let out = Command::new("tmux")
            .args([
                "-L",
                &s.ns,
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                "seed",
            ])
            .env("DMUX_RUNTIME_DIR", s.locks.path())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        s
    }

    fn env(&self) -> OperationEnv {
        OperationEnv {
            db_path: self.data.path().join("registry.sqlite3"),
            lock_dir: self.locks.path().to_path_buf(),
        }
    }

    fn epoch(&self) -> ServerEpoch {
        match tmux_bootstrap(&self.env(), &self.ns).unwrap() {
            dmux::operations::TmuxBootstrapOutcome::Bootstrapped { epoch }
            | dmux::operations::TmuxBootstrapOutcome::AlreadyBound { epoch }
            | dmux::operations::TmuxBootstrapOutcome::Rebound { epoch, .. } => epoch,
        }
    }

    fn scope(&self, epoch: ServerEpoch) -> InventoryScope {
        InventoryScope::managed(Backend::Tmux, self.ns.clone(), epoch)
    }

    fn tmux(&self, args: &[&str]) -> String {
        let out = Command::new("tmux")
            .args(["-L", &self.ns])
            .args(args)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn session_names(&self) -> Vec<String> {
        self.tmux(&["list-sessions", "-F", "#{session_name}"])
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn registry(&self) -> Registry {
        Registry::open(RegistryConfig::new(
            self.data.path().join("registry.sqlite3"),
            self.locks.path(),
        ))
        .unwrap()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.ns, "kill-server"])
            .output();
    }
}

fn request(_s: &Scratch, name: &str, marker: &std::path::Path) -> CreateRequest {
    CreateRequest {
        request_uid: Uuid::new_v4(),
        name: name.to_string(),
        cwd: None,
        program: vec![
            "sh".into(),
            "-c".into(),
            format!(
                "printf %s \"$DMUX_SPACE_UID\" > {} && exec sleep 300",
                marker.display()
            ),
        ],
        helper_bin: env!("CARGO_BIN_EXE_pane-bootstrap").to_string(),
    }
}

fn child_shape(child_ref: &str) -> ChildRefShape {
    parse_ref(&format!("x/{child_ref}"))
        .expect("child ref parses")
        .child
        .expect("child ref is present")
}

#[test]
fn create_replay_rename_remove_full_cycle() {
    let s = Scratch::new("cycle");
    let epoch = s.epoch();
    let scope = s.scope(epoch);
    let provider = TmuxProvider::new(s.ns.clone());
    let marker = s.data.path().join("marker");

    // --- Create: one-window scratch Space with the real helper. ---
    let req = request(&s, "proj", &marker);
    let created = create_space(&s.env(), &provider, &scope, Backend::Tmux, &req).unwrap();
    assert!(!created.replayed);
    assert!(
        created.native_token.starts_with('$'),
        "tmux binding is the session id"
    );
    assert!(created.group_ref.starts_with('g') && created.split_ref.starts_with('p'));
    assert!(s.session_names().contains(&"proj".to_string()));

    // The helper exec'd the user program with the marker env exported.
    let deadline = Instant::now() + Duration::from_secs(10);
    let stamped = loop {
        if let Ok(text) = std::fs::read_to_string(&marker) {
            break text;
        }
        assert!(Instant::now() < deadline, "helper never exec'd the program");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(stamped, created.space_uid.0.to_string());

    // A GUI marker is only a locator. Revalidation must bind its complete
    // tuple to the authority row, published epoch, and exact live child
    // parentage before keys/status/presentation may consume it.
    let identity = s.registry().identity().unwrap();
    let marker_context = MarkerContext {
        host_uid: identity.host_uid,
        space_uid: created.space_uid,
        space_no: created.space_no,
        backend: Backend::Tmux,
        domain: None,
        server_epoch: epoch,
        group_ref: created.group_ref.clone(),
        split_ref: created.split_ref.clone(),
    };
    let validated = validate_marker_context(&s.env(), &provider, &scope, &marker_context).unwrap();
    assert_eq!(validated.logical_name, "proj");
    assert_eq!((validated.group_count, validated.split_count), (1, 1));
    assert_eq!(validated.context, marker_context);
    let mut mismatched_parent = marker_context.clone();
    mismatched_parent.group_ref = format!("g{}.tx-999999", epoch.0);
    assert!(matches!(
        validate_marker_context(&s.env(), &provider, &scope, &mismatched_parent),
        Err(OpError::StaleRef(_))
    ));

    // Registry: active + bound + bootstrap completed.
    let registry = s.registry();
    let space = registry.space(created.space_uid).unwrap();
    assert_eq!(space.lifecycle, dmux::model::Lifecycle::Active);
    let binding = registry
        .current_binding(created.space_uid)
        .unwrap()
        .unwrap();
    assert_eq!(binding.native_token, created.native_token);
    drop(registry);

    // --- Acknowledgement-loss replay: same request UID, no second spawn. ---
    let replayed = create_space(&s.env(), &provider, &scope, Backend::Tmux, &req).unwrap();
    assert!(replayed.replayed);
    assert_eq!(replayed.space_uid, created.space_uid);
    assert_eq!(
        s.session_names().iter().filter(|n| *n == "proj").count(),
        1,
        "replay must not create a second session"
    );

    // --- Rename: native + registry atomically journaled. ---
    rename_space(
        &s.env(),
        &provider,
        &scope,
        Backend::Tmux,
        created.space_uid,
        "renamed",
        Uuid::new_v4(),
    )
    .unwrap();
    let names = s.session_names();
    assert!(names.contains(&"renamed".to_string()) && !names.contains(&"proj".to_string()));
    assert_eq!(
        s.registry().space(created.space_uid).unwrap().logical_name,
        "renamed"
    );

    // --- Remove: verified absence, tombstone, no identity reuse. ---
    remove_space(
        &s.env(),
        &provider,
        &scope,
        Backend::Tmux,
        created.space_uid,
        Uuid::new_v4(),
    )
    .unwrap();
    assert!(!s.session_names().contains(&"renamed".to_string()));
    assert_eq!(
        s.registry().space(created.space_uid).unwrap().lifecycle,
        dmux::model::Lifecycle::Deleted
    );

    // Recreate under the old original name: fresh identity, larger number.
    let marker2 = s.data.path().join("marker2");
    let again = create_space(
        &s.env(),
        &provider,
        &scope,
        Backend::Tmux,
        &request(&s, "proj", &marker2),
    )
    .unwrap();
    assert_ne!(again.space_uid, created.space_uid);
    assert!(again.space_no > created.space_no);
}

#[test]
fn failure_injection_conflict_and_wrong_epoch() {
    let s = Scratch::new("fail");
    let epoch = s.epoch();
    let provider = TmuxProvider::new(s.ns.clone());
    let marker = s.data.path().join("m");

    // Unmanaged same-name session blocks creation (the seed session).
    let err = create_space(
        &s.env(),
        &provider,
        &s.scope(epoch),
        Backend::Tmux,
        &request(&s, "seed", &marker),
    )
    .unwrap_err();
    assert!(matches!(err, OpError::NameConflict(_)), "{err}");

    // Wrong expected epoch is refused at the complete-inventory guard,
    // BEFORE any reservation: creation fails closed (plan §2.10) and no
    // identity is consumed.
    let wrong = s.scope(ServerEpoch(Uuid::from_u128(42)));
    let err = create_space(
        &s.env(),
        &provider,
        &wrong,
        Backend::Tmux,
        &request(&s, "epochy", &marker),
    )
    .unwrap_err();
    assert!(matches!(err, OpError::Indeterminate(_)), "{err}");
    assert!(!s.session_names().contains(&"epochy".to_string()));

    // A failure AFTER reservation (helper exits instantly, pane dies before
    // the handshake) aborts the create and consumes its SpaceNo.
    let mut dead_helper = request(&s, "aborty", &marker);
    dead_helper.helper_bin = "/usr/bin/true".into();
    let err = create_space(
        &s.env(),
        &provider,
        &s.scope(epoch),
        Backend::Tmux,
        &dead_helper,
    )
    .unwrap_err();
    assert!(
        matches!(err, OpError::Provider(_) | OpError::Bootstrap(_)),
        "post-reservation failure must be typed: {err}"
    );
    assert!(!s.session_names().contains(&"aborty".to_string()));

    // The aborted reservation consumed its SpaceNo: the next create skips it.
    let ok = create_space(
        &s.env(),
        &provider,
        &s.scope(epoch),
        Backend::Tmux,
        &request(&s, "after-abort", &s.data.path().join("m2")),
    )
    .unwrap();
    assert!(
        ok.space_no.get() >= 2,
        "aborted reservation must consume a number"
    );
    // Durable conflict: the live name guard also blocks managed duplicates.
    let err = create_space(
        &s.env(),
        &provider,
        &s.scope(epoch),
        Backend::Tmux,
        &request(&s, "after-abort", &s.data.path().join("m3")),
    )
    .unwrap_err();
    assert!(matches!(err, OpError::NameConflict(_)), "{err}");
}

#[test]
fn remove_resume_requires_the_exact_journal_owner_and_uses_the_fenced_path() {
    let s = Scratch::new("remove-resume");
    let epoch = s.epoch();
    let scope = s.scope(epoch);
    let provider = TmuxProvider::new(s.ns.clone());
    let created = create_space(
        &s.env(),
        &provider,
        &scope,
        Backend::Tmux,
        &request(&s, "resume-me", &s.data.path().join("resume-marker")),
    )
    .unwrap();

    // Simulate a crash after durable deleting intent but before the native
    // remove/ack. Only this operation/request pair may resume it.
    let request_uid = Uuid::new_v4();
    let operation_uid = s
        .registry()
        .begin_remove(created.space_uid, request_uid)
        .unwrap();
    let wrong = resume_remove_space(
        &s.env(),
        &provider,
        &scope,
        Backend::Tmux,
        created.space_uid,
        Uuid::new_v4(),
        operation_uid,
    )
    .unwrap_err();
    assert!(matches!(wrong, OpError::Refused(_)), "{wrong}");
    assert!(s.session_names().contains(&"resume-me".to_string()));

    resume_remove_space(
        &s.env(),
        &provider,
        &scope,
        Backend::Tmux,
        created.space_uid,
        request_uid,
        operation_uid,
    )
    .unwrap();
    assert!(!s.session_names().contains(&"resume-me".to_string()));
    assert_eq!(
        s.registry().space(created.space_uid).unwrap().lifecycle,
        dmux::model::Lifecycle::Deleted
    );
}

#[test]
fn exact_child_actions_are_fenced_and_ack_replay_does_not_toggle_twice() {
    let s = Scratch::new("exact-actions");
    let epoch = s.epoch();
    let scope = s.scope(epoch);
    let provider = TmuxProvider::new(s.ns.clone());
    let created = create_space(
        &s.env(),
        &provider,
        &scope,
        Backend::Tmux,
        &request(&s, "actions", &s.data.path().join("action-marker")),
    )
    .unwrap();

    let root_group = child_shape(&created.group_ref);
    let root_split = child_shape(&created.split_ref);
    let second = split_new(
        &s.env(),
        &provider,
        &scope,
        &SplitNewRequest {
            request_uid: Uuid::new_v4(),
            space_uid: created.space_uid,
            group: root_group.clone(),
            direction: SplitDirection::Right,
            percent: Some(40),
            cwd: None,
            program: vec!["sh".into(), "-c".into(), "exec sleep 300".into()],
            helper_bin: env!("CARGO_BIN_EXE_pane-bootstrap").into(),
        },
    )
    .unwrap();
    let second_split = child_shape(&second.split_ref);

    let activate_uid = Uuid::new_v4();
    let activated = group_activate_exact(
        &s.env(),
        &provider,
        &scope,
        created.space_uid,
        &root_group,
        activate_uid,
    )
    .unwrap();
    assert_eq!(activated.group_ref, created.group_ref);
    assert!(!activated.replayed);
    assert!(
        group_activate_exact(
            &s.env(),
            &provider,
            &scope,
            created.space_uid,
            &root_group,
            activate_uid,
        )
        .unwrap()
        .replayed
    );

    let selected = split_direction(
        &s.env(),
        &provider,
        &scope,
        created.space_uid,
        &second_split,
        SplitDirection::Left,
        Uuid::new_v4(),
    )
    .unwrap();
    assert_eq!(selected.group_ref, created.group_ref);
    assert_eq!(
        selected.split_ref.as_deref(),
        Some(created.split_ref.as_str())
    );

    let resized = split_resize(
        &s.env(),
        &provider,
        &scope,
        created.space_uid,
        &second_split,
        SplitDirection::Left,
        3,
        Uuid::new_v4(),
    )
    .unwrap();
    assert_eq!(resized.split_ref, second.split_ref);

    // Zoom is deliberately non-idempotent at the backend. Once the first
    // result is durably recorded, an acknowledgement-loss retry must replay
    // the result instead of invoking tmux's toggle a second time.
    let zoom_uid = Uuid::new_v4();
    let zoomed = split_zoom(
        &s.env(),
        &provider,
        &scope,
        created.space_uid,
        &second_split,
        zoom_uid,
    )
    .unwrap();
    assert!(zoomed.zoomed);
    assert_eq!(
        s.tmux(&["display-message", "-p", "#{window_zoomed_flag}"])
            .trim(),
        "1"
    );
    let replayed = split_zoom(
        &s.env(),
        &provider,
        &scope,
        created.space_uid,
        &second_split,
        zoom_uid,
    )
    .unwrap();
    assert!(replayed.replayed && replayed.zoomed);
    assert_eq!(
        s.tmux(&["display-message", "-p", "#{window_zoomed_flag}"])
            .trim(),
        "1"
    );

    let mut stale = root_split;
    stale.epoch = ServerEpoch(Uuid::new_v4());
    assert!(matches!(
        split_zoom(
            &s.env(),
            &provider,
            &scope,
            created.space_uid,
            &stale,
            Uuid::new_v4(),
        ),
        Err(OpError::StaleRef(_))
    ));
}

// ---------------------------------------------------------------------------
// Scripted-provider regressions at the operations layer (ADR 012 WS-A.8,
// WS-A.10, WS-A.11, WS-A.12; review report 08 §7 "call-chain only" items
// operations.rs:2243/2277 and registry/mod.rs:2812). No native server is
// involved: the provider answers exactly what each case needs and records
// every call, and the assertions are about what the operations layer wrote
// to the registry and what it handed the provider — nothing else.

mod scripted {
    use std::cell::RefCell;

    use dmux::backend::{
        Capabilities, CreateSpec, InventoryOutcome, InventoryScope, NativeBinding, NativeGroupRow,
        NativeInventory, NativeSpaceRow, NativeSplitRow, NormalizePlan, PresentationTarget,
        Provider, ProviderError, ProviderResult, SplitSpec,
    };
    use dmux::model::{Backend, ProviderHandle, ServerEpoch};

    /// One scripted backend. `inventory` answers `Complete` under `epoch`
    /// with `rows`; every mutation records its call and the binding epoch it
    /// was handed, then fails typed so the operations layer aborts instead of
    /// waiting on a helper that does not exist. `split_list` mirrors the
    /// adapters' `required_epoch`/`required_action_epoch`: it refuses an
    /// unpinned scope — the refusal `operations::group_new` used to reach
    /// only after its mutation had landed (ADR 012 §3.4).
    pub struct Script {
        pub backend: Backend,
        pub epoch: ServerEpoch,
        pub rows: Vec<NativeSpaceRow>,
        pub calls: RefCell<Vec<&'static str>>,
        pub handed_epochs: RefCell<Vec<ServerEpoch>>,
    }

    impl Script {
        pub fn new(backend: Backend, epoch: ServerEpoch, rows: Vec<NativeSpaceRow>) -> Script {
            Script {
                backend,
                epoch,
                rows,
                calls: RefCell::new(Vec::new()),
                handed_epochs: RefCell::new(Vec::new()),
            }
        }

        pub fn calls(&self) -> Vec<&'static str> {
            self.calls.borrow().clone()
        }

        /// The `NativeBinding.server_epoch` values the provider was handed,
        /// in call order.
        pub fn handed_epochs(&self) -> Vec<ServerEpoch> {
            self.handed_epochs.borrow().clone()
        }

        fn note(&self, call: &'static str) {
            self.calls.borrow_mut().push(call);
        }

        fn refuse(&self, call: &'static str) -> ProviderError {
            ProviderError::NativeFailure {
                detail: format!("scripted {call}: reached the provider"),
            }
        }
    }

    /// One Space row with one Group holding one Split, as a fresh create
    /// leaves it.
    pub fn one_window(token: &str, handle: fn(u64) -> ProviderHandle) -> NativeSpaceRow {
        NativeSpaceRow {
            native_token: token.into(),
            native_name: token.into(),
            groups: vec![NativeGroupRow {
                handle: handle(1),
                title: None,
                splits: vec![NativeSplitRow {
                    handle: handle(1),
                    title: None,
                    cwd: None,
                }],
            }],
            multi_window: false,
        }
    }

    impl Provider for Script {
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                backend: self.backend,
                cas_rename: false,
                probed: Vec::new(),
            }
        }

        fn inventory(&self, _scope: &InventoryScope) -> InventoryOutcome {
            self.note("inventory");
            InventoryOutcome::Complete(NativeInventory {
                server_epoch: Some(self.epoch),
                rows: self.rows.clone(),
            })
        }

        fn create(
            &self,
            _scope: &InventoryScope,
            _spec: &CreateSpec,
        ) -> ProviderResult<NativeBinding> {
            self.note("create");
            Err(self.refuse("create"))
        }

        fn prepare_presentation(
            &self,
            _scope: &InventoryScope,
            _binding: &NativeBinding,
            _child: Option<&ProviderHandle>,
        ) -> ProviderResult<PresentationTarget> {
            self.note("prepare_presentation");
            Err(self.refuse("prepare_presentation"))
        }

        fn rename(
            &self,
            _scope: &InventoryScope,
            binding: &NativeBinding,
            _new_native_name: &str,
        ) -> ProviderResult<()> {
            self.note("rename");
            self.handed_epochs.borrow_mut().push(binding.server_epoch);
            Err(self.refuse("rename"))
        }

        fn remove(&self, _scope: &InventoryScope, binding: &NativeBinding) -> ProviderResult<()> {
            self.note("remove");
            self.handed_epochs.borrow_mut().push(binding.server_epoch);
            Err(self.refuse("remove"))
        }

        fn group_list(
            &self,
            _scope: &InventoryScope,
            _binding: &NativeBinding,
        ) -> ProviderResult<Vec<NativeGroupRow>> {
            self.note("group_list");
            Err(self.refuse("group_list"))
        }

        fn group_new(
            &self,
            _scope: &InventoryScope,
            binding: &NativeBinding,
            _spec: &CreateSpec,
        ) -> ProviderResult<ProviderHandle> {
            self.note("group_new");
            self.handed_epochs.borrow_mut().push(binding.server_epoch);
            Err(self.refuse("group_new"))
        }

        fn group_activate(
            &self,
            _scope: &InventoryScope,
            _handle: &ProviderHandle,
        ) -> ProviderResult<()> {
            self.note("group_activate");
            Err(self.refuse("group_activate"))
        }

        fn group_rename(
            &self,
            _scope: &InventoryScope,
            _handle: &ProviderHandle,
            _title: &str,
        ) -> ProviderResult<()> {
            self.note("group_rename");
            Err(self.refuse("group_rename"))
        }

        fn group_remove(
            &self,
            _scope: &InventoryScope,
            _handle: &ProviderHandle,
        ) -> ProviderResult<()> {
            self.note("group_remove");
            Err(self.refuse("group_remove"))
        }

        fn split_list(
            &self,
            scope: &InventoryScope,
            group: &ProviderHandle,
        ) -> ProviderResult<Vec<NativeSplitRow>> {
            self.note("split_list");
            if scope.expected_epoch().is_none() {
                return Err(ProviderError::WrongInstance {
                    detail: "scripted split_list: a managed read requires a pinned scope".into(),
                });
            }
            Ok(self
                .rows
                .iter()
                .flat_map(|row| row.groups.iter())
                .find(|row| row.handle == *group)
                .map(|row| row.splits.clone())
                .unwrap_or_default())
        }

        fn split_new(
            &self,
            _scope: &InventoryScope,
            _group: &ProviderHandle,
            _spec: &SplitSpec,
        ) -> ProviderResult<ProviderHandle> {
            self.note("split_new");
            Err(self.refuse("split_new"))
        }

        fn split_activate(
            &self,
            _scope: &InventoryScope,
            _handle: &ProviderHandle,
        ) -> ProviderResult<()> {
            self.note("split_activate");
            Err(self.refuse("split_activate"))
        }

        fn split_remove(
            &self,
            _scope: &InventoryScope,
            _handle: &ProviderHandle,
        ) -> ProviderResult<()> {
            self.note("split_remove");
            Err(self.refuse("split_remove"))
        }

        fn normalize_plan(
            &self,
            _scope: &InventoryScope,
            native_token: &str,
        ) -> ProviderResult<NormalizePlan> {
            self.note("normalize_plan");
            Err(ProviderError::NativeFailure {
                detail: format!("scripted normalize_plan: {native_token}"),
            })
        }

        fn inspect(
            &self,
            _scope: &InventoryScope,
            binding: &NativeBinding,
        ) -> ProviderResult<NativeSpaceRow> {
            self.note("inspect");
            self.handed_epochs.borrow_mut().push(binding.server_epoch);
            Err(self.refuse("inspect"))
        }
    }
}

mod scripted_registry {
    use dmux::model::{Backend, BackendInstanceUid, ServerEpoch, SpaceUid};
    use dmux::operations::OperationEnv;
    use dmux::registry::{NativeBindingSpec, NativeKind, Registry, RegistryConfig};
    use uuid::Uuid;

    pub struct ScriptedEnv {
        _data: tempfile::TempDir,
        _locks: tempfile::TempDir,
        pub env: OperationEnv,
    }

    pub fn scripted_env() -> ScriptedEnv {
        let data = tempfile::tempdir().unwrap();
        let locks = tempfile::tempdir().unwrap();
        let env = OperationEnv {
            db_path: data.path().join("registry.sqlite3"),
            lock_dir: locks.path().to_path_buf(),
        };
        ScriptedEnv {
            _data: data,
            _locks: locks,
            env,
        }
    }

    pub fn registry(env: &OperationEnv) -> Registry {
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap()
    }

    /// The registry state a finished create leaves behind, minus the server:
    /// the managed `backend` instance on `endpoint`, its incarnation
    /// published as `published`, and one Active/Healthy Space bound to
    /// `token` with `binding_epoch` recorded on the binding.
    pub fn seed_bound_space(
        env: &OperationEnv,
        backend: Backend,
        endpoint: &str,
        published: ServerEpoch,
        token: &str,
        binding_epoch: Option<ServerEpoch>,
    ) -> (BackendInstanceUid, SpaceUid) {
        let mut registry = registry(env);
        let instance = registry
            .register_backend_instance(backend, Some(endpoint), None)
            .unwrap();
        registry
            .publish_backend_server(instance, published, Some(4242), Some("start"), None, None)
            .unwrap();
        let reservation = registry
            .reserve_space("proj", instance, Uuid::new_v4())
            .unwrap();
        registry
            .finalize_create(
                reservation.space_uid,
                reservation.operation_uid,
                &NativeBindingSpec {
                    native_token: token.into(),
                    native_kind: match backend {
                        Backend::Tmux => NativeKind::TmuxSessionId,
                        Backend::Wez => NativeKind::WezWorkspaceKey,
                    },
                    server_epoch: binding_epoch,
                },
            )
            .unwrap();
        (instance, reservation.space_uid)
    }

    /// Rows in `bootstrap_requests` — the durable journal WS-A.10 is about.
    pub fn bootstrap_rows(env: &OperationEnv) -> i64 {
        registry(env)
            .raw_connection()
            .query_row("SELECT COUNT(*) FROM bootstrap_requests", [], |row| {
                row.get(0)
            })
            .unwrap()
    }
}

use scripted::{Script, one_window};
use scripted_registry::{bootstrap_rows, scripted_env, seed_bound_space};

fn group_request(space_uid: dmux::model::SpaceUid) -> GroupNewRequest {
    GroupNewRequest {
        request_uid: Uuid::new_v4(),
        space_uid,
        cwd: None,
        program: Vec::new(),
        helper_bin: "/unused/pane-bootstrap".into(),
    }
}

/// ADR 012 §3.4 / WS-A.11 (case 26): the scripted `split_list` refuses an
/// unpinned scope exactly as both adapters do. Before the fix
/// `operations::group_new` reached that refusal only after
/// `provider.group_new` had created the window and the bootstrap row had
/// been journaled. Now nothing is created: the provider sees one inventory
/// call and the journal stays empty. The pinned control proves the refusal
/// is the scope, not the seed — the same request reaches the provider's
/// `group_new` and the journal row it aborts carries the pinned epoch.
#[test]
fn group_new_under_an_unpinned_scope_creates_nothing() {
    let scratch = scripted_env();
    let env = &scratch.env;
    let epoch = ServerEpoch(Uuid::new_v4());
    let (_instance, space_uid) =
        seed_bound_space(env, Backend::Tmux, "tmux-script", epoch, "$1", Some(epoch));
    let provider = Script::new(
        Backend::Tmux,
        epoch,
        vec![one_window("$1", dmux::model::ProviderHandle::Tx)],
    );

    let unpinned = InventoryScope::unmanaged_endpoint(Backend::Tmux, "tmux-script");
    let err = group_new(env, &provider, &unpinned, &group_request(space_uid)).unwrap_err();
    assert!(matches!(err, OpError::Indeterminate(_)), "{err}");
    assert!(err.to_string().contains("pinned"), "{err}");
    assert_eq!(
        provider.calls(),
        vec!["inventory"],
        "no mutation, no split_list"
    );
    assert_eq!(
        bootstrap_rows(env),
        0,
        "nothing journaled under an unpinned scope"
    );

    // Control: the pinned scope reaches the mutation, journaled first.
    let pinned = InventoryScope::managed(Backend::Tmux, "tmux-script", epoch);
    let err = group_new(env, &provider, &pinned, &group_request(space_uid)).unwrap_err();
    assert!(matches!(err, OpError::Provider(_)), "{err}");
    assert_eq!(
        provider.calls(),
        vec!["inventory", "inventory", "group_new"]
    );
    assert_eq!(bootstrap_rows(env), 1);
}
fn split_request(space_uid: dmux::model::SpaceUid, group: ChildRefShape) -> SplitNewRequest {
    SplitNewRequest {
        request_uid: Uuid::new_v4(),
        space_uid,
        group,
        direction: SplitDirection::Right,
        percent: None,
        cwd: None,
        program: Vec::new(),
        helper_bin: "/unused/pane-bootstrap".into(),
    }
}

fn journaled_epochs(env: &OperationEnv) -> Vec<String> {
    let registry = scripted_registry::registry(env);
    let mut stmt = registry
        .raw_connection()
        .prepare("SELECT server_epoch FROM bootstrap_requests ORDER BY created_at")
        .unwrap();
    stmt.query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

/// WS-A.10 (finding #10; the executable form of report 08 §7's
/// `operations.rs:2243`): `bootstrap_requests.server_epoch` is a non-Option
/// column with 25 readers, so the only value allowed into it is the epoch
/// the scope was pinned to and the scan confirmed. An unpinned scope — an
/// epoch nothing in the registry vouches for — journals nothing at all; the
/// pinned control journals exactly the pin, and the provider is reached only
/// after that row exists.
#[test]
fn bootstrap_issue_journals_only_the_pinned_epoch() {
    let scratch = scripted_env();
    let env = &scratch.env;
    let epoch = ServerEpoch(Uuid::new_v4());
    let (_instance, space_uid) =
        seed_bound_space(env, Backend::Tmux, "tmux-script", epoch, "$1", Some(epoch));
    let provider = Script::new(
        Backend::Tmux,
        epoch,
        vec![one_window("$1", dmux::model::ProviderHandle::Tx)],
    );
    let group = child_shape(&format!("g{}.tx-1", epoch.0));

    let unpinned = InventoryScope::unmanaged_endpoint(Backend::Tmux, "tmux-script");
    let err = split_new(
        env,
        &provider,
        &unpinned,
        &split_request(space_uid, group.clone()),
    )
    .unwrap_err();
    assert!(matches!(err, OpError::Indeterminate(_)), "{err}");
    assert_eq!(provider.calls(), vec!["inventory"]);
    assert!(
        journaled_epochs(env).is_empty(),
        "an unverified epoch reached the journal"
    );

    let pinned = InventoryScope::managed(Backend::Tmux, "tmux-script", epoch);
    let err = split_new(env, &provider, &pinned, &split_request(space_uid, group)).unwrap_err();
    assert!(matches!(err, OpError::Provider(_)), "{err}");
    assert_eq!(
        provider.calls(),
        vec!["inventory", "inventory", "split_new"]
    );
    assert_eq!(journaled_epochs(env), vec![epoch.0.to_string()]);
}

/// The create path's journal row comes from `scan_epoch_for_create`: a
/// selected target whose scope is unpinned is refused before a SpaceNo is
/// consumed or a bootstrap row exists, even though its scan is complete and
/// epoched. Pinned, the same request reserves, journals the pin, and reaches
/// the provider.
#[test]
fn owner_fenced_create_refuses_an_unpinned_selected_scope_before_reserving() {
    let scratch = scripted_env();
    let env = &scratch.env;
    let epoch = ServerEpoch(Uuid::new_v4());
    let instance = {
        let mut registry = scripted_registry::registry(env);
        let instance = registry
            .register_backend_instance(Backend::Tmux, Some("tmux-script"), None)
            .unwrap();
        registry
            .publish_backend_server(instance, epoch, Some(4242), Some("start"), None, None)
            .unwrap();
        instance
    };
    let provider = Script::new(Backend::Tmux, epoch, Vec::new());
    let request = |name: &str| CreateRequest {
        request_uid: Uuid::new_v4(),
        name: name.into(),
        cwd: None,
        program: Vec::new(),
        helper_bin: "/unused/pane-bootstrap".into(),
    };

    let unpinned = InventoryScope::unmanaged_endpoint(Backend::Tmux, "tmux-script");
    let err = create_space_owner_fenced(
        env,
        OwnerCreateTarget {
            backend: Backend::Tmux,
            instance,
            provider: &provider,
            scope: &unpinned,
        },
        None,
        false,
        &request("proj"),
    )
    .unwrap_err();
    assert!(matches!(err, OpError::Indeterminate(_)), "{err}");
    assert_eq!(provider.calls(), vec!["inventory"]);
    assert!(
        scripted_registry::registry(env)
            .spaces()
            .unwrap()
            .is_empty()
    );
    assert!(journaled_epochs(env).is_empty());

    let pinned = InventoryScope::managed(Backend::Tmux, "tmux-script", epoch);
    let err = create_space_owner_fenced(
        env,
        OwnerCreateTarget {
            backend: Backend::Tmux,
            instance,
            provider: &provider,
            scope: &pinned,
        },
        None,
        false,
        &request("proj"),
    )
    .unwrap_err();
    assert!(matches!(err, OpError::Provider(_)), "{err}");
    assert_eq!(provider.calls(), vec!["inventory", "inventory", "create"]);
    assert_eq!(journaled_epochs(env), vec![epoch.0.to_string()]);
    let spaces = scripted_registry::registry(env).spaces().unwrap();
    assert_eq!(spaces.len(), 1);
    assert_eq!(spaces[0].lifecycle, dmux::model::Lifecycle::Aborted);
}

/// WS-A.12 (review report 07's residual): `repair_scan_wez` used to
/// `register_backend_instance` from the scope's endpoint, and the registry
/// answered with the owner's existing Wez instance whatever the socket — so
/// the `--socket` seam fenced instance A while scanning endpoint B. Now the
/// endpoint is compared with the registry before anything is fenced, and a
/// mismatch (or an instance with no recorded endpoint at all) is refused
/// naming both; the provider is never consulted. The matching endpoint is
/// the ordinary path and reaches the scan.
#[test]
fn repair_scan_refuses_an_endpoint_other_than_the_instances_recorded_socket() {
    let scratch = scripted_env();
    let env = &scratch.env;
    let epoch = ServerEpoch(Uuid::new_v4());
    let instance = scripted_registry::registry(env)
        .register_backend_instance(Backend::Wez, Some("/run/dmux/a.sock"), None)
        .unwrap();
    let provider = Script::new(Backend::Wez, epoch, Vec::new());

    let elsewhere = InventoryScope::unmanaged_endpoint(Backend::Wez, "/run/dmux/b.sock");
    let err = dmux::operations::repair_scan_wez(env, &provider, &elsewhere).unwrap_err();
    assert!(matches!(err, OpError::Refused(_)), "{err}");
    let text = err.to_string();
    assert!(
        text.contains("/run/dmux/a.sock") && text.contains("/run/dmux/b.sock"),
        "{text}"
    );
    assert!(text.contains(&instance.0.to_string()), "{text}");
    assert!(
        provider.calls().is_empty(),
        "refused before any scan: {:?}",
        provider.calls()
    );
    assert_eq!(
        scripted_registry::registry(env)
            .backend_instance_info(instance)
            .unwrap()
            .socket_path
            .as_deref(),
        Some("/run/dmux/a.sock")
    );

    let recorded = InventoryScope::unmanaged_endpoint(Backend::Wez, "/run/dmux/a.sock");
    assert!(
        dmux::operations::repair_scan_wez(env, &provider, &recorded)
            .unwrap()
            .is_empty()
    );
    assert_eq!(provider.calls(), vec!["inventory"]);

    // No recorded endpoint: nothing vouches for any socket, so none is scanned.
    let unaddressable = scripted_env();
    scripted_registry::registry(&unaddressable.env)
        .register_backend_instance(Backend::Wez, None, None)
        .unwrap();
    let provider = Script::new(Backend::Wez, epoch, Vec::new());
    let err =
        dmux::operations::repair_scan_wez(&unaddressable.env, &provider, &elsewhere).unwrap_err();
    assert!(matches!(err, OpError::Refused(_)), "{err}");
    assert!(err.to_string().contains("<none>"), "{err}");
    assert!(provider.calls().is_empty());
}

/// WS-A.8 at the operations layer (findings #5/#18; the executable form of
/// report 08 §7's `operations.rs:2277` and `registry/mod.rs:2812`): the
/// binding handed to a tmux adapter carries the registry's recorded epoch,
/// and a binding recorded under another incarnation than the one the scope
/// is pinned to is refused `backend_epoch_changed` before any journal row or
/// native command — a `$N` on the new server is not provably this Space.
/// Once the registry records the pinned epoch the same verbs reach the
/// provider, and what they hand it is that recorded value.
#[test]
fn a_tmux_binding_recorded_under_another_incarnation_is_refused_before_any_native_command() {
    let scratch = scripted_env();
    let env = &scratch.env;
    let recorded = ServerEpoch(Uuid::new_v4());
    let published = ServerEpoch(Uuid::new_v4());
    let (_instance, space_uid) = seed_bound_space(
        env,
        Backend::Tmux,
        "tmux-script",
        published,
        "$1",
        Some(recorded),
    );
    let provider = Script::new(
        Backend::Tmux,
        published,
        vec![one_window("$1", dmux::model::ProviderHandle::Tx)],
    );
    let scope = InventoryScope::managed(Backend::Tmux, "tmux-script", published);

    let err = group_new(env, &provider, &scope, &group_request(space_uid)).unwrap_err();
    assert!(matches!(err, OpError::StaleRef(_)), "{err}");
    assert!(err.to_string().contains(&recorded.0.to_string()), "{err}");
    assert!(err.to_string().contains(&published.0.to_string()), "{err}");
    assert_eq!(provider.calls(), vec!["inventory"]);
    assert_eq!(bootstrap_rows(env), 0);

    let err = remove_space(
        env,
        &provider,
        &scope,
        Backend::Tmux,
        space_uid,
        Uuid::new_v4(),
    )
    .unwrap_err();
    assert!(matches!(err, OpError::StaleRef(_)), "{err}");
    let err = rename_space(
        env,
        &provider,
        &scope,
        Backend::Tmux,
        space_uid,
        "renamed",
        Uuid::new_v4(),
    )
    .unwrap_err();
    assert!(matches!(err, OpError::StaleRef(_)), "{err}");
    assert_eq!(
        provider.calls(),
        vec!["inventory"],
        "no native command for rm/rename"
    );
    let registry = scripted_registry::registry(env);
    let row = registry.space(space_uid).unwrap();
    assert_eq!(row.lifecycle, dmux::model::Lifecycle::Active);
    assert_eq!(row.logical_name, "proj");
    assert!(
        registry.unfinished_operation(space_uid).unwrap().is_none(),
        "refused before any intent was journaled"
    );
    assert_eq!(
        registry.current_binding_epoch(space_uid).unwrap(),
        Some(recorded)
    );
    drop(registry);

    // Control: the registry now records the pinned epoch; the provider is
    // reached and handed exactly that value.
    scripted_registry::registry(env)
        .observe_binding_epoch(space_uid, published)
        .unwrap();
    let err = group_new(env, &provider, &scope, &group_request(space_uid)).unwrap_err();
    assert!(matches!(err, OpError::Provider(_)), "{err}");
    assert_eq!(
        provider.calls(),
        vec!["inventory", "inventory", "group_new"]
    );
    assert_eq!(provider.handed_epochs(), vec![published]);
    assert_eq!(bootstrap_rows(env), 1);
}

/// The wez half of WS-A.8. A workspace key is registry-minted identity that
/// survives a restart (cold recovery restores it by key, plan §15.3), so a
/// binding recorded under an earlier incarnation is not stale by itself: a
/// complete scan under the pin that lists the key proves it live, the
/// registry's recorded epoch is refreshed as observation metadata, and the
/// provider is handed that refreshed value. A key the pinned scan does not
/// list has nothing live to kill: an explicit removal proceeds to its
/// tombstone without a native command, and nothing is refreshed.
#[test]
fn a_wez_binding_recorded_under_another_incarnation_is_refreshed_by_a_pinned_scan() {
    let scratch = scripted_env();
    let env = &scratch.env;
    let recorded = ServerEpoch(Uuid::new_v4());
    let published = ServerEpoch(Uuid::new_v4());
    let key = "dmux:host:space";
    let (_instance, space_uid) = seed_bound_space(
        env,
        Backend::Wez,
        "/run/dmux/wez.sock",
        published,
        key,
        Some(recorded),
    );
    let provider = Script::new(
        Backend::Wez,
        published,
        vec![one_window(key, dmux::model::ProviderHandle::Wz)],
    );
    let scope = InventoryScope::managed(Backend::Wez, "/run/dmux/wez.sock", published);

    let err = group_new(env, &provider, &scope, &group_request(space_uid)).unwrap_err();
    assert!(matches!(err, OpError::Provider(_)), "{err}");
    assert_eq!(provider.calls(), vec!["inventory", "group_new"]);
    assert_eq!(provider.handed_epochs(), vec![published]);
    assert_eq!(
        scripted_registry::registry(env)
            .current_binding_epoch(space_uid)
            .unwrap(),
        Some(published),
        "the recorded epoch is refreshed by the verified scan"
    );

    // Removal with the key live hands the (now pinned) recorded epoch.
    let err = remove_space(
        env,
        &provider,
        &scope,
        Backend::Wez,
        space_uid,
        Uuid::new_v4(),
    )
    .unwrap_err();
    assert!(matches!(err, OpError::Provider(_)), "{err}");
    assert_eq!(provider.calls(), vec!["inventory", "group_new", "remove"]);
    assert_eq!(provider.handed_epochs(), vec![published, published]);

    // A key the pinned scan no longer lists: explicit remove, verified absent,
    // no native command, the recorded epoch left as it was.
    let scratch = scripted_env();
    let env = &scratch.env;
    let (_instance, space_uid) = seed_bound_space(
        env,
        Backend::Wez,
        "/run/dmux/wez.sock",
        published,
        key,
        Some(recorded),
    );
    let provider = Script::new(Backend::Wez, published, Vec::new());
    remove_space(
        env,
        &provider,
        &scope,
        Backend::Wez,
        space_uid,
        Uuid::new_v4(),
    )
    .unwrap();
    assert_eq!(
        provider.calls(),
        vec!["inventory", "inventory"],
        "scan, final-empty scan"
    );
    assert!(
        provider.handed_epochs().is_empty(),
        "no binding was handed to the provider"
    );
    let registry = scripted_registry::registry(env);
    assert_eq!(
        registry.space(space_uid).unwrap().lifecycle,
        dmux::model::Lifecycle::Deleted
    );
    let severed: Option<String> = registry
        .raw_connection()
        .query_row(
            "SELECT server_epoch FROM native_bindings WHERE space_uid = ?1",
            [space_uid.0.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(severed.as_deref(), Some(recorded.0.to_string().as_str()));
}

// ---------------------------------------------------------------------------
// The tmux socket witness (ADR 012 WS-A.9; review finding #11, the
// executable form of report 08 §7's `operations.rs:133`), on a real scratch
// server.

/// What `tmux -L <ns>` itself reports for the running server: pid and the
/// resolved socket path.
fn live_tmux_identity(s: &Scratch) -> (i64, String) {
    let line = s.tmux(&["list-sessions", "-F", "#{pid}\t#{socket_path}"]);
    let (pid, socket) = line
        .lines()
        .next()
        .and_then(|line| line.split_once('\t'))
        .expect("pid and socket_path");
    (pid.parse().unwrap(), socket.to_string())
}

/// `tmux_bootstrap` used to publish `socket_dev`/`socket_ino` as literal
/// `None`, so the stat-based replacement witness was structurally
/// unreachable for tmux. It now records the dev/ino of the socket the
/// server reports, beside the pid — exactly what a fresh `stat` returns.
#[test]
fn tmux_bootstrap_publishes_the_sockets_device_and_inode() {
    use std::os::unix::fs::MetadataExt;
    let s = Scratch::new("witness");
    let epoch = s.epoch();
    let registry = s.registry();
    let instance = registry
        .backend_instance_for_backend(Backend::Tmux)
        .unwrap()
        .expect("bootstrap registered the instance");
    let published = registry.backend_server(instance).unwrap();
    let (pid, socket) = live_tmux_identity(&s);
    let meta = std::fs::metadata(&socket).unwrap();
    assert_eq!(published.server_epoch, Some(epoch));
    assert_eq!(published.server_pid, Some(pid));
    assert_eq!(published.socket_dev, Some(meta.dev() as i64));
    assert_eq!(published.socket_ino, Some(meta.ino() as i64));

    // Idempotent: the same incarnation is already bound, witnesses included.
    assert!(matches!(
        tmux_bootstrap(&s.env(), &s.ns).unwrap(),
        dmux::operations::TmuxBootstrapOutcome::AlreadyBound { epoch: again } if again == epoch
    ));
}

/// A replaced server on the same namespace — same socket path, new inode,
/// new pid — that presents the OLD `@dmux_server_epoch` is exactly what the
/// epoch option alone cannot tell apart (ADR 002: tmux is the easiest backend
/// to spoof). Every operations-layer verification now compares the
/// registry's witnesses against a fresh probe and refuses it as a stale
/// incarnation: reads, the marker revalidation, and a child mutation, which
/// creates nothing on the impostor and journals nothing.
#[test]
fn a_replaced_tmux_socket_presenting_the_old_epoch_is_refused_everywhere() {
    let s = Scratch::new("replaced");
    let epoch = s.epoch();
    let scope = s.scope(epoch);
    let provider = TmuxProvider::new(s.ns.clone());
    let created = create_space(
        &s.env(),
        &provider,
        &scope,
        Backend::Tmux,
        &request(&s, "proj", &s.data.path().join("replaced-marker")),
    )
    .unwrap();
    let identity = s.registry().identity().unwrap();
    let marker = MarkerContext {
        host_uid: identity.host_uid,
        space_uid: created.space_uid,
        space_no: created.space_no,
        backend: Backend::Tmux,
        domain: None,
        server_epoch: epoch,
        group_ref: created.group_ref.clone(),
        split_ref: created.split_ref.clone(),
    };
    assert!(validate_marker_context(&s.env(), &provider, &scope, &marker).is_ok());
    let (old_pid, socket) = live_tmux_identity(&s);

    // Replace the server: kill it, start another on the same namespace with
    // the managed Space's session name and id recycled, then copy the old
    // epoch onto it. Nothing the registry recorded survives but the epoch.
    s.tmux(&["kill-server"]);
    let out = Command::new("tmux")
        .args([
            "-L",
            &s.ns,
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "seed",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    s.tmux(&["new-session", "-d", "-s", "proj"]);
    s.tmux(&[
        "set-option",
        "-g",
        "@dmux_server_epoch",
        &epoch.0.to_string(),
    ]);
    let (new_pid, new_socket) = live_tmux_identity(&s);
    assert_eq!(socket, new_socket, "same path");
    assert_ne!(old_pid, new_pid, "different process");
    assert!(
        s.session_names().contains(&"proj".to_string()),
        "the impostor carries the session name"
    );
    let windows_before = s.tmux(&["list-windows", "-a", "-F", "#{window_id}"]);

    let err = hierarchy(&s.env(), &provider, &scope, created.space_uid).unwrap_err();
    assert!(matches!(err, OpError::StaleRef(_)), "hierarchy: {err}");
    assert!(
        err.to_string()
            .contains("not the registry-published incarnation"),
        "{err}"
    );
    let pane = s
        .tmux(&["list-panes", "-t", "proj", "-F", "#{pane_id}"])
        .trim()
        .to_string();
    let err = context_read(&s.env(), &provider, &scope, created.space_uid, &pane).unwrap_err();
    assert!(matches!(err, OpError::StaleRef(_)), "context_read: {err}");
    let err = validate_marker_context(&s.env(), &provider, &scope, &marker).unwrap_err();
    assert!(matches!(err, OpError::StaleRef(_)), "marker: {err}");
    let err = group_new(
        &s.env(),
        &provider,
        &scope,
        &GroupNewRequest {
            request_uid: Uuid::new_v4(),
            space_uid: created.space_uid,
            cwd: None,
            program: vec!["sh".into(), "-c".into(), "exec sleep 300".into()],
            helper_bin: env!("CARGO_BIN_EXE_pane-bootstrap").into(),
        },
    )
    .unwrap_err();
    assert!(matches!(err, OpError::StaleRef(_)), "group_new: {err}");
    assert_eq!(
        s.tmux(&["list-windows", "-a", "-F", "#{window_id}"]),
        windows_before,
        "nothing was created on the impostor"
    );
    assert_eq!(
        bootstrap_rows(&s.env()),
        1,
        "only the original create's row"
    );

    // The hook re-running on the impostor republishes its witnesses; the
    // same verbs then verify again.
    s.epoch();
    assert!(hierarchy(&s.env(), &provider, &scope, created.space_uid).is_ok());
}
