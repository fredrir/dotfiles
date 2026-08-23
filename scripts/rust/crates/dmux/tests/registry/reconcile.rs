//! `dmux repair reconcile`, black box (plan §10.2/§10.3, cases 11, 13, 39).
//!
//! The library half of crash reconciliation is unit-tested beside the code in
//! `src/operations.rs`; what has to be proven from outside is the operator
//! contract: the preview, §7.4's confirmation rule, and case 43's "exactly
//! one §16.2 document on every branch, refusals included".
//!
//! These drive the real binary against a scratch registry through the hidden
//! `--data-dir`/`--lock-dir` seams `repair normalize` already uses, so no
//! production path is consulted; XDG and the runtime dir are redirected too,
//! belt and braces.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use dmux::model::{
    Backend, BackendInstanceUid, Lifecycle, OperationKind, OperationState, ServerEpoch,
};
use dmux::registry::{Registry, SpaceReservation};
use serde_json::Value;
use uuid::Uuid;

use super::util::{self, Scratch};

/// The managed tmux instance a crashed holder's row points at, in the state
/// every production instance is in once `tmux_bootstrap` has run: a recorded
/// namespace and a published epoch. Reconciliation refuses anything less
/// before it decides (`backend::scope::resolve_managed_instance`), so the
/// fixture has to be what the registry can vouch for. Later
/// `util::tmux_instance` calls in the same test resolve to this row —
/// `register_backend_instance` returns the existing `(owner, backend)` one.
fn published_tmux_instance(registry: &mut Registry) -> BackendInstanceUid {
    let instance = registry
        .register_backend_instance(Backend::Tmux, Some("dmux-reconcile-scratch"), None)
        .unwrap();
    registry
        .publish_backend_server(
            instance,
            ServerEpoch(Uuid::new_v4()),
            // A live pid: WS-B.1 refutes a published row whose process is dead.
            Some(i64::from(std::process::id())),
            Some("start"),
            None,
            None,
        )
        .unwrap();
    instance
}

/// A `reserved` Space beside a `prepared` adopt row: exactly what a SIGKILL
/// between `reserve_space_kind` and `abort_create` leaves behind, and what no
/// verb could reap before `repair reconcile`.
fn stranded_adoption(scratch: &Scratch) -> SpaceReservation {
    let mut registry = util::open(&scratch.config);
    let instance = published_tmux_instance(&mut registry);
    registry
        .reserve_space_kind("legacy", instance, Uuid::new_v4(), OperationKind::Adopt)
        .unwrap()
}

/// The same crash on the Wez side. It looks identical in the registry — a
/// `reserved` Space and a `prepared` adopt row — but `adopt_wez` renames the
/// source workspace to the reservation's opaque key *before* it binds, so
/// closing this row without checking the workspace can leave it wearing a key
/// for a Space that never existed.
fn stranded_wez_adoption(scratch: &Scratch) -> SpaceReservation {
    let mut registry = util::open(&scratch.config);
    let instance = registry
        .register_backend_instance(Backend::Wez, Some("/run/dmux/absent.sock"), None)
        .unwrap();
    registry
        .reserve_space_kind("legacy", instance, Uuid::new_v4(), OperationKind::Adopt)
        .unwrap()
}

fn dmux_command(scratch: &Scratch, args: &[&str]) -> Command {
    let data_dir = scratch.dir.path().display().to_string();
    let lock_dir = scratch.config.lock_dir.display().to_string();
    let sink = scratch.dir.path().join("xdg");
    std::fs::create_dir_all(&sink).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_dmux"));
    command
        .args(args)
        .args(["--data-dir", &data_dir, "--lock-dir", &lock_dir])
        // The seams above are what the command reads; these only guarantee
        // that a mistake in them cannot reach the real registry.
        .env("XDG_DATA_HOME", &sink)
        .env("XDG_STATE_HOME", &sink)
        .env("XDG_RUNTIME_DIR", &sink)
        .env("DMUX_RUNTIME_DIR", &sink)
        .env("TMUX_TMPDIR", &sink)
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

fn dmux(scratch: &Scratch, args: &[&str]) -> Output {
    dmux_command(scratch, args).output().expect("dmux runs")
}

/// A `tmux` on PATH that records every invocation before handing it to the
/// real binary: proof of which native commands a verb ran — or that it ran
/// none — on a real server.
struct TmuxWrapper {
    bin: tempfile::TempDir,
    log: PathBuf,
}

impl TmuxWrapper {
    fn install() -> TmuxWrapper {
        let real = std::env::var_os("PATH")
            .and_then(|path| {
                std::env::split_paths(&path)
                    .map(|dir| dir.join("tmux"))
                    .find(|candidate| candidate.is_file())
            })
            .expect("a real tmux on PATH");
        let bin = tempfile::tempdir().unwrap();
        let log = bin.path().join("tmux.log");
        let script = bin.path().join("tmux");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\nexec {} \"$@\"\n",
                log.display(),
                real.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        TmuxWrapper { bin, log }
    }

    /// PATH with the wrapper first, the real binaries behind it.
    fn path(&self) -> std::ffi::OsString {
        let mut dirs = vec![self.bin.path().to_path_buf()];
        if let Some(path) = std::env::var_os("PATH") {
            dirs.extend(std::env::split_paths(&path));
        }
        std::env::join_paths(dirs).unwrap()
    }

    fn invocations(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .map(|text| text.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    }
}

/// A private tmux server nothing has epoched, answering on a namespace the
/// registry records: the "foreign server" of the review's reproduction.
struct ForeignTmux {
    ns: String,
}

impl ForeignTmux {
    fn start(tag: &str) -> ForeignTmux {
        let server = ForeignTmux {
            ns: format!("dmux-reconcile-{tag}-{}", std::process::id()),
        };
        let out = Command::new("tmux")
            .args(["-L", &server.ns, "-f", "/dev/null"])
            .args(["new-session", "-d", "-s", "seed"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        server
    }

    fn sessions(&self) -> usize {
        let out = Command::new("tmux")
            .args(["-L", &self.ns, "list-sessions", "-F", "#{session_id}"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).lines().count()
    }
}

impl Drop for ForeignTmux {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.ns, "kill-server"])
            .output();
    }
}

/// `dmux` able to reach a [`ForeignTmux`]: the child's `tmux` is the logging
/// wrapper, and `TMUX_TMPDIR` is left alone so its socket directory is the
/// one the test's own `tmux` used.
fn dmux_with_tmux(scratch: &Scratch, wrapper: &TmuxWrapper, args: &[&str]) -> Output {
    dmux_command(scratch, args)
        .env("PATH", wrapper.path())
        .env_remove("TMUX_TMPDIR")
        .output()
        .expect("dmux runs")
}

/// Exactly one §16.2 document on stdout, and nothing else.
fn one_document(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.trim().lines().count(),
        1,
        "expected exactly one document, got: {stdout}"
    );
    serde_json::from_str(stdout.trim()).expect("stdout is one JSON document")
}

fn state_of(scratch: &Scratch, reservation: &SpaceReservation) -> (Lifecycle, OperationState) {
    let registry = util::open(&scratch.config);
    (
        registry.space(reservation.space_uid).unwrap().lifecycle,
        registry.operation(reservation.operation_uid).unwrap().state,
    )
}

#[test]
fn json_without_yes_previews_the_stranded_row_and_changes_nothing() {
    let scratch = util::scratch();
    let reservation = stranded_adoption(&scratch);

    let output = dmux(&scratch, &["--format", "json", "repair", "reconcile"]);
    assert_eq!(output.status.code(), Some(5));
    let document = one_document(&output);
    assert_eq!(document["ok"], false);
    assert_eq!(document["action"], "repair_reconcile");
    assert_eq!(document["errors"][0]["code"], "confirmation_required");
    // §7.4: a JSON destructive verb never prompts, so the preview has to
    // travel inside the confirmation document or the operator never sees it.
    let target = &document["result"]["targets"][0];
    assert_eq!(target["kind"], "adopt");
    assert_eq!(target["state"], "prepared");
    assert_eq!(target["lifecycle"], "reserved");
    assert_eq!(target["duty"], "adoption_reconcile");
    assert_eq!(target["in_flight"], false);
    // Every other branch of this verb reports `targets` beside `results`; the
    // confirmation is not entitled to a shape of its own.
    assert_eq!(document["result"]["results"].as_array().unwrap().len(), 0);
    assert_eq!(
        state_of(&scratch, &reservation),
        (Lifecycle::Reserved, OperationState::Prepared)
    );
}

/// A Wez adoption is not decidable from the registry alone: the CAS rename
/// lands before the binding does, so "no binding" is not evidence that
/// nothing was renamed. Without a reachable server there is no evidence
/// either way, and releasing would be the fabricated success §10.2 forbids.
#[test]
fn a_wez_adoption_is_not_released_without_the_evidence_only_its_server_has() {
    let scratch = util::scratch();
    let reservation = stranded_wez_adoption(&scratch);

    let output = dmux(
        &scratch,
        &["--format", "json", "repair", "reconcile", "--yes"],
    );
    assert_eq!(
        output.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let document = one_document(&output);
    assert_eq!(document["ok"], false);
    assert_eq!(document["result"]["results"][0]["outcome"], "failed_closed");
    assert_eq!(document["errors"][0]["code"], "repair_required");
    let detail = document["errors"][0]["message"].as_str().unwrap();
    assert!(
        detail.contains("wez server could not be reached"),
        "{detail}"
    );
    assert!(
        detail.contains(&reservation.space_uid.0.to_string()),
        "{detail}"
    );
    assert_eq!(
        state_of(&scratch, &reservation),
        (Lifecycle::Reserved, OperationState::Prepared)
    );
}

#[test]
fn yes_releases_the_burned_name_in_one_document() {
    let scratch = util::scratch();
    let reservation = stranded_adoption(&scratch);

    let output = dmux(
        &scratch,
        &["--format", "json", "repair", "reconcile", "--yes"],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let document = one_document(&output);
    assert_eq!(document["ok"], true);
    assert_eq!(document["errors"].as_array().unwrap().len(), 0);
    assert_eq!(
        document["result"]["results"][0]["outcome"],
        "reservation_released"
    );
    assert_eq!(
        state_of(&scratch, &reservation),
        (Lifecycle::Aborted, OperationState::Aborted)
    );

    // The whole point: the logical name is available again.
    let mut registry = util::open(&scratch.config);
    let instance = util::tmux_instance(&mut registry);
    registry
        .reserve_space("legacy", instance, Uuid::new_v4())
        .unwrap();
}

#[test]
fn a_second_run_finds_nothing_and_still_answers_with_a_document() {
    let scratch = util::scratch();
    stranded_adoption(&scratch);
    dmux(
        &scratch,
        &["--format", "json", "repair", "reconcile", "--yes"],
    );

    let output = dmux(
        &scratch,
        &["--format", "json", "repair", "reconcile", "--yes"],
    );
    assert_eq!(output.status.code(), Some(0));
    let document = one_document(&output);
    assert_eq!(document["ok"], true);
    assert_eq!(document["result"]["targets"].as_array().unwrap().len(), 0);
    assert_eq!(document["result"]["results"].as_array().unwrap().len(), 0);
}

#[test]
fn a_ref_that_is_not_stranded_is_a_document_too_never_a_silent_empty_run() {
    let scratch = util::scratch();
    let reservation = stranded_adoption(&scratch);

    let output = dmux(
        &scratch,
        &[
            "--format",
            "json",
            "repair",
            "reconcile",
            "--yes",
            "nosuchspace",
        ],
    );
    assert_eq!(output.status.code(), Some(3));
    let document = one_document(&output);
    assert_eq!(document["ok"], false);
    assert_eq!(document["action"], "repair_reconcile");
    assert_eq!(document["errors"][0]["code"], "not_found");
    assert_eq!(document["errors"][0]["target"], "nosuchspace");
    assert_eq!(
        state_of(&scratch, &reservation),
        (Lifecycle::Reserved, OperationState::Prepared)
    );
}

#[test]
fn a_named_ref_reconciles_only_that_row() {
    let scratch = util::scratch();
    let first = stranded_adoption(&scratch);
    let second = {
        let mut registry = util::open(&scratch.config);
        let instance = util::tmux_instance(&mut registry);
        registry
            .reserve_space_kind("other", instance, Uuid::new_v4(), OperationKind::Adopt)
            .unwrap()
    };

    let output = dmux(
        &scratch,
        &["--format", "json", "repair", "reconcile", "--yes", "legacy"],
    );
    assert_eq!(output.status.code(), Some(0));
    let document = one_document(&output);
    assert_eq!(document["result"]["results"].as_array().unwrap().len(), 1);
    assert_eq!(
        state_of(&scratch, &first),
        (Lifecycle::Aborted, OperationState::Aborted)
    );
    assert_eq!(
        state_of(&scratch, &second),
        (Lifecycle::Reserved, OperationState::Prepared)
    );
}

#[test]
fn a_pipe_without_yes_changes_nothing_and_exits_five() {
    let scratch = util::scratch();
    let reservation = stranded_adoption(&scratch);

    let output = dmux(&scratch, &["repair", "reconcile"]);
    assert_eq!(output.status.code(), Some(5));
    // The preview still prints: a human reading the pipe learns which rows
    // are stranded even though nothing was applied.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("legacy"), "{stdout}");
    assert!(stdout.contains("adoption_reconcile"), "{stdout}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("confirmation required"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        state_of(&scratch, &reservation),
        (Lifecycle::Reserved, OperationState::Prepared)
    );
}

#[test]
fn a_row_a_live_holder_owns_is_reported_as_a_partial_never_touched() {
    let scratch = util::scratch();
    let reservation = stranded_adoption(&scratch);
    let owner = util::open(&scratch.config).identity().unwrap().host_uid;

    // Stand in for the still-running holder: the §10.1 locks a live adopt
    // keeps down for the whole of its call. The kernel drops them when a
    // holder dies, which is exactly what makes them the crash discriminator.
    let mut holder = dmux::locks::OrderedLocks::new(&scratch.config.lock_dir);
    holder
        .acquire(
            dmux::locks::LockScope::AuthorityGate,
            dmux::locks::LockMode::Shared,
        )
        .unwrap();
    holder
        .acquire_decisions(owner, &["legacy"], dmux::locks::LockMode::Exclusive)
        .unwrap();

    let output = dmux(
        &scratch,
        &["--format", "json", "repair", "reconcile", "--yes"],
    );
    assert_eq!(output.status.code(), Some(7));
    let document = one_document(&output);
    assert_eq!(document["ok"], false);
    assert_eq!(
        document["result"]["results"][0]["outcome"],
        "skipped_in_flight"
    );
    assert_eq!(document["errors"][0]["code"], "operation_in_progress");
    assert_eq!(
        state_of(&scratch, &reservation),
        (Lifecycle::Reserved, OperationState::Prepared)
    );
    drop(holder);
}

#[test]
fn an_empty_registry_answers_with_a_document_rather_than_nothing() {
    let scratch = util::scratch();
    // Materialize the registry without stranding anything.
    let _ = util::open(&scratch.config);

    let output = dmux(
        &scratch,
        &["--format", "json", "repair", "reconcile", "--yes"],
    );
    assert_eq!(output.status.code(), Some(0));
    let document = one_document(&output);
    assert_eq!(document["ok"], true);
    assert_eq!(document["result"]["targets"].as_array().unwrap().len(), 0);
}

/// Review finding #7 inverted (ADR 012 WS-A.5; cases 11 and 13). The
/// registry knows the tmux instance and its namespace but has published no
/// epoch for it, and a server nobody verified answers on that namespace.
/// Reconciliation used to build an unpinned scope, run the crashed create's
/// keyed lookup against the stranger, find nothing under the reserved key,
/// and release the reservation — a durable `abort_create` driven by an
/// unverified read. Now the target is refused before any native call:
/// `backend_epoch_changed`, the row exactly as the crash left it, the server
/// never spoken to.
#[test]
fn a_stranded_create_on_an_unpublished_tmux_instance_is_refused_not_released() {
    let scratch = util::scratch();
    let server = ForeignTmux::start("unpub");
    let wrapper = TmuxWrapper::install();
    let (instance, reservation) = {
        let mut registry = util::open(&scratch.config);
        let instance = registry
            .register_backend_instance(Backend::Tmux, Some(&server.ns), None)
            .unwrap();
        let reservation = registry
            .reserve_space_kind("proj", instance, Uuid::new_v4(), OperationKind::Create)
            .unwrap();
        (instance, reservation)
    };

    let output = dmux_with_tmux(
        &scratch,
        &wrapper,
        &["--format", "json", "repair", "reconcile", "--yes"],
    );
    assert_eq!(
        output.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let document = one_document(&output);
    assert_eq!(document["ok"], false);
    assert_eq!(document["result"]["targets"][0]["kind"], "create");
    assert_eq!(document["result"]["targets"][0]["in_flight"], false);
    assert_eq!(document["result"]["results"][0]["outcome"], "failed_closed");
    assert_eq!(document["errors"][0]["code"], "backend_epoch_changed");
    assert_eq!(
        document["errors"][0]["target"],
        reservation.space_uid.0.to_string()
    );
    let detail = document["errors"][0]["message"].as_str().unwrap();
    assert!(detail.contains("has published no server epoch"), "{detail}");
    assert!(detail.contains(&instance.0.to_string()), "{detail}");
    assert_eq!(
        state_of(&scratch, &reservation),
        (Lifecycle::Reserved, OperationState::Prepared)
    );
    assert_eq!(
        wrapper.invocations(),
        Vec::<String>::new(),
        "a refused target must not be scanned"
    );
    assert_eq!(server.sessions(), 1, "the stranger was never touched");
}

/// The same row on an instance with no recorded endpoint is refused too —
/// with the code this verb's siblings already answer a missing namespace
/// with — rather than released on the registry's word alone. Before, the
/// missing namespace produced "no provider", and on tmux "no provider"
/// meant "decide without evidence".
#[test]
fn a_stranded_row_on_an_unaddressable_tmux_instance_is_refused() {
    let scratch = util::scratch();
    let (instance, reservation) = {
        let mut registry = util::open(&scratch.config);
        let instance = util::tmux_instance(&mut registry);
        let reservation = registry
            .reserve_space_kind("legacy", instance, Uuid::new_v4(), OperationKind::Adopt)
            .unwrap();
        (instance, reservation)
    };

    let output = dmux(
        &scratch,
        &["--format", "json", "repair", "reconcile", "--yes"],
    );
    assert_eq!(
        output.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let document = one_document(&output);
    assert_eq!(document["ok"], false);
    assert_eq!(document["result"]["results"][0]["outcome"], "failed_closed");
    assert_eq!(document["errors"][0]["code"], "provider_unavailable");
    let detail = document["errors"][0]["message"].as_str().unwrap();
    assert!(detail.contains("has no recorded endpoint"), "{detail}");
    assert!(detail.contains(&instance.0.to_string()), "{detail}");
    assert_eq!(
        state_of(&scratch, &reservation),
        (Lifecycle::Reserved, OperationState::Prepared)
    );
}

// ---------------------------------------------------------------------------
// ADR 012 WS-D.2: the compensation for a crashed Wez adoption aims its
// reverse CAS at the SOURCE the journal recorded, not at the logical name.
// Driven at the operations layer against the scripted mux, since the fork
// CAS build is not a test dependency; what is provable is the exact argv.

use dmux::backend::InventoryScope;
use dmux::backend::wez::WezProvider;
use dmux::operations::{
    OperationEnv, ReconcileBackend, ReconcileOutcome, reconcile_apply, reconcile_scan,
};

use super::util::{Cas, FakeMux};

/// A managed Wez instance published as the scripted mux's epoch, the env
/// that reaches it, and the scope every scan is pinned to.
fn wez_reconcile_home(
    scratch: &Scratch,
    mux: &FakeMux,
) -> (OperationEnv, BackendInstanceUid, InventoryScope) {
    let mut registry = util::open(&scratch.config);
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
    let env = OperationEnv {
        db_path: scratch.config.db_path.clone(),
        lock_dir: scratch.config.lock_dir.clone(),
    };
    (
        env,
        instance,
        InventoryScope::managed(Backend::Wez, "/run/dmux/wez.sock", ServerEpoch(mux.epoch)),
    )
}

fn opaque_key(scratch: &Scratch, reservation: &SpaceReservation) -> String {
    let owner = util::open(&scratch.config).identity().unwrap().host_uid;
    format!("dmux:{}:{}", owner.0, reservation.space_uid.0)
}

/// The CAS argv of the one compensation: `(--if-workspace, --if-sole-window, NEW)`.
fn compensation(mux: &FakeMux) -> (String, bool, String) {
    let calls = mux.cas_calls();
    assert_eq!(calls.len(), 1, "exactly one compensating CAS: {calls:?}");
    let argv = &calls[0];
    let after = |flag: &str| {
        argv.iter()
            .position(|a| a == flag)
            .map(|i| argv[i + 1].clone())
            .unwrap()
    };
    (
        after("--if-workspace"),
        argv.iter().any(|a| a == "--if-sole-window"),
        argv.last().unwrap().clone(),
    )
}

/// ADR 011's recorded limitation, closed: `dmux adopt --name other` died
/// after its CAS rename of workspace `legacy` to the opaque key. The
/// journal now carries `legacy`, so the reverse rename restores `legacy` —
/// not `other`, which is a name nothing on the server ever had.
#[test]
fn a_crashed_wez_adopt_with_a_name_is_reversed_to_the_source_not_the_logical_name() {
    let scratch = util::scratch();
    let mux = FakeMux::new(Cas::Fork, &[]);
    let (env, instance, scope) = wez_reconcile_home(&scratch, &mux);
    let reservation = util::open(&scratch.config)
        .reserve_adoption(
            "other",
            instance,
            Uuid::new_v4(),
            OperationKind::Adopt,
            "legacy",
        )
        .unwrap();
    // The CAS had landed: the workspace wears the reservation's key.
    let key = opaque_key(&scratch, &reservation);
    mux.add_pane(11, 110, 1100, &key);
    let provider = WezProvider::with_runner("/opt/homebrew/bin/wezterm", "/etc/dmux/wez.lua", &mux);

    let targets = reconcile_scan(&env).unwrap();
    assert_eq!(targets.len(), 1, "{targets:?}");
    let result = reconcile_apply(
        &env,
        &targets[0],
        Some(ReconcileBackend::restorable(&provider, &scope, &provider)),
    );
    assert_eq!(
        result.outcome,
        ReconcileOutcome::ReservationReleased,
        "{result:?}"
    );
    assert!(
        result.detail.contains("journaled source token"),
        "{result:?}"
    );
    assert_eq!(
        compensation(&mux),
        (key, true, "legacy".to_string()),
        "the reverse CAS is guarded like the adopt's and aims at the source"
    );
    assert_eq!(mux.workspaces(), vec!["legacy".to_string()]);
    assert_eq!(
        state_of(&scratch, &reservation),
        (Lifecycle::Aborted, OperationState::Aborted)
    );
}

/// A row journaled before schema v5 carries no source, and reconciles
/// exactly as it did: the reverse rename aims at the logical name and the
/// detail says why.
#[test]
fn a_pre_v5_wez_adopt_row_is_reversed_to_the_logical_name_as_before() {
    let scratch = util::scratch();
    let mux = FakeMux::new(Cas::Fork, &[]);
    let (env, instance, scope) = wez_reconcile_home(&scratch, &mux);
    let reservation = util::open(&scratch.config)
        .reserve_space_kind("legacy", instance, Uuid::new_v4(), OperationKind::Adopt)
        .unwrap();
    assert_eq!(
        util::open(&scratch.config)
            .operation(reservation.operation_uid)
            .unwrap()
            .source_native_token,
        None,
        "the fixture models a pre-v5 row"
    );
    let key = opaque_key(&scratch, &reservation);
    mux.add_pane(11, 110, 1100, &key);
    let provider = WezProvider::with_runner("/opt/homebrew/bin/wezterm", "/etc/dmux/wez.lua", &mux);

    let targets = reconcile_scan(&env).unwrap();
    let result = reconcile_apply(
        &env,
        &targets[0],
        Some(ReconcileBackend::restorable(&provider, &scope, &provider)),
    );
    assert_eq!(
        result.outcome,
        ReconcileOutcome::ReservationReleased,
        "{result:?}"
    );
    assert!(
        result
            .detail
            .contains("predates the journaled source token"),
        "{result:?}"
    );
    assert_eq!(compensation(&mux), (key, true, "legacy".to_string()));
    assert_eq!(mux.workspaces(), vec!["legacy".to_string()]);
    assert_eq!(
        state_of(&scratch, &reservation),
        (Lifecycle::Aborted, OperationState::Aborted)
    );
}
