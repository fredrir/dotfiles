use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::os::fd::AsFd;
use std::path::Path;
use std::process::Command;

use nix::poll::{PollFd, PollFlags, poll};
use serde::{Deserialize, Serialize};

use crate::{
    Result,
    config::{self, clean},
    process,
    tmux::Context,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Choice {
    pub label: String,
    pub kind: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub table: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub copy: String,
}

impl Choice {
    pub fn new(label: impl Into<String>, kind: &str, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            kind: kind.into(),
            value: value.into(),
            table: String::new(),
            copy: String::new(),
        }
    }
}

#[derive(Deserialize, Serialize)]
struct Picker {
    rows: Vec<Choice>,
    title: String,
    header: String,
    colors: String,
    #[serde(default)]
    popup: bool,
    #[serde(default)]
    copy: String,
}

fn clipboard_command(ctx: &Context) -> String {
    let mut args = vec![ctx.tmux.binary.to_string_lossy().into_owned()];
    if let Some(socket) = &ctx.tmux.socket {
        args.extend(["-S".into(), socket.clone()]);
    }
    args.extend(["set-buffer".into(), "-w".into()]);
    if let Some(client) = &ctx.client {
        args.extend(["-t".into(), client.clone()]);
    }
    args.push("--".into());
    process::shell(&args)
}

pub fn choose(
    ctx: &mut Context,
    rows: Vec<Choice>,
    title: &str,
    header: &str,
) -> Result<Option<Choice>> {
    if rows.is_empty() {
        ctx.notice(&format!("{title}: empty"));
        return Ok(None);
    }
    let directory = tempfile::Builder::new()
        .prefix("tmux-workspace-")
        .tempdir()?;
    let data = directory.path().join("choices.json");
    let output = directory.path().join("choice.json");
    let popup = ctx.pane.is_some() || !io::stdin().is_terminal();
    if popup {
        ctx.resolve()?;
    }
    let picker = Picker {
        rows,
        title: title.into(),
        header: header.into(),
        colors: ctx.tmux.option("@theme_fzf_colors"),
        popup,
        copy: clipboard_command(ctx),
    };
    config::atomic_json(&data, &picker)?;
    let command = ctx.self_command(
        "_pick",
        &[
            "--data",
            data.to_str().ok_or("invalid picker path")?,
            "--result",
            output.to_str().ok_or("invalid picker path")?,
        ],
    )?;
    if popup {
        ctx.popup(&command, title, &ctx.cwd(), true)?;
    } else {
        process::interactive(Command::new(&command[0]).args(&command[1..]))?;
    }
    if !output.is_file() {
        return Ok(None);
    }
    let index: usize = serde_json::from_str(&fs::read_to_string(output)?)?;
    Ok(picker.rows.get(index).cloned())
}

pub fn pick(data: &Path, output: &Path) -> Result<()> {
    let picker: Picker = serde_json::from_str(&fs::read_to_string(data)?)?;
    let index = if process::which("fzf").is_some() {
        let lines = picker
            .rows
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let copy = if row.copy.is_empty() {
                    &row.label
                } else {
                    &row.copy
                };
                format!("{i}\t{}\t{}", clean(copy, 1200), clean(&row.label, 1200))
            })
            .collect::<Vec<_>>()
            .join("\n");
        let mut cmd = Command::new("fzf");
        cmd.env("FZF_DEFAULT_OPTS", "")
            .env("FZF_DEFAULT_OPTS_FILE", "")
            .args([
                "--layout=reverse",
                "--no-multi",
                "--delimiter=\t",
                "--with-nth=3..",
                "--cycle",
                "--header",
                &format!("{} · ⌃y copy row", picker.header),
                "--bind",
                &format!("ctrl-y:execute-silent({} {{2}})", picker.copy),
            ]);
        if picker.popup {
            cmd.args(["--padding=0,1", "--prompt", "› "]);
        } else {
            cmd.args([
                "--border=rounded",
                "--prompt",
                &format!("{} › ", picker.title),
            ]);
        }
        if !picker.colors.is_empty() {
            cmd.args(["--color", &picker.colors]);
        }
        let result = process::capture_foreground(&mut cmd, Some(lines.as_bytes()))?;
        match result.code {
            0 => Some(
                result
                    .out
                    .split('\t')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .parse::<usize>()?,
            ),
            1 | 130 => None,
            _ => {
                result.checked()?;
                None
            }
        }
    } else {
        println!(
            "{} — fzf unavailable; enter a number or search text\n",
            picker.title
        );
        let mut search = String::new();
        loop {
            for (i, row) in picker
                .rows
                .iter()
                .enumerate()
                .filter(|(_, r)| r.label.to_lowercase().contains(&search))
                .take(80)
            {
                println!("{:>3}  {}", i + 1, clean(&row.label, 180));
            }
            print!("Number / search / empty to cancel › ");
            io::stdout().flush()?;
            let mut line = String::new();
            io::stdin().read_line(&mut line)?;
            let value = line.trim();
            if value.is_empty() {
                break None;
            }
            if let Ok(index) = value.parse::<usize>()
                && index > 0
                && index <= picker.rows.len()
            {
                break Some(index - 1);
            }
            search = value.to_lowercase();
        }
    };
    if let Some(index) = index {
        if index >= picker.rows.len() {
            return Err("invalid picker result".into());
        }
        config::atomic_json(output, &index)?;
    }
    Ok(())
}

pub fn bindings(ctx: &Context) -> Result<Vec<Choice>> {
    let mut rows = Vec::new();
    for table in ["prefix", "workspace-resize", "copy-mode-vi"] {
        let result =
            ctx.tmux
                .try_run(&["list-keys", "-F", "#{key_string}\t#{key_note}", "-T", table])?;
        for line in result.out.lines() {
            if let Some((key, note)) = line.split_once('\t')
                && !note.trim().is_empty()
            {
                let mut row = Choice::new(
                    format!(
                        "{} {key:<9} {note}",
                        if table == "prefix" { "P" } else { table }
                    ),
                    "binding",
                    key,
                );
                row.table = table.into();
                rows.push(row);
            }
        }
    }
    for (value, label) in [
        ("agent-codex", "agent       Start managed Codex"),
        ("agent-claude", "agent       Start managed Claude"),
        ("handoff-status", "agent       Handoff status"),
        (
            "agent-follow",
            "agent       Follow execution to destination",
        ),
        ("handoff-cancel", "agent       Cancel queued move"),
        ("handoff-recover", "agent       Recover failed handoff"),
        ("inspect-keys", "keys        Read actual input bytes"),
        ("favorite", "favorites   Favorite this project"),
    ] {
        rows.push(Choice::new(label, "action", value));
    }
    for host in ctx.paths.hosts()? {
        rows.push(Choice::new(
            format!("host        Connect {host}"),
            "host",
            host,
        ));
    }
    Ok(rows)
}

pub fn palette(ctx: &mut Context) -> Result<i32> {
    ctx.resolve()?;
    let rows = bindings(ctx)?;
    let Some(row) = choose(
        ctx,
        rows,
        "Actions",
        "Bindings come from the running server · P = prefix",
    )?
    else {
        return Ok(0);
    };
    match row.kind.as_str() {
        "binding" => {
            if row.table == "copy-mode-vi" {
                ctx.tmux.run(&["copy-mode", "-t", ctx.pane()?])?;
            } else {
                ctx.tmux
                    .run(&["switch-client", "-c", ctx.client()?, "-T", &row.table])?;
            }
            ctx.tmux
                .run(&["send-keys", "-K", "-c", ctx.client()?, &row.value])?;
        }
        "host" => return crate::integrations::host(ctx, Some(&row.value), None, false),
        _ => return crate::cli::action(ctx, &row.value),
    }
    Ok(0)
}

pub fn copy(ctx: &Context, value: &str) -> Result<()> {
    let mut args = vec!["set-buffer", "-w"];
    if let Some(client) = &ctx.client {
        args.extend(["-t", client]);
    }
    args.extend(["--", value]);
    if ctx.tmux.try_run(&args)?.code != 0 {
        ctx.tmux.input(&["load-buffer", "-"], value.as_bytes())?;
    }
    ctx.notice("Copied");
    Ok(())
}

pub fn quick_select(ctx: &mut Context) -> Result<()> {
    ctx.resolve()?;
    match crate::plugins::fingers(ctx)? {
        0 | 130 => return Ok(()),
        3 => {}
        code => return Err(format!("fingers exited {code}").into()),
    }
    let text = ctx.tmux.run(&["capture-pane", "-pJ", "-t", ctx.pane()?])?;
    let pattern =
        regex::Regex::new(r#"https?://[^\s<>"']+|(?:~|\.{1,2})?/[^\s<>"']+|\b[0-9a-f]{7,40}\b"#)?;
    let mut seen = std::collections::HashSet::new();
    let rows = pattern
        .find_iter(&text)
        .filter(|m| seen.insert(m.as_str()))
        .map(|m| Choice::new(m.as_str(), "text", m.as_str()))
        .collect();
    if let Some(row) = choose(
        ctx,
        rows,
        "Quick select",
        "Paths · URLs · hashes · Enter copies",
    )? {
        copy(ctx, &row.value)?;
    }
    Ok(())
}

pub fn output(ctx: &mut Context) -> Result<()> {
    ctx.resolve()?;
    let copying = ctx.fmt("#{pane_in_mode}")? != "0";
    ctx.tmux.run(&["copy-mode", "-t", ctx.pane()?])?;
    let captured = ctx
        .tmux
        .run(&["capture-pane", "-p", "-t", ctx.pane()?, "-S", "-100000"])?;
    let rows = captured
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(i, line)| {
            let mut row = Choice::new(
                format!("{:>6}  {}", i + 1, clean(line, 1200)),
                "line",
                (i + 1).to_string(),
            );
            row.copy = clean(line, 1200);
            row
        })
        .collect();
    if let Some(row) = choose(
        ctx,
        rows,
        "Scrollback",
        "Search 100,000 lines · Enter jumps to the selected line",
    )? {
        ctx.tmux
            .run(&["send-keys", "-X", "-t", ctx.pane()?, "history-top"])?;
        let line: usize = row.value.parse()?;
        if line > 1 {
            ctx.tmux.run(&[
                "send-keys",
                "-X",
                "-N",
                &(line - 1).to_string(),
                "-t",
                ctx.pane()?,
                "cursor-down",
            ])?;
        }
    } else if !copying {
        ctx.tmux
            .run(&["send-keys", "-X", "-t", ctx.pane()?, "cancel"])?;
    }
    Ok(())
}

#[derive(Deserialize, Serialize)]
pub struct Report {
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub failed: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<(String, String)>,
}

impl Report {
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            failed: false,
            details: Vec::new(),
        }
    }
    pub fn failure(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            failed: true,
            ..Self::new(title, body)
        }
    }
    pub fn detail(mut self, label: &str, value: impl Into<String>) -> Self {
        self.details.push((label.into(), value.into()));
        self
    }
    fn label_width(&self) -> usize {
        self.details
            .iter()
            .map(|(label, _)| label.chars().count())
            .max()
            .unwrap_or(0)
    }
    fn plain(&self) -> String {
        let mut text = self.body.trim_end().to_owned();
        if !self.details.is_empty() {
            let width = self.label_width();
            text.push('\n');
            for (label, value) in &self.details {
                text.push_str(&format!("\n{label:<width$}   {value}"));
            }
        }
        text
    }
    fn rows(&self, width: usize) -> Vec<Row> {
        let inner = width.saturating_sub(4).max(8);
        let mut rows = vec![Row::Text(String::new())];
        for line in self.body.trim_end().lines() {
            rows.extend(wrap(line, inner).into_iter().map(Row::Text));
        }
        if !self.details.is_empty() {
            rows.push(Row::Text(String::new()));
            let label_width = self.label_width();
            for (label, value) in &self.details {
                let mut pieces =
                    wrap(value, inner.saturating_sub(label_width + 3).max(8)).into_iter();
                rows.push(Row::Detail(
                    format!("{label:<label_width$}"),
                    pieces.next().unwrap_or_default(),
                ));
                rows.extend(
                    pieces.map(|piece| Row::Text(format!("{:label_width$}   {piece}", ""))),
                );
            }
        }
        rows.extend([
            Row::Text(String::new()),
            Row::Hint,
            Row::Text(String::new()),
        ]);
        rows
    }
}

enum Row {
    Text(String),
    Detail(String, String),
    Hint,
}

const REPORT_HINT: &str = "q close · y copy all · v or drag to select";

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in text.split(' ') {
        let mut word: Vec<char> = word.chars().collect();
        loop {
            let current = lines.last_mut().expect("at least one line");
            let used = current.chars().count();
            if used == 0 && word.len() > width {
                current.extend(word.drain(..width));
                lines.push(String::new());
                continue;
            }
            let needed = if used == 0 {
                word.len()
            } else {
                used + 1 + word.len()
            };
            if needed <= width {
                if used > 0 {
                    current.push(' ');
                }
                current.extend(word.iter());
            } else if used == 0 {
                current.extend(word.iter());
            } else {
                lines.push(String::new());
                continue;
            }
            break;
        }
    }
    lines
}

fn fit(wanted: usize, least: usize, total: usize, percent: usize) -> usize {
    let most = (total * percent / 100).max(1);
    wanted.min(most).max(least.min(most))
}

pub fn report(ctx: &Context, report: &Report) -> Result<()> {
    if ctx.client.is_none() || ctx.pane.is_none() {
        println!("{}", report.plain());
        return Ok(());
    }
    let directory = ctx.paths.state.join("reports");
    fs::create_dir_all(&directory)?;
    let file = directory.join(format!(
        "{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    config::atomic_json(&file, report)?;
    let command = ctx.self_command(
        "_report",
        &["--data", file.to_str().ok_or("invalid report path")?],
    )?;
    let (window_width, window_height) = ctx.window_size()?;
    let widest = report
        .plain()
        .lines()
        .map(|line| line.chars().count())
        .chain([REPORT_HINT.chars().count()])
        .max()
        .unwrap_or(0);
    let width = fit(widest + 4, 40, window_width, 82);
    let height = fit(report.rows(width).len(), 4, window_height, 72);
    ctx.float(&command, &report.title, report.failed, width, height)
}

fn paint(hex: &str) -> String {
    let hex = hex.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return String::new();
    }
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16);
    match (channel(0), channel(2), channel(4)) {
        (Ok(r), Ok(g), Ok(b)) => format!("\x1b[38;2;{r};{g};{b}m"),
        _ => String::new(),
    }
}

struct RawTerminal;

impl RawTerminal {
    fn enable() -> Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        print!("\x1b[?25l");
        io::stdout().flush()?;
        Ok(Self)
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        print!("\x1b[?25h");
        let _ = io::stdout().flush();
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

pub fn show_report(ctx: &Context, path: &Path) -> Result<i32> {
    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, read};

    let report: Report = serde_json::from_str(&fs::read_to_string(path)?)?;
    let _ = fs::remove_file(path);
    let muted = paint(&ctx.tmux.option("@theme_muted"));
    let reset = "\x1b[0m";
    let own = std::env::var("TMUX_PANE").ok();
    let size = own
        .as_deref()
        .and_then(|pane| {
            ctx.tmux
                .format("#{pane_width}\t#{pane_height}", Some(pane), None)
                .ok()
        })
        .and_then(|value| {
            let (width, height) = value.split_once('\t')?;
            Some((width.parse::<usize>().ok()?, height.parse::<usize>().ok()?))
        });
    let (width, height) = size.unwrap_or((80, usize::MAX));
    let rows = report.rows(width);
    let _raw = RawTerminal::enable()?;
    let screen = rows
        .iter()
        .map(|row| match row {
            Row::Text(text) => format!("  {text}"),
            Row::Detail(label, value) => format!("  {muted}{label}{reset}   {value}"),
            Row::Hint => format!("  {muted}{REPORT_HINT}{reset}"),
        })
        .collect::<Vec<_>>()
        .join("\r\n");
    let mut out = io::stdout().lock();
    write!(out, "{screen}")?;
    out.flush()?;
    if rows.len() > height
        && let Some(pane) = &own
    {
        ctx.tmux.run(&["copy-mode", "-t", pane])?;
        ctx.tmux
            .run(&["send-keys", "-X", "-t", pane, "history-top"])?;
    }
    loop {
        let Event::Key(key) = read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter => break,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
            KeyCode::Char('y') => copy(ctx, &report.plain())?,
            KeyCode::Char('v') => {
                if let Some(pane) = &own {
                    ctx.tmux.run(&["copy-mode", "-t", pane])?;
                    ctx.tmux
                        .run(&["send-keys", "-X", "-t", pane, "begin-selection"])?;
                }
            }
            _ => {}
        }
    }
    Ok(0)
}

pub fn key_reader() -> Result<()> {
    println!("Press keys. Bytes shown after tmux decoding; Ctrl-C exits.\r");
    let _raw = RawTerminal::enable()?;
    let mut stdin = io::stdin();
    loop {
        let mut buf = [0; 128];
        let n = stdin.read(&mut buf)?;
        if n == 0 || buf[..n].contains(&3) {
            break;
        }
        let mut bytes = buf[..n].to_vec();
        loop {
            let mut fds = [PollFd::new(stdin.as_fd(), PollFlags::POLLIN)];
            if poll(&mut fds, 30u16)? == 0 {
                break;
            }
            let n = stdin.read(&mut buf)?;
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&buf[..n]);
        }
        let hex = bytes
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ");
        write!(io::stdout(), "  {hex}   {bytes:?}\r\n")?;
        io::stdout().flush()?;
    }
    Ok(())
}
