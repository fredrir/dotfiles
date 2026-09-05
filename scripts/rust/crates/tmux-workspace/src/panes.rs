use crate::{
    Result,
    config::{self, clean, identity},
    process,
    tmux::Context,
    ui::{self, Choice},
};

pub const SHELF: &str = "__workspace-shelf";

pub fn internal(ctx: &Context, name: &str, kind: &str, cwd: &str) -> Result<String> {
    if ctx
        .tmux
        .try_run(&["has-session", "-t", &format!("={name}")])?
        .code
        != 0
    {
        ctx.tmux.run(&[
            "new-session",
            "-d",
            "-s",
            name,
            "-c",
            cwd,
            "-x",
            "100",
            "-y",
            "30",
        ])?;
    }
    let id = ctx
        .tmux
        .format("#{session_id}", Some(&format!("={name}:")), None)?;
    ctx.tmux
        .run(&["set-option", "-t", &id, "@workspace-internal", kind])?;
    ctx.tmux.run(&["set-option", "-t", &id, "status", "off"])?;
    Ok(id)
}

pub fn park(ctx: &mut Context) -> Result<()> {
    ctx.resolve()?;
    if ctx.fmt("#{@workspace-tool}")? == "scratch-view" {
        return scratch(ctx);
    }
    let _lock = config::lock(
        &ctx.paths
            .state
            .join("locks")
            .join(format!("{}-panes.lock", identity(&ctx.tmux.socket()?))),
        true,
    )?;
    if ctx.fmt("#{session_name}")? == SHELF {
        return Err("pane is already on the shelf".into());
    }
    let cwd = ctx.cwd().to_string_lossy().into_owned();
    let origin = clean(&ctx.fmt("#{session_name}:#{window_name}")?, 160);
    let shelf = internal(ctx, SHELF, "shelf", &cwd)?;
    let window = ctx.fmt("#{window_id}")?;
    if ctx
        .tmux
        .panes()?
        .iter()
        .filter(|p| p.window == window && !p.floating)
        .count()
        == 1
    {
        ctx.tmux.run(&[
            "new-window",
            "-d",
            "-t",
            &ctx.fmt("#{session_id}")?,
            "-c",
            &cwd,
            "-n",
            "shell",
        ])?;
    }
    let pane = ctx.pane()?;
    ctx.tmux
        .run(&["set-option", "-p", "-t", pane, "@workspace-origin", &origin])?;
    ctx.tmux
        .run(&["set-option", "-p", "-t", pane, "@workspace-tool", "shelf"])?;
    ctx.tmux.run(&[
        "break-pane",
        "-d",
        "-s",
        pane,
        "-t",
        &format!("{shelf}:"),
        "-n",
        &origin.replace(':', "-"),
    ])?;
    ctx.notice("Pane parked · P . to retrieve");
    Ok(())
}

pub fn shelf(ctx: &mut Context, take: Option<&str>) -> Result<()> {
    ctx.resolve()?;
    let rows: Vec<_> = ctx
        .tmux
        .panes()?
        .into_iter()
        .filter(|p| p.tool == "shelf")
        .map(|p| {
            Choice::new(
                format!("{} · {} · {}", p.command, p.path, p.id),
                "pane",
                p.id,
            )
        })
        .collect();
    let selected = if let Some(id) = take {
        Some(
            rows.into_iter()
                .find(|r| r.value == id)
                .ok_or("shelf pane not found")?,
        )
    } else {
        ui::choose(
            ctx,
            rows,
            "Pane shelf",
            "Live processes · Enter retrieves into this window",
        )?
    };
    if let Some(row) = selected
        && row.value != ctx.pane()?
    {
        let _lock = config::lock(
            &ctx.paths
                .state
                .join("locks")
                .join(format!("{}-panes.lock", identity(&ctx.tmux.socket()?))),
            true,
        )?;
        if !ctx
            .tmux
            .panes()?
            .iter()
            .any(|p| p.id == row.value && p.tool == "shelf")
        {
            return Err("shelf pane changed; reopen the shelf".into());
        }
        ctx.tmux
            .run(&["join-pane", "-h", "-s", &row.value, "-t", ctx.pane()?])?;
        ctx.tmux
            .run(&["set-option", "-pu", "-t", &row.value, "@workspace-tool"])?;
    }
    Ok(())
}

pub fn scratch(ctx: &mut Context) -> Result<()> {
    ctx.resolve()?;
    let _lock = config::lock(
        &ctx.paths
            .state
            .join("locks")
            .join(format!("{}-panes.lock", identity(&ctx.tmux.socket()?))),
        true,
    )?;
    let root = if ctx.fmt("#{@workspace-tool}")? == "scratch-view" {
        ctx.fmt("#{@workspace-project}")?
    } else {
        ctx.root().to_string_lossy().into_owned()
    };
    let window = ctx.fmt("#{window_id}")?;
    if let Some(view) = ctx
        .tmux
        .panes()?
        .into_iter()
        .find(|p| p.tool == "scratch-view" && p.project == root && p.window == window)
    {
        ctx.tmux.run(&["kill-pane", "-t", &view.id])?;
        return Ok(());
    }
    let name = internal(
        ctx,
        &format!("__workspace-scratch-{}", identity(&root)),
        "scratch",
        &root,
    )?;
    for option in ["prefix", "prefix2"] {
        ctx.tmux.run(&["set-option", "-t", &name, option, "None"])?;
    }
    let socket = ctx.tmux.socket()?;
    let binary = ctx.tmux.binary.to_string_lossy();
    let pane = ctx.tmux.run(&[
        "new-pane",
        "-P",
        "-F",
        "#{pane_id}",
        "-t",
        ctx.pane()?,
        "-x",
        "82%",
        "-y",
        "72%",
        "-c",
        &root,
        "env",
        "-u",
        "TMUX",
        "-u",
        "TMUX_PANE",
        &binary,
        "-S",
        &socket,
        "attach-session",
        "-t",
        &name,
    ])?;
    ctx.tmux.run(&[
        "set-option",
        "-p",
        "-t",
        &pane,
        "@workspace-tool",
        "scratch-view",
    ])?;
    ctx.tmux
        .run(&["set-option", "-p", "-t", &pane, "@workspace-project", &root])?;
    ctx.tmux
        .run(&["select-pane", "-t", &pane, "-T", "scratch · toggle to hide"])?;
    Ok(())
}

pub fn close(ctx: &mut Context, window: bool) -> Result<()> {
    ctx.resolve()?;
    let (command, target, label) = if window {
        (
            "kill-window",
            ctx.fmt("#{window_id}")?,
            "window and its processes",
        )
    } else {
        (
            "kill-pane",
            ctx.pane()?.to_owned(),
            "pane and its processes",
        )
    };
    let action = process::shell(&[command.into(), "-t".into(), target]);
    let mut args = vec!["confirm-before", "-p"];
    let prompt = format!("Close {label}? (y/n)");
    args.push(&prompt);
    if let Some(client) = &ctx.client {
        args.extend(["-t", client]);
    }
    args.push(&action);
    ctx.tmux.run(&args)?;
    Ok(())
}
