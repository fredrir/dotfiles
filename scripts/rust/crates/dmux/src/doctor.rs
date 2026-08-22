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

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::Duration;

use dmux::output::{self, OutputFormat};
use dmux::registry::{self, DirExposure};
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
    let report = Report {
        this,
        peer,
        wezterm: wezterm.join().unwrap_or(false),
        tmux: tmux.join().unwrap_or(None),
        usb: usb.join().unwrap_or(None),
        ssh: ssh.join().unwrap_or(false),
        wez_first: wez_first_detail(&wez_first_provenance()),
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
            crate::production_authority_revision(),
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
