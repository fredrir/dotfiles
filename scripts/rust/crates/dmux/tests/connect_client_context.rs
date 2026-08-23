//! The ambient tmux client-context reader behind the gated `dmux con`
//! (`connect_cli::production_connect_client_context`) compares the socket
//! witnesses `tmux_bootstrap` published — pid, start token, socket dev/ino —
//! against a fresh probe of the live server before it trusts the server's
//! self-reported epoch (ADR 012 WS-A.9 at this reader, O's close → D3;
//! acceptance case 27). The model is
//! `operations_flow::a_replaced_tmux_socket_presenting_the_old_epoch_is_refused_everywhere`,
//! driven here through the real binary with a scratch tmux server on a
//! private `-L` namespace, the way an operator's shell reaches it.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
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
            ns: format!("dmux-d3-{tag}-{}", std::process::id()),
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

    fn server_pid(&self) -> String {
        // `#{pid}` is the server PID; `server_pid` is not a tmux format.
        self.tmux(&["display-message", "-p", "#{pid}"])
    }

    fn kill(&self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.ns, "kill-server"])
            .output();
    }
}

impl Drop for ScratchTmux {
    fn drop(&mut self) {
        self.kill();
    }
}

/// The owner-side seams the gated binary reads: `XDG_DATA_HOME` for the
/// registry (`<data>/dmux/registry.sqlite3`), `DMUX_RUNTIME_DIR` for every
/// lock, `XDG_STATE_HOME` for history. Nothing here can reach the live
/// runtime directory.
struct Home {
    home: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        let home = Home {
            home: tempfile::tempdir().unwrap(),
        };
        std::fs::create_dir_all(home.data_home().join("dmux")).unwrap();
        std::fs::create_dir_all(home.runtime_dir()).unwrap();
        std::fs::set_permissions(home.runtime_dir(), std::fs::Permissions::from_mode(0o700))
            .unwrap();
        std::fs::create_dir_all(home.state_home()).unwrap();
        home
    }

    fn data_home(&self) -> PathBuf {
        self.home.path().join("data")
    }

    fn runtime_dir(&self) -> PathBuf {
        self.home.path().join("rt")
    }

    fn state_home(&self) -> PathBuf {
        self.home.path().join("state")
    }

    fn env(&self) -> OperationEnv {
        OperationEnv {
            db_path: self.data_home().join("dmux").join("registry.sqlite3"),
            lock_dir: self.runtime_dir(),
        }
    }

    fn registry(&self) -> Registry {
        let env = self.env();
        Registry::open(RegistryConfig::new(env.db_path, env.lock_dir)).unwrap()
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dmux"));
        command
            .args(args)
            .env("XDG_DATA_HOME", self.data_home())
            .env("XDG_STATE_HOME", self.state_home())
            .env("DMUX_RUNTIME_DIR", self.runtime_dir())
            .env("DMUX_WEZ_FIRST", "1")
            .env_remove("DMUX_DRY_RUN")
            .env_remove("DMUX_LEGACY_POLICY")
            .env_remove("WEZTERM_PANE")
            .env_remove("WEZTERM_UNIX_SOCKET")
            .env_remove("TERM_PROGRAM")
            .stdin(Stdio::null());
        command
    }

    /// Mint the pane's exact marker the way the prompt hook does, so the
    /// environment `con` reads below is the authoritative one.
    fn marker(&self, space_uid: Uuid, socket_path: &str, pane: &str) -> serde_json::Value {
        let env = self.env();
        let out = self
            .command(&[
                "_context",
                "--data-dir",
                env.db_path.parent().unwrap().to_str().unwrap(),
                "--lock-dir",
                env.lock_dir.to_str().unwrap(),
            ])
            .env("DMUX_SPACE_UID", space_uid.to_string())
            .env("TMUX", format!("{socket_path},1,0"))
            .env("TMUX_PANE", pane)
            .output()
            .expect("dmux runs");
        assert!(
            out.status.success(),
            "_context must mint the real marker: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }

    /// The gated `dmux con REF` from inside the pane: `$TMUX`/`$TMUX_PANE`
    /// select the invoking client, the `DMUX_*` marker describes it.
    fn con(
        &self,
        marker: &serde_json::Value,
        socket_path: &str,
        pane: &str,
        reference: &str,
    ) -> Output {
        let text = |key: &str| match &marker[key] {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Null => String::new(),
            other => other.to_string(),
        };
        self.command(&["con", reference])
            .env("TMUX", format!("{socket_path},1,0"))
            .env("TMUX_PANE", pane)
            .env("DMUX_CONTEXT_VERSION", "1")
            .env("DMUX_HOST_UID", text("host_uid"))
            .env("DMUX_SPACE_UID", text("space_uid"))
            .env("DMUX_SPACE_NO", text("space_no"))
            .env("DMUX_BACKEND", text("backend"))
            .env("DMUX_DOMAIN", text("domain"))
            .env("DMUX_SERVER_EPOCH", text("server_epoch"))
            .env("DMUX_GROUP_REF", text("group_ref"))
            .env("DMUX_SPLIT_REF", text("split_ref"))
            .output()
            .expect("dmux runs")
    }
}

/// An Active tmux Space bound to `session_id` on an instance registered at
/// `namespace`; the incarnation is published by `tmux_bootstrap` afterwards.
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
        TmuxBootstrapOutcome::Bootstrapped { epoch }
        | TmuxBootstrapOutcome::AlreadyBound { epoch }
        | TmuxBootstrapOutcome::Rebound { epoch, .. } => epoch,
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A replaced server on the same namespace — same socket path, new inode,
/// new pid — that presents the OLD `@dmux_server_epoch` is what the epoch
/// option alone cannot tell apart. The client-context reader now refuses it
/// as a stale incarnation (`backend_epoch_changed`, the operations layer's
/// code for state F) before the ref is even resolved; the same invocation on
/// the published incarnation gets past the reader and fails later, on the
/// ref, which is the positive control.
#[test]
fn a_replaced_tmux_socket_presenting_the_old_epoch_is_refused_by_the_client_context_reader() {
    let Some(server) = ScratchTmux::start("replaced") else {
        eprintln!("skipping: no usable tmux");
        return;
    };
    let home = Home::new();
    let session = server.session_id("proj");
    let socket = server.socket_path();
    let pane = server.pane_id();
    let reservation = seed_space(&home, &server.ns, &session);
    let space = reservation.space_uid.0;
    let epoch = publish(&home, &server.ns);
    let instance = home
        .registry()
        .backend_instance_for_backend(Backend::Tmux)
        .unwrap()
        .expect("registered");
    let published = home.registry().backend_server(instance).unwrap();
    assert!(
        published.socket_dev.is_some() && published.socket_ino.is_some(),
        "tmux_bootstrap publishes the socket witnesses the reader compares"
    );
    let marker = home.marker(space, &socket, &pane);
    assert_eq!(marker["server_epoch"], epoch.0.to_string().as_str());

    // Positive control: on the published incarnation the reader is satisfied
    // and `con` proceeds to resolve the ref — which names nothing (exit 3).
    let out = home.con(&marker, &socket, &pane, "99");
    assert_eq!(
        out.status.code(),
        Some(3),
        "the reader must pass on the live incarnation; stderr: {}",
        stderr(&out)
    );
    assert!(
        !stderr(&out).contains("stale incarnation"),
        "{}",
        stderr(&out)
    );

    // Replace the server: kill it, start another on the same namespace with
    // the session name recycled, then copy the old epoch onto it. Nothing
    // the registry recorded survives but the epoch.
    let old_pid = server.server_pid();
    server.kill();
    let replacement = ScratchTmux {
        ns: server.ns.clone(),
    };
    std::mem::forget(server);
    let started = Command::new("tmux")
        .args(["-L", &replacement.ns, "-f", "/dev/null"])
        .args(["new-session", "-d", "-s", "proj"])
        .status()
        .unwrap();
    assert!(started.success());
    replacement.tmux(&[
        "set-option",
        "-g",
        "@dmux_server_epoch",
        &epoch.0.to_string(),
    ]);
    assert_eq!(replacement.socket_path(), socket, "same socket path");
    assert_ne!(replacement.server_pid(), old_pid, "different process");
    assert_eq!(
        replacement.tmux(&["show-option", "-gv", "@dmux_server_epoch"]),
        epoch.0.to_string(),
        "the impostor presents the old epoch"
    );

    let out = home.con(&marker, &socket, &replacement.pane_id(), "99");
    assert_eq!(
        out.status.code(),
        Some(1),
        "backend_epoch_changed exits 1; stderr: {}",
        stderr(&out)
    );
    let text = stderr(&out);
    assert!(
        text.contains("stale incarnation") && text.contains("ADR 012 §3.1 state F"),
        "the reader's own witness comparison must name the fault: {text}"
    );
    assert!(
        text.contains("repair retire-incarnation"),
        "the remedy is the operator's retire-then-bootstrap: {text}"
    );
    assert!(
        !text.contains("tmux server incarnation changed"),
        "verify_epoch's wrong_backend_instance must not have fired first: {text}"
    );
    assert!(
        !text.contains("no Space") && !text.contains("not found"),
        "the refusal precedes ref resolution: {text}"
    );
}

/// A row published before WS-A.9 carries no socket witnesses; the reader
/// then verifies by identity and epoch alone and still refuses a replaced
/// server — through `verify_epoch`'s pid comparison — so the compatibility
/// arm never widens what an unwitnessed row admits.
#[test]
fn an_unwitnessed_row_still_refuses_a_replaced_server_by_identity() {
    let Some(server) = ScratchTmux::start("unwitnessed") else {
        eprintln!("skipping: no usable tmux");
        return;
    };
    let home = Home::new();
    let session = server.session_id("proj");
    let socket = server.socket_path();
    let pane = server.pane_id();
    let reservation = seed_space(&home, &server.ns, &session);
    let space = reservation.space_uid.0;
    let epoch = publish(&home, &server.ns);
    let marker = home.marker(space, &socket, &pane);

    // Strip the witnesses the way an r5-era row would look, keeping pid,
    // start token and epoch.
    {
        let mut registry = home.registry();
        let instance = registry
            .backend_instance_for_backend(Backend::Tmux)
            .unwrap()
            .unwrap();
        let published = registry.backend_server(instance).unwrap();
        registry
            .publish_backend_server(
                instance,
                epoch,
                published.server_pid,
                published.server_start_token.as_deref(),
                None,
                None,
            )
            .unwrap();
    }
    let out = home.con(&marker, &socket, &pane, "99");
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));

    server.kill();
    let replacement = ScratchTmux {
        ns: server.ns.clone(),
    };
    std::mem::forget(server);
    let started = Command::new("tmux")
        .args(["-L", &replacement.ns, "-f", "/dev/null"])
        .args(["new-session", "-d", "-s", "proj"])
        .status()
        .unwrap();
    assert!(started.success());
    replacement.tmux(&[
        "set-option",
        "-g",
        "@dmux_server_epoch",
        &epoch.0.to_string(),
    ]);
    let out = home.con(&marker, &socket, &replacement.pane_id(), "99");
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let text = stderr(&out);
    assert!(
        text.contains("tmux server incarnation changed"),
        "identity alone refuses the replacement: {text}"
    );
}
