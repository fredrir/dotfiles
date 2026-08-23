//! Instance state F end to end (ADR 012 WS-B.1; plan §5.2 as amended, §8.1;
//! review finding #1 and report 04 row F).
//!
//! Before this, a published epoch was write-once-and-never-invalidated: a
//! row naming a dead pid was permanent and every reader treated it as
//! authoritative — on Macie the registry named pid 5458 while the live mux
//! was pid 54528, and nothing in the crate could observe it. Now
//! `backend::scope::resolve_managed` refutes the row with a liveness probe
//! (pid, start token, socket dev/ino against a fresh `stat`) and classifies
//! it `StaleIncarnation`. This proves, through the real binary against a
//! real scratch tmux server that is bootstrapped, killed and replaced: every
//! verb refuses with `backend_epoch_changed` and a `stale_incarnation`
//! detail, runs no tmux command against the replacement, mutates nothing —
//! and `dmux repair retire-incarnation` is the clear that returns the
//! instance to `Unpublished`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use dmux::backend::scope::{self, ManagedTarget, ObservedIncarnation, resolve_managed};
use dmux::model::{Backend, Health, OperationKind, ServerEpoch};
use dmux::operations::{OperationEnv, TmuxBootstrapOutcome, tmux_bootstrap};
use dmux::registry::{NativeBindingSpec, NativeKind, Registry, RegistryConfig};
use serde_json::Value;
use uuid::Uuid;

/// The real `tmux` on this process's PATH.
fn real_tmux() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").expect("PATH"))
        .map(|dir| dir.join("tmux"))
        .find(|candidate| candidate.is_file())
        .expect("a real tmux on PATH (the suite already depends on one)")
}

/// A private tmux server on its own `-L` namespace. The test talks to it
/// through the real binary directly, never through the recording wrapper
/// the `dmux` children get, so the wrapper's log holds only what dmux ran.
struct ScratchTmux {
    ns: String,
}

impl ScratchTmux {
    /// Start (or restart, on a namespace whose previous server was just
    /// killed) with a bounded retry: Linux tmux can still be tearing the
    /// old incarnation down when the newcomer binds, and then reports
    /// "server exited unexpectedly" for what is a transient (ADR 012 §10).
    fn start(ns: &str) -> Option<ScratchTmux> {
        let server = ScratchTmux { ns: ns.to_string() };
        for attempt in 0..40 {
            let started = Command::new(real_tmux())
                .args(["-L", &server.ns, "-f", "/dev/null"])
                .args(["new-session", "-d", "-s", "proj"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            if matches!(started, Ok(status) if status.success()) {
                return Some(server);
            }
            if attempt == 39 {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        None
    }

    fn tmux(&self, args: &[&str]) -> String {
        let out = Command::new(real_tmux())
            .args(["-L", &self.ns])
            .args(args)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn session_id(&self) -> String {
        self.tmux(&["display-message", "-p", "-t", "proj", "#{session_id}"])
    }

    fn pane_id(&self) -> String {
        self.tmux(&["display-message", "-p", "-t", "proj", "#{pane_id}"])
    }

    fn socket_path(&self) -> String {
        self.tmux(&["display-message", "-p", "#{socket_path}"])
    }

    fn pid(&self) -> i64 {
        self.tmux(&["display-message", "-p", "#{pid}"])
            .parse()
            .expect("tmux #{pid}")
    }

    fn window_count(&self) -> usize {
        self.tmux(&["list-windows", "-t", "proj", "-F", "#{window_id}"])
            .lines()
            .count()
    }

    fn sessions(&self) -> Vec<String> {
        self.tmux(&["list-sessions", "-F", "#{session_name}"])
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// Kill the server and wait until its process is really gone, so the
    /// published pid is dead rather than exiting.
    fn kill_and_reap(&self) {
        let pid = self.pid();
        let _ = Command::new(real_tmux())
            .args(["-L", &self.ns, "kill-server"])
            .output();
        for _ in 0..200 {
            if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("tmux server {pid} did not exit after kill-server");
    }
}

impl Drop for ScratchTmux {
    fn drop(&mut self) {
        let _ = Command::new(real_tmux())
            .args(["-L", &self.ns, "kill-server"])
            .output();
    }
}

/// A private HOME/XDG/runtime root the binary resolves everything from,
/// plus a `tmux` wrapper ahead of the real one on the children's PATH that
/// records every invocation before forwarding it.
struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("dmux")).unwrap();
        fs::create_dir_all(dir.path().join("rt")).unwrap();
        fs::set_permissions(dir.path().join("rt"), fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir_all(dir.path().join("bin")).unwrap();
        let log = dir.path().join("tmux.log");
        let wrapper = dir.path().join("bin/tmux");
        fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexec '{}' \"$@\"\n",
                log.display(),
                real_tmux().display()
            ),
        )
        .unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
        Home { dir }
    }

    fn data_dir(&self) -> PathBuf {
        self.dir.path().join("dmux")
    }

    fn runtime_dir(&self) -> PathBuf {
        self.dir.path().join("rt")
    }

    fn env(&self) -> OperationEnv {
        OperationEnv {
            db_path: self.data_dir().join("registry.sqlite3"),
            lock_dir: self.runtime_dir(),
        }
    }

    fn registry(&self) -> Registry {
        let env = self.env();
        Registry::open(RegistryConfig::new(env.db_path, env.lock_dir)).unwrap()
    }

    /// Every tmux argv a dmux child ran since the last `clear_tmux_log`.
    fn tmux_log(&self) -> String {
        fs::read_to_string(self.dir.path().join("tmux.log")).unwrap_or_default()
    }

    fn clear_tmux_log(&self) {
        let _ = fs::remove_file(self.dir.path().join("tmux.log"));
    }

    fn command(&self, args: &[&str]) -> Command {
        let path = std::env::join_paths(
            std::iter::once(self.dir.path().join("bin"))
                .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_dmux"));
        command
            .args(args)
            .env("PATH", path)
            .env("HOME", self.dir.path())
            .env("XDG_DATA_HOME", self.dir.path())
            .env("XDG_STATE_HOME", self.dir.path())
            .env("DMUX_RUNTIME_DIR", self.runtime_dir())
            .env("DMUX_WEZ_FIRST", "1")
            .env_remove("DMUX_DRY_RUN")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("DMUX_HOST_UID")
            .env_remove("DMUX_SPACE_UID")
            .env_remove("DMUX_GROUP_REF")
            .env_remove("DMUX_SPLIT_REF")
            .env_remove("WEZTERM_UNIX_SOCKET")
            .env_remove("WEZTERM_PANE")
            .env_remove("NO_COLOR")
            .stdin(Stdio::null());
        command
    }

    fn dmux(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("dmux runs")
    }

    /// The hidden seams for the verbs that take them, pointed at the same
    /// registry and lock directory the XDG resolution reaches.
    fn seams(&self) -> [String; 4] {
        [
            "--data-dir".to_string(),
            self.data_dir().display().to_string(),
            "--lock-dir".to_string(),
            self.runtime_dir().display().to_string(),
        ]
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn sole_document(out: &Output) -> Value {
    let text = stdout(out);
    assert_eq!(
        text.trim().lines().count(),
        1,
        "not one document: {text:?}\nstderr: {}",
        stderr(out)
    );
    serde_json::from_str(text.trim()).unwrap_or_else(|e| panic!("{e}: {text}"))
}

/// The refusal every verb owes a stale incarnation: `backend_epoch_changed`
/// carrying the `stale_incarnation` detail, exit 1, nothing else.
fn refused_stale(verb: &str, out: &Output) -> Value {
    assert_eq!(
        out.status.code(),
        Some(1),
        "{verb}: stdout {} stderr {}",
        stdout(out),
        stderr(out)
    );
    let doc = sole_document(out);
    assert_eq!(doc["ok"], false, "{verb}: {doc}");
    assert_eq!(
        doc["errors"][0]["code"], "backend_epoch_changed",
        "{verb}: {doc}"
    );
    let message = doc["errors"][0]["message"].as_str().unwrap();
    assert!(
        message.contains("stale_incarnation"),
        "{verb}: the detail names the state: {message}"
    );
    assert!(
        message.contains("is dead"),
        "{verb}: the detail names the observation: {message}"
    );
    doc
}

/// Bootstrap the scratch server into the registry (state E) and bind an
/// Active, Healthy Space to its session under the published epoch.
fn bootstrap_and_bind(home: &Home, server: &ScratchTmux) -> (ServerEpoch, Uuid) {
    match tmux_bootstrap(&home.env(), &server.ns).unwrap() {
        TmuxBootstrapOutcome::Bootstrapped { .. } => {}
        other => panic!("a fresh server bootstraps: {other:?}"),
    }
    let mut registry = home.registry();
    let instance = registry
        .backend_instance_for_backend(Backend::Tmux)
        .unwrap()
        .expect("tmux_bootstrap registered the instance");
    let record = registry.backend_server(instance).unwrap();
    let epoch = record.server_epoch.expect("tmux_bootstrap published");
    assert_eq!(record.server_pid, Some(server.pid()));
    assert!(record.socket_dev.is_some() && record.socket_ino.is_some());
    let reservation = registry
        .reserve_space("proj", instance, Uuid::new_v4())
        .unwrap();
    registry
        .finalize_create(
            reservation.space_uid,
            reservation.operation_uid,
            &NativeBindingSpec {
                native_token: server.session_id(),
                native_kind: NativeKind::TmuxSessionId,
                server_epoch: Some(epoch),
            },
        )
        .unwrap();
    registry
        .set_space_health(reservation.space_uid, Health::Healthy)
        .unwrap();
    (epoch, reservation.space_uid.0)
}

#[test]
fn every_verb_refuses_a_published_incarnation_whose_process_is_dead_until_it_is_retired() {
    let ns = format!("dmux-stale-{}", std::process::id());
    let Some(server) = ScratchTmux::start(&ns) else {
        eprintln!("skipping: no usable tmux");
        return;
    };
    let home = Home::new();
    let (epoch, space) = bootstrap_and_bind(&home, &server);
    let published_pid = server.pid();

    // State E: the published incarnation is the live one.
    assert!(matches!(
        resolve_managed(&home.registry(), Backend::Tmux).unwrap(),
        ManagedTarget::Managed { .. }
    ));
    let ls_live = home.dmux(&["--format", "json", "ls"]);
    assert_eq!(ls_live.status.code(), Some(0), "{}", stderr(&ls_live));
    assert_eq!(sole_document(&ls_live)["result"][0]["observation"], "live");

    // Kill the published server and put a replacement on the same
    // namespace: a fresh pid, a fresh socket inode, no epoch option. The
    // registry row is now exactly Macie's shape — a published epoch, pid,
    // start token and dev/ino that nothing live answers to.
    server.kill_and_reap();
    let replacement = ScratchTmux::start(&ns).expect("replacement server");
    std::mem::forget(server);
    assert_ne!(replacement.pid(), published_pid);
    let head_before = home.registry().authority_head().unwrap();
    home.clear_tmux_log();

    // The resolver says F, observed as a dead process, and hands out no scope.
    let target = resolve_managed(&home.registry(), Backend::Tmux).unwrap();
    let ManagedTarget::StaleIncarnation {
        published,
        observed,
        ..
    } = &target
    else {
        panic!("expected StaleIncarnation, got {target:?}");
    };
    assert_eq!(published.epoch, epoch);
    assert_eq!(
        *observed,
        ObservedIncarnation::ProcessDead { pid: published_pid }
    );
    assert!(target.scope().is_none());

    // `ls`: partial, the typed epoch fault with the stale detail, the Space
    // `unreachable` — never `stopped` (§8.1), never `absent` — and nothing
    // discovered from the replacement.
    let out = home.dmux(&["--format", "json", "ls"]);
    assert_eq!(out.status.code(), Some(7), "{}", stderr(&out));
    let doc = sole_document(&out);
    assert_eq!(doc["ok"], false);
    assert_eq!(doc["errors"][0]["code"], "backend_epoch_changed", "{doc}");
    let message = doc["errors"][0]["message"].as_str().unwrap();
    assert!(message.contains("stale_incarnation"), "{message}");
    assert!(message.contains(&epoch.0.to_string()), "{message}");
    let rows = doc["result"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "only the durable row: {doc}");
    assert_eq!(rows[0]["name"], "proj");
    assert_eq!(rows[0]["observation"], "unreachable", "{doc}");
    assert!(
        stderr(&out).contains("stale_incarnation"),
        "the operator sees it too: {}",
        stderr(&out)
    );

    // `new`: refused before any lookup or reservation, explicit backend and
    // auto selection alike.
    for args in [
        vec!["new", "other", "--backend", "tmux", "--no-connect"],
        vec!["new", "other", "--no-connect"],
    ] {
        let out = home.dmux(&args);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {}", stderr(&out));
        assert!(
            stderr(&out).contains("stale_incarnation"),
            "{args:?}: {}",
            stderr(&out)
        );
        assert_eq!(stdout(&out), "", "{args:?}: a receipt was printed");
    }

    // `adopt`: the replacement's session is not adoptable on a stale row.
    let native = dmux::output::native_ref(Backend::Tmux, &replacement.session_id());
    refused_stale("adopt", &home.dmux(&["--format", "json", "adopt", &native]));

    // `group new`: no window on the replacement.
    let doc = refused_stale(
        "group new",
        &home.dmux(&[
            "--format",
            "json",
            "group",
            "new",
            "proj",
            "--no-connect",
            "--",
            "sleep",
            "300",
        ]),
    );
    assert_eq!(doc["action"], "group_new", "{doc}");
    assert_eq!(replacement.window_count(), 1);

    // `rm --row 1`: the row is inside the durable prefix, so it resolves to
    // the Space — and the frozen target refuses before the fenced remove.
    refused_stale(
        "rm --row",
        &home.dmux(&["--format", "json", "rm", "--yes", "--row", "1"]),
    );

    // `_context`: the pane claims the replacement on the recorded namespace;
    // no marker is minted.
    let seams = home.seams();
    let out = home
        .command(&["_context", &seams[0], &seams[1], &seams[2], &seams[3]])
        .env("DMUX_SPACE_UID", space.to_string())
        .env("TMUX", format!("{},1,0", replacement.socket_path()))
        .env("TMUX_PANE", replacement.pane_id())
        .output()
        .unwrap();
    assert!(!out.status.success(), "_context: {}", stdout(&out));
    assert_eq!(stdout(&out), "", "_context: a marker was minted");
    assert!(
        stderr(&out).contains("stale_incarnation"),
        "_context: {}",
        stderr(&out)
    );

    // Nothing above ran a tmux command against the replacement, and
    // nothing wrote to the registry.
    assert_eq!(
        home.tmux_log(),
        "",
        "a verb ran tmux against a server the registry does not vouch for"
    );
    assert_eq!(replacement.sessions(), ["proj"]);
    let registry = home.registry();
    assert_eq!(registry.authority_head().unwrap(), head_before);
    assert_eq!(registry.spaces().unwrap().len(), 1);
    assert_eq!(
        registry.spaces().unwrap()[0].lifecycle,
        dmux::model::Lifecycle::Active
    );
    drop(registry);

    // `repair reconcile`: a row a crashed create stranded on this instance
    // fails closed rather than being decided on the replacement's word.
    let stranded = {
        let mut registry = home.registry();
        let instance = registry
            .backend_instance_for_backend(Backend::Tmux)
            .unwrap()
            .unwrap();
        registry
            .reserve_space_kind("stranded", instance, Uuid::new_v4(), OperationKind::Create)
            .unwrap()
    };
    let out = home.dmux(&[
        "--format",
        "json",
        "repair",
        "reconcile",
        "--yes",
        &seams[0],
        &seams[1],
        &seams[2],
        &seams[3],
    ]);
    assert_eq!(out.status.code(), Some(7), "{}", stderr(&out));
    let doc = sole_document(&out);
    assert_eq!(
        doc["result"]["results"][0]["outcome"], "failed_closed",
        "{doc}"
    );
    assert_eq!(doc["errors"][0]["code"], "backend_epoch_changed", "{doc}");
    assert!(
        doc["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("stale_incarnation"),
        "{doc}"
    );
    assert_eq!(
        home.registry().space(stranded.space_uid).unwrap().lifecycle,
        dmux::model::Lifecycle::Reserved
    );
    assert_eq!(home.tmux_log(), "");

    // The operator's clear: retire the named epoch (the pid is dead, so no
    // --allow-live-pid), after which the instance is Unpublished (state C)
    // and a fresh bootstrap can publish the replacement.
    let out = home.dmux(&[
        "--format",
        "json",
        "repair",
        "retire-incarnation",
        "--backend",
        "tmux",
        "--epoch",
        &epoch.0.to_string(),
        "--yes",
        &seams[0],
        &seams[1],
        &seams[2],
        &seams[3],
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc = sole_document(&out);
    assert_eq!(doc["action"], "repair_retire_incarnation", "{doc}");
    assert_eq!(doc["result"]["retired_pid"], published_pid, "{doc}");
    assert!(matches!(
        resolve_managed(&home.registry(), Backend::Tmux).unwrap(),
        ManagedTarget::Unpublished(_)
    ));
    let out = home.dmux(&["--format", "json", "ls"]);
    assert_eq!(out.status.code(), Some(7), "{}", stderr(&out));
    let doc = sole_document(&out);
    assert_eq!(doc["errors"][0]["code"], "backend_epoch_changed");
    assert!(
        doc["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("has published no server epoch"),
        "{doc}"
    );
    assert!(
        !doc["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("stale_incarnation")
    );
}

/// The socket witness on its own: a published pid that is still alive (this
/// test's) with the socket dev/ino the real server bound, then the server
/// replaced — the fresh bind gets a fresh inode, and the OS probe refutes
/// the row without asking any server. The same row is refuted by `ls`'s
/// server-asking probe on the pid it answers with.
#[test]
fn a_replaced_tmux_socket_refutes_a_published_row_whose_pid_is_still_alive() {
    use std::os::unix::fs::MetadataExt;

    let ns = format!("dmux-stale-sock-{}", std::process::id());
    let Some(server) = ScratchTmux::start(&ns) else {
        eprintln!("skipping: no usable tmux");
        return;
    };
    // The resolver's own socket-path rule agrees with the server's report.
    let reported = fs::metadata(server.socket_path()).unwrap();
    let derived = fs::metadata(scope::tmux_socket_path(&ns)).unwrap();
    assert_eq!(
        (reported.dev(), reported.ino()),
        (derived.dev(), derived.ino())
    );

    let home = Home::new();
    let epoch = ServerEpoch(Uuid::new_v4());
    let own_pid = i64::from(std::process::id());
    {
        let mut registry = home.registry();
        let instance = registry
            .register_backend_instance(Backend::Tmux, Some(&ns), None)
            .unwrap();
        registry
            .publish_backend_server(
                instance,
                epoch,
                Some(own_pid),
                Some("1700000000"),
                Some(reported.dev() as i64),
                Some(reported.ino() as i64),
            )
            .unwrap();
    }
    assert!(matches!(
        resolve_managed(&home.registry(), Backend::Tmux).unwrap(),
        ManagedTarget::Managed { .. }
    ));

    server.kill_and_reap();
    let replacement = ScratchTmux::start(&ns).expect("replacement server");
    std::mem::forget(server);
    let fresh = fs::metadata(replacement.socket_path()).unwrap();
    assert_ne!(
        fresh.ino(),
        reported.ino(),
        "a fresh bind gets a fresh inode"
    );

    let target = resolve_managed(&home.registry(), Backend::Tmux).unwrap();
    let ManagedTarget::StaleIncarnation { observed, .. } = &target else {
        panic!("expected StaleIncarnation, got {target:?}");
    };
    match observed {
        ObservedIncarnation::Process {
            pid,
            start_token,
            socket_ino,
            ..
        } => {
            assert_eq!(*pid, own_pid);
            assert_eq!(
                *start_token, None,
                "the OS probe does not guess tmux's token"
            );
            assert_eq!(*socket_ino, Some(fresh.ino() as i64));
        }
        other => panic!("expected a process witness, got {other:?}"),
    }

    // `ls` asks the server, which answers with its own pid and start token.
    let target = scope::resolve_managed_with(
        &home.registry(),
        Backend::Tmux,
        &dmux::ls_cli::LiveIncarnationProbe,
    )
    .unwrap();
    let ManagedTarget::StaleIncarnation { observed, .. } = &target else {
        panic!("expected StaleIncarnation, got {target:?}");
    };
    match observed {
        ObservedIncarnation::Process {
            pid, start_token, ..
        } => {
            assert_eq!(*pid, replacement.pid());
            assert!(start_token.is_some());
        }
        other => panic!("expected a process witness, got {other:?}"),
    }
    let out = home.dmux(&["--format", "json", "ls"]);
    assert_eq!(out.status.code(), Some(7), "{}", stderr(&out));
    let doc = sole_document(&out);
    assert_eq!(doc["errors"][0]["code"], "backend_epoch_changed", "{doc}");
    assert!(
        doc["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("stale_incarnation"),
        "{doc}"
    );
    assert_eq!(
        doc["result"].as_array().unwrap().len(),
        0,
        "no unmanaged rows from the replacement: {doc}"
    );
    let _ = Path::new("");
}
