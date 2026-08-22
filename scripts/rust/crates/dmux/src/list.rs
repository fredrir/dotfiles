//! The merged session listing every verb resolves against.
//!
//! Wezterm workspaces and tmux sessions come from two subprocess probes run
//! in parallel; either probe failing simply contributes nothing, since a
//! machine without a wezterm mux or a tmux server is a normal state, not an
//! error. Ordering is deterministic — wezterm rows by name, then tmux rows
//! by creation time — because the printed index is what `con`/`rm` resolve
//! numeric targets against.
//!
//! This is the legacy, registry-free path: it runs whenever `DMUX_WEZ_FIRST`
//! is unset and is what every rollback returns to (plan §21), so it is kept
//! for one release after the cutover and then deleted. Until then it makes
//! exactly two concessions to the managed service (ADR 001, plan §15.1):
//! when the service descriptor names a socket, the wezterm probe is pinned
//! to that socket and bounded by a dmux-side deadline; and the reserved
//! `dmux:system:<epoch>` sentinel workspace is never presented as a row,
//! whichever server answered.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use unicode_width::UnicodeWidthStr;
use workstation::Style;

use dmux::childio::{bounded_read, join_capture, kill_process_group};
use dmux::model::WEZ_SENTINEL_PREFIX;
use dmux::runtime;

use crate::PROGRAM;
use crate::hosts::{self, Context, Host};

const TMUX_FORMAT: &str =
    "#{session_name}|#{session_created}|#{session_windows}|#{session_attached}";

#[derive(Clone, Debug, Serialize)]
pub struct Row {
    pub index: usize,
    pub name: String,
    pub kind: Kind,
    /// Which machine the row came from, so `--json` output is
    /// self-describing; filled in by `gather`.
    pub host: &'static str,
    /// One live pane inside a wezterm workspace — the handle an attach from
    /// inside wezterm activates. Internal: tmux rows have none, and the
    /// `--json` shape stays as it is.
    #[serde(skip)]
    pub pane: Option<u64>,
    /// The exact socket this wezterm row was listed from, when the managed
    /// service descriptor named one. A pane id means something only on the
    /// server that issued it, so an attach must send its `activate-pane` to
    /// this same socket — carried on the row rather than re-resolved, so
    /// the two cannot disagree. Internal, like `pane`; tmux rows have none.
    #[serde(skip)]
    pub socket: Option<String>,
    pub created: Option<i64>,
    pub windows: usize,
    pub attached: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Wez,
    Tmux,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Wez => "wez",
            Kind::Tmux => "tmux",
        }
    }
}

pub fn run(
    context: &Context,
    only_tmux: bool,
    only_wez: bool,
    json: bool,
    names: bool,
) -> Result<ExitCode, String> {
    // Remote wezterm workspaces are not enumerable over this path — only the
    // peer's tmux server answers ssh — so an explicit `--wez` there would
    // always print nothing. Humans get an error; --json/--names are for
    // scripts, which get a well-formed empty result plus the note on stderr.
    if !context.local && only_wez && !only_tmux {
        let message = format!(
            "wezterm workspaces on {} are not listable over ssh; only tmux sessions are (--tmux)",
            context.host.name()
        );
        if json || names {
            eprintln!("{PROGRAM}: {message}");
            if json {
                println!("[]");
            }
            return Ok(ExitCode::SUCCESS);
        }
        return Err(message);
    }
    // Indices are assigned over the full merged set before any filter, so the
    // number a filtered listing prints is the same one con/rm resolve — gaps
    // in the output are the point, not a bug.
    let mut rows = gather(context, true, true)?;
    let include_wez = only_wez || !only_tmux;
    let include_tmux = only_tmux || !only_wez;
    rows.retain(|row| match row.kind {
        Kind::Wez => include_wez,
        Kind::Tmux => include_tmux,
    });
    if names {
        for row in &rows {
            println!("{}", row.name);
        }
        return Ok(ExitCode::SUCCESS);
    }
    if json {
        let text = serde_json::to_string(&rows).map_err(|error| error.to_string())?;
        println!("{text}");
        return Ok(ExitCode::SUCCESS);
    }
    for line in render(&rows, &Style::for_stdout()) {
        println!("{line}");
    }
    Ok(ExitCode::SUCCESS)
}

pub fn gather(
    context: &Context,
    include_wez: bool,
    include_tmux: bool,
) -> Result<Vec<Row>, String> {
    let (wez, tmux) = if context.local {
        let wez_probe = include_wez.then(|| thread::spawn(wez_rows));
        let tmux = if include_tmux {
            local_tmux_rows()
        } else {
            Vec::new()
        };
        let wez = match wez_probe {
            Some(handle) => handle.join().unwrap_or_default(),
            None => Vec::new(),
        };
        (wez, tmux)
    } else {
        let tmux = if include_tmux {
            remote_tmux_rows(context.host)?
        } else {
            Vec::new()
        };
        (Vec::new(), tmux)
    };
    let mut rows = wez;
    rows.extend(tmux);
    for (position, row) in rows.iter_mut().enumerate() {
        row.index = position + 1;
        row.host = context.host.name();
    }
    Ok(rows)
}

/// The sole endpoint selector the stock CLI honours (ADR 001).
pub const WEZ_SOCKET_ENV: &str = "WEZTERM_UNIX_SOCKET";

/// Per-child deadline for the wezterm probe, the same bound the managed
/// provider puts on every `wezterm cli` child (`backend::wez`). ADR 001: a
/// live-but-silent socket hangs the stock CLI indefinitely, so the timeout
/// is manufactured by dmux killing the child, never by wezterm.
const WEZ_LIST_DEADLINE: Duration = Duration::from_secs(10);

/// Grace for the stdout reader once the child is gone, shared with the
/// crate's other bounded probes (`remote::wez_compat`).
const WEZ_LIST_DRAIN_GRACE: Duration = Duration::from_secs(2);

/// Cap on the listing the child may hand back. Past this the answer is not a
/// listing, and `childio::bounded_read` keeps the capture from becoming an
/// out-of-memory abort while still draining the pipe to EOF.
const WEZ_LIST_MAX_OUTPUT: usize = 4 * 1024 * 1024;

/// The sentinel is the service's own reserved workspace (plan §15.1,
/// ADR 002): never a Space, never a target, and "cannot be addressed by
/// public commands" — so it must not appear as a row or be attachable.
pub fn is_sentinel_workspace(name: &str) -> bool {
    name.starts_with(WEZ_SENTINEL_PREFIX)
}

/// The exact socket of the managed `wezterm-mux-server`, when the service
/// descriptor (plan §15.1) names one; `None` when there is no descriptor.
///
/// With a descriptor present `WEZTERM_UNIX_SOCKET` is pinned to its socket,
/// so neither an ambient GUI socket nor wezterm's own discovery can answer
/// on the service's behalf (ADR 001: the env variable is the sole endpoint
/// selector). The descriptor's `state` is deliberately not required to be
/// `ready`: a `starting` or `failed` descriptor still names the only socket
/// this path may talk to, and pinning to a down server yields an honest
/// empty listing where discovery could have listed some other server.
/// Without a descriptor nothing is pinned and today's discovery stands — the
/// legacy path gains no new fallback and no registry dependency (the
/// descriptor is a service file, not the registry). A descriptor that exists
/// but cannot be read (ownership or mode, a rewrite in progress, malformed
/// JSON) counts as absent for the same reason; the sentinel filter protects
/// the listing either way.
///
/// The runtime directory comes from `runtime::dmux_runtime_dir()`, which
/// honours the owner-side `DMUX_RUNTIME_DIR` seam (ADR 012 WS-E.1); that is
/// what lets a test point this process at a scratch descriptor instead of
/// the live runtime directory.
pub fn managed_wez_socket() -> Option<String> {
    // The resolver honours the `DMUX_RUNTIME_DIR` seam itself (ADR 012
    // WS-E.1), so tests and the legacy path read the same descriptor path.
    let runtime_dir = runtime::dmux_runtime_dir().ok()?;
    let descriptor = runtime::read_wez_descriptor_in(&runtime_dir).ok()??;
    // An empty value would fall through to wezterm's socket discovery
    // (ADR 006) — that is a malformed descriptor, not a pin.
    (!descriptor.socket.is_empty()).then_some(descriptor.socket)
}

/// One pane as `wezterm cli list --format json` reports it; the fields the
/// listing consumes, the rest ignored.
#[derive(Deserialize)]
struct WezPane {
    window_id: u64,
    pane_id: u64,
    workspace: String,
}

/// A workspace is the set of windows whose panes carry its name; the one
/// holding `WEZTERM_PANE` is the workspace this process is sitting in.
/// `--no-auto-start` because a listing is a question, not a request to
/// daemonize a wezterm-mux-server on a machine that runs none.
fn wez_rows() -> Vec<Row> {
    let socket = managed_wez_socket();
    let Some(stdout) = run_wezterm_list(socket.as_deref()) else {
        return Vec::new();
    };
    let Ok(panes) = serde_json::from_slice::<Vec<WezPane>>(&stdout) else {
        return Vec::new();
    };
    // Same staleness rule as `Context::resolve`: a WEZTERM_PANE frozen into
    // a tmux environment must not mark a workspace attached.
    let trusted = hosts::trust_wezterm_env(
        std::env::var_os("TMUX").is_some(),
        std::env::var("TERM_PROGRAM").ok().as_deref(),
    );
    let current: Option<u64> = if trusted {
        std::env::var("WEZTERM_PANE")
            .ok()
            .and_then(|value| value.parse().ok())
    } else {
        None
    };
    wez_rows_from(panes, current, socket)
}

/// `wezterm cli --no-auto-start list --format json` on the pinned socket
/// when there is one, bounded the way every other wezterm child in the crate
/// is (`childio`): its own process group, a capped stdout reader, and a
/// dmux-side deadline after which the group is killed. `None` for anything
/// but a clean, complete answer — a missing binary, a nonzero exit, a
/// timeout, or a listing past the cap all contribute no rows, exactly as a
/// failed probe always has.
fn run_wezterm_list(socket: Option<&str>) -> Option<Vec<u8>> {
    let mut command = Command::new("wezterm");
    command.args(["cli", "--no-auto-start", "list", "--format", "json"]);
    if let Some(socket) = socket {
        command.env(WEZ_SOCKET_ENV, socket);
    }
    let mut child = command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let reader = thread::spawn(move || bounded_read(stdout, WEZ_LIST_MAX_OUTPUT));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // Closes the pipe write end any descendant inherited, so the
                // reader below reaches EOF instead of outliving the deadline.
                kill_process_group(child.id());
                break status;
            }
            Ok(None) if started.elapsed() >= WEZ_LIST_DEADLINE => {
                abandon_wezterm_list(&mut child, reader);
                return None;
            }
            Ok(None) => thread::sleep(Duration::from_millis(5)),
            Err(_) => {
                abandon_wezterm_list(&mut child, reader);
                return None;
            }
        }
    };
    let capture = join_capture(reader, Instant::now() + WEZ_LIST_DRAIN_GRACE)?;
    (status.success() && !capture.truncated).then_some(capture.bytes)
}

/// The deadline and wait-error exit: kill the whole group first, so the
/// reader's pipe is closed before it is waited on, then bound that wait too.
fn abandon_wezterm_list(
    child: &mut std::process::Child,
    reader: thread::JoinHandle<dmux::childio::BoundedCapture>,
) {
    kill_process_group(child.id());
    let _ = child.kill();
    let _ = child.wait();
    let _ = join_capture(reader, Instant::now() + WEZ_LIST_DRAIN_GRACE);
}

/// Rows from a listing, the sentinel left out. Pure, so the filter is
/// provable without a wezterm on PATH.
fn wez_rows_from(panes: Vec<WezPane>, current: Option<u64>, socket: Option<String>) -> Vec<Row> {
    // Windows, attachment, and the lowest pane id — a stable handle for
    // `attach` to activate the workspace with from inside wezterm.
    let mut workspaces: BTreeMap<String, (BTreeSet<u64>, bool, Option<u64>)> = BTreeMap::new();
    for pane in panes {
        // Plan §15.1: the reserved sentinel is excluded from user inventory
        // and is never a target — whichever server answered this listing.
        if is_sentinel_workspace(&pane.workspace) {
            continue;
        }
        let entry = workspaces.entry(pane.workspace).or_default();
        entry.0.insert(pane.window_id);
        entry.1 |= current == Some(pane.pane_id);
        entry.2 = Some(
            entry
                .2
                .map_or(pane.pane_id, |lowest| lowest.min(pane.pane_id)),
        );
    }
    workspaces
        .into_iter()
        .map(|(name, (windows, attached, pane))| Row {
            index: 0,
            name,
            kind: Kind::Wez,
            host: "",
            pane,
            socket: socket.clone(),
            created: None,
            windows: windows.len(),
            attached,
        })
        .collect()
}

fn local_tmux_rows() -> Vec<Row> {
    let Ok(output) = Command::new("tmux")
        .args(["list-sessions", "-F", TMUX_FORMAT])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    tmux_rows_from(&String::from_utf8_lossy(&output.stdout))
}

/// tmux's own failures ("no server running") pass through ssh as an ordinary
/// nonzero exit and mean an empty list; 255 is ssh itself failing, which is
/// worth an error because the host was asked for by name — and worth ssh's
/// own first line of explanation, not just "cannot reach". Exit 127 (or a
/// "command not found" on stderr) is the remote shell failing to find tmux
/// at all — a PATH problem, not an empty server — and silently answering
/// `[]` to that would hide a broken transport, so it errs loudly instead.
fn remote_tmux_rows(host: Host) -> Result<Vec<Row>, String> {
    let command = format!(
        "{}tmux list-sessions -F '{TMUX_FORMAT}'",
        hosts::REMOTE_PATH_PREFIX
    );
    let output = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", hosts::SSH_CONNECT_TIMEOUT])
        .args([host.name(), &command])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("ssh: {error}"))?;
    if output.status.code() == Some(255) {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr.lines().map(str::trim).find(|line| !line.is_empty());
        return Err(match reason {
            Some(reason) => format!("cannot reach {}: {reason}", host.name()),
            None => format!("cannot reach {}", host.name()),
        });
    }
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.status.code() == Some(127) || stderr.contains("command not found") {
            return Err(format!(
                "tmux not found on {} (non-interactive ssh PATH)",
                host.name()
            ));
        }
        return Ok(Vec::new());
    }
    Ok(tmux_rows_from(&String::from_utf8_lossy(&output.stdout)))
}

fn tmux_rows_from(text: &str) -> Vec<Row> {
    let mut rows: Vec<Row> = text.lines().filter_map(parse_tmux_line).collect();
    rows.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.name.cmp(&b.name)));
    rows
}

/// Split from the right: the three numeric fields cannot contain `|`, while
/// a session created by another tool can be named anything at all.
fn parse_tmux_line(line: &str) -> Option<Row> {
    let mut fields = line.rsplitn(4, '|');
    let attached = fields.next()?.parse::<u64>().ok()?;
    let windows = fields.next()?.parse::<usize>().ok()?;
    let created = fields.next()?.parse::<i64>().ok()?;
    let name = fields.next()?;
    (!name.is_empty()).then(|| Row {
        index: 0,
        name: name.to_string(),
        kind: Kind::Tmux,
        host: "",
        pane: None,
        socket: None,
        created: Some(created),
        windows,
        attached: attached > 0,
    })
}

pub fn render(rows: &[Row], style: &Style) -> Vec<String> {
    let index_width = rows.last().map_or(1, |row| row.index.to_string().len());
    let name_width = rows.iter().map(|row| row.name.width()).max().unwrap_or(0);
    let windows_width = rows
        .iter()
        .map(|row| row.windows.to_string().len())
        .max()
        .unwrap_or(1);
    rows.iter()
        .map(|row| {
            let created = row.created.map_or_else(|| "-".to_string(), format_created);
            let mut line = format!(
                "{}  {}  {}  {}  {}",
                style.dim(&format!("{:>index_width$}", row.index)),
                style.teal(&pad_display(&row.name, name_width)),
                style.dim(&format!("{created:<11}")),
                style.dim(&format!("{:<4}", row.kind.label())),
                style.dim(&format!("{:>windows_width$}", row.windows)),
            );
            if row.attached {
                line.push_str("  ");
                line.push_str(&style.green("attached"));
            }
            line
        })
        .collect()
}

/// Pad to a display width, not a char count: CJK and emoji names occupy two
/// columns per glyph, and `format!`'s `{:<width$}` counts neither.
fn pad_display(name: &str, width: usize) -> String {
    let padding = width.saturating_sub(name.width());
    format!("{name}{:padding$}", "")
}

/// `HH:MM DD.MM` in local time, straight from `localtime_r` — the promised
/// zero-dependency alternative to pulling in a date crate for one format.
pub fn format_created(epoch: i64) -> String {
    let time = epoch as libc::time_t;
    // SAFETY: localtime_r fills the buffer it is handed and signals failure
    // by returning null, in which case the buffer is not read.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&time, &mut tm) }.is_null() {
        return "-".to_string();
    }
    format!(
        "{:02}:{:02} {:02}.{:02}",
        tm.tm_hour,
        tm.tm_min,
        tm.tm_mday,
        tm.tm_mon + 1
    )
}

/// An exact session name wins over an index: numeric names are legal, and a
/// user typing one almost certainly means the session they can see, not the
/// row that happens to sit at that position.
pub fn resolve<'a>(rows: &'a [Row], target: &str) -> Result<&'a Row, String> {
    let named = rows
        .iter()
        .find(|row| row.kind == Kind::Tmux && row.name == target)
        .or_else(|| rows.iter().find(|row| row.name == target));
    if let Some(row) = named {
        return Ok(row);
    }
    if !target.is_empty() && target.bytes().all(|byte| byte.is_ascii_digit()) {
        return target
            .parse::<usize>()
            .ok()
            .and_then(|index| index.checked_sub(1))
            .and_then(|index| rows.get(index))
            .ok_or_else(|| missing(target));
    }
    require_valid(target)?;
    Err(missing(target))
}

fn missing(target: &str) -> String {
    format!("no session '{target}' (dmux new {target} to create it)")
}

pub fn require_valid(name: &str) -> Result<(), String> {
    if valid_name(name) {
        Ok(())
    } else {
        Err("session names may contain letters, numbers, _ and -".to_string())
    }
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Session names for dynamic completion: a short hard deadline instead of a
/// plain `output()`, because a wedged tmux socket must not wedge the shell.
pub fn completion_names() -> Vec<String> {
    let Ok(mut child) = Command::new("tmux")
        .args(["list-sessions", "-F", "#{session_name}"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Vec::new();
    };
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) | Err(_) => return Vec::new(),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Vec::new();
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
        }
    }
    let mut text = String::new();
    let Some(mut stdout) = child.stdout.take() else {
        return Vec::new();
    };
    if stdout.read_to_string(&mut text).is_err() {
        return Vec::new();
    }
    text.lines().map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, kind: Kind, created: Option<i64>) -> Row {
        Row {
            index: 0,
            name: name.to_string(),
            kind,
            host: "",
            pane: None,
            socket: None,
            created,
            windows: 1,
            attached: false,
        }
    }

    fn pane(window_id: u64, pane_id: u64, workspace: &str) -> WezPane {
        WezPane {
            window_id,
            pane_id,
            workspace: workspace.to_string(),
        }
    }

    /// The sentinel prefix is the frozen `dmux:system:` constant, matched
    /// as a prefix: every epoch's sentinel, and nothing merely similar.
    #[test]
    fn the_sentinel_is_recognised_by_its_frozen_prefix() {
        assert!(is_sentinel_workspace(
            "dmux:system:895ca35a-78ac-4ff7-ae9c-222a9aee3a81"
        ));
        assert!(is_sentinel_workspace("dmux:system:"));
        assert!(!is_sentinel_workspace("dmux:ws:work"));
        assert!(!is_sentinel_workspace("dmux:systemic"));
        assert!(!is_sentinel_workspace("work"));
    }

    /// Plan §15.1: a sentinel-only server lists as empty, and a sentinel
    /// beside real workspaces vanishes without disturbing them — their
    /// window counts, lowest-pane handles and the socket pin all stand.
    #[test]
    fn sentinel_workspaces_never_become_rows() {
        let sentinel = "dmux:system:895ca35a-78ac-4ff7-ae9c-222a9aee3a81";
        let only = vec![pane(1, 1, sentinel), pane(1, 2, sentinel)];
        assert!(wez_rows_from(only, Some(1), None).is_empty());

        let mixed = vec![
            pane(1, 1, sentinel),
            pane(2, 7, "work"),
            pane(2, 3, "work"),
            pane(3, 9, "other"),
        ];
        let rows = wez_rows_from(mixed, Some(3), Some("/tmp/wez-dmux.sock".to_string()));
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, ["other", "work"]);
        let work = &rows[1];
        assert_eq!(work.pane, Some(3));
        assert_eq!(work.windows, 1);
        assert!(work.attached);
        assert_eq!(work.socket.as_deref(), Some("/tmp/wez-dmux.sock"));
        assert_eq!(rows[0].socket.as_deref(), Some("/tmp/wez-dmux.sock"));
    }

    fn indexed(mut rows: Vec<Row>) -> Vec<Row> {
        for (position, row) in rows.iter_mut().enumerate() {
            row.index = position + 1;
        }
        rows
    }

    #[test]
    fn parses_the_tmux_format_line() {
        let row = parse_tmux_line("main|1700000000|3|1").unwrap();
        assert_eq!(row.name, "main");
        assert_eq!(row.created, Some(1_700_000_000));
        assert_eq!(row.windows, 3);
        assert!(row.attached);
    }

    #[test]
    fn a_name_may_contain_the_delimiter() {
        let row = parse_tmux_line("odd|name|1700000000|2|0").unwrap();
        assert_eq!(row.name, "odd|name");
        assert_eq!(row.windows, 2);
        assert!(!row.attached);
    }

    #[test]
    fn garbage_lines_are_dropped() {
        assert!(parse_tmux_line("").is_none());
        assert!(parse_tmux_line("only|two|fields").is_none());
        assert!(parse_tmux_line("|1700000000|1|0").is_none());
    }

    #[test]
    fn tmux_rows_sort_by_creation_time() {
        let rows = tmux_rows_from("young|200|1|0\nold|100|1|0\n");
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, ["old", "young"]);
    }

    #[test]
    fn numeric_targets_resolve_by_index() {
        let rows = indexed(vec![
            row("work", Kind::Wez, None),
            row("main", Kind::Tmux, Some(100)),
        ]);
        assert_eq!(resolve(&rows, "2").unwrap().name, "main");
        assert!(resolve(&rows, "0").is_err());
        assert!(resolve(&rows, "3").is_err());
    }

    #[test]
    fn a_numeric_name_shadows_the_index() {
        let rows = indexed(vec![
            row("work", Kind::Tmux, Some(100)),
            row("3", Kind::Tmux, Some(200)),
        ]);
        assert_eq!(resolve(&rows, "3").unwrap().name, "3");
        assert_eq!(resolve(&rows, "1").unwrap().name, "work");
    }

    #[test]
    fn named_targets_prefer_the_tmux_row() {
        let rows = indexed(vec![
            row("main", Kind::Wez, None),
            row("main", Kind::Tmux, Some(100)),
        ]);
        assert_eq!(resolve(&rows, "main").unwrap().kind, Kind::Tmux);
    }

    #[test]
    fn a_missing_target_suggests_creating_it() {
        let error = resolve(&[], "scratch").unwrap_err();
        assert_eq!(
            error,
            "no session 'scratch' (dmux new scratch to create it)"
        );
    }

    #[test]
    fn names_are_letters_numbers_underscore_dash() {
        assert!(valid_name("main-2_x"));
        assert!(!valid_name(""));
        assert!(!valid_name("has space"));
        assert!(!valid_name("semi;colon"));
        assert!(!valid_name("=main"));
    }

    #[test]
    fn rendering_aligns_on_text_not_escapes() {
        let rows = indexed(vec![
            row("a", Kind::Wez, None),
            row("longer-name", Kind::Tmux, Some(0)),
        ]);
        let lines = render(&rows, &Style::plain());
        assert_eq!(lines[0].find("wez"), lines[1].find("tmux"));
        assert!(lines[0].contains("a          "));
        assert!(lines[0].contains('-'));
    }

    /// Byte offsets differ — multibyte names are longer in bytes — so the
    /// aligned columns are equal in display width, the unit terminals use.
    #[test]
    fn alignment_uses_display_width_not_char_count() {
        let rows = indexed(vec![
            row("日本語", Kind::Tmux, Some(0)), // 3 chars, width 6
            row("abcdef", Kind::Tmux, Some(0)), // 6 chars, width 6
        ]);
        let lines = render(&rows, &Style::plain());
        let width_before_kind = |line: &str| line[..line.find("tmux").unwrap()].width();
        assert_eq!(width_before_kind(&lines[0]), width_before_kind(&lines[1]));
    }

    // The libc crate does not bind tzset on every platform, but POSIX does.
    unsafe extern "C" {
        fn tzset();
    }

    #[test]
    fn formats_created_at_a_pinned_zone() {
        // SAFETY: the only test in this binary that touches the environment,
        // and tzset is what makes libc reread TZ.
        unsafe {
            std::env::set_var("TZ", "UTC0");
            tzset();
        }
        assert_eq!(format_created(0), "00:00 01.01");
        assert_eq!(format_created(1_700_000_100), "22:15 14.11");
    }
}
