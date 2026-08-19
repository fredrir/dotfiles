//! Black-box coverage for the destructive verbs' §7.4 rules.
//!
//! Three groups live here. The first pins the confirmation contract that
//! every `-y/--yes` command shares — a non-TTY changes nothing and exits 5 —
//! for `group rm`, `split rm`, `repair normalize`, `host forget`, `recovery
//! abort` and Wez-first `rm`, none of which had a test. The second pins case
//! 44: a deprecated row index can never quietly become a stable ID, on
//! either side of the gate. `tests/json_envelope.rs` owns the JSON envelope
//! half of `repair normalize`.
//!
//! The third drives `rm` where it actually destroys things: a scratch
//! registry the CLI resolves through XDG plus a scratch tmux server on its
//! own `-L` namespace. Refusals and hints can be proven hermetically;
//! "removed it, tombstoned it, and left nothing half-deleted" cannot.
//!
//! The first two groups are hermetic like `tests/cli.rs`: PATH holds stubs,
//! XDG points at a temp tree, and `DMUX_DRY_RUN=1` turns the legacy execs
//! into printed plans. The third cannot be — it needs the real tmux — so it
//! is isolated by namespace instead: every Space, session and registry row
//! it touches was created by the test that touches it.
//!
//! Owned by the P6 mutation agent (plan §19.3).

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::FromRawFd;
use std::os::unix::net::UnixListener;
use std::process::{Command, Output, Stdio};

use dmux::backend::InventoryScope;
use dmux::backend::tmux::TmuxProvider;
use dmux::model::{Backend, Lifecycle, ServerEpoch, SpaceUid};
use dmux::operations::{
    CreateRequest, CreatedSpace, OperationEnv, TmuxBootstrapOutcome, create_space, tmux_bootstrap,
};
use dmux::registry::{Registry, RegistryConfig};
use serde_json::Value;
use tempfile::TempDir;
use uuid::Uuid;

struct Sandbox {
    bin: TempDir,
    state: TempDir,
}

impl Sandbox {
    fn empty() -> Sandbox {
        Sandbox {
            bin: tempfile::tempdir().unwrap(),
            state: tempfile::tempdir().unwrap(),
        }
    }

    /// The same two-session fake tmux `tests/cli.rs` uses (alpha, then beta)
    /// plus the one-workspace fake wezterm, so the merged legacy listing is
    /// row 1 `work`, row 2 `alpha`, row 3 `beta`.
    fn legacy_listing() -> Sandbox {
        let sandbox = Sandbox::empty();
        sandbox.stub(
            "tmux",
            "case \"$1\" in\n\
             list-sessions) printf 'alpha|1700000000|2|1\\nbeta|1700000100|1|0\\n' ;;\n\
             esac",
        );
        sandbox.stub(
            "wezterm",
            r#"printf '[{"window_id":1,"pane_id":1,"workspace":"work"}]'"#,
        );
        sandbox
    }

    fn stub(&self, name: &str, script: &str) {
        let path = self.bin.path().join(name);
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dmux"));
        command
            .args(args)
            .env("PATH", self.bin.path())
            .env("XDG_DATA_HOME", self.state.path())
            .env("XDG_STATE_HOME", self.state.path())
            .env("XDG_RUNTIME_DIR", self.state.path())
            .env("DMUX_DRY_RUN", "1")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("DMUX_WEZ_FIRST")
            .env_remove("DMUX_HOST_UID")
            .env_remove("DMUX_SPACE_UID")
            .env_remove("DMUX_GROUP_REF")
            .env_remove("DMUX_SPLIT_REF")
            .env_remove("WEZTERM_UNIX_SOCKET")
            .env_remove("WEZTERM_PANE")
            .env_remove("NO_COLOR");
        command
    }

    fn dmux(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("dmux runs")
    }

    /// The Wez-first gate on, which is what the canary machine exports.
    fn wez_first(&self, args: &[&str]) -> Output {
        self.command(args)
            .env("DMUX_WEZ_FIRST", "1")
            .output()
            .expect("dmux runs")
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn code(output: &Output) -> i32 {
    output.status.code().expect("dmux exits normally")
}

/// A ref shaped like a live Group/Split. Every command below refuses before
/// it is ever resolved, which is the point: confirmation comes first.
const GROUP_REF: &str = "demo/g00000000-0000-4000-8000-000000000001.wz-1";
const SPLIT_REF: &str = "demo/p00000000-0000-4000-8000-000000000001.wz-1";

// ---------------------------------------------------------------------------
// §7.4: no terminal, no confirmation, no mutation, exit 5

#[test]
fn group_rm_without_a_terminal_refuses() {
    let sandbox = Sandbox::empty();
    let output = sandbox.dmux(&["group", "rm", GROUP_REF]);
    assert_eq!(code(&output), 5, "{}", stderr(&output));
    assert!(stderr(&output).contains("confirmation required"));
    assert_eq!(stdout(&output), "");
}

#[test]
fn split_rm_without_a_terminal_refuses() {
    let sandbox = Sandbox::empty();
    let output = sandbox.dmux(&["split", "rm", SPLIT_REF]);
    assert_eq!(code(&output), 5, "{}", stderr(&output));
    assert!(stderr(&output).contains("confirmation required"));
    assert_eq!(stdout(&output), "");
}

/// `dmux host forget` takes the host as a positional, which shares clap's
/// `host` id with the global `-H/--host`; a legacy host name keeps that
/// collision from failing the parse before the confirmation is reached.
#[test]
fn host_forget_without_a_terminal_refuses() {
    let sandbox = Sandbox::empty();
    let output = sandbox.dmux(&["host", "forget", "archie"]);
    assert_eq!(code(&output), 5, "{}", stderr(&output));
    assert!(stderr(&output).contains("confirmation required"));
    assert_eq!(stdout(&output), "");
}

#[test]
fn recovery_abort_without_a_terminal_refuses() {
    let sandbox = Sandbox::empty();
    let output = sandbox.dmux(&["recovery", "abort"]);
    assert_eq!(code(&output), 5, "{}", stderr(&output));
    assert!(stderr(&output).contains("requires confirmation"));
    assert_eq!(stdout(&output), "");
}

/// `repair normalize` is the one confirmation that sits behind a scan, so it
/// needs a listening endpoint and a fake `wezterm cli list` before the
/// prompt is reachable at all. The preview still prints; the mutation does
/// not run.
#[test]
fn repair_normalize_previews_then_refuses_without_a_terminal() {
    let sandbox = Sandbox::empty();
    let scratch = tempfile::tempdir_in("/tmp").unwrap();
    let socket = scratch.path().join("wez.sock");
    let _listener = UnixListener::bind(&socket).unwrap();
    sandbox.stub(
        "wezterm",
        &format!(
            "printf '{}'",
            multi_window_listing("00000000-0000-4000-8000-0000000000ff")
        ),
    );

    let socket = socket.display().to_string();
    let data = scratch.path().join("data").display().to_string();
    let lock = scratch.path().join("lock").display().to_string();
    fs::create_dir_all(&data).unwrap();
    fs::create_dir_all(&lock).unwrap();
    let output = sandbox
        .command(&[
            "repair",
            "normalize",
            "--socket",
            &socket,
            "--data-dir",
            &data,
            "--lock-dir",
            &lock,
        ])
        .env("DMUX_WEZ_BIN", sandbox.bin.path().join("wezterm"))
        .output()
        .expect("dmux runs");

    assert_eq!(code(&output), 5, "{}", stderr(&output));
    assert!(stdout(&output).contains("legacy"), "{}", stdout(&output));
    assert!(stderr(&output).contains("confirmation required"));
}

/// One `wezterm cli list` response: the mandatory epoch sentinel plus a
/// workspace spread over two windows, which is what makes it a normalize
/// target (plan §10.3).
fn multi_window_listing(epoch: &str) -> String {
    format!(r#"[{{"window_id":9,"tab_id":9,"pane_id":9,"workspace":"dmux:system:{epoch}"}},"#)
        + r#"{"window_id":1,"tab_id":1,"pane_id":1,"workspace":"legacy"},"#
        + r#"{"window_id":2,"tab_id":2,"pane_id":2,"workspace":"legacy"}]"#
}

// ---------------------------------------------------------------------------
// Wez-first `rm`: case 41

#[test]
fn wez_first_rm_without_a_terminal_changes_nothing_and_exits_5() {
    let sandbox = Sandbox::empty();
    let output = sandbox.wez_first(&["rm", "alpha"]);
    assert_eq!(code(&output), 5, "{}", stderr(&output));
    assert_eq!(stdout(&output), "");
    assert!(stderr(&output).contains("re-run with --yes"));
}

#[test]
fn wez_first_rm_in_json_emits_one_confirmation_document() {
    let sandbox = Sandbox::empty();
    let output = sandbox.wez_first(&["--format", "json", "rm", "alpha", "beta"]);
    assert_eq!(code(&output), 5, "{}", stderr(&output));
    let document: serde_json::Value =
        serde_json::from_str(stdout(&output).trim()).expect("one JSON document on stdout");
    assert_eq!(document["schema_version"], 1);
    assert_eq!(document["action"], "rm");
    assert_eq!(document["ok"], false);
    assert_eq!(document["result"], serde_json::Value::Null);
    assert_eq!(document["errors"].as_array().unwrap().len(), 1);
    assert_eq!(document["errors"][0]["code"], "confirmation_required");
    assert_eq!(document["errors"][0]["target"], "alpha, beta");
}

/// The declined-prompt half of case 41 shares its exit with the non-TTY
/// half, so a --yes-less JSON run and a --yes-less pipe both land on 5 and
/// leave the authority untouched.
///
/// The JSON spelling is the sharp one: its envelope has to carry an
/// `authority_revision`, and reading one by opening the registry would
/// allocate this host's permanent HostUid/RegistryUid — an explicit §17.2
/// migration step performed by a destructive verb the operator declined.
#[test]
fn wez_first_rm_refusal_never_reaches_the_authority() {
    for spelling in [
        vec!["rm", "--all"],
        vec!["--format", "json", "rm", "--all"],
        vec!["--format", "json", "rm", "alpha"],
    ] {
        let sandbox = Sandbox::empty();
        let output = sandbox.wez_first(&spelling);
        assert_eq!(code(&output), 5, "{spelling:?}: {}", stderr(&output));
        // Nothing resolved, so nothing wrote a registry into the fresh state
        // tree either.
        let stray = fs::read_dir(sandbox.state.path()).unwrap().count();
        assert_eq!(stray, 0, "{spelling:?}: a refused rm created state");
    }
}

/// Case 41's envelope still has to report a revision. With no database
/// there is none, and fabricating one by creating the database is the
/// change the case forbids: zero is the honest answer.
#[test]
fn a_refused_json_rm_reports_revision_zero_rather_than_allocating_one() {
    let sandbox = Sandbox::empty();
    let output = sandbox.wez_first(&["--format", "json", "rm", "alpha"]);
    let document: Value = serde_json::from_str(stdout(&output).trim()).expect("one document");
    assert_eq!(document["authority_revision"], 0, "{document}");
    assert!(
        !sandbox.state.path().join("dmux/registry.sqlite3").exists(),
        "a refused rm created the authority database"
    );
}

#[test]
fn wez_first_rm_rejects_the_legacy_window_flag() {
    let sandbox = Sandbox::empty();
    let output = sandbox.wez_first(&["rm", "--yes", "-w", "2", "alpha"]);
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(stderr(&output).contains("dmux split rm"));
}

// ---------------------------------------------------------------------------
// Case 44: row indices never silently become stable IDs

/// `--row` is an index into the Wez-first listing, so it refuses outright
/// when either backend's inventory is indeterminate — an index into an
/// incomplete listing is exactly the silent retarget the case forbids.
#[test]
fn wez_first_row_refuses_to_index_an_incomplete_listing() {
    let sandbox = Sandbox::empty();
    let output = sandbox.wez_first(&["rm", "--yes", "--row", "3"]);
    assert_eq!(code(&output), 6, "{}", stderr(&output));
    assert!(stderr(&output).contains("--row cannot index an incomplete listing"));
}

#[test]
fn wez_first_row_zero_is_a_usage_error() {
    let sandbox = Sandbox::empty();
    let output = sandbox.wez_first(&["rm", "--yes", "--row", "0"]);
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(stderr(&output).contains("rows start at 1"));
}

/// Bare digits are a permanent SpaceNo under the gate, never a row index:
/// `3` is looked up as Space number 3 and simply is not found here.
#[test]
fn wez_first_bare_digits_are_a_space_number() {
    let sandbox = Sandbox::empty();
    let output = sandbox.wez_first(&["rm", "--yes", "3"]);
    assert_eq!(code(&output), 3, "{}", stderr(&output));
    assert!(!stderr(&output).contains("row"), "{}", stderr(&output));
}

/// Legacy side: the index still resolves exactly as it always did — the
/// frozen `cli::a_filtered_listing_keeps_the_merged_indices` compares this
/// stdout byte for byte — but it now says so on stderr.
///
/// The replacement it offers must be the NAME. This listing is wez rows then
/// tmux rows; the gated one is managed rows by permanent SpaceNo then
/// unmanaged, so the same N routinely names a different resource across the
/// gate — here legacy row 3 is `beta` while gated `--row 3` is whatever the
/// third managed Space is. Advising `--row N` on a destructive verb would be
/// the silent retarget case 44 exists to prevent.
#[test]
fn a_legacy_index_removal_keeps_its_plan_and_gains_a_hint() {
    let sandbox = Sandbox::legacy_listing();
    let output = sandbox.dmux(&["rm", "--yes", "3"]);
    assert_eq!(stdout(&output), "would run: tmux kill-session -t '=beta'\n");
    assert!(
        stderr(&output).contains("matched listing row 3 ('beta')"),
        "{}",
        stderr(&output)
    );
    assert!(stderr(&output).contains("row indices go away next release"));
    assert!(
        stderr(&output).contains("use the name 'beta'"),
        "{}",
        stderr(&output)
    );
    assert!(
        !stderr(&output).contains("--row"),
        "the hint may not point at the other index space: {}",
        stderr(&output)
    );
}

#[test]
fn a_legacy_index_rename_keeps_its_plan_and_gains_a_hint() {
    let sandbox = Sandbox::legacy_listing();
    let output = sandbox.dmux(&["rename", "2", "fresh"]);
    assert_eq!(
        stdout(&output),
        "would run: tmux rename-session -t '=alpha' fresh\n"
    );
    assert!(
        stderr(&output).contains("matched listing row 2 ('alpha')"),
        "{}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("use the name 'alpha'") && !stderr(&output).contains("--row"),
        "{}",
        stderr(&output)
    );
}

/// A session genuinely named `3` is a name, not an index, so it must not be
/// warned about — `unit::list::tests::a_numeric_name_shadows_the_index`
/// pins the resolution and this pins the hint that reads it.
#[test]
fn a_numeric_session_name_is_not_reported_as_an_index() {
    let sandbox = Sandbox::empty();
    sandbox.stub(
        "tmux",
        "case \"$1\" in\n\
         list-sessions) printf '3|1700000000|1|0\\nbeta|1700000100|1|0\\n' ;;\n\
         esac",
    );
    let output = sandbox.dmux(&["rm", "--yes", "3"]);
    assert_eq!(stdout(&output), "would run: tmux kill-session -t '=3'\n");
    assert!(
        !stderr(&output).contains("matched listing row"),
        "{}",
        stderr(&output)
    );
}

// ---------------------------------------------------------------------------
// Wez-first `rename` grammar (plan §7.1)

#[test]
fn rename_rejects_a_selector_flag_beside_two_positionals() {
    let sandbox = Sandbox::empty();
    let output = sandbox.wez_first(&["rename", "--name", "old", "new", "extra"]);
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(stderr(&output).contains("--name/--row already select the Space"));
}

#[test]
fn rename_rejects_a_new_name_outside_the_managed_grammar() {
    let sandbox = Sandbox::empty();
    let output = sandbox.wez_first(&["rename", "old", "not a name"]);
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(stderr(&output).contains("invalid new name"));
}

// ---------------------------------------------------------------------------
// Wez-first `rm` against a real scratch owner (plan §7.4, §14; cases 41, 42, 44)
//
// Everything above proves refusals, which a fake backend can carry. Nothing
// above can prove the opposite claim — that a removal reached the native
// resource, tombstoned the record, and left no half-deleted row behind — so
// this group runs the verb for real against a scratch registry and a scratch
// tmux server, and reads the durable answer out of both afterwards.

/// A scratch owner: its own registry under `XDG_DATA_HOME`, its own tmux
/// server on its own `-L` namespace. Seeded in this process through the same
/// fenced operations the owner uses, then driven through the real binary,
/// because the exit status and the confirmation contract are the subject.
struct Owner {
    ns: String,
    data: TempDir,
    locks: TempDir,
}

impl Owner {
    fn start(tag: &str) -> Owner {
        let owner = Owner {
            ns: format!("dmux-rmcli-{tag}-{}", std::process::id()),
            data: tempfile::tempdir().unwrap(),
            locks: tempfile::tempdir().unwrap(),
        };
        fs::create_dir_all(owner.data.path().join("dmux")).unwrap();
        // `seed` is never adopted, so the server outlives the last managed
        // Space: "the server is stopped" is a different verdict entirely and
        // must not be what these cases accidentally measure.
        let out = Command::new("tmux")
            .args([
                "-L",
                &owner.ns,
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                "seed",
            ])
            .env("DMUX_RUNTIME_DIR", owner.locks.path())
            .output()
            .expect("tmux starts");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        owner.epoch();
        owner
    }

    fn env(&self) -> OperationEnv {
        OperationEnv {
            db_path: self.data.path().join("dmux/registry.sqlite3"),
            lock_dir: self.locks.path().to_path_buf(),
        }
    }

    fn epoch(&self) -> ServerEpoch {
        match tmux_bootstrap(&self.env(), &self.ns).unwrap() {
            TmuxBootstrapOutcome::Bootstrapped { epoch }
            | TmuxBootstrapOutcome::AlreadyBound { epoch }
            | TmuxBootstrapOutcome::Rebound { epoch, .. } => epoch,
        }
    }

    fn create(&self, name: &str) -> CreatedSpace {
        let scope = InventoryScope {
            backend: Backend::Tmux,
            endpoint: self.ns.clone(),
            expected_epoch: Some(self.epoch()),
        };
        create_space(
            &self.env(),
            &TmuxProvider::new(self.ns.clone()),
            &scope,
            Backend::Tmux,
            &CreateRequest {
                request_uid: Uuid::new_v4(),
                name: name.to_string(),
                cwd: None,
                program: vec!["sh".into(), "-c".into(), "exec sleep 300".into()],
                helper_bin: env!("CARGO_BIN_EXE_pane-bootstrap").to_string(),
            },
        )
        .unwrap_or_else(|error| panic!("create {name}: {error}"))
    }

    /// The exact crash §14 journals for: durable `deleting` intent written,
    /// native session still alive, no acknowledgement. Only the unfinished
    /// operation this leaves behind may ever finish the removal.
    fn wedge(&self, space_uid: SpaceUid) {
        Registry::open(RegistryConfig::new(self.env().db_path, self.env().lock_dir))
            .unwrap()
            .begin_remove(space_uid, Uuid::new_v4())
            .unwrap();
    }

    fn lifecycles(&self) -> Vec<(u64, String, Lifecycle)> {
        Registry::open(RegistryConfig::new(self.env().db_path, self.env().lock_dir))
            .unwrap()
            .spaces()
            .unwrap()
            .into_iter()
            .map(|row| (row.space_no.get(), row.logical_name, row.lifecycle))
            .collect()
    }

    /// Everything still alive on the scratch server, `seed` included.
    fn sessions(&self) -> Vec<String> {
        let out = Command::new("tmux")
            .args(["-L", &self.ns, "list-sessions", "-F", "#{session_name}"])
            .output()
            .unwrap();
        let mut names: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect();
        names.sort();
        names
    }

    /// The real binary, resolved against this owner's registry alone. The
    /// inherited PATH stays: these cases need the real tmux, and the wez
    /// backend is never invoked because no wez instance is registered here.
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dmux"));
        command
            .args(args)
            .env("XDG_DATA_HOME", self.data.path())
            .env("XDG_STATE_HOME", self.data.path())
            .env("XDG_RUNTIME_DIR", self.locks.path())
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
            .env_remove("NO_COLOR");
        command
    }

    fn dmux(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("dmux runs")
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.ns, "kill-server"])
            .output();
    }
}

fn document(output: &Output) -> Value {
    serde_json::from_str(stdout(output).trim())
        .unwrap_or_else(|error| panic!("{error}: {:?}", stdout(output)))
}

/// One run whose stdin is a real terminal, answering the prompt with
/// `answer`. §7.4 only prompts on a TTY, so nothing else can reach the
/// preview that names what is about to be destroyed.
fn answer_prompt(command: &mut Command, answer: &str) -> Output {
    let mut controller: libc::c_int = -1;
    let mut device: libc::c_int = -1;
    let opened = unsafe {
        libc::openpty(
            &mut controller,
            &mut device,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(opened, 0, "openpty: {}", std::io::Error::last_os_error());
    let mut writer = unsafe { fs::File::from_raw_fd(controller) };
    let child = command
        .stdin(unsafe { Stdio::from_raw_fd(device) })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("dmux runs");
    writer.write_all(format!("{answer}\n").as_bytes()).unwrap();
    child.wait_with_output().expect("dmux exits")
}

/// A native removal that failed after the `deleting` intent was journaled
/// leaves the Space, its name and its permanent number unusable until some
/// verb finishes the delete. `rm` is that verb: §10.2's remove step 3 is a
/// resume of the journaled operation, not a second one, and there is no
/// other CLI entry point to it. Without it the row is wedged forever — `rm`
/// and `rename` both answer `repair_required` and `dmux repair` has no
/// applicable subcommand.
#[test]
fn a_wedged_remove_is_finished_by_the_verb_that_started_it() {
    let owner = Owner::start("wedged");
    let space = owner.create("wedged");
    owner.wedge(space.space_uid);
    assert_eq!(
        owner.lifecycles(),
        vec![(1, "wedged".to_string(), Lifecycle::Deleting)]
    );
    assert!(owner.sessions().contains(&"wedged".to_string()));

    let output = owner.dmux(&["--format", "json", "rm", "--yes", "wedged"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let doc = document(&output);
    assert_eq!(doc["ok"], true, "{doc}");
    assert_eq!(doc["errors"].as_array().unwrap().len(), 0, "{doc}");
    assert_eq!(doc["result"][0]["removed"], true, "{doc}");

    assert_eq!(
        owner.lifecycles(),
        vec![(1, "wedged".to_string(), Lifecycle::Deleted)]
    );
    assert_eq!(owner.sessions(), vec!["seed".to_string()]);
}

/// Plan §7.4: `--all` is every Space on the selected host. Membership is the
/// durable record, not a live scan, so a `deleting` row — which `dmux ls`
/// still lists — is swept like any other. Reporting `ok:true` with an empty
/// `errors[]` over a host that still holds Spaces is the one answer a script
/// cannot recover from.
#[test]
fn rm_all_sweeps_every_space_the_listing_still_shows() {
    let owner = Owner::start("all");
    let half_deleted = owner.create("half-deleted");
    owner.create("ordinary");
    owner.wedge(half_deleted.space_uid);

    let output = owner.dmux(&["--format", "json", "rm", "--all", "--yes"]);
    let doc = document(&output);
    assert_eq!(code(&output), 0, "{}: {}", doc, stderr(&output));
    assert_eq!(doc["result"].as_array().unwrap().len(), 2, "{doc}");

    assert!(
        owner
            .lifecycles()
            .iter()
            .all(|(_, _, lifecycle)| *lifecycle == Lifecycle::Deleted),
        "{:?}",
        owner.lifecycles()
    );
    assert_eq!(owner.sessions(), vec!["seed".to_string()]);
}

/// `rm` is the only verb that can tombstone, so it must not require the
/// target to be presentable first: a session someone killed outside dmux is
/// exactly the record that has to be removable. The backend still has to
/// answer — §14 never waives reachability — but a complete scan that simply
/// does not contain the session is absence, not unavailability.
#[test]
fn an_externally_killed_session_is_still_removable() {
    let owner = Owner::start("orphan");
    owner.create("orphan");
    let killed = Command::new("tmux")
        .args(["-L", &owner.ns, "kill-session", "-t", "=orphan"])
        .output()
        .unwrap();
    assert!(killed.status.success());
    assert_eq!(owner.sessions(), vec!["seed".to_string()]);

    let output = owner.dmux(&["--format", "json", "rm", "--yes", "orphan"]);
    let doc = document(&output);
    assert_eq!(code(&output), 0, "{doc}: {}", stderr(&output));
    assert_eq!(
        owner.lifecycles(),
        vec![(1, "orphan".to_string(), Lifecycle::Deleted)]
    );
}

/// A stopped server is a determinate answer, and it is not the same answer
/// as a scan that established nothing: §16.3 code 6 is "the provider is
/// unavailable", and the remedy — start it — belongs in the message. The
/// connect resolver's "could not be proven by complete live owner scans" is
/// false about a server that was never scanned.
#[test]
fn a_stopped_server_names_itself_instead_of_blaming_the_scan() {
    let owner = Owner::start("stopped");
    owner.create("parked");
    Command::new("tmux")
        .args(["-L", &owner.ns, "kill-server"])
        .output()
        .unwrap();

    let output = owner.dmux(&["--format", "json", "rm", "--yes", "parked"]);
    let doc = document(&output);
    assert_eq!(code(&output), 6, "{doc}");
    assert_eq!(doc["errors"][0]["code"], "provider_unavailable", "{doc}");
    assert_eq!(doc["errors"][0]["target"], "parked", "{doc}");
    assert_eq!(
        doc["errors"][0]["message"], "the managed tmux server is stopped; start it, then remove",
        "{doc}"
    );
    assert_eq!(
        owner.lifecycles(),
        vec![(1, "parked".to_string(), Lifecycle::Active)]
    );
}

/// Two spellings of one Space are one removal. Killing it twice reports the
/// second attempt as a failure the caller never caused — §16.3 code 0 covers
/// the repeat as a documented idempotent no-op — and the batch that fully
/// satisfied the request must not exit 7.
#[test]
fn two_spellings_of_one_space_are_one_removal() {
    let owner = Owner::start("dup");
    owner.create("dup");

    let output = owner.dmux(&["--format", "json", "rm", "--yes", "1", "dup"]);
    let doc = document(&output);
    assert_eq!(code(&output), 0, "{doc}: {}", stderr(&output));
    assert_eq!(doc["ok"], true, "{doc}");
    assert_eq!(doc["errors"].as_array().unwrap().len(), 0, "{doc}");
    assert_eq!(doc["result"].as_array().unwrap().len(), 1, "{doc}");
    assert_eq!(owner.sessions(), vec!["seed".to_string()]);
}

/// Case 42 wants every target preflighted, so an unresolvable one joins the
/// report instead of ending the run at the first failure — and each error
/// carries the exact word the caller typed, which is the only thing they can
/// act on in a batch. Nothing is mutated: one bad target still voids the run.
#[test]
fn every_bad_target_in_a_batch_is_reported_and_nothing_is_mutated() {
    let owner = Owner::start("preflight");
    owner.create("kept");

    let output = owner.dmux(&["--format", "json", "rm", "--yes", "kept", "nope1", "nope2"]);
    let doc = document(&output);
    assert_eq!(code(&output), 3, "{doc}");
    let targets: Vec<&str> = doc["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|error| error["target"].as_str().unwrap_or("<none>"))
        .collect();
    assert_eq!(targets, vec!["nope1", "nope2"], "{doc}");
    assert_eq!(
        owner.lifecycles(),
        vec![(1, "kept".to_string(), Lifecycle::Active)]
    );
    assert_eq!(
        owner.sessions(),
        vec!["kept".to_string(), "seed".to_string()]
    );
}

/// §17.13's compatibility escape has to work on the hosts that need it. The
/// managed prefix of the listing IS the durable record in permanent SpaceNo
/// order, so an index inside it cannot move whatever the scans said; only
/// the unmanaged tail depends on a complete scan, and indexing an unproven
/// tail is the silent retarget case 44 forbids. Refusing both halves — as a
/// host with only one registered backend used to — makes the escape useless
/// on exactly the machines mid-migration.
#[test]
fn row_indexes_the_managed_prefix_even_with_one_registered_backend() {
    let owner = Owner::start("row");
    owner.create("first");
    owner.create("second");

    let output = owner.dmux(&["--format", "json", "rm", "--yes", "--row", "2"]);
    let doc = document(&output);
    assert_eq!(code(&output), 0, "{doc}: {}", stderr(&output));
    assert_eq!(doc["result"][0]["name"], "second", "{doc}");
    // Case 44: the substitution is echoed as the stable ref it resolved to.
    assert!(
        stderr(&output).contains("--row 2 resolved to 2 (second)"),
        "{}",
        stderr(&output)
    );

    // `seed` is unmanaged, so row 3 lives in the tail the wez scan could not
    // establish — that one still refuses.
    let tail = owner.dmux(&["--format", "json", "rm", "--yes", "--row", "3"]);
    assert_eq!(code(&tail), 6, "{}", stdout(&tail));
    assert_eq!(document(&tail)["errors"][0]["code"], "provider_unavailable");
    assert_eq!(
        owner.lifecycles(),
        vec![
            (1, "first".to_string(), Lifecycle::Active),
            (2, "second".to_string(), Lifecycle::Deleted),
        ]
    );
}

/// §14: the prompt prints stable ref, name, backend, owner, Group count and
/// Split count — of the Space, not of the selector. `--row 2` and `--all`
/// say nothing about what they resolved to, and case 44 is not satisfied by
/// an echo the operator only reads after committing. Declining still exits 5
/// and changes nothing (case 41).
#[test]
fn the_prompt_names_the_space_before_it_asks() {
    let owner = Owner::start("prompt");
    owner.create("victim");

    let output = answer_prompt(&mut owner.command(&["rm", "--row", "1"]), "n");
    assert_eq!(code(&output), 5, "{}", stderr(&output));
    let text = stderr(&output);
    let preview = text.find("dmux: remove ").expect(&text);
    let prompt = text.find("[y/N]").expect(&text);
    assert!(
        preview < prompt,
        "the preview must precede the prompt: {text}"
    );
    assert!(text[preview..prompt].contains("victim"), "{text}");
    assert!(text[preview..prompt].contains("tmux"), "{text}");
    assert!(text.contains("rm declined; nothing changed"), "{text}");

    assert_eq!(
        owner.lifecycles(),
        vec![(1, "victim".to_string(), Lifecycle::Active)]
    );
    assert_eq!(
        owner.sessions(),
        vec!["seed".to_string(), "victim".to_string()]
    );
}
