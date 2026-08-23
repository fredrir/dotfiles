//! Shared scaffolding: every registry lives in a scratch tempdir with an
//! injected lock dir — tests never touch the real registry, runtime, or
//! state directories.

use std::time::Duration;

use dmux::model::{Backend, BackendInstanceUid};
use dmux::registry::{
    BusyPolicy, NativeBindingSpec, NativeKind, Registry, RegistryConfig, SpaceReservation,
};
use uuid::Uuid;

pub struct Scratch {
    /// Keep the tempdir alive for the test's duration.
    pub dir: tempfile::TempDir,
    pub config: RegistryConfig,
}

pub fn scratch() -> Scratch {
    let dir = tempfile::tempdir().unwrap();
    let config = RegistryConfig {
        db_path: dir.path().join("registry.sqlite3"),
        lock_dir: dir.path().join("locks"),
        busy: fast_busy(),
    };
    Scratch { dir, config }
}

/// Contract semantics, test-speed timings: the production default stays at
/// the contract's 5000 ms busy timeout; tests only shrink the waits.
pub fn fast_busy() -> BusyPolicy {
    BusyPolicy {
        busy_timeout: Duration::from_millis(500),
        attempts: 5,
        retry_base: Duration::from_millis(2),
    }
}

pub fn open(config: &RegistryConfig) -> Registry {
    Registry::open(config.clone()).unwrap()
}

pub fn tmux_instance(reg: &mut Registry) -> BackendInstanceUid {
    reg.register_backend_instance(Backend::Tmux, None, None)
        .unwrap()
}

pub fn reserve(reg: &mut Registry, name: &str, instance: BackendInstanceUid) -> SpaceReservation {
    reg.reserve_space(name, instance, Uuid::new_v4()).unwrap()
}

pub fn finalize(reg: &mut Registry, reservation: &SpaceReservation, token: &str) {
    reg.finalize_create(
        reservation.space_uid,
        reservation.operation_uid,
        &NativeBindingSpec {
            native_token: token.to_string(),
            native_kind: NativeKind::TmuxSessionId,
            server_epoch: None,
        },
    )
    .unwrap()
}

// ---------------------------------------------------------------------------
// A scripted `wezterm cli` for the Wez legs of adoption, rebind and crash
// reconciliation: answers `list` from an in-memory pane table and applies
// the ADR 006 CAS rename to it with the server-side compare the fork
// performs. The fork build is not a test dependency; what has to be provable
// without it is exactly the argv dmux issues and the zero-mutation refusals.

use std::cell::RefCell;

use dmux::backend::wez::{
    CAS_FAILED_MARKER, CAS_MISSING_PDU_STDERR, ProbeOutcome, RunError, RunOutput, WezInvocation,
    WezRunner,
};

/// Whether the fake server carries the fork CAS verb. A stock codec-45
/// server rejects PDU ident 63 with a frozen stderr reason.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Cas {
    Fork,
    Stock,
}

pub struct FakeMux {
    pub epoch: Uuid,
    cas: Cas,
    /// `(window_id, tab_id, pane_id, workspace)`.
    panes: RefCell<Vec<(u64, u64, u64, String)>>,
    /// Everything this mux was asked, in order: a socket probe as
    /// `["probe", socket]`, otherwise the `wezterm cli` argv.
    commands: RefCell<Vec<Vec<String>>>,
    /// When set, every non-probe CAS answers `workspace_mismatch` naming this
    /// as the window's actual workspace — a concurrent external rename that
    /// landed between dmux's scan and its CAS.
    race: RefCell<Option<String>>,
}

impl FakeMux {
    pub fn new(cas: Cas, panes: &[(u64, u64, u64, &str)]) -> FakeMux {
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
            race: RefCell::new(None),
        }
    }

    /// A pane that appeared after construction — what a test needs when
    /// the workspace name is only known once the registry minted it.
    pub fn add_pane(&self, window: u64, tab: u64, pane: u64, workspace: &str) {
        self.panes
            .borrow_mut()
            .push((window, tab, pane, workspace.to_string()));
    }

    /// Make the next real CAS lose to an external rename to `actual`.
    pub fn race_to(&self, actual: &str) {
        *self.race.borrow_mut() = Some(actual.to_string());
    }

    pub fn commands(&self) -> Vec<Vec<String>> {
        self.commands.borrow().clone()
    }

    /// The `rename-workspace` invocations that were not the capability
    /// probe (the probe targets window id `u64::MAX`).
    pub fn cas_calls(&self) -> Vec<Vec<String>> {
        self.commands()
            .into_iter()
            .filter(|argv| {
                argv.iter().any(|a| a == "rename-workspace")
                    && !argv.iter().any(|a| a == &u64::MAX.to_string())
            })
            .collect()
    }

    pub fn workspaces(&self) -> Vec<String> {
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
        if let Some(raced) = self.race.borrow().as_deref() {
            return failed(format!(
                "workspace_mismatch window_id={window} actual=\"{raced}\""
            ));
        }
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
