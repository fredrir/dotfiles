use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Mutex, MutexGuard, OnceLock};

use dotfile_cli::cancel;
use dotfile_cli::cli::{Resolution, SyncCli};
use dotfile_cli::context::Context;
use dotfile_cli::decision::{self, Choice, Prompt};
use dotfile_cli::event::{Event, Phase, VecSink};
use dotfile_cli::push::{self, DecisionClient};
use tempfile::TempDir;

const HOSTS: &str = "archie {\n  hostnames = archie, archie.local\n  role = desktop\n}\n\nmacie {\n  hostnames = macie\n  role = laptop\n}\n";
const WIRE_STUB: &str = r#"#!/bin/sh
case "$*" in
  'sync --wire-probe 2')
    case "${PUSH_WIRE_SCENARIO:-}" in
      stale) [ -f "$PUSH_REMOTE_HOME/.wire-current" ] ;;
      update-fail) exit 1 ;;
      *) exit 0 ;;
    esac
    exit $?
    ;;
  'sync --wire 2') label='sync' ;;
  'sync --wire 2 --force') label='sync --force' ;;
  'sync --wire 2 --resolve repo') label='sync --resolve repo' ;;
  'sync --wire 2 --resolve live') label='sync --resolve live' ;;
  *) exit 2 ;;
esac
printf '%s\n' "$label" >> "$PUSH_SYNC_LOG"
printf '%s\n' '{"message":"sync-ready","version":2}'
case "${PUSH_WIRE_SCENARIO:-}" in
  decisions)
    printf '%s\n' '{"message":"decision-request","id":41,"prompt":{"prompt":"merge","path":"/remote/settings.json","key":"font","repo":"mono","live":"sans"}}'
    IFS= read -r first || exit 3
    printf 'response:%s\n' "$first" >> "$PUSH_SYNC_LOG"
    printf '%s\n' '{"message":"decision-request","id":42,"prompt":{"prompt":"merge-target","path":"/remote/settings.json","key":"font","targets":["shared","macos"],"default":1}}'
    IFS= read -r second || exit 3
    printf 'response:%s\n' "$second" >> "$PUSH_SYNC_LOG"
    ;;
  mismatch)
    printf '%s\n' '{"message":"sync-ready","version":99}'
    exit 2
    ;;
  malformed)
    printf '%s\n' 'not-json'
    exit 2
    ;;
  error-eof)
    printf '%s\n' '{"message":"error","operation":"sync","value":"remote failed","code":1}'
    cat >/dev/null
    exit 2
    ;;
  eof) exit 2 ;;
esac
printf '%s\n' '{"message":"event","value":{"event":"started","profile":"test","dry_run":false,"peer":null}}'
printf '%s\n' '{"message":"event","value":{"event":"item","action":"merge","path":"/remote/settings.json","detail":"updated","changed":true}}'
printf '%s\n' '{"message":"event","value":{"event":"warning","message":"remote warning","hint":"inspect it"}}'
printf '%s\n' '{"message":"event","value":{"event":"finished","profile":"test","checked":1,"changed":0,"links":0,"merges":0,"secrets":0,"generated":0,"dry_run":false,"elapsed":{"secs":0,"nanos":1}}}'
printf '%s\n' '{"message":"completed"}'
"#;
const SETUP_STUB: &str = r#"#!/bin/sh
printf 'update\n' >> "$PUSH_SYNC_LOG"
if [ "${PUSH_WIRE_SCENARIO:-}" = update-fail ]; then
  printf 'native update failed\n' >&2
  exit 5
fi
touch "$PUSH_REMOTE_HOME/.wire-current"
"#;

struct Environment {
    values: Vec<(OsString, Option<OsString>)>,
}

impl Environment {
    fn set(values: &[(&str, OsString)]) -> Self {
        let previous = values
            .iter()
            .map(|(name, _)| (OsString::from(name), std::env::var_os(name)))
            .collect();
        for (name, value) in values {
            unsafe { std::env::set_var(name, value) };
        }
        Self { values: previous }
    }
}

impl Drop for Environment {
    fn drop(&mut self) {
        for (name, value) in self.values.drain(..).rev() {
            if let Some(value) = value {
                unsafe { std::env::set_var(name, value) };
            } else {
                unsafe { std::env::remove_var(name) };
            }
        }
    }
}

struct Machine {
    _temporary: TempDir,
    context: Context,
    root: PathBuf,
    remote: PathBuf,
    ssh_log: PathBuf,
    sync_log: PathBuf,
    bin: PathBuf,
}

impl Machine {
    fn new() -> Self {
        let temporary = TempDir::new().unwrap();
        let home = temporary.path().join("local-home");
        let root = home.join("dotfiles");
        let remote_home = temporary.path().join("remote-home");
        let remote = remote_home.join("dotfiles");
        let origin = temporary.path().join("origin.git");
        let bin = temporary.path().join("bin");
        fs::create_dir_all(root.join("config")).unwrap();
        fs::create_dir_all(root.join("shared/alpha")).unwrap();
        fs::create_dir_all(remote_home.join(".local/bin")).unwrap();
        fs::create_dir_all(&bin).unwrap();
        fs::write(root.join("config/targets.dotfile"), "").unwrap();
        fs::write(root.join("config/hosts.dotfile"), HOSTS).unwrap();
        fs::write(root.join("shared/alpha/value"), "alpha\n").unwrap();
        executable(&root.join("setup.sh"), SETUP_STUB);

        git(
            temporary.path(),
            &["init", "-q", "--bare", "-b", "main", path(&origin)],
        );
        git(temporary.path(), &["init", "-q", "-b", "main", path(&root)]);
        configure(&root);
        git(&root, &["remote", "add", "origin", path(&origin)]);
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-qm", "first"]);
        git(&root, &["push", "-q", "-u", "origin", "main"]);
        git(
            temporary.path(),
            &["clone", "-q", "-b", "main", path(&origin), path(&remote)],
        );
        configure(&remote);

        let ssh_log = temporary.path().join("ssh.log");
        let sync_log = temporary.path().join("sync.log");
        executable(
            &bin.join("ssh"),
            "#!/bin/sh\nif [ \"${PUSH_SSH_BLOCK:-0}\" = 1 ]; then printf 'blocked\\n' >> \"$PUSH_SSH_LOG\"; exec sleep 30; fi\nif [ \"${PUSH_SSH_HANDSHAKE_EOF:-0}\" = 1 ]; then printf '%s\\n' '{\"message\":\"hello\",\"version\":2,\"host\":\"archie\"}' '{\"message\":\"state\",\"branch\":\"main\"}'; printf 'zsh: read-only variable: status\\n' >&2; exit 1; fi\nif [ \"${PUSH_LEGACY:-0}\" = 1 ]; then\n  case \"$3\" in\n    *json_string*) printf 'protocol\\n' >> \"$PUSH_SSH_LOG\"; exit 0 ;;\n    *'git status --porcelain --branch'*) printf 'status\\n' >> \"$PUSH_SSH_LOG\"; printf '## main...origin/main\\n'; exit 0 ;;\n    *'git pull --ff-only || exit 1'*) printf 'sync\\n' >> \"$PUSH_SSH_LOG\"; exit 0 ;;\n  esac\nfi\nprintf 'session\\n' >> \"$PUSH_SSH_LOG\"\nexport HOME=\"$PUSH_REMOTE_HOME\"\nexec \"${PUSH_REMOTE_SHELL:-/bin/sh}\" -c \"$3\"\n",
        );
        executable(&remote_home.join(".local/bin/dotfile"), WIRE_STUB);
        let state = home.join(".config/dotfile");
        let context = Context::new(root.clone(), home, state).unwrap();
        Self {
            _temporary: temporary,
            context,
            root,
            remote,
            ssh_log,
            sync_log,
            bin,
        }
    }

    fn environment(&self, legacy: bool) -> Environment {
        let path = std::env::join_paths(
            std::iter::once(self.bin.as_os_str()).chain(
                std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                    .map(|path| path.into_os_string())
                    .collect::<Vec<_>>()
                    .iter()
                    .map(OsString::as_os_str),
            ),
        )
        .unwrap();
        Environment::set(&[
            ("PATH", path),
            ("SYSINFO_HOST", OsString::from("macie")),
            (
                "PUSH_REMOTE_HOME",
                self.remote.parent().unwrap().as_os_str().to_os_string(),
            ),
            ("PUSH_SSH_LOG", self.ssh_log.as_os_str().to_os_string()),
            ("PUSH_SYNC_LOG", self.sync_log.as_os_str().to_os_string()),
            (
                "PUSH_LEGACY",
                OsString::from(if legacy { "1" } else { "0" }),
            ),
            ("PUSH_SSH_BLOCK", OsString::from("0")),
            ("PUSH_SSH_HANDSHAKE_EOF", OsString::from("0")),
            ("PUSH_REMOTE_SHELL", OsString::from("/bin/sh")),
            ("PUSH_WIRE_SCENARIO", OsString::new()),
        ])
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(&self.ssh_log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

fn lock_environment() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

fn cli() -> SyncCli {
    SyncCli {
        profile: None,
        dry_run: false,
        overrides: Vec::new(),
        force: false,
        resolve: Resolution::Skip,
        push: true,
        to: None,
        verbose: false,
    }
}

fn run(machine: &Machine, cli: &SyncCli) -> (Result<(), String>, Vec<Event>) {
    cancel::reset();
    let sink = VecSink::default();
    let result = push::run(&machine.context, cli, &sink);
    (result, sink.events())
}

fn answer(choice: Choice) -> (decision::Client, std::thread::JoinHandle<Prompt>) {
    let (client, server) = decision::channel();
    let thread = std::thread::spawn(move || {
        loop {
            if let Some(request) = server.try_recv() {
                let prompt = request.prompt.clone();
                server.respond(&request, choice).unwrap();
                return prompt;
            }
            std::thread::yield_now();
        }
    });
    (client, thread)
}

fn git(directory: &Path, arguments: &[&str]) -> Output {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?}: {}",
        arguments,
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn configure(directory: &Path) {
    git(directory, &["config", "user.email", "push@example.com"]);
    git(directory, &["config", "user.name", "push test"]);
}

fn executable(path: &Path, source: &str) {
    fs::write(path, source).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }
}

fn path(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn push_structured_session_transfers_commits_and_streams_remote_phases() {
    let _lock = lock_environment();
    let machine = Machine::new();
    let _environment = machine.environment(false);
    fs::write(machine.root.join("shared/alpha/value"), "beta\n").unwrap();
    git(&machine.root, &["commit", "-qam", "second"]);

    let (result, events) = run(&machine, &cli());

    assert_eq!(result, Ok(()));
    assert_eq!(machine.calls(), ["session"]);
    assert_eq!(
        fs::read_to_string(machine.remote.join("shared/alpha/value")).unwrap(),
        "beta\n"
    );
    assert_eq!(fs::read_to_string(&machine.sync_log).unwrap(), "sync\n");
    assert!(events.iter().any(|event| matches!(
        event,
        Event::Item {
            action: dotfile_cli::event::Action::Push,
            detail,
            ..
        } if detail == "1 committed change(s)"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        Event::Progress {
            phase: Phase::Remote,
            completed: 2,
            ..
        }
    )));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Started { .. } | Event::Finished(_)))
    );
    assert!(events.iter().any(|event| matches!(
        event,
        Event::Item { path, .. } if path == Path::new("archie:/remote/settings.json")
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        Event::Warning { message, .. } if message == "archie: remote warning"
    )));
}

#[test]
fn push_structured_session_is_compatible_with_zsh() {
    if Command::new("zsh").arg("--version").output().is_err() {
        return;
    }
    let _lock = lock_environment();
    let machine = Machine::new();
    let _environment = machine.environment(false);
    let _shell = Environment::set(&[("PUSH_REMOTE_SHELL", OsString::from("zsh"))]);

    let (result, _) = run(&machine, &cli());

    assert_eq!(result, Ok(()));
    assert_eq!(machine.calls(), ["session"]);
}

#[test]
fn push_noop_skips_remote_pull_and_native_update() {
    let _lock = lock_environment();
    let machine = Machine::new();
    let _environment = machine.environment(false);
    git(
        &machine.remote,
        &["remote", "set-url", "origin", "/missing-origin"],
    );

    let (result, _) = run(&machine, &cli());

    assert_eq!(result, Ok(()));
    assert_eq!(fs::read_to_string(&machine.sync_log).unwrap(), "sync\n");
}

#[test]
fn push_reports_an_incomplete_handshake_before_writing_to_the_remote() {
    let _lock = lock_environment();
    let machine = Machine::new();
    let _environment = machine.environment(false);
    let _scenario = Environment::set(&[("PUSH_SSH_HANDSHAKE_EOF", OsString::from("1"))]);

    let (result, _) = run(&machine, &cli());

    let error = result.unwrap_err();
    assert!(error.contains("read-only variable: status"));
    assert!(!error.contains("Broken pipe"));
}

#[test]
fn push_wire_round_trips_remote_merge_and_target_decisions_on_one_ssh() {
    let _lock = lock_environment();
    let machine = Machine::new();
    let _environment = machine.environment(false);
    let _scenario = Environment::set(&[("PUSH_WIRE_SCENARIO", OsString::from("decisions"))]);
    cancel::reset();
    let plan = push::preflight(&machine.context, &cli()).unwrap();
    let sink = VecSink::default();
    let (decisions, server) = decision::channel();
    let responder = std::thread::spawn(move || {
        let mut prompts = Vec::new();
        for choice in [Choice::Repo, Choice::Target(1)] {
            loop {
                if let Some(request) = server.try_recv() {
                    prompts.push(request.prompt.clone());
                    server.respond(&request, choice).unwrap();
                    break;
                }
                std::thread::yield_now();
            }
        }
        prompts
    });

    let result =
        push::run_preflighted_with_decisions(&machine.context, &cli(), plan, &sink, &decisions);

    assert_eq!(result, Ok(()));
    assert_eq!(machine.calls(), ["session"]);
    let prompts = responder.join().unwrap();
    assert!(matches!(
        &prompts[0],
        Prompt::Merge { path, .. } if path == Path::new("archie:/remote/settings.json")
    ));
    assert!(matches!(
        &prompts[1],
        Prompt::MergeTarget {
            path,
            targets,
            default: 1,
            ..
        } if path == Path::new("archie:/remote/settings.json")
            && targets == &["shared".to_string(), "macos".to_string()]
    ));
    let responses = fs::read_to_string(&machine.sync_log).unwrap();
    assert!(responses.contains("\"id\":41"));
    assert!(responses.contains("\"id\":42"));
    assert!(responses.contains("\"choice\":\"repo\""));
    assert!(responses.contains("\"choice\":\"target\",\"target\":1"));
}

#[test]
fn push_wire_updates_a_compatible_but_stale_remote_before_sync() {
    let _lock = lock_environment();
    let machine = Machine::new();
    let _environment = machine.environment(false);
    let _scenario = Environment::set(&[("PUSH_WIRE_SCENARIO", OsString::from("stale"))]);

    let (result, _) = run(&machine, &cli());

    assert_eq!(result, Ok(()));
    assert_eq!(machine.calls(), ["session"]);
    assert_eq!(
        fs::read_to_string(&machine.sync_log).unwrap(),
        "update\nsync\n"
    );
}

#[test]
fn push_wire_reports_update_failure_without_running_stale_sync() {
    let _lock = lock_environment();
    let machine = Machine::new();
    let _environment = machine.environment(false);
    let _scenario = Environment::set(&[("PUSH_WIRE_SCENARIO", OsString::from("update-fail"))]);

    let (result, _) = run(&machine, &cli());

    let error = result.unwrap_err();
    assert!(error.contains("native update failed"));
    assert!(error.contains("setup.sh --commands-only"));
    assert_eq!(machine.calls(), ["session"]);
    assert_eq!(fs::read_to_string(&machine.sync_log).unwrap(), "update\n");
}

#[test]
fn push_wire_rejects_version_malformed_and_early_eof_frames() {
    let _lock = lock_environment();
    for (scenario, expected) in [
        ("mismatch", "incompatible"),
        ("malformed", "invalid push protocol frame"),
        ("error-eof", "remote failed"),
        ("eof", "ended unexpectedly"),
    ] {
        let machine = Machine::new();
        let _environment = machine.environment(false);
        let _scenario = Environment::set(&[("PUSH_WIRE_SCENARIO", OsString::from(scenario))]);

        let (result, _) = run(&machine, &cli());

        assert!(
            result.unwrap_err().contains(expected),
            "scenario {scenario} should mention {expected}"
        );
        assert_eq!(machine.calls(), ["session"]);
    }
}

#[test]
fn push_refuses_a_branch_behind_its_upstream_before_network_access() {
    let _lock = lock_environment();
    let machine = Machine::new();
    let _environment = machine.environment(false);
    fs::write(machine.remote.join("shared/alpha/value"), "remote commit\n").unwrap();
    git(&machine.remote, &["commit", "-qam", "remote"]);
    git(&machine.remote, &["push", "-q"]);

    let (result, _) = run(&machine, &cli());

    let error = result.unwrap_err();
    assert!(error.contains("1 commit(s) behind origin/main"));
    assert!(error.contains("pull with --ff-only or rebase"));
    assert!(!machine.ssh_log.exists());
}

#[test]
fn sync_push_rejects_initial_dirt_before_local_reconciliation_or_ssh() {
    let _lock = lock_environment();
    let machine = Machine::new();
    let _environment = machine.environment(false);
    let config_home = machine.context.home.join(".config");
    fs::create_dir_all(machine.root.join("environment/test")).unwrap();
    fs::write(machine.root.join("environment/test/manifest"), "shared\n").unwrap();
    fs::write(
        machine.root.join("config/targets.dotfile"),
        "shared/alpha/value = ~/.linked-value\n",
    )
    .unwrap();
    git(&machine.root, &["add", "-A"]);
    git(&machine.root, &["commit", "-qm", "sync fixture"]);
    git(&machine.root, &["push", "-q"]);
    fs::write(machine.root.join("shared/alpha/value"), "dirty\n").unwrap();
    let _discovery = Environment::set(&[
        ("DOTFILE_ROOT", machine.root.as_os_str().to_os_string()),
        ("HOME", machine.context.home.as_os_str().to_os_string()),
        ("XDG_CONFIG_HOME", config_home.into_os_string()),
    ]);
    let mut options = cli();
    options.profile = Some("test".to_string());
    let (decisions, _server) = decision::channel();
    let sink = VecSink::default();
    cancel::reset();

    let result = dotfile_cli::sync::run(&options, &sink, &decisions);

    assert!(result.unwrap_err().contains("uncommitted changes"));
    assert!(!machine.context.home.join(".linked-value").exists());
    assert!(!machine.ssh_log.exists());
}

#[test]
fn push_race_checks_origin_again_after_preflight_before_ssh() {
    let _lock = lock_environment();
    let machine = Machine::new();
    let _environment = machine.environment(false);
    cancel::reset();
    let options = cli();
    let plan = push::preflight(&machine.context, &options).unwrap();
    fs::write(machine.remote.join("shared/alpha/value"), "advanced\n").unwrap();
    git(&machine.remote, &["commit", "-qam", "advance"]);
    git(&machine.remote, &["push", "-q"]);

    let result = push::run_preflighted(&machine.context, &options, plan, &VecSink::default());

    assert!(result.unwrap_err().contains("fetch first"));
    assert!(!machine.ssh_log.exists());
}

#[test]
fn push_dry_run_contacts_no_peer_and_changes_nothing() {
    let _lock = lock_environment();
    let machine = Machine::new();
    let _environment = machine.environment(false);
    fs::write(machine.root.join("shared/alpha/value"), "uncommitted\n").unwrap();
    let before = git(&machine.root, &["status", "--porcelain"]).stdout;
    let mut options = cli();
    options.dry_run = true;

    let (result, _) = run(&machine, &options);

    assert_eq!(result, Ok(()));
    assert!(!machine.ssh_log.exists());
    assert!(!machine.sync_log.exists());
    assert_eq!(
        git(&machine.root, &["status", "--porcelain"]).stdout,
        before
    );
}

#[test]
fn push_refuses_local_uncommitted_changes_before_network_access() {
    let _lock = lock_environment();
    let machine = Machine::new();
    let _environment = machine.environment(false);
    fs::write(machine.root.join("shared/alpha/value"), "uncommitted\n").unwrap();

    let (result, _) = run(&machine, &cli());

    let error = result.unwrap_err();
    assert!(error.contains("review and commit them before dotfile sync -p"));
    assert!(!machine.ssh_log.exists());
}

#[test]
fn push_refuses_remote_changes_without_force_and_preserves_them() {
    let _lock = lock_environment();
    let machine = Machine::new();
    let _environment = machine.environment(false);
    fs::write(machine.remote.join("shared/alpha/value"), "remote edit\n").unwrap();

    let (result, _) = run(&machine, &cli());

    assert!(result.unwrap_err().contains("rerun with --force"));
    assert_eq!(machine.calls(), ["session"]);
    assert_eq!(
        fs::read_to_string(machine.remote.join("shared/alpha/value")).unwrap(),
        "remote edit\n"
    );
    assert!(!machine.sync_log.exists());
}

#[test]
fn push_force_discards_remote_changes_and_forwards_repo_resolution() {
    let _lock = lock_environment();
    let machine = Machine::new();
    let _environment = machine.environment(false);
    fs::write(machine.remote.join("shared/alpha/value"), "remote edit\n").unwrap();
    let mut options = cli();
    options.force = true;

    let (result, _) = run(&machine, &options);

    assert_eq!(result, Ok(()));
    assert_eq!(
        fs::read_to_string(machine.remote.join("shared/alpha/value")).unwrap(),
        "alpha\n"
    );
    assert_eq!(
        fs::read_to_string(&machine.sync_log).unwrap(),
        "sync --force\n"
    );
}

#[test]
fn push_decision_client_routes_remote_changes_and_discards_only_on_discard() {
    let _lock = lock_environment();
    let machine = Machine::new();
    let _environment = machine.environment(false);
    fs::write(machine.remote.join("shared/alpha/value"), "remote edit\n").unwrap();
    cancel::reset();
    let plan = push::preflight(&machine.context, &cli()).unwrap();
    let sink = VecSink::default();
    let (decisions, responder) = answer(Choice::Discard);

    let result =
        push::run_preflighted_with_decisions(&machine.context, &cli(), plan, &sink, &decisions);

    assert_eq!(result, Ok(()));
    assert_eq!(
        responder.join().unwrap(),
        Prompt::RemoteChanges {
            host: "archie".to_string(),
            changes: vec![" M shared/alpha/value".to_string()],
        }
    );
    assert_eq!(
        fs::read_to_string(machine.remote.join("shared/alpha/value")).unwrap(),
        "alpha\n"
    );
}

#[test]
fn push_decision_client_rejects_choices_from_the_wrong_prompt() {
    let _lock = lock_environment();
    cancel::reset();
    let (decisions, responder) = answer(Choice::Repo);

    let result = decisions.discard_remote_changes("archie", &[" M value".to_string()]);

    assert!(result.unwrap_err().contains("invalid remote choice Repo"));
    assert!(matches!(
        responder.join().unwrap(),
        Prompt::RemoteChanges { .. }
    ));
}

#[test]
fn push_stops_on_remote_branch_mismatch() {
    let _lock = lock_environment();
    let machine = Machine::new();
    let _environment = machine.environment(false);
    git(&machine.remote, &["checkout", "-qb", "other"]);

    let (result, _) = run(&machine, &cli());

    let error = result.unwrap_err();
    assert!(error.contains("archie is on 'other' but this machine is on 'main'"));
    assert!(!machine.sync_log.exists());
}

#[test]
fn push_named_host_works_when_push_boolean_is_false() {
    let _lock = lock_environment();
    let machine = Machine::new();
    let _environment = machine.environment(false);
    let mut options = cli();
    options.push = false;
    options.to = Some("archie".to_string());

    let (result, _) = run(&machine, &options);

    assert_eq!(result, Ok(()));
    assert_eq!(machine.calls(), ["session"]);
}

#[test]
fn push_legacy_fallback_completes_without_a_protocol_hello() {
    let _lock = lock_environment();
    let machine = Machine::new();
    let _environment = machine.environment(true);
    cancel::reset();
    let options = cli();
    let plan = push::preflight(&machine.context, &options).unwrap();
    let (decisions, _server) = decision::channel();

    let result = push::run_preflighted_with_decisions_summary(
        &machine.context,
        &options,
        plan,
        &VecSink::default(),
        &decisions,
    );

    assert_eq!(result, Ok(None));
    assert_eq!(machine.calls(), ["protocol", "status", "sync"]);
}

#[test]
fn push_unknown_host_fails_without_network_access() {
    let _lock = lock_environment();
    let machine = Machine::new();
    let _environment = machine.environment(false);
    let mut options = cli();
    options.to = Some("nosuch".to_string());

    let (result, _) = run(&machine, &options);

    let error = result.unwrap_err();
    assert!(error.contains("unknown machine 'nosuch'"));
    assert!(error.contains("archie, macie"));
    assert!(!machine.ssh_log.exists());
}

#[test]
fn push_requires_ssh_before_inspecting_git() {
    let _lock = lock_environment();
    let machine = Machine::new();
    let empty_path = machine._temporary.path().join("empty-path");
    fs::create_dir(&empty_path).unwrap();
    let _environment = Environment::set(&[
        ("PATH", empty_path.into_os_string()),
        ("SYSINFO_HOST", OsString::from("macie")),
    ]);

    let (result, _) = run(&machine, &cli());

    assert!(result.unwrap_err().contains("ssh is not installed"));
    assert!(!machine.ssh_log.exists());
}

#[test]
fn push_requires_an_upstream_before_network_access() {
    let _lock = lock_environment();
    let machine = Machine::new();
    let _environment = machine.environment(false);
    git(&machine.root, &["branch", "--unset-upstream"]);

    let (result, _) = run(&machine, &cli());

    assert!(result.unwrap_err().contains("tracks no remote branch"));
    assert!(!machine.ssh_log.exists());
}

#[test]
fn push_honors_cancellation_before_starting_commands() {
    let _lock = lock_environment();
    let machine = Machine::new();
    let _environment = machine.environment(false);
    cancel::request();
    let sink = VecSink::default();

    let result = push::run(&machine.context, &cli(), &sink);

    assert_eq!(result, Err("cancelled".to_string()));
    assert!(!machine.ssh_log.exists());
    cancel::reset();
}

#[test]
fn push_cancellation_terminates_and_waits_for_an_active_ssh_child() {
    let _lock = lock_environment();
    let machine = Machine::new();
    let _environment = machine.environment(false);
    let _blocking = Environment::set(&[("PUSH_SSH_BLOCK", OsString::from("1"))]);
    let context = machine.context.clone();
    let started = std::time::Instant::now();
    let worker = std::thread::spawn(move || {
        cancel::reset();
        push::run(&context, &cli(), &VecSink::default())
    });
    while machine.calls().is_empty() && started.elapsed() < std::time::Duration::from_secs(2) {
        std::thread::yield_now();
    }
    assert_eq!(machine.calls(), ["blocked"]);

    cancel::request();
    let result = worker.join().unwrap();

    assert_eq!(result, Err("cancelled".to_string()));
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    cancel::reset();
}
