//! P8a gate: local normalization end to end (plan §10.3) at the operations
//! layer, on a real scratch STOCK wezterm-mux-server — production normalize
//! must not depend on the fork (`move-pane-to-new-tab` is stock CLI).
//! Root-owned.

use std::process::{Child, Command};
use std::time::{Duration, Instant};

use dmux::backend::wez::WezProvider;
use dmux::backend::{InventoryOutcome, InventoryScope, Provider};
use dmux::model::{Backend, ServerEpoch};
use dmux::operations::{
    CreateRequest, OpError, OperationEnv, create_space, normalize_apply, normalize_preview,
    repair_normalize_batch, repair_scan_wez,
};
use uuid::Uuid;

const STOCK_WEZTERM: &str = "/opt/homebrew/bin/wezterm";
const STOCK_MUX_SERVER: &str = "/opt/homebrew/bin/wezterm-mux-server";

struct WezScratch {
    server: Child,
    socket: String,
    config: String,
    epoch: ServerEpoch,
    dir: tempfile::TempDir,
}

impl WezScratch {
    fn start(tag: &str) -> WezScratch {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let socket = dir.path().join("sock").display().to_string();
        let epoch = Uuid::new_v4();
        let config_path = dir.path().join("mux.lua");
        std::fs::write(
            &config_path,
            format!(
                r#"local wezterm = require 'wezterm'
local config = wezterm.config_builder and wezterm.config_builder() or {{}}
config.unix_domains = {{ {{ name = 'norm{tag}', socket_path = os.getenv('DMUX_SOCKET'),
                            no_serve_automatically = true }} }}
config.default_prog = {{ '/bin/sh', '-c', 'echo DMUX-CANARY; sleep 600' }}
wezterm.on('mux-startup', function()
  wezterm.mux.spawn_window {{
    workspace = 'dmux:system:{epoch}',
    args = {{ '/bin/sh', '-c', 'while :; do sleep 3600; done' }},
  }}
end)
return config
"#
            ),
        )
        .unwrap();
        let server = Command::new(STOCK_MUX_SERVER)
            .args(["--config-file", config_path.to_str().unwrap()])
            .env("DMUX_SOCKET", &socket)
            .env_remove("WEZTERM_UNIX_SOCKET")
            .spawn()
            .unwrap();
        WezScratch {
            server,
            socket,
            config: config_path.display().to_string(),
            epoch: ServerEpoch(epoch),
            dir,
        }
    }

    fn spawn_window(&self, workspace: &str) {
        let out = Command::new(STOCK_WEZTERM)
            .args(["--config-file", &self.config, "cli", "--no-auto-start"])
            .args([
                "spawn",
                "--new-window",
                "--workspace",
                workspace,
                "--",
                "/bin/sh",
                "-c",
                "sleep 300",
            ])
            .env("WEZTERM_UNIX_SOCKET", &self.socket)
            .env_remove("WEZTERM_PANE")
            .env_remove("TMUX")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn provider(&self) -> WezProvider<dmux::backend::wez::SystemRunner> {
        WezProvider::new(STOCK_WEZTERM, self.config.clone())
    }

    /// Pinned to the epoch the scratch server was started with: since
    /// WS-A.6 every wez verb beyond `inventory` refuses an unpinned scope.
    fn scope(&self) -> InventoryScope {
        InventoryScope::managed(Backend::Wez, self.socket.clone(), self.epoch)
    }

    fn wait_ready(&self, provider: &WezProvider<dmux::backend::wez::SystemRunner>) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let InventoryOutcome::Complete(_) = provider.inventory(&self.scope()) {
                return;
            }
            assert!(Instant::now() < deadline, "mux server never became ready");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn row(
        &self,
        provider: &WezProvider<dmux::backend::wez::SystemRunner>,
        token: &str,
    ) -> dmux::backend::NativeSpaceRow {
        let InventoryOutcome::Complete(inv) = provider.inventory(&self.scope()) else {
            panic!("scan must stay complete");
        };
        inv.rows
            .into_iter()
            .find(|r| r.native_token == token)
            .expect("workspace listed")
    }
}

impl Drop for WezScratch {
    fn drop(&mut self) {
        let _ = self.server.kill();
        let _ = self.server.wait();
        let _ = std::fs::remove_dir_all(self.dir.path());
    }
}

#[test]
fn normalize_merges_and_is_idempotent_by_request_uid() {
    if !std::path::Path::new(STOCK_MUX_SERVER).exists() {
        eprintln!("skipping: stock wezterm-mux-server not installed");
        return;
    }
    let data = tempfile::tempdir().unwrap();
    let locks = tempfile::tempdir().unwrap();
    let env = OperationEnv {
        db_path: data.path().join("registry.sqlite3"),
        lock_dir: locks.path().to_path_buf(),
    };
    let s = WezScratch::start("a");
    let provider = s.provider();
    s.wait_ready(&provider);

    s.spawn_window("sprawl");
    s.spawn_window("sprawl");
    s.spawn_window("sprawl");
    let before = s.row(&provider, "sprawl");
    assert!(before.multi_window, "setup: three native windows");
    let pane_count: usize = before.groups.iter().map(|g| g.splits.len()).sum();

    let plan = normalize_preview(&provider, &s.scope(), "sprawl").unwrap();
    assert_eq!(plan.moves.len(), 2, "two panes move into the target window");

    let request = Uuid::new_v4();
    normalize_apply(&env, &provider, &s.scope(), &plan, request).unwrap();
    let after = s.row(&provider, "sprawl");
    assert!(!after.multi_window, "exactly one window proven");
    assert_eq!(
        after.groups.iter().map(|g| g.splits.len()).sum::<usize>(),
        pane_count,
        "every pane survived the merge"
    );

    // Idempotent by request UID: the replay does not re-execute (the live
    // tree has changed, so a re-run would be drift — the ledger returns).
    normalize_apply(&env, &provider, &s.scope(), &plan, request).unwrap();

    // A sole-window resource previews to an empty plan; applying it is a
    // verified no-op.
    let plan = normalize_preview(&provider, &s.scope(), "sprawl").unwrap();
    assert!(plan.moves.is_empty());
    normalize_apply(&env, &provider, &s.scope(), &plan, Uuid::new_v4()).unwrap();
}

/// P8b gate: a MANAGED Space that grows a second native window is detected
/// by the repair scan (health recorded as `multi_window`), healed by the
/// previewed batch — here driven through the REAL `dmux repair normalize`
/// CLI — and the re-scan proves zero unresolved managed multi-window
/// Spaces.
#[test]
fn repair_batch_detects_and_heals_managed_multi_window() {
    if !std::path::Path::new(STOCK_MUX_SERVER).exists() {
        eprintln!("skipping: stock wezterm-mux-server not installed");
        return;
    }
    let data = tempfile::tempdir().unwrap();
    let locks = tempfile::tempdir().unwrap();
    let env = OperationEnv {
        db_path: data.path().join("registry.sqlite3"),
        lock_dir: locks.path().to_path_buf(),
    };
    let s = WezScratch::start("c");
    let provider = s.provider();
    s.wait_ready(&provider);

    // Managed Space through the full broker (env shim per ADR 009 §4a).
    let shim = data.path().join("helper-shim.sh");
    std::fs::write(
        &shim,
        format!(
            "#!/bin/sh\nexport DMUX_RUNTIME_DIR={}\nexec {} \"$@\"\n",
            locks.path().display(),
            env!("CARGO_BIN_EXE_pane-bootstrap")
        ),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // First contact on the bare endpoint yields the sentinel epoch; the
    // create itself runs under a registered, published, pinned instance —
    // mutations refuse an unpinned scope (ADR 012 WS-A.10/A.11).
    let epoch = {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let InventoryOutcome::Complete(inv) = provider.inventory(&s.scope())
                && let Some(epoch) = inv.server_epoch
            {
                break epoch;
            }
            assert!(
                Instant::now() < deadline,
                "sentinel never published an epoch"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    {
        let mut registry = dmux::registry::Registry::open(dmux::registry::RegistryConfig::new(
            &env.db_path,
            &env.lock_dir,
        ))
        .unwrap();
        let instance = registry
            .register_backend_instance(Backend::Wez, Some(&s.socket), None)
            .unwrap();
        registry
            .publish_backend_server(instance, epoch, None, None, None, None)
            .unwrap();
    }
    let scope = InventoryScope::managed(Backend::Wez, s.socket.clone(), epoch);
    let created = create_space(
        &env,
        &provider,
        &scope,
        Backend::Wez,
        &CreateRequest {
            request_uid: Uuid::new_v4(),
            name: "proj".into(),
            cwd: None,
            program: vec!["sh".into(), "-c".into(), "exec sleep 300".into()],
            helper_bin: shim.display().to_string(),
        },
    )
    .unwrap();

    // An external second window lands in the managed workspace.
    s.spawn_window(&created.native_token);
    assert!(s.row(&provider, &created.native_token).multi_window);

    // Detection records managed quarantine.
    let targets = repair_scan_wez(&env, &provider, &scope).unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].managed, Some(created.space_uid));
    let registry = dmux::registry::Registry::open(dmux::registry::RegistryConfig::new(
        &env.db_path,
        &env.lock_dir,
    ))
    .unwrap();
    assert_eq!(
        registry.space(created.space_uid).unwrap().health,
        dmux::model::Health::MultiWindow
    );
    drop(registry);

    // The seam is one pin, not a hatch (ADR 012 WS-A.6): `--socket` alone
    // is refused at the grammar, and a pin the sentinel does not serve is
    // refused typed — both before any move, the workspace stays two-window.
    let seam_cli = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_dmux"))
            .args([
                "repair",
                "normalize",
                "--yes",
                "--json",
                "--data-dir",
                data.path().to_str().unwrap(),
                "--lock-dir",
                locks.path().to_str().unwrap(),
                "--socket",
                &s.socket,
            ])
            .args(extra)
            .env("DMUX_WEZ_BIN", STOCK_WEZTERM)
            .env("DMUX_WEZ_CONFIG", &s.config)
            .output()
            .unwrap()
    };
    let lone = seam_cli(&[]);
    assert!(
        !lone.status.success(),
        "--socket without --epoch must refuse"
    );
    assert!(
        String::from_utf8_lossy(&lone.stderr).contains("--epoch"),
        "stderr: {}",
        String::from_utf8_lossy(&lone.stderr)
    );
    let wrong = seam_cli(&["--epoch", &Uuid::new_v4().to_string()]);
    assert!(
        !wrong.status.success(),
        "a pin the sentinel does not serve must refuse"
    );
    let wrong_text = format!(
        "{}{}",
        String::from_utf8_lossy(&wrong.stdout),
        String::from_utf8_lossy(&wrong.stderr)
    );
    assert!(wrong_text.contains("backend_epoch_changed"), "{wrong_text}");
    assert!(
        s.row(&provider, &created.native_token).multi_window,
        "zero mutation on either refusal"
    );

    // Heal through the REAL CLI with --yes and the test seams.
    let out = seam_cli(&["--epoch", &epoch.0.to_string()]);
    assert!(
        out.status.success(),
        "stdout: {} stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("\"results\""), "{text}");
    assert!(text.contains("normalized"), "{text}");

    // Healed: one window, health restored, and the re-scan is empty —
    // zero unresolved managed multi-window Spaces (P8b gate).
    assert!(!s.row(&provider, &created.native_token).multi_window);
    let registry = dmux::registry::Registry::open(dmux::registry::RegistryConfig::new(
        &env.db_path,
        &env.lock_dir,
    ))
    .unwrap();
    assert_eq!(
        registry.space(created.space_uid).unwrap().health,
        dmux::model::Health::Healthy
    );
    drop(registry);
    assert!(repair_scan_wez(&env, &provider, &scope).unwrap().is_empty());

    // The batch API itself also runs clean on an empty target list.
    assert!(repair_normalize_batch(&env, &provider, &scope, &[]).is_empty());
}

#[test]
fn normalize_refuses_drift_with_zero_mutation() {
    if !std::path::Path::new(STOCK_MUX_SERVER).exists() {
        eprintln!("skipping: stock wezterm-mux-server not installed");
        return;
    }
    let data = tempfile::tempdir().unwrap();
    let locks = tempfile::tempdir().unwrap();
    let env = OperationEnv {
        db_path: data.path().join("registry.sqlite3"),
        lock_dir: locks.path().to_path_buf(),
    };
    let s = WezScratch::start("b");
    let provider = s.provider();
    s.wait_ready(&provider);

    s.spawn_window("sprawl");
    s.spawn_window("sprawl");
    let plan = normalize_preview(&provider, &s.scope(), "sprawl").unwrap();

    // The live tree changes after planning: apply must refuse untouched.
    s.spawn_window("sprawl");
    let err = normalize_apply(&env, &provider, &s.scope(), &plan, Uuid::new_v4()).unwrap_err();
    match &err {
        OpError::Provider(detail) => {
            assert!(detail.contains("normalize_drift"), "{detail}")
        }
        other => panic!("expected drift refusal, got {other:?}"),
    }
    let row = s.row(&provider, "sprawl");
    assert!(row.multi_window, "zero mutation on refusal");
    assert_eq!(row.groups.iter().map(|g| g.splits.len()).sum::<usize>(), 3);

    // Re-plan against the current tree and converge.
    let plan = normalize_preview(&provider, &s.scope(), "sprawl").unwrap();
    normalize_apply(&env, &provider, &s.scope(), &plan, Uuid::new_v4()).unwrap();
    assert!(!s.row(&provider, "sprawl").multi_window);
}
