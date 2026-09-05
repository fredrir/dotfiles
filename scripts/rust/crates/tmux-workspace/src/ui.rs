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
}

impl Choice {
    pub fn new(label: impl Into<String>, kind: &str, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            kind: kind.into(),
            value: value.into(),
            table: String::new(),
        }
    }
}

#[derive(Deserialize, Serialize)]
struct Picker {
    rows: Vec<Choice>,
    title: String,
    header: String,
    colors: String,
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
    let picker = Picker {
        rows,
        title: title.into(),
        header: header.into(),
        colors: ctx.tmux.option("@theme_fzf_colors"),
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
    if ctx.pane.is_none() && io::stdin().is_terminal() {
        process::interactive(Command::new(&command[0]).args(&command[1..]))?;
    } else {
        ctx.resolve()?;
        ctx.popup(&command, title, &ctx.cwd(), true)?;
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
            .map(|(i, row)| format!("{i}\t{}", clean(&row.label, 1200)))
            .collect::<Vec<_>>()
            .join("\n");
        let mut cmd = Command::new("fzf");
        cmd.env("FZF_DEFAULT_OPTS", "")
            .env("FZF_DEFAULT_OPTS_FILE", "")
            .args([
                "--layout=reverse",
                "--border=rounded",
                "--no-multi",
                "--delimiter=\t",
                "--with-nth=2..",
                "--cycle",
                "--prompt",
                &format!("{} › ", picker.title),
                "--header",
                &picker.header,
            ]);
        if !picker.colors.is_empty() {
            cmd.args(["--color", &picker.colors]);
        }
        let result = process::capture(&mut cmd, Some(lines.as_bytes()), None)?;
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
            Choice::new(
                format!("{:>6}  {}", i + 1, clean(line, 1200)),
                "line",
                (i + 1).to_string(),
            )
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

pub fn report(ctx: &Context, value: &str, title: &str) -> Result<()> {
    if ctx.client.is_none() {
        println!("{value}");
        return Ok(());
    }
    let directory = tempfile::Builder::new().prefix("tmux-report-").tempdir()?;
    let file = directory.path().join("report");
    fs::write(&file, value)?;
    ctx.popup(
        &ctx.self_command(
            "_report",
            &["--data", file.to_str().ok_or("invalid report path")?],
        )?,
        title,
        &ctx.cwd(),
        true,
    )
}

pub fn show_report(path: &Path) -> Result<i32> {
    if process::which("less").is_some() {
        return process::interactive(Command::new("less").args(["-R", "--"]).arg(path));
    }
    println!("{}", fs::read_to_string(path)?);
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(0)
}

pub fn key_reader() -> Result<()> {
    println!("Press keys. Bytes shown after tmux decoding; Ctrl-C exits.\r");
    crossterm::terminal::enable_raw_mode()?;
    struct Raw;
    impl Drop for Raw {
        fn drop(&mut self) {
            let _ = crossterm::terminal::disable_raw_mode();
        }
    }
    let _raw = Raw;
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
