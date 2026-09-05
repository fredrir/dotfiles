use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;

use crate::{
    Result, config, process,
    tmux::Context,
    ui::{self, Choice},
};

pub fn yazi(ctx: &mut Context, cwd_file: Option<&Path>) -> Result<()> {
    ctx.resolve()?;
    if process::which("yazi").is_none() {
        return Err("yazi: not installed".into());
    }
    if let Some(cwd_file) = cwd_file {
        let directory = tempfile::Builder::new().prefix("tmux-yazi-").tempdir()?;
        let chosen = directory.path().join("chosen");
        ctx.popup(
            &[
                "yazi".into(),
                "--chooser-file".into(),
                chosen.display().to_string(),
                "--cwd-file".into(),
                cwd_file.display().to_string(),
                ctx.cwd().display().to_string(),
            ],
            "Files",
            &ctx.cwd(),
            true,
        )?;
        if let Ok(value) = fs::read_to_string(chosen) {
            let value = value.trim_end_matches('\n');
            if !value.contains('\n') && Path::new(value).is_dir() {
                fs::write(cwd_file, value)?;
            }
        }
    } else if ctx.fmt("#{pane_current_command}")? == "zsh" {
        ctx.tmux
            .run(&["send-keys", "-t", ctx.pane()?, "-l", "\x1b[115;9u"])?;
    } else {
        ctx.popup(
            &["yazi".into(), ctx.cwd().display().to_string()],
            "Files",
            &ctx.cwd(),
            true,
        )?;
    }
    Ok(())
}

pub fn launch(ctx: &mut Context, action: &str) -> Result<i32> {
    ctx.resolve()?;
    let program = if action == "lazygit" {
        "lazygit"
    } else {
        "agent-hop"
    };
    if process::which(program).is_none() {
        return Err(format!("{program}: not installed").into());
    }
    let root = ctx.root();
    match action {
        "lazygit" | "agent" => ctx.popup(
            &[program.into()],
            if action == "agent" {
                "Agent sessions"
            } else {
                "Git"
            },
            &root,
            true,
        )?,
        "agent-codex" | "agent-claude" => {
            let agent = action.strip_prefix("agent-").ok_or("invalid agent")?;
            ctx.tmux.run(&[
                "new-window",
                "-t",
                &ctx.fmt("#{session_id}")?,
                "-c",
                root.to_str().ok_or("invalid directory")?,
                "-n",
                agent,
                "agent-hop",
                "run",
                agent,
            ])?;
        }
        "agent-follow" => {
            let output = process::run(
                Command::new(program)
                    .args(["status", "--pane", ctx.pane()?])
                    .current_dir(&root),
            )?;
            let status: serde_json::Value = serde_json::from_str(&output.out)?;
            if !matches!(
                status["phase"].as_str(),
                Some("moved" | "commit-uncertain" | "source-stopped")
            ) {
                ui::report(ctx, &output.out, "Handoff pending · destination not ready")?;
            } else {
                client_window(ctx, "_agent-follow-client", "agent-remote")?;
            }
        }
        "handoff-recover" => client_window(ctx, "_agent-recover-client", "agent-recovery")?,
        "handoff" | "handoff-status" | "handoff-cancel" => {
            let (operation, title) = match action {
                "handoff" => ("move", "Move execution · q closes"),
                "handoff-status" => ("status", "Agent handoff"),
                _ => ("cancel", "Cancel queued move"),
            };
            let output = process::capture(
                Command::new(program)
                    .args([operation, "--pane", ctx.pane()?])
                    .current_dir(&root),
                None,
                None,
            )?;
            ui::report(ctx, &(output.out + &output.err), title)?;
            return Ok(output.code);
        }
        _ => return Err("unknown agent action".into()),
    }
    Ok(0)
}

fn client_window(ctx: &Context, action: &str, name: &str) -> Result<()> {
    let command = ctx.self_command(action, &[])?;
    let session = ctx.fmt("#{session_id}")?;
    let root = ctx.root();
    let mut args = vec![
        "new-window",
        "-t",
        &session,
        "-c",
        root.to_str().ok_or("invalid directory")?,
        "-n",
        name,
    ];
    args.extend(command.iter().map(String::as_str));
    ctx.tmux.run(&args)?;
    Ok(())
}

pub fn agent_client(ctx: &Context, recover: bool) -> Result<i32> {
    let operation = if recover { "recover" } else { "follow" };
    let status =
        process::interactive(Command::new("agent-hop").args([operation, "--pane", ctx.pane()?]))?;
    if status != 0 {
        pause(&format!("Agent {operation} exited {status}."));
    }
    Ok(status)
}

fn pause(message: &str) {
    println!("{message}\nEnter to close");
    let _ = io::stdin().read_line(&mut String::new());
}

pub fn host(
    ctx: &mut Context,
    target: Option<&str>,
    session: Option<&str>,
    child: bool,
) -> Result<i32> {
    let target = if let Some(target) = target {
        target.to_owned()
    } else {
        let rows = ctx
            .paths
            .hosts()?
            .into_iter()
            .filter(|h| h != &config::hostname())
            .map(|h| Choice::new(h.clone(), "host", h))
            .collect();
        let Some(row) = ui::choose(ctx, rows, "Connect host", "SSH workspace attachment")? else {
            return Ok(0);
        };
        row.value
    };
    if !regex::Regex::new(r"^[a-zA-Z0-9_][a-zA-Z0-9_.@-]*$")?.is_match(&target) {
        return Err("invalid SSH host".into());
    }
    if target == config::hostname() {
        return if ctx.pane.is_some() {
            crate::projects::choose(ctx)
        } else {
            crate::projects::enter(ctx, None, session, None, false)
        };
    }
    let session = session.unwrap_or("main");
    if !child && (ctx.pane.is_some() || std::env::var_os("TMUX").is_some()) {
        ctx.resolve()?;
        let command = [
            std::env::current_exe()?.display().to_string(),
            "_host-client".into(),
            target.clone(),
            "--session".into(),
            session.into(),
            "--config".into(),
            ctx.paths.config.display().to_string(),
        ];
        let id = ctx.fmt("#{session_id}")?;
        let mut args = vec!["new-window", "-t", &id, "-n", &target];
        args.extend(command.iter().map(String::as_str));
        ctx.tmux.run(&args)?;
        return Ok(0);
    }
    let remote = format!(
        "unset HWIRE_SESSION TMUX TMUX_PANE TMUX_WORKSPACE_SOCKET; export PATH=\"$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH\"; exec tmux-workspace enter --from {} --session {}",
        process::quote(&config::hostname()),
        process::quote(session)
    );
    let status = process::interactive(Command::new("ssh").args([
        "-tt",
        "-o",
        "ConnectTimeout=8",
        "-o",
        "LogLevel=ERROR",
        "--",
        &target,
        &remote,
    ]))?;
    if status != 0 && child {
        pause(&format!("Connection to {target} exited {status}."));
    }
    Ok(status)
}
