//! P6 gate: explicit external adoption end to end (plan §10.3). Root-owned.
//! tmux: stamp + verify + bind, identity surviving external rename.
//! Wez: the fork CAS rename to the opaque key against a real fork-build
//! scratch server, zero-mutation loss handling, and the unstamped health
//! landing.

use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use dmux::backend::tmux::TmuxProvider;
use dmux::backend::wez::WezProvider;
use dmux::backend::{InventoryOutcome, InventoryScope, Provider};
use dmux::model::{Backend, Health, Lifecycle};
use dmux::operations::{OpError, OperationEnv, adopt_tmux, adopt_wez, tmux_bootstrap};
use dmux::registry::{Registry, RegistryConfig};
use uuid::Uuid;

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

fn require_fork() -> bool {
    if fork_binary("DMUX_TEST_FORK_WEZTERM").is_some()
        && fork_binary("DMUX_TEST_FORK_MUX_SERVER").is_some()
    {
        return true;
    }
    assert_ne!(
        std::env::var("DMUX_TEST_REQUIRE_FORK").as_deref(),
        Ok("1"),
        "the release gate requires exact DMUX_TEST_FORK_WEZTERM and \
         DMUX_TEST_FORK_MUX_SERVER binaries"
    );
    eprintln!("skipping fork adoption gate: exact test binaries were not supplied");
    false
}

fn env_of(data: &tempfile::TempDir, locks: &tempfile::TempDir) -> OperationEnv {
    OperationEnv {
        db_path: data.path().join("registry.sqlite3"),
        lock_dir: locks.path().to_path_buf(),
    }
}

fn registry_of(env: &OperationEnv) -> Registry {
    Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap()
}

// ---------------------------------------------------------------------------
// tmux adoption

struct TmuxScratch {
    ns: String,
}

impl TmuxScratch {
    fn tmux(&self, args: &[&str]) -> String {
        let out = Command::new("tmux")
            .args(["-L", &self.ns, "-f", "/dev/null"])
            .args(args)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

impl Drop for TmuxScratch {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.ns, "kill-server"])
            .output();
    }
}

#[test]
fn tmux_adoption_stamps_binds_and_survives_external_rename() {
    let data = tempfile::tempdir().unwrap();
    let locks = tempfile::tempdir().unwrap();
    let env = env_of(&data, &locks);
    let s = TmuxScratch {
        ns: format!("dmux-p6a-{}", std::process::id()),
    };
    s.tmux(&["new-session", "-d", "-s", "legacy"]);

    let epoch = match tmux_bootstrap(&env, &s.ns).unwrap() {
        dmux::operations::TmuxBootstrapOutcome::Bootstrapped { epoch } => epoch,
        other => panic!("fresh server must bootstrap: {other:?}"),
    };
    let scope = InventoryScope {
        backend: Backend::Tmux,
        endpoint: s.ns.clone(),
        expected_epoch: Some(epoch),
    };
    let provider = TmuxProvider::new(s.ns.clone());

    let session_id = s
        .tmux(&["list-sessions", "-F", "#{session_id}"])
        .trim()
        .to_string();
    let adopted = adopt_tmux(&env, &provider, &scope, &session_id, None, Uuid::new_v4()).unwrap();
    assert_eq!(adopted.name, "legacy");
    assert_eq!(adopted.native_token, session_id);

    // Registry: active + unstamped + bound to the immutable session id.
    let registry = registry_of(&env);
    let space = registry.space(adopted.space_uid).unwrap();
    assert_eq!(space.lifecycle, Lifecycle::Active);
    assert_eq!(space.health, Health::Unstamped);
    let binding = registry
        .current_binding(adopted.space_uid)
        .unwrap()
        .unwrap();
    assert_eq!(binding.native_token, session_id);
    drop(registry);

    // External rename preserves identity: markers stay readable by $id.
    s.tmux(&["rename-session", "-t", &session_id, "externally-moved"]);
    let stamped = provider.read_markers(&scope, &session_id).unwrap();
    assert_eq!(
        stamped.space_uid.as_deref(),
        Some(adopted.space_uid.0.to_string().as_str())
    );

    // Adoption is not repeatable into a name conflict.
    s.tmux(&["new-session", "-d", "-s", "another"]);
    let other_id = s
        .tmux(&["list-sessions", "-F", "#{session_id} #{session_name}"])
        .lines()
        .find(|l| l.ends_with("another"))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    let err = adopt_tmux(
        &env,
        &provider,
        &scope,
        &other_id,
        Some("legacy"),
        Uuid::new_v4(),
    )
    .unwrap_err();
    assert!(matches!(err, OpError::NameConflict(_)), "{err}");

    // Unknown session id: typed not-found, no identity consumed.
    let err = adopt_tmux(&env, &provider, &scope, "$999", None, Uuid::new_v4()).unwrap_err();
    assert!(matches!(err, OpError::NotFound(_)), "{err}");
}

/// The replay §10.3 has to survive: an operator still holds the NATIVE_REF
/// `ls` printed before the session was adopted (or renamed), and re-runs it.
/// The name guard cannot catch that one — the name moved — so identity must,
/// *before* the stamp, or the live session ends up advertising a Space that
/// was never finalized and the reservation holding the name is unreapable.
#[test]
fn a_replayed_native_ref_is_refused_before_the_session_is_restamped() {
    let data = tempfile::tempdir().unwrap();
    let locks = tempfile::tempdir().unwrap();
    let env = env_of(&data, &locks);
    let s = TmuxScratch {
        ns: format!("dmux-p6replay-{}", std::process::id()),
    };
    s.tmux(&["new-session", "-d", "-s", "legacy"]);

    let epoch = match tmux_bootstrap(&env, &s.ns).unwrap() {
        dmux::operations::TmuxBootstrapOutcome::Bootstrapped { epoch } => epoch,
        other => panic!("fresh server must bootstrap: {other:?}"),
    };
    let scope = InventoryScope {
        backend: Backend::Tmux,
        endpoint: s.ns.clone(),
        expected_epoch: Some(epoch),
    };
    let provider = TmuxProvider::new(s.ns.clone());
    let session_id = s
        .tmux(&["list-sessions", "-F", "#{session_id}"])
        .trim()
        .to_string();

    let adopted = adopt_tmux(&env, &provider, &scope, &session_id, None, Uuid::new_v4()).unwrap();
    s.tmux(&["rename-session", "-t", &session_id, "prod"]);
    let revision = registry_of(&env).authority_head().unwrap().revision;

    // Same ref, and the inherited name is now "prod" — a name nothing holds.
    let err = adopt_tmux(&env, &provider, &scope, &session_id, None, Uuid::new_v4()).unwrap_err();
    let OpError::NameConflict(detail) = &err else {
        panic!("{err}");
    };
    assert!(
        detail.starts_with(dmux::operations::ADOPT_IDENTITY_CONFLICT),
        "{detail}"
    );

    // Nothing durable moved: one Space, no `reserved` gravestone holding
    // "prod", no unfinished journal row, and the session still points at the
    // Space that really owns it.
    let registry = registry_of(&env);
    let spaces = registry.spaces().unwrap();
    assert_eq!(spaces.len(), 1, "{spaces:?}");
    assert_eq!(spaces[0].space_uid, adopted.space_uid);
    assert_eq!(spaces[0].lifecycle, Lifecycle::Active);
    assert!(
        registry
            .unfinished_operation(adopted.space_uid)
            .unwrap()
            .is_none()
    );
    assert_eq!(registry.authority_head().unwrap().revision, revision);
    let markers = provider.read_markers(&scope, &session_id).unwrap();
    assert_eq!(
        markers.space_uid.as_deref(),
        Some(adopted.space_uid.0.to_string().as_str())
    );

    // And the explicit-name variant, which needs no external rename at all.
    let err = adopt_tmux(
        &env,
        &provider,
        &scope,
        &session_id,
        Some("other"),
        Uuid::new_v4(),
    )
    .unwrap_err();
    assert!(matches!(err, OpError::NameConflict(_)), "{err}");
    assert_eq!(registry_of(&env).spaces().unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// Wez adoption (fork build)

struct WezScratch {
    server: Child,
    socket: String,
    config: String,
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
config.unix_domains = {{ {{ name = 'adopt{tag}', socket_path = os.getenv('DMUX_SOCKET'),
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
        let server = Command::new(fork_mux_server())
            .args(["--config-file", config_path.to_str().unwrap()])
            .env("DMUX_SOCKET", &socket)
            .env_remove("WEZTERM_UNIX_SOCKET")
            .spawn()
            .unwrap();
        WezScratch {
            server,
            socket,
            config: config_path.display().to_string(),
            dir,
        }
    }

    fn cli(&self, args: &[&str]) -> std::process::Output {
        Command::new(fork_wezterm())
            .args(["--config-file", &self.config, "cli", "--no-auto-start"])
            .args(args)
            .env("WEZTERM_UNIX_SOCKET", &self.socket)
            .env_remove("WEZTERM_PANE")
            .env_remove("TMUX")
            .output()
            .unwrap()
    }

    fn provider(&self) -> WezProvider<dmux::backend::wez::SystemRunner> {
        let fork = fork_wezterm().to_string_lossy().into_owned();
        WezProvider::new(fork.clone(), self.config.clone()).with_cas_binary(fork)
    }

    fn scope(&self) -> InventoryScope {
        InventoryScope {
            backend: Backend::Wez,
            endpoint: self.socket.clone(),
            expected_epoch: None,
        }
    }

    fn wait_ready(&self, provider: &WezProvider<dmux::backend::wez::SystemRunner>) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let InventoryOutcome::Complete(_) = provider.inventory(&self.scope()) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "fork mux server never became ready"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
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
fn wez_adoption_cas_renames_to_the_opaque_key() {
    if !require_fork() {
        return;
    }
    let data = tempfile::tempdir().unwrap();
    let locks = tempfile::tempdir().unwrap();
    let env = env_of(&data, &locks);
    let s = WezScratch::start("a");
    let provider = s.provider();
    s.wait_ready(&provider);

    // External workspace to adopt.
    let out = s.cli(&[
        "spawn",
        "--new-window",
        "--workspace",
        "legacy",
        "--",
        "/bin/sh",
        "-c",
        "sleep 300",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let adopted = adopt_wez(&env, &provider, &s.scope(), "legacy", None, Uuid::new_v4()).unwrap();
    assert_eq!(adopted.name, "legacy");
    assert!(
        adopted.native_token.starts_with("dmux:"),
        "{}",
        adopted.native_token
    );

    // The workspace now carries the opaque key; "legacy" is gone.
    let InventoryOutcome::Complete(inv) = provider.inventory(&s.scope()) else {
        panic!("scan must stay complete")
    };
    let names: Vec<&str> = inv.rows.iter().map(|r| r.native_token.as_str()).collect();
    assert!(names.contains(&adopted.native_token.as_str()));
    assert!(!names.contains(&"legacy"));

    // Registry: active + unstamped, bound to the opaque key.
    let registry = registry_of(&env);
    let space = registry.space(adopted.space_uid).unwrap();
    assert_eq!(
        (space.lifecycle, space.health),
        (Lifecycle::Active, Health::Unstamped)
    );
    assert_eq!(
        registry
            .current_binding(adopted.space_uid)
            .unwrap()
            .unwrap()
            .native_token,
        adopted.native_token
    );
    drop(registry);

    // The source token is spent: a repeat adoption is typed not-found and
    // consumes no identity.
    let err = adopt_wez(&env, &provider, &s.scope(), "legacy", None, Uuid::new_v4()).unwrap_err();
    assert!(matches!(err, OpError::NotFound(_)), "{err}");
}

#[test]
fn wez_adoption_refuses_multi_window_resources() {
    if !require_fork() {
        return;
    }
    let data = tempfile::tempdir().unwrap();
    let locks = tempfile::tempdir().unwrap();
    let env = env_of(&data, &locks);
    let s = WezScratch::start("b");
    let provider = s.provider();
    s.wait_ready(&provider);

    for _ in 0..2 {
        let out = s.cli(&[
            "spawn",
            "--new-window",
            "--workspace",
            "sprawl",
            "--",
            "/bin/sh",
            "-c",
            "sleep 300",
        ]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let err = adopt_wez(&env, &provider, &s.scope(), "sprawl", None, Uuid::new_v4()).unwrap_err();
    match &err {
        OpError::Provider(detail) => assert!(detail.contains("multi"), "{detail}"),
        other => panic!("expected multi-window refusal, got {other:?}"),
    }
    // Quarantined, never half-managed: no registry row was allocated.
    let registry = registry_of(&env);
    assert!(registry.spaces().unwrap().is_empty());
}
