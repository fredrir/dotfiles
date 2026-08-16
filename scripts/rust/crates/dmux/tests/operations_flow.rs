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
    CreateRequest, OpError, OperationEnv, SplitNewRequest, create_space, group_activate_exact,
    remove_space, rename_space, resume_remove_space, split_direction, split_new, split_resize,
    split_zoom, tmux_bootstrap, validate_marker_context,
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
        InventoryScope {
            backend: Backend::Tmux,
            endpoint: self.ns.clone(),
            expected_epoch: Some(epoch),
        }
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
