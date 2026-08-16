//! Real-tmux integration tests for the P3a adapter (plan §18 P3a, §11.2).
//!
//! Every test runs on its own scratch socket namespace
//! (`tmux -L dmux-p3a-<pid>-<n>`) and kills that server from a Drop guard so
//! cleanup happens on panic paths too. The user's default tmux server is
//! never touched. Tests soft-skip when no tmux binary is installed.

use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use dmux::backend::tmux::{
    EpochSetOutcome, SpaceMarkers, SystemRunner, TmuxProvider, TmuxServerIdentity,
};
use dmux::backend::{
    CreateSpec, InventoryOutcome, InventoryScope, NativeBinding, PresentationTarget, Provider,
    ProviderError,
};
use dmux::model::{Backend, ProviderHandle, ServerEpoch};
use uuid::Uuid;

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn tmux_available() -> bool {
    Command::new("tmux").arg("-V").output().is_ok()
}

/// Scratch namespace with guaranteed `kill-server` cleanup (also on panic).
struct ScratchServer {
    ns: String,
}

impl ScratchServer {
    fn new() -> Self {
        Self::with_prefix("dmux-p3a")
    }

    /// P5 epoch-bootstrap tests use their own `dmux-p5tx-<pid>-<n>` scratch
    /// namespaces; same Drop-guard `kill-server` cleanup.
    fn with_prefix(prefix: &str) -> Self {
        ScratchServer {
            ns: format!(
                "{prefix}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst)
            ),
        }
    }

    fn tmux(&self, args: &[&str]) -> std::process::Output {
        Command::new("tmux")
            .arg("-L")
            .arg(&self.ns)
            .args(args)
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .output()
            .expect("spawn tmux")
    }

    fn tmux_ok(&self, args: &[&str]) -> String {
        let out = self.tmux(args);
        assert!(
            out.status.success(),
            "tmux -L {} {args:?}: {}",
            self.ns,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).expect("utf8 tmux output")
    }

    /// Start the server with a long-lived holder session (a tmux server only
    /// lives while it has at least one session) and return the holder's ids.
    fn start_with_holder(&self) -> (String, u64, u64) {
        self.spawn_session("holder")
    }

    fn spawn_session(&self, name: &str) -> (String, u64, u64) {
        let out = self.tmux_ok(&[
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{session_id}|#{window_id}|#{pane_id}",
            "-s",
            name,
            "--",
            "/bin/sh",
            "-c",
            "sleep 300",
        ]);
        parse_triple(&out)
    }

    /// Simulate the P5 bootstrap hook: install the server epoch.
    fn set_epoch(&self, epoch: ServerEpoch) {
        self.tmux_ok(&[
            "set-option",
            "-g",
            "@dmux_server_epoch",
            &epoch.0.to_string(),
        ]);
    }

    fn scope(&self, expected: Option<ServerEpoch>) -> InventoryScope {
        InventoryScope {
            backend: Backend::Tmux,
            endpoint: self.ns.clone(),
            expected_epoch: expected,
        }
    }

    fn provider(&self) -> TmuxProvider<SystemRunner> {
        TmuxProvider::new(self.ns.clone())
    }
}

impl Drop for ScratchServer {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .arg("-L")
            .arg(&self.ns)
            .arg("kill-server")
            .output();
    }
}

fn parse_triple(spawn_return: &str) -> (String, u64, u64) {
    let line = spawn_return.trim_end_matches('\n');
    let parts: Vec<&str> = line.split('|').collect();
    assert_eq!(parts.len(), 3, "spawn return {line:?}");
    (
        parts[0].to_string(),
        parts[1].strip_prefix('@').unwrap().parse().unwrap(),
        parts[2].strip_prefix('%').unwrap().parse().unwrap(),
    )
}

fn bootstrap_argv() -> Vec<String> {
    // Stand-in for the ADR 004 bootstrap helper argv.
    vec!["/bin/sh".into(), "-c".into(), "sleep 300".into()]
}

fn fresh_epoch() -> ServerEpoch {
    ServerEpoch(Uuid::new_v4())
}

#[test]
fn inventory_round_trip_matches_exact_native_ids() {
    if !tmux_available() {
        eprintln!("skipping: tmux not installed");
        return;
    }
    let srv = ScratchServer::new();
    // 2 sessions, 3 windows, 4 panes, all ids captured at creation.
    let (s_alpha, w_alpha0, p_alpha0) = srv.spawn_session("alpha");
    let epoch = fresh_epoch();
    srv.set_epoch(epoch);
    let (s_beta, w_beta0, p_beta0) = srv.spawn_session("beta");
    let (_, w_beta1, p_beta1) = parse_triple(&srv.tmux_ok(&[
        "new-window",
        "-P",
        "-F",
        "#{session_id}|#{window_id}|#{pane_id}",
        "-t",
        &s_beta,
        "-n",
        "build",
        "-c",
        "/",
        "--",
        "/bin/sh",
        "-c",
        "sleep 300",
    ]));
    let (_, _, p_alpha1) = parse_triple(&srv.tmux_ok(&[
        "split-window",
        "-P",
        "-F",
        "#{session_id}|#{window_id}|#{pane_id}",
        "-t",
        &format!("%{p_alpha0}"),
        "--",
        "/bin/sh",
        "-c",
        "sleep 300",
    ]));

    let outcome = srv.provider().inventory(&srv.scope(Some(epoch)));
    let InventoryOutcome::Complete(inv) = outcome else {
        panic!("expected complete inventory, got {outcome:?}");
    };
    assert_eq!(inv.server_epoch, Some(epoch));
    assert_eq!(inv.rows.len(), 2);

    let alpha = inv
        .rows
        .iter()
        .find(|r| r.native_token == s_alpha)
        .expect("alpha row");
    assert_eq!(alpha.native_name, "alpha");
    assert!(!alpha.multi_window, "multi_window is always false on tmux");
    assert_eq!(alpha.groups.len(), 1);
    assert_eq!(alpha.groups[0].handle, ProviderHandle::Tx(w_alpha0));
    let alpha_panes: Vec<&ProviderHandle> =
        alpha.groups[0].splits.iter().map(|s| &s.handle).collect();
    assert_eq!(
        alpha_panes,
        vec![&ProviderHandle::Tx(p_alpha0), &ProviderHandle::Tx(p_alpha1)]
    );

    let beta = inv
        .rows
        .iter()
        .find(|r| r.native_token == s_beta)
        .expect("beta row");
    assert_eq!(beta.native_name, "beta");
    let beta_windows: Vec<&ProviderHandle> = beta.groups.iter().map(|g| &g.handle).collect();
    assert_eq!(
        beta_windows,
        vec![&ProviderHandle::Tx(w_beta0), &ProviderHandle::Tx(w_beta1)]
    );
    let build = &beta.groups[1];
    assert_eq!(build.title.as_deref(), Some("build"));
    assert_eq!(build.splits.len(), 1);
    assert_eq!(build.splits[0].handle, ProviderHandle::Tx(p_beta1));
    assert_eq!(build.splits[0].cwd.as_deref(), Some("/"));
    assert_eq!(beta.groups[0].splits[0].handle, ProviderHandle::Tx(p_beta0));

    let total_panes: usize = inv
        .rows
        .iter()
        .flat_map(|r| &r.groups)
        .map(|g| g.splits.len())
        .sum();
    assert_eq!(total_panes, 4);
    let total_windows: usize = inv.rows.iter().map(|r| r.groups.len()).sum();
    assert_eq!(total_windows, 3);
}

#[test]
fn unepoched_server_reports_none_and_ls_never_writes_the_option() {
    if !tmux_available() {
        eprintln!("skipping: tmux not installed");
        return;
    }
    let srv = ScratchServer::new();
    srv.start_with_holder();

    let outcome = srv.provider().inventory(&srv.scope(None));
    let InventoryOutcome::Complete(inv) = outcome else {
        panic!("expected complete inventory, got {outcome:?}");
    };
    assert_eq!(inv.server_epoch, None, "unepoched server must scan as None");
    assert_eq!(inv.rows.len(), 1);

    // Prove `ls` never brings a server under management: the global option
    // table still has no @dmux_server_epoch after the scan (plan §11.2).
    let globals = srv.tmux_ok(&["show-options", "-g"]);
    assert!(
        !globals.contains("@dmux_server_epoch"),
        "inventory must never write the epoch option; globals:\n{globals}"
    );

    // The P5 identity probe is equally read-only: of the bootstrap
    // primitives, only set_epoch_if_absent writes the option.
    srv.provider()
        .server_identity(&srv.ns)
        .expect("server_identity");
    let globals = srv.tmux_ok(&["show-options", "-g"]);
    assert!(
        !globals.contains("@dmux_server_epoch"),
        "server_identity must never write the epoch option; globals:\n{globals}"
    );
}

#[test]
fn missing_server_namespace_is_server_stopped() {
    if !tmux_available() {
        eprintln!("skipping: tmux not installed");
        return;
    }
    let srv = ScratchServer::new(); // never started
    match srv.provider().inventory(&srv.scope(None)) {
        InventoryOutcome::ServerStopped { .. } => {}
        other => panic!("expected server_stopped, got {other:?}"),
    }
}

#[test]
fn create_returns_consistent_ids_and_asserts_managed_options() {
    if !tmux_available() {
        eprintln!("skipping: tmux not installed");
        return;
    }
    let srv = ScratchServer::new();
    srv.start_with_holder();
    let epoch = fresh_epoch();
    srv.set_epoch(epoch);

    let spec = CreateSpec {
        native_token: "managed-space".into(),
        cwd: Some("/".into()),
        bootstrap_argv: bootstrap_argv(),
    };
    let binding = srv
        .provider()
        .create(&srv.scope(Some(epoch)), &spec)
        .expect("create");

    // The binding holds the immutable session ID, not the requested name.
    assert!(binding.native_token.starts_with('$'), "{binding:?}");
    assert_eq!(binding.server_epoch, epoch);

    // The three spawn-return ids must be mutually consistent with a scan.
    let InventoryOutcome::Complete(inv) = srv.provider().inventory(&srv.scope(Some(epoch))) else {
        panic!("expected complete inventory");
    };
    let row = inv
        .rows
        .iter()
        .find(|r| r.native_token == binding.native_token)
        .expect("created session listed");
    assert_eq!(row.native_name, "managed-space");
    assert_eq!(row.groups.len(), 1);
    assert_eq!(row.groups[0].handle, binding.root_group);
    assert_eq!(row.groups[0].splits.len(), 1);
    assert_eq!(row.groups[0].splits[0].handle, binding.root_split);
    assert_eq!(row.groups[0].splits[0].cwd.as_deref(), Some("/"));

    // ADR 004/005 managed options were asserted on the root window.
    let ProviderHandle::Tx(window) = binding.root_group else {
        panic!("tmux handle expected");
    };
    let target = format!("@{window}");
    let set_title = srv.tmux_ok(&[
        "show-options",
        "-w",
        "-t",
        &target,
        "-qv",
        "allow-set-title",
    ]);
    assert_eq!(set_title.trim(), "on");
    let passthrough = srv.tmux_ok(&[
        "show-options",
        "-w",
        "-t",
        &target,
        "-qv",
        "allow-passthrough",
    ]);
    assert_eq!(passthrough.trim(), "all");
}

#[test]
fn create_on_unepoched_server_is_a_typed_error() {
    if !tmux_available() {
        eprintln!("skipping: tmux not installed");
        return;
    }
    let srv = ScratchServer::new();
    srv.start_with_holder(); // running but unepoched
    let spec = CreateSpec {
        native_token: "nope".into(),
        cwd: None,
        bootstrap_argv: bootstrap_argv(),
    };
    // Without a caller-held epoch the scope is unaddressable.
    match srv.provider().create(&srv.scope(None), &spec) {
        Err(ProviderError::WrongInstance { .. }) => {}
        other => panic!("expected wrong_instance, got {other:?}"),
    }
    // With a claimed epoch the live re-read exposes the unepoched server.
    match srv
        .provider()
        .create(&srv.scope(Some(fresh_epoch())), &spec)
    {
        Err(ProviderError::EpochChanged { observed: None, .. }) => {}
        other => panic!("expected epoch_changed with observed=None, got {other:?}"),
    }
}

#[test]
fn child_create_verifies_epoch_and_rejects_mismatch() {
    if !tmux_available() {
        eprintln!("skipping: tmux not installed");
        return;
    }
    let srv = ScratchServer::new();
    srv.start_with_holder();
    let epoch = fresh_epoch();
    srv.set_epoch(epoch);
    let provider = srv.provider();
    let binding = provider
        .create(
            &srv.scope(Some(epoch)),
            &CreateSpec {
                native_token: "space".into(),
                cwd: None,
                bootstrap_argv: bootstrap_argv(),
            },
        )
        .expect("create");

    // A wrong caller-held epoch must be rejected before mutation.
    let wrong = fresh_epoch();
    let stale_binding = NativeBinding {
        server_epoch: wrong,
        ..binding.clone()
    };
    match provider.group_new(
        &srv.scope(Some(wrong)),
        &stale_binding,
        &CreateSpec {
            native_token: "build".into(),
            cwd: None,
            bootstrap_argv: bootstrap_argv(),
        },
    ) {
        Err(ProviderError::EpochChanged { expected, observed }) => {
            assert_eq!(expected, wrong);
            assert_eq!(observed, Some(epoch));
        }
        other => panic!("expected epoch_changed, got {other:?}"),
    }

    // The right epoch creates a Group on the exact session and a Split on
    // the exact pane.
    let group = provider
        .group_new(
            &srv.scope(Some(epoch)),
            &binding,
            &CreateSpec {
                native_token: "build".into(),
                cwd: Some("/".into()),
                bootstrap_argv: bootstrap_argv(),
            },
        )
        .expect("group_new");
    assert_ne!(group, binding.root_group);

    let split = provider
        .split_new(
            &srv.scope(Some(epoch)),
            &binding.root_split,
            &CreateSpec {
                native_token: String::new(),
                cwd: None,
                bootstrap_argv: bootstrap_argv(),
            },
        )
        .expect("split_new");
    assert_ne!(split, binding.root_split);

    let groups = provider
        .group_list(&srv.scope(Some(epoch)), &binding)
        .expect("group_list");
    let handles: Vec<&ProviderHandle> = groups.iter().map(|g| &g.handle).collect();
    assert!(handles.contains(&&binding.root_group));
    assert!(handles.contains(&&group));
    let root_splits = provider
        .split_list(&srv.scope(Some(epoch)), &binding.root_group)
        .expect("split_list");
    let split_handles: Vec<&ProviderHandle> = root_splits.iter().map(|s| &s.handle).collect();
    assert_eq!(split_handles, vec![&binding.root_split, &split]);
}

#[test]
fn remove_verifies_absence() {
    if !tmux_available() {
        eprintln!("skipping: tmux not installed");
        return;
    }
    let srv = ScratchServer::new();
    srv.start_with_holder();
    let epoch = fresh_epoch();
    srv.set_epoch(epoch);
    let provider = srv.provider();
    let binding = provider
        .create(
            &srv.scope(Some(epoch)),
            &CreateSpec {
                native_token: "doomed".into(),
                cwd: None,
                bootstrap_argv: bootstrap_argv(),
            },
        )
        .expect("create");

    provider
        .remove(&srv.scope(Some(epoch)), &binding)
        .expect("remove");

    let listing = srv.tmux_ok(&["list-sessions", "-F", "#{session_id}"]);
    assert!(
        !listing.lines().any(|l| l == binding.native_token),
        "session must be verifiably absent after remove: {listing}"
    );
    // Removing again converges on benign absence (ADR 005).
    provider
        .remove(&srv.scope(Some(epoch)), &binding)
        .expect("second remove is success-equivalent after verified absence");
}

#[test]
fn markers_survive_external_rename() {
    if !tmux_available() {
        eprintln!("skipping: tmux not installed");
        return;
    }
    let srv = ScratchServer::new();
    let (sid, _, _) = srv.start_with_holder();
    let epoch = fresh_epoch();
    srv.set_epoch(epoch);
    let provider = srv.provider();

    let markers = SpaceMarkers {
        host_uid: Uuid::new_v4().to_string(),
        registry_uid: Uuid::new_v4().to_string(),
        space_uid: Uuid::new_v4().to_string(),
        space_no: "7".into(),
    };
    provider
        .stamp_markers(&srv.scope(Some(epoch)), &sid, &markers)
        .expect("stamp");

    // External rename (outside dmux) must not disturb id-keyed markers.
    srv.tmux_ok(&["rename-session", "-t", &sid, "externally-renamed"]);

    let readback = provider
        .read_markers(&srv.scope(Some(epoch)), &sid)
        .expect("readback");
    assert_eq!(
        readback.host_uid.as_deref(),
        Some(markers.host_uid.as_str())
    );
    assert_eq!(
        readback.registry_uid.as_deref(),
        Some(markers.registry_uid.as_str())
    );
    assert_eq!(
        readback.space_uid.as_deref(),
        Some(markers.space_uid.as_str())
    );
    assert_eq!(readback.space_no.as_deref(), Some("7"));

    // And the immutable token still resolves the renamed session.
    let name = srv.tmux_ok(&["display-message", "-p", "-t", &sid, "#{session_name}"]);
    assert_eq!(name.trim(), "externally-renamed");
}

#[test]
fn provider_rename_is_verified_and_presentation_argv_is_exact() {
    if !tmux_available() {
        eprintln!("skipping: tmux not installed");
        return;
    }
    let srv = ScratchServer::new();
    srv.start_with_holder();
    let epoch = fresh_epoch();
    srv.set_epoch(epoch);
    let provider = srv.provider();
    let binding = provider
        .create(
            &srv.scope(Some(epoch)),
            &CreateSpec {
                native_token: "before".into(),
                cwd: None,
                bootstrap_argv: bootstrap_argv(),
            },
        )
        .expect("create");

    provider
        .rename(&srv.scope(Some(epoch)), &binding, "after rename")
        .expect("rename");
    let row = provider
        .inspect(&srv.scope(Some(epoch)), &binding)
        .expect("inspect");
    assert_eq!(row.native_name, "after rename");
    assert_eq!(row.native_token, binding.native_token);

    let target = provider
        .prepare_presentation(&srv.scope(Some(epoch)), &binding, Some(&binding.root_split))
        .expect("prepare_presentation");
    assert_eq!(
        target,
        PresentationTarget::Tmux {
            exact_argv: vec![
                "tmux".to_string(),
                "-L".to_string(),
                srv.ns.clone(),
                "attach".to_string(),
                "-t".to_string(),
                binding.native_token.clone(),
            ],
        }
    );
}

#[test]
fn capabilities_are_probed_by_running_against_the_real_server() {
    if !tmux_available() {
        eprintln!("skipping: tmux not installed");
        return;
    }
    let srv = ScratchServer::new();
    srv.start_with_holder();

    let caps = srv.provider().capabilities();
    assert_eq!(caps.backend, Backend::Tmux);
    assert!(!caps.cas_rename, "tmux never advertises CAS rename");
    for probe in [
        "exact_id_targeting",
        "session_options",
        "allow_passthrough_all",
        "detach_client",
    ] {
        assert!(
            caps.probed.iter().any(|p| p == probe),
            "missing probed capability {probe}; got {:?}",
            caps.probed
        );
    }

    // The probe session cleaned up after itself.
    let listing = srv.tmux_ok(&["list-sessions", "-F", "#{session_name}"]);
    assert!(
        !listing.contains("dmux-probe-"),
        "probe session must be removed: {listing}"
    );
}

// -- P5 epoch bootstrap primitives (plan §11.2) ------------------------------
//
// These tests exercise the `dmux _tmux-bootstrap` building blocks on a real
// scratch server (`tmux -L dmux-p5tx-<pid>-<n>`). The kernel lock the real
// bootstrap holds is the CALLER's job and is not simulated here; the
// primitives themselves are lock-free.

#[test]
fn epoch_bootstrap_sets_once_and_verifies_identity_and_epoch() {
    if !tmux_available() {
        eprintln!("skipping: tmux not installed");
        return;
    }
    let srv = ScratchServer::with_prefix("dmux-p5tx");
    srv.start_with_holder();
    let provider = srv.provider();

    // Identity probe on the running incarnation; read-only.
    let id = provider.server_identity(&srv.ns).expect("server_identity");
    assert!(id.pid > 0);
    let start: u64 = id
        .start_token
        .parse()
        .expect("start_token is #{start_time}: whole seconds since the epoch");
    assert!(
        start > 1_500_000_000,
        "implausible server start time {start}"
    );
    // Parsing cross-check against a directly issued listing.
    let raw = srv.tmux_ok(&["list-sessions", "-F", "#{pid}"]);
    assert_eq!(raw.lines().next().unwrap(), id.pid.to_string());

    // Fresh server → epoch absent → Set, verified by native readback.
    let e1 = fresh_epoch();
    assert_eq!(
        provider
            .set_epoch_if_absent(&srv.ns, e1)
            .expect("first set"),
        EpochSetOutcome::Set
    );
    let readback = srv.tmux_ok(&["show-options", "-gqv", "@dmux_server_epoch"]);
    assert_eq!(readback.trim(), e1.0.to_string());

    // Second bootstrap attempt observes the first winner.
    assert_eq!(
        provider
            .set_epoch_if_absent(&srv.ns, fresh_epoch())
            .expect("second call"),
        EpochSetOutcome::AlreadySet(e1)
    );

    // verify_epoch: success, then each mismatch case typed.
    provider
        .verify_epoch(&srv.ns, e1, &id)
        .expect("matching identity and epoch");
    match provider.verify_epoch(&srv.ns, fresh_epoch(), &id) {
        Err(ProviderError::EpochChanged { observed, .. }) => {
            assert_eq!(observed, Some(e1));
        }
        other => panic!("expected epoch_changed, got {other:?}"),
    }
    let wrong_pid = TmuxServerIdentity {
        pid: id.pid.wrapping_add(1),
        start_token: id.start_token.clone(),
    };
    match provider.verify_epoch(&srv.ns, e1, &wrong_pid) {
        Err(ProviderError::WrongInstance { .. }) => {}
        other => panic!("expected wrong_instance for pid mismatch, got {other:?}"),
    }
    let wrong_start = TmuxServerIdentity {
        pid: id.pid,
        start_token: "1".into(),
    };
    match provider.verify_epoch(&srv.ns, e1, &wrong_start) {
        Err(ProviderError::WrongInstance { .. }) => {}
        other => panic!("expected wrong_instance for start-token mismatch, got {other:?}"),
    }
}

#[test]
fn external_epoch_writer_first_wins_and_malformed_value_is_typed() {
    if !tmux_available() {
        eprintln!("skipping: tmux not installed");
        return;
    }
    let srv = ScratchServer::with_prefix("dmux-p5tx");
    srv.start_with_holder();
    let provider = srv.provider();

    // An external actor (not dmux) installed an epoch before our bootstrap.
    let external = fresh_epoch();
    srv.set_epoch(external);
    assert_eq!(
        provider
            .set_epoch_if_absent(&srv.ns, fresh_epoch())
            .expect("bootstrap after external write"),
        EpochSetOutcome::AlreadySet(external)
    );

    // A malformed existing value is a typed error and is never overwritten.
    srv.tmux_ok(&["set-option", "-g", "@dmux_server_epoch", "not-a-uuid"]);
    match provider.set_epoch_if_absent(&srv.ns, fresh_epoch()) {
        Err(ProviderError::NativeFailure { detail }) => {
            assert!(detail.contains("@dmux_server_epoch"), "{detail}");
        }
        other => panic!("expected native_failure, got {other:?}"),
    }
    let still = srv.tmux_ok(&["show-options", "-gqv", "@dmux_server_epoch"]);
    assert_eq!(still.trim(), "not-a-uuid", "malformed value left untouched");
}

#[test]
fn fresh_incarnation_has_no_epoch_and_a_new_identity() {
    if !tmux_available() {
        eprintln!("skipping: tmux not installed");
        return;
    }
    let srv = ScratchServer::with_prefix("dmux-p5tx");
    srv.start_with_holder();
    let provider = srv.provider();

    let id1 = provider.server_identity(&srv.ns).expect("identity 1");
    let e1 = fresh_epoch();
    assert_eq!(
        provider.set_epoch_if_absent(&srv.ns, e1).expect("set 1"),
        EpochSetOutcome::Set
    );

    // Restart the incarnation on the SAME socket namespace. #{start_time}
    // has whole-second resolution, so cross a second boundary to prove the
    // start token itself changes, independent of the pid.
    srv.tmux_ok(&["kill-server"]);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    srv.start_with_holder();

    // Global options died with the old server: the epoch is absent again.
    let globals = srv.tmux_ok(&["show-options", "-g"]);
    assert!(
        !globals.contains("@dmux_server_epoch"),
        "fresh incarnation must be unepoched; globals:\n{globals}"
    );

    let id2 = provider.server_identity(&srv.ns).expect("identity 2");
    assert_ne!(id1, id2, "new incarnation must have a new identity");
    assert_ne!(
        id1.start_token, id2.start_token,
        "start token must change across restarts"
    );

    // Stale identity/epoch from the previous incarnation are rejected before
    // any child mutation could run.
    match provider.verify_epoch(&srv.ns, e1, &id1) {
        Err(ProviderError::WrongInstance { .. }) => {}
        other => panic!("expected wrong_instance for stale incarnation, got {other:?}"),
    }

    // And the new incarnation bootstraps cleanly.
    let e2 = fresh_epoch();
    assert_eq!(
        provider.set_epoch_if_absent(&srv.ns, e2).expect("set 2"),
        EpochSetOutcome::Set
    );
    provider
        .verify_epoch(&srv.ns, e2, &id2)
        .expect("new binding verifies");
}
