//! Real wezterm-mux-server integration tests for the P3b strict Wez read
//! adapter (plan §18 P3b gate: two-domain isolation, socket replacement,
//! no auto-start from list, unique tab counts, malformed/stopped
//! classification).
//!
//! Every test runs against scratch servers with SHORT socket paths under a
//! `mkdtemp`-style `/tmp/dmux-p3b-*` directory (macOS `sun_path` is ~104
//! bytes, ADR 001) and a dmux-managed scratch config whose `mux-startup`
//! handler spawns the reserved `dmux:system:<epoch>` sentinel (ADR 002
//! frozen shape; the epoch arrives via `DMUX_SERVER_EPOCH`). A Drop guard
//! kills the server and its panes on success and panic paths alike. The
//! user's live GUI, default sockets, and `~/.local/share/wezterm` are never
//! touched: every CLI call sets an explicit `WEZTERM_UNIX_SOCKET` plus
//! `--no-auto-start`, and `daemon_options` point into the scratch dir.
//! Tests soft-skip when the wezterm binaries are not installed.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use dmux::backend::wez::{SystemRunner, WezProvider, WezRunner, cli_invocation};
use dmux::backend::{InventoryOutcome, InventoryScope, NativeBinding, Provider, ProviderError};
use dmux::model::{Backend, ProviderHandle, ServerEpoch};
use uuid::Uuid;

const WEZTERM: &str = "wezterm";
const MUX_SERVER: &str = "wezterm-mux-server";
const CLI_DEADLINE: Duration = Duration::from_secs(10);

fn wez_available() -> bool {
    Command::new(WEZTERM)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
        && Command::new(MUX_SERVER)
            .arg("--help")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
}

macro_rules! require_wez {
    () => {
        if !wez_available() {
            eprintln!("skipping: wezterm/wezterm-mux-server not installed");
            return;
        }
    };
}

/// One scratch mux server: short socket path, dmux-managed config with the
/// ADR 002 sentinel `mux-startup` handler, guaranteed kill on drop.
struct ScratchMux {
    dir: tempfile::TempDir,
    cfg: PathBuf,
    sock: PathBuf,
    epoch: ServerEpoch,
    child: Option<Child>,
}

impl ScratchMux {
    fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("dmux-p3b-")
            .tempdir_in("/tmp")
            .expect("mkdtemp under /tmp");
        let sock = dir.path().join("sock");
        let cfg = dir.path().join("wez.lua");
        let config = format!(
            r#"local wezterm = require 'wezterm'
local config = {{}}
config.unix_domains = {{
  {{ name = 'dmux-p3b', socket_path = '{sock}', no_serve_automatically = true }},
}}
-- ADR 002 canary: an unmanaged default shell would surface under the
-- 'default' workspace and trip the missing-sentinel classification.
config.default_prog = {{ '/bin/sh', '-c', 'echo UNMANAGED-DEFAULT-SHELL; exec /bin/sleep 600' }}
-- Keep every artifact away from ~/.local/share/wezterm (live GUI owns it).
config.daemon_options = {{
  pid_file = '{dir}/daemon.pid',
  stdout = '{dir}/daemon-stdout.log',
  stderr = '{dir}/daemon-stderr.log',
}}
wezterm.on('mux-startup', function()
  local epoch = os.getenv 'DMUX_SERVER_EPOCH' or 'EPOCH-ENV-MISSING'
  wezterm.mux.spawn_window {{
    workspace = 'dmux:system:' .. epoch,
    args = {{ '/bin/sh', '-c', 'trap "" TERM; while :; do sleep 3600; done' }},
  }}
end)
return config
"#,
            sock = sock.display(),
            dir = dir.path().display(),
        );
        std::fs::write(&cfg, config).expect("write scratch config");
        ScratchMux {
            dir,
            cfg,
            sock,
            epoch: ServerEpoch(Uuid::new_v4()),
            child: None,
        }
    }

    fn start(&mut self) {
        let stdout = std::fs::File::create(self.dir.path().join("server-stdout.log")).unwrap();
        let stderr = std::fs::File::create(self.dir.path().join("server-stderr.log")).unwrap();
        let child = Command::new(MUX_SERVER)
            .arg("--config-file")
            .arg(&self.cfg)
            .env_remove("WEZTERM_PANE")
            .env_remove("WEZTERM_UNIX_SOCKET")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env("DMUX_SERVER_EPOCH", self.epoch.0.to_string())
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .expect("spawn wezterm-mux-server");
        self.child = Some(child);
        self.wait_ready();
    }

    /// Poll until the sentinel handshake passes (the socket may exist before
    /// `mux-startup` has spawned the sentinel; that window classifies as
    /// malformed and is retried here).
    fn wait_ready(&self) {
        let provider = self.provider();
        let scope = self.scope(Some(self.epoch));
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut last = String::new();
        while Instant::now() < deadline {
            match provider.inventory(&scope) {
                InventoryOutcome::Complete(_) => return,
                other => last = format!("{other:?}"),
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let log =
            std::fs::read_to_string(self.dir.path().join("server-stderr.log")).unwrap_or_default();
        panic!(
            "scratch mux on {} never became ready; last outcome: {last}; server stderr: {log}",
            self.sock.display()
        );
    }

    fn provider(&self) -> WezProvider<SystemRunner> {
        WezProvider::new(WEZTERM, self.cfg.to_str().expect("utf8 cfg path"))
    }

    fn scope(&self, expected: Option<ServerEpoch>) -> InventoryScope {
        self.scope_at(&self.sock, expected)
    }

    fn scope_at(&self, endpoint: &Path, expected: Option<ServerEpoch>) -> InventoryScope {
        InventoryScope {
            backend: Backend::Wez,
            endpoint: endpoint.to_str().expect("utf8 socket path").to_string(),
            expected_epoch: expected,
        }
    }

    /// Exact-socket CLI call through the frozen invocation template.
    fn cli(&self, args: &[&str]) -> dmux::backend::wez::RunOutput {
        let invocation = cli_invocation(
            WEZTERM,
            self.cfg.to_str().unwrap(),
            self.sock.to_str().unwrap(),
            args,
        )
        .expect("cli invocation");
        SystemRunner
            .run(&invocation, CLI_DEADLINE)
            .expect("run wezterm cli")
    }

    fn cli_ok(&self, args: &[&str]) -> String {
        let out = self.cli(args);
        assert!(
            out.ok(),
            "wezterm cli {args:?} on {}: {}",
            self.sock.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).expect("utf8 wezterm stdout")
    }

    /// `cli spawn` prints the new pane id on stdout.
    fn spawn_workspace(&self, workspace: &str) -> u64 {
        let out = self.cli_ok(&[
            "spawn",
            "--new-window",
            "--workspace",
            workspace,
            "--",
            "/bin/sh",
            "-c",
            "sleep 300",
        ]);
        out.trim().parse().expect("pane id from spawn")
    }

    fn spawn_tab_in_window(&self, window_id: u64) -> u64 {
        let out = self.cli_ok(&[
            "spawn",
            "--window-id",
            &window_id.to_string(),
            "--",
            "/bin/sh",
            "-c",
            "sleep 300",
        ]);
        out.trim().parse().expect("pane id from spawn")
    }

    fn split_pane(&self, pane_id: u64) -> u64 {
        let out = self.cli_ok(&[
            "split-pane",
            "--pane-id",
            &pane_id.to_string(),
            "--",
            "/bin/sh",
            "-c",
            "sleep 300",
        ]);
        out.trim().parse().expect("pane id from split-pane")
    }

    /// Raw list rows for test-side correlation (window ids are not part of
    /// the provider's normalized rows).
    fn raw_list(&self) -> serde_json::Value {
        serde_json::from_str(&self.cli_ok(&["list", "--format", "json"])).expect("list json")
    }

    fn window_id_of_workspace(&self, workspace: &str) -> u64 {
        self.raw_list()
            .as_array()
            .expect("array")
            .iter()
            .find(|row| row["workspace"] == workspace)
            .unwrap_or_else(|| panic!("no row for workspace {workspace}"))["window_id"]
            .as_u64()
            .expect("window_id")
    }

    /// SIGKILL the server, leaving its stale socket file behind.
    fn kill_hard(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.child = None;
    }
}

impl Drop for ScratchMux {
    fn drop(&mut self) {
        // Best-effort pane cleanup while the server is still alive, so no
        // scratch shell outlives the test even if pty teardown misbehaves.
        if let Some(child) = self.child.as_mut()
            && child.try_wait().ok().flatten().is_none()
        {
            let cfg = self.cfg.to_str().unwrap_or_default().to_string();
            let sock = self.sock.to_str().unwrap_or_default().to_string();
            if let Ok(invocation) =
                cli_invocation(WEZTERM, &cfg, &sock, &["list", "--format", "json"])
                && let Ok(out) = SystemRunner.run(&invocation, Duration::from_secs(5))
                && let Ok(rows) = serde_json::from_slice::<serde_json::Value>(&out.stdout)
            {
                for row in rows.as_array().map(Vec::as_slice).unwrap_or_default() {
                    if let Some(pane) = row["pane_id"].as_u64()
                        && let Ok(kill) =
                            dmux::backend::wez::kill_pane_invocation(WEZTERM, &cfg, &sock, pane)
                    {
                        let _ = SystemRunner.run(&kill, Duration::from_secs(5));
                    }
                }
            }
        }
        self.kill_hard();
    }
}

fn expect_complete(outcome: InventoryOutcome) -> dmux::backend::NativeInventory {
    match outcome {
        InventoryOutcome::Complete(inv) => inv,
        other => panic!("expected complete inventory, got {other:?}"),
    }
}

/// Live `wezterm-mux-server` processes NOT belonging to this test suite
/// (every suite-owned server carries `dmux-p3b-` in its argv; an
/// auto-started server would run bare `wezterm-mux-server --daemonize`
/// without our config, ADR 002). Filtering makes the census immune to
/// sibling tests starting/stopping their own scratch servers in parallel.
fn foreign_mux_server_census() -> Vec<String> {
    let out = Command::new("pgrep")
        .args(["-fl", "wezterm-mux-server"])
        .output()
        .expect("run pgrep");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|line| !line.contains("dmux-p3b-"))
        .map(str::to_string)
        .collect()
}

// ---------------------------------------------------------------------------
// Gate tests
// ---------------------------------------------------------------------------

/// Gate: two servers on two sockets — inventory against each endpoint
/// returns only that server's rows and epoch (ADR 001 two-domain proof).
#[test]
fn two_domain_inventories_are_isolated() {
    require_wez!();
    let mut a = ScratchMux::new();
    let mut b = ScratchMux::new();
    a.start();
    b.start();
    a.spawn_workspace("wsA");
    b.spawn_workspace("wsB");

    let inv_a = expect_complete(a.provider().inventory(&a.scope(Some(a.epoch))));
    assert_eq!(inv_a.server_epoch, Some(a.epoch));
    assert_eq!(
        inv_a
            .rows
            .iter()
            .map(|r| r.native_token.as_str())
            .collect::<Vec<_>>(),
        vec!["wsA"],
        "server A must list exactly its own user rows"
    );

    let inv_b = expect_complete(b.provider().inventory(&b.scope(Some(b.epoch))));
    assert_eq!(inv_b.server_epoch, Some(b.epoch));
    assert_eq!(
        inv_b
            .rows
            .iter()
            .map(|r| r.native_token.as_str())
            .collect::<Vec<_>>(),
        vec!["wsB"],
        "server B must list exactly its own user rows"
    );
}

/// Gate: an atomic symlink repoint of the published socket path from server
/// A to server B (whose sentinel epoch differs from the expected one) is a
/// typed rejection with the rows discarded — never silent wrong-server rows.
#[test]
fn socket_replacement_is_rejected_typed() {
    require_wez!();
    let mut a = ScratchMux::new();
    let mut b = ScratchMux::new();
    a.start();
    b.start();
    a.spawn_workspace("wsA");
    b.spawn_workspace("wsB");

    let published = a.dir.path().join("cur.sock");
    std::os::unix::fs::symlink(&a.sock, &published).expect("publish symlink");
    let scope = a.scope_at(&published, Some(a.epoch));
    let inv = expect_complete(a.provider().inventory(&scope));
    assert_eq!(inv.server_epoch, Some(a.epoch));

    // Atomic repoint (ADR 001 task 4): symlink to B, rename(2) over the
    // published path. The stock CLI would happily return B's rows exit 0.
    let staging = a.dir.path().join("cur.sock.new");
    std::os::unix::fs::symlink(&b.sock, &staging).expect("staging symlink");
    std::fs::rename(&staging, &published).expect("atomic repoint");

    match a.provider().inventory(&scope) {
        InventoryOutcome::Malformed { detail } => {
            assert!(
                detail.starts_with("backend_epoch_changed"),
                "epoch handshake must name the mismatch: {detail}"
            );
            assert!(detail.contains(&b.epoch.0.to_string()), "{detail}");
        }
        other => panic!("swapped socket must be rejected typed, got {other:?}"),
    }
}

/// Gate: an absent socket is owner-proven `ServerStopped`, and neither the
/// provider probe nor a raw `--no-auto-start` CLI call spawns any server.
#[test]
fn absent_socket_is_server_stopped_and_spawns_nothing() {
    require_wez!();
    let scratch = ScratchMux::new(); // never started; config + paths only
    let absent = scratch.dir.path().join("never-created.sock");

    let census_before = foreign_mux_server_census();
    let scope = scratch.scope_at(&absent, None);
    match scratch.provider().inventory(&scope) {
        InventoryOutcome::ServerStopped { detail } => {
            assert!(detail.contains("socket absent"), "{detail}");
        }
        other => panic!("absent socket must be ServerStopped, got {other:?}"),
    }

    // Belt and braces: even the raw CLI under the frozen template must fail
    // without spawning anything (ADR 001: --no-auto-start spawned zero
    // processes in every failure case).
    let invocation = cli_invocation(
        WEZTERM,
        scratch.cfg.to_str().unwrap(),
        absent.to_str().unwrap(),
        &["list", "--format", "json"],
    )
    .expect("invocation");
    let out = SystemRunner
        .run(&invocation, CLI_DEADLINE)
        .expect("run wezterm cli");
    assert!(!out.ok(), "list against an absent socket must fail");

    let census_after = foreign_mux_server_census();
    assert_eq!(
        census_before, census_after,
        "no wezterm-mux-server may be auto-started by list"
    );
}

/// Gate: a stale socket file left by a SIGKILLed server classifies as
/// owner-proven `ServerStopped` (ECONNREFUSED), not unreachable/malformed.
#[test]
fn stale_socket_after_kill_is_server_stopped() {
    require_wez!();
    let mut mux = ScratchMux::new();
    mux.start();
    mux.kill_hard();
    assert!(
        mux.sock.exists(),
        "wezterm never unlinks its socket on shutdown (ADR 002)"
    );

    match mux.provider().inventory(&mux.scope(Some(mux.epoch))) {
        InventoryOutcome::ServerStopped { detail } => {
            assert!(detail.contains("connection refused"), "{detail}");
        }
        other => panic!("stale socket must be ServerStopped, got {other:?}"),
    }
}

/// Gate: a live unix socket owned by something that is not a wezterm mux
/// server yields the malformed/protocol-mismatch class, never rows.
#[test]
fn live_non_wezterm_listener_is_malformed() {
    require_wez!();
    let scratch = ScratchMux::new(); // config only; no server
    let fake = scratch.dir.path().join("fake.sock");
    let listener = std::os::unix::net::UnixListener::bind(&fake).expect("bind fake listener");
    std::thread::spawn(move || {
        // Accept and immediately close: the CLI sees EOF instead of a
        // wezterm codec response.
        while let Ok((stream, _)) = listener.accept() {
            drop(stream);
        }
    });

    let scope = scratch.scope_at(&fake, None);
    match scratch.provider().inventory(&scope) {
        InventoryOutcome::Malformed { .. } | InventoryOutcome::ProtocolMismatch { .. } => {}
        other => panic!("non-wezterm listener must be malformed class, got {other:?}"),
    }
}

/// Gate: unique-tab grouping — a workspace with 2 tabs / 3 panes built via
/// exact-socket spawn/split-pane groups by unique tab_id with correct split
/// membership, and inspect answers typed for present/absent/stale bindings.
#[test]
fn grouping_two_tabs_three_panes_and_inspect() {
    require_wez!();
    let mut mux = ScratchMux::new();
    mux.start();
    let pane1 = mux.spawn_workspace("alpha");
    let window = mux.window_id_of_workspace("alpha");
    let pane2 = mux.spawn_tab_in_window(window);
    let pane3 = mux.split_pane(pane1);

    let provider = mux.provider();
    let scope = mux.scope(Some(mux.epoch));
    let inv = expect_complete(provider.inventory(&scope));
    assert_eq!(inv.server_epoch, Some(mux.epoch));
    assert_eq!(inv.rows.len(), 1, "sentinel excluded, one user workspace");
    let row = &inv.rows[0];
    assert_eq!(row.native_token, "alpha");
    assert_eq!(row.native_name, "alpha");
    assert!(!row.multi_window);
    assert_eq!(
        row.groups.len(),
        2,
        "unique tab_id count is the Group count"
    );
    let handles: std::collections::HashSet<_> =
        row.groups.iter().map(|g| g.handle.clone()).collect();
    assert_eq!(handles.len(), 2, "tab handles unique");

    let mut split_counts: Vec<usize> = row.groups.iter().map(|g| g.splits.len()).collect();
    split_counts.sort_unstable();
    assert_eq!(split_counts, vec![1, 2], "3 panes across 2 tabs");
    let panes: std::collections::HashSet<u64> = row
        .groups
        .iter()
        .flat_map(|g| &g.splits)
        .map(|s| match s.handle {
            ProviderHandle::Wz(id) => id,
            ref other => panic!("wez split handle expected, got {other}"),
        })
        .collect();
    assert_eq!(
        panes,
        [pane1, pane2, pane3].into_iter().collect(),
        "exact pane membership"
    );
    // The split tab holds pane1+pane3, the fresh tab holds pane2.
    let of = |needle: u64| {
        row.groups
            .iter()
            .find(|g| {
                g.splits
                    .iter()
                    .any(|s| s.handle == ProviderHandle::Wz(needle))
            })
            .map(|g| g.handle.clone())
            .expect("pane in some tab")
    };
    assert_eq!(of(pane1), of(pane3), "split stays in its pane's tab");
    assert_ne!(of(pane1), of(pane2), "spawned tab is a distinct Group");

    let binding = NativeBinding {
        native_token: "alpha".into(),
        server_epoch: mux.epoch,
        root_group: of(pane1),
        root_split: ProviderHandle::Wz(pane1),
    };
    let inspected = provider.inspect(&scope, &binding).expect("inspect alpha");
    assert_eq!(inspected.native_token, "alpha");
    assert_eq!(inspected.groups.len(), 2);

    let absent = NativeBinding {
        native_token: "missing".into(),
        ..binding.clone()
    };
    match provider.inspect(&scope, &absent) {
        Err(ProviderError::NotFound { native_ref }) => assert_eq!(native_ref, "missing"),
        other => panic!("absent workspace must be NotFound, got {other:?}"),
    }

    let stale = NativeBinding {
        server_epoch: ServerEpoch(Uuid::new_v4()),
        ..binding
    };
    match provider.inspect(&scope, &stale) {
        Err(ProviderError::EpochChanged { .. }) => {}
        other => panic!("stale binding must be EpochChanged, got {other:?}"),
    }
}

/// Gate: two `--new-window` spawns into the same workspace trip the
/// one-window diagnosis (plan §2.3) while the Group count stays the unique
/// tab count (§11.1).
#[test]
fn multi_window_workspace_is_flagged() {
    require_wez!();
    let mut mux = ScratchMux::new();
    mux.start();
    mux.spawn_workspace("mw");
    mux.spawn_workspace("mw");

    let inv = expect_complete(mux.provider().inventory(&mux.scope(Some(mux.epoch))));
    assert_eq!(inv.rows.len(), 1);
    let row = &inv.rows[0];
    assert_eq!(row.native_token, "mw");
    assert!(row.multi_window, "two distinct window_ids in one workspace");
    assert_eq!(row.groups.len(), 2, "still grouped by unique tab_id");
}
