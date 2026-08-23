//! Real-tmux integration tests for the P3a adapter (plan §18 P3a, §11.2).
//!
//! Every test runs on its own scratch socket namespace
//! (`tmux -L dmux-p3a-<pid>-<n>`) and kills that server from a Drop guard so
//! cleanup happens on panic paths too. The user's default tmux server is
//! never touched. Tests soft-skip when no tmux binary is installed.

use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use dmux::backend::tmux::{
    EpochSetOutcome, ExpectedIncarnation, SpaceMarkers, SystemRunner, TmuxProvider,
    TmuxServerIdentity,
};
use dmux::backend::{
    CreateSpec, InventoryOutcome, InventoryScope, NativeBinding, Provider, ProviderError,
    SplitDirection, SplitSpec,
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
    /// When set, every harness command carries `-f /dev/null`, so the
    /// command that starts the server pins default config. The P8a geometry
    /// and cwd tests need this: the user's tmux.conf must not leak
    /// base-index, default-size, status, or border settings into pane
    /// coordinate assertions. (Config is only read at server start; the
    /// provider's own commands run against the already-started server.)
    default_config: bool,
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
            default_config: false,
        }
    }

    /// P8a behavioral tests: `dmux-p8a-<pid>-<n>` namespace started with
    /// `-f /dev/null` for deterministic geometry.
    fn p8a() -> Self {
        let mut srv = Self::with_prefix("dmux-p8a");
        srv.default_config = true;
        srv
    }

    fn tmux(&self, args: &[&str]) -> std::process::Output {
        let mut cmd = Command::new("tmux");
        cmd.arg("-L").arg(&self.ns);
        if self.default_config {
            cmd.arg("-f").arg("/dev/null");
        }
        cmd.args(args)
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
        match expected {
            Some(epoch) => InventoryScope::managed(Backend::Tmux, self.ns.clone(), epoch),
            None => InventoryScope::unmanaged_endpoint(Backend::Tmux, self.ns.clone()),
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
            }
            .into(),
        )
        .expect("split_new");
    assert_ne!(split, binding.root_split);

    let groups = provider
        .inspect(&srv.scope(Some(epoch)), &binding)
        .expect("inspect")
        .groups;
    let handles: Vec<&ProviderHandle> = groups.iter().map(|g| &g.handle).collect();
    assert!(handles.contains(&&binding.root_group));
    assert!(handles.contains(&&group));
    let root_splits = provider
        .split_list(&srv.scope(Some(epoch)), &binding.root_group)
        .expect("split_list");
    let split_handles: Vec<&ProviderHandle> = root_splits.iter().map(|s| &s.handle).collect();
    assert_eq!(split_handles, vec![&binding.root_split, &split]);
}

/// ADR 012 WS-A.8 / WS-E.2 (review finding #5; report 08 §7): the
/// `binding_epoch`-fenced verbs the review could prove only by call chain
/// (`rename`, `remove`, `inspect`; `prepare_presentation` and `group_list`
/// were driven here too until WS-E.3 row 9 retired them from the trait),
/// driven against a real server that was RESTARTED behind the binding. The
/// binding was minted under E1; the namespace is then served by a fresh
/// incarnation at E2 that recycles the same session name and — ids restart
/// from `$0` — the same session id. Pinned to E2, every verb refuses
/// `EpochChanged { expected: E2, observed: E1 }`; unpinned, every verb
/// refuses `WrongInstance` (the binding's own epoch used to be the pin, so
/// the server was fenced against its own word). Either way the impostor is
/// untouched: same sessions, same names, same windows. The same session id
/// re-recorded under E2 — what `dmux repair rebind` produces — is served.
#[test]
fn binding_verbs_refuse_a_binding_from_a_previous_incarnation_on_a_real_server() {
    if !tmux_available() {
        eprintln!("skipping: tmux not installed");
        return;
    }
    let srv = ScratchServer::new();
    srv.start_with_holder();
    let e1 = fresh_epoch();
    srv.set_epoch(e1);
    let provider = srv.provider();
    let binding = provider
        .create(
            &srv.scope(Some(e1)),
            &CreateSpec {
                native_token: "proj".into(),
                cwd: None,
                bootstrap_argv: bootstrap_argv(),
            },
        )
        .expect("create");

    // Restart the namespace: a fresh incarnation, the same session name,
    // the same session id.
    srv.tmux_ok(&["kill-server"]);
    srv.start_with_holder();
    let (sid, _, _) = srv.spawn_session("proj");
    assert_eq!(
        sid, binding.native_token,
        "the impostor recycles the session id"
    );
    let e2 = fresh_epoch();
    srv.set_epoch(e2);
    let snapshot = || {
        (
            srv.tmux_ok(&["list-sessions", "-F", "#{session_id}|#{session_name}"]),
            srv.tmux_ok(&["list-windows", "-a", "-F", "#{window_id}"]),
        )
    };
    let before = snapshot();

    let drive = |scope: &InventoryScope| -> Vec<(&'static str, Result<(), ProviderError>)> {
        vec![
            ("rename", provider.rename(scope, &binding, "renamed")),
            ("remove", provider.remove(scope, &binding)),
            ("inspect", provider.inspect(scope, &binding).map(|_| ())),
        ]
    };
    for (verb, result) in drive(&srv.scope(Some(e2))) {
        match result {
            Err(ProviderError::EpochChanged { expected, observed }) => {
                assert_eq!(expected, e2, "{verb}");
                assert_eq!(observed, Some(e1), "{verb}");
            }
            other => panic!("{verb}: expected epoch_changed, got {other:?}"),
        }
    }
    for (verb, result) in drive(&srv.scope(None)) {
        match result {
            Err(ProviderError::WrongInstance { detail }) => {
                assert!(detail.contains("managed scope"), "{verb}: {detail}");
            }
            other => panic!("{verb}: expected wrong_instance, got {other:?}"),
        }
    }
    assert_eq!(snapshot(), before, "the impostor was never touched");

    // Positive control: the id re-recorded under the live incarnation.
    let rebound = NativeBinding {
        server_epoch: e2,
        ..binding.clone()
    };
    let row = provider
        .inspect(&srv.scope(Some(e2)), &rebound)
        .expect("inspect under the live incarnation");
    assert_eq!(row.native_token, binding.native_token);
    assert_eq!(row.native_name, "proj");
}

/// ADR 012 WS-A.9 at the adapter (review finding #11): a server restarted
/// on the same namespace re-binds the same socket PATH with a fresh inode.
/// With the published incarnation handed in, the adapter refuses the
/// impostor even when the old `@dmux_server_epoch` was copied onto it — by
/// the socket witness alone when pid/start token are made to agree, and on
/// every read: `verify_incarnation`, `inventory` (`unreachable` with a
/// `stale_incarnation` detail) and `read_markers`. The live incarnation's
/// own witnesses verify.
#[test]
fn a_restarted_server_on_the_same_socket_path_is_refused_by_its_inode_even_with_the_copied_epoch() {
    if !tmux_available() {
        eprintln!("skipping: tmux not installed");
        return;
    }
    let srv = ScratchServer::new();
    srv.start_with_holder();
    let e1 = fresh_epoch();
    srv.set_epoch(e1);
    let provider = srv.provider();
    let before = provider
        .server_incarnation(&srv.ns)
        .expect("live incarnation");
    let published = ExpectedIncarnation::from_live(&before);
    provider
        .verify_incarnation(&srv.ns, e1, &published)
        .expect("the live incarnation verifies");
    let pinned = srv.provider().with_expected_incarnation(published.clone());
    assert!(matches!(
        pinned.inventory(&srv.scope(Some(e1))),
        InventoryOutcome::Complete(_)
    ));

    // Restart on the same namespace; copy the old epoch onto the impostor.
    srv.tmux_ok(&["kill-server"]);
    let (holder, _, _) = srv.start_with_holder();
    srv.set_epoch(e1);
    let after = provider
        .server_incarnation(&srv.ns)
        .expect("impostor incarnation");
    assert_eq!(after.socket_path, before.socket_path, "same path");
    assert_ne!(after.socket_ino, before.socket_ino, "fresh inode");

    // The socket witness alone: identity made to agree, only dev/ino stale.
    let socket_only = ExpectedIncarnation {
        identity: after.identity.clone(),
        socket: published.socket,
    };
    match provider.verify_incarnation(&srv.ns, e1, &socket_only) {
        Err(ProviderError::WrongInstance { detail }) => {
            assert!(detail.starts_with("stale_incarnation: "), "{detail}");
            assert!(detail.contains("re-bound"), "{detail}");
        }
        other => panic!("socket witness: expected wrong_instance, got {other:?}"),
    }
    // The published row as a whole: refused on identity first.
    match provider.verify_incarnation(&srv.ns, e1, &published) {
        Err(ProviderError::WrongInstance { detail }) => {
            assert!(detail.starts_with("stale_incarnation: "), "{detail}");
        }
        other => panic!("published row: expected wrong_instance, got {other:?}"),
    }
    // Reads with the published incarnation installed.
    match pinned.inventory(&srv.scope(Some(e1))) {
        InventoryOutcome::Unreachable { detail } => {
            assert!(detail.starts_with("stale_incarnation: "), "{detail}");
        }
        other => panic!("inventory: expected unreachable, got {other:?}"),
    }
    match pinned.read_markers(&srv.scope(Some(e1)), &holder) {
        Err(ProviderError::WrongInstance { detail }) => {
            assert!(detail.starts_with("stale_incarnation: "), "{detail}");
        }
        other => panic!("read_markers: expected wrong_instance, got {other:?}"),
    }

    // The live incarnation's witnesses verify.
    let relive = ExpectedIncarnation::from_live(&after);
    provider
        .verify_incarnation(&srv.ns, e1, &relive)
        .expect("the impostor verifies as itself");
    let repinned = srv.provider().with_expected_incarnation(relive);
    assert!(matches!(
        repinned.inventory(&srv.scope(Some(e1))),
        InventoryOutcome::Complete(_)
    ));
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
fn provider_rename_is_verified_by_inspect() {
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

// -- P8a child-operation behavior (plan §7.2, §11.2, §11.3) -------------------
//
// Live pins the Group/Split orchestration relies on: real split geometry for
// every direction plus `-l N%`, exact-pane target semantics, cwd inheritance
// (explicit and None), typed NotFound for stale handles, and marker/handle
// stability across `move-window` and session rename. Scratch servers use
// `-f /dev/null` (ScratchServer::p8a) so geometry is deterministic.

/// Unwrap a tmux numeric handle.
fn tx(handle: &ProviderHandle) -> u64 {
    match handle {
        ProviderHandle::Tx(n) => *n,
        other => panic!("expected tmux handle, got {other}"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Geom {
    left: u32,
    top: u32,
    width: u32,
    height: u32,
}

/// Real pane geometry by exact pane id. Uses a filtered `list-panes -a`
/// because `display-message -p -t %N` silently ignores a bad pane target on
/// tmux 3.7b (probed: it prints the message with exit 0).
fn pane_geometry(srv: &ScratchServer, pane: u64) -> Geom {
    let listing = srv.tmux_ok(&[
        "list-panes",
        "-a",
        "-F",
        "#{pane_id} #{pane_left} #{pane_top} #{pane_width} #{pane_height}",
    ]);
    let needle = format!("%{pane} ");
    let row = listing
        .lines()
        .find(|l| l.starts_with(&needle))
        .unwrap_or_else(|| panic!("pane %{pane} not listed:\n{listing}"));
    let nums: Vec<u32> = row
        .split_whitespace()
        .skip(1)
        .map(|n| n.parse().expect("numeric geometry field"))
        .collect();
    Geom {
        left: nums[0],
        top: nums[1],
        width: nums[2],
        height: nums[3],
    }
}

/// Real pane cwd by exact pane id (`#{pane_current_path}`).
fn pane_cwd(srv: &ScratchServer, pane: u64) -> String {
    let listing = srv.tmux_ok(&[
        "list-panes",
        "-a",
        "-F",
        "#{pane_id}__DMUX_FIELD_7F4A9C2E__#{pane_current_path}",
    ]);
    let needle = format!("%{pane}__DMUX_FIELD_7F4A9C2E__");
    listing
        .lines()
        .find_map(|l| l.strip_prefix(&needle))
        .unwrap_or_else(|| panic!("pane %{pane} not listed:\n{listing}"))
        .to_string()
}

fn split_spec(direction: SplitDirection, percent: Option<u8>, cwd: Option<String>) -> SplitSpec {
    SplitSpec {
        spec: CreateSpec {
            native_token: String::new(),
            cwd,
            bootstrap_argv: bootstrap_argv(),
        },
        direction,
        percent,
    }
}

#[test]
fn split_new_directions_and_percent_shape_real_geometry() {
    if !tmux_available() {
        eprintln!("skipping: tmux not installed");
        return;
    }
    let srv = ScratchServer::p8a();
    srv.start_with_holder();
    let epoch = fresh_epoch();
    srv.set_epoch(epoch);
    let provider = srv.provider();
    let scope = srv.scope(Some(epoch));
    let binding = provider
        .create(
            &scope,
            &CreateSpec {
                native_token: "geometry".into(),
                cwd: None,
                bootstrap_argv: bootstrap_argv(),
            },
        )
        .expect("create");

    const PERCENT: u8 = 30;
    for direction in [
        SplitDirection::Down,
        SplitDirection::Up,
        SplitDirection::Right,
        SplitDirection::Left,
    ] {
        // Fresh single-pane window per direction: clean 80x24 geometry.
        let group = provider
            .group_new(
                &scope,
                &binding,
                &CreateSpec {
                    native_token: String::new(),
                    cwd: None,
                    bootstrap_argv: bootstrap_argv(),
                },
            )
            .expect("group_new");
        let splits = provider.split_list(&scope, &group).expect("split_list");
        assert_eq!(splits.len(), 1, "fresh window has exactly the root pane");
        let target = splits[0].handle.clone();
        let before = pane_geometry(&srv, tx(&target));

        let new = provider
            .split_new(&scope, &target, &split_spec(direction, Some(PERCENT), None))
            .expect("split_new");
        assert_ne!(new, target);
        let new_g = pane_geometry(&srv, tx(&new));
        let target_g = pane_geometry(&srv, tx(&target));

        // Placement relative to the (resized) target pane, plus off-axis
        // sanity: a vertical split never moves the horizontal edges and
        // vice versa.
        match direction {
            SplitDirection::Down => {
                assert!(
                    new_g.top > target_g.top,
                    "{direction:?}: {new_g:?} vs {target_g:?}"
                );
                assert_eq!(new_g.left, before.left, "{direction:?}");
                assert_eq!(new_g.width, before.width, "{direction:?}");
            }
            SplitDirection::Up => {
                assert!(
                    new_g.top < target_g.top,
                    "{direction:?}: {new_g:?} vs {target_g:?}"
                );
                assert_eq!(new_g.left, before.left, "{direction:?}");
                assert_eq!(new_g.width, before.width, "{direction:?}");
            }
            SplitDirection::Right => {
                assert!(
                    new_g.left > target_g.left,
                    "{direction:?}: {new_g:?} vs {target_g:?}"
                );
                assert_eq!(new_g.top, before.top, "{direction:?}");
                assert_eq!(new_g.height, before.height, "{direction:?}");
            }
            SplitDirection::Left => {
                assert!(
                    new_g.left < target_g.left,
                    "{direction:?}: {new_g:?} vs {target_g:?}"
                );
                assert_eq!(new_g.top, before.top, "{direction:?}");
                assert_eq!(new_g.height, before.height, "{direction:?}");
            }
        }

        // `-l N%` sizes the NEW pane at N% of the target's pre-split extent
        // on the split axis (probed on 3.7b: 30% of h24 -> 7, of w80 -> 24);
        // allow +-2 cells for integer truncation and the border line.
        let (new_size, before_size) = match direction {
            SplitDirection::Down | SplitDirection::Up => (new_g.height, before.height),
            SplitDirection::Right | SplitDirection::Left => (new_g.width, before.width),
        };
        let wanted = (before_size * u32::from(PERCENT)) / 100;
        assert!(
            new_size.abs_diff(wanted) <= 2,
            "{direction:?}: new pane size {new_size} not within 2 of {PERCENT}% of {before_size}"
        );

        // The target shrank on the split axis; both panes tile its extent.
        let target_size = match direction {
            SplitDirection::Down | SplitDirection::Up => target_g.height,
            SplitDirection::Right | SplitDirection::Left => target_g.width,
        };
        assert_eq!(
            target_size + new_size + 1,
            before_size,
            "{direction:?}: target+new+border must tile the pre-split extent"
        );
    }
}

#[test]
fn split_and_group_cwd_explicit_wins_and_none_inherits_client_cwd() {
    if !tmux_available() {
        eprintln!("skipping: tmux not installed");
        return;
    }
    let srv = ScratchServer::p8a();
    srv.start_with_holder();
    let epoch = fresh_epoch();
    srv.set_epoch(epoch);
    let provider = srv.provider();
    let scope = srv.scope(Some(epoch));

    // Canonicalized scratch dirs (macOS /tmp and /var are symlinks;
    // #{pane_current_path} reports the resolved path).
    let mk = |tag: &str| {
        let dir = std::env::temp_dir().join(format!("dmux-p8a-cwd-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir.canonicalize().expect("canonicalize")
    };
    let dir_a = mk("a");
    let dir_b = mk("b");
    let client_cwd = std::env::current_dir()
        .expect("current_dir")
        .canonicalize()
        .expect("canonicalize cwd");
    assert_ne!(dir_a, client_cwd);
    assert_ne!(dir_b, client_cwd);

    // Space created with an explicit cwd: the root pane starts there and it
    // becomes the session working directory.
    let binding = provider
        .create(
            &scope,
            &CreateSpec {
                native_token: "cwd-space".into(),
                cwd: Some(dir_a.display().to_string()),
                bootstrap_argv: bootstrap_argv(),
            },
        )
        .expect("create");
    assert_eq!(
        pane_cwd(&srv, tx(&binding.root_split)),
        dir_a.display().to_string()
    );

    // group_new with an explicit cwd starts its pane there.
    let group = provider
        .group_new(
            &scope,
            &binding,
            &CreateSpec {
                native_token: String::new(),
                cwd: Some(dir_b.display().to_string()),
                bootstrap_argv: bootstrap_argv(),
            },
        )
        .expect("group_new");
    let group_pane = provider.split_list(&scope, &group).expect("split_list")[0]
        .handle
        .clone();
    assert_eq!(pane_cwd(&srv, tx(&group_pane)), dir_b.display().to_string());

    // split_new with an explicit cwd starts its pane there, wherever the
    // target pane lives.
    let explicit = provider
        .split_new(
            &scope,
            &group_pane,
            &split_spec(
                SplitDirection::Down,
                None,
                Some(dir_a.display().to_string()),
            ),
        )
        .expect("split_new explicit cwd");
    assert_eq!(pane_cwd(&srv, tx(&explicit)), dir_a.display().to_string());

    // PINNED: with cwd None, tmux resolves the start directory from the
    // invoking COMMAND CLIENT's cwd (this process) — NOT the target pane's
    // path (dir_b) and NOT the session working directory (dir_a). tmux
    // server_client_get_cwd prefers a sessionless client's cwd; the provider
    // spawns `tmux split-window`/`new-window` as such a client. Orchestration
    // must therefore always compute and pass the §11.3 inheritance cwd
    // (target Split cwd / Space default) explicitly; None is only
    // "wherever the dmux process happens to run".
    let inherited_split = provider
        .split_new(
            &scope,
            &group_pane,
            &split_spec(SplitDirection::Down, None, None),
        )
        .expect("split_new cwd None");
    let observed = pane_cwd(&srv, tx(&inherited_split));
    assert_eq!(observed, client_cwd.display().to_string());
    assert_ne!(observed, dir_a.display().to_string());
    assert_ne!(observed, dir_b.display().to_string());

    // Same pin for group_new with cwd None.
    let inherited_group = provider
        .group_new(
            &scope,
            &binding,
            &CreateSpec {
                native_token: String::new(),
                cwd: None,
                bootstrap_argv: bootstrap_argv(),
            },
        )
        .expect("group_new cwd None");
    let pane = provider
        .split_list(&scope, &inherited_group)
        .expect("split_list")[0]
        .handle
        .clone();
    assert_eq!(pane_cwd(&srv, tx(&pane)), client_cwd.display().to_string());

    let _ = std::fs::remove_dir(&dir_a);
    let _ = std::fs::remove_dir(&dir_b);
}

#[test]
fn split_new_targets_the_exact_pane_and_reads_window_numbers_as_panes() {
    if !tmux_available() {
        eprintln!("skipping: tmux not installed");
        return;
    }
    let srv = ScratchServer::p8a();
    srv.start_with_holder(); // $0 @0 %0
    let epoch = fresh_epoch();
    srv.set_epoch(epoch);
    let provider = srv.provider();
    let scope = srv.scope(Some(epoch));
    let binding = provider
        .create(
            &scope,
            &CreateSpec {
                native_token: "target-space".into(),
                cwd: None,
                bootstrap_argv: bootstrap_argv(),
            },
        )
        .expect("create"); // $1 @1 %1
    let pane_a = binding.root_split.clone();

    // First split makes pane B — which is now the window's ACTIVE pane
    // (split-window without -d activates the new pane).
    let pane_b = provider
        .split_new(
            &scope,
            &pane_a,
            &split_spec(SplitDirection::Down, None, None),
        )
        .expect("first split");
    let active = srv.tmux_ok(&[
        "display-message",
        "-p",
        "-t",
        &binding.native_token,
        "#{pane_id}",
    ]);
    assert_eq!(
        active.trim(),
        format!("%{}", tx(&pane_b)),
        "precondition: the NEW pane is the active pane"
    );

    // PINNED: splitting pane A splits exactly A — not the active pane B.
    let a_before = pane_geometry(&srv, tx(&pane_a));
    let b_before = pane_geometry(&srv, tx(&pane_b));
    let pane_c = provider
        .split_new(
            &scope,
            &pane_a,
            &split_spec(SplitDirection::Right, None, None),
        )
        .expect("split of non-active pane A");
    let a_after = pane_geometry(&srv, tx(&pane_a));
    let c_geom = pane_geometry(&srv, tx(&pane_c));
    assert!(a_after.width < a_before.width, "A was split: {a_after:?}");
    assert_eq!(a_after.top, a_before.top);
    assert!(
        c_geom.left > a_before.left && c_geom.left < a_before.left + a_before.width,
        "C sits inside A's former horizontal extent: {c_geom:?} vs {a_before:?}"
    );
    assert_eq!(
        pane_geometry(&srv, tx(&pane_b)),
        b_before,
        "the active pane B is untouched — targeting is exact, never active-pane fallback"
    );

    // PINNED: handle kinds are positional (`ProviderHandle::Tx` carries only
    // a number), so split_new ALWAYS interprets its parent handle in the
    // PANE namespace (`%N`). A window handle `@N` cannot be expressed
    // distinctly: passing a Group's number targets pane %N. Construct a live
    // window @W whose number-twin pane %W is dead and prove the split fails
    // NotFound on the pane while the window exists.
    let group_w = provider
        .group_new(
            &scope,
            &binding,
            &CreateSpec {
                native_token: "alive-window".into(),
                cwd: None,
                bootstrap_argv: bootstrap_argv(),
            },
        )
        .expect("group_new"); // window @2, pane %3
    let w = tx(&group_w);
    // Pane %W lives in a DIFFERENT window (it is one of $1's root-window
    // panes) — kill it while window @W stays alive.
    let listing = srv.tmux_ok(&["list-panes", "-a", "-F", "#{pane_id}"]);
    assert!(
        listing.lines().any(|l| l == format!("%{w}")),
        "test topology: pane %{w} must exist before the kill:\n{listing}"
    );
    provider
        .split_remove(&scope, &ProviderHandle::Tx(w))
        .expect("kill number-twin pane");
    let groups = provider.inspect(&scope, &binding).expect("inspect").groups;
    assert!(
        groups.iter().any(|g| g.handle == group_w),
        "window @{w} must still be alive"
    );
    match provider.split_new(
        &scope,
        &group_w,
        &split_spec(SplitDirection::Down, None, None),
    ) {
        Err(ProviderError::NotFound { native_ref }) => {
            assert!(
                native_ref.contains("pane"),
                "the lookup failed in the PANE namespace, proving a window \
                 handle is never split: {native_ref}"
            );
        }
        other => panic!("expected not_found for pane %{w}, got {other:?}"),
    }
}

#[test]
fn stale_child_handles_fail_typed_not_found() {
    if !tmux_available() {
        eprintln!("skipping: tmux not installed");
        return;
    }
    let srv = ScratchServer::p8a();
    srv.start_with_holder();
    let epoch = fresh_epoch();
    srv.set_epoch(epoch);
    let provider = srv.provider();
    let scope = srv.scope(Some(epoch));
    let binding = provider
        .create(
            &scope,
            &CreateSpec {
                native_token: "stale-space".into(),
                cwd: None,
                bootstrap_argv: bootstrap_argv(),
            },
        )
        .expect("create");

    // A pane killed behind dmux's back: every pane-targeting operation
    // returns typed NotFound, never NativeFailure.
    let dead_pane = provider
        .split_new(
            &scope,
            &binding.root_split,
            &split_spec(SplitDirection::Down, None, None),
        )
        .expect("split_new");
    srv.tmux_ok(&["kill-pane", "-t", &format!("%{}", tx(&dead_pane))]);
    match provider.split_new(
        &scope,
        &dead_pane,
        &split_spec(SplitDirection::Down, None, None),
    ) {
        Err(ProviderError::NotFound { .. }) => {}
        other => panic!("split_new on dead pane: expected not_found, got {other:?}"),
    }
    match provider.split_activate(&scope, &dead_pane) {
        Err(ProviderError::NotFound { .. }) => {}
        other => panic!("split_activate on dead pane: expected not_found, got {other:?}"),
    }
    // Removes converge on benign absence instead (ADR 005 pin).
    provider
        .split_remove(&scope, &dead_pane)
        .expect("split_remove of an already-dead pane is success-equivalent");

    // A window killed behind dmux's back.
    let dead_group = provider
        .group_new(
            &scope,
            &binding,
            &CreateSpec {
                native_token: "doomed".into(),
                cwd: None,
                bootstrap_argv: bootstrap_argv(),
            },
        )
        .expect("group_new");
    srv.tmux_ok(&["kill-window", "-t", &format!("@{}", tx(&dead_group))]);
    match provider.group_activate(&scope, &dead_group) {
        Err(ProviderError::NotFound { .. }) => {}
        other => panic!("group_activate on dead window: expected not_found, got {other:?}"),
    }
    match provider.group_rename(&scope, &dead_group, "zombie") {
        Err(ProviderError::NotFound { .. }) => {}
        other => panic!("group_rename on dead window: expected not_found, got {other:?}"),
    }
    match provider.split_list(&scope, &dead_group) {
        Err(ProviderError::NotFound { .. }) => {}
        other => panic!("split_list on dead window: expected not_found, got {other:?}"),
    }
    provider
        .group_remove(&scope, &dead_group)
        .expect("group_remove of an already-dead window is success-equivalent");

    // A session killed behind dmux's back: session-scoped child operations
    // are NotFound too.
    srv.tmux_ok(&["kill-session", "-t", &binding.native_token]);
    match provider.inspect(&scope, &binding) {
        Err(ProviderError::NotFound { .. }) => {}
        other => panic!("inspect on dead session: expected not_found, got {other:?}"),
    }
    match provider.group_new(
        &scope,
        &binding,
        &CreateSpec {
            native_token: String::new(),
            cwd: None,
            bootstrap_argv: bootstrap_argv(),
        },
    ) {
        Err(ProviderError::NotFound { .. }) => {}
        other => panic!("group_new on dead session: expected not_found, got {other:?}"),
    }
}

#[test]
fn markers_and_child_handles_survive_move_window_and_rename() {
    if !tmux_available() {
        eprintln!("skipping: tmux not installed");
        return;
    }
    let srv = ScratchServer::p8a();
    srv.start_with_holder();
    let epoch = fresh_epoch();
    srv.set_epoch(epoch);
    let provider = srv.provider();
    let scope = srv.scope(Some(epoch));

    let mk_space = |name: &str| {
        provider
            .create(
                &scope,
                &CreateSpec {
                    native_token: name.into(),
                    cwd: None,
                    bootstrap_argv: bootstrap_argv(),
                },
            )
            .expect("create")
    };
    let mk_markers = |no: &str| SpaceMarkers {
        host_uid: Uuid::new_v4().to_string(),
        registry_uid: Uuid::new_v4().to_string(),
        space_uid: Uuid::new_v4().to_string(),
        space_no: no.into(),
    };
    let src = mk_space("src");
    let dst = mk_space("dst");
    let src_markers = mk_markers("1");
    let dst_markers = mk_markers("2");
    provider
        .stamp_markers(&scope, &src.native_token, &src_markers)
        .expect("stamp src");
    provider
        .stamp_markers(&scope, &dst.native_token, &dst_markers)
        .expect("stamp dst");

    // A Group created in src, then moved across sessions externally.
    let moved = provider
        .group_new(
            &scope,
            &src,
            &CreateSpec {
                native_token: "mover".into(),
                cwd: None,
                bootstrap_argv: bootstrap_argv(),
            },
        )
        .expect("group_new");
    let moved_pane = provider.split_list(&scope, &moved).expect("split_list")[0]
        .handle
        .clone();
    srv.tmux_ok(&[
        "move-window",
        "-s",
        &format!("@{}", tx(&moved)),
        "-t",
        &format!("{}:9", dst.native_token),
    ]);

    // Markers are session-scoped options keyed by `$N`: unaffected on both
    // sides of the move.
    let src_back = provider
        .read_markers(&scope, &src.native_token)
        .expect("read src");
    assert_eq!(
        src_back.space_uid.as_deref(),
        Some(src_markers.space_uid.as_str())
    );
    assert_eq!(src_back.space_no.as_deref(), Some("1"));
    let dst_back = provider
        .read_markers(&scope, &dst.native_token)
        .expect("read dst");
    assert_eq!(
        dst_back.space_uid.as_deref(),
        Some(dst_markers.space_uid.as_str())
    );
    assert_eq!(dst_back.space_no.as_deref(), Some("2"));

    // PINNED: `move-window` preserves the window id `@N` and its pane ids;
    // group_list/split_list reflect the post-move parentage exactly.
    let src_groups = provider.inspect(&scope, &src).expect("src inspect").groups;
    assert!(
        !src_groups.iter().any(|g| g.handle == moved),
        "moved window must leave the source session"
    );
    let dst_groups = provider.inspect(&scope, &dst).expect("dst inspect").groups;
    let moved_row = dst_groups
        .iter()
        .find(|g| g.handle == moved)
        .expect("moved window listed under destination with the SAME @N");
    assert_eq!(moved_row.title.as_deref(), Some("mover"));
    let moved_splits = provider
        .split_list(&scope, &moved)
        .expect("moved split_list");
    assert_eq!(
        moved_splits.iter().map(|s| &s.handle).collect::<Vec<_>>(),
        vec![&moved_pane],
        "pane ids survive the move"
    );

    // External session rename after the move changes nothing addressed by
    // immutable ids: markers, group_list, split_list, inspect all still work.
    srv.tmux_ok(&["rename-session", "-t", &dst.native_token, "renamed dst"]);
    let renamed_back = provider
        .read_markers(&scope, &dst.native_token)
        .expect("read after rename");
    assert_eq!(
        renamed_back.space_uid.as_deref(),
        Some(dst_markers.space_uid.as_str())
    );
    let row = provider
        .inspect(&scope, &dst)
        .expect("inspect after rename");
    assert!(row.groups.iter().any(|g| g.handle == moved));
    assert_eq!(row.native_name, "renamed dst");

    assert_eq!(row.native_token, dst.native_token);
    assert!(
        provider
            .split_list(&scope, &moved)
            .expect("split_list after rename")
            .iter()
            .any(|s| s.handle == moved_pane)
    );
}
