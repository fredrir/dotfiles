//! Flag-on dispatch coverage (ADR 012 WS-E.4; review report 08 §2).
//!
//! The review reproduced every gated verb by calling the library entry
//! point the gate dispatches to, never the gate itself: with
//! `WEZ_FIRST_BY_DEFAULT = false` nobody had run the binary with
//! `DMUX_WEZ_FIRST=1` on the child. Each test here drives the REAL binary
//! twice with one argv: once gated (`DMUX_WEZ_FIRST=1`, no opt-out) and
//! once legacy (`DMUX_WEZ_FIRST` removed, `DMUX_LEGACY_POLICY=1`, the §21
//! rollback environment). The gated run must show the gate arm was taken
//! — the §16.2 envelope, or a typed refusal only the Wez-first path can
//! produce — and the legacy run must behave as the baseline `cli::` tests
//! document. Verbs the gate does not touch (`repair`, `_context`) are run
//! both ways too, to pin that the flag does not change their dispatch.
//!
//! Hermetic: the registry lives under a scratch `XDG_DATA_HOME`, every
//! lock and socket under a scratch `DMUX_RUNTIME_DIR`, PATH holds
//! recording stand-ins plus — only where a verified scan is the point —
//! the real `tmux`, which talks to a private `-L` scratch server killed
//! on drop. No wez mux server is ever contacted.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use dmux::backend::InventoryScope;
use dmux::backend::tmux::TmuxProvider;
use dmux::model::{Backend, BackendInstanceUid, Health, ServerEpoch};
use dmux::operations::{
    CreateRequest, CreatedSpace, OperationEnv, OwnerCreateTarget, TmuxBootstrapOutcome,
    create_space_owner_fenced, tmux_bootstrap,
};
use dmux::registry::{NativeBindingSpec, NativeKind, Registry, RegistryConfig};
use serde_json::Value;
use tempfile::TempDir;
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Policy {
    /// `DMUX_WEZ_FIRST=1`, no opt-out: the canary host's environment.
    Gated,
    /// `DMUX_LEGACY_POLICY=1` with `DMUX_WEZ_FIRST` removed: the §21
    /// rollback environment, stated rather than assumed (ADR 011 D1).
    Legacy,
}

const BOTH: [Policy; 2] = [Policy::Gated, Policy::Legacy];

struct Sandbox {
    data: TempDir,
    bin: TempDir,
    state: TempDir,
    runtime: TempDir,
}

impl Sandbox {
    fn new() -> Sandbox {
        let runtime = tempfile::tempdir().unwrap();
        fs::set_permissions(runtime.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let sandbox = Sandbox {
            data: tempfile::tempdir().unwrap(),
            bin: tempfile::tempdir().unwrap(),
            state: tempfile::tempdir().unwrap(),
            runtime,
        };
        fs::create_dir_all(sandbox.data.path().join("dmux")).unwrap();
        sandbox
    }

    /// The binary with every persistent path pinned to this sandbox and
    /// every ambient marker scrubbed. PATH is the stub directory alone;
    /// `with_tmux` appends the real tmux.
    fn command(&self, policy: Policy, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dmux"));
        command
            .args(args)
            .env("PATH", self.bin.path())
            .env("HOME", self.state.path())
            .env("XDG_DATA_HOME", self.data.path())
            .env("XDG_STATE_HOME", self.state.path())
            .env("XDG_RUNTIME_DIR", self.runtime.path())
            .env("DMUX_RUNTIME_DIR", self.runtime.path())
            .env_remove("DMUX_DRY_RUN")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("DMUX_CONTEXT_VERSION")
            .env_remove("DMUX_BACKEND")
            .env_remove("DMUX_HOST_UID")
            .env_remove("DMUX_SPACE_UID")
            .env_remove("DMUX_SPACE_NO")
            .env_remove("DMUX_DOMAIN")
            .env_remove("DMUX_SERVER_EPOCH")
            .env_remove("DMUX_GROUP_REF")
            .env_remove("DMUX_SPLIT_REF")
            .env_remove("WEZTERM_UNIX_SOCKET")
            .env_remove("WEZTERM_PANE")
            .env_remove("TERM_PROGRAM")
            .env_remove("NO_COLOR")
            .stdin(Stdio::null());
        match policy {
            Policy::Gated => {
                command.env("DMUX_WEZ_FIRST", "1");
                command.env_remove("DMUX_LEGACY_POLICY");
            }
            Policy::Legacy => {
                command.env_remove("DMUX_WEZ_FIRST");
                command.env("DMUX_LEGACY_POLICY", "1");
            }
        }
        command
    }

    fn run(&self, policy: Policy, args: &[&str]) -> Output {
        self.command(policy, args).output().expect("dmux runs")
    }

    /// The same, with the real `tmux` reachable behind the stubs.
    fn run_with_tmux(&self, policy: Policy, args: &[&str]) -> Output {
        self.command(policy, args)
            .env("PATH", self.path_with_real_tmux())
            .output()
            .expect("dmux runs")
    }

    fn path_with_real_tmux(&self) -> String {
        format!(
            "{}:{}",
            self.bin.path().display(),
            real_tmux_dir().display()
        )
    }

    fn env(&self) -> OperationEnv {
        OperationEnv {
            db_path: self.data.path().join("dmux/registry.sqlite3"),
            lock_dir: self.runtime.path().to_path_buf(),
        }
    }

    fn registry(&self) -> Registry {
        let env = self.env();
        Registry::open(RegistryConfig::new(env.db_path, env.lock_dir)).unwrap()
    }

    /// A stand-in on the stub PATH that records every invocation and
    /// answers nothing; "never called" is proven by the witness's absence.
    fn recording_stub(&self, name: &str) -> PathBuf {
        let witness = self.state.path().join(format!("{name}-ran"));
        let stub = self.bin.path().join(name);
        fs::write(
            &stub,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 1\n",
                witness.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
        witness
    }

    /// Register the scratch server's namespace and publish the epoch it
    /// really serves, through the production bootstrap (instance state E).
    fn publish_tmux(&self, server: &ScratchTmux) -> ServerEpoch {
        match tmux_bootstrap(&self.env(), &server.namespace).unwrap() {
            TmuxBootstrapOutcome::Bootstrapped { epoch }
            | TmuxBootstrapOutcome::AlreadyBound { epoch }
            | TmuxBootstrapOutcome::Rebound { epoch, .. } => epoch,
        }
    }

    /// Register the namespace with NO published epoch: the row
    /// `dmux-mux-start.sh` leaves when coordination never completes.
    fn register_unpublished_tmux(&self, server: &ScratchTmux) -> BackendInstanceUid {
        let mut registry = self.registry();
        let instance = registry
            .register_backend_instance(Backend::Tmux, Some(&server.namespace), None)
            .unwrap();
        assert!(
            registry
                .backend_server(instance)
                .unwrap()
                .server_epoch
                .is_none()
        );
        instance
    }

    fn published_epoch(&self) -> ServerEpoch {
        self.registry()
            .backend_server(self.tmux_instance())
            .unwrap()
            .server_epoch
            .expect("the tmux instance has a published epoch")
    }

    fn tmux_instance(&self) -> BackendInstanceUid {
        self.registry()
            .backend_instance_for_backend(Backend::Tmux)
            .unwrap()
            .expect("a tmux instance is registered")
    }

    /// A managed Space on the published scratch server, created through
    /// the production owner-fenced path with a real pane bootstrap.
    fn create(&self, server: &ScratchTmux, name: &str) -> CreatedSpace {
        let epoch = self.publish_tmux(server);
        let scope = InventoryScope::managed(Backend::Tmux, server.namespace.clone(), epoch);
        let provider = TmuxProvider::new(server.namespace.clone());
        create_space_owner_fenced(
            &self.env(),
            OwnerCreateTarget {
                backend: Backend::Tmux,
                instance: self.tmux_instance(),
                provider: &provider,
                scope: &scope,
            },
            None,
            false,
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

    fn authority_revision(&self) -> u64 {
        self.registry().authority_head().unwrap().revision
    }
}

/// The directory holding the real `tmux` on this process's PATH.
fn real_tmux_dir() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").expect("PATH"))
        .find(|dir| dir.join("tmux").is_file())
        .expect("a real tmux on PATH (the suite already depends on one)")
}

/// A tmux server on a private `-L` namespace, killed on drop. Its
/// environment carries the sandbox runtime dir so a pane bootstrap reaches
/// the broker FIFOs there; window titles are frozen so listings are
/// deterministic.
struct ScratchTmux {
    namespace: String,
}

impl ScratchTmux {
    fn start(sandbox: &Sandbox, tag: &str, session: &str) -> ScratchTmux {
        let server = ScratchTmux {
            namespace: format!("dmux-gated-{tag}-{}", std::process::id()),
        };
        let out = Command::new("tmux")
            .args(["-L", &server.namespace, "-f", "/dev/null"])
            .args(["new-session", "-d", "-s", session])
            .args([";", "set-option", "-g", "automatic-rename", "off"])
            .args([";", "set-option", "-g", "allow-rename", "off"])
            .env("DMUX_RUNTIME_DIR", sandbox.runtime.path())
            .output()
            .expect("tmux runs");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        server
    }

    fn tmux(&self, args: &[&str]) -> String {
        let out = Command::new("tmux")
            .args(["-L", &self.namespace])
            .args(args)
            .output()
            .expect("tmux runs");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn session_id(&self, name: &str) -> String {
        self.tmux(&["display-message", "-p", "-t", name, "#{session_id}"])
    }

    fn pane_id(&self, name: &str) -> String {
        self.tmux(&["display-message", "-p", "-t", name, "#{pane_id}"])
    }

    fn socket_path(&self) -> String {
        self.tmux(&["display-message", "-p", "#{socket_path}"])
    }

    fn sessions(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .tmux(&["list-sessions", "-F", "#{session_name}"])
            .lines()
            .map(str::to_string)
            .collect();
        names.sort();
        names
    }

    fn option(&self, session: &str, name: &str) -> String {
        self.tmux(&["show-options", "-t", session, "-qv", name])
    }
}

impl Drop for ScratchTmux {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.namespace, "kill-server"])
            .output();
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("dmux exits normally")
}

/// Case 43: stdout is exactly one schema-versioned document for `action`.
fn document(out: &Output, action: &str) -> Value {
    let text = stdout(out);
    assert_eq!(text.lines().count(), 1, "not one document: {text:?}");
    let doc: Value = serde_json::from_str(text.trim()).unwrap_or_else(|e| panic!("{e}: {text}"));
    assert_eq!(doc["schema_version"], 1, "{doc}");
    assert_eq!(doc["action"], action, "{doc}");
    assert!(doc["ok"].is_boolean(), "{doc}");
    assert!(doc["errors"].is_array(), "{doc}");
    doc
}

/// The legacy arm's typed usage refusal of a Wez-first-only flag, as the
/// baseline `cli::` tests pin it: exit 2, and the `require DMUX_WEZ_FIRST=1`
/// text — on stderr for a human run, inside the one document for
/// `--format json` (`main.rs` `refuse`).
fn legacy_refusal(out: &Output, action: &str, json: bool) {
    assert_eq!(
        code(out),
        2,
        "stdout {:?} stderr {:?}",
        stdout(out),
        stderr(out)
    );
    if json {
        let doc = document(out, action);
        assert_eq!(doc["ok"], false, "{doc}");
        assert_eq!(doc["errors"][0]["code"], "usage", "{doc}");
        assert!(
            doc["errors"][0]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("DMUX_WEZ_FIRST=1"),
            "{doc}"
        );
    } else {
        assert_eq!(stdout(out), "", "a refusal prints no human report");
        assert!(stderr(out).contains("DMUX_WEZ_FIRST=1"), "{}", stderr(out));
    }
}

/// `native:<backend>:<base64url-no-padding>` (plan §6.2), spelled by the
/// crate's own encoder so the test cannot drift from the grammar.
fn native_ref(backend: Backend, token: &str) -> String {
    dmux::output::native_ref(backend, token)
}

fn witness_lines(witness: &Path) -> String {
    fs::read_to_string(witness).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// ls: --format json, --tree, --all-hosts

/// Gated `ls --format json` is the §16.2 envelope for `list` rendered from a
/// verified scan of the published scratch server; the legacy arm refuses
/// the flag as one document.
#[test]
fn ls_format_json_is_the_envelope_under_the_gate_and_a_typed_refusal_under_legacy() {
    let sandbox = Sandbox::new();
    let server = ScratchTmux::start(&sandbox, "lsjson", "seed");
    let created = sandbox.create(&server, "proj");
    let wezterm = sandbox.recording_stub("wezterm");

    let gated = sandbox.run_with_tmux(Policy::Gated, &["ls", "--format", "json"]);
    let doc = document(&gated, "list");
    assert_eq!(code(&gated), 0, "{} / {}", stderr(&gated), doc);
    assert_eq!(doc["ok"], true, "{doc}");
    assert_eq!(
        doc["authority_revision"],
        sandbox.authority_revision(),
        "{doc}"
    );
    let rows = doc["result"].as_array().expect("result is the row array");
    let managed = rows
        .iter()
        .find(|row| row["name"] == "proj")
        .unwrap_or_else(|| panic!("the managed Space is listed: {doc}"));
    assert_eq!(managed["backend"], "tmux", "{managed}");
    assert_eq!(managed["observation"], "live", "{managed}");
    assert_eq!(
        managed["space_uid"],
        created.space_uid.0.to_string(),
        "{managed}"
    );
    let stranger = rows
        .iter()
        .find(|row| row["native_name"] == "seed")
        .unwrap_or_else(|| panic!("the stranger session is listed unmanaged: {doc}"));
    assert_eq!(stranger["managed"], false, "{stranger}");
    assert_eq!(stranger["provider"], "tmux", "{stranger}");
    assert_eq!(
        stranger["server_epoch"],
        sandbox.published_epoch().0.to_string(),
        "an unmanaged row on a published instance names the verified epoch: {stranger}"
    );
    assert!(
        !wezterm.exists(),
        "ls ran wezterm with no wez instance registered:\n{}",
        witness_lines(&wezterm)
    );

    let legacy = sandbox.run_with_tmux(Policy::Legacy, &["ls", "--format", "json"]);
    legacy_refusal(&legacy, "list", true);
}

/// Gated `ls --tree` expands the managed Space's live Groups from a pinned
/// scan — the `tree` key of the JSON row carries the created Group ref —
/// while legacy refuses `--tree` before any scan.
#[test]
fn ls_tree_expands_live_children_under_the_gate_and_is_refused_under_legacy() {
    let sandbox = Sandbox::new();
    let server = ScratchTmux::start(&sandbox, "lstree", "seed");
    let created = sandbox.create(&server, "proj");

    let human = sandbox.run_with_tmux(Policy::Gated, &["ls", "--tree"]);
    assert_eq!(code(&human), 0, "{}", stderr(&human));
    assert!(stdout(&human).contains("proj"), "{}", stdout(&human));
    assert!(
        stdout(&human).contains(&created.group_ref),
        "the tree names the live Group: {}",
        stdout(&human)
    );

    let gated = sandbox.run_with_tmux(Policy::Gated, &["ls", "--tree", "--format", "json"]);
    let doc = document(&gated, "list");
    assert_eq!(code(&gated), 0, "{}", stderr(&gated));
    let row = doc["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "proj")
        .unwrap_or_else(|| panic!("{doc}"))
        .clone();
    assert_eq!(
        row["tree"][0]["group_ref"],
        created.group_ref.as_str(),
        "{row}"
    );
    assert_eq!(
        row["tree"][0]["splits"][0]["split_ref"],
        created.split_ref.as_str(),
        "{row}"
    );

    let legacy = sandbox.run_with_tmux(Policy::Legacy, &["ls", "--tree"]);
    legacy_refusal(&legacy, "list", false);
    let legacy_json = sandbox.run_with_tmux(Policy::Legacy, &["ls", "--tree", "--format", "json"]);
    legacy_refusal(&legacy_json, "list", true);
}

/// Gated `ls --all-hosts` lists every enrolled host — here the local
/// authority alone — and legacy refuses the flag.
#[test]
fn ls_all_hosts_lists_the_enrolled_hosts_under_the_gate_and_is_refused_under_legacy() {
    let sandbox = Sandbox::new();
    let server = ScratchTmux::start(&sandbox, "lsall", "seed");
    let created = sandbox.create(&server, "proj");

    let gated = sandbox.run_with_tmux(Policy::Gated, &["ls", "--all-hosts", "--format", "json"]);
    let doc = document(&gated, "list");
    assert_eq!(code(&gated), 0, "{}", stderr(&gated));
    assert_eq!(doc["ok"], true, "{doc}");
    let rows = doc["result"].as_array().unwrap();
    assert!(
        rows.iter()
            .any(|row| row["space_uid"] == created.space_uid.0.to_string()),
        "{doc}"
    );
    let human = sandbox.run_with_tmux(Policy::Gated, &["ls", "--all-hosts"]);
    assert_eq!(code(&human), 0, "{}", stderr(&human));
    assert!(stdout(&human).contains("proj"), "{}", stdout(&human));

    let legacy = sandbox.run_with_tmux(Policy::Legacy, &["ls", "--all-hosts"]);
    legacy_refusal(&legacy, "list", false);
}

// ---------------------------------------------------------------------------
// rm --row, rename

/// Gated `rm --row` indexes the managed listing and, on a pipe without
/// `--yes`, answers the case-41 confirmation document and changes nothing;
/// legacy refuses the flag.
#[test]
fn rm_row_reaches_the_gated_confirmation_and_is_refused_under_legacy() {
    let sandbox = Sandbox::new();
    let server = ScratchTmux::start(&sandbox, "rmrow", "seed");
    let created = sandbox.create(&server, "proj");
    let revision = sandbox.authority_revision();

    let gated = sandbox.run_with_tmux(Policy::Gated, &["rm", "--row", "1", "--format", "json"]);
    let doc = document(&gated, "rm");
    assert_eq!(code(&gated), 5, "{} / {}", stderr(&gated), doc);
    assert_eq!(doc["ok"], false, "{doc}");
    assert_eq!(doc["errors"][0]["code"], "confirmation_required", "{doc}");
    assert_eq!(
        sandbox.authority_revision(),
        revision,
        "a refusal moved the chain"
    );
    assert!(server.sessions().contains(&"seed".to_string()));
    assert_eq!(
        sandbox
            .registry()
            .space(created.space_uid)
            .unwrap()
            .lifecycle,
        dmux::model::Lifecycle::Active
    );

    let legacy = sandbox.run_with_tmux(Policy::Legacy, &["rm", "--row", "1"]);
    legacy_refusal(&legacy, "rm", false);
    let legacy_json =
        sandbox.run_with_tmux(Policy::Legacy, &["rm", "--row", "1", "--format", "json"]);
    legacy_refusal(&legacy_json, "rm", true);
    assert_eq!(sandbox.authority_revision(), revision);
}

/// Gated `rename` is the registry-journaled rename with its envelope;
/// legacy refuses the Wez-first flags and keeps its positional plan.
#[test]
fn rename_is_journaled_under_the_gate_and_refused_with_a_format_under_legacy() {
    let sandbox = Sandbox::new();
    let server = ScratchTmux::start(&sandbox, "rename", "seed");
    let created = sandbox.create(&server, "proj");
    let revision = sandbox.authority_revision();

    let gated = sandbox.run_with_tmux(
        Policy::Gated,
        &["rename", "proj", "renamed", "--format", "json"],
    );
    let doc = document(&gated, "rename");
    assert_eq!(code(&gated), 0, "{} / {}", stderr(&gated), doc);
    assert_eq!(doc["ok"], true, "{doc}");
    let row = sandbox.registry().space(created.space_uid).unwrap();
    assert_eq!(row.logical_name, "renamed");
    assert!(
        sandbox.authority_revision() > revision,
        "the rename was not journaled"
    );

    let legacy = sandbox.run_with_tmux(
        Policy::Legacy,
        &["rename", "renamed", "again", "--format", "json"],
    );
    legacy_refusal(&legacy, "rename", true);
    let legacy_name =
        sandbox.run_with_tmux(Policy::Legacy, &["rename", "--name", "renamed", "again"]);
    legacy_refusal(&legacy_name, "rename", false);
    assert_eq!(
        sandbox
            .registry()
            .space(created.space_uid)
            .unwrap()
            .logical_name,
        "renamed"
    );
}

// ---------------------------------------------------------------------------
// adopt, migrate

/// Gated `adopt` resolves the instance through the registry: an
/// unpublished instance refuses `backend_epoch_changed` before any tmux
/// call (review finding #2 inverted, through the dispatch), a published one
/// answers the typed not-found for an absent session. Legacy refuses the
/// verb outright.
#[test]
fn adopt_refuses_an_unpublished_instance_under_the_gate_and_the_verb_under_legacy() {
    let sandbox = Sandbox::new();
    let server = ScratchTmux::start(&sandbox, "adopt", "legacy");
    sandbox.register_unpublished_tmux(&server);
    let tmux = sandbox.recording_stub("tmux");
    let native = native_ref(Backend::Tmux, &server.session_id("legacy"));
    let revision = sandbox.authority_revision();

    let gated = sandbox.run(Policy::Gated, &["adopt", &native, "--format", "json"]);
    let doc = document(&gated, "adopt");
    assert_eq!(code(&gated), 1, "{} / {}", stderr(&gated), doc);
    assert_eq!(doc["errors"][0]["code"], "backend_epoch_changed", "{doc}");
    assert!(
        !tmux.exists(),
        "adopt ran tmux against an unpublished instance:\n{}",
        witness_lines(&tmux)
    );
    assert_eq!(sandbox.authority_revision(), revision);
    assert_eq!(server.option("legacy", "@dmux_space_uid"), "");

    let legacy = sandbox.run(Policy::Legacy, &["adopt", &native, "--format", "json"]);
    legacy_refusal(&legacy, "adopt", true);
    let legacy_human = sandbox.run(Policy::Legacy, &["adopt", &native]);
    legacy_refusal(&legacy_human, "adopt", false);
    assert!(!tmux.exists());

    // Control: once the epoch the server really serves is published, the
    // same gated dispatch scans it (the stand-in is removed so the real
    // tmux answers) and reports what is there.
    sandbox.publish_tmux(&server);
    fs::remove_file(sandbox.bin.path().join("tmux")).unwrap();
    let absent = sandbox.run_with_tmux(
        Policy::Gated,
        &[
            "adopt",
            &native_ref(Backend::Tmux, "$999"),
            "--format",
            "json",
        ],
    );
    let doc = document(&absent, "adopt");
    assert_eq!(code(&absent), 3, "{} / {}", stderr(&absent), doc);
    assert_eq!(doc["errors"][0]["code"], "not_found", "{doc}");
    assert_eq!(server.sessions(), ["legacy"]);
}

/// Gated `migrate` previews the deterministic mapping without adopting,
/// then `--commit --yes` adopts through the normal lease and writes the
/// cutover stamp; legacy refuses both spellings and leaves no stamp.
#[test]
fn migrate_preview_and_commit_run_under_the_gate_and_are_refused_under_legacy() {
    let sandbox = Sandbox::new();
    let server = ScratchTmux::start(&sandbox, "migrate", "legacy");
    sandbox.publish_tmux(&server);
    let stamp = sandbox.data.path().join("dmux/migrated-v1.json");
    let revision = sandbox.authority_revision();

    for args in [
        &["migrate", "--format", "json"][..],
        &["migrate", "--commit", "--yes", "--format", "json"][..],
    ] {
        let legacy = sandbox.run_with_tmux(Policy::Legacy, args);
        legacy_refusal(&legacy, "migrate", true);
    }
    let legacy_human = sandbox.run_with_tmux(Policy::Legacy, &["migrate"]);
    legacy_refusal(&legacy_human, "migrate", false);
    assert!(!stamp.exists(), "a legacy refusal wrote the cutover stamp");
    assert_eq!(sandbox.authority_revision(), revision);

    let preview = sandbox.run_with_tmux(Policy::Gated, &["migrate", "--format", "json"]);
    let doc = document(&preview, "migrate");
    assert_eq!(code(&preview), 0, "{} / {}", stderr(&preview), doc);
    assert_eq!(doc["ok"], true, "{doc}");
    assert_eq!(doc["result"]["committed"], false, "{doc}");
    let spaces = doc["result"]["spaces"]
        .as_array()
        .unwrap_or_else(|| panic!("{doc}"));
    assert!(
        spaces.iter().any(|m| m["disposition"] == "adopt"
            && m["name"] == "legacy"
            && m["backend"] == "tmux"),
        "{doc}"
    );
    assert_eq!(
        doc["result"]["checks"][0]["check"], "complete_owner_scans",
        "{doc}"
    );
    assert_eq!(doc["result"]["checks"][0]["ok"], true, "{doc}");
    assert!(!stamp.exists(), "a preview recorded a cutover");
    assert_eq!(
        sandbox.authority_revision(),
        revision,
        "a preview wrote to the registry"
    );
    assert_eq!(server.option("legacy", "@dmux_space_uid"), "");

    let commit = sandbox.run_with_tmux(
        Policy::Gated,
        &["migrate", "--commit", "--yes", "--format", "json"],
    );
    let doc = document(&commit, "migrate");
    assert_eq!(code(&commit), 0, "{} / {}", stderr(&commit), doc);
    assert_eq!(doc["result"]["committed"], true, "{doc}");
    assert_eq!(doc["result"]["adopted"], 1, "{doc}");
    assert!(stamp.exists(), "the commit did not record the cutover");
    let spaces = sandbox.registry().spaces().unwrap();
    assert_eq!(spaces.len(), 1, "{spaces:?}");
    assert_eq!(spaces[0].logical_name, "legacy");
    assert_eq!(
        server.option("legacy", "@dmux_space_uid"),
        spaces[0].space_uid.0.to_string(),
        "the adopted session carries its marker"
    );

    // A second commit answers with the recorded cutover, adopts nothing
    // again and leaves the one Space as it was
    // (`migrate_cli::a_second_commit_is_a_clean_no_op_not_a_second_migration`).
    let again = sandbox.run_with_tmux(
        Policy::Gated,
        &["migrate", "--commit", "--yes", "--format", "json"],
    );
    let doc = document(&again, "migrate");
    assert_eq!(code(&again), 0, "{doc}");
    assert_eq!(doc["result"]["already_migrated"], true, "{doc}");
    assert_eq!(doc["result"]["committed"], false, "{doc}");
    assert_eq!(sandbox.registry().spaces().unwrap().len(), 1);
    assert_eq!(server.sessions(), ["legacy"]);
}

// ---------------------------------------------------------------------------
// new, con

/// `new` through the dispatch: the gated arm owns the typed dry-run refusal
/// that only `new_cli` emits; legacy refuses the Wez-first flags and keeps
/// the positional legacy create.
#[test]
fn new_dispatches_to_new_cli_under_the_gate_and_to_the_legacy_create_otherwise() {
    let sandbox = Sandbox::new();
    sandbox.recording_stub("tmux");
    let gated = sandbox
        .command(
            Policy::Gated,
            &["new", "project", "--backend", "tmux", "--no-connect"],
        )
        .env("DMUX_DRY_RUN", "1")
        .output()
        .unwrap();
    assert_eq!(code(&gated), 2, "{}", stderr(&gated));
    assert!(
        stderr(&gated).contains("cannot preview a Wez-first new operation"),
        "{}",
        stderr(&gated)
    );

    let legacy = sandbox
        .command(
            Policy::Legacy,
            &["new", "project", "--backend", "tmux", "--no-connect"],
        )
        .env("DMUX_DRY_RUN", "1")
        .output()
        .unwrap();
    legacy_refusal(&legacy, "new", false);
    let legacy_plain = sandbox
        .command(Policy::Legacy, &["new", "project"])
        .env("DMUX_DRY_RUN", "1")
        .output()
        .unwrap();
    assert_eq!(code(&legacy_plain), 0, "{}", stderr(&legacy_plain));
    assert!(
        stdout(&legacy_plain).contains("project"),
        "{}",
        stdout(&legacy_plain)
    );

    // The opt-out beats the opt-in through the same dispatch (ADR 010 §2).
    let escaped = sandbox
        .command(Policy::Legacy, &["new", "project", "--no-connect"])
        .env("DMUX_WEZ_FIRST", "1")
        .env("DMUX_DRY_RUN", "1")
        .output()
        .unwrap();
    legacy_refusal(&escaped, "new", false);
}

/// `con` through the dispatch: gated owns the typed dry-run refusal that
/// protects the attach token; legacy keeps the positional attach plan.
#[test]
fn con_dispatches_to_connect_cli_under_the_gate_and_to_the_legacy_attach_otherwise() {
    let sandbox = Sandbox::new();
    let stub = sandbox.bin.path().join("tmux");
    fs::write(
        &stub,
        "#!/bin/sh\ncase \"$1\" in\nlist-sessions) printf 'alpha|1700000000|1|0\\n' ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();

    for args in [&["con", "alpha"][..], &["alpha"][..], &["con", "2"][..]] {
        let gated = sandbox
            .command(Policy::Gated, args)
            .env("DMUX_DRY_RUN", "1")
            .output()
            .unwrap();
        assert_eq!(code(&gated), 2, "{args:?}: {}", stderr(&gated));
        assert!(
            stderr(&gated).contains("DMUX_DRY_RUN cannot preview"),
            "{args:?}: {}",
            stderr(&gated)
        );
        assert!(!stdout(&gated).contains("--token"), "{args:?}");
    }

    let legacy = sandbox
        .command(Policy::Legacy, &["con", "alpha"])
        .env("DMUX_DRY_RUN", "1")
        .output()
        .unwrap();
    assert_eq!(code(&legacy), 0, "{}", stderr(&legacy));
    assert_eq!(stdout(&legacy), "would exec: tmux attach -t '=alpha'\n");
    let legacy_flag = sandbox
        .command(Policy::Legacy, &["con", "alpha", "--backend", "wez"])
        .env("DMUX_DRY_RUN", "1")
        .output()
        .unwrap();
    legacy_refusal(&legacy_flag, "connect", false);
}

// ---------------------------------------------------------------------------
// repair reconcile, repair retire-incarnation, _context: not gated

/// `repair reconcile` is dispatched identically under both policies: the
/// same document for the same registry.
#[test]
fn repair_reconcile_is_dispatched_the_same_way_under_both_policies() {
    let sandbox = Sandbox::new();
    let mut documents = Vec::new();
    for policy in BOTH {
        let out = sandbox.run(
            policy,
            &["--format", "json", "repair", "reconcile", "--yes"],
        );
        let doc = document(&out, "repair_reconcile");
        assert_eq!(code(&out), 0, "{policy:?}: {} / {}", stderr(&out), doc);
        assert_eq!(doc["ok"], true, "{policy:?}: {doc}");
        documents.push(doc);
    }
    assert_eq!(
        documents[0], documents[1],
        "the flag changed repair's dispatch"
    );
}

/// `repair retire-incarnation` clears a dead incarnation the same way under
/// both policies, each against its own seeded registry.
#[test]
fn repair_retire_incarnation_is_dispatched_the_same_way_under_both_policies() {
    for policy in BOTH {
        let sandbox = Sandbox::new();
        let epoch = ServerEpoch(Uuid::new_v4());
        let child = Command::new("true").spawn().unwrap();
        let dead_pid = i64::from(child.id());
        let _ = child.wait_with_output();
        let instance = {
            let mut registry = sandbox.registry();
            let instance = registry
                .register_backend_instance(Backend::Tmux, Some("dmux-gated-retire"), None)
                .unwrap();
            registry
                .publish_backend_server(instance, epoch, Some(dead_pid), Some("gone"), None, None)
                .unwrap();
            instance
        };
        let out = sandbox.run(
            policy,
            &[
                "--format",
                "json",
                "repair",
                "retire-incarnation",
                "--backend",
                "tmux",
                "--epoch",
                &epoch.0.to_string(),
                "-y",
            ],
        );
        let doc = document(&out, "repair_retire_incarnation");
        assert_eq!(code(&out), 0, "{policy:?}: {} / {}", stderr(&out), doc);
        assert_eq!(doc["ok"], true, "{policy:?}: {doc}");
        assert!(
            sandbox
                .registry()
                .backend_server(instance)
                .unwrap()
                .server_epoch
                .is_none(),
            "{policy:?}: the incarnation was not retired"
        );
    }
}

/// `_context` on a tmux pane is not gated: under both policies an
/// unpublished instance refuses with no marker, and the published server
/// mints the same document (the tmux arm's four refusals are
/// `context_cli`'s; this pins that the flag does not alter the dispatch).
#[test]
fn context_mints_or_refuses_identically_under_both_policies() {
    let sandbox = Sandbox::new();
    let server = ScratchTmux::start(&sandbox, "context", "proj");
    let session = server.session_id("proj");
    let pane = server.pane_id("proj");
    let socket = server.socket_path();
    let instance = sandbox.register_unpublished_tmux(&server);
    let space_uid = {
        let mut registry = sandbox.registry();
        let reservation = registry
            .reserve_space("proj", instance, Uuid::new_v4())
            .unwrap();
        registry
            .finalize_create(
                reservation.space_uid,
                reservation.operation_uid,
                &NativeBindingSpec {
                    native_token: session.clone(),
                    native_kind: NativeKind::TmuxSessionId,
                    server_epoch: None,
                },
            )
            .unwrap();
        registry
            .set_space_health(reservation.space_uid, Health::Healthy)
            .unwrap();
        reservation.space_uid
    };
    let context = |policy: Policy| {
        sandbox
            .command(policy, &["_context"])
            .env("PATH", sandbox.path_with_real_tmux())
            .env("DMUX_SPACE_UID", space_uid.0.to_string())
            .env("TMUX", format!("{socket},1,0"))
            .env("TMUX_PANE", &pane)
            .output()
            .expect("dmux runs")
    };

    for policy in BOTH {
        let out = context(policy);
        assert!(
            !out.status.success(),
            "{policy:?}: an unpublished instance minted a marker"
        );
        assert_eq!(
            stdout(&out),
            "",
            "{policy:?}: no marker may be minted on refusal"
        );
        assert!(
            stderr(&out).contains("published no server epoch"),
            "{policy:?}: {}",
            stderr(&out)
        );
    }

    let epoch = sandbox.publish_tmux(&server);
    let mut minted = Vec::new();
    for policy in BOTH {
        let out = context(policy);
        assert!(out.status.success(), "{policy:?}: {}", stderr(&out));
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(
            doc["space_uid"],
            space_uid.0.to_string(),
            "{policy:?}: {doc}"
        );
        assert_eq!(
            doc["server_epoch"],
            epoch.0.to_string(),
            "{policy:?}: {doc}"
        );
        minted.push(doc);
    }
    assert_eq!(minted[0], minted[1], "the flag changed the minted marker");
}
