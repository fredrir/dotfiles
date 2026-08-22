//! P6 CLI gate: `dmux adopt NATIVE_REF` (plan §10.3, §7.4; case 13).
//!
//! `tests/adopt_flow.rs` proves the fenced operations against real servers.
//! This file proves the verb wrapped around them: that the opaque ref is
//! decoded and nothing else, that the receipt admits the Space is still
//! `unstamped`, and that the three refusals §10.3 gives distinct remedies
//! arrive as distinct exit statuses instead of a generic backend failure.
//!
//! tmux runs against a real scratch server. Wez runs against a fake mux —
//! the fork CAS build is not a test dependency here, and the capability
//! refusal is precisely what must be provable without it.

use std::cell::RefCell;
use std::process::Command;
use std::time::Duration;

use dmux::adopt_cli::{AdoptArgs, AdoptOutput, WezCli, adopt_in};
use dmux::backend::wez::{
    CAS_FAILED_MARKER, CAS_MISSING_PDU_STDERR, ProbeOutcome, RunError, RunOutput, WezInvocation,
    WezRunner,
};
use dmux::error::ExitStatus;
use dmux::model::{Backend, BackendInstanceUid, Health, Lifecycle, ServerEpoch};
use dmux::operations::{OperationEnv, tmux_bootstrap};
use dmux::output::{OutputFormat, native_ref};
use dmux::registry::{NativeBindingSpec, NativeKind, Registry, RegistryConfig};
use uuid::Uuid;

struct Scratch {
    data: tempfile::TempDir,
    locks: tempfile::TempDir,
}

impl Scratch {
    fn new() -> Scratch {
        Scratch {
            data: tempfile::tempdir().unwrap(),
            locks: tempfile::tempdir().unwrap(),
        }
    }

    fn env(&self) -> OperationEnv {
        OperationEnv {
            db_path: self.data.path().join("registry.sqlite3"),
            lock_dir: self.locks.path().to_path_buf(),
        }
    }

    fn registry(&self) -> Registry {
        let env = self.env();
        Registry::open(RegistryConfig::new(&env.db_path, &env.lock_dir)).unwrap()
    }

    fn space_count(&self) -> usize {
        self.registry().spaces().unwrap().len()
    }

    fn revision(&self) -> u64 {
        self.registry().authority_head().unwrap().revision
    }

    /// Every row of one registry table, whatever its state: a reservation
    /// that was aborted still counts, because it was written.
    fn table_rows(&self, table: &str) -> i64 {
        self.registry()
            .raw_connection()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }
}

/// Wired into every tmux case: adoption of one backend must never reach the
/// other provider.
struct NeverWez;

impl WezRunner for NeverWez {
    fn probe(&self, socket: &str, _pid: Option<u32>) -> ProbeOutcome {
        panic!("tmux adoption probed the wez socket {socket}")
    }
    fn run(&self, invocation: &WezInvocation, _: Duration) -> Result<RunOutput, RunError> {
        panic!("tmux adoption ran wezterm: {:?}", invocation.argv)
    }
}

fn never_wez() -> WezCli<NeverWez> {
    WezCli {
        bin: "/nonexistent/wezterm".into(),
        config: "/nonexistent/wez.lua".into(),
        runner: NeverWez,
    }
}

fn args(native_ref: String) -> AdoptArgs {
    AdoptArgs {
        host: None,
        native_ref,
        name: None,
    }
}

fn named(native_ref: String, name: &str) -> AdoptArgs {
    AdoptArgs {
        name: Some(name.to_string()),
        ..args(native_ref)
    }
}

fn document(out: &AdoptOutput) -> serde_json::Value {
    serde_json::from_str(&out.stdout).unwrap_or_else(|e| panic!("{e}: {:?}", out.stdout))
}

// ---------------------------------------------------------------------------
// tmux, against a real scratch server

struct TmuxScratch {
    ns: String,
}

impl TmuxScratch {
    fn start(tag: &str) -> TmuxScratch {
        TmuxScratch {
            ns: format!("dmux-p6cli-{tag}-{}", std::process::id()),
        }
    }

    fn tmux(&self, args: &[&str]) -> String {
        let out = Command::new("tmux")
            .args(["-L", &self.ns, "-f", "/dev/null"])
            .args(args)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn marker(&self, session: &str, option: &str) -> Option<String> {
        let value = self.tmux(&["show-options", "-t", session, "-qv", option]);
        let value = value.trim_end_matches('\n');
        (!value.is_empty()).then(|| value.to_string())
    }

    /// The exact `$N` an unmanaged `ls` row would carry as its native token.
    fn session_id(&self, name: &str) -> String {
        self.tmux(&["list-sessions", "-F", "#{session_id} #{session_name}"])
            .lines()
            .find(|line| line.ends_with(name))
            .unwrap_or_else(|| panic!("no session {name}"))
            .split_whitespace()
            .next()
            .unwrap()
            .to_string()
    }
}

impl Drop for TmuxScratch {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.ns, "kill-server"])
            .output();
    }
}

/// A bootstrapped scratch server is the precondition for adoption: an
/// unepoched one lists unmanaged and is never written to (plan §11.2).
fn bootstrapped(scratch: &Scratch, tmux: &TmuxScratch) {
    match tmux_bootstrap(&scratch.env(), &tmux.ns).unwrap() {
        dmux::operations::TmuxBootstrapOutcome::Bootstrapped { .. } => {}
        other => panic!("fresh server must bootstrap: {other:?}"),
    }
}

#[test]
fn tmux_adoption_lands_active_unstamped_and_says_so() {
    let scratch = Scratch::new();
    let tmux = TmuxScratch::start("adopt");
    tmux.tmux(&["new-session", "-d", "-s", "legacy"]);
    bootstrapped(&scratch, &tmux);
    let session = tmux.session_id("legacy");

    let out = adopt_in(
        &scratch.env(),
        never_wez(),
        None,
        args(native_ref(Backend::Tmux, &session)),
    );
    assert_eq!(out.status, ExitStatus::Success, "{}", out.stderr);

    let registry = scratch.registry();
    let row = registry.spaces().unwrap().pop().unwrap();
    assert_eq!(row.logical_name, "legacy");
    assert_eq!(row.lifecycle, Lifecycle::Active);
    assert_eq!(row.health, Health::Unstamped);
    assert_eq!(
        registry
            .current_binding(row.space_uid)
            .unwrap()
            .unwrap()
            .native_token,
        session
    );

    // The receipt must not imply adoption finished: §10.3 keeps the Space
    // unstamped until every pre-existing pane acknowledges its marker.
    let no = row.space_no.get();
    assert_eq!(
        out.stdout,
        format!(
            "adopted {no} \"legacy\" (tmux) as dmux://{}/spaces/{}\n\
             unstamped: every pane that predates adoption must run \
             `dmux context stamp {no}`\n",
            row.owner.0, row.space_uid.0
        )
    );
}

#[test]
fn a_second_adopt_of_the_same_native_ref_is_refused() {
    let scratch = Scratch::new();
    let tmux = TmuxScratch::start("twice");
    tmux.tmux(&["new-session", "-d", "-s", "legacy"]);
    bootstrapped(&scratch, &tmux);
    let reference = native_ref(Backend::Tmux, &tmux.session_id("legacy"));

    let first = adopt_in(&scratch.env(), never_wez(), None, args(reference.clone()));
    assert_eq!(first.status, ExitStatus::Success, "{}", first.stderr);

    // `--name` steers the replay past the name guard, which is the only
    // thing that used to stand between a re-used NATIVE_REF and the live
    // session's markers: uniqueness on the native token fires at
    // finalization, i.e. after the stamp (case 13, §10.3).
    let space = scratch.registry().spaces().unwrap().pop().unwrap();
    let session = tmux.session_id("legacy");
    let before = scratch.revision();
    let again = adopt_in(
        &scratch.env(),
        never_wez(),
        Some(OutputFormat::Json),
        named(reference.clone(), "other"),
    );
    assert_eq!(again.status, ExitStatus::Conflict);
    let doc = document(&again);
    assert_eq!(doc["ok"], false);
    assert_eq!(doc["action"], "adopt");
    assert_eq!(doc["errors"][0]["code"], "identity_conflict");
    assert_eq!(doc["errors"][0]["target"], reference);
    assert_eq!(scratch.space_count(), 1, "adoption must not duplicate");

    // Nothing moved: no reservation was burned, the chain did not advance,
    // and the session still advertises the Space it actually belongs to.
    assert_eq!(scratch.revision(), before);
    assert_eq!(
        tmux.marker(&session, "@dmux_space_uid").as_deref(),
        Some(space.space_uid.0.to_string().as_str())
    );
    assert_eq!(
        tmux.marker(&session, "@dmux_space_no").as_deref(),
        Some(space.space_no.to_string().as_str())
    );

    // The same replay under the original name is still the name guard.
    let same = adopt_in(
        &scratch.env(),
        never_wez(),
        Some(OutputFormat::Json),
        args(reference.clone()),
    );
    assert_eq!(same.status, ExitStatus::Conflict);
    assert_eq!(document(&same)["errors"][0]["code"], "identity_conflict");
    assert_eq!(scratch.space_count(), 1);
}

#[test]
fn a_session_carrying_foreign_dmux_markers_is_a_conflict_not_a_rebind() {
    let scratch = Scratch::new();
    let tmux = TmuxScratch::start("foreign");
    tmux.tmux(&["new-session", "-d", "-s", "legacy"]);
    bootstrapped(&scratch, &tmux);
    let session = tmux.session_id("legacy");

    // Exactly what a restored registry or a re-enrolled machine leaves
    // behind: `inventory` calls this row unmanaged because no binding in
    // *this* registry claims it, so `ls` invites the operator to adopt it.
    let foreign = [
        ("@dmux_host_uid", "11111111-1111-4111-8111-111111111111"),
        ("@dmux_registry_uid", "22222222-2222-4222-8222-222222222222"),
        ("@dmux_space_uid", "33333333-3333-4333-8333-333333333333"),
        ("@dmux_space_no", "99"),
    ];
    for (option, value) in foreign {
        tmux.tmux(&["set-option", "-t", &session, option, value]);
    }
    let before = scratch.revision();

    let out = adopt_in(
        &scratch.env(),
        never_wez(),
        Some(OutputFormat::Json),
        args(native_ref(Backend::Tmux, &session)),
    );
    assert_eq!(out.status, ExitStatus::Conflict, "{}", out.stdout);
    assert_eq!(document(&out)["errors"][0]["code"], "identity_conflict");
    assert_eq!(scratch.space_count(), 0);
    assert_eq!(scratch.revision(), before);
    for (option, value) in foreign {
        assert_eq!(
            tmux.marker(&session, option).as_deref(),
            Some(value),
            "{option} was overwritten"
        );
    }
}

#[test]
fn an_operator_name_answers_to_the_new_name_grammar() {
    let scratch = Scratch::new();
    let before = scratch.revision();

    // `dmux new 7` is `invalid_name`; adopting under the same name has to
    // be too, or the numeric-ref grammar (§7.3) permanently shadows it.
    for name in ["7", "", "a:b", "-x"] {
        let out = adopt_in(
            &scratch.env(),
            never_wez(),
            Some(OutputFormat::Json),
            named(native_ref(Backend::Tmux, "$0"), name),
        );
        assert_eq!(out.status, ExitStatus::Usage, "{name:?}");
        assert_eq!(
            document(&out)["errors"][0]["code"],
            "invalid_name",
            "{name:?}"
        );
    }
    assert_eq!(scratch.space_count(), 0);
    assert_eq!(scratch.revision(), before);
}

#[test]
fn a_name_live_on_the_opposite_backend_is_a_collision() {
    let scratch = Scratch::new();
    let tmux = TmuxScratch::start("cross");
    tmux.tmux(&["new-session", "-d", "-s", "shared"]);
    bootstrapped(&scratch, &tmux);
    let session = tmux.session_id("shared");

    // One active Wez Space already owns the name. `dmux new shared` refuses
    // this with a `--allow-name-collision` remedy (§2.12, case 6); adopt
    // must not walk into it silently.
    {
        let mut registry = scratch.registry();
        let instance = registry
            .register_backend_instance(Backend::Wez, Some("/run/dmux/wez.sock"), None)
            .unwrap();
        let reservation = registry
            .reserve_space("shared", instance, Uuid::new_v4())
            .unwrap();
        registry
            .finalize_create(
                reservation.space_uid,
                reservation.operation_uid,
                &NativeBindingSpec {
                    native_token: "dmux:ws:shared".into(),
                    native_kind: NativeKind::WezWorkspaceKey,
                    server_epoch: None,
                },
            )
            .unwrap();
    }

    let out = adopt_in(
        &scratch.env(),
        never_wez(),
        Some(OutputFormat::Json),
        args(native_ref(Backend::Tmux, &session)),
    );
    assert_eq!(out.status, ExitStatus::Conflict, "{}", out.stdout);
    let doc = document(&out);
    assert_eq!(doc["errors"][0]["code"], "name_conflict");
    assert!(
        doc["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("--name"),
        "{doc}"
    );
    assert_eq!(scratch.space_count(), 1, "only the pre-existing Wez Space");
    assert_eq!(tmux.marker(&session, "@dmux_space_uid"), None);

    // The acknowledgement is an explicit different name, and it works.
    let ok = adopt_in(
        &scratch.env(),
        never_wez(),
        None,
        named(native_ref(Backend::Tmux, &session), "sharedtmux"),
    );
    assert_eq!(ok.status, ExitStatus::Success, "{}", ok.stderr);
    assert_eq!(scratch.space_count(), 2);
}

#[test]
fn a_malformed_native_ref_is_invalid_ref_and_mutates_nothing() {
    let scratch = Scratch::new();
    let before = scratch.revision();

    for malformed in [
        // A bare backend token: `adopt` accepts no command string (§7.4).
        "$1",
        "legacy",
        "native:tmux",
        "native:zellij:bGVnYWN5",
        "native:tmux:not base64",
        // Padded base64url would be a second spelling of one resource.
        "native:tmux:bGVnYWN5==",
    ] {
        let out = adopt_in(
            &scratch.env(),
            never_wez(),
            Some(OutputFormat::Json),
            args(malformed.to_string()),
        );
        assert_eq!(out.status, ExitStatus::Usage, "{malformed}");
        assert_eq!(
            document(&out)["errors"][0]["code"],
            "invalid_ref",
            "{malformed}"
        );
    }

    assert_eq!(scratch.space_count(), 0);
    assert_eq!(scratch.revision(), before);
}

#[test]
fn a_native_ref_naming_no_live_session_is_not_found_and_mutates_nothing() {
    let scratch = Scratch::new();
    let tmux = TmuxScratch::start("absent");
    tmux.tmux(&["new-session", "-d", "-s", "legacy"]);
    bootstrapped(&scratch, &tmux);
    let before = scratch.revision();

    let out = adopt_in(
        &scratch.env(),
        never_wez(),
        Some(OutputFormat::Json),
        args(native_ref(Backend::Tmux, "$999")),
    );
    assert_eq!(out.status, ExitStatus::NotFound);
    assert_eq!(document(&out)["errors"][0]["code"], "not_found");
    assert_eq!(scratch.space_count(), 0);
    assert_eq!(scratch.revision(), before);
}

#[test]
fn a_non_local_host_is_refused_before_any_scan() {
    let scratch = Scratch::new();
    let peer = {
        let mut registry = scratch.registry();
        registry
            .enroll_host(dmux::model::HostUid(Uuid::new_v4()), Some("archie"))
            .unwrap()
            .alias
    };
    let before = scratch.revision();

    let out = adopt_in(
        &scratch.env(),
        never_wez(),
        Some(OutputFormat::Json),
        AdoptArgs {
            host: Some(peer),
            native_ref: native_ref(Backend::Tmux, "$1"),
            name: None,
        },
    );
    // There is no ADOPT method in the agent protocol; adoption is
    // owner-local by §2.6, so this is a typed protocol answer, not a
    // backend failure.
    assert_eq!(out.status, ExitStatus::Unavailable);
    assert_eq!(document(&out)["errors"][0]["code"], "protocol_mismatch");
    assert_eq!(scratch.space_count(), 0);
    assert_eq!(scratch.revision(), before);
}

// ---------------------------------------------------------------------------
// Wez, against a fake mux

/// Whether the fake server carries the ADR 006 fork CAS verb. A stock
/// codec-45 server rejects PDU ident 63 with a frozen stderr reason.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cas {
    Fork,
    Stock,
}

/// A `wezterm cli` that answers `list` from an in-memory pane table and
/// applies the CAS rename to it, with the server-side compare the fork
/// performs. Enough to prove this CLI's mappings; `adopt_flow` covers the
/// live fork build.
struct FakeMux {
    epoch: Uuid,
    cas: Cas,
    /// `(window_id, tab_id, pane_id, workspace)`.
    panes: RefCell<Vec<(u64, u64, u64, String)>>,
    /// Everything this mux was asked, in order: a socket probe as
    /// `["probe", socket]`, otherwise the `wezterm cli` argv.
    commands: RefCell<Vec<Vec<String>>>,
}

impl FakeMux {
    fn new(cas: Cas, panes: &[(u64, u64, u64, &str)]) -> FakeMux {
        FakeMux {
            epoch: Uuid::new_v4(),
            cas,
            panes: RefCell::new(
                panes
                    .iter()
                    .map(|(w, t, p, ws)| (*w, *t, *p, ws.to_string()))
                    .collect(),
            ),
            commands: RefCell::new(Vec::new()),
        }
    }

    fn commands(&self) -> Vec<Vec<String>> {
        self.commands.borrow().clone()
    }

    fn workspaces(&self) -> Vec<String> {
        let mut names: Vec<String> = self.panes.borrow().iter().map(|p| p.3.clone()).collect();
        names.sort();
        names.dedup();
        names
    }

    fn listing(&self) -> String {
        let row = |w: u64, t: u64, p: u64, ws: &str| {
            format!(r#"{{"window_id":{w},"tab_id":{t},"pane_id":{p},"workspace":"{ws}"}}"#)
        };
        let mut rows = vec![row(0, 0, 0, &format!("dmux:system:{}", self.epoch))];
        rows.extend(
            self.panes
                .borrow()
                .iter()
                .map(|(w, t, p, ws)| row(*w, *t, *p, ws)),
        );
        format!("[{}]", rows.join(","))
    }

    fn cas_rename(&self, argv: &[String]) -> RunOutput {
        let after = |flag: &str| {
            argv.iter()
                .position(|a| a == flag)
                .map(|i| argv[i + 1].clone())
        };
        let window: u64 = after("--window-id").unwrap().parse().unwrap();
        let expected = after("--if-workspace").unwrap();
        let sole = argv.iter().any(|a| a == "--if-sole-window");
        let new = argv.last().unwrap().clone();

        let mut panes = self.panes.borrow_mut();
        let failed = |reason: String| RunOutput {
            status: 1,
            stdout: Vec::new(),
            stderr: format!("ERROR wezterm > {CAS_FAILED_MARKER}{reason}; terminating").into(),
        };
        let Some(actual) = panes.iter().find(|p| p.0 == window).map(|p| p.3.clone()) else {
            return failed("no_such_window".into());
        };
        if actual != expected {
            return failed(format!(
                "workspace_mismatch window_id={window} actual=\"{actual}\""
            ));
        }
        if sole && panes.iter().any(|p| p.3 == actual && p.0 != window) {
            return failed("not_sole_window".into());
        }
        for pane in panes.iter_mut().filter(|p| p.0 == window) {
            pane.3 = new.clone();
        }
        RunOutput {
            status: 0,
            stdout: Vec::new(),
            stderr: Vec::new(),
        }
    }
}

impl WezRunner for &FakeMux {
    fn probe(&self, socket: &str, _pid: Option<u32>) -> ProbeOutcome {
        self.commands
            .borrow_mut()
            .push(vec!["probe".to_string(), socket.to_string()]);
        ProbeOutcome::Connectable
    }

    fn run(&self, invocation: &WezInvocation, _: Duration) -> Result<RunOutput, RunError> {
        let argv = &invocation.argv;
        self.commands.borrow_mut().push(argv.clone());
        if argv.iter().any(|a| a == "list") {
            return Ok(RunOutput {
                status: 0,
                stdout: self.listing().into(),
                stderr: Vec::new(),
            });
        }
        assert!(
            argv.iter().any(|a| a == "rename-workspace"),
            "unexpected wez verb: {argv:?}"
        );
        if self.cas == Cas::Stock {
            return Ok(RunOutput {
                status: 1,
                stdout: Vec::new(),
                stderr: format!("ERROR wezterm > {CAS_MISSING_PDU_STDERR}").into(),
            });
        }
        Ok(self.cas_rename(argv))
    }
}

/// Register the managed Wez instance the way the service would, so the CLI
/// finds the socket and published epoch in the registry (as `ls` does).
fn wez_registered<'a>(scratch: &Scratch, mux: &'a FakeMux) -> WezCli<&'a FakeMux> {
    let mut registry = scratch.registry();
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
    WezCli {
        bin: "/opt/homebrew/bin/wezterm".into(),
        config: "/etc/dmux/wez.lua".into(),
        runner: mux,
    }
}

/// Register the managed Wez instance the way `dmux-mux-start.sh` leaves it
/// before the mux coordinates — addressable, but with no published epoch —
/// which is also what a service that never finished coordinating leaves
/// behind for good.
fn wez_registered_unpublished<'a>(
    scratch: &Scratch,
    mux: &'a FakeMux,
) -> (BackendInstanceUid, WezCli<&'a FakeMux>) {
    let instance = scratch
        .registry()
        .register_backend_instance(Backend::Wez, Some("/run/dmux/wez.sock"), None)
        .unwrap();
    (
        instance,
        WezCli {
            bin: "/opt/homebrew/bin/wezterm".into(),
            config: "/etc/dmux/wez.lua".into(),
            runner: mux,
        },
    )
}

/// Review finding #2, inverted (ADR 012 WS-A.5). A registered, addressable
/// Wez instance whose `server_epoch` is NULL used to be adopted *from*: the
/// CAS rename landed on whatever server answered at the endpoint, the verb
/// exited 0, and the binding carried that stranger's epoch. Nothing in the
/// registry vouches for that server, so there is nothing to fence the
/// mutation against: `adopt` refuses before the re-resolving scan, before
/// the reservation, and before the rename.
#[test]
fn a_wez_instance_with_no_published_epoch_is_refused_before_any_rename() {
    let scratch = Scratch::new();
    let mux = FakeMux::new(Cas::Fork, &[(1, 10, 100, "alpha")]);
    let (instance, wez) = wez_registered_unpublished(&scratch, &mux);
    let reference = native_ref(Backend::Wez, "alpha");
    let before = scratch.revision();

    let out = adopt_in(
        &scratch.env(),
        wez,
        Some(OutputFormat::Json),
        args(reference.clone()),
    );
    assert_eq!(out.status, ExitStatus::OperationFailure, "{}", out.stdout);
    let doc = document(&out);
    assert_eq!(doc["ok"], false);
    assert_eq!(doc["action"], "adopt");
    assert_eq!(doc["errors"][0]["code"], "backend_epoch_changed");
    assert!(
        doc["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("has published no server epoch"),
        "{doc}"
    );
    assert_eq!(doc["authority_revision"], before);

    // Nothing reached the mux: no socket probe, not the re-resolving
    // `list`, not the CAS rename. The workspace is exactly where it was.
    assert!(mux.commands().is_empty(), "{:?}", mux.commands());
    assert_eq!(mux.workspaces(), vec!["alpha".to_string()]);

    // Nothing durable: no Space row in any lifecycle (so no reservation to
    // abort), no binding, no journal row, no epoch minted from the
    // stranger, and the authority chain did not move.
    assert_eq!(scratch.table_rows("spaces"), 0);
    assert_eq!(scratch.table_rows("native_bindings"), 0);
    assert_eq!(scratch.table_rows("operations"), 0);
    assert_eq!(
        scratch
            .registry()
            .backend_server(instance)
            .unwrap()
            .server_epoch,
        None
    );
    assert_eq!(scratch.revision(), before);

    // The human renderer is the same refusal, on stderr only.
    let human = adopt_in(
        &scratch.env(),
        wez_registered_unpublished(&scratch, &mux).1,
        None,
        args(reference),
    );
    assert_eq!(human.status, ExitStatus::OperationFailure);
    assert_eq!(human.stdout, "");
    assert!(
        human.stderr.contains("has published no server epoch"),
        "{:?}",
        human.stderr
    );
    assert!(mux.commands().is_empty());
    assert_eq!(scratch.revision(), before);
}

/// The tmux arm of the same refusal. `tmux_bootstrap` registers the instance
/// before it publishes the epoch; a bootstrap that died in between leaves
/// exactly this row, and a live session on that namespace is not adoptable
/// until something publishes what the server is.
#[test]
fn a_tmux_instance_with_no_published_epoch_is_refused_before_any_scan() {
    let scratch = Scratch::new();
    let tmux = TmuxScratch::start("unpublished");
    tmux.tmux(&["new-session", "-d", "-s", "legacy"]);
    scratch
        .registry()
        .register_backend_instance(Backend::Tmux, Some(&tmux.ns), None)
        .unwrap();
    let session = tmux.session_id("legacy");
    let before = scratch.revision();

    let out = adopt_in(
        &scratch.env(),
        never_wez(),
        Some(OutputFormat::Json),
        args(native_ref(Backend::Tmux, &session)),
    );
    assert_eq!(out.status, ExitStatus::OperationFailure, "{}", out.stdout);
    assert_eq!(document(&out)["errors"][0]["code"], "backend_epoch_changed");

    // The session was neither stamped nor bootstrapped behind the
    // operator's back, and the registry holds nothing new.
    assert_eq!(tmux.marker(&session, "@dmux_space_uid"), None);
    assert_eq!(tmux.marker(&session, "@dmux_space_no"), None);
    assert_eq!(scratch.table_rows("spaces"), 0);
    assert_eq!(scratch.table_rows("operations"), 0);
    assert_eq!(scratch.revision(), before);
}

#[test]
fn wez_adoption_cas_renames_to_the_opaque_key_and_lands_unstamped() {
    let scratch = Scratch::new();
    let mux = FakeMux::new(Cas::Fork, &[(1, 10, 100, "alpha"), (1, 11, 101, "alpha")]);

    let out = adopt_in(
        &scratch.env(),
        wez_registered(&scratch, &mux),
        Some(OutputFormat::Json),
        args(native_ref(Backend::Wez, "alpha")),
    );
    assert_eq!(out.status, ExitStatus::Success, "{}", out.stderr);

    let row = scratch.registry().spaces().unwrap().pop().unwrap();
    assert_eq!(row.lifecycle, Lifecycle::Active);
    assert_eq!(row.health, Health::Unstamped);

    let doc = document(&out);
    assert_eq!(doc["ok"], true);
    assert_eq!(doc["action"], "adopt");
    assert_eq!(doc["result"]["backend"], "wez");
    assert_eq!(doc["result"]["name"], "alpha");
    assert_eq!(doc["result"]["health"], "unstamped");
    assert_eq!(doc["result"]["lifecycle"], "active");
    assert_eq!(
        doc["result"]["pending_stamp_command"],
        format!("dmux context stamp {}", row.space_no.get())
    );

    // The friendly name never becomes the native key (§2.4): the workspace
    // now carries the opaque one, and both panes moved together.
    let key = format!("dmux:{}:{}", row.owner.0, row.space_uid.0);
    assert_eq!(doc["result"]["native_token"], key);
    assert_eq!(mux.workspaces(), vec![key]);
}

#[test]
fn wez_adoption_without_the_fork_cas_verb_leaves_it_unmanaged() {
    let scratch = Scratch::new();
    let mux = FakeMux::new(Cas::Stock, &[(1, 10, 100, "alpha")]);
    let before = scratch.revision();

    let out = adopt_in(
        &scratch.env(),
        wez_registered(&scratch, &mux),
        Some(OutputFormat::Json),
        args(native_ref(Backend::Wez, "alpha")),
    );
    // A build incompatibility, not a backend failure: exit 6, and the
    // workspace stays exactly where it was (§2.7).
    assert_eq!(out.status, ExitStatus::Unavailable);
    let doc = document(&out);
    assert_eq!(doc["errors"][0]["code"], "version_mismatch");
    assert!(
        doc["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("stays unmanaged"),
        "{doc}"
    );
    assert_eq!(mux.workspaces(), vec!["alpha".to_string()]);
    assert_eq!(scratch.space_count(), 0);
    // Registering the instance advanced the chain; adoption did not.
    assert!(scratch.revision() > before);
    assert_eq!(scratch.registry().spaces().unwrap().len(), 0);
}

#[test]
fn a_multi_window_wez_workspace_is_refused_until_normalized() {
    let scratch = Scratch::new();
    let mux = FakeMux::new(Cas::Fork, &[(3, 13, 104, "mw"), (4, 14, 105, "mw")]);
    let reference = native_ref(Backend::Wez, "mw");

    let out = adopt_in(
        &scratch.env(),
        wez_registered(&scratch, &mux),
        Some(OutputFormat::Json),
        args(reference.clone()),
    );
    assert_eq!(out.status, ExitStatus::Conflict);
    let doc = document(&out);
    assert_eq!(doc["errors"][0]["code"], "repair_required");
    assert!(
        doc["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains(&format!("dmux repair normalize {reference}")),
        "{doc}"
    );
    assert_eq!(mux.workspaces(), vec!["mw".to_string()]);
    assert_eq!(scratch.space_count(), 0);
}
