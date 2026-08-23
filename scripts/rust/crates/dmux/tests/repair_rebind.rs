//! `dmux repair rebind SPACE_REF NATIVE_REF` (plan §7.1, §10.3; ADR 012
//! WS-D.1; ADR 011 D8's named remedy for the orphan `repair reconcile`
//! refuses to bind). Case 13's tamper clause end to end: external native-key
//! tampering becomes an explicit `absent`, the operator's confirmed rebind is
//! the only way back, the Space lands `unstamped`, and `context stamp` heals
//! it. The tmux leg drives the real binary against a scratch server on a
//! private `-L` namespace; the Wez leg drives the operations entry point
//! against a scripted `wezterm cli` (the fork CAS build is not a test
//! dependency; what is provable without it is the exact CAS argv and the
//! zero-mutation refusals).

use std::process::{Command, Output, Stdio};

use dmux::backend::InventoryScope;
use dmux::backend::wez::WezProvider;
use dmux::model::{Backend, BackendInstanceUid, Health, Lifecycle, OperationKind, ServerEpoch};
use dmux::operations::{OpError, OperationEnv, TmuxBootstrapOutcome, rebind_wez, tmux_bootstrap};
use dmux::output::native_ref;
use dmux::registry::{
    BindingState, NativeBindingSpec, NativeKind, Registry, RegistryConfig, SpaceReservation,
};
use serde_json::Value;
use uuid::Uuid;

#[path = "registry/util.rs"]
#[allow(dead_code)]
mod util;

use util::{Cas, FakeMux};

/// A home for one test: the registry under `XDG_DATA_HOME`, every lock and
/// runtime path under `DMUX_RUNTIME_DIR`. The binary resolves the Space
/// through its production resolver, so the seams are the environment, not
/// `--data-dir`/`--lock-dir`.
struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("xdg/dmux")).unwrap();
        std::fs::create_dir_all(dir.path().join("rt")).unwrap();
        Home { dir }
    }

    fn env(&self) -> OperationEnv {
        OperationEnv {
            db_path: self.dir.path().join("xdg/dmux/registry.sqlite3"),
            lock_dir: self.dir.path().join("rt"),
        }
    }

    fn registry(&self) -> Registry {
        let env = self.env();
        Registry::open(RegistryConfig::new(env.db_path, env.lock_dir)).unwrap()
    }

    fn revision(&self) -> u64 {
        self.registry().authority_head().unwrap().revision
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dmux"));
        command
            .args(args)
            .env("XDG_DATA_HOME", self.dir.path().join("xdg"))
            .env("XDG_STATE_HOME", self.dir.path().join("xdg/state"))
            .env("DMUX_RUNTIME_DIR", self.dir.path().join("rt"))
            .env("DMUX_WEZ_FIRST", "1")
            .env_remove("DMUX_LEGACY_POLICY")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("DMUX_HOST_UID")
            .env_remove("DMUX_SPACE_UID")
            .env_remove("WEZTERM_UNIX_SOCKET")
            .env_remove("WEZTERM_PANE")
            // Not a terminal, which is the state §7.4's rule is written for.
            .stdin(Stdio::null());
        command
    }

    fn dmux(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("dmux runs")
    }

    fn json(&self, args: &[&str]) -> Output {
        let mut argv = vec!["--format", "json"];
        argv.extend_from_slice(args);
        self.dmux(&argv)
    }

    /// `dmux context stamp` exactly as a pane's prompt hook runs it.
    fn stamp(&self, space: &str, pane: &str) -> Output {
        self.command(&["--format", "json", "context", "stamp", space])
            .env("TMUX_PANE", pane)
            .output()
            .expect("dmux runs")
    }

    /// The durable facts a rebind is judged by, straight from the tables.
    fn bindings(&self, space_uid: &str) -> Vec<(String, String)> {
        let registry = self.registry();
        let mut stmt = registry
            .raw_connection()
            .prepare(
                "SELECT native_token, binding_state FROM native_bindings \
                 WHERE space_uid = ?1 ORDER BY binding_id",
            )
            .unwrap();
        stmt.query_map([space_uid], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }

    fn operations(&self, space_uid: &str) -> Vec<(String, String)> {
        let registry = self.registry();
        let mut stmt = registry
            .raw_connection()
            .prepare(
                "SELECT kind, operation_state FROM operations \
                 WHERE space_uid = ?1 ORDER BY rowid",
            )
            .unwrap();
        stmt.query_map([space_uid], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }
}

/// Exactly one §16.2 document on stdout, and nothing else.
fn document(out: &Output) -> Value {
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        text.trim().lines().count(),
        1,
        "not one document: {text:?} (stderr: {})",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_str(text.trim()).unwrap_or_else(|e| panic!("{e}: {text}"))
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The managed row for Space number `no` in a `ls --format json` document.
fn managed_row(doc: &Value, no: u64) -> Value {
    doc["result"]
        .as_array()
        .unwrap_or_else(|| panic!("no result rows: {doc}"))
        .iter()
        .find(|row| row["managed"] == true && row["space_no"] == no)
        .cloned()
        .unwrap_or_else(|| panic!("no managed row {no} in {doc}"))
}

/// The native ref `ls` prints for the unmanaged resource `token`.
fn unmanaged_ref(doc: &Value, token: &str) -> String {
    let expected = native_ref(Backend::Tmux, token);
    doc["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["managed"] == false && row["native_ref"] == expected)
        .map(|row| row["native_ref"].as_str().unwrap().to_string())
        .unwrap_or_else(|| panic!("no unmanaged row for {token} in {doc}"))
}

// ---------------------------------------------------------------------------
// tmux, against a real scratch server

struct TmuxScratch {
    ns: String,
}

impl TmuxScratch {
    /// A server that keeps running across the tamper: `keep` is never
    /// touched, so killing and re-creating `proj` replaces the session
    /// without replacing the server incarnation.
    fn start(tag: &str) -> TmuxScratch {
        let scratch = TmuxScratch {
            ns: format!("dmux-rebind-{tag}-{}", std::process::id()),
        };
        let out = Command::new("tmux")
            .args(["-L", &scratch.ns, "-f", "/dev/null"])
            .args(["new-session", "-d", "-s", "keep"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        scratch
    }

    fn tmux(&self, args: &[&str]) -> String {
        let out = Command::new("tmux")
            .args(["-L", &self.ns, "-f", "/dev/null"])
            .args(args)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn session_id(&self, name: &str) -> String {
        self.tmux(&["display-message", "-p", "-t", name, "#{session_id}"])
    }

    fn pane_id(&self, name: &str) -> String {
        self.tmux(&["display-message", "-p", "-t", name, "#{pane_id}"])
    }

    fn marker(&self, session: &str, option: &str) -> Option<String> {
        let value = self.tmux(&["show-options", "-t", session, "-qv", option]);
        (!value.is_empty()).then_some(value)
    }

    /// The tamper: the session named `name` is replaced by a fresh one of
    /// the same name, so the immutable id the binding carries no longer
    /// answers. The server keeps its incarnation.
    fn replace_session(&self, name: &str) -> String {
        self.tmux(&["kill-session", "-t", name]);
        self.tmux(&["new-session", "-d", "-s", name]);
        self.session_id(name)
    }
}

impl Drop for TmuxScratch {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.ns, "kill-server"])
            .output();
    }
}

/// A bootstrapped server is the precondition for every managed verb; the
/// binary then resolves the instance through the registry's recorded `-L`
/// namespace and published epoch.
fn bootstrapped(home: &Home, tmux: &TmuxScratch) {
    match tmux_bootstrap(&home.env(), &tmux.ns).unwrap() {
        TmuxBootstrapOutcome::Bootstrapped { .. } => {}
        other => panic!("fresh server must bootstrap: {other:?}"),
    }
}

/// `dmux adopt` through the binary, returning the Space number and uid.
fn adopt(home: &Home, session_id: &str) -> (u64, String) {
    let out = home.json(&["adopt", &native_ref(Backend::Tmux, session_id)]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc = document(&out);
    (
        doc["result"]["space_no"].as_u64().unwrap(),
        doc["result"]["space_uid"].as_str().unwrap().to_string(),
    )
}

/// Case 13, the tamper clause, end to end through the binary: adopt →
/// stamp → healthy → external replacement of the native session → `ls`
/// says `absent` and offers the stranger as unmanaged → `repair rebind`
/// (confirmed) binds exactly it, severs the old binding, journals a
/// completed `rebind`, stamps the markers, lands `unstamped` → the pane's
/// `context stamp` → `healthy`.
#[test]
fn tamper_then_rebind_then_stamp_restores_health_on_tmux() {
    let home = Home::new();
    let tmux = TmuxScratch::start("chain");
    tmux.tmux(&["new-session", "-d", "-s", "proj"]);
    bootstrapped(&home, &tmux);
    let old_id = tmux.session_id("proj");
    let (no, space_uid) = adopt(&home, &old_id);
    let compact = no.to_string();

    // A previously managed, healthy Space: the pane acknowledges.
    let out = home.stamp(&compact, &tmux.pane_id("proj"));
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(document(&out)["result"]["health"], "healthy");
    let doc = document(&home.json(&["ls"]));
    let row = managed_row(&doc, no);
    assert_eq!(row["observation"], "live", "{row}");
    assert_eq!(row["health"], "healthy", "{row}");

    // The tamper. Same server incarnation, same name, new immutable id.
    let new_id = tmux.replace_session("proj");
    assert_ne!(new_id, old_id);
    let doc = document(&home.json(&["ls"]));
    let row = managed_row(&doc, no);
    assert_eq!(row["observation"], "absent", "tampering is explicit: {row}");
    assert_eq!(
        row["health"], "healthy",
        "health is not rewritten by a scan: {row}"
    );
    let stranger = unmanaged_ref(&doc, &new_id);

    // §7.4: JSON without --yes is one confirmation document and no change;
    // a pipe without --yes exits 5 and no change.
    let before = home.revision();
    let out = home.json(&["repair", "rebind", &compact, &stranger]);
    assert_eq!(out.status.code(), Some(5), "{}", stderr(&out));
    let doc = document(&out);
    assert_eq!(doc["action"], "repair_rebind", "{doc}");
    assert_eq!(doc["errors"][0]["code"], "confirmation_required", "{doc}");
    assert_eq!(
        doc["errors"][0]["target"],
        format!("{compact} -> {stranger}"),
        "{doc}"
    );
    let out = home.dmux(&["repair", "rebind", &compact, &stranger]);
    assert_eq!(out.status.code(), Some(5), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("confirmation required"),
        "{}",
        stderr(&out)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    assert_eq!(home.revision(), before);
    assert_eq!(
        home.bindings(&space_uid),
        vec![(old_id.clone(), "current".to_string())]
    );
    assert_eq!(tmux.marker(&new_id, "@dmux_space_uid"), None);

    // The confirmed rebind: both identities in one document.
    let out = home.json(&["repair", "rebind", &compact, &stranger, "--yes"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc = document(&out);
    assert_eq!(doc["ok"], true, "{doc}");
    assert_eq!(doc["action"], "repair_rebind", "{doc}");
    let result = &doc["result"];
    assert_eq!(result["space_uid"], space_uid, "{doc}");
    assert_eq!(result["space_no"], no, "{doc}");
    assert_eq!(result["compact_ref"], compact, "{doc}");
    assert_eq!(result["name"], "proj", "{doc}");
    assert_eq!(result["backend"], "tmux", "{doc}");
    assert_eq!(result["native_ref"], stranger, "{doc}");
    assert_eq!(result["native_token"], new_id, "{doc}");
    assert_eq!(result["severed_native_token"], old_id, "{doc}");
    assert_eq!(result["lifecycle"], "active", "{doc}");
    assert_eq!(result["health"], "unstamped", "{doc}");
    assert_eq!(
        result["pending_stamp_command"],
        format!("dmux context stamp {compact}"),
        "{doc}"
    );
    assert!(
        result["uri"]
            .as_str()
            .unwrap()
            .ends_with(&format!("/spaces/{space_uid}")),
        "{doc}"
    );
    assert!(doc["authority_revision"].as_u64().unwrap() > before);

    // Durable state: old binding severed, new one current, the journal row
    // a completed rebind beside the adoption, markers on the new session.
    assert_eq!(
        home.bindings(&space_uid),
        vec![
            (old_id.clone(), "severed".to_string()),
            (new_id.clone(), "current".to_string())
        ]
    );
    assert_eq!(
        home.operations(&space_uid),
        vec![
            ("adopt".to_string(), "completed".to_string()),
            ("rebind".to_string(), "completed".to_string())
        ]
    );
    assert_eq!(
        tmux.marker(&new_id, "@dmux_space_uid").as_deref(),
        Some(space_uid.as_str())
    );
    assert_eq!(
        tmux.marker(&new_id, "@dmux_space_no").as_deref(),
        Some(compact.as_str())
    );
    let registry = home.registry();
    let space = registry
        .space(dmux::model::SpaceUid(space_uid.parse().unwrap()))
        .unwrap();
    assert_eq!(space.lifecycle, Lifecycle::Active);
    assert_eq!(space.health, Health::Unstamped);
    drop(registry);

    // `ls` agrees: live again, unstamped until the pane acknowledges.
    let doc = document(&home.json(&["ls"]));
    let row = managed_row(&doc, no);
    assert_eq!(row["observation"], "live", "{row}");
    assert_eq!(row["health"], "unstamped", "{row}");
    assert!(
        !doc["result"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["managed"] == false && row["native_ref"] == stranger),
        "the rebound session is no longer offered as unmanaged: {doc}"
    );

    // The Space is not absent any more, so a second rebind of anything is
    // an identity conflict and changes nothing.
    let out = home.json(&["repair", "rebind", &compact, &stranger, "--yes"]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    let doc = document(&out);
    assert_eq!(doc["errors"][0]["code"], "identity_conflict", "{doc}");
    assert!(
        doc["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("is not absent"),
        "{doc}"
    );
    assert_eq!(
        home.operations(&space_uid).len(),
        2,
        "no journal row for a refusal"
    );

    // The pane that survived the tamper acknowledges, and health heals.
    let out = home.stamp(&compact, &tmux.pane_id("proj"));
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc = document(&out);
    assert_eq!(doc["result"]["health"], "healthy", "{doc}");
    assert_eq!(doc["result"]["pending_panes"], 0, "{doc}");
    let row = managed_row(&document(&home.json(&["ls"])), no);
    assert_eq!(row["observation"], "live", "{row}");
    assert_eq!(row["health"], "healthy", "{row}");
}

/// A resource that is some Space's current binding is never stolen by a
/// rebind: case 13's pre-mutation guard, the same one `adopt` has.
#[test]
fn a_resource_bound_to_another_space_is_an_identity_conflict() {
    let home = Home::new();
    let tmux = TmuxScratch::start("bound");
    tmux.tmux(&["new-session", "-d", "-s", "proj"]);
    tmux.tmux(&["new-session", "-d", "-s", "other"]);
    bootstrapped(&home, &tmux);
    let proj_id = tmux.session_id("proj");
    let other_id = tmux.session_id("other");
    let (proj_no, proj_uid) = adopt(&home, &proj_id);
    let (_, other_uid) = adopt(&home, &other_id);
    tmux.replace_session("proj");

    let before = home.revision();
    let out = home.json(&[
        "repair",
        "rebind",
        &proj_no.to_string(),
        &native_ref(Backend::Tmux, &other_id),
        "--yes",
    ]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    let doc = document(&out);
    assert_eq!(doc["errors"][0]["code"], "identity_conflict", "{doc}");
    assert!(
        doc["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("already bound"),
        "{doc}"
    );
    assert_eq!(home.revision(), before);
    assert_eq!(
        home.bindings(&proj_uid),
        vec![(proj_id, "current".to_string())]
    );
    assert_eq!(
        home.bindings(&other_uid),
        vec![(other_id.clone(), "current".to_string())]
    );
    assert_eq!(
        tmux.marker(&other_id, "@dmux_space_uid").as_deref(),
        Some(other_uid.as_str()),
        "the other Space's session keeps its own markers"
    );
    assert_eq!(
        home.operations(&proj_uid).len(),
        1,
        "no journal row for a refusal"
    );
}

/// A stranger already advertising foreign dmux identity is a conflict, not
/// a rebind: its markers are never overwritten (plan §10.3, case 13).
#[test]
fn a_session_carrying_foreign_markers_is_refused_untouched() {
    let home = Home::new();
    let tmux = TmuxScratch::start("foreign");
    tmux.tmux(&["new-session", "-d", "-s", "proj"]);
    bootstrapped(&home, &tmux);
    let (no, space_uid) = adopt(&home, &tmux.session_id("proj"));
    let new_id = tmux.replace_session("proj");
    let foreign = [
        ("@dmux_host_uid", "11111111-1111-4111-8111-111111111111"),
        ("@dmux_registry_uid", "22222222-2222-4222-8222-222222222222"),
        ("@dmux_space_uid", "33333333-3333-4333-8333-333333333333"),
        ("@dmux_space_no", "99"),
    ];
    for (option, value) in foreign {
        tmux.tmux(&["set-option", "-t", &new_id, option, value]);
    }

    let before = home.revision();
    let out = home.json(&[
        "repair",
        "rebind",
        &no.to_string(),
        &native_ref(Backend::Tmux, &new_id),
        "--yes",
    ]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    assert_eq!(document(&out)["errors"][0]["code"], "identity_conflict");
    for (option, value) in foreign {
        assert_eq!(
            tmux.marker(&new_id, option).as_deref(),
            Some(value),
            "{option} was overwritten"
        );
    }
    assert_eq!(home.revision(), before);
    assert_eq!(home.operations(&space_uid).len(), 1);
}

/// The refusals that precede confirmation: none of them needs `--yes` to be
/// reached, so each exits with its own code rather than 5, and each is one
/// document (case 43).
#[test]
fn malformed_mismatched_and_unknown_targets_refuse_before_confirmation() {
    let home = Home::new();
    let tmux = TmuxScratch::start("refuse");
    tmux.tmux(&["new-session", "-d", "-s", "proj"]);
    bootstrapped(&home, &tmux);
    let (no, space_uid) = adopt(&home, &tmux.session_id("proj"));
    let compact = no.to_string();
    let before = home.revision();

    // Not a native ref: `adopt` accepts no backend command string (§7.4),
    // and neither does rebind.
    for malformed in ["$5", "proj", "native:tmux", "native:zellij:JDU"] {
        let out = home.json(&["repair", "rebind", &compact, malformed]);
        assert_eq!(out.status.code(), Some(2), "{malformed}: {}", stderr(&out));
        let doc = document(&out);
        assert_eq!(doc["action"], "repair_rebind", "{doc}");
        assert_eq!(
            doc["errors"][0]["code"], "invalid_ref",
            "{malformed}: {doc}"
        );
    }
    // The wrong backend's resource can never become a tmux Space's binding.
    let out = home.json(&[
        "repair",
        "rebind",
        &compact,
        &native_ref(Backend::Wez, "alpha"),
    ]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    let doc = document(&out);
    assert_eq!(doc["errors"][0]["code"], "backend_mismatch", "{doc}");
    // A Space that does not exist.
    let out = home.json(&[
        "repair",
        "rebind",
        "nosuchspace",
        &native_ref(Backend::Tmux, "$9"),
    ]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert_eq!(document(&out)["errors"][0]["code"], "not_found");
    // A child ref names a live Group/Split, not a Space.
    let out = home.json(&[
        "repair",
        "rebind",
        "proj/g00000000-0000-4000-8000-000000000001.tx-1",
        &native_ref(Backend::Tmux, "$9"),
    ]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert_eq!(document(&out)["errors"][0]["code"], "usage");
    // A resource that is not in the scan, after confirmation: not-found,
    // and no journal row was opened for it.
    let out = home.json(&[
        "repair",
        "rebind",
        &compact,
        &native_ref(Backend::Tmux, "$999"),
        "--yes",
    ]);
    assert_eq!(out.status.code(), Some(4), "{}", stderr(&out));
    assert_eq!(
        document(&out)["errors"][0]["code"],
        "identity_conflict",
        "the Space is live, which is checked before the stranger is looked for"
    );

    assert_eq!(home.revision(), before);
    assert_eq!(home.operations(&space_uid).len(), 1);
}

/// Rebind is owner-local (ADR 011 D7): a Space named on another host is a
/// typed protocol answer, never a local act, and nothing is resolved.
#[test]
fn a_remote_space_is_refused_as_protocol_mismatch() {
    let home = Home::new();
    let peer = home
        .registry()
        .enroll_host(dmux::model::HostUid(Uuid::new_v4()), Some("archie"))
        .unwrap();
    let before = home.revision();

    for spelling in [
        format!("{}:1", peer.alias),
        "archie:proj".to_string(),
        format!("{}:7", peer.host_uid.0),
        format!("dmux://{}/spaces/{}", peer.host_uid.0, Uuid::now_v7()),
    ] {
        let out = home.json(&[
            "repair",
            "rebind",
            &spelling,
            &native_ref(Backend::Tmux, "$1"),
        ]);
        assert_eq!(out.status.code(), Some(6), "{spelling}: {}", stderr(&out));
        let doc = document(&out);
        assert_eq!(
            doc["errors"][0]["code"], "protocol_mismatch",
            "{spelling}: {doc}"
        );
        assert!(
            doc["errors"][0]["message"]
                .as_str()
                .unwrap()
                .contains("owner-local"),
            "{doc}"
        );
    }
    assert_eq!(home.revision(), before);
}

/// A Space whose instance has published no epoch cannot be rebound: nothing
/// about the live server can be verified, so the verb refuses before any
/// provider exists (`backend_epoch_changed`, the one text every verb uses).
#[test]
fn an_unpublished_instance_is_refused_before_any_scan() {
    let home = Home::new();
    let (no, space_uid) = {
        let mut registry = home.registry();
        let instance = registry
            .register_backend_instance(Backend::Tmux, Some("dmux-rebind-unpublished"), None)
            .unwrap();
        let reservation = registry
            .reserve_space("proj", instance, Uuid::new_v4())
            .unwrap();
        registry
            .finalize_create(
                reservation.space_uid,
                reservation.operation_uid,
                &NativeBindingSpec {
                    native_token: "$9".into(),
                    native_kind: NativeKind::TmuxSessionId,
                    server_epoch: None,
                },
            )
            .unwrap();
        (
            reservation.space_no.get(),
            reservation.space_uid.0.to_string(),
        )
    };
    let before = home.revision();
    let out = home.json(&[
        "repair",
        "rebind",
        &no.to_string(),
        &native_ref(Backend::Tmux, "$10"),
        "--yes",
    ]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let doc = document(&out);
    assert_eq!(doc["errors"][0]["code"], "backend_epoch_changed", "{doc}");
    assert!(
        doc["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("has published no server epoch"),
        "{doc}"
    );
    assert_eq!(home.revision(), before);
    assert_eq!(
        home.bindings(&space_uid),
        vec![("$9".to_string(), "current".to_string())]
    );
}

// ---------------------------------------------------------------------------
// Wez, at the operations layer against the scripted mux

struct WezHome {
    home: Home,
    instance: BackendInstanceUid,
    owner: dmux::model::HostUid,
}

impl WezHome {
    /// The managed Wez instance published as the fake mux's epoch, plus one
    /// healthy Space bound to its opaque key.
    fn new(mux: &FakeMux) -> (WezHome, SpaceReservation, String) {
        let home = Home::new();
        let mut registry = home.registry();
        let owner = registry.identity().unwrap().host_uid;
        let instance = registry
            .register_backend_instance(Backend::Wez, Some("/run/dmux/wez.sock"), None)
            .unwrap();
        registry
            .publish_backend_server(
                instance,
                ServerEpoch(mux.epoch),
                Some(4242),
                Some("start-token"),
                None,
                None,
            )
            .unwrap();
        let reservation = registry
            .reserve_space("alpha", instance, Uuid::new_v4())
            .unwrap();
        let key = format!("dmux:{}:{}", owner.0, reservation.space_uid.0);
        registry
            .finalize_create(
                reservation.space_uid,
                reservation.operation_uid,
                &NativeBindingSpec {
                    native_token: key.clone(),
                    native_kind: NativeKind::WezWorkspaceKey,
                    server_epoch: Some(ServerEpoch(mux.epoch)),
                },
            )
            .unwrap();
        drop(registry);
        (
            WezHome {
                home,
                instance,
                owner,
            },
            reservation,
            key,
        )
    }

    fn scope(&self, mux: &FakeMux) -> InventoryScope {
        InventoryScope::managed(Backend::Wez, "/run/dmux/wez.sock", ServerEpoch(mux.epoch))
    }

    fn provider<'a>(&self, mux: &'a FakeMux) -> WezProvider<&'a FakeMux> {
        WezProvider::with_runner("/opt/homebrew/bin/wezterm", "/etc/dmux/wez.lua", mux)
    }
}

/// The Wez leg of the primitive: the stranger workspace is CAS-renamed to
/// the Space's own opaque key under `--if-workspace`/`--if-sole-window`,
/// the old binding is severed, the journal carries a completed `rebind`,
/// and the Space lands unstamped.
#[test]
fn wez_rebind_cas_renames_the_stranger_to_the_opaque_key_and_lands_unstamped() {
    // The tamper already happened: the opaque key answers nowhere, a
    // two-tab single-window workspace `alpha2` is what is left.
    let mux = FakeMux::new(Cas::Fork, &[(1, 10, 100, "alpha2"), (1, 11, 101, "alpha2")]);
    let (wez, reservation, key) = WezHome::new(&mux);
    let space_uid = reservation.space_uid;
    let before = wez.home.revision();

    let rebound = rebind_wez(
        &wez.home.env(),
        &wez.provider(&mux),
        &wez.scope(&mux),
        space_uid,
        "alpha2",
        Uuid::new_v4(),
    )
    .expect("rebind");
    assert_eq!(rebound.space_uid, space_uid);
    assert_eq!(rebound.name, "alpha");
    assert_eq!(rebound.backend, Backend::Wez);
    assert_eq!(rebound.severed_native_token, key);
    assert_eq!(
        rebound.native_token, key,
        "the key is the Space's own, not a new one"
    );
    assert_eq!(rebound.server_epoch, ServerEpoch(mux.epoch));

    // Exactly one real CAS, guarded the way adoption guards it; both tabs
    // moved together and nothing else was renamed.
    let cas = mux.cas_calls();
    assert_eq!(cas.len(), 1, "{cas:?}");
    let argv = &cas[0];
    assert!(argv.iter().any(|a| a == "rename-workspace"), "{argv:?}");
    let after = |flag: &str| {
        argv.iter()
            .position(|a| a == flag)
            .map(|i| argv[i + 1].as_str())
    };
    assert_eq!(after("--window-id"), Some("1"), "{argv:?}");
    assert_eq!(after("--if-workspace"), Some("alpha2"), "{argv:?}");
    assert!(argv.iter().any(|a| a == "--if-sole-window"), "{argv:?}");
    assert_eq!(
        argv.last().map(String::as_str),
        Some(key.as_str()),
        "{argv:?}"
    );
    assert_eq!(mux.workspaces(), vec![key.clone()]);

    let registry = wez.home.registry();
    let row = registry.space(space_uid).unwrap();
    assert_eq!(row.lifecycle, Lifecycle::Active);
    assert_eq!(row.health, Health::Unstamped);
    let binding = registry.current_binding(space_uid).unwrap().unwrap();
    assert_eq!(binding.native_token, key);
    assert_eq!(binding.native_kind, NativeKind::WezWorkspaceKey);
    assert_eq!(binding.binding_state, BindingState::Current);
    assert_eq!(
        registry.current_binding_epoch(space_uid).unwrap(),
        Some(ServerEpoch(mux.epoch))
    );
    assert_eq!(
        registry
            .current_binding_by_native(wez.instance, &key)
            .unwrap()
            .map(|b| b.space_uid),
        Some(space_uid)
    );
    assert!(registry.authority_head().unwrap().revision > before);
    drop(registry);
    assert_eq!(
        wez.home.bindings(&space_uid.0.to_string()),
        vec![
            (key.clone(), "severed".to_string()),
            (key, "current".to_string())
        ]
    );
    assert_eq!(
        wez.home.operations(&space_uid.0.to_string()),
        vec![
            ("create".to_string(), "completed".to_string()),
            ("rebind".to_string(), "completed".to_string())
        ]
    );
    let _ = wez.owner;
}

/// A CAS that loses its race is a typed conflict with zero mutation: the
/// journal row is aborted, the binding and health are untouched, and no
/// second rename is attempted. Never a silent rebind (case 13).
#[test]
fn wez_rebind_cas_mismatch_is_a_typed_conflict_never_a_silent_rebind() {
    let mux = FakeMux::new(Cas::Fork, &[(1, 10, 100, "alpha2")]);
    let (wez, reservation, key) = WezHome::new(&mux);
    let space_uid = reservation.space_uid;
    mux.race_to("renamed-by-someone-else");

    let err = rebind_wez(
        &wez.home.env(),
        &wez.provider(&mux),
        &wez.scope(&mux),
        space_uid,
        "alpha2",
        Uuid::new_v4(),
    )
    .unwrap_err();
    assert!(
        matches!(&err, OpError::NameConflict(detail) if detail.contains("lost its race")),
        "{err}"
    );
    assert_eq!(
        mux.cas_calls().len(),
        1,
        "one attempt, no retry: {:?}",
        mux.commands()
    );
    assert_eq!(mux.workspaces(), vec!["alpha2".to_string()]);

    let registry = wez.home.registry();
    let row = registry.space(space_uid).unwrap();
    assert_eq!(
        row.health,
        Health::Healthy,
        "health is untouched by a refused rebind"
    );
    let binding = registry.current_binding(space_uid).unwrap().unwrap();
    assert_eq!(binding.native_token, key);
    drop(registry);
    assert_eq!(
        wez.home.bindings(&space_uid.0.to_string()),
        vec![(key, "current".to_string())]
    );
    assert_eq!(
        wez.home.operations(&space_uid.0.to_string()),
        vec![
            ("create".to_string(), "completed".to_string()),
            ("rebind".to_string(), "aborted".to_string())
        ],
        "the intent was journaled before the CAS and closed as aborted after it"
    );
    // The row is closed, so the Space is not stuck: a second attempt opens
    // a fresh row rather than answering operation_in_progress.
    let registry = wez.home.registry();
    assert!(registry.unfinished_operation(space_uid).unwrap().is_none());
}

/// Without the fork CAS verb nothing is journaled and nothing moves (§2.7).
#[test]
fn wez_rebind_without_the_fork_cas_verb_changes_nothing() {
    let mux = FakeMux::new(Cas::Stock, &[(1, 10, 100, "alpha2")]);
    let (wez, reservation, key) = WezHome::new(&mux);
    let space_uid = reservation.space_uid;
    let before = wez.home.revision();

    let err = rebind_wez(
        &wez.home.env(),
        &wez.provider(&mux),
        &wez.scope(&mux),
        space_uid,
        "alpha2",
        Uuid::new_v4(),
    )
    .unwrap_err();
    assert!(
        matches!(&err, OpError::Provider(detail) if detail.contains("cas_capability_missing")),
        "{err}"
    );
    assert_eq!(mux.workspaces(), vec!["alpha2".to_string()]);
    assert_eq!(wez.home.revision(), before);
    assert_eq!(
        wez.home.bindings(&space_uid.0.to_string()),
        vec![(key, "current".to_string())]
    );
    assert_eq!(wez.home.operations(&space_uid.0.to_string()).len(), 1);
}

/// A Space whose opaque key still answers is not absent: the verb refuses
/// as an identity conflict before any journal row or CAS, and names the
/// live binding so the operator reaches for rename/remove instead.
#[test]
fn wez_rebind_refuses_while_the_opaque_key_still_answers() {
    let mux = FakeMux::new(Cas::Fork, &[(1, 10, 100, "alpha2")]);
    let (wez, reservation, key) = WezHome::new(&mux);
    let space_uid = reservation.space_uid;
    // The Space's own workspace is alive on window 2.
    mux.add_pane(2, 20, 200, &key);
    let before = wez.home.revision();

    let err = rebind_wez(
        &wez.home.env(),
        &wez.provider(&mux),
        &wez.scope(&mux),
        space_uid,
        "alpha2",
        Uuid::new_v4(),
    )
    .unwrap_err();
    assert!(
        matches!(&err, OpError::NameConflict(detail)
            if detail.starts_with(dmux::operations::ADOPT_IDENTITY_CONFLICT)
                && detail.contains("is not absent")),
        "{err}"
    );
    assert!(mux.cas_calls().is_empty(), "{:?}", mux.commands());
    assert_eq!(wez.home.revision(), before);
    assert_eq!(wez.home.operations(&space_uid.0.to_string()).len(), 1);
    assert_eq!(
        wez.home.bindings(&space_uid.0.to_string()),
        vec![(key, "current".to_string())]
    );
}

/// The registry half on its own: `begin_rebind` refuses anything but an
/// active Space and a free journal slot; `finalize_rebind` is one
/// transaction that severs, binds, drops health to unstamped and advances
/// the chain; `abort_rebind` closes the row and changes nothing else.
#[test]
fn the_registry_rebind_journal_is_two_phase_and_typed() {
    let home = Home::new();
    let mut registry = home.registry();
    let instance = registry
        .register_backend_instance(Backend::Tmux, Some("dmux-rebind-journal"), None)
        .unwrap();
    let reservation = registry
        .reserve_space("proj", instance, Uuid::new_v4())
        .unwrap();
    let space_uid = reservation.space_uid;

    // Not active yet: a reservation cannot be rebound.
    let err = registry
        .begin_rebind(space_uid, Uuid::new_v4(), "$2", "$2")
        .unwrap_err();
    assert!(
        matches!(err, dmux::registry::RegistryError::NotFound { .. }),
        "{err}"
    );
    registry
        .finalize_create(
            space_uid,
            reservation.operation_uid,
            &NativeBindingSpec {
                native_token: "$1".into(),
                native_kind: NativeKind::TmuxSessionId,
                server_epoch: None,
            },
        )
        .unwrap();

    let head = registry.authority_head().unwrap().revision;
    let op = registry
        .begin_rebind(space_uid, Uuid::new_v4(), "$2", "$2")
        .unwrap();
    let row = registry.operation(op).unwrap();
    assert_eq!(row.kind, OperationKind::Rebind);
    assert_eq!(row.state, dmux::model::OperationState::Prepared);
    let payload: Value = serde_json::from_str(&row.payload_json).unwrap();
    assert_eq!(payload["source_native_token"], "$2");
    assert_eq!(payload["destination_native_token"], "$2");
    // One unfinished operation per Space.
    let err = registry
        .begin_rebind(space_uid, Uuid::new_v4(), "$3", "$3")
        .unwrap_err();
    assert!(
        matches!(
            err,
            dmux::registry::RegistryError::OperationInProgress { .. }
        ),
        "{err}"
    );
    // Abort: row closed, binding and health untouched, no revision.
    let after_begin = registry.authority_head().unwrap().revision;
    registry.abort_rebind(space_uid, op).unwrap();
    assert_eq!(
        registry.operation(op).unwrap().state,
        dmux::model::OperationState::Aborted
    );
    assert_eq!(registry.authority_head().unwrap().revision, after_begin);
    assert_eq!(registry.space(space_uid).unwrap().health, Health::Healthy);
    assert_eq!(
        registry
            .current_binding(space_uid)
            .unwrap()
            .unwrap()
            .native_token,
        "$1"
    );
    // A finished row cannot be finalized.
    let err = registry
        .finalize_rebind(
            space_uid,
            op,
            &NativeBindingSpec {
                native_token: "$2".into(),
                native_kind: NativeKind::TmuxSessionId,
                server_epoch: None,
            },
        )
        .unwrap_err();
    assert!(
        matches!(err, dmux::registry::RegistryError::Corrupt(_)),
        "{err}"
    );

    // Finalize: sever, bind, unstamped, chain advanced, row completed.
    let op = registry
        .begin_rebind(space_uid, Uuid::new_v4(), "$2", "$2")
        .unwrap();
    let epoch = ServerEpoch(Uuid::new_v4());
    registry
        .finalize_rebind(
            space_uid,
            op,
            &NativeBindingSpec {
                native_token: "$2".into(),
                native_kind: NativeKind::TmuxSessionId,
                server_epoch: Some(epoch),
            },
        )
        .unwrap();
    assert_eq!(registry.space(space_uid).unwrap().health, Health::Unstamped);
    let binding = registry.current_binding(space_uid).unwrap().unwrap();
    assert_eq!(binding.native_token, "$2");
    assert_eq!(
        registry.current_binding_epoch(space_uid).unwrap(),
        Some(epoch)
    );
    assert!(registry.authority_head().unwrap().revision > head + 1);
    assert_eq!(
        registry.operation(op).unwrap().state,
        dmux::model::OperationState::Completed
    );
    assert!(registry.unfinished_operation(space_uid).unwrap().is_none());
    drop(registry);
    assert_eq!(
        home.bindings(&space_uid.0.to_string()),
        vec![
            ("$1".to_string(), "severed".to_string()),
            ("$2".to_string(), "current".to_string())
        ]
    );
    // The wrong kind is typed: a create row cannot be finalized as a rebind.
    let mut registry = home.registry();
    let other = registry
        .reserve_space("other", instance, Uuid::new_v4())
        .unwrap();
    let err = registry
        .finalize_rebind(
            other.space_uid,
            other.operation_uid,
            &NativeBindingSpec {
                native_token: "$5".into(),
                native_kind: NativeKind::TmuxSessionId,
                server_epoch: None,
            },
        )
        .unwrap_err();
    assert!(
        matches!(err, dmux::registry::RegistryError::KindNotAllowed { .. }),
        "{err}"
    );
}

// ---------------------------------------------------------------------------
// A crashed rebind (ADR 012 WS-D.2; plan §10.3 "crash reconciliation by
// source/destination/epoch yields unmanaged, active-unstamped, healthy, or
// conflict — never silent success"). The journal row is the operator's
// confirmed assertion; reconciliation reads the evidence and settles it
// into rolled back, committed, or conflict — writing nothing native.

use dmux::backend::tmux::{SpaceMarkers, TmuxProvider};
use dmux::operations::{ReconcileBackend, ReconcileOutcome, reconcile_apply, reconcile_scan};

fn stranded_rebind(home: &Home, space_uid: &str, source: &str, destination: &str) -> Uuid {
    home.registry()
        .begin_rebind(
            dmux::model::SpaceUid(space_uid.parse().unwrap()),
            Uuid::new_v4(),
            source,
            destination,
        )
        .unwrap()
}

fn sole_target(home: &Home) -> dmux::operations::ReconcileTarget {
    let targets = reconcile_scan(&home.env()).unwrap();
    assert_eq!(targets.len(), 1, "{targets:?}");
    assert_eq!(targets[0].kind, OperationKind::Rebind);
    assert_eq!(targets[0].duty, "adoption_reconcile");
    assert!(!targets[0].in_flight);
    targets.into_iter().next().unwrap()
}

fn op_state(home: &Home, operation_uid: Uuid) -> dmux::model::OperationState {
    home.registry().operation(operation_uid).unwrap().state
}

/// tmux: the stamp is the native step. No markers on the named session
/// means it never ran (rolled back, the Space stays absent); this Space's
/// markers mean it landed (committed: bound, unstamped); a stranger's
/// markers are a conflict. Without a marker reader the verdict is
/// refused, not guessed.
#[test]
fn a_crashed_tmux_rebind_is_settled_by_the_markers_on_the_named_session() {
    let home = Home::new();
    let tmux = TmuxScratch::start("crash");
    tmux.tmux(&["new-session", "-d", "-s", "proj"]);
    let epoch = match tmux_bootstrap(&home.env(), &tmux.ns).unwrap() {
        TmuxBootstrapOutcome::Bootstrapped { epoch } => epoch,
        other => panic!("fresh server must bootstrap: {other:?}"),
    };
    let old_id = tmux.session_id("proj");
    let (_, space_uid) = adopt(&home, &old_id);
    let new_id = tmux.replace_session("proj");
    let provider = TmuxProvider::new(tmux.ns.clone());
    let scope = InventoryScope::managed(Backend::Tmux, tmux.ns.clone(), epoch);

    // Died before the stamp: rolled back.
    let op = stranded_rebind(&home, &space_uid, &new_id, &new_id);
    let result = reconcile_apply(
        &home.env(),
        &sole_target(&home),
        Some(ReconcileBackend::scan_only(&provider, &scope).with_markers(&provider)),
    );
    assert_eq!(
        result.outcome,
        ReconcileOutcome::RebindRolledBack,
        "{result:?}"
    );
    assert!(result.ok);
    assert!(result.detail.contains("stamp never landed"), "{result:?}");
    assert_eq!(op_state(&home, op), dmux::model::OperationState::Aborted);
    assert_eq!(
        home.bindings(&space_uid),
        vec![(old_id.clone(), "current".to_string())]
    );
    assert_eq!(tmux.marker(&new_id, "@dmux_space_uid"), None);

    // Died after the stamp, before the binding: committed.
    let identity = home.registry().identity().unwrap();
    let space_no = home
        .registry()
        .space(dmux::model::SpaceUid(space_uid.parse().unwrap()))
        .unwrap()
        .space_no;
    provider
        .stamp_markers(
            &scope,
            &new_id,
            &SpaceMarkers {
                host_uid: identity.host_uid.0.to_string(),
                registry_uid: identity.registry_uid.0.to_string(),
                space_uid: space_uid.clone(),
                space_no: space_no.to_string(),
            },
        )
        .unwrap();
    let op = stranded_rebind(&home, &space_uid, &new_id, &new_id);

    // Handed over without its marker reader, the verdict is refused.
    let blind = reconcile_apply(
        &home.env(),
        &sole_target(&home),
        Some(ReconcileBackend::scan_only(&provider, &scope)),
    );
    assert_eq!(blind.outcome, ReconcileOutcome::FailedClosed, "{blind:?}");
    assert!(blind.detail.contains("no marker reader"), "{blind:?}");
    assert_eq!(op_state(&home, op), dmux::model::OperationState::Prepared);

    let result = reconcile_apply(
        &home.env(),
        &sole_target(&home),
        Some(ReconcileBackend::scan_only(&provider, &scope).with_markers(&provider)),
    );
    assert_eq!(
        result.outcome,
        ReconcileOutcome::RebindCommitted,
        "{result:?}"
    );
    assert!(result.ok);
    assert!(result.detail.starts_with("active-unstamped"), "{result:?}");
    assert_eq!(op_state(&home, op), dmux::model::OperationState::Completed);
    assert_eq!(
        home.bindings(&space_uid),
        vec![
            (old_id.clone(), "severed".to_string()),
            (new_id.clone(), "current".to_string())
        ]
    );
    let registry = home.registry();
    let space = registry
        .space(dmux::model::SpaceUid(space_uid.parse().unwrap()))
        .unwrap();
    assert_eq!(space.health, Health::Unstamped);
    assert_eq!(
        registry.current_binding_epoch(space.space_uid).unwrap(),
        Some(epoch)
    );
    drop(registry);
    // Idempotent: nothing is left to reconcile, and the Space is listed
    // live again.
    assert!(reconcile_scan(&home.env()).unwrap().is_empty());
    let row = managed_row(&document(&home.json(&["ls"])), space_no.get());
    assert_eq!(row["observation"], "live", "{row}");
    assert_eq!(row["health"], "unstamped", "{row}");

    // A stranger's markers on the named session: conflict, nothing bound.
    let third_id = tmux.replace_session("proj");
    for (option, value) in [
        ("@dmux_host_uid", "11111111-1111-4111-8111-111111111111"),
        ("@dmux_registry_uid", "22222222-2222-4222-8222-222222222222"),
        ("@dmux_space_uid", "33333333-3333-4333-8333-333333333333"),
        ("@dmux_space_no", "99"),
    ] {
        tmux.tmux(&["set-option", "-t", &third_id, option, value]);
    }
    let op = stranded_rebind(&home, &space_uid, &third_id, &third_id);
    let result = reconcile_apply(
        &home.env(),
        &sole_target(&home),
        Some(ReconcileBackend::scan_only(&provider, &scope).with_markers(&provider)),
    );
    assert_eq!(result.outcome, ReconcileOutcome::FailedClosed, "{result:?}");
    assert!(result.detail.contains("another identity"), "{result:?}");
    assert_eq!(op_state(&home, op), dmux::model::OperationState::Prepared);
    assert_eq!(
        home.registry()
            .current_binding(dmux::model::SpaceUid(space_uid.parse().unwrap()))
            .unwrap()
            .unwrap()
            .native_token,
        new_id
    );

    // The named session is gone altogether: rolled back.
    tmux.tmux(&["kill-session", "-t", "proj"]);
    let result = reconcile_apply(
        &home.env(),
        &sole_target(&home),
        Some(ReconcileBackend::scan_only(&provider, &scope).with_markers(&provider)),
    );
    assert_eq!(
        result.outcome,
        ReconcileOutcome::RebindRolledBack,
        "{result:?}"
    );
    assert!(result.detail.contains("no longer answers"), "{result:?}");
    assert_eq!(op_state(&home, op), dmux::model::OperationState::Aborted);
}

/// Wez: the CAS rename is the native step, settled by which of source and
/// destination answer under the published epoch — old-only, new-only,
/// both, neither — exactly the rename table's four outcomes, with both and
/// neither refused.
#[test]
fn a_crashed_wez_rebind_is_settled_by_source_and_destination_under_the_epoch() {
    // Old-only: the rename never landed.
    {
        let mux = FakeMux::new(Cas::Fork, &[(1, 10, 100, "alpha2")]);
        let (wez, reservation, key) = WezHome::new(&mux);
        let space_uid = reservation.space_uid.0.to_string();
        let op = stranded_rebind(&wez.home, &space_uid, "alpha2", &key);
        let provider = wez.provider(&mux);
        let scope = wez.scope(&mux);
        let result = reconcile_apply(
            &wez.home.env(),
            &sole_target(&wez.home),
            Some(ReconcileBackend::restorable(&provider, &scope, &provider)),
        );
        assert_eq!(
            result.outcome,
            ReconcileOutcome::RebindRolledBack,
            "{result:?}"
        );
        assert!(result.detail.contains("never landed"), "{result:?}");
        assert_eq!(
            op_state(&wez.home, op),
            dmux::model::OperationState::Aborted
        );
        assert_eq!(
            wez.home.bindings(&space_uid),
            vec![(key, "current".to_string())]
        );
        assert!(
            mux.cas_calls().is_empty(),
            "nothing native is written: {:?}",
            mux.commands()
        );
        assert_eq!(
            wez.home
                .registry()
                .space(reservation.space_uid)
                .unwrap()
                .health,
            Health::Healthy
        );
    }
    // New-only: the rename landed; the registry half is completed.
    {
        let mux = FakeMux::new(Cas::Fork, &[]);
        let (wez, reservation, key) = WezHome::new(&mux);
        mux.add_pane(1, 10, 100, &key);
        let space_uid = reservation.space_uid.0.to_string();
        let op = stranded_rebind(&wez.home, &space_uid, "alpha2", &key);
        let provider = wez.provider(&mux);
        let scope = wez.scope(&mux);
        let result = reconcile_apply(
            &wez.home.env(),
            &sole_target(&wez.home),
            Some(ReconcileBackend::restorable(&provider, &scope, &provider)),
        );
        assert_eq!(
            result.outcome,
            ReconcileOutcome::RebindCommitted,
            "{result:?}"
        );
        assert_eq!(
            op_state(&wez.home, op),
            dmux::model::OperationState::Completed
        );
        assert_eq!(
            wez.home.bindings(&space_uid),
            vec![
                (key.clone(), "severed".to_string()),
                (key, "current".to_string())
            ]
        );
        let registry = wez.home.registry();
        assert_eq!(
            registry.space(reservation.space_uid).unwrap().health,
            Health::Unstamped
        );
        assert_eq!(
            registry
                .current_binding_epoch(reservation.space_uid)
                .unwrap(),
            Some(ServerEpoch(mux.epoch))
        );
        assert!(mux.cas_calls().is_empty(), "{:?}", mux.commands());
    }
    // Both and neither: conflict, the row stays open, nothing moves.
    for (panes, expect) in [(vec!["alpha2", "KEY"], "both"), (vec![], "neither")] {
        let mux = FakeMux::new(Cas::Fork, &[]);
        let (wez, reservation, key) = WezHome::new(&mux);
        for (i, name) in panes.iter().enumerate() {
            let name = if *name == "KEY" { key.as_str() } else { name };
            mux.add_pane(i as u64 + 1, i as u64 + 10, i as u64 + 100, name);
        }
        let space_uid = reservation.space_uid.0.to_string();
        let op = stranded_rebind(&wez.home, &space_uid, "alpha2", &key);
        let provider = wez.provider(&mux);
        let scope = wez.scope(&mux);
        let result = reconcile_apply(
            &wez.home.env(),
            &sole_target(&wez.home),
            Some(ReconcileBackend::restorable(&provider, &scope, &provider)),
        );
        assert_eq!(
            result.outcome,
            ReconcileOutcome::FailedClosed,
            "{expect}: {result:?}"
        );
        assert!(!result.ok);
        assert!(result.detail.contains(expect), "{expect}: {result:?}");
        assert_eq!(
            op_state(&wez.home, op),
            dmux::model::OperationState::Prepared
        );
        assert_eq!(
            wez.home.bindings(&space_uid),
            vec![(key, "current".to_string())],
            "{expect}"
        );
        assert!(mux.cas_calls().is_empty(), "{expect}: {:?}", mux.commands());
    }
}

/// `--host` reaches the handler: an enrolled peer named on the global flag
/// is the same owner-local refusal as a host in the ref (ADR 011 D7), and
/// nothing is touched locally.
#[test]
fn a_remote_host_flag_is_refused_as_protocol_mismatch_too() {
    let home = Home::new();
    home.registry()
        .enroll_host(dmux::model::HostUid(Uuid::new_v4()), Some("archie"))
        .unwrap();
    let before = home.revision();
    let out = home.json(&[
        "--host",
        "archie",
        "repair",
        "rebind",
        "1",
        &native_ref(Backend::Tmux, "$1"),
    ]);
    assert_eq!(out.status.code(), Some(6), "{}", stderr(&out));
    let doc = document(&out);
    assert_eq!(doc["errors"][0]["code"], "protocol_mismatch", "{doc}");
    assert_eq!(
        home.revision(),
        before,
        "a refused remote rebind writes nothing"
    );
}
