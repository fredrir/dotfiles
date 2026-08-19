//! P11 acceptance case 45: `dmux migrate` (plan §17, §20.2).
//!
//! > Existing resources migrate once through explicit adoption; every
//! > multi-window Wez resource is normalized or quarantined unmanaged before
//! > cutover, duplicate names remain independently addressable, and wrappers
//! > expand to the same plans as direct dmux.
//!
//! The wrapper half of the case is `tests/cli.rs`
//! (`the_wrapper_verb_allowlist_matches_the_cli`); everything else is here.
//!
//! tmux runs against a real scratch `tmux -L` namespace, so nothing in this
//! file can reach the developer's own server. Wez runs against a fake mux —
//! the fork CAS build is not a test dependency, and the multi-window
//! quarantine is precisely what must be provable without one.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::process::Command;
use std::time::Duration;

use dmux::adopt_cli::WezCli;
use dmux::backend::wez::{ProbeOutcome, RunError, RunOutput, WezInvocation, WezRunner};
use dmux::error::ExitStatus;
use dmux::history::History;
use dmux::migrate_cli::{
    BACKUP_FILE, Consent, MigrateArgs, MigrateEnv, MigrateOutput, STAMP_FILE, migrate_in,
};
use dmux::model::{Backend, Health, Lifecycle, ServerEpoch};
use dmux::operations::{OperationEnv, tmux_bootstrap};
use dmux::output::OutputFormat;
use dmux::registry::{Registry, RegistryConfig};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Scratch environment

struct Scratch {
    data: tempfile::TempDir,
    locks: tempfile::TempDir,
    state: tempfile::TempDir,
}

impl Scratch {
    fn new() -> Scratch {
        Scratch {
            data: tempfile::tempdir().unwrap(),
            locks: tempfile::tempdir().unwrap(),
            state: tempfile::tempdir().unwrap(),
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

    fn spaces(&self) -> Vec<dmux::registry::SpaceRow> {
        self.registry().spaces().unwrap()
    }

    fn revision(&self) -> u64 {
        self.registry().authority_head().unwrap().revision
    }

    fn backup(&self) -> std::path::PathBuf {
        self.data.path().join(BACKUP_FILE)
    }

    fn stamp(&self) -> std::path::PathBuf {
        self.data.path().join(STAMP_FILE)
    }

    fn history(&self) -> History {
        History::new(self.state.path())
    }
}

/// A terminal that is not there. `--yes` short-circuits before consent is
/// consulted, so every case that passes this and omits `--yes` is asserting
/// §7.4's non-TTY rule.
struct NoTerminal;

impl Consent for NoTerminal {
    fn confirm(&self, _preview: &str) -> Option<bool> {
        None
    }
}

/// A terminal that answers, and remembers what it was shown.
struct Answers {
    yes: bool,
    shown: RefCell<Vec<String>>,
}

impl Consent for Answers {
    fn confirm(&self, preview: &str) -> Option<bool> {
        self.shown.borrow_mut().push(preview.to_string());
        Some(self.yes)
    }
}

fn preview_args() -> MigrateArgs {
    MigrateArgs {
        commit: false,
        yes: false,
        previous_sessions: BTreeMap::new(),
    }
}

fn commit_args() -> MigrateArgs {
    MigrateArgs {
        commit: true,
        yes: true,
        previous_sessions: BTreeMap::new(),
    }
}

fn document(out: &MigrateOutput) -> serde_json::Value {
    serde_json::from_str(&out.stdout).unwrap_or_else(|e| panic!("{e}: {:?}", out.stdout))
}

/// Every row of the printed mapping, by disposition.
fn rows<'a>(doc: &'a serde_json::Value, disposition: &str) -> Vec<&'a serde_json::Value> {
    doc["result"]["spaces"]
        .as_array()
        .unwrap_or_else(|| panic!("no spaces in {doc}"))
        .iter()
        .filter(|row| row["disposition"] == disposition)
        .collect()
}

// ---------------------------------------------------------------------------
// tmux, against a real scratch server

struct TmuxScratch {
    ns: String,
}

impl TmuxScratch {
    fn start(tag: &str) -> TmuxScratch {
        TmuxScratch {
            ns: format!("dmux-p11mig-{tag}-{}", std::process::id()),
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

fn bootstrapped(scratch: &Scratch, tmux: &TmuxScratch) {
    match tmux_bootstrap(&scratch.env(), &tmux.ns).unwrap() {
        dmux::operations::TmuxBootstrapOutcome::Bootstrapped { .. } => {}
        other => panic!("fresh server must bootstrap: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Wez, against a fake mux

/// A `wezterm cli` that answers `list` from an in-memory pane table and
/// applies the fork CAS rename to it. `wez_multi_window` is the only shape
/// the migration must decide about without a fork build.
struct FakeMux {
    epoch: Uuid,
    /// `(window_id, tab_id, pane_id, workspace)`.
    panes: RefCell<Vec<(u64, u64, u64, String)>>,
}

impl FakeMux {
    fn new(panes: &[(u64, u64, u64, &str)]) -> FakeMux {
        FakeMux {
            epoch: Uuid::new_v4(),
            panes: RefCell::new(
                panes
                    .iter()
                    .map(|(w, t, p, ws)| (*w, *t, *p, ws.to_string()))
                    .collect(),
            ),
        }
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
        let mut out = vec![row(0, 0, 0, &format!("dmux:system:{}", self.epoch))];
        out.extend(
            self.panes
                .borrow()
                .iter()
                .map(|(w, t, p, ws)| row(*w, *t, *p, ws)),
        );
        format!("[{}]", out.join(","))
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
            stderr: format!(
                "ERROR wezterm > {}{reason}; terminating",
                dmux::backend::wez::CAS_FAILED_MARKER
            )
            .into(),
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
    fn probe(&self, _socket: &str, _pid: Option<u32>) -> ProbeOutcome {
        ProbeOutcome::Connectable
    }

    fn run(&self, invocation: &WezInvocation, _: Duration) -> Result<RunOutput, RunError> {
        let argv = &invocation.argv;
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
        Ok(self.cas_rename(argv))
    }
}

/// A wezterm CLI that must never be reached. Every tmux-only case takes it,
/// so a migration cannot quietly probe the other backend.
struct NeverWez;

impl WezRunner for NeverWez {
    fn probe(&self, socket: &str, _pid: Option<u32>) -> ProbeOutcome {
        panic!("a tmux-only migration probed the wez socket {socket}")
    }
    fn run(&self, invocation: &WezInvocation, _: Duration) -> Result<RunOutput, RunError> {
        panic!("a tmux-only migration ran wezterm: {:?}", invocation.argv)
    }
}

fn never_wez() -> WezCli<NeverWez> {
    WezCli {
        bin: "/nonexistent/wezterm".into(),
        config: "/nonexistent/wez.lua".into(),
        runner: NeverWez,
    }
}

/// Register the managed Wez instance the way the service would, so the
/// driver finds the socket and published epoch exactly as `ls` does.
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

fn env<'a, R: WezRunner>(
    ops: &'a OperationEnv,
    wez: WezCli<R>,
    history: History,
    consent: &'a dyn Consent,
) -> MigrateEnv<'a, R> {
    MigrateEnv {
        ops,
        wez,
        history,
        consent,
    }
}

// ---------------------------------------------------------------------------
// §17.7 — the preview mutates nothing and prints a deterministic mapping

#[test]
fn a_preview_prints_the_mapping_and_adopts_nothing() {
    let scratch = Scratch::new();
    let tmux = TmuxScratch::start("preview");
    tmux.tmux(&["new-session", "-d", "-s", "beta"]);
    tmux.tmux(&["new-session", "-d", "-s", "alpha"]);
    bootstrapped(&scratch, &tmux);
    let (alpha, beta) = (tmux.session_id("alpha"), tmux.session_id("beta"));
    let before = scratch.revision();

    let out = migrate_in(
        env(&scratch.env(), never_wez(), scratch.history(), &NoTerminal),
        Some(OutputFormat::Json),
        preview_args(),
    );
    assert_eq!(out.status, ExitStatus::Success, "{}", out.stdout);
    let doc = document(&out);
    assert_eq!(doc["ok"], true);
    assert_eq!(doc["action"], "migrate");
    assert_eq!(doc["result"]["committed"], false);

    // Deterministic order: backend, then name. `beta` was created first and
    // lists first natively; the plan still puts `alpha` above it.
    let adopt = rows(&doc, "adopt");
    assert_eq!(adopt.len(), 2, "{doc}");
    assert_eq!(adopt[0]["name"], "alpha");
    assert_eq!(adopt[1]["name"], "beta");
    assert_eq!(adopt[0]["backend"], "tmux");

    // Nothing was adopted, nothing stamped, no backup written, and the
    // authority chain did not move.
    assert_eq!(scratch.spaces().len(), 0);
    assert_eq!(scratch.revision(), before);
    assert!(!scratch.backup().exists(), "a preview wrote a backup");
    assert!(!scratch.stamp().exists(), "a preview recorded a cutover");
    for session in [&alpha, &beta] {
        assert_eq!(tmux.marker(session, "@dmux_space_uid"), None);
    }

    // Two previews of one authority are byte-identical.
    let again = migrate_in(
        env(&scratch.env(), never_wez(), scratch.history(), &NoTerminal),
        Some(OutputFormat::Json),
        preview_args(),
    );
    assert_eq!(again.stdout, out.stdout);
}

#[test]
fn the_proposed_space_no_is_the_permanent_counter_not_the_row_index() {
    let scratch = Scratch::new();
    let tmux = TmuxScratch::start("rowindex");
    tmux.tmux(&["new-session", "-d", "-s", "legacy"]);
    bootstrapped(&scratch, &tmux);

    // Spend the first two numbers, so the one candidate is row 1 of the
    // plan and can only be Space 3. §17.7: current row indices are not
    // preserved as SpaceNo values.
    {
        let mut registry = scratch.registry();
        let instance = registry
            .backend_instance_for_backend(Backend::Tmux)
            .unwrap()
            .unwrap();
        for name in ["spent-a", "spent-b"] {
            registry
                .reserve_space(name, instance, Uuid::new_v4())
                .unwrap();
        }
    }

    let doc = document(&migrate_in(
        env(&scratch.env(), never_wez(), scratch.history(), &NoTerminal),
        Some(OutputFormat::Json),
        preview_args(),
    ));
    let adopt = rows(&doc, "adopt");
    assert_eq!(adopt.len(), 1, "{doc}");
    assert_eq!(adopt[0]["row"], 1, "the candidate is the first printed row");
    assert_eq!(adopt[0]["space_no"], 3, "{doc}");

    // And the promise is kept: the committed number is the proposed one.
    let out = migrate_in(
        env(&scratch.env(), never_wez(), scratch.history(), &NoTerminal),
        Some(OutputFormat::Json),
        commit_args(),
    );
    assert_eq!(out.status, ExitStatus::Success, "{}", out.stdout);
    let committed = document(&out);
    assert_eq!(rows(&committed, "adopt")[0]["space_no"], 3);
}

// ---------------------------------------------------------------------------
// §7.4 — confirmation

#[test]
fn json_commit_without_yes_is_one_confirmation_document_and_exit_5() {
    let scratch = Scratch::new();
    let tmux = TmuxScratch::start("jsonconfirm");
    tmux.tmux(&["new-session", "-d", "-s", "legacy"]);
    bootstrapped(&scratch, &tmux);
    let session = tmux.session_id("legacy");
    let before = scratch.revision();

    let out = migrate_in(
        env(
            &scratch.env(),
            never_wez(),
            scratch.history(),
            &Answers {
                yes: true,
                shown: RefCell::new(Vec::new()),
            },
        ),
        Some(OutputFormat::Json),
        MigrateArgs {
            commit: true,
            yes: false,
            previous_sessions: BTreeMap::new(),
        },
    );
    assert_eq!(out.status, ExitStatus::ConfirmationRequired);
    assert_eq!(
        out.stdout.trim_end().lines().count(),
        1,
        "exactly one document: {:?}",
        out.stdout
    );
    let doc = document(&out);
    assert_eq!(doc["ok"], false);
    assert_eq!(doc["action"], "migrate");
    assert_eq!(doc["errors"][0]["code"], "confirmation_required");
    // The plan travels inside it: `--yes` must not be agreement to
    // something the caller never saw.
    assert_eq!(rows(&doc, "adopt").len(), 1, "{doc}");

    assert_eq!(scratch.spaces().len(), 0);
    assert_eq!(scratch.revision(), before);
    assert!(!scratch.backup().exists());
    assert_eq!(tmux.marker(&session, "@dmux_space_uid"), None);
}

#[test]
fn a_human_commit_without_a_terminal_or_with_a_decline_exits_5() {
    for (label, consent) in [
        ("no terminal", Box::new(NoTerminal) as Box<dyn Consent>),
        (
            "declined",
            Box::new(Answers {
                yes: false,
                shown: RefCell::new(Vec::new()),
            }),
        ),
    ] {
        let scratch = Scratch::new();
        let tmux = TmuxScratch::start("decline");
        tmux.tmux(&["new-session", "-d", "-s", "legacy"]);
        bootstrapped(&scratch, &tmux);
        let before = scratch.revision();

        let out = migrate_in(
            env(
                &scratch.env(),
                never_wez(),
                scratch.history(),
                consent.as_ref(),
            ),
            None,
            MigrateArgs {
                commit: true,
                yes: false,
                previous_sessions: BTreeMap::new(),
            },
        );
        assert_eq!(out.status, ExitStatus::ConfirmationRequired, "{label}");
        assert_eq!(scratch.spaces().len(), 0, "{label}");
        assert_eq!(scratch.revision(), before, "{label}");
        assert!(!scratch.backup().exists(), "{label}");
        assert!(!scratch.stamp().exists(), "{label}");
    }
}

#[test]
fn the_interactive_prompt_shows_the_plan_it_is_asking_about() {
    let scratch = Scratch::new();
    let tmux = TmuxScratch::start("prompt");
    tmux.tmux(&["new-session", "-d", "-s", "legacy"]);
    bootstrapped(&scratch, &tmux);
    let consent = Answers {
        yes: true,
        shown: RefCell::new(Vec::new()),
    };

    let out = migrate_in(
        env(&scratch.env(), never_wez(), scratch.history(), &consent),
        None,
        MigrateArgs {
            commit: true,
            yes: false,
            previous_sessions: BTreeMap::new(),
        },
    );
    assert_eq!(out.status, ExitStatus::Success, "{}", out.stderr);
    let shown = consent.shown.borrow();
    assert_eq!(shown.len(), 1);
    assert!(shown[0].contains("legacy"), "{:?}", shown[0]);
    assert!(shown[0].contains("adopt"), "{:?}", shown[0]);
    assert!(shown[0].contains(BACKUP_FILE), "{:?}", shown[0]);
}

// ---------------------------------------------------------------------------
// §17.1 and §17.8 — back up, then batch-adopt

#[test]
fn a_commit_backs_up_first_prints_the_location_and_batch_adopts() {
    let scratch = Scratch::new();
    let tmux = TmuxScratch::start("commit");
    tmux.tmux(&["new-session", "-d", "-s", "alpha"]);
    tmux.tmux(&["new-session", "-d", "-s", "beta"]);
    bootstrapped(&scratch, &tmux);

    let out = migrate_in(
        env(&scratch.env(), never_wez(), scratch.history(), &NoTerminal),
        Some(OutputFormat::Json),
        commit_args(),
    );
    assert_eq!(out.status, ExitStatus::Success, "{}", out.stdout);
    let doc = document(&out);
    assert_eq!(doc["result"]["committed"], true);
    assert_eq!(doc["result"]["adopted"], 2);
    assert_eq!(doc["result"]["recorded"], true);
    assert_eq!(
        doc["result"]["backup"]["path"],
        scratch.backup().display().to_string()
    );
    assert_eq!(doc["result"]["backup"]["created"], true);

    // The backup is a readable registry, and it predates the adoptions:
    // it holds none of the Spaces the commit went on to create.
    assert!(scratch.backup().exists());
    let copy = Registry::open(RegistryConfig::new(scratch.backup(), scratch.locks.path())).unwrap();
    assert_eq!(copy.spaces().unwrap().len(), 0, "the backup is post-hoc");

    // Both sessions are adopted, active + unstamped, with distinct identity.
    let spaces = scratch.spaces();
    assert_eq!(spaces.len(), 2);
    let mut names: Vec<&str> = spaces.iter().map(|s| s.logical_name.as_str()).collect();
    names.sort();
    assert_eq!(names, ["alpha", "beta"]);
    for space in &spaces {
        assert_eq!(space.lifecycle, Lifecycle::Active);
        assert_eq!(space.health, Health::Unstamped);
        assert!(
            tmux.marker(&tmux.session_id(&space.logical_name), "@dmux_space_uid")
                .is_some()
        );
    }
}

// ---------------------------------------------------------------------------
// Case 45 — "migrate once"

#[test]
fn a_second_commit_is_a_clean_no_op_not_a_second_migration() {
    let scratch = Scratch::new();
    let tmux = TmuxScratch::start("once");
    tmux.tmux(&["new-session", "-d", "-s", "legacy"]);
    bootstrapped(&scratch, &tmux);

    let first = migrate_in(
        env(&scratch.env(), never_wez(), scratch.history(), &NoTerminal),
        Some(OutputFormat::Json),
        commit_args(),
    );
    assert_eq!(first.status, ExitStatus::Success, "{}", first.stdout);
    let space = scratch.spaces().pop().unwrap();
    let session = tmux.session_id("legacy");
    let stamped_uid = tmux.marker(&session, "@dmux_space_uid");
    let after_first = scratch.revision();

    let second = migrate_in(
        env(&scratch.env(), never_wez(), scratch.history(), &NoTerminal),
        Some(OutputFormat::Json),
        commit_args(),
    );
    assert_eq!(second.status, ExitStatus::Success, "{}", second.stdout);
    let doc = document(&second);
    assert_eq!(doc["ok"], true);
    assert_eq!(doc["result"]["already_migrated"], true);
    assert_eq!(doc["result"]["committed"], false);

    assert_eq!(scratch.spaces().len(), 1, "a second migration ran");
    assert_eq!(scratch.spaces()[0].space_uid, space.space_uid);
    assert_eq!(scratch.revision(), after_first, "the chain advanced");
    assert_eq!(tmux.marker(&session, "@dmux_space_uid"), stamped_uid);
}

/// The stamp is a convenience, not the guarantee. With it deleted, the
/// binding is what makes re-migration impossible: an adopted resource is
/// managed, so it is never a candidate again.
#[test]
fn deleting_the_cutover_record_still_cannot_re_adopt_a_bound_resource() {
    let scratch = Scratch::new();
    let tmux = TmuxScratch::start("nostamp");
    tmux.tmux(&["new-session", "-d", "-s", "legacy"]);
    bootstrapped(&scratch, &tmux);

    let first = migrate_in(
        env(&scratch.env(), never_wez(), scratch.history(), &NoTerminal),
        Some(OutputFormat::Json),
        commit_args(),
    );
    assert_eq!(first.status, ExitStatus::Success, "{}", first.stdout);
    let space = scratch.spaces().pop().unwrap();
    let session = tmux.session_id("legacy");
    let stamped_uid = tmux.marker(&session, "@dmux_space_uid");
    let after_first = scratch.revision();

    std::fs::remove_file(scratch.stamp()).unwrap();

    let again = migrate_in(
        env(&scratch.env(), never_wez(), scratch.history(), &NoTerminal),
        Some(OutputFormat::Json),
        commit_args(),
    );
    assert_eq!(again.status, ExitStatus::Success, "{}", again.stdout);
    let doc = document(&again);
    assert_eq!(doc["result"]["adopted"], 0, "{doc}");
    assert!(rows(&doc, "adopt").is_empty(), "{doc}");
    let managed = rows(&doc, "managed");
    assert_eq!(managed.len(), 1);
    assert_eq!(managed[0]["space_uid"], space.space_uid.0.to_string());

    assert_eq!(scratch.spaces().len(), 1);
    assert_eq!(scratch.revision(), after_first);
    assert_eq!(tmux.marker(&session, "@dmux_space_uid"), stamped_uid);

    // The pre-migration backup is never replaced by post-migration state.
    assert_eq!(doc["result"]["backup"]["created"], false);
    let copy = Registry::open(RegistryConfig::new(scratch.backup(), scratch.locks.path())).unwrap();
    assert_eq!(copy.spaces().unwrap().len(), 0);
}

// ---------------------------------------------------------------------------
// §17.9 — normalize or quarantine, never a managed multi-window Space

#[test]
fn a_multi_window_wez_workspace_is_quarantined_unmanaged_with_its_remedy() {
    let scratch = Scratch::new();
    let mux = FakeMux::new(&[
        (1, 10, 100, "single"),
        (3, 13, 104, "spread"),
        (4, 14, 105, "spread"),
    ]);
    let wez = wez_registered(&scratch, &mux);

    let out = migrate_in(
        env(&scratch.env(), wez, scratch.history(), &NoTerminal),
        Some(OutputFormat::Json),
        commit_args(),
    );
    assert_eq!(out.status, ExitStatus::Success, "{}", out.stdout);
    let doc = document(&out);

    let quarantined = rows(&doc, "quarantine");
    assert_eq!(quarantined.len(), 1, "{doc}");
    assert_eq!(quarantined[0]["name"], "spread");
    assert_eq!(quarantined[0]["reason"], "multi_window");
    assert!(
        quarantined[0]["remedy"]
            .as_str()
            .unwrap()
            .contains("dmux repair normalize native:wez:"),
        "{doc}"
    );

    // The one-window workspace migrated; the multi-window one was left
    // exactly as it was, and no managed Space claims it.
    assert_eq!(doc["result"]["adopted"], 1);
    let spaces = scratch.spaces();
    assert_eq!(spaces.len(), 1);
    assert_eq!(spaces[0].logical_name, "single");
    assert_ne!(spaces[0].health, Health::MultiWindow);
    assert!(
        mux.workspaces().contains(&"spread".to_string()),
        "{:?}",
        mux.workspaces()
    );
}

// ---------------------------------------------------------------------------
// §17.6 — a complete owner scan, or no commit at all

#[test]
fn an_indeterminate_owner_scan_blocks_the_commit_and_emits_a_document() {
    let scratch = Scratch::new();
    let tmux = TmuxScratch::start("indeterminate");
    tmux.tmux(&["new-session", "-d", "-s", "legacy"]);
    bootstrapped(&scratch, &tmux);

    // A registered Wez instance with no reachable server: the scan
    // establishes nothing, and an unseen resource is not an absent one.
    struct Down;
    impl WezRunner for Down {
        fn probe(&self, _socket: &str, _pid: Option<u32>) -> ProbeOutcome {
            ProbeOutcome::Failed {
                detail: "no such socket".into(),
            }
        }
        fn run(&self, _: &WezInvocation, _: Duration) -> Result<RunOutput, RunError> {
            Err(RunError::Io {
                detail: "no such socket".into(),
            })
        }
    }
    {
        let mut registry = scratch.registry();
        let instance = registry
            .register_backend_instance(Backend::Wez, Some("/run/dmux/absent.sock"), None)
            .unwrap();
        registry
            .publish_backend_server(
                instance,
                ServerEpoch(Uuid::new_v4()),
                Some(1),
                Some("t"),
                None,
                None,
            )
            .unwrap();
    }
    let wez = WezCli {
        bin: "/opt/homebrew/bin/wezterm".into(),
        config: "/etc/dmux/wez.lua".into(),
        runner: Down,
    };
    let before = scratch.revision();

    let out = migrate_in(
        env(&scratch.env(), wez, scratch.history(), &NoTerminal),
        Some(OutputFormat::Json),
        commit_args(),
    );
    assert_eq!(out.status, ExitStatus::Unavailable, "{}", out.stdout);
    assert_eq!(out.stdout.trim_end().lines().count(), 1);
    let doc = document(&out);
    assert_eq!(doc["ok"], false);
    assert_eq!(doc["action"], "migrate");
    assert_eq!(doc["errors"][0]["code"], "provider_unavailable");
    assert!(
        doc["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("indeterminate"),
        "{doc}"
    );

    // Nothing was adopted, not even on the backend that did scan cleanly.
    assert_eq!(scratch.spaces().len(), 0);
    assert_eq!(scratch.revision(), before);
    assert!(!scratch.backup().exists());
    assert!(!scratch.stamp().exists());
    assert_eq!(
        tmux.marker(&tmux.session_id("legacy"), "@dmux_space_uid"),
        None
    );
}

// ---------------------------------------------------------------------------
// §17.10 — duplicate cross-backend names stay independently addressable

#[test]
fn a_duplicate_cross_backend_name_is_flagged_and_never_shares_identity() {
    let scratch = Scratch::new();
    let tmux = TmuxScratch::start("dup");
    tmux.tmux(&["new-session", "-d", "-s", "shared"]);
    bootstrapped(&scratch, &tmux);
    let mux = FakeMux::new(&[(1, 10, 100, "shared")]);
    let wez = wez_registered(&scratch, &mux);

    let preview_wez = WezCli {
        bin: "/opt/homebrew/bin/wezterm".into(),
        config: "/etc/dmux/wez.lua".into(),
        runner: &mux,
    };
    let preview = document(&migrate_in(
        env(&scratch.env(), preview_wez, scratch.history(), &NoTerminal),
        Some(OutputFormat::Json),
        preview_args(),
    ));
    let flagged = rows(&preview, "adopt");
    assert_eq!(flagged.len(), 2, "{preview}");
    for row in &flagged {
        assert_eq!(row["name"], "shared");
        assert_eq!(row["duplicate_name"], true, "{preview}");
    }
    // Different proposed numbers before anything is allocated.
    assert_ne!(flagged[0]["space_no"], flagged[1]["space_no"]);

    let out = migrate_in(
        env(&scratch.env(), wez, scratch.history(), &NoTerminal),
        Some(OutputFormat::Json),
        commit_args(),
    );
    // One lands; `operations::require_no_cross_backend_name` refuses the
    // second rather than minting a colliding managed name. Partial (7),
    // with the refusal typed and the remedy named.
    assert_eq!(out.status, ExitStatus::Partial, "{}", out.stdout);
    let doc = document(&out);
    assert_eq!(doc["result"]["adopted"], 1, "{doc}");
    assert_eq!(doc["errors"][0]["code"], "name_conflict");
    assert!(
        doc["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("--name"),
        "{doc}"
    );
    assert!(
        doc["errors"][0]["target"]
            .as_str()
            .unwrap()
            .starts_with("native:")
    );

    // The refusal is not a cutover: `migrate` stays available for the retry.
    assert_eq!(doc["result"]["recorded"], false);
    assert!(!scratch.stamp().exists());
    assert_eq!(scratch.spaces().len(), 1);
}

// ---------------------------------------------------------------------------
// §17.11 — history converts only when unambiguous

#[test]
fn history_converts_unambiguous_names_and_warns_on_the_rest() {
    let scratch = Scratch::new();
    let tmux = TmuxScratch::start("history");
    tmux.tmux(&["new-session", "-d", "-s", "solo"]);
    tmux.tmux(&["new-session", "-d", "-s", "twin"]);
    bootstrapped(&scratch, &tmux);
    // `twin` also exists on Wez, so the name is ambiguous by §17.10.
    let mux = FakeMux::new(&[(1, 10, 100, "twin")]);
    let wez = wez_registered(&scratch, &mux);

    let label = {
        let mut registry = scratch.registry();
        let local = registry.identity().unwrap().host_uid;
        registry.set_host_label(local, "macie").unwrap();
        "macie".to_string()
    };
    let previous_sessions = BTreeMap::from([
        (label.clone(), "solo".to_string()),
        (format!("{label}:current"), "twin".to_string()),
        ("archie".to_string(), "gone".to_string()),
    ]);

    let out = migrate_in(
        env(&scratch.env(), wez, scratch.history(), &NoTerminal),
        Some(OutputFormat::Json),
        MigrateArgs {
            commit: true,
            yes: true,
            previous_sessions,
        },
    );
    // The duplicate `twin` pair refuses one adoption, which is a partial —
    // but the history decisions are what this case is about.
    let doc = document(&out);
    let history = doc["result"]["history"].as_array().unwrap();
    let outcome = |key: &str| {
        history
            .iter()
            .find(|entry| entry["key"] == key)
            .unwrap_or_else(|| panic!("no history entry {key}: {doc}"))
    };

    // Unambiguous: converts to a real SpaceUid.
    let solo = outcome(&label);
    assert_eq!(solo["outcome"], "convert");
    assert_eq!(solo["name"], "solo");
    assert!(solo["space_uid"].is_string(), "{doc}");
    let solo_uid: Uuid = solo["space_uid"].as_str().unwrap().parse().unwrap();

    // Ambiguous: dropped with the candidate count, never a guessed identity.
    let twin = outcome(&format!("{label}:current"));
    assert_eq!(twin["outcome"], "drop_ambiguous");
    assert_eq!(twin["candidates"], 2);
    assert!(twin["space_uid"].is_null());

    // Missing: no Space of that name on this authority.
    let gone = outcome("archie");
    assert_eq!(gone["outcome"], "drop_missing");
    assert!(gone["space_uid"].is_null());

    // The converted entry actually landed in `dmux -` history, keyed by
    // stable identity rather than by the legacy name.
    let local = scratch.registry().identity().unwrap().host_uid;
    assert_eq!(
        scratch.history().current(local).map(|uid| uid.0),
        Some(solo_uid)
    );
}
