//! Site 2, review finding #4 inverted (ADR 012 WS-A.5; acceptance case 26).
//!
//! `dmux group new` on a tmux Space builds its provider/scope in
//! `space_cli::resolve`. Before this fix that arm hardcoded an unmanaged
//! scope (`expected_epoch: None`) for a Space whose managed instance is a
//! foreign key the registry vouches for — so `group_new` ran a native
//! spawn against whatever server answered on the namespace. The review
//! proved a window `@2` appearing on an impostor tmux server.
//!
//! The fix routes the tmux arm through `backend::scope::resolve_managed_instance`:
//! a published instance pins to its epoch, an instance the registry cannot
//! vouch for (no published epoch, no recorded endpoint) is refused before
//! any provider exists. This proves the refusal end to end through the real
//! binary: NULL epoch → `group new` refuses `backend_epoch_changed`, no
//! window is created on the scratch server, and the Space's binding is
//! untouched.
//!
//! The heavier ordering bug the footnote names (`operations::group_new`
//! mutates then calls `split_list`, which refuses under an unpinned scope,
//! leaking a window — ADR 012 §3.4/WS-A.11) belongs to the operations owner
//! in a later wave. This fix makes the unpinned scope unreachable from the
//! verb, so the window-count assertion below holds regardless of it.

use std::process::{Command, Output, Stdio};

use dmux::model::{Backend, Health};
use dmux::registry::{NativeBindingSpec, NativeKind, Registry, RegistryConfig, SpaceReservation};
use serde_json::Value;
use uuid::Uuid;

/// A private tmux server on its own `-L` namespace, holding one session. Its
/// epoch is never stamped, so nothing in the registry can vouch for it — the
/// "impostor" of the review's reproduction.
struct ScratchTmux {
    ns: String,
}

impl ScratchTmux {
    fn start(tag: &str) -> Option<ScratchTmux> {
        let server = ScratchTmux {
            ns: format!("dmux-p11a5b-{tag}-{}", std::process::id()),
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
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn session_id(&self) -> String {
        self.tmux(&["list-sessions", "-F", "#{session_id}"])
            .trim()
            .to_string()
    }

    fn window_count(&self) -> usize {
        self.tmux(&["list-windows", "-t", "proj", "-F", "#{window_id}"])
            .lines()
            .count()
    }
}

impl Drop for ScratchTmux {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.ns, "kill-server"])
            .output();
    }
}

/// A private HOME/XDG root so the `dmux` binary resolves the registry this
/// test built, never the developer's. The group verbs open the production
/// registry (`OperationEnv::production`), which reads `$XDG_DATA_HOME/dmux`.
struct Home {
    home: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        Home {
            home: tempfile::tempdir().unwrap(),
        }
    }

    fn db_path(&self) -> std::path::PathBuf {
        self.home.path().join("dmux/registry.sqlite3")
    }

    /// A registry on the scratch db, on a scratch lock directory — reads
    /// take no writer's fence, so the test's own reads never collide with
    /// the CLI's.
    fn registry(&self) -> Registry {
        let locks = self.home.path().join("locks");
        std::fs::create_dir_all(&locks).unwrap();
        Registry::open(RegistryConfig::new(self.db_path(), locks)).unwrap()
    }

    fn dmux(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_dmux"))
            .args(args)
            .env("HOME", self.home.path())
            .env("XDG_DATA_HOME", self.home.path())
            .env("XDG_STATE_HOME", self.home.path())
            .env_remove("DMUX_WEZ_FIRST")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("DMUX_HOST_UID")
            .env_remove("DMUX_SPACE_UID")
            .env_remove("DMUX_GROUP_REF")
            .env_remove("DMUX_SPLIT_REF")
            .env_remove("WEZTERM_UNIX_SOCKET")
            .env_remove("WEZTERM_PANE")
            .env_remove("NO_COLOR")
            .stdin(Stdio::null())
            .output()
            .expect("dmux runs")
    }
}

/// Seed an Active, Healthy tmux Space bound to `session_id` on `namespace`,
/// whose backend instance has NO published epoch — exactly the state finding
/// #4 describes: an FK instance the registry vouches for, minus the epoch
/// that would let anything verify its live server.
fn seed_unpublished_active_space(
    home: &Home,
    namespace: &str,
    session_id: &str,
) -> (dmux::model::BackendInstanceUid, SpaceReservation) {
    let mut registry = home.registry();
    let instance = registry
        .register_backend_instance(Backend::Tmux, Some(namespace), None)
        .unwrap();
    // No publish_backend_server: server_epoch stays NULL.
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
    (instance, reservation)
}

fn sole_document(out: &Output) -> Value {
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(text.lines().count(), 1, "not one document: {text:?}");
    serde_json::from_str(text.trim()).unwrap_or_else(|e| panic!("{e}: {text}"))
}

#[test]
fn group_new_on_an_unpublished_tmux_space_refuses_and_creates_no_window() {
    let Some(server) = ScratchTmux::start("groupnew") else {
        eprintln!("skipping: no usable tmux");
        return;
    };
    let home = Home::new();
    let session_id = server.session_id();
    let (instance, reservation) = seed_unpublished_active_space(&home, &server.ns, &session_id);
    assert_eq!(server.window_count(), 1, "the seed session has one window");

    let out = home.dmux(&[
        "--format",
        "json",
        "group",
        "new",
        "proj",
        "--no-connect",
        "--",
        "sleep",
        "300",
    ]);

    // Refused before any native call: `backend_epoch_changed`, one document.
    assert_eq!(
        out.status.code(),
        Some(1),
        "stdout: {} stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let doc = sole_document(&out);
    assert_eq!(doc["action"], "group_new", "{doc}");
    assert_eq!(doc["ok"], false, "{doc}");
    assert_eq!(doc["errors"][0]["code"], "backend_epoch_changed", "{doc}");
    let detail = doc["errors"][0]["message"].as_str().unwrap();
    assert!(detail.contains("has published no server epoch"), "{detail}");
    assert!(detail.contains(&instance.0.to_string()), "{detail}");

    // No window was created on the impostor server, and no second session.
    assert_eq!(
        server.window_count(),
        1,
        "group new must not spawn a window under an unverified scope"
    );
    assert_eq!(
        server
            .tmux(&["list-sessions", "-F", "#{session_id}"])
            .lines()
            .count(),
        1,
        "no session was created either"
    );

    // The Space's binding is exactly what it was: no child rows minted, no
    // rebinding to whatever answered on the namespace.
    let registry = home.registry();
    let binding = registry
        .current_binding(reservation.space_uid)
        .unwrap()
        .expect("the seeded binding is intact");
    assert_eq!(binding.native_token, session_id);
    assert!(
        registry
            .unfinished_operation(reservation.space_uid)
            .unwrap()
            .is_none(),
        "a refused group new leaves no unfinished operation behind"
    );
}
