//! The merged session listing every verb resolves against.
//!
//! Wezterm workspaces and tmux sessions come from two subprocess probes run
//! in parallel; either probe failing simply contributes nothing, since a
//! machine without a wezterm mux or a tmux server is a normal state, not an
//! error. Ordering is deterministic — wezterm rows by name, then tmux rows
//! by creation time — because the printed index is what `con`/`rm` resolve
//! numeric targets against.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use unicode_width::UnicodeWidthStr;
use workstation::Style;

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

/// A workspace is the set of windows whose panes carry its name; the one
/// holding `WEZTERM_PANE` is the workspace this process is sitting in.
/// `--no-auto-start` because a listing is a question, not a request to
/// daemonize a wezterm-mux-server on a machine that runs none.
fn wez_rows() -> Vec<Row> {
    let Ok(output) = Command::new("wezterm")
        .args(["cli", "--no-auto-start", "list", "--format", "json"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    #[derive(Deserialize)]
    struct Pane {
        window_id: u64,
        pane_id: u64,
        workspace: String,
    }
    let Ok(panes) = serde_json::from_slice::<Vec<Pane>>(&output.stdout) else {
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
    // Windows, attachment, and the lowest pane id — a stable handle for
    // `attach` to activate the workspace with from inside wezterm.
    let mut workspaces: BTreeMap<String, (BTreeSet<u64>, bool, Option<u64>)> = BTreeMap::new();
    for pane in panes {
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
            created,
            windows: 1,
            attached: false,
        }
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
