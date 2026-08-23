//! One glance at everything transport selection depends on.
//!
//! Each line is a fact dmux would otherwise act on silently: which machine
//! this is, whether wezterm and tmux are within reach, whether the cable
//! answers, what `dmux -` would attach, and who besides this user can reach
//! the registry directory. The slow probes run on their own threads so the
//! whole report costs one ssh timeout, not the sum. `--format json` emits the
//! probes inside the one versioned document every bounded command emits
//! (plan §16.2); the deprecated `--json` emits the same probes as a bare
//! `name: {ok, detail}` object for scripts; the human report is unchanged.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::Duration;

use dmux::backend::scope::{
    self, Liveness, ManagedTarget, ObservedIncarnation, PublishedIncarnation,
};
use dmux::backend::tmux::TmuxProvider;
use dmux::backend::wez::WezProvider;
use dmux::backend::{InventoryOutcome, InventoryScope, Provider};
use dmux::inventory;
use dmux::locks::{self, LockMode, LockScope};
use dmux::ls_cli::{LiveIncarnationProbe, unpublished_state_detail};
use dmux::model::{Backend, BackendInstanceUid};
use dmux::output::{self, OutputFormat};
use dmux::registry::{
    self, DirExposure, HolderLiveness, Lease, LeaseScope, Registry, RegistryConfig, probe_pid,
};
use dmux::runtime;
use serde_json::{Value, json};
use workstation::Style;

use crate::hosts::{self, Context, Host};
use crate::state;

struct Report {
    this: Host,
    peer: Host,
    wezterm: bool,
    tmux: Option<usize>,
    usb: Option<Duration>,
    ssh: bool,
    /// Where `DMUX_WEZ_FIRST` came from, as `(ok, detail)` (ADR 012 WS-F.1).
    wez_first: (bool, String),
    /// The runtime directory this dmux resolves, and whether the
    /// `DMUX_RUNTIME_DIR` seam redirected it (ADR 012 WS-E.1 follow-up).
    runtime_dir: (bool, String),
    /// The read-only registry snapshot the instance lines were read from.
    registry_snapshot: (bool, String),
    /// The snapshot's authority revision (0 when no registry was read).
    authority_revision: u64,
    /// One line per backend instance, classified A–F (ADR 012 WS-B.4).
    instances: Vec<InstanceReport>,
}

pub fn run(context: &Context, json: bool, format: Option<OutputFormat>) -> ExitCode {
    let Ok(this) = Host::this() else {
        return ExitCode::FAILURE;
    };
    let peer = this.peer();
    let wezterm = thread::spawn(wezterm_ok);
    let tmux = thread::spawn(tmux_sessions);
    let usb = thread::spawn(|| hosts::usb_latency(hosts::PROBE_TIMEOUT));
    let peer_name = peer.name();
    let ssh = thread::spawn(move || ssh_ok(peer_name));
    // The registry is read from a read-only snapshot, never opened in
    // place (`Registry::open` takes locks and may repair); the instance
    // lines are the same A–F classification `ls` makes, plus what `ls`
    // never shows — the live descriptor, a fresh `stat` of the socket, and
    // the sentinel handshake under the published epoch.
    let snapshot = snapshot_registry();
    let (registry_snapshot, authority_revision, instances) = match &snapshot {
        Ok(snapshot) => (
            (true, snapshot.detail()),
            snapshot.revision,
            [Backend::Wez, Backend::Tmux]
                .into_iter()
                .map(|backend| probe_instance(&snapshot.registry, backend))
                .collect(),
        ),
        Err(why) => (
            (false, why.clone()),
            0,
            [Backend::Wez, Backend::Tmux]
                .into_iter()
                .map(|backend| InstanceReport::unavailable(backend, why))
                .collect(),
        ),
    };
    let report = Report {
        this,
        peer,
        wezterm: wezterm.join().unwrap_or(false),
        tmux: tmux.join().unwrap_or(None),
        usb: usb.join().unwrap_or(None),
        ssh: ssh.join().unwrap_or(false),
        wez_first: wez_first_detail(&wez_first_provenance()),
        runtime_dir: runtime_dir_detail(),
        registry_snapshot,
        authority_revision,
        instances,
    };
    // The envelope wins over the deprecated flag: it is the shape that
    // survives the release, and asking for both cannot mean two documents.
    if format == Some(OutputFormat::Json) {
        envelope(context, &report)
    } else if json {
        eprintln!("{}", crate::JSON_FLAG_HINT);
        machine(context, &report)
    } else {
        human(context, &report)
    }
}

fn human(context: &Context, report: &Report) -> ExitCode {
    let style = Style::for_stdout();
    let line = |label: &str, value: String| println!("{label:<15} {value}");
    let status = |ok: bool, detail: &str| {
        if ok {
            style.green(detail)
        } else {
            style.red(detail)
        }
    };
    line(
        "host",
        format!(
            "{} ({})",
            style.green(report.this.name()),
            std::env::consts::OS
        ),
    );
    line("peer", style.dim(&peer_detail(report.peer)));
    line(
        "inside wezterm",
        status(context.inside_wezterm, yes_no(context.inside_wezterm)),
    );
    line(
        "inside tmux",
        status(context.inside_tmux, yes_no(context.inside_tmux)),
    );
    line(
        "wezterm cli",
        status(report.wezterm, reachable(report.wezterm)),
    );
    line(
        "tmux server",
        status(report.tmux.is_some(), &tmux_detail(report.tmux)),
    );
    line(
        "usb link",
        status(report.usb.is_some(), &usb_detail(report.usb)),
    );
    line(
        &format!("ssh {}", report.peer.name()),
        status(report.ssh, reachable(report.ssh)),
    );
    let (state_ok, state_text) = state_detail(context);
    line(
        "state",
        if state_ok {
            style.dim(&state_text)
        } else {
            style.red(&state_text)
        },
    );
    let (registry_ok, registry_text) = registry_detail();
    line(
        "registry dir",
        if registry_ok {
            style.dim(&registry_text)
        } else {
            style.red(&registry_text)
        },
    );
    let (wez_first_ok, wez_first_text) = &report.wez_first;
    line(
        "wez-first flag",
        if *wez_first_ok {
            style.dim(wez_first_text)
        } else {
            style.red(wez_first_text)
        },
    );
    let (runtime_ok, runtime_text) = &report.runtime_dir;
    line(
        "runtime dir",
        if *runtime_ok {
            style.dim(runtime_text)
        } else {
            style.red(runtime_text)
        },
    );
    let (snapshot_ok, snapshot_text) = &report.registry_snapshot;
    line(
        "registry",
        if *snapshot_ok {
            style.dim(snapshot_text)
        } else {
            style.red(snapshot_text)
        },
    );
    for instance in &report.instances {
        let text = instance.human();
        line(
            &format!("{} instance", instance.backend),
            match instance.state {
                Some(state) if state.is_healthy() => style.green(&text),
                Some(InstanceState::A) => style.dim(&text),
                _ => style.red(&text),
            },
        );
        if let Some(remedy) = instance.remedy.as_deref() {
            line("", format!("remedy: {remedy}"));
        }
    }
    ExitCode::SUCCESS
}

fn machine(context: &Context, report: &Report) -> ExitCode {
    println!("{}", probes(context, report));
    ExitCode::SUCCESS
}

/// The §16.2 document: the same probes, versioned. A red probe is a finding,
/// not a failed command — doctor reports and exits 0 either way, so `ok`
/// stays true and `errors` empty.
fn envelope(context: &Context, report: &Report) -> ExitCode {
    println!(
        "{}",
        output::document(
            "doctor",
            true,
            probes(context, report),
            &[],
            report.authority_revision,
        )
    );
    ExitCode::SUCCESS
}

fn probes(context: &Context, report: &Report) -> serde_json::Value {
    let probe = |ok: bool, detail: String| serde_json::json!({ "ok": ok, "detail": detail });
    let (state_ok, state_text) = state_detail(context);
    let (registry_ok, registry_text) = registry_detail();
    serde_json::json!({
        "host": probe(
            true,
            format!("{} ({})", report.this.name(), std::env::consts::OS)
        ),
        "peer": probe(true, peer_detail(report.peer)),
        "inside_wezterm": probe(context.inside_wezterm, yes_no(context.inside_wezterm).to_string()),
        "inside_tmux": probe(context.inside_tmux, yes_no(context.inside_tmux).to_string()),
        "wezterm_cli": probe(report.wezterm, reachable(report.wezterm).to_string()),
        "tmux_server": probe(report.tmux.is_some(), tmux_detail(report.tmux)),
        "usb_link": probe(report.usb.is_some(), usb_detail(report.usb)),
        "ssh_peer": probe(report.ssh, reachable(report.ssh).to_string()),
        "state": probe(state_ok, state_text),
        "registry_dir": probe(registry_ok, registry_text),
        "wez_first": probe(report.wez_first.0, report.wez_first.1.clone()),
        "runtime_dir": probe(report.runtime_dir.0, report.runtime_dir.1.clone()),
        "registry_snapshot": probe(report.registry_snapshot.0, report.registry_snapshot.1.clone()),
        "backend_instances": report.instances.iter().map(InstanceReport::json).collect::<Vec<_>>(),
    })
}

fn yes_no(answer: bool) -> &'static str {
    if answer { "yes" } else { "no" }
}

fn reachable(ok: bool) -> &'static str {
    if ok { "reachable" } else { "unreachable" }
}

fn peer_detail(peer: Host) -> String {
    format!(
        "{} (usb {}, ts {})",
        peer.name(),
        peer.usb_address(),
        peer.ts_address()
    )
}

fn tmux_detail(sessions: Option<usize>) -> String {
    match sessions {
        Some(1) => "running (1 session)".to_string(),
        Some(count) => format!("running ({count} sessions)"),
        None => "not running".to_string(),
    }
}

fn usb_detail(latency: Option<Duration>) -> String {
    match latency {
        Some(latency) => format!("up ({} ms)", latency.as_millis()),
        None => "down".to_string(),
    }
}

fn state_detail(context: &Context) -> (bool, String) {
    match state::file() {
        Some(path) => (
            true,
            format!(
                "{} (last on {}: {})",
                path.display(),
                context.host.name(),
                state::previous(context.host).unwrap_or_else(|| "nothing".to_string())
            ),
        ),
        None => (false, "unavailable (no HOME)".to_string()),
    }
}

/// Who besides this user can reach the directory the registry sits in.
///
/// The registry file is `0600` and re-hardened on every open, but that only
/// closes the *contents*: a group- or world-traversable parent still leaks
/// the database's existence and name, its `-wal`/`-shm` sidecars and the
/// lock filenames, and a writable one lets another uid put files beside it.
/// The mode of a directory the user already had is deliberately never forced
/// — `--data-dir X` makes X itself the parent — so this is where that
/// decision becomes visible instead of silent. Reported, never repaired:
/// doctor is a report.
///
/// Only the production location is inspected; `doctor` takes no `--data-dir`
/// and never opens the registry, so this costs a couple of `stat` calls and
/// creates nothing.
fn registry_detail() -> (bool, String) {
    let Some(db_path) = registry::production_db_path() else {
        return (false, "unavailable (no HOME)".to_string());
    };
    match registry::parent_dir_exposure(&db_path) {
        Ok(exposure) => (exposure.is_private(), dir_detail(&exposure)),
        Err(error) => (
            false,
            format!(
                "{} (unreadable: {error})",
                db_path.parent().unwrap_or(&db_path).display()
            ),
        ),
    }
}

fn dir_detail(exposure: &DirExposure) -> String {
    format!("{} ({})", exposure.dir.display(), exposure.summary())
}

// ---------------------------------------------------------------------------
// Where DMUX_WEZ_FIRST comes from (ADR 012 WS-F.1)
//
// `launchctl setenv` and `systemctl --user set-environment` are runtime-only,
// so a canary that was enabled that way silently comes back legacy after a
// reboot — which is how Macie's registry and live mux diverged (ADR 012
// §3.1). The durable source is a per-host file: `~/.config/dmux/service.env`
// on macOS (copied into the launchd session by com.fredrir.dmux-env and read
// by dmux-mux-start.sh itself), `~/.config/environment.d/50-dmux.conf` on
// Linux (read by the systemd user manager). This section reports the flag at
// each layer a service or GUI could have inherited it from, so a canary
// report can say whether enablement survived the last boot. The flag is
// three-valued (ADR 010 §5): `1` states Wez-first, `0` states legacy, and
// anything else states no preference.

/// One layer's answer for `DMUX_WEZ_FIRST`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FlagLayer {
    /// The layer carries no value.
    Unset,
    /// The layer carries exactly this value.
    Value(String),
    /// The layer could not be read: the probe failed or the file is
    /// malformed, with the reason.
    Unavailable(String),
}

/// The flag at each layer, with every input injectable so the classification
/// is testable without `launchctl`, `systemctl`, or a home directory.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FlagProvenance {
    /// This process's own environment: what the `dmux` in this shell sees.
    process: FlagLayer,
    /// The service manager's session environment — `launchd` on macOS,
    /// `systemd` on Linux — which is what the mux job and the GUI inherit.
    manager: &'static str,
    manager_value: FlagLayer,
    /// The durable per-host file, displayed as written.
    file: String,
    file_value: FlagLayer,
}

/// The classification: `ok` when every layer that states a preference agrees
/// and, if anything is stated at all, the file states it too (so a reboot
/// reproduces it). Never repairs anything — doctor is a report.
fn wez_first_detail(provenance: &FlagProvenance) -> (bool, String) {
    let manager = provenance.manager;
    let file = &provenance.file;
    let layers = format!(
        "process={} {manager}={} file={}",
        flag_layer_text(&provenance.process),
        flag_layer_text(&provenance.manager_value),
        flag_layer_text(&provenance.file_value),
    );
    if let FlagLayer::Unavailable(why) = &provenance.manager_value {
        return (
            false,
            format!("{layers}; {manager} could not be read: {why}"),
        );
    }
    if let FlagLayer::Unavailable(why) = &provenance.file_value {
        return (
            false,
            format!("{layers}; {file} is malformed and applies nothing: {why}"),
        );
    }
    let stated = |layer: &FlagLayer| match layer {
        FlagLayer::Value(value) if value == "1" || value == "0" => Some(value.clone()),
        _ => None,
    };
    let process = stated(&provenance.process);
    let in_manager = stated(&provenance.manager_value);
    let in_file = stated(&provenance.file_value);
    let mut values: Vec<&String> = [&process, &in_manager, &in_file]
        .into_iter()
        .flatten()
        .collect();
    values.dedup();
    let Some(value) = values.first().copied() else {
        return (
            true,
            format!("{layers}; no preference stated anywhere, the tracked default applies"),
        );
    };
    if values.len() > 1 {
        return (
            false,
            format!(
                "{layers}; layers disagree: {}, then restart the mux and relaunch the GUI",
                reload_hint(manager)
            ),
        );
    }
    let meaning = if value == "1" { "Wez-first" } else { "legacy" };
    match (in_file.is_some(), in_manager.is_some()) {
        (true, true) => (
            true,
            format!("{layers}; durable {meaning}: {file} is loaded into {manager}"),
        ),
        (true, false) => (
            false,
            format!(
                "{layers}; {file} states {value} but {manager} does not carry it: {}, then restart the mux and relaunch the GUI",
                reload_hint(manager)
            ),
        ),
        (false, true) => (
            false,
            format!(
                "{layers}; runtime-only {meaning}: {manager} carries {value} but {file} does not, so a reboot clears it"
            ),
        ),
        (false, false) => (
            false,
            format!(
                "{layers}; this shell only: {value} is exported here but neither {manager} nor {file} carries it"
            ),
        ),
    }
}

fn flag_layer_text(layer: &FlagLayer) -> String {
    match layer {
        FlagLayer::Unset => "unset".to_string(),
        FlagLayer::Value(value) if value == "1" || value == "0" => value.clone(),
        FlagLayer::Value(value) => format!("{value:?}(no preference)"),
        FlagLayer::Unavailable(_) => "unreadable".to_string(),
    }
}

fn reload_hint(manager: &str) -> &'static str {
    if manager == "launchd" {
        "run `launchctl kickstart gui/$UID/com.fredrir.dmux-env`"
    } else {
        "run `systemctl --user daemon-reload`"
    }
}

/// The real inputs: this process, `launchctl getenv` or
/// `systemctl --user show-environment`, and the per-host file. All read-only.
fn wez_first_provenance() -> FlagProvenance {
    let process = match std::env::var_os("DMUX_WEZ_FIRST") {
        Some(value) => FlagLayer::Value(value.to_string_lossy().into_owned()),
        None => FlagLayer::Unset,
    };
    let (manager, manager_value, file) = if cfg!(target_os = "macos") {
        (
            "launchd",
            launchd_layer(),
            config_home().map(|home| home.join("dmux/service.env")),
        )
    } else {
        (
            "systemd",
            systemd_layer(),
            config_home().map(|home| home.join("environment.d/50-dmux.conf")),
        )
    };
    let (file, file_value) = match file {
        Some(path) => (path.display().to_string(), file_layer(&path)),
        None => (
            "the per-host file".to_string(),
            FlagLayer::Unavailable("neither XDG_CONFIG_HOME nor HOME is set".to_string()),
        ),
    };
    FlagProvenance {
        process,
        manager,
        manager_value,
        file,
        file_value,
    }
}

/// `$XDG_CONFIG_HOME`, else `$HOME/.config` — the same rule as
/// `dmux_service_env_path` in `shared/wezterm/mux/dmux-service-env.sh`.
fn config_home() -> Option<PathBuf> {
    match std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir)),
        None => Some(PathBuf::from(std::env::var_os("HOME")?).join(".config")),
    }
}

/// `launchctl getenv` prints the value, or an empty line when unset, and
/// exits 0 either way.
fn launchd_layer() -> FlagLayer {
    let output = Command::new("launchctl")
        .args(["getenv", "DMUX_WEZ_FIRST"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let value = String::from_utf8_lossy(&output.stdout);
            let value = value.trim_end_matches(['\n', '\r']);
            if value.is_empty() {
                FlagLayer::Unset
            } else {
                FlagLayer::Value(value.to_string())
            }
        }
        Ok(output) => FlagLayer::Unavailable(format!("launchctl getenv exited {}", output.status)),
        Err(error) => FlagLayer::Unavailable(format!("launchctl: {error}")),
    }
}

/// `systemctl --user show-environment` dumps the block the user manager
/// passes to every process it spawns, one `KEY=VALUE` per line (values with
/// shell-special characters are `$'…'`-quoted; the flag never is).
fn systemd_layer() -> FlagLayer {
    let output = Command::new("systemctl")
        .args(["--user", "show-environment"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match output {
        Ok(output) if output.status.success() => {
            manager_block_layer(&String::from_utf8_lossy(&output.stdout))
        }
        Ok(output) => FlagLayer::Unavailable(format!(
            "systemctl --user show-environment exited {}",
            output.status
        )),
        Err(error) => FlagLayer::Unavailable(format!("systemctl: {error}")),
    }
}

fn manager_block_layer(block: &str) -> FlagLayer {
    block
        .lines()
        .find_map(|line| line.strip_prefix("DMUX_WEZ_FIRST="))
        .map_or(FlagLayer::Unset, |value| {
            FlagLayer::Value(value.to_string())
        })
}

/// The file layer: absent is `Unset`; a malformed file is `Unavailable`,
/// because the shell readers refuse such a file whole and apply nothing.
fn file_layer(path: &Path) -> FlagLayer {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return FlagLayer::Unset,
        Err(error) => return FlagLayer::Unavailable(error.to_string()),
    };
    match parse_service_env(&text) {
        Ok(assignments) => assignments
            .into_iter()
            .rev()
            .find(|(key, _)| key == "DMUX_WEZ_FIRST")
            .map_or(FlagLayer::Unset, |(_, value)| FlagLayer::Value(value)),
        Err(why) => FlagLayer::Unavailable(why),
    }
}

/// The grammar of `dmux-service-env.sh`, byte for byte: blank lines and
/// `#` comments (after leading ASCII whitespace) are ignored; every other
/// line is `KEY=VALUE` with `KEY` matching `^DMUX_[A-Z0-9_]*$` and `VALUE`
/// matching `^[A-Za-z0-9_./:@+,-]*$`. Lines are split on `\n` only, so a
/// CRLF file is malformed here exactly as it is there. Any bad line
/// refuses the whole file; the error names how many and the first.
fn parse_service_env(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut assignments = Vec::new();
    let mut bad = Vec::new();
    for (index, raw) in text.split('\n').enumerate() {
        let line = raw.trim_start_matches(|c: char| c.is_ascii_whitespace());
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let number = index + 1;
        let Some((key, value)) = line.split_once('=') else {
            bad.push(format!("line {number}: expected KEY=VALUE"));
            continue;
        };
        let key_ok = key.strip_prefix("DMUX_").is_some_and(|rest| {
            rest.bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        });
        if !key_ok {
            bad.push(format!("line {number}: key must match ^DMUX_[A-Z0-9_]*$"));
            continue;
        }
        let value_ok = value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_./:@+,-".contains(&b));
        if !value_ok {
            bad.push(format!(
                "line {number}: value must match ^[A-Za-z0-9_./:@+,-]*$"
            ));
            continue;
        }
        assignments.push((key.to_string(), value.to_string()));
    }
    match bad.len() {
        0 => Ok(assignments),
        1 => Err(bad.remove(0)),
        count => Err(format!(
            "{count} malformed lines, first at {}",
            bad.remove(0)
        )),
    }
}

// ---------------------------------------------------------------------------
// Backend instances: the A–F classification (ADR 012 WS-B.4; plan §5.2)
//
// Both operator-facing epoch remedies in the crate end "re-run `dmux
// doctor`", and until now doctor could not observe an epoch at all (review
// finding #14). This section reports, per backend instance, what the
// registry published — epoch, pid, start token, socket dev/ino — against
// what the host shows: the process (`kill(pid, 0)` and the OS start
// witness), a fresh `stat` of the socket, the live Wez descriptor, the tmux
// server's own incarnation probe, and the sentinel handshake of an
// inventory pinned to the published epoch. The classification is the one
// `ls` makes (`backend::scope::resolve_managed_with` for A/B/E/F, the fence
// and recovery lease for C/D), so the two never disagree; doctor only adds
// the evidence.

/// The six instance states (review report 04; plan §5.2 as amended).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstanceState {
    /// Not registered.
    A,
    /// Registered without an endpoint.
    B,
    /// Registered, unpublished, idle.
    C,
    /// Registered, unpublished, a bootstrap or recovery in flight.
    D,
    /// Published, and the host agrees.
    E,
    /// Published, and the host disagrees or the process is dead.
    F,
}

impl InstanceState {
    fn letter(self) -> &'static str {
        match self {
            InstanceState::A => "A",
            InstanceState::B => "B",
            InstanceState::C => "C",
            InstanceState::D => "D",
            InstanceState::E => "E",
            InstanceState::F => "F",
        }
    }

    fn name(self) -> &'static str {
        match self {
            InstanceState::A => "not_registered",
            InstanceState::B => "unaddressable",
            InstanceState::C => "unpublished_idle",
            InstanceState::D => "coordinator_in_flight",
            InstanceState::E => "published_live",
            InstanceState::F => "stale_incarnation",
        }
    }

    /// Green in the human report: a live, agreeing incarnation.
    fn is_healthy(self) -> bool {
        self == InstanceState::E
    }
}

/// What the host showed for one instance, every part optional because each
/// is read independently and a missing one is reported as missing.
#[derive(Debug, Clone, Default)]
struct HostWitness {
    /// The liveness verdict for the published row, when one was published.
    liveness: Option<Liveness>,
    /// `true` when the non-blocking shared fence was refused (an exclusive
    /// holder exists); `None` when the lock file does not exist (nobody
    /// ever held it) or could not be tried.
    exclusive_held: Option<bool>,
    recovery_lease: Option<Lease>,
    /// The live Wez descriptor as a JSON object, or `{"unreadable": ..}`.
    descriptor: Option<Value>,
    /// A fresh `stat` of the recorded endpoint.
    socket: Option<Value>,
    /// The inventory under the published epoch, when one is published.
    inventory: Option<Value>,
    /// The epoch the live side presents, when it presents one: the
    /// descriptor's (wez) or the inventory's sentinel (either backend).
    live_epoch: Option<uuid::Uuid>,
}

#[derive(Debug, Clone)]
struct InstanceReport {
    backend: Backend,
    instance: Option<BackendInstanceUid>,
    /// `None` only when the registry could not be read at all.
    state: Option<InstanceState>,
    published: Option<PublishedIncarnation>,
    witness: HostWitness,
    detail: String,
    remedy: Option<String>,
}

impl InstanceReport {
    fn unavailable(backend: Backend, why: &str) -> InstanceReport {
        InstanceReport {
            backend,
            instance: None,
            state: None,
            published: None,
            witness: HostWitness::default(),
            detail: format!("registry unreadable: {why}"),
            remedy: None,
        }
    }

    fn human(&self) -> String {
        match self.state {
            Some(state) => format!("{} ({}) {}", state.letter(), state.name(), self.detail),
            None => self.detail.clone(),
        }
    }

    fn json(&self) -> Value {
        let published = self.published.as_ref().map(|published| {
            json!({
                "epoch": published.epoch.0.to_string(),
                "pid": published.pid,
                "start_token": published.start_token,
                "socket_dev": published.socket_dev,
                "socket_ino": published.socket_ino,
            })
        });
        let (process, witness) = match &self.witness.liveness {
            Some(Liveness::Live(observed)) => (Some("alive"), Some(observed.to_string())),
            Some(Liveness::Stale(ObservedIncarnation::ProcessDead { .. })) => (
                Some("dead"),
                self.witness.liveness.as_ref().map(liveness_text),
            ),
            Some(Liveness::Stale(observed)) => (Some("alive"), Some(observed.to_string())),
            Some(Liveness::Unobservable(why)) => {
                (Some("unobservable"), Some(format!("unobservable: {why}")))
            }
            None => (None, None),
        };
        json!({
            "backend": self.backend.as_str(),
            "instance": self.instance.map(|uid| uid.0.to_string()),
            "state": self.state.map(InstanceState::letter),
            "state_name": self.state.map(InstanceState::name),
            "published": published,
            "observed": {
                "process": process,
                "witness": witness,
                "exclusive_lock_held": self.witness.exclusive_held,
                "recovery_lease": self.witness.recovery_lease.as_ref().map(lease_json),
                "descriptor": self.witness.descriptor,
                "socket": self.witness.socket,
                "inventory": self.witness.inventory,
            },
            "detail": self.detail,
            "remedy": self.remedy,
        })
    }
}

fn liveness_text(liveness: &Liveness) -> String {
    match liveness {
        Liveness::Live(observed) | Liveness::Stale(observed) => observed.to_string(),
        Liveness::Unobservable(why) => format!("unobservable: {why}"),
    }
}

fn lease_json(lease: &Lease) -> Value {
    json!({
        "holder_pid": lease.holder_pid,
        "holder_alive": lease.holder_pid.map(|pid| probe_pid(pid) == HolderLiveness::Alive),
        "expires_at": lease.expires_at,
        "fencing_token": lease.fencing_token,
    })
}

/// The pure classification over injected inputs: the resolver's verdict,
/// the two C/D witnesses, and the live side's epoch. `(state, detail,
/// remedy)`; the remedies are plan §5.2's as amended and report 04's column
/// "safe operator advice", and none of them is an unconditional restart.
fn classify(
    backend: Backend,
    target: &ManagedTarget,
    witness: &HostWitness,
) -> (InstanceState, String, Option<String>) {
    match target {
        ManagedTarget::Unregistered => (
            InstanceState::A,
            format!("no {backend} backend instance is registered for this owner"),
            Some(match backend {
                Backend::Wez => "nothing is enrolled: a managed (flag-on) service start registers \
                                 and publishes the Wez instance; `dmux adopt` then brings existing \
                                 workspaces under it"
                    .to_string(),
                Backend::Tmux => "nothing is enrolled: `dmux _tmux-bootstrap --namespace <ns>` \
                                  against a running managed server registers and publishes the \
                                  tmux instance; `dmux adopt` then brings existing sessions under \
                                  it"
                .to_string(),
            }),
        ),
        ManagedTarget::Unaddressable(instance) => (
            InstanceState::B,
            format!("instance {} is registered without an endpoint", instance.0),
            Some(
                "re-register the instance with its endpoint (socket path / -L namespace); \
                 nothing can address it until then"
                    .to_string(),
            ),
        ),
        ManagedTarget::Unpublished(instance) => {
            let exclusive = witness.exclusive_held == Some(true);
            let lease = witness.recovery_lease.as_ref();
            let detail = unpublished_state_detail(backend, *instance, exclusive, lease);
            if exclusive || lease.is_some() {
                (
                    InstanceState::D,
                    detail,
                    Some(
                        "a bootstrap or recovery is in flight; wait, then re-run `dmux doctor`"
                            .to_string(),
                    ),
                )
            } else {
                (
                    InstanceState::C,
                    detail,
                    Some(match backend {
                        Backend::Wez => {
                            "wait for the managed mux to coordinate: a managed (flag-on) \
                                         service start publishes the incarnation; if the service \
                                         is up and still unpublished it started without \
                                         DMUX_WEZ_FIRST (see the wez-first flag line), and a \
                                         flag-on restart is safe only while it holds no user panes"
                                .to_string()
                        }
                        Backend::Tmux => "run `dmux _tmux-bootstrap --namespace <ns>` against a \
                                          running managed tmux server; it publishes the \
                                          incarnation"
                            .to_string(),
                    }),
                )
            }
        }
        ManagedTarget::StaleIncarnation {
            instance,
            published,
            observed,
        } => (
            InstanceState::F,
            format!(
                "instance {} publishes a stale incarnation: the registry records {published}, \
                 but the host shows {observed}{}",
                instance.0,
                live_side_note(witness)
            ),
            Some(stale_remedy(backend, published)),
        ),
        ManagedTarget::Managed { instance, scope } => {
            let published_epoch = scope
                .expected_epoch()
                .expect("a Managed scope carries its published epoch");
            if let Some(live) = witness.live_epoch
                && live != published_epoch.0
            {
                return (
                    InstanceState::F,
                    format!(
                        "instance {} publishes epoch {} and its process is alive, but the live \
                         server presents epoch {live}{}",
                        instance.0,
                        published_epoch.0,
                        live_side_note(witness)
                    ),
                    Some(stale_remedy_for_epoch(backend, published_epoch.0)),
                );
            }
            (
                InstanceState::E,
                format!(
                    "instance {} publishes epoch {} and the host agrees{}{}",
                    instance.0,
                    published_epoch.0,
                    witness
                        .liveness
                        .as_ref()
                        .map(|liveness| format!(" ({})", liveness_text(liveness)))
                        .unwrap_or_default(),
                    live_side_note(witness)
                ),
                None,
            )
        }
    }
}

fn stale_remedy(backend: Backend, published: &PublishedIncarnation) -> String {
    stale_remedy_for_epoch(backend, published.epoch.0)
}

fn stale_remedy_for_epoch(backend: Backend, epoch: uuid::Uuid) -> String {
    format!(
        "the published incarnation is stale: if the managed service holds no user panes, a \
         managed (flag-on) restart republishes it; otherwise, once the published process is \
         confirmed gone, `dmux repair retire-incarnation --backend {backend} --epoch {epoch}` \
         clears the row and the next managed start publishes afresh"
    )
}

/// What the live side says, for the detail line: the descriptor's state and
/// epoch (wez) and the inventory's verdict.
fn live_side_note(witness: &HostWitness) -> String {
    let mut parts = Vec::new();
    if let Some(descriptor) = &witness.descriptor {
        parts.push(match descriptor.get("unreadable").and_then(Value::as_str) {
            Some(why) => format!("descriptor unreadable ({why})"),
            None => format!(
                "descriptor {} epoch {} pid {}",
                descriptor["state"].as_str().unwrap_or("?"),
                descriptor["epoch"].as_str().unwrap_or("?"),
                descriptor["pid"]
            ),
        });
    }
    if let Some(inventory) = &witness.inventory {
        parts.push(match inventory["outcome"].as_str() {
            Some("complete") => format!(
                "inventory complete under the published epoch ({} user rows)",
                inventory["rows"]
            ),
            Some(outcome) => format!(
                "inventory {outcome}: {}",
                inventory["detail"].as_str().unwrap_or("")
            ),
            None => "inventory ?".to_string(),
        });
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("; {}", parts.join("; "))
    }
}

/// A read-only snapshot of the production registry: the live file is opened
/// `SQLITE_OPEN_READ_ONLY` and copied through SQLite's online backup API
/// (a consistent read transaction, so a concurrent writer cannot tear it)
/// into a scratch directory, and the `Registry` is opened on the copy with
/// a scratch lock directory. `Registry::open` is never run on the live path:
/// it takes the maintenance gate, re-hardens the file mode and may migrate,
/// and doctor is a report. A WAL database whose `-shm` has not been created
/// cannot be opened read-only; that reads as "unavailable", not as a reason
/// to open it read-write.
struct RegistrySnapshot {
    registry: Registry,
    source: PathBuf,
    revision: u64,
    _scratch: tempfile::TempDir,
}

impl RegistrySnapshot {
    fn detail(&self) -> String {
        format!(
            "read-only snapshot of {} (authority revision {})",
            self.source.display(),
            self.revision
        )
    }
}

fn snapshot_registry() -> Result<RegistrySnapshot, String> {
    let source = registry::production_db_path().ok_or_else(|| "no HOME".to_string())?;
    if !source.exists() {
        return Err(format!("{} does not exist", source.display()));
    }
    let scratch = tempfile::Builder::new()
        .prefix("dmux-doctor-")
        .tempdir()
        .map_err(|e| format!("scratch directory: {e}"))?;
    let copy = scratch.path().join("registry.sqlite3");
    {
        let live = rusqlite::Connection::open_with_flags(
            &source,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| format!("open {} read-only: {e}", source.display()))?;
        let mut destination =
            rusqlite::Connection::open(&copy).map_err(|e| format!("scratch copy: {e}"))?;
        let backup = rusqlite::backup::Backup::new(&live, &mut destination)
            .map_err(|e| format!("snapshot {}: {e}", source.display()))?;
        backup
            .run_to_completion(64, Duration::from_millis(5), None)
            .map_err(|e| format!("snapshot {}: {e}", source.display()))?;
    }
    let registry = Registry::open(RegistryConfig::new(&copy, scratch.path().join("locks")))
        .map_err(|e| format!("open snapshot: {e}"))?;
    let revision = registry
        .authority_head()
        .map_err(|e| format!("snapshot authority head: {e}"))?
        .revision;
    Ok(RegistrySnapshot {
        registry,
        source,
        revision,
        _scratch: scratch,
    })
}

/// The whole probe for one backend: resolve (the same resolver and the same
/// server-asking probe `ls` uses), then gather what the host shows, then
/// classify. Every read here is read-only: the fence is *tried* shared and
/// released at once, and only when its lock file already exists, so doctor
/// creates nothing in the runtime directory either.
fn probe_instance(registry: &Registry, backend: Backend) -> InstanceReport {
    let target = match scope::resolve_managed_with(registry, backend, &LiveIncarnationProbe) {
        Ok(target) => target,
        Err(error) => {
            return InstanceReport::unavailable(backend, &format!("resolve {backend}: {error}"));
        }
    };
    let mut witness = HostWitness::default();
    let instance = target.instance();
    let runtime_dir = runtime::dmux_runtime_dir().ok();

    if let Some(instance) = instance
        && let Some(runtime_dir) = &runtime_dir
    {
        let scope_lock = LockScope::BackendInstance(instance);
        if runtime_dir.join(scope_lock.file_name()).exists() {
            witness.exclusive_held = locks::try_acquire(runtime_dir, scope_lock, LockMode::Shared)
                .ok()
                .map(|held| held.is_none());
        } else {
            witness.exclusive_held = Some(false);
        }
        witness.recovery_lease = registry
            .current_lease(&LeaseScope::Recovery(instance))
            .ok()
            .flatten();
    }

    let endpoint = match &target {
        ManagedTarget::Managed { scope, .. } => Some(scope.endpoint.clone()),
        ManagedTarget::StaleIncarnation { instance, .. } | ManagedTarget::Unpublished(instance) => {
            registry
                .backend_instance_info(*instance)
                .ok()
                .and_then(|info| info.socket_path)
        }
        _ => None,
    };
    let published = match &target {
        ManagedTarget::Managed { instance, .. }
        | ManagedTarget::StaleIncarnation { instance, .. } => registry
            .backend_server(*instance)
            .ok()
            .and_then(|record| PublishedIncarnation::from_record(&record)),
        _ => None,
    };

    if let (Some(endpoint), Some(published)) = (&endpoint, &published) {
        witness.liveness = Some(match &target {
            ManagedTarget::StaleIncarnation { observed, .. } => Liveness::Stale(observed.clone()),
            _ => scope::liveness(backend, endpoint, published, &LiveIncarnationProbe),
        });
    }
    if let Some(endpoint) = &endpoint {
        witness.socket = Some(socket_json(backend, endpoint));
    }
    if backend == Backend::Wez
        && let Some(runtime_dir) = &runtime_dir
    {
        let (descriptor, epoch) = descriptor_json(runtime_dir);
        witness.descriptor = descriptor;
        witness.live_epoch = epoch;
    }
    if let (Some(endpoint), Some(published)) = (&endpoint, &published) {
        let pinned = InventoryScope::managed(backend, endpoint.clone(), published.epoch);
        let outcome = match backend {
            Backend::Wez => {
                let (bin, config) = runtime::production_wez_paths();
                WezProvider::new(bin, config).inventory(&pinned)
            }
            Backend::Tmux => TmuxProvider::new(endpoint.clone()).inventory(&pinned),
        };
        let (value, live_epoch) = inventory_json(&outcome);
        witness.inventory = Some(value);
        if let Some(live_epoch) = live_epoch {
            witness.live_epoch = Some(live_epoch);
        }
    }

    let (state, detail, remedy) = classify(backend, &target, &witness);
    InstanceReport {
        backend,
        instance,
        state: Some(state),
        published,
        witness,
        detail,
        remedy,
    }
}

fn socket_json(backend: Backend, endpoint: &str) -> Value {
    let path = match backend {
        Backend::Wez => PathBuf::from(endpoint),
        Backend::Tmux => scope::tmux_socket_path(endpoint),
    };
    match std::fs::metadata(&path) {
        Ok(meta) => json!({
            "path": path.display().to_string(),
            "dev": meta.dev(),
            "ino": meta.ino(),
            "is_socket": std::os::unix::fs::FileTypeExt::is_socket(&meta.file_type()),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            json!({ "path": path.display().to_string(), "absent": true })
        }
        Err(error) => json!({ "path": path.display().to_string(), "error": error.to_string() }),
    }
}

/// The live Wez descriptor in `runtime_dir`: `(json, epoch)`.
fn descriptor_json(runtime_dir: &Path) -> (Option<Value>, Option<uuid::Uuid>) {
    match runtime::read_wez_descriptor_in(runtime_dir) {
        Ok(None) => (None, None),
        Ok(Some(descriptor)) => {
            let epoch = uuid::Uuid::parse_str(&descriptor.epoch).ok();
            (
                Some(json!({
                    "state": descriptor.state,
                    "epoch": descriptor.epoch,
                    "pid": descriptor.pid,
                    "start_token": descriptor.start_token,
                    "socket": descriptor.socket,
                    "socket_dev": descriptor.socket_dev,
                    "socket_ino": descriptor.socket_ino,
                    "backend_instance_uid": descriptor.backend_instance_uid,
                    "written_at": descriptor.written_at,
                    "error": descriptor.error,
                })),
                epoch,
            )
        }
        Err(error) => (Some(json!({ "unreadable": error.to_string() })), None),
    }
}

/// The pinned inventory as `(json, live epoch)`: a complete scan's verified
/// epoch is the sentinel's (wez) or the server option's (tmux); an epoch
/// mismatch carries the observed epoch in its detail (`backend_epoch_changed:
/// expected X observed Y`).
fn inventory_json(outcome: &InventoryOutcome) -> (Value, Option<uuid::Uuid>) {
    let (token, detail): (&str, Option<String>) = match outcome {
        InventoryOutcome::Complete(_) => ("complete", None),
        InventoryOutcome::ServerStopped { detail } => ("server_stopped", Some(detail.clone())),
        InventoryOutcome::Unreachable { detail } => ("unreachable", Some(detail.clone())),
        InventoryOutcome::AuthFailed { detail } => ("auth_failed", Some(detail.clone())),
        InventoryOutcome::HostKeyIdentityFailed { detail } => {
            ("host_key_identity_failed", Some(detail.clone()))
        }
        InventoryOutcome::CommandMissing { detail } => ("command_missing", Some(detail.clone())),
        InventoryOutcome::VersionMismatch { detail } => ("version_mismatch", Some(detail.clone())),
        InventoryOutcome::ProtocolMismatch { detail } => {
            ("protocol_mismatch", Some(detail.clone()))
        }
        InventoryOutcome::Malformed { detail } => ("malformed", Some(detail.clone())),
        InventoryOutcome::Timeout { detail } => ("timeout", Some(detail.clone())),
        InventoryOutcome::PermissionFailure { detail } => {
            ("permission_failure", Some(detail.clone()))
        }
    };
    match outcome {
        InventoryOutcome::Complete(inventory) => (
            json!({
                "outcome": token,
                "server_epoch": inventory.server_epoch.map(|epoch| epoch.0.to_string()),
                "rows": inventory.rows.len(),
                "native_names": inventory.rows.iter().map(|row| row.native_name.clone()).collect::<Vec<_>>(),
            }),
            inventory.server_epoch.map(|epoch| epoch.0),
        ),
        _ => {
            let observed = inventory::epoch_changed_detail(outcome).and_then(|detail| {
                detail
                    .split_whitespace()
                    .skip_while(|word| *word != "observed")
                    .nth(1)
                    .and_then(|epoch| uuid::Uuid::parse_str(epoch).ok())
            });
            (
                json!({
                    "outcome": token,
                    "detail": detail,
                    "epoch_changed": inventory::epoch_changed_detail(outcome).is_some(),
                }),
                observed,
            )
        }
    }
}

/// The runtime directory this dmux resolves. `DMUX_RUNTIME_DIR` is an
/// owner-side test seam (ADR 009 §6) that every socket, descriptor, bridge
/// key and lock path is built from; exported in a production shell it
/// silently redirects all of them away from the service's directory, so it
/// is a finding whenever it is set.
fn runtime_dir_detail() -> (bool, String) {
    let resolved = match runtime::dmux_runtime_dir() {
        Ok(dir) => dir.display().to_string(),
        Err(error) => format!("unresolvable ({error})"),
    };
    match std::env::var_os(runtime::RUNTIME_DIR_SEAM_ENV) {
        Some(seam) if !seam.is_empty() => {
            let production = runtime::platform_runtime_dir()
                .map(|dir| dir.display().to_string())
                .unwrap_or_else(|error| format!("unresolvable ({error})"));
            (
                false,
                format!(
                    "{resolved} ({}={} is set: every socket, descriptor and lock is redirected \
                     away from the service's {production})",
                    runtime::RUNTIME_DIR_SEAM_ENV,
                    seam.to_string_lossy()
                ),
            )
        }
        _ => (true, resolved),
    }
}

fn wezterm_ok() -> bool {
    Command::new("wezterm")
        .args(["cli", "--no-auto-start", "list", "--format", "json"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn tmux_sessions() -> Option<usize> {
    let output = Command::new("tmux")
        .args(["list-sessions", "-F", "#{session_name}"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).lines().count())
}

fn ssh_ok(peer: &str) -> bool {
    Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=2",
            peer,
            "true",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn details_spell_out_each_probe_state() {
        assert_eq!(tmux_detail(None), "not running");
        assert_eq!(tmux_detail(Some(1)), "running (1 session)");
        assert_eq!(tmux_detail(Some(3)), "running (3 sessions)");
        assert_eq!(usb_detail(None), "down");
        assert_eq!(usb_detail(Some(Duration::from_millis(7))), "up (7 ms)");
        assert_eq!(yes_no(true), "yes");
        assert_eq!(reachable(false), "unreachable");
        assert!(peer_detail(Host::Archie).starts_with("archie (usb 10.77.77.2"));
    }

    /// The registry line names the directory and what its mode grants, and
    /// it is a finding only when another uid can actually get in. `/tmp`
    /// rather than `$TMPDIR`: on macOS the per-user temp dir is 0700, which
    /// would make every parent unreachable and the assertion vacuous.
    #[test]
    fn the_registry_line_names_the_directory_and_what_its_mode_grants() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::Builder::new()
            .prefix("dmux-doctor-")
            .tempdir_in("/tmp")
            .unwrap();
        let db = dir.path().join("registry.sqlite3");
        let chmod = |mode: u32| {
            std::fs::set_permissions(dir.path(), PermissionsExt::from_mode(mode)).unwrap()
        };

        chmod(0o700);
        let private = registry::parent_dir_exposure(&db).unwrap();
        assert!(private.is_private());
        assert_eq!(
            dir_detail(&private),
            format!("{} (0700, private)", dir.path().display())
        );

        chmod(0o755);
        let exposed = registry::parent_dir_exposure(&db).unwrap();
        assert!(
            !exposed.is_private(),
            "/tmp must be traversable for this assertion to mean anything"
        );
        assert_eq!(
            dir_detail(&exposed),
            format!(
                "{} (0755, any local user can enter and list)",
                dir.path().display()
            )
        );
        chmod(0o700);
    }

    fn layers(
        process: FlagLayer,
        manager_value: FlagLayer,
        file_value: FlagLayer,
    ) -> FlagProvenance {
        FlagProvenance {
            process,
            manager: "launchd",
            manager_value,
            file: "~/.config/dmux/service.env".to_string(),
            file_value,
        }
    }

    fn value(value: &str) -> FlagLayer {
        FlagLayer::Value(value.to_string())
    }

    /// Every state a canary host can be in has its own verdict, and only the
    /// two reboot-proof ones (nothing stated; file loaded into the manager)
    /// are green. No `launchctl`/`systemctl` runs here: the inputs are
    /// injected.
    #[test]
    fn the_flag_provenance_names_every_state_a_canary_can_be_in() {
        use FlagLayer::{Unavailable, Unset};

        let (ok, detail) = wez_first_detail(&layers(Unset, Unset, Unset));
        assert!(ok);
        assert_eq!(
            detail,
            "process=unset launchd=unset file=unset; no preference stated anywhere, the tracked default applies"
        );

        let (ok, detail) = wez_first_detail(&layers(value("1"), value("1"), value("1")));
        assert!(ok);
        assert_eq!(
            detail,
            "process=1 launchd=1 file=1; durable Wez-first: ~/.config/dmux/service.env is loaded into launchd"
        );

        let (ok, detail) = wez_first_detail(&layers(value("0"), value("0"), value("0")));
        assert!(ok);
        assert!(detail.contains("durable legacy"), "{detail}");

        // The Macie failure mode (ADR 012 §3.1): enabled by `launchctl setenv`
        // alone, gone after the reboot.
        let (ok, detail) = wez_first_detail(&layers(value("1"), value("1"), Unset));
        assert!(!ok);
        assert_eq!(
            detail,
            "process=1 launchd=1 file=unset; runtime-only Wez-first: launchd carries 1 but ~/.config/dmux/service.env does not, so a reboot clears it"
        );

        // The file is written but the loader has not run since.
        let (ok, detail) = wez_first_detail(&layers(Unset, Unset, value("1")));
        assert!(!ok);
        assert!(
            detail.contains("states 1 but launchd does not carry it")
                && detail.contains("launchctl kickstart gui/$UID/com.fredrir.dmux-env"),
            "{detail}"
        );

        let (ok, detail) = wez_first_detail(&layers(value("1"), Unset, Unset));
        assert!(!ok);
        assert!(
            detail.contains("this shell only: 1 is exported here"),
            "{detail}"
        );

        let (ok, detail) = wez_first_detail(&layers(Unset, value("1"), value("0")));
        assert!(!ok);
        assert!(detail.contains("layers disagree"), "{detail}");

        // Anything but 1 or 0 states no preference (ADR 010 §5).
        let (ok, detail) = wez_first_detail(&layers(value("yes"), Unset, Unset));
        assert!(ok);
        assert!(
            detail.starts_with("process=\"yes\"(no preference) launchd=unset"),
            "{detail}"
        );

        let malformed = Unavailable("line 2: value must match ^[A-Za-z0-9_./:@+,-]*$".to_string());
        let (ok, detail) = wez_first_detail(&layers(Unset, value("1"), malformed));
        assert!(!ok);
        assert!(
            detail.contains("file=unreadable")
                && detail.contains("is malformed and applies nothing: line 2: value must match"),
            "{detail}"
        );

        let unreadable = Unavailable("launchctl: not found".to_string());
        let (ok, detail) = wez_first_detail(&layers(Unset, unreadable, Unset));
        assert!(!ok);
        assert!(
            detail.contains("launchd could not be read: launchctl: not found"),
            "{detail}"
        );

        let mut archie = layers(Unset, Unset, value("1"));
        archie.manager = "systemd";
        archie.file = "~/.config/environment.d/50-dmux.conf".to_string();
        let (ok, detail) = wez_first_detail(&archie);
        assert!(!ok);
        assert!(
            detail.starts_with("process=unset systemd=unset file=1; ~/.config/environment.d/50-dmux.conf states 1 but systemd does not carry it: run `systemctl --user daemon-reload`"),
            "{detail}"
        );
    }

    /// The Rust reader accepts exactly what `dmux-service-env.sh` accepts
    /// and refuses exactly what it refuses, line for line, so the file layer
    /// doctor reports is the one the loader and the mux wrapper applied.
    #[test]
    fn the_service_env_grammar_matches_the_shell_parser() {
        let good = "# policy for this host\n\nDMUX_WEZ_FIRST=1\n   \n  # indented comment\n  DMUX_LEGACY_POLICY=0\nDMUX_WEZTERM_MUX_SERVER=/opt/homebrew/bin/wezterm-mux-server\nDMUX_=empty.key+is@allowed:by,the-grammar\nDMUX_WEZ_FIRST=0";
        let pair = |key: &str, value: &str| (key.to_string(), value.to_string());
        assert_eq!(
            parse_service_env(good).unwrap(),
            vec![
                pair("DMUX_WEZ_FIRST", "1"),
                pair("DMUX_LEGACY_POLICY", "0"),
                pair(
                    "DMUX_WEZTERM_MUX_SERVER",
                    "/opt/homebrew/bin/wezterm-mux-server"
                ),
                pair("DMUX_", "empty.key+is@allowed:by,the-grammar"),
                pair("DMUX_WEZ_FIRST", "0"),
            ]
        );
        assert_eq!(parse_service_env("").unwrap(), vec![]);
        assert_eq!(parse_service_env("# nothing\n\n").unwrap(), vec![]);

        let refused = [
            ("PATH=/tmp/evil", "key must match"),
            ("dmux_wez_first=1", "key must match"),
            ("DMUX_lower=1", "key must match"),
            ("DMUX_WEZ FIRST=1", "key must match"),
            ("DMUXWEZ=1", "key must match"),
            ("DMUX_WEZ_FIRST", "expected KEY=VALUE"),
            ("DMUX_WEZ_FIRST=$(touch pwned)", "value must match"),
            ("DMUX_WEZ_FIRST=`touch pwned`", "value must match"),
            ("DMUX_WEZ_FIRST=1;touch pwned", "value must match"),
            ("DMUX_WEZ_FIRST=1 # trailing comment", "value must match"),
            ("DMUX_WEZ_FIRST=\"1\"", "value must match"),
            ("DMUX_WEZ_FIRST='1'", "value must match"),
            ("DMUX_WEZ_FIRST=1 ", "value must match"),
            ("DMUX_WEZ_FIRST=a\\nb", "value must match"),
            ("DMUX_WEZ_FIRST=~/x", "value must match"),
            ("DMUX_WEZ_FIRST=${HOME}", "value must match"),
            ("DMUX_WEZ_FIRST=1|cat", "value must match"),
            ("DMUX_WEZ_FIRST=1&", "value must match"),
            ("DMUX_WEZ_FIRST=>out", "value must match"),
            ("DMUX_WEZ_FIRST=1\r", "value must match"),
            ("DMUX_WEZ_FIRST=é", "value must match"),
        ];
        for (bad, why) in refused {
            let text = format!("DMUX_WEZ_FIRST=1\n{bad}\n");
            let error = parse_service_env(&text).unwrap_err();
            assert!(
                error.starts_with(&format!("line 2: {why}")),
                "{bad:?} -> {error}"
            );
        }
        assert_eq!(
            parse_service_env("x\nDMUX_WEZ_FIRST=1\ny\n").unwrap_err(),
            "2 malformed lines, first at line 1: expected KEY=VALUE"
        );
    }

    /// Absent is "no preference", the last assignment wins, and a malformed
    /// file is unreadable rather than partially read — the same verdict the
    /// shell readers reach.
    #[test]
    fn the_file_layer_reads_absent_good_and_malformed_files() {
        let dir = tempfile::Builder::new()
            .prefix("dmux-doctor-env-")
            .tempdir()
            .unwrap();
        let path = dir.path().join("service.env");
        assert_eq!(file_layer(&path), FlagLayer::Unset);
        std::fs::write(&path, "DMUX_LEGACY_POLICY=1\n").unwrap();
        assert_eq!(file_layer(&path), FlagLayer::Unset);
        std::fs::write(&path, "DMUX_WEZ_FIRST=0\nDMUX_WEZ_FIRST=1\n").unwrap();
        assert_eq!(file_layer(&path), value("1"));
        std::fs::write(&path, "DMUX_WEZ_FIRST=1\nPATH=x\n").unwrap();
        assert_eq!(
            file_layer(&path),
            FlagLayer::Unavailable("line 2: key must match ^DMUX_[A-Z0-9_]*$".to_string())
        );
        assert_eq!(
            manager_block_layer("PATH=/usr/bin\nDMUX_WEZ_FIRST=1\nHOME=/home/x\n"),
            value("1")
        );
        assert_eq!(manager_block_layer("PATH=/usr/bin\n"), FlagLayer::Unset);
    }

    fn uid(n: u128) -> BackendInstanceUid {
        BackendInstanceUid(uuid::Uuid::from_u128(n))
    }

    fn published(epoch: u128) -> PublishedIncarnation {
        PublishedIncarnation {
            epoch: dmux::model::ServerEpoch(uuid::Uuid::from_u128(epoch)),
            pid: Some(4242),
            start_token: Some("macos:1:1".into()),
            socket_dev: Some(7),
            socket_ino: Some(9),
        }
    }

    fn managed(epoch: u128) -> ManagedTarget {
        ManagedTarget::Managed {
            instance: uid(1),
            scope: InventoryScope::managed(
                Backend::Wez,
                "/run/dmux/wez.sock",
                dmux::model::ServerEpoch(uuid::Uuid::from_u128(epoch)),
            ),
        }
    }

    fn lease(pid: i32) -> Lease {
        Lease {
            lease_id: 1,
            scope: "recovery:x".into(),
            holder_request_uid: uuid::Uuid::nil(),
            fencing_token: 3,
            holder_pid: Some(pid),
            holder_start_token: None,
            expires_at: "2026-08-23T00:00:00Z".into(),
            state: "held".into(),
        }
    }

    /// Every one of the six states has its own letter, name and remedy;
    /// C and D never say restart; F names the clear and the no-user-panes
    /// condition; the live side's epoch turns an otherwise-live E into F.
    /// All inputs are injected: no registry, lock, descriptor or server.
    #[test]
    fn the_classifier_names_every_instance_state_and_its_safe_remedy() {
        let free = HostWitness::default();

        let (state, detail, remedy) = classify(Backend::Wez, &ManagedTarget::Unregistered, &free);
        assert_eq!((state.letter(), state.name()), ("A", "not_registered"));
        assert!(
            detail.contains("no wez backend instance is registered"),
            "{detail}"
        );
        assert!(remedy.unwrap().contains("nothing is enrolled"));

        let (state, detail, _) =
            classify(Backend::Tmux, &ManagedTarget::Unaddressable(uid(1)), &free);
        assert_eq!((state.letter(), state.name()), ("B", "unaddressable"));
        assert!(detail.contains("without an endpoint"), "{detail}");

        let (state, detail, remedy) =
            classify(Backend::Wez, &ManagedTarget::Unpublished(uid(1)), &free);
        assert_eq!((state.letter(), state.name()), ("C", "unpublished_idle"));
        assert!(detail.contains("instance state C"), "{detail}");
        let remedy = remedy.unwrap();
        assert!(
            remedy.contains("wait for the managed mux to coordinate"),
            "{remedy}"
        );
        assert!(!remedy.to_lowercase().starts_with("restart"), "{remedy}");
        assert!(!detail.to_lowercase().contains("restart"), "{detail}");

        let held = HostWitness {
            exclusive_held: Some(true),
            ..HostWitness::default()
        };
        let (state, detail, remedy) =
            classify(Backend::Wez, &ManagedTarget::Unpublished(uid(1)), &held);
        assert_eq!(
            (state.letter(), state.name()),
            ("D", "coordinator_in_flight")
        );
        assert!(detail.contains("held exclusively"), "{detail}");
        let remedy = remedy.unwrap();
        assert!(remedy.contains("in flight") && remedy.contains("re-run `dmux doctor`"));
        assert!(!remedy.to_lowercase().contains("restart"), "{remedy}");

        let leased = HostWitness {
            exclusive_held: Some(false),
            recovery_lease: Some(lease(std::process::id() as i32)),
            ..HostWitness::default()
        };
        let (state, detail, _) =
            classify(Backend::Tmux, &ManagedTarget::Unpublished(uid(1)), &leased);
        assert_eq!(state, InstanceState::D);
        assert!(detail.contains("recovery lease is held by pid"), "{detail}");

        let (state, detail, remedy) = classify(Backend::Wez, &managed(0xe), &free);
        assert_eq!((state.letter(), state.name()), ("E", "published_live"));
        assert!(detail.contains("the host agrees"), "{detail}");
        assert_eq!(remedy, None);

        // The row passed the liveness probe, but the live server presents
        // another epoch (a descriptor or sentinel from another incarnation).
        let disagreeing = HostWitness {
            live_epoch: Some(uuid::Uuid::from_u128(0xf)),
            descriptor: Some(
                serde_json::json!({"state": "starting", "epoch": uuid::Uuid::from_u128(0xf).to_string(), "pid": 54528}),
            ),
            ..HostWitness::default()
        };
        let (state, detail, remedy) = classify(Backend::Wez, &managed(0xe), &disagreeing);
        assert_eq!((state.letter(), state.name()), ("F", "stale_incarnation"));
        assert!(detail.contains("live server presents epoch"), "{detail}");
        assert!(detail.contains("descriptor starting epoch"), "{detail}");
        let remedy = remedy.unwrap();
        assert!(
            remedy.contains("retire-incarnation --backend wez --epoch"),
            "{remedy}"
        );
        assert!(remedy.contains("holds no user panes"), "{remedy}");

        let stale = ManagedTarget::StaleIncarnation {
            instance: uid(1),
            published: published(0xe),
            observed: ObservedIncarnation::ProcessDead { pid: 4242 },
        };
        let (state, detail, remedy) = classify(Backend::Wez, &stale, &free);
        assert_eq!((state.letter(), state.name()), ("F", "stale_incarnation"));
        assert!(detail.contains("process 4242 is dead"), "{detail}");
        assert!(remedy.unwrap().contains("retire-incarnation --backend wez"));
    }

    /// The JSON row carries the letter, the name, both sides and the
    /// remedy, and a registry that could not be read is a null state with
    /// the reason rather than a guess.
    #[test]
    fn the_instance_row_serialises_both_sides() {
        let report = InstanceReport {
            backend: Backend::Tmux,
            instance: Some(uid(2)),
            state: Some(InstanceState::F),
            published: Some(published(0xe)),
            witness: HostWitness {
                liveness: Some(Liveness::Stale(ObservedIncarnation::ProcessDead {
                    pid: 4242,
                })),
                exclusive_held: Some(false),
                ..HostWitness::default()
            },
            detail: "d".into(),
            remedy: Some("r".into()),
        };
        let json = report.json();
        assert_eq!(json["state"], "F");
        assert_eq!(json["state_name"], "stale_incarnation");
        assert_eq!(json["backend"], "tmux");
        assert_eq!(json["published"]["pid"], 4242);
        assert_eq!(json["observed"]["process"], "dead");
        assert_eq!(json["observed"]["exclusive_lock_held"], false);
        assert_eq!(json["remedy"], "r");
        assert_eq!(report.human(), "F (stale_incarnation) d");

        let unavailable = InstanceReport::unavailable(Backend::Wez, "locked");
        let json = unavailable.json();
        assert!(json["state"].is_null());
        assert_eq!(json["detail"], "registry unreadable: locked");
    }

    /// The seam is a finding whenever it is set, and the line always names
    /// the directory this process would actually use.
    #[test]
    fn the_runtime_dir_line_reports_the_seam() {
        let (ok, detail) = runtime_dir_detail();
        match std::env::var_os(runtime::RUNTIME_DIR_SEAM_ENV) {
            Some(seam) if !seam.is_empty() => {
                assert!(!ok);
                assert!(detail.contains("DMUX_RUNTIME_DIR="), "{detail}");
                assert!(detail.contains("redirected"), "{detail}");
            }
            _ => {
                assert!(ok);
                assert!(!detail.contains("redirected"), "{detail}");
            }
        }
    }

    /// Whatever the environment, the probe answers without panicking and
    /// never creates or repairs anything.
    #[test]
    fn the_registry_probe_is_read_only_and_always_answers() {
        let db = registry::production_db_path();
        let before = db.as_ref().map(|path| path.exists());
        let (_ok, detail) = registry_detail();
        assert!(!detail.is_empty());
        assert_eq!(
            db.as_ref().map(|path| path.exists()),
            before,
            "the probe must not create the registry"
        );
        if let Some(db) = db {
            assert!(
                detail.contains(&db.parent().unwrap().display().to_string())
                    || detail.contains("unreadable"),
                "{detail}"
            );
        }
    }
}
