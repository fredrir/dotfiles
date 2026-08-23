//! Case 43 for the verbs P4/P6 do not own: `--format json` is exactly one
//! schema-versioned document (plan §16.2, ADR 008 §1) with nothing else on
//! stdout, and the deprecated `--json` flags keep their byte-for-byte legacy
//! payload with their migration hint on stderr.
//!
//! Hermetic where the verb allows it: `doctor` and `host ls` reach no
//! backend, and `repair normalize` runs against a stub `wezterm` plus a
//! listening socket, so no mux server is involved. `group ls`/`split ls`
//! need a real managed Space, so they drive a scratch tmux server the way
//! `hierarchy_flow` does.

use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};

use dmux::model::HostUid;
use dmux::operations::{
    CreateRequest, OperationEnv, OwnerCreateTarget, create_space_owner_fenced, tmux_bootstrap,
};
use dmux::registry::{Registry, RegistryConfig};
use serde_json::Value;
use uuid::Uuid;

/// A private HOME/XDG root: the registry the CLI resolves is the one this
/// test built, never the developer's.
struct Sandbox {
    home: tempfile::TempDir,
    locks: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Sandbox {
        Sandbox {
            home: tempfile::tempdir().unwrap(),
            locks: tempfile::tempdir().unwrap(),
        }
    }

    fn db_path(&self) -> std::path::PathBuf {
        self.home.path().join("dmux/registry.sqlite3")
    }

    /// The registry as the CLI will resolve it (`$XDG_DATA_HOME/dmux`), open
    /// on a scratch lock directory: locks fence writers, and every read here
    /// is the test's own.
    fn registry(&self) -> Registry {
        Registry::open(RegistryConfig::new(self.db_path(), self.locks.path())).unwrap()
    }

    fn authority_revision(&self) -> u64 {
        self.registry().authority_head().unwrap().revision
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dmux"));
        command
            .args(args)
            .env("HOME", self.home.path())
            .env("XDG_DATA_HOME", self.home.path())
            .env("XDG_STATE_HOME", self.home.path())
            // The CLI under test fences on the same lock dir the harness's
            // own `registry()` opens on, and never on the live runtime dir
            // (ADR 012 WS-E.1; §20.1 "suite runs leave it unchanged").
            .env("DMUX_RUNTIME_DIR", self.locks.path())
            .env_remove("DMUX_WEZ_FIRST")
            .env_remove("DMUX_DRY_RUN")
            .env_remove("TMUX")
            .env_remove("TMUX_PANE")
            .env_remove("WEZTERM_PANE")
            .env_remove("WEZTERM_UNIX_SOCKET")
            .env_remove("DMUX_HOST_UID")
            .env_remove("DMUX_SPACE_UID")
            .env_remove("DMUX_GROUP_REF")
            .env_remove("NO_COLOR");
        command
    }

    fn dmux(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("dmux runs")
    }

    /// A registry on disk with one enrolled peer, so a refusal has a real
    /// head to stamp and cannot pass by fabricating zero.
    fn with_a_peer(&self) -> (HostUid, u64) {
        let uid = HostUid(Uuid::new_v4());
        let mut registry = self.registry();
        registry.enroll_host(uid, Some("peer")).unwrap();
        drop(registry);
        let revision = self.authority_revision();
        assert!(revision > 0, "enrollment must advance the chain");
        (uid, revision)
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Case 43 itself: stdout is one document and nothing else — one line, one
/// JSON value, every §16.2 field present.
fn sole_document(out: &Output, action: &str) -> Value {
    let text = stdout(out);
    assert_eq!(text.lines().count(), 1, "not one document: {text:?}");
    let doc: Value = serde_json::from_str(text.trim()).unwrap_or_else(|e| panic!("{e}: {text}"));
    assert_eq!(doc["schema_version"], 1, "{doc}");
    assert_eq!(doc["action"], action, "{doc}");
    assert!(doc["ok"].is_boolean(), "{doc}");
    assert!(doc["errors"].is_array(), "{doc}");
    assert!(doc["authority_revision"].is_u64(), "{doc}");
    assert!(doc.get("result").is_some(), "{doc}");
    doc
}

const HINT: &str = "--json is deprecated";

/// Doctor reaches no registry of its own, so the revision it reports has to
/// come from the one on disk — enrolling a peer moves it off zero, which a
/// fabricated constant could not follow.
#[test]
fn doctor_format_json_is_one_document_carrying_the_real_revision() {
    let sandbox = Sandbox::new();
    let bin = tempfile::tempdir().unwrap();
    stub(bin.path(), "tmux", "printf 'alpha|1|2|1\\nbeta|1|1|0\\n'");
    stub(bin.path(), "wezterm", "printf '[]'");
    stub(bin.path(), "ssh", "exit 255");

    // No registry at all: doctor reports rather than creating one.
    let out = sandbox
        .command(&["doctor", "--format", "json"])
        .env("PATH", bin.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let doc = sole_document(&out, "doctor");
    assert_eq!(doc["authority_revision"], 0, "{doc}");
    assert!(!sandbox.db_path().exists(), "doctor created a registry");

    let mut registry = sandbox.registry();
    registry
        .enroll_host(HostUid(Uuid::new_v4()), Some("peer"))
        .unwrap();
    drop(registry);
    let revision = sandbox.authority_revision();
    assert!(revision > 0, "enrollment must advance the chain");

    let out = sandbox
        .command(&["doctor", "--format", "json"])
        .env("PATH", bin.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let doc = sole_document(&out, "doctor");
    assert_eq!(doc["authority_revision"], revision, "{doc}");
    assert_eq!(doc["ok"], true, "a red probe is a finding, not a failure");
    assert_eq!(doc["errors"], serde_json::json!([]), "{doc}");
    assert_eq!(doc["result"]["tmux_server"]["ok"], true, "{doc}");
    assert_eq!(doc["result"]["ssh_peer"]["ok"], false, "{doc}");
    assert!(doc["result"]["host"]["detail"].is_string(), "{doc}");
    assert!(stderr(&out).is_empty(), "{}", stderr(&out));

    // The deprecated flag keeps the bare probe object, hint on stderr only.
    let legacy = sandbox
        .command(&["doctor", "--json"])
        .env("PATH", bin.path())
        .output()
        .unwrap();
    assert!(legacy.status.success());
    let bare: Value = serde_json::from_str(stdout(&legacy).trim()).unwrap();
    assert!(bare.get("schema_version").is_none(), "{bare}");
    assert_eq!(bare["tmux_server"]["ok"], true, "{bare}");
    assert!(stderr(&legacy).contains(HINT), "{}", stderr(&legacy));
}

#[test]
fn host_ls_format_json_is_one_document_and_json_keeps_its_bare_array() {
    let sandbox = Sandbox::new();
    let mut registry = sandbox.registry();
    registry
        .enroll_host(HostUid(Uuid::new_v4()), Some("peer"))
        .unwrap();
    drop(registry);
    let revision = sandbox.authority_revision();

    let out = sandbox.dmux(&["host", "ls", "--format", "json"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let doc = sole_document(&out, "host_list");
    assert_eq!(doc["ok"], true, "{doc}");
    assert_eq!(doc["authority_revision"], revision, "{doc}");
    let rows = doc["result"].as_array().expect("row array");
    assert_eq!(
        rows.len(),
        2,
        "local authority plus the enrolled peer: {doc}"
    );
    assert!(rows.iter().any(|r| r["label"] == "peer"), "{doc}");
    assert!(stderr(&out).is_empty(), "{}", stderr(&out));

    // Byte for byte the payload scripts already parse, hint on stderr only.
    let legacy = sandbox.dmux(&["host", "ls", "--json"]);
    assert!(legacy.status.success(), "{}", stderr(&legacy));
    assert_eq!(
        stdout(&legacy).trim(),
        serde_json::to_string(&doc["result"]).unwrap(),
        "the legacy array must not change shape"
    );
    assert!(stderr(&legacy).contains(HINT), "{}", stderr(&legacy));
}

// ---------------------------------------------------------------------------
// repair normalize: the empty, refused, and per-target-result documents,
// against a stub wez CLI and a socket that only has to answer connect(2).

struct WezStub {
    dir: tempfile::TempDir,
    _listener: std::os::unix::net::UnixListener,
    socket: String,
    bin: String,
    /// The sentinel epoch the stub answers with; the seam pins to it.
    epoch: Uuid,
}

impl WezStub {
    /// A `wezterm` that answers every call with `rows`, and a live socket so
    /// the provider's strict-endpoint probe finds something connectable.
    fn new(epoch: Uuid, rows: &str) -> WezStub {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let socket = dir.path().join("sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let bin = dir.path().join("wezterm-stub");
        std::fs::write(&bin, format!("#!/bin/sh\ncat <<'EOF'\n{rows}\nEOF\n")).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        WezStub {
            socket: socket.display().to_string(),
            bin: bin.display().to_string(),
            _listener: listener,
            dir,
            epoch,
        }
    }

    fn normalize(&self, sandbox: &Sandbox, args: &[&str]) -> Output {
        let mut command = sandbox.command(&["repair", "normalize"]);
        command
            .args(args)
            .args(["--data-dir", self.dir.path().to_str().unwrap()])
            .args(["--lock-dir", self.dir.path().to_str().unwrap()])
            .args(["--socket", &self.socket, "--epoch", &self.epoch.to_string()])
            .env("DMUX_WEZ_BIN", &self.bin)
            .env("DMUX_WEZ_CONFIG", self.dir.path().join("wez.lua"));
        command.output().expect("dmux runs")
    }

    fn revision(&self) -> u64 {
        Registry::open(RegistryConfig::new(
            self.dir.path().join("registry.sqlite3"),
            self.dir.path(),
        ))
        .unwrap()
        .authority_head()
        .unwrap()
        .revision
    }
}

fn sentinel_rows(epoch: Uuid, extra: &str) -> String {
    format!(
        r#"[{{"window_id":0,"tab_id":0,"pane_id":0,"workspace":"dmux:system:{}"}}{extra}]"#,
        epoch
    )
}

#[test]
fn repair_normalize_format_json_documents_the_empty_scan() {
    let sandbox = Sandbox::new();
    let epoch = Uuid::new_v4();
    let stub = WezStub::new(
        epoch,
        &sentinel_rows(
            epoch,
            r#",{"window_id":1,"tab_id":1,"pane_id":1,"workspace":"solo"}"#,
        ),
    );

    let out = stub.normalize(&sandbox, &["--format", "json"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let doc = sole_document(&out, "repair_normalize");
    assert_eq!(doc["ok"], true, "{doc}");
    assert_eq!(doc["result"]["targets"], serde_json::json!([]), "{doc}");
    assert_eq!(doc["authority_revision"], stub.revision(), "{doc}");

    let legacy = stub.normalize(&sandbox, &["--json"]);
    assert!(legacy.status.success(), "{}", stderr(&legacy));
    assert_eq!(stdout(&legacy).trim(), r#"{"targets":[]}"#);
    assert!(stderr(&legacy).contains(HINT), "{}", stderr(&legacy));
}

/// §7.4: a JSON destructive verb never prompts. Without `--yes` it emits one
/// `confirmation_required` document carrying the preview, changes nothing,
/// and exits 5. The deprecated flag keeps its own older refusal payload for
/// this release, so both spellings refuse without prompting and only the
/// shape differs.
#[test]
fn repair_normalize_without_yes_refuses_in_one_document() {
    let sandbox = Sandbox::new();
    let epoch = Uuid::new_v4();
    let stub = WezStub::new(
        epoch,
        &sentinel_rows(
            epoch,
            r#",{"window_id":1,"tab_id":1,"pane_id":1,"workspace":"sprawl"},
           {"window_id":2,"tab_id":2,"pane_id":2,"workspace":"sprawl"}"#,
        ),
    );

    let out = stub.normalize(&sandbox, &["--format", "json"]);
    assert_eq!(out.status.code(), Some(5), "{}", stderr(&out));
    let doc = sole_document(&out, "repair_normalize");
    assert_eq!(doc["ok"], false, "{doc}");
    assert_eq!(doc["errors"][0]["code"], "confirmation_required", "{doc}");
    assert_eq!(doc["errors"][0]["target"], "sprawl", "{doc}");
    assert_eq!(
        doc["result"]["targets"][0]["native_token"], "sprawl",
        "{doc}"
    );

    let legacy = stub.normalize(&sandbox, &["--json"]);
    assert_eq!(legacy.status.code(), Some(5), "{}", stderr(&legacy));
    let bare: Value = serde_json::from_str(stdout(&legacy).trim()).unwrap();
    assert_eq!(bare["confirmation_required"], true, "{bare}");
    assert_eq!(bare["targets"][0]["native_token"], "sprawl", "{bare}");
    assert!(stderr(&legacy).contains(HINT), "{}", stderr(&legacy));
}

/// Case 42's shape for this verb: a target that could not be merged is a
/// typed error beside a real result, which is the §16.3 partial (7).
#[test]
fn repair_normalize_reports_per_target_failure_and_exits_partial() {
    let sandbox = Sandbox::new();
    // The stub never actually moves a pane, so every apply fails its
    // postcondition — one quarantined target, zero mutation.
    let epoch = Uuid::new_v4();
    let stub = WezStub::new(
        epoch,
        &sentinel_rows(
            epoch,
            r#",{"window_id":1,"tab_id":1,"pane_id":1,"workspace":"sprawl"},
           {"window_id":2,"tab_id":2,"pane_id":2,"workspace":"sprawl"}"#,
        ),
    );

    let out = stub.normalize(&sandbox, &["--format", "json", "--yes"]);
    assert_eq!(out.status.code(), Some(7), "{}", stderr(&out));
    let doc = sole_document(&out, "repair_normalize");
    assert_eq!(doc["ok"], false, "{doc}");
    assert_eq!(doc["result"]["results"][0]["ok"], false, "{doc}");
    assert_eq!(
        doc["result"]["results"][0]["native_token"], "sprawl",
        "{doc}"
    );
    assert_eq!(doc["errors"][0]["target"], "sprawl", "{doc}");
    assert_eq!(doc["errors"][0]["code"], "operation_failed", "{doc}");
}

// ---------------------------------------------------------------------------
// group/split ls: a real managed Space on a scratch tmux server. The CLI
// resolves the production registry, so the sandbox HOME is what makes that
// the one seeded here.

struct TmuxScratch {
    ns: String,
    locks: tempfile::TempDir,
}

impl TmuxScratch {
    fn start() -> Option<TmuxScratch> {
        let scratch = TmuxScratch {
            ns: format!("dmux-p8json-{}", std::process::id()),
            locks: tempfile::tempdir().unwrap(),
        };
        // Window titles are frozen at creation. The Space's pane runs
        // `pane-bootstrap` and then `sleep`, and tmux's automatic rename
        // follows the foreground command on its own schedule; the test
        // below compares four listings against one `hierarchy()` snapshot
        // byte for byte, and under load the rename landed between two of
        // them (`title: "tmux"` became `"sleep"`) — a fixture race, not a
        // shape change (ADR 012 WS-E.4 flake triage).
        let started = Command::new("tmux")
            .args(["-L", &scratch.ns, "-f", "/dev/null"])
            .args(["new-session", "-d", "-s", "seed"])
            .args([";", "set-option", "-g", "automatic-rename", "off"])
            .args([";", "set-option", "-g", "allow-rename", "off"])
            .env("DMUX_RUNTIME_DIR", scratch.locks.path())
            .status();
        match started {
            Ok(status) if status.success() => Some(scratch),
            _ => None,
        }
    }
}

impl Drop for TmuxScratch {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.ns, "kill-server"])
            .output();
    }
}

#[test]
fn group_and_split_ls_format_json_are_one_document_each() {
    let sandbox = Sandbox::new();
    let Some(scratch) = TmuxScratch::start() else {
        eprintln!("skipping: no usable tmux");
        return;
    };
    // The Space is created against the same database the CLI will resolve
    // from the sandbox HOME; only the lock directory differs, and a read
    // takes no writer's fence.
    let env = OperationEnv {
        db_path: sandbox.db_path(),
        lock_dir: scratch.locks.path().to_path_buf(),
    };
    let epoch = match tmux_bootstrap(&env, &scratch.ns).unwrap() {
        dmux::operations::TmuxBootstrapOutcome::Bootstrapped { epoch }
        | dmux::operations::TmuxBootstrapOutcome::AlreadyBound { epoch }
        | dmux::operations::TmuxBootstrapOutcome::Rebound { epoch, .. } => epoch,
    };
    let provider = dmux::backend::tmux::TmuxProvider::new(scratch.ns.clone());
    let scope = dmux::backend::InventoryScope::managed(
        dmux::model::Backend::Tmux,
        scratch.ns.clone(),
        epoch,
    );
    let instance = sandbox
        .registry()
        .backend_instance_for_backend(dmux::model::Backend::Tmux)
        .unwrap()
        .expect("tmux_bootstrap registered the instance");
    let created = create_space_owner_fenced(
        &env,
        OwnerCreateTarget {
            backend: dmux::model::Backend::Tmux,
            instance,
            provider: &provider,
            scope: &scope,
        },
        None,
        false,
        &CreateRequest {
            request_uid: Uuid::new_v4(),
            name: "proj".into(),
            cwd: None,
            program: vec!["sh".into(), "-c".into(), "exec sleep 300".into()],
            helper_bin: env!("CARGO_BIN_EXE_pane-bootstrap").to_string(),
        },
    )
    .unwrap();
    let revision = sandbox.authority_revision();

    let out = sandbox.dmux(&["group", "ls", "proj", "--format", "json"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let doc = sole_document(&out, "group_list");
    assert_eq!(doc["ok"], true, "{doc}");
    assert_eq!(doc["authority_revision"], revision, "{doc}");
    assert_eq!(
        doc["result"]["space_uid"],
        created.space_uid.0.to_string(),
        "{doc}"
    );
    assert_eq!(
        doc["result"]["groups"][0]["group_ref"],
        created.group_ref.as_str(),
        "{doc}"
    );
    assert!(stderr(&out).is_empty(), "{}", stderr(&out));

    // Byte for byte the serialized hierarchy, which is what the deprecated
    // flag has always printed; the envelope carries the same value.
    let tree = dmux::operations::hierarchy(&env, &provider, &scope, created.space_uid).unwrap();
    assert_eq!(doc["result"], serde_json::to_value(&tree).unwrap(), "{doc}");
    let legacy = sandbox.dmux(&["group", "ls", "proj", "--json"]);
    assert!(legacy.status.success(), "{}", stderr(&legacy));
    assert_eq!(
        stdout(&legacy).trim(),
        serde_json::to_string(&tree).unwrap(),
        "the legacy hierarchy must not change shape"
    );
    assert!(stderr(&legacy).contains(HINT), "{}", stderr(&legacy));

    let group_ref = format!("proj/{}", created.group_ref);
    let out = sandbox.dmux(&["split", "ls", &group_ref, "--format", "json"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let doc = sole_document(&out, "split_list");
    assert_eq!(
        doc["result"]["group_ref"],
        created.group_ref.as_str(),
        "{doc}"
    );
    assert_eq!(
        doc["result"]["splits"][0]["split_ref"],
        created.split_ref.as_str(),
        "{doc}"
    );

    let listed = tree
        .groups
        .iter()
        .find(|g| g.group_ref == created.group_ref)
        .expect("the created group is listed");
    assert_eq!(
        doc["result"],
        serde_json::to_value(listed).unwrap(),
        "{doc}"
    );
    let legacy = sandbox.dmux(&["split", "ls", &group_ref, "--json"]);
    assert!(legacy.status.success(), "{}", stderr(&legacy));
    assert_eq!(
        stdout(&legacy).trim(),
        serde_json::to_string(listed).unwrap(),
        "the legacy group object must not change shape"
    );
    assert!(stderr(&legacy).contains(HINT), "{}", stderr(&legacy));
}

fn stub(dir: &std::path::Path, name: &str, script: &str) {
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

// ---------------------------------------------------------------------------
// Case 43 off the happy path: the branches that used to print a human line
// to stderr and leave stdout empty.

/// Every refusal is a document, and §16.3 shows through it: a misspelled ref
/// is validation (2), a ref that names nothing is target-not-found (3), and
/// only an unreachable provider is 6. All of these exited 1 or 2 with an
/// empty stdout before.
#[test]
fn every_refusal_branch_is_one_document() {
    let sandbox = Sandbox::new();
    let (_, revision) = sandbox.with_a_peer();

    for (args, action, exit, code) in [
        (vec!["group", "ls"], "group_list", 2, "usage"),
        (
            vec!["group", "ls", "demo/g"],
            "group_list",
            2,
            "invalid_ref",
        ),
        (
            vec!["group", "ls", "nosuchspace"],
            "group_list",
            3,
            "not_found",
        ),
        (
            vec!["group", "rename", "nosuchspace", "other"],
            "group_rename",
            3,
            "not_found",
        ),
        (
            vec!["split", "ls", "nosuchspace"],
            "split_list",
            3,
            "not_found",
        ),
        (
            vec!["context", "stamp", "nosuchspace"],
            "context_stamp",
            3,
            "not_found",
        ),
        (
            vec!["repair", "normalize"],
            "repair_normalize",
            6,
            "provider_unavailable",
        ),
        (
            vec!["host", "forget", "nosuchhost", "--yes"],
            "host_forget",
            3,
            "not_found",
        ),
        // `migrate`'s gate refusal moved to
        // `the_gate_refusal_is_one_document_under_the_legacy_policy`: the
        // default runs it since the §21 step 9 flip.
    ] {
        let mut argv = args.clone();
        argv.extend(["--format", "json"]);
        let out = sandbox.dmux(&argv);
        assert_eq!(out.status.code(), Some(exit), "{args:?}: {}", stderr(&out));
        let doc = sole_document(&out, action);
        assert_eq!(doc["ok"], false, "{args:?}: {doc}");
        assert_eq!(doc["result"], Value::Null, "{args:?}: {doc}");
        assert_eq!(doc["errors"][0]["code"], code, "{args:?}: {doc}");
        assert_eq!(doc["authority_revision"], revision, "{args:?}: {doc}");

        // The same refusal in human mode keeps stdout empty: one shape or
        // the other, never a document beside a sentence.
        let human = sandbox.dmux(&args);
        assert_eq!(human.status.code(), Some(exit), "{args:?}");
        assert_eq!(stdout(&human), "", "{args:?}");
        assert!(!stderr(&human).is_empty(), "{args:?}");
    }
}

/// The gate's own refusal is one document too. Since the §21 step 9 flip the
/// gate is only reachable under an explicit opt-out, so this is the one row
/// of the table above that pins the legacy environment (`DMUX_LEGACY_POLICY=1`,
/// the §21 rollback spelling `tests/cli.rs` uses) instead of the default.
#[test]
fn the_gate_refusal_is_one_document_under_the_legacy_policy() {
    let sandbox = Sandbox::new();
    let (_, revision) = sandbox.with_a_peer();
    for format in [true, false] {
        let mut argv = vec!["migrate"];
        if format {
            argv.extend(["--format", "json"]);
        }
        let mut command = sandbox.command(&argv);
        command.env("DMUX_LEGACY_POLICY", "1");
        let out = command.output().expect("dmux runs");
        assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
        if format {
            let doc = sole_document(&out, "migrate");
            assert_eq!(doc["ok"], false, "{doc}");
            assert_eq!(doc["errors"][0]["code"], "usage", "{doc}");
            assert_eq!(doc["authority_revision"], revision, "{doc}");
        } else {
            assert_eq!(stdout(&out), "");
            assert!(
                stderr(&out).contains("DMUX_WEZ_FIRST=1"),
                "{}",
                stderr(&out)
            );
        }
    }
}

/// §7.4 for the child verbs: a JSON destructive command never prompts and
/// never mutates — it answers with the one `confirmation_required` document
/// carrying the batch it refused.
#[test]
fn group_and_split_rm_refuse_in_one_confirmation_document() {
    let sandbox = Sandbox::new();
    let (_, revision) = sandbox.with_a_peer();
    let group = "demo/g00000000-0000-4000-8000-000000000001.wz-1";
    let split = "demo/p00000000-0000-4000-8000-000000000001.wz-1";

    for (verb, target, action) in [("group", group, "group_rm"), ("split", split, "split_rm")] {
        let out = sandbox.dmux(&[verb, "rm", target, "--format", "json"]);
        assert_eq!(out.status.code(), Some(5), "{}", stderr(&out));
        let doc = sole_document(&out, action);
        assert_eq!(doc["errors"][0]["code"], "confirmation_required", "{doc}");
        assert_eq!(doc["errors"][0]["target"], target, "{doc}");
        assert_eq!(doc["authority_revision"], revision, "{doc}");
        assert!(
            stderr(&out).is_empty(),
            "a JSON refusal never prompts: {}",
            stderr(&out)
        );
    }
}

/// Case 43's other half: a verb with no bounded JSON result must refuse the
/// flag as a document rather than print its human report under a flag it
/// never reads. Nothing runs first — the sandbox has no registry after.
#[test]
fn format_json_on_a_verb_that_has_none_refuses_as_a_document() {
    let sandbox = Sandbox::new();
    for (args, action) in [
        (vec!["keys"], "keys"),
        (vec!["con", "whatever"], "connect"),
        (vec!["new", "whatever"], "new"),
        (vec!["disconnect"], "disconnect"),
    ] {
        let mut argv = args.clone();
        argv.extend(["--format", "json"]);
        let out = sandbox.dmux(&argv);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
        let doc = sole_document(&out, action);
        assert_eq!(doc["errors"][0]["code"], "usage", "{args:?}: {doc}");
        assert_eq!(doc["authority_revision"], 0, "no registry to read: {doc}");
    }
    assert!(
        !sandbox.db_path().exists(),
        "a refused verb must not reach the authority"
    );
}

/// The rule P11 settled on (F8): `DMUX_WEZ_FIRST` decides what a document
/// says, never whether there is one. `recovery --format json` in particular
/// worked without the flag before P11 promoted `--format` to a global.
#[test]
fn the_gate_changes_the_document_not_whether_there_is_one() {
    let sandbox = Sandbox::new();
    sandbox.with_a_peer();
    let bin = tempfile::tempdir().unwrap();
    stub(bin.path(), "tmux", "exit 1");
    stub(bin.path(), "wezterm", "printf '[]'");
    stub(bin.path(), "ssh", "exit 255");

    for (args, action) in [
        (vec!["doctor"], "doctor"),
        (vec!["host", "ls"], "host_list"),
        (vec!["group", "ls", "nosuchspace"], "group_list"),
        (vec!["recovery", "status"], "recovery_status"),
        (vec!["migrate"], "migrate"),
        (vec!["ls"], "list"),
    ] {
        let mut argv = args.clone();
        argv.extend(["--format", "json"]);
        for gated in [false, true] {
            let mut command = sandbox.command(&argv);
            command.env("PATH", bin.path());
            if gated {
                command.env("DMUX_WEZ_FIRST", "1");
            }
            let out = command.output().expect("dmux runs");
            sole_document(&out, action);
        }
    }
}

/// F1/F2: `recovery` carried its own `--format json` before P11 pointed it
/// at the global flag, and answered with a three-field shape — no `action`,
/// no `errors[]`, no `authority_revision`. Resume and abort ignored the flag
/// altogether and printed their human sentence to stdout.
#[test]
fn recovery_answers_the_same_envelope_as_every_other_verb() {
    let sandbox = Sandbox::new();
    let (_, revision) = sandbox.with_a_peer();

    for (verb, action) in [("status", "recovery_status"), ("resume", "recovery_resume")] {
        let out = sandbox.dmux(&["recovery", verb, "--format", "json"]);
        assert_eq!(out.status.code(), Some(3), "{verb}: {}", stderr(&out));
        let doc = sole_document(&out, action);
        assert_eq!(doc["ok"], false, "{doc}");
        assert_eq!(doc["errors"][0]["code"], "not_found", "{doc}");
        assert_eq!(doc["authority_revision"], revision, "{doc}");
    }

    // §7.4: the JSON destructive verb never prompts, and its refusal is the
    // same one document `repair normalize` already emitted.
    let out = sandbox.dmux(&["recovery", "abort", "--format", "json"]);
    assert_eq!(out.status.code(), Some(5), "{}", stderr(&out));
    let doc = sole_document(&out, "recovery_abort");
    assert_eq!(doc["errors"][0]["code"], "confirmation_required", "{doc}");
    assert!(stderr(&out).is_empty(), "{}", stderr(&out));

    // Human mode is unchanged: the refusal stays on stderr, stdout is empty.
    let human = sandbox.dmux(&["recovery", "abort"]);
    assert_eq!(human.status.code(), Some(5), "{}", stderr(&human));
    assert_eq!(stdout(&human), "");
    assert!(stderr(&human).contains("requires confirmation"));
}

/// F7: both host-admin verbs document an alias, label, or HostUid, but their
/// positional shared clap's `host` id with the global `-H/--host`, so every
/// value also went through the legacy-host gate — which knows only `macie`
/// and `archie`. Nothing they document could reach them.
#[test]
fn host_label_and_forget_reach_every_ref_they_document() {
    let sandbox = Sandbox::new();
    let (uid, _) = sandbox.with_a_peer();

    // The label the peer was enrolled with.
    let out = sandbox.dmux(&["host", "label", "peer", "friendly", "--format", "json"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let doc = sole_document(&out, "host_label");
    assert_eq!(doc["result"]["label"], "friendly", "{doc}");
    assert_eq!(doc["result"]["host_uid"], uid.0.to_string(), "{doc}");

    // The full HostUid.
    let out = sandbox.dmux(&[
        "host",
        "label",
        &uid.0.to_string(),
        "friendlier",
        "--format",
        "json",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        sole_document(&out, "host_label")["result"]["label"],
        "friendlier"
    );

    // The alias the listing prints, on the destructive verb.
    let alias = sole_document(
        &sandbox.dmux(&["host", "ls", "--format", "json"]),
        "host_list",
    )["result"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["host_uid"] == uid.0.to_string())
        .and_then(|row| row["alias"].as_str().map(str::to_string))
        .expect("the enrolled peer has an alias");
    let out = sandbox.dmux(&["host", "forget", &alias, "--yes", "--format", "json"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        sole_document(&out, "host_forget")["result"]["host_uid"],
        uid.0.to_string()
    );
}
