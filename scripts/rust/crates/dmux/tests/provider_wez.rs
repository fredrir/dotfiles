//! Real wezterm-mux-server integration tests for the strict Wez adapter:
//! the P3b read gate (two-domain isolation, socket replacement, no
//! auto-start from list, unique tab counts, malformed/stopped
//! classification) plus the P6 mutation slice (verified create/group/split
//! mutations, ADR 005 kill convergence with adversarial failure injection,
//! epoch-mismatch injection, acknowledgement-loss keyed-conflict, and the
//! ADR 006 fork CAS verb/probe against fork and stock scratch servers).
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

use dmux::backend::wez::{CasRenameOutcome, SystemRunner, WezProvider, WezRunner, cli_invocation};
use dmux::backend::{
    CreateSpec, InventoryOutcome, InventoryScope, NativeBinding, NormalizePlan, Provider,
    ProviderError, SplitDirection, SplitSpec,
};
use dmux::model::{Backend, ProviderHandle, ServerEpoch};
use uuid::Uuid;

const WEZTERM: &str = "wezterm";
const MUX_SERVER: &str = "wezterm-mux-server";
/// The rollout gate supplies the exact frozen fork CLI and mux-server.
/// Developer runs may omit them and soft-skip these fork-only cases; a
/// release run sets `DMUX_TEST_REQUIRE_FORK=1`, making absence a hard error.
const CLI_DEADLINE: Duration = Duration::from_secs(10);

fn fork_binary(variable: &str) -> Option<PathBuf> {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|path| path.is_file())
}

fn fork_wezterm() -> PathBuf {
    fork_binary("DMUX_TEST_FORK_WEZTERM").expect("validated exact fork wezterm test binary")
}

fn fork_mux_server() -> PathBuf {
    fork_binary("DMUX_TEST_FORK_MUX_SERVER").expect("validated exact fork mux-server test binary")
}

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

fn fork_available() -> bool {
    fork_binary("DMUX_TEST_FORK_WEZTERM").is_some()
        && fork_binary("DMUX_TEST_FORK_MUX_SERVER").is_some()
}

macro_rules! require_wez {
    () => {
        if !wez_available() {
            eprintln!("skipping: wezterm/wezterm-mux-server not installed");
            return;
        }
    };
}

macro_rules! require_fork {
    () => {
        require_wez!();
        if !fork_available() {
            assert_ne!(
                std::env::var("DMUX_TEST_REQUIRE_FORK").as_deref(),
                Ok("1"),
                "the release gate requires exact DMUX_TEST_FORK_WEZTERM and \
                 DMUX_TEST_FORK_MUX_SERVER binaries"
            );
            eprintln!("skipping: exact fork test binaries were not supplied");
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
    /// Which `wezterm-mux-server` binary `start` launches (stock by
    /// default; the pinned fork build for the CAS suite).
    mux_server: String,
}

impl ScratchMux {
    fn new() -> Self {
        Self::with_server(MUX_SERVER)
    }

    /// Scratch server running the pinned fork build (ADR 006 CAS suite).
    /// Reads still go through the stock CLI (a codec-45 CLI lists fine
    /// against a codec-46 server, ADR 006); only CAS calls need the fork
    /// CLI, wired per-provider via `with_cas_binary`.
    fn fork() -> Self {
        let server = fork_mux_server();
        Self::with_server(server.to_string_lossy().as_ref())
    }

    fn with_server(mux_server: &str) -> Self {
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
-- Pin the headless window geometry the split direction/percent tests
-- assert against (matches the stock defaults; pinned so a default change
-- upstream cannot silently move the geometry assertions).
config.initial_cols = 80
config.initial_rows = 24
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
            mux_server: mux_server.to_string(),
        }
    }

    fn start(&mut self) {
        let stdout = std::fs::File::create(self.dir.path().join("server-stdout.log")).unwrap();
        let stderr = std::fs::File::create(self.dir.path().join("server-stderr.log")).unwrap();
        let child = Command::new(&self.mux_server)
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
        match expected {
            Some(epoch) => InventoryScope::managed(
                Backend::Wez,
                endpoint.to_str().expect("utf8 socket path").to_string(),
                epoch,
            ),
            None => InventoryScope::unmanaged_endpoint(
                Backend::Wez,
                endpoint.to_str().expect("utf8 socket path").to_string(),
            ),
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

    /// `(window_id, tab_id, pane_id)` tuples of one workspace, list order.
    fn workspace_rows(&self, workspace: &str) -> Vec<(u64, u64, u64)> {
        self.raw_list()
            .as_array()
            .expect("array")
            .iter()
            .filter(|row| row["workspace"] == workspace)
            .map(|row| {
                (
                    row["window_id"].as_u64().expect("window_id"),
                    row["tab_id"].as_u64().expect("tab_id"),
                    row["pane_id"].as_u64().expect("pane_id"),
                )
            })
            .collect()
    }

    /// Provider-facing create spec: a plain long-lived shell stands in for
    /// the bootstrap helper argv (the broker handshake is orchestration
    /// above the provider; the provider only requires a non-empty argv).
    fn boot_spec(&self, token: &str) -> CreateSpec {
        CreateSpec {
            native_token: token.into(),
            cwd: None,
            bootstrap_argv: vec!["/bin/sh".into(), "-c".into(), "sleep 300".into()],
        }
    }

    /// `boot_spec` with an explicit owner-validated cwd (plan §11.3
    /// pass-through coverage).
    fn boot_spec_cwd(&self, token: &str, cwd: &Path) -> CreateSpec {
        CreateSpec {
            cwd: Some(cwd.to_str().expect("utf8 cwd").to_string()),
            ..self.boot_spec(token)
        }
    }

    /// `(left_col, top_row, size.rows, size.cols)` of one pane from the raw
    /// list JSON. Live-pinned observability fact: a headless mux server DOES
    /// expose pane geometry in `cli list --format json` (`left_col`,
    /// `top_row`, `size{rows,cols,...}`), with windows sized by
    /// `initial_cols`/`initial_rows` and a 1-cell divider per split.
    fn pane_geometry(&self, pane_id: u64) -> (u64, u64, u64, u64) {
        let list = self.raw_list();
        let row = list
            .as_array()
            .expect("array")
            .iter()
            .find(|r| r["pane_id"] == pane_id)
            .unwrap_or_else(|| panic!("pane {pane_id} not listed"));
        (
            row["left_col"].as_u64().expect("left_col"),
            row["top_row"].as_u64().expect("top_row"),
            row["size"]["rows"].as_u64().expect("size.rows"),
            row["size"]["cols"].as_u64().expect("size.cols"),
        )
    }

    /// Raw `cwd` field of one pane row (live-pinned shape: a `file://` URI
    /// with the macOS-canonicalized path and a trailing slash).
    fn pane_cwd_uri(&self, pane_id: u64) -> String {
        self.raw_list()
            .as_array()
            .expect("array")
            .iter()
            .find(|r| r["pane_id"] == pane_id)
            .unwrap_or_else(|| panic!("pane {pane_id} not listed"))["cwd"]
            .as_str()
            .unwrap_or_default()
            .to_string()
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

// ---------------------------------------------------------------------------
// P6 gate tests: verified mutations (stock server)
// ---------------------------------------------------------------------------

/// Gate: `create` spawns exactly once, verifies one window/one tab/one pane
/// in a same-epoch re-list, and returns the exact native binding. A repeated
/// identical create finds the existing key in the complete keyed pre-scan
/// and returns a typed conflict WITHOUT a second spawn (acknowledgement-loss
/// replay shape, plan §10.2).
#[test]
fn create_verifies_binding_and_repeat_is_keyed_conflict() {
    require_wez!();
    let mut mux = ScratchMux::new();
    mux.start();
    let provider = mux.provider();
    let scope = mux.scope(Some(mux.epoch));
    let spec = mux.boot_spec("dmux:h:s1");

    let binding = provider.create(&scope, &spec).expect("create");
    assert_eq!(binding.native_token, "dmux:h:s1");
    assert_eq!(binding.server_epoch, mux.epoch);
    let rows = mux.workspace_rows("dmux:h:s1");
    assert_eq!(rows.len(), 1, "exactly one pane row");
    let (_, tab, pane) = rows[0];
    assert_eq!(binding.root_group, ProviderHandle::Wz(tab));
    assert_eq!(binding.root_split, ProviderHandle::Wz(pane));

    // Acknowledgement-loss shape: identical create again. The keyed re-list
    // must find the existing workspace and refuse to spawn.
    match provider.create(&scope, &spec) {
        Err(ProviderError::PostconditionFailed { detail }) => {
            assert!(detail.starts_with("workspace_exists"), "{detail}");
            assert!(
                detail.contains(&format!("{pane}")),
                "existing pane id surfaces for journal rebind: {detail}"
            );
        }
        other => panic!("repeated create must be typed conflict, got {other:?}"),
    }
    let rows_after = mux.workspace_rows("dmux:h:s1");
    assert_eq!(rows_after, rows, "no second window/pane was spawned");
}

/// Gate: group_new spawns into the workspace's sole window and verifies the
/// pane landed in a NEW tab there; split_new anchors the group's first pane
/// and verifies the new pane landed in the SAME tab.
#[test]
fn group_new_and_split_new_verify_parentage() {
    require_wez!();
    let mut mux = ScratchMux::new();
    mux.start();
    let provider = mux.provider();
    let scope = mux.scope(Some(mux.epoch));
    let binding = provider
        .create(&scope, &mux.boot_spec("alpha"))
        .expect("create");
    let root_window = mux.workspace_rows("alpha")[0].0;

    let group = provider
        .group_new(&scope, &binding, &mux.boot_spec("alpha"))
        .expect("group_new");
    assert_ne!(group, binding.root_group, "a fresh Group (new tab)");
    let ProviderHandle::Wz(new_tab) = group else {
        panic!("wez tab handle expected, got {group}")
    };
    let rows = mux.workspace_rows("alpha");
    assert_eq!(rows.len(), 2);
    let new_row = rows
        .iter()
        .find(|(_, t, _)| *t == new_tab)
        .expect("new tab listed");
    assert_eq!(new_row.0, root_window, "same sole window (plan §2.3)");

    let split = provider
        .split_new(&scope, &group, &mux.boot_spec("alpha").into())
        .expect("split_new");
    let ProviderHandle::Wz(new_pane) = split else {
        panic!("wez pane handle expected, got {split}")
    };
    let rows = mux.workspace_rows("alpha");
    assert_eq!(rows.len(), 3);
    let split_row = rows
        .iter()
        .find(|(_, _, p)| *p == new_pane)
        .expect("split listed");
    assert_eq!(split_row.1, new_tab, "split stays in its parent Group");

    let groups = provider.group_list(&scope, &binding).expect("group_list");
    assert_eq!(groups.len(), 2);
    let splits = provider.split_list(&scope, &group).expect("split_list");
    assert_eq!(splits.len(), 2, "anchor pane + new split");
}

/// Gate: group_rename sets the tab title and proves it in the re-list.
#[test]
fn group_rename_is_verified_in_relist() {
    require_wez!();
    let mut mux = ScratchMux::new();
    mux.start();
    let provider = mux.provider();
    let scope = mux.scope(Some(mux.epoch));
    let binding = provider
        .create(&scope, &mux.boot_spec("alpha"))
        .expect("create");

    provider
        .group_rename(&scope, &binding.root_group, "editor")
        .expect("group_rename");
    let groups = provider.group_list(&scope, &binding).expect("group_list");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].title.as_deref(), Some("editor"));
}

/// Gate: whole-Space removal converges cleanly over tabs, splits, AND a
/// second native window (§2.3 sanctions whole-Space removal of a
/// multi-window resource as repair), leaving other workspaces untouched.
#[test]
fn remove_converges_clean_and_leaves_neighbors() {
    require_wez!();
    let mut mux = ScratchMux::new();
    mux.start();
    let provider = mux.provider();
    let scope = mux.scope(Some(mux.epoch));
    let binding = provider
        .create(&scope, &mux.boot_spec("victim"))
        .expect("create victim");
    let keeper = provider
        .create(&scope, &mux.boot_spec("keeper"))
        .expect("create keeper");
    provider
        .group_new(&scope, &binding, &mux.boot_spec("victim"))
        .expect("second tab");
    provider
        .split_new(&scope, &binding.root_group, &mux.boot_spec("victim").into())
        .expect("split");
    mux.spawn_workspace("victim"); // second native window: multi_window
    assert!(mux.workspace_rows("victim").len() >= 4);

    provider.remove(&scope, &binding).expect("remove converges");
    assert!(mux.workspace_rows("victim").is_empty(), "workspace gone");
    assert_eq!(mux.workspace_rows("keeper").len(), 1, "neighbor untouched");
    provider
        .inspect(&scope, &keeper)
        .expect("keeper still inspectable");
}

/// Gate (ADR 005 matrix through the provider): an adversary spawning fresh
/// windows into the workspace defeats convergence — the bounded loop hits
/// its round limit and returns a typed partial naming survivors, and never
/// tombstone-material success. After the adversary stops, the identical
/// remove converges.
#[test]
fn remove_bound_hit_under_adversary_then_converges() {
    require_wez!();
    let mut mux = ScratchMux::new();
    mux.start();
    let provider = mux.provider();
    let scope = mux.scope(Some(mux.epoch));
    let binding = provider
        .create(&scope, &mux.boot_spec("contested"))
        .expect("create");

    let stop = std::sync::atomic::AtomicBool::new(false);
    let outcome = std::thread::scope(|s| {
        for _ in 0..2 {
            s.spawn(|| {
                // Hot adversary: keep recreating panes/windows in the
                // workspace until told to stop (spawn recreates the
                // workspace even after it was fully killed).
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = mux.cli(&[
                        "spawn",
                        "--new-window",
                        "--workspace",
                        "contested",
                        "--",
                        "/bin/sh",
                        "-c",
                        "sleep 300",
                    ]);
                }
            });
        }
        let outcome = provider.remove(&scope, &binding);
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        outcome
    });
    match outcome {
        Err(ProviderError::PostconditionFailed { detail }) => {
            assert!(detail.starts_with("remove_unconverged"), "{detail}");
            assert!(detail.contains("pane(s) ["), "survivors listed: {detail}");
        }
        other => panic!("adversarial remove must hit the bound typed, got {other:?}"),
    }

    // Adversary stopped: the same remove now converges and verifies absence.
    provider
        .remove(&scope, &binding)
        .expect("remove after adversary stops");
    assert!(mux.workspace_rows("contested").is_empty());
}

/// Gate: the provider never deletes a Space through a child remove — the
/// workspace's last Split and last Group are refused typed (plan §7.2), and
/// legitimate child removals verify absence.
#[test]
fn child_removes_refuse_hidden_space_cascade() {
    require_wez!();
    let mut mux = ScratchMux::new();
    mux.start();
    let provider = mux.provider();
    let scope = mux.scope(Some(mux.epoch));
    let binding = provider
        .create(&scope, &mux.boot_spec("alpha"))
        .expect("create");

    match provider.split_remove(&scope, &binding.root_split) {
        Err(ProviderError::NativeFailure { detail }) => {
            assert!(detail.starts_with("refused_last_pane"), "{detail}");
        }
        other => panic!("last split must refuse, got {other:?}"),
    }
    match provider.group_remove(&scope, &binding.root_group) {
        Err(ProviderError::NativeFailure { detail }) => {
            assert!(detail.starts_with("refused_last_group"), "{detail}");
        }
        other => panic!("last group must refuse, got {other:?}"),
    }
    assert_eq!(mux.workspace_rows("alpha").len(), 1, "nothing was killed");

    // With a second Group present both child removals become legal.
    let group = provider
        .group_new(&scope, &binding, &mux.boot_spec("alpha"))
        .expect("group_new");
    let split = provider
        .split_new(&scope, &group, &mux.boot_spec("alpha").into())
        .expect("split_new");
    provider.split_remove(&scope, &split).expect("split_remove");
    provider.group_remove(&scope, &group).expect("group_remove");
    let rows = mux.workspace_rows("alpha");
    assert_eq!(rows.len(), 1, "back to the root pane only");
    assert_eq!(ProviderHandle::Wz(rows[0].2), binding.root_split);
}

/// Gate: epoch-mismatch injection — every mutation with a wrong expected
/// epoch is rejected typed by the sentinel handshake with ZERO native
/// mutation.
#[test]
fn epoch_mismatch_rejects_every_mutation_without_mutating() {
    require_wez!();
    let mut mux = ScratchMux::new();
    mux.start();
    let provider = mux.provider();
    let good_scope = mux.scope(Some(mux.epoch));
    let binding = provider
        .create(&good_scope, &mux.boot_spec("alpha"))
        .expect("create");
    provider
        .group_new(&good_scope, &binding, &mux.boot_spec("alpha"))
        .expect("second tab");
    let before = mux.raw_list();

    let wrong_epoch = ServerEpoch(Uuid::new_v4());
    let scope = mux.scope(Some(wrong_epoch));
    let stale = NativeBinding {
        server_epoch: wrong_epoch,
        ..binding.clone()
    };
    let results: Vec<(&str, Result<(), ProviderError>)> = vec![
        (
            "create",
            provider.create(&scope, &mux.boot_spec("fresh")).map(|_| ()),
        ),
        (
            "group_new",
            provider
                .group_new(&scope, &stale, &mux.boot_spec("alpha"))
                .map(|_| ()),
        ),
        (
            "split_new",
            provider
                .split_new(&scope, &binding.root_group, &mux.boot_spec("alpha").into())
                .map(|_| ()),
        ),
        (
            "group_rename",
            provider.group_rename(&scope, &binding.root_group, "x"),
        ),
        ("remove", provider.remove(&scope, &stale)),
        (
            "group_remove",
            provider.group_remove(&scope, &binding.root_group),
        ),
        (
            "split_remove",
            provider.split_remove(&scope, &binding.root_split),
        ),
        (
            "cas_rename_workspace",
            provider
                .cas_rename_workspace(&scope, 0, "alpha", "beta", false)
                .map(|_| ()),
        ),
        (
            "normalize_apply",
            provider.normalize_apply(
                &scope,
                &NormalizePlan {
                    native_token: "alpha".into(),
                    server_epoch: wrong_epoch,
                    target_window: 0,
                    moves: vec![],
                },
            ),
        ),
    ];
    for (verb, result) in results {
        match result {
            Err(ProviderError::EpochChanged { .. }) => {}
            other => panic!("{verb} with wrong epoch must be EpochChanged, got {other:?}"),
        }
    }
    assert_eq!(mux.raw_list(), before, "zero native mutation happened");
    assert!(mux.workspace_rows("fresh").is_empty(), "no spawn leaked");
}

// ---------------------------------------------------------------------------
// P8a gate tests: normalization (plan §10.3) + placement/cwd (stock server)
// ---------------------------------------------------------------------------

/// Gate (plan §10.3, P8a): a 3-window workspace (one window holding a
/// 2-pane split) normalizes to exactly ONE window — the lowest window id —
/// with every pane surviving the merge (panes are moved into tabs of the
/// target window, never killed) and a neighbor workspace untouched.
#[test]
fn normalize_merges_three_windows_preserving_every_pane() {
    require_wez!();
    let mut mux = ScratchMux::new();
    mux.start();
    let provider = mux.provider();
    let scope = mux.scope(Some(mux.epoch));

    let p1 = mux.spawn_workspace("mw");
    let p2 = mux.spawn_workspace("mw");
    let p3 = mux.spawn_workspace("mw");
    let p4 = mux.split_pane(p2);
    provider
        .create(&scope, &mux.boot_spec("keeper"))
        .expect("create keeper");
    let keeper_rows = mux.workspace_rows("keeper");

    let before = mux.workspace_rows("mw");
    let mut windows_before: Vec<u64> = before.iter().map(|(w, _, _)| *w).collect();
    windows_before.sort_unstable();
    windows_before.dedup();
    assert_eq!(windows_before.len(), 3, "three native windows");
    let target = windows_before[0];
    let mut panes_before: Vec<u64> = before.iter().map(|(_, _, p)| *p).collect();
    panes_before.sort_unstable();
    let mut spawned = vec![p1, p2, p3, p4];
    spawned.sort_unstable();
    assert_eq!(panes_before, spawned, "4 panes across 3 windows");

    let plan = provider.normalize_plan(&scope, "mw").expect("plan");
    assert_eq!(plan.native_token, "mw");
    assert_eq!(plan.server_epoch, mux.epoch);
    assert_eq!(plan.target_window, target, "lowest window id is the target");
    let mut expected_moves: Vec<(u64, u64)> = before
        .iter()
        .filter(|(w, _, _)| *w != target)
        .map(|(w, _, p)| (*w, *p))
        .collect();
    expected_moves.sort_unstable();
    assert_eq!(
        plan.moves
            .iter()
            .map(|m| (m.from_window, m.pane_id))
            .collect::<Vec<_>>(),
        expected_moves,
        "every extra-window pane, ascending (window_id, pane_id)"
    );

    provider.normalize_apply(&scope, &plan).expect("apply");

    let after = mux.workspace_rows("mw");
    let mut windows_after: Vec<u64> = after.iter().map(|(w, _, _)| *w).collect();
    windows_after.sort_unstable();
    windows_after.dedup();
    assert_eq!(
        windows_after,
        vec![target],
        "exactly one window: the target"
    );
    let mut panes_after: Vec<u64> = after.iter().map(|(_, _, p)| *p).collect();
    panes_after.sort_unstable();
    assert_eq!(
        panes_after, panes_before,
        "pane-count preservation: every pane survived, nothing was killed"
    );

    let inv = expect_complete(provider.inventory(&scope));
    let row = inv
        .rows
        .iter()
        .find(|r| r.native_token == "mw")
        .expect("mw row");
    assert!(
        !row.multi_window,
        "one-window invariant restored (plan §2.3)"
    );

    assert_eq!(
        mux.workspace_rows("keeper"),
        keeper_rows,
        "neighbor workspace untouched"
    );
}

/// Gate (plan §10.3): a sole-window workspace plans zero moves and apply is
/// a verified no-op success; an absent opaque key is typed NotFound.
#[test]
fn normalize_sole_window_plan_is_empty_and_apply_is_noop() {
    require_wez!();
    let mut mux = ScratchMux::new();
    mux.start();
    let provider = mux.provider();
    let scope = mux.scope(Some(mux.epoch));
    let binding = provider
        .create(&scope, &mux.boot_spec("solo"))
        .expect("create");
    provider
        .group_new(&scope, &binding, &mux.boot_spec("solo"))
        .expect("second tab");
    let before = mux.workspace_rows("solo");

    let plan = provider.normalize_plan(&scope, "solo").expect("plan");
    assert_eq!(plan.target_window, before[0].0);
    assert!(plan.moves.is_empty(), "sole window: nothing to do");
    provider
        .normalize_apply(&scope, &plan)
        .expect("empty plan applies as a no-op success");
    assert_eq!(mux.workspace_rows("solo"), before, "zero mutation");

    match provider.normalize_plan(&scope, "missing") {
        Err(ProviderError::NotFound { native_ref }) => assert_eq!(native_ref, "missing"),
        other => panic!("zero rows must be NotFound, got {other:?}"),
    }
}

/// Gate (plan §10.3): drift between confirmation and apply — an extra
/// window spawned into the workspace — refuses `normalize_drift:` with
/// ZERO mutation, and a stale plan epoch is EpochChanged (IDs discarded).
/// Re-planning against the drifted tree then applying converges.
#[test]
fn normalize_apply_refuses_drift_and_stale_epoch_then_replan_converges() {
    require_wez!();
    let mut mux = ScratchMux::new();
    mux.start();
    let provider = mux.provider();
    let scope = mux.scope(Some(mux.epoch));
    mux.spawn_workspace("drift");
    mux.spawn_workspace("drift");

    let plan = provider.normalize_plan(&scope, "drift").expect("plan");
    assert_eq!(plan.moves.len(), 1, "one extra-window pane planned");

    // Stale plan epoch: typed EpochChanged before any move.
    let stale = NormalizePlan {
        server_epoch: ServerEpoch(Uuid::new_v4()),
        ..plan.clone()
    };
    let snapshot = mux.workspace_rows("drift");
    match provider.normalize_apply(&scope, &stale) {
        Err(ProviderError::EpochChanged { .. }) => {}
        other => panic!("stale plan epoch must be EpochChanged, got {other:?}"),
    }
    assert_eq!(mux.workspace_rows("drift"), snapshot, "zero mutation");

    // Drift: a third window appears after the plan was confirmed.
    mux.spawn_workspace("drift");
    let snapshot = mux.workspace_rows("drift");
    match provider.normalize_apply(&scope, &plan) {
        Err(ProviderError::PostconditionFailed { detail }) => {
            assert!(detail.starts_with("normalize_drift:"), "{detail}");
        }
        other => panic!("drifted tree must refuse typed, got {other:?}"),
    }
    assert_eq!(
        mux.workspace_rows("drift"),
        snapshot,
        "zero mutation on the drift refusal"
    );

    // Re-plan against the live tree; the fresh plan applies and converges.
    let plan2 = provider.normalize_plan(&scope, "drift").expect("re-plan");
    assert_eq!(plan2.moves.len(), 2, "both extra windows planned now");
    provider
        .normalize_apply(&scope, &plan2)
        .expect("apply after re-plan");
    let after = mux.workspace_rows("drift");
    let mut windows: Vec<u64> = after.iter().map(|(w, _, _)| *w).collect();
    windows.sort_unstable();
    windows.dedup();
    assert_eq!(windows, vec![plan2.target_window], "merged to the target");
    assert_eq!(after.len(), snapshot.len(), "every pane survived");
}

/// Gate (plan §7.2 placement): each split direction lands the new pane on
/// the geometrically correct side of its anchor, and `--percent` sizes the
/// new pane's share of the split axis. Live-pinned observability: a
/// headless mux server exposes `left_col`/`top_row`/`size{rows,cols}` in
/// `cli list --format json`; windows are `initial_cols`x`initial_rows`
/// (pinned 80x24 in the scratch config) with a 1-cell divider per split.
#[test]
fn split_new_directions_and_percent_land_geometrically() {
    require_wez!();
    let mut mux = ScratchMux::new();
    mux.start();
    let provider = mux.provider();
    let scope = mux.scope(Some(mux.epoch));

    for (direction, name) in [
        (SplitDirection::Down, "down"),
        (SplitDirection::Up, "up"),
        (SplitDirection::Right, "right"),
        (SplitDirection::Left, "left"),
    ] {
        let token = format!("geo-{name}");
        let binding = provider
            .create(&scope, &mux.boot_spec(&token))
            .expect("create");
        let ProviderHandle::Wz(anchor) = binding.root_split else {
            panic!("wez pane handle expected");
        };
        let split = provider
            .split_new(
                &scope,
                &binding.root_group,
                &SplitSpec {
                    spec: mux.boot_spec(&token),
                    direction,
                    percent: None,
                },
            )
            .expect("split_new");
        let ProviderHandle::Wz(new_pane) = split else {
            panic!("wez pane handle expected");
        };
        let (a_left, a_top, a_rows, a_cols) = mux.pane_geometry(anchor);
        let (n_left, n_top, n_rows, n_cols) = mux.pane_geometry(new_pane);
        match direction {
            SplitDirection::Down => {
                assert_eq!(n_left, a_left, "{name}: same column band");
                assert!(n_top > a_top, "{name}: new pane below ({n_top} vs {a_top})");
                assert_eq!(n_cols, a_cols, "{name}: full-width halves");
            }
            SplitDirection::Up => {
                assert_eq!(n_left, a_left, "{name}: same column band");
                assert!(n_top < a_top, "{name}: new pane above ({n_top} vs {a_top})");
                assert_eq!(n_cols, a_cols, "{name}: full-width halves");
            }
            SplitDirection::Right => {
                assert_eq!(n_top, a_top, "{name}: same row band");
                assert!(
                    n_left > a_left,
                    "{name}: new pane right of anchor ({n_left} vs {a_left})"
                );
                assert_eq!(n_rows, a_rows, "{name}: full-height halves");
            }
            SplitDirection::Left => {
                assert_eq!(n_top, a_top, "{name}: same row band");
                assert!(
                    n_left < a_left,
                    "{name}: new pane left of anchor ({n_left} vs {a_left})"
                );
                assert_eq!(n_rows, a_rows, "{name}: full-height halves");
            }
        }
    }

    // --percent 25 --bottom on a fresh 24-row window: the new pane gets
    // exactly 6 rows (25% of 24, live-pinned) and the anchor keeps 17
    // (24 minus the new pane and the 1-row divider).
    let binding = provider
        .create(&scope, &mux.boot_spec("geo-pct"))
        .expect("create");
    let ProviderHandle::Wz(anchor) = binding.root_split else {
        panic!("wez pane handle expected");
    };
    let split = provider
        .split_new(
            &scope,
            &binding.root_group,
            &SplitSpec {
                spec: mux.boot_spec("geo-pct"),
                direction: SplitDirection::Down,
                percent: Some(25),
            },
        )
        .expect("split_new percent");
    let ProviderHandle::Wz(new_pane) = split else {
        panic!("wez pane handle expected");
    };
    let (_, _, a_rows, _) = mux.pane_geometry(anchor);
    let (_, _, n_rows, _) = mux.pane_geometry(new_pane);
    assert_eq!(n_rows, 6, "25% of the pinned 24-row window");
    assert_eq!(a_rows, 17, "anchor keeps the rest minus the 1-row divider");
}

/// Gate (plan §11.3 pass-through): create/group_new/split_new with an
/// explicit cwd start the pane there. Live-pinned observability: the list
/// JSON exposes cwd as a `file://` URI with the canonicalized process path
/// (macOS `/tmp` -> `/private/tmp`) and a TRAILING slash; the provider's
/// normalized rows carry the parsed plain path.
#[test]
fn explicit_cwd_reaches_every_created_pane() {
    require_wez!();
    let mut mux = ScratchMux::new();
    mux.start();
    let provider = mux.provider();
    let scope = mux.scope(Some(mux.epoch));

    let dir_a = mux.dir.path().join("cwd-a");
    let dir_b = mux.dir.path().join("cwd-b");
    let dir_c = mux.dir.path().join("cwd-c");
    for dir in [&dir_a, &dir_b, &dir_c] {
        std::fs::create_dir_all(dir).expect("mk cwd dir");
    }
    let canon = |dir: &Path| {
        std::fs::canonicalize(dir)
            .expect("canonicalize")
            .to_str()
            .expect("utf8")
            .to_string()
    };

    let binding = provider
        .create(&scope, &mux.boot_spec_cwd("cwdws", &dir_a))
        .expect("create");
    let group = provider
        .group_new(&scope, &binding, &mux.boot_spec_cwd("cwdws", &dir_b))
        .expect("group_new");
    let split = provider
        .split_new(
            &scope,
            &group,
            &SplitSpec {
                spec: mux.boot_spec_cwd("cwdws", &dir_c),
                direction: SplitDirection::Down,
                percent: None,
            },
        )
        .expect("split_new");

    // cwd surfaces from process inspection; poll briefly, then assert.
    let read_cwds = || {
        let row = provider.inspect(&scope, &binding).expect("inspect");
        let trimmed = |cwd: Option<String>| cwd.map(|c| c.trim_end_matches('/').to_string());
        let cwd_of = |handle: &ProviderHandle| {
            row.groups
                .iter()
                .flat_map(|g| &g.splits)
                .find(|s| s.handle == *handle)
                .and_then(|s| s.cwd.clone())
        };
        // group_new returned the TAB handle; its anchor pane is the split
        // row of that group that is not the new split pane.
        let anchor_cwd = row
            .groups
            .iter()
            .find(|g| g.handle == group)
            .and_then(|g| g.splits.iter().find(|s| s.handle != split))
            .and_then(|s| s.cwd.clone());
        (
            trimmed(cwd_of(&binding.root_split)),
            trimmed(anchor_cwd),
            trimmed(cwd_of(&split)),
        )
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let (got_a, got_b, got_c) = loop {
        let got = read_cwds();
        let done = got.0.as_deref() == Some(canon(&dir_a).as_str())
            && got.1.as_deref() == Some(canon(&dir_b).as_str())
            && got.2.as_deref() == Some(canon(&dir_c).as_str());
        if done || Instant::now() >= deadline {
            break got;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    assert_eq!(got_a.as_deref(), Some(canon(&dir_a).as_str()), "create cwd");
    assert_eq!(
        got_b.as_deref(),
        Some(canon(&dir_b).as_str()),
        "group_new cwd"
    );
    assert_eq!(
        got_c.as_deref(),
        Some(canon(&dir_c).as_str()),
        "split_new cwd"
    );

    let ProviderHandle::Wz(root_pane) = binding.root_split.clone() else {
        panic!("wez pane handle expected");
    };
    let uri = mux.pane_cwd_uri(root_pane);
    assert!(uri.starts_with("file://"), "cwd is a file:// URI: {uri}");
    assert!(uri.ends_with('/'), "live-pinned trailing slash: {uri}");
}

// ---------------------------------------------------------------------------
// P6 gate tests: ADR 006 CAS verb and capability probe (fork + stock)
// ---------------------------------------------------------------------------

/// Gate: against a scratch server running the pinned fork build, the
/// positive capability probe answers true and the CAS verb walks the full
/// ADR 006 demo matrix (Renamed / WorkspaceMismatch / NoSuchWindow /
/// NotSoleWindow), every non-Renamed outcome mutation-free.
#[test]
fn fork_server_probe_true_and_cas_matrix() {
    require_fork!();
    let mut mux = ScratchMux::fork();
    mux.start();
    // Reads through the stock CLI (codec-45 list against the codec-46
    // server works, ADR 006); CAS through the fork CLI via `with_cas_binary`.
    let provider = mux
        .provider()
        .with_cas_binary(fork_wezterm().to_string_lossy().into_owned());
    let scope = mux.scope(Some(mux.epoch));

    assert!(
        provider.probe_cas_rename(&scope).expect("probe"),
        "fork server must probe capable"
    );

    mux.spawn_workspace("old");
    // The adopt-flow lookup: sole_window_id feeds the CAS call, live.
    let window = provider
        .sole_window_id(&scope, "old")
        .expect("sole_window_id");
    assert_eq!(window, mux.window_id_of_workspace("old"), "raw cross-check");
    match provider.sole_window_id(&scope, "no-such-workspace") {
        Err(ProviderError::NotFound { native_ref }) => {
            assert_eq!(native_ref, "no-such-workspace");
        }
        other => panic!("absent workspace must be NotFound, got {other:?}"),
    }

    // Renamed (with the sole-window fence held).
    let outcome = provider
        .cas_rename_workspace(&scope, window, "old", "dmux:h:adopted", true)
        .expect("cas rename");
    assert_eq!(outcome, CasRenameOutcome::Renamed);
    assert!(mux.workspace_rows("old").is_empty());
    assert_eq!(mux.window_id_of_workspace("dmux:h:adopted"), window);

    // Stale expectation: typed mismatch naming the actual key, no mutation.
    let outcome = provider
        .cas_rename_workspace(&scope, window, "old", "other", false)
        .expect("cas mismatch is a typed outcome");
    assert_eq!(
        outcome,
        CasRenameOutcome::WorkspaceMismatch {
            actual: "dmux:h:adopted".into()
        }
    );
    assert_eq!(mux.window_id_of_workspace("dmux:h:adopted"), window);

    // Nonexistent window.
    let outcome = provider
        .cas_rename_workspace(&scope, u64::MAX, "dmux:h:adopted", "x", false)
        .expect("cas no-such-window is a typed outcome");
    assert_eq!(outcome, CasRenameOutcome::NoSuchWindow);

    // Interloper window in the same workspace defeats --if-sole-window.
    mux.spawn_workspace("dmux:h:adopted");
    let windows = |mux: &ScratchMux| -> std::collections::HashSet<u64> {
        mux.workspace_rows("dmux:h:adopted")
            .iter()
            .map(|(w, _, _)| *w)
            .collect()
    };
    let before = windows(&mux);
    assert_eq!(before.len(), 2, "target + interloper windows");
    assert!(before.contains(&window));
    match provider.sole_window_id(&scope, "dmux:h:adopted") {
        Err(ProviderError::MultiWindow { window_count, .. }) => assert_eq!(window_count, 2),
        other => panic!("interloper must make sole_window_id MultiWindow, got {other:?}"),
    }
    let outcome = provider
        .cas_rename_workspace(&scope, window, "dmux:h:adopted", "x", true)
        .expect("cas not-sole-window is a typed outcome");
    assert_eq!(outcome, CasRenameOutcome::NotSoleWindow);
    assert_eq!(windows(&mux), before, "no mutation on NotSoleWindow");
}

/// Gate: when the server on `PATH` is a stock codec-45 build, the fork CLI
/// classifies its stable invalid-PDU rejection as capability-missing — probe
/// false, zero mutation, and the CAS method itself fails typed.  Once the
/// maintained fork is installed system-wide, this negative compatibility
/// leg is no longer present locally and is soft-skipped like a missing stock
/// binary; the pinned fork-positive matrix above remains mandatory.
#[test]
fn stock_server_probe_false_via_invalid_pdu() {
    require_fork!();
    let mut mux = ScratchMux::new(); // stock server
    mux.start();
    let provider = mux
        .provider()
        .with_cas_binary(fork_wezterm().to_string_lossy().into_owned());
    let scope = mux.scope(Some(mux.epoch));

    if provider.probe_cas_rename(&scope).expect("probe") {
        eprintln!(
            "skipping stock-negative CAS leg: wezterm-mux-server on PATH is already the capable maintained fork"
        );
        return;
    }

    mux.spawn_workspace("old");
    let window = mux.window_id_of_workspace("old");
    match provider.cas_rename_workspace(&scope, window, "old", "new", false) {
        Err(ProviderError::NativeFailure { detail }) => {
            assert!(detail.starts_with("cas_capability_missing"), "{detail}");
        }
        other => panic!("CAS on stock server must fail typed, got {other:?}"),
    }
    assert_eq!(
        mux.workspace_rows("old").len(),
        1,
        "zero mutation on capability-missing"
    );
}
