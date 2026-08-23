//! `dmux _context` on a tmux pane, review finding #8 inverted (ADR 012
//! WS-A.7; plan §13.1; acceptance cases 28 and 31).
//!
//! Before this fix the tmux arm of `_context` built an unmanaged scope from
//! the ambient `$TMUX` socket and never compared it with the Space's
//! recorded instance endpoint or published epoch, so the review minted a
//! marker four ways — from a stranger's server, from a stale incarnation,
//! from an instance with no published epoch, and after a rebind. The fix
//! resolves the Space's instance through `backend::scope::resolve_managed_instance`,
//! refuses an ambient namespace that is not the recorded endpoint, and pins
//! the scan to the published epoch. Each of the four now refuses with no
//! output; the positive control still mints the marker.

use std::process::{Command, Output, Stdio};

use dmux::model::{Backend, Health, ServerEpoch};
use dmux::operations::{OperationEnv, TmuxBootstrapOutcome, tmux_bootstrap};
use dmux::registry::{NativeBindingSpec, NativeKind, Registry, RegistryConfig, SpaceReservation};
use uuid::Uuid;

struct ScratchTmux {
    ns: String,
}

impl ScratchTmux {
    fn start(tag: &str) -> Option<ScratchTmux> {
        let server = ScratchTmux {
            ns: format!("dmux-a7-{tag}-{}", std::process::id()),
        };
        let started = Command::new("tmux")
            .args(["-L", &server.ns, "-f", "/dev/null"])
            .args(["new-session", "-d", "-s", "proj"])
            .status();
        match started {
            Ok(status) if status.success() => Some(server),
            _ => None,
        }
    }

    fn tmux(&self, args: &[&str]) -> String {
        let out = Command::new("tmux")
            .args(["-L", &self.ns])
            .args(args)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn session_id(&self, name: &str) -> String {
        self.tmux(&["display-message", "-p", "-t", name, "#{session_id}"])
    }

    fn pane_id(&self) -> String {
        self.tmux(&["display-message", "-p", "-t", "proj", "#{pane_id}"])
    }

    fn socket_path(&self) -> String {
        self.tmux(&["display-message", "-p", "#{socket_path}"])
    }

    /// `kill-server`, then wait for the server to be gone: tmux's client
    /// returns once the server acknowledges, while the server unlinks its
    /// socket as it exits. A replacement started on the same `-L` name in
    /// that window loses its fresh socket to the dying server's cleanup and
    /// reports "server exited unexpectedly" — deterministic on Linux tmux
    /// 3.7 (Archie, ADR 012 §10), masked by timing on macOS.
    fn kill(&self) {
        let socket = Command::new("tmux")
            .args(["-L", &self.ns, "display-message", "-p", "#{socket_path}"])
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string());
        let _ = Command::new("tmux")
            .args(["-L", &self.ns, "kill-server"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if let Some(socket) = socket.filter(|s| !s.is_empty()) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while std::path::Path::new(&socket).exists() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
}

impl Drop for ScratchTmux {
    fn drop(&mut self) {
        self.kill();
    }
}

struct Home {
    home: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        Home {
            home: tempfile::tempdir().unwrap(),
        }
    }

    fn data_dir(&self) -> std::path::PathBuf {
        self.home.path().join("data")
    }

    fn lock_dir(&self) -> std::path::PathBuf {
        let locks = self.home.path().join("locks");
        std::fs::create_dir_all(&locks).unwrap();
        locks
    }

    fn env(&self) -> OperationEnv {
        std::fs::create_dir_all(self.data_dir()).unwrap();
        OperationEnv {
            db_path: self.data_dir().join("registry.sqlite3"),
            lock_dir: self.lock_dir(),
        }
    }

    fn registry(&self) -> Registry {
        let env = self.env();
        Registry::open(RegistryConfig::new(env.db_path, env.lock_dir)).unwrap()
    }

    /// `dmux _context` exactly as the prompt hook runs it: marker env plus
    /// pane env in, one document or nothing out.
    fn context(&self, space_uid: Uuid, socket_path: &str, pane: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_dmux"))
            .args([
                "_context",
                "--data-dir",
                self.data_dir().to_str().unwrap(),
                "--lock-dir",
                self.lock_dir().to_str().unwrap(),
            ])
            .env("DMUX_SPACE_UID", space_uid.to_string())
            .env("TMUX", format!("{socket_path},1,0"))
            .env("TMUX_PANE", pane)
            .env("DMUX_RUNTIME_DIR", self.lock_dir())
            .env_remove("WEZTERM_PANE")
            .env_remove("DMUX_WEZ_FIRST")
            .stdin(Stdio::null())
            .output()
            .expect("dmux runs")
    }
}

/// An Active tmux Space bound to `session_id`, on an instance registered at
/// `namespace` with NO published epoch.
fn seed_space(home: &Home, namespace: &str, session_id: &str) -> SpaceReservation {
    let mut registry = home.registry();
    let instance = registry
        .register_backend_instance(Backend::Tmux, Some(namespace), None)
        .unwrap();
    let reservation = registry
        .reserve_space("proj", instance, Uuid::new_v4())
        .unwrap();
    registry
        .finalize_create(
            reservation.space_uid,
            reservation.operation_uid,
            &NativeBindingSpec {
                native_token: session_id.to_string(),
                native_kind: NativeKind::TmuxSessionId,
                server_epoch: None,
            },
        )
        .unwrap();
    registry
        .set_space_health(reservation.space_uid, Health::Healthy)
        .unwrap();
    reservation
}

fn publish(home: &Home, namespace: &str) -> ServerEpoch {
    match tmux_bootstrap(&home.env(), namespace).unwrap() {
        TmuxBootstrapOutcome::Bootstrapped { .. }
        | TmuxBootstrapOutcome::AlreadyBound { .. }
        | TmuxBootstrapOutcome::Rebound { .. } => {}
    }
    let registry = home.registry();
    let instance = registry
        .backend_instance_for_backend(Backend::Tmux)
        .unwrap()
        .expect("registered");
    registry
        .backend_server(instance)
        .unwrap()
        .server_epoch
        .expect("tmux_bootstrap published an epoch")
}

fn refused(out: &Output, needle: &str) {
    assert!(
        !out.status.success(),
        "expected a refusal, got exit {:?} with stdout {:?}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        out.stdout.is_empty(),
        "no marker may be minted on refusal: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains(needle), "stderr: {stderr}");
}

#[test]
fn context_refuses_every_way_the_review_minted_a_marker_and_still_mints_the_real_one() {
    let Some(server) = ScratchTmux::start("own") else {
        eprintln!("skipping: no usable tmux");
        return;
    };
    let home = Home::new();
    let session = server.session_id("proj");
    let pane = server.pane_id();
    let socket = server.socket_path();
    let reservation = seed_space(&home, &server.ns, &session);
    let space = reservation.space_uid.0;

    // 1. Registry NULL: the instance has published no epoch, so nothing can
    //    verify the server that answers on the namespace.
    refused(
        &home.context(space, &socket, &pane),
        "has published no server epoch",
    );

    // Publish the real server's incarnation; from here the instance is E.
    let epoch = publish(&home, &server.ns);

    // Positive control: the pane's own managed server mints the marker.
    let out = home.context(space, &socket, &pane);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["space_uid"], space.to_string().as_str());
    assert_eq!(doc["server_epoch"], epoch.0.to_string().as_str());

    // 2. Stranger endpoint: the pane claims a different `-L` server. The
    //    ambient namespace is not the instance's recorded endpoint, so the
    //    stranger is never scanned.
    let stranger = ScratchTmux::start("stranger").expect("second scratch server");
    refused(
        &home.context(space, &stranger.socket_path(), &stranger.pane_id()),
        "is not the managed instance's recorded endpoint",
    );

    // 3. Rebind/tamper: the registry binds a second Space to another session
    //    on the same server, and the pane claims to be that Space's pane.
    //    The pane is not part of the bound token, so the answer is a typed
    //    not-found, never a marker for whatever session it is in.
    server.tmux(&["new-session", "-d", "-s", "other"]);
    let other = server.session_id("other");
    let rebound = {
        let mut registry = home.registry();
        let instance = registry
            .backend_instance_for_backend(Backend::Tmux)
            .unwrap()
            .unwrap();
        let reservation = registry
            .reserve_space("other", instance, Uuid::new_v4())
            .unwrap();
        registry
            .finalize_create(
                reservation.space_uid,
                reservation.operation_uid,
                &NativeBindingSpec {
                    native_token: other.clone(),
                    native_kind: NativeKind::TmuxSessionId,
                    server_epoch: Some(epoch),
                },
            )
            .unwrap();
        registry
            .set_space_health(reservation.space_uid, Health::Healthy)
            .unwrap();
        reservation.space_uid.0
    };
    refused(&home.context(rebound, &socket, &pane), "is not part of");

    // 4. Stale live epoch: the published incarnation is gone and a fresh,
    //    unstamped server answers on the same namespace. The pinned scan
    //    refuses it; nothing is minted from the newcomer.
    server.kill();
    let replacement = ScratchTmux {
        ns: server.ns.clone(),
    };
    let started = Command::new("tmux")
        .args(["-L", &replacement.ns, "-f", "/dev/null"])
        .args(["new-session", "-d", "-s", "proj"])
        .status()
        .unwrap();
    assert!(started.success());
    let out = home.context(space, &replacement.socket_path(), &replacement.pane_id());
    assert!(!out.status.success(), "a replaced server must refuse");
    assert!(out.stdout.is_empty(), "no marker from a replaced server");
    std::mem::forget(server);
}
