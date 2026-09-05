use std::time::Duration;

use crate::{
    Result, clients, process,
    tmux::{Context, Tmux},
    ui,
};

pub fn reload(ctx: &mut Context) -> Result<()> {
    let source = [
        ctx.paths.config.join(".tmux.conf"),
        ctx.paths.config.join("tmux.conf"),
    ]
    .into_iter()
    .find(|p| p.is_file())
    .ok_or("tmux.conf not found")?;
    let directory = tempfile::Builder::new()
        .prefix("tmux-validate-")
        .tempdir()?;
    let validation = Tmux {
        binary: ctx.tmux.binary.clone(),
        socket: Some(directory.path().join("socket").display().to_string()),
    };
    let checked = (|| -> Result<()> {
        let invoke = |args: &[&str]| {
            process::capture(
                validation
                    .command()
                    .args(args)
                    .env("DOTFILES_TMUX_VALIDATE", "1")
                    .env_remove("TMUX")
                    .env_remove("TMUX_PANE"),
                None,
                Some(Duration::from_secs(10)),
            )
        };
        invoke(&[
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "validate",
            "/bin/sh",
        ])?
        .checked()?;
        invoke(&[
            "set-option",
            "-g",
            "@workspace_config",
            ctx.paths.config.to_str().ok_or("invalid config path")?,
        ])?
        .checked()?;
        let output = invoke(&["source-file", source.to_str().ok_or("invalid config path")?])?;
        if output.code != 0 || !output.err.trim().is_empty() || !output.out.trim().is_empty() {
            return Err(format!("reload validation: {}{}", output.out, output.err).into());
        }
        Ok(())
    })();
    let _ = validation.run(&["kill-server"]);
    checked?;
    let output = ctx
        .tmux
        .try_run(&["source-file", source.to_str().ok_or("invalid config path")?])?;
    if output.code != 0 || !output.err.trim().is_empty() {
        return Err(format!("reload: {}{}", output.out, output.err).into());
    }
    for client in ctx.tmux.clients()?.into_iter().filter(|c| !c.internal) {
        let mut client_ctx = ctx.clone();
        client_ctx.client = Some(client.name);
        clients::update(&client_ctx, false, None, None)?;
    }
    ctx.notice("Tmux reloaded");
    Ok(())
}

pub fn inspect(ctx: &mut Context, json: bool) -> Result<()> {
    ctx.resolve()?;
    let mut values = serde_json::Map::new();
    for field in [
        "host",
        "version",
        "client_name",
        "client_pid",
        "client_created",
        "client_termname",
        "client_termfeatures",
        "client_key_table",
        "client_prefix",
        "pane_id",
        "pane_current_command",
        "pane_key_mode",
        "pane_in_mode",
        "pane_floating_flag",
        "@workspace-client-label",
    ] {
        let value = ctx.fmt(&format!(
            "#{{{}{field}}}",
            if field == "@workspace-client-label" {
                "E:"
            } else {
                ""
            }
        ))?;
        values.insert(field.into(), value.into());
    }
    for option in [
        "extended-keys",
        "extended-keys-format",
        "escape-time",
        "set-clipboard",
    ] {
        values.insert(
            option.into(),
            ctx.tmux.run(&["show-options", "-sv", option])?.into(),
        );
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&values)?);
        return Ok(());
    }
    let mut lines: Vec<_> = values
        .iter()
        .map(|(key, value)| format!("{key:26} {}", value.as_str().unwrap_or("")))
        .collect();
    lines.extend([
        "".into(),
        "Desktop gesture → WezTerm adapter → tmux key table → application".into(),
        "P P forwards a prefix into a nested remote tmux.".into(),
        "P Space → Read actual input bytes: checks bytes reaching a tmux pane.".into(),
        "The terminal may consume a key before tmux sees it.".into(),
        "".into(),
        "Registered actions:".into(),
    ]);
    lines.extend(ui::bindings(ctx)?.into_iter().map(|r| r.label));
    ui::report(ctx, &lines.join("\n"), "Key routing")
}

pub fn doctor(ctx: &Context, json: bool) -> Result<()> {
    let mut tools = serde_json::Map::new();
    for tool in [
        "fzf",
        "git",
        "curl",
        "zoxide",
        "lazygit",
        "yazi",
        "agent-hop",
        "ssh",
        "bash",
    ] {
        tools.insert(
            tool.into(),
            process::which(tool)
                .map(|p| serde_json::json!(p))
                .unwrap_or(serde_json::Value::Null),
        );
    }
    let plugins =
        crate::plugins::status(ctx).unwrap_or_else(|e| serde_json::json!({"error": e.to_string()}));
    let report = serde_json::json!({"host": crate::config::hostname(), "tmux": ctx.tmux.run(&["-V"])?, "controller": std::env::current_exe()?, "config": ctx.paths.config, "socket": ctx.tmux.socket, "tools": tools, "plugins": plugins, "minimum": "3.7c"});
    let content = if json {
        serde_json::to_string_pretty(&report)?
    } else {
        let mut lines = vec![
            format!("host        {}", crate::config::hostname()),
            format!(
                "tmux        {} (minimum 3.7c)",
                report["tmux"].as_str().unwrap_or("")
            ),
            format!("controller  {}", std::env::current_exe()?.display()),
            format!("config      {}", ctx.paths.config.display()),
        ];
        lines.extend(
            tools
                .iter()
                .map(|(key, value)| format!("{key:12}{}", value.as_str().unwrap_or("missing"))),
        );
        lines.push(format!(
            "\nPlugins\n{}",
            serde_json::to_string_pretty(&plugins)?
        ));
        lines.join("\n")
    };
    if json {
        println!("{content}");
        Ok(())
    } else {
        ui::report(ctx, &content, "Workspace doctor")
    }
}

pub fn plugin_status(ctx: &Context, json: bool) -> Result<()> {
    let state = crate::plugins::status(ctx)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&state)?);
    } else {
        for name in ["resurrect", "fingers"] {
            let item = &state[name];
            println!(
                "{name:10} {}  {}  {}",
                item[if name == "fingers" {
                    "version"
                } else {
                    "revision"
                }]
                .as_str()
                .unwrap_or(""),
                if item["installed"] == true {
                    "installed"
                } else {
                    "missing"
                },
                item["path"].as_str().unwrap_or("")
            );
            if let Some(error) = item["error"].as_str() {
                println!("{error}");
            }
        }
        if let Some(errors) = state["installation"]["errors"].as_array() {
            for error in errors {
                println!("{}", error.as_str().unwrap_or(""));
            }
        }
        if let Some(error) = state["server"]["error"].as_str().filter(|v| !v.is_empty()) {
            println!("server: {error}");
        }
    }
    Ok(())
}
