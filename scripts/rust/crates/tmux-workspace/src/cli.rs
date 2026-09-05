use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use workstation::{Completable, Completions};

use crate::{
    Result, clients, diagnostics, integrations, panes, plugins, projects, recovery, tmux::Context,
    ui,
};

#[derive(Parser)]
#[command(
    name = "tmux-workspace",
    version,
    about = "Projects, tools and persistent tmux workspaces",
    disable_help_subcommand = true
)]
pub struct Cli {
    #[arg(long, global = true, help = "Target pane ID")]
    pub pane: Option<String>,
    #[arg(long, global = true, help = "Target attached client")]
    pub client: Option<String>,
    #[arg(long, global = true, help = "Use this tmux socket")]
    pub socket: Option<String>,
    #[arg(long, global = true, help = "Use this tmux configuration directory")]
    pub config: Option<PathBuf>,
    #[command(flatten)]
    pub completions: Completions,
    #[command(subcommand)]
    pub command: Option<Action>,
}

impl Completable for Cli {
    fn completions(&self) -> &Completions {
        &self.completions
    }
}

#[derive(Args)]
pub struct Json {
    #[arg(long, help = "Print JSON")]
    pub json: bool,
}

#[derive(Subcommand)]
pub enum Action {
    #[command(about = "Enter a project or named workspace")]
    Enter {
        target: Option<String>,
        #[arg(long)]
        session: Option<String>,
        #[arg(long = "from")]
        origin: Option<String>,
        #[arg(long, help = "Create or find the session and print its ID")]
        detach: bool,
    },
    #[command(about = "Connect to a workspace over SSH")]
    Host {
        target: Option<String>,
        #[arg(long)]
        session: Option<String>,
    },
    #[command(about = "Choose projects, worktrees and running sessions")]
    Projects(Json),
    #[command(about = "Search workspace actions", visible_alias = "help")]
    Palette,
    #[command(about = "Favorite the current project")]
    Favorite,
    #[command(about = "Park the current pane without stopping its jobs")]
    ShelfPark,
    #[command(about = "Retrieve a parked pane")]
    Shelf {
        #[arg(long, help = "Retrieve this shelf pane ID")]
        take: Option<String>,
    },
    #[command(about = "Toggle the persistent scratch drawer")]
    Scratch,
    #[command(about = "List workspace panes")]
    Panes,
    #[command(about = "Open Lazygit for the current project")]
    Lazygit,
    #[command(about = "Open Yazi for the current directory")]
    Yazi {
        #[arg(long)]
        cwd_file: Option<PathBuf>,
    },
    #[command(about = "Choose or resume an agent conversation")]
    Agent,
    #[command(about = "Start managed Codex")]
    AgentCodex,
    #[command(about = "Start managed Claude")]
    AgentClaude,
    #[command(about = "Move agent execution to another host")]
    Handoff,
    #[command(about = "Show agent handoff status")]
    HandoffStatus,
    #[command(about = "Follow execution to its destination")]
    AgentFollow,
    #[command(about = "Cancel a queued agent move")]
    HandoffCancel,
    #[command(about = "Recover a failed handoff")]
    HandoffRecover,
    #[command(about = "Inspect key routing and terminal capabilities")]
    Inspect(Json),
    #[command(about = "Read actual input bytes in a pane")]
    InspectKeys,
    #[command(about = "Search scrollback and jump to a line")]
    Output,
    #[command(about = "Select and copy a path, URL or token")]
    QuickSelect,
    #[command(about = "Save a recovery snapshot")]
    Save,
    #[command(about = "Restore layouts and supported programs")]
    Restore {
        #[arg(long, help = "Restore without the confirmation picker")]
        yes: bool,
    },
    #[command(about = "Show versions, tools and plugin health")]
    Doctor(Json),
    #[command(about = "Validate and reload the configuration")]
    Reload,
    #[command(about = "Confirm closing this pane and its jobs")]
    ClosePane,
    #[command(about = "Confirm closing this window and its jobs")]
    CloseWindow,
    #[command(about = "Install or load pinned integrations")]
    Plugins {
        #[command(subcommand)]
        command: PluginAction,
    },
    #[command(hide = true)]
    ClientUpdate {
        #[arg(long = "from")]
        origin: Option<String>,
    },
    #[command(hide = true)]
    ClientRemove {
        #[arg(long)]
        tty: Option<String>,
    },
    #[command(name = "_pick", hide = true)]
    Pick {
        #[arg(long)]
        data: PathBuf,
        #[arg(long)]
        result: PathBuf,
        #[arg(long)]
        done: Option<PathBuf>,
    },
    #[command(name = "_report", hide = true)]
    Report {
        #[arg(long)]
        data: PathBuf,
    },
    #[command(name = "_keys", hide = true)]
    Keys,
    #[command(name = "_host-client", hide = true)]
    HostClient {
        target: String,
        #[arg(long)]
        session: Option<String>,
    },
    #[command(name = "_agent-follow-client", hide = true)]
    AgentFollowClient,
    #[command(name = "_agent-recover-client", hide = true)]
    AgentRecoverClient,
}

#[derive(Subcommand)]
pub enum PluginAction {
    #[command(about = "Provision pinned plugins without a running server")]
    Install,
    #[command(about = "Load installed plugins into this server", alias = "bootstrap")]
    Load,
    #[command(about = "Show installed artifacts and server health")]
    Status(Json),
    #[command(hide = true)]
    Fingers,
    #[command(hide = true)]
    Save,
    #[command(hide = true)]
    Restore,
}

pub fn dispatch(ctx: &mut Context, command: Action) -> Result<i32> {
    match command {
        Action::Enter {
            target,
            session,
            origin,
            detach,
        } => {
            return projects::enter(
                ctx,
                target.as_deref(),
                session.as_deref(),
                origin.as_deref(),
                detach,
            );
        }
        Action::Host { target, session } => {
            return integrations::host(ctx, target.as_deref(), session.as_deref(), false);
        }
        Action::Projects(json) if json.json => {
            println!("{}", serde_json::to_string_pretty(&projects::rows(ctx)?)?)
        }
        Action::Projects(_) => return projects::choose(ctx),
        Action::Palette => return ui::palette(ctx),
        Action::Favorite => projects::favorite(ctx)?,
        Action::ShelfPark => panes::park(ctx)?,
        Action::Shelf { take } => panes::shelf(ctx, take.as_deref())?,
        Action::Scratch => panes::scratch(ctx)?,
        Action::Panes => println!("{}", serde_json::to_string_pretty(&ctx.tmux.panes()?)?),
        Action::Lazygit => return integrations::launch(ctx, "lazygit"),
        Action::Yazi { cwd_file } => integrations::yazi(ctx, cwd_file.as_deref())?,
        Action::Agent => return integrations::launch(ctx, "agent"),
        Action::AgentCodex => return integrations::launch(ctx, "agent-codex"),
        Action::AgentClaude => return integrations::launch(ctx, "agent-claude"),
        Action::Handoff => return integrations::launch(ctx, "handoff"),
        Action::HandoffStatus => return integrations::launch(ctx, "handoff-status"),
        Action::AgentFollow => return integrations::launch(ctx, "agent-follow"),
        Action::HandoffCancel => return integrations::launch(ctx, "handoff-cancel"),
        Action::HandoffRecover => return integrations::launch(ctx, "handoff-recover"),
        Action::Inspect(json) => diagnostics::inspect(ctx, json.json)?,
        Action::InspectKeys => {
            ctx.resolve()?;
            let command = ctx.self_command("_keys", &[])?;
            let mut args = vec!["split-window", "-t", ctx.pane()?, "-l", "40%"];
            args.extend(command.iter().map(String::as_str));
            ctx.tmux.run(&args)?;
        }
        Action::Output => ui::output(ctx)?,
        Action::QuickSelect => ui::quick_select(ctx)?,
        Action::Save => recovery::run(ctx, false, false)?,
        Action::Restore { yes } => recovery::run(ctx, true, yes)?,
        Action::Doctor(json) => diagnostics::doctor(ctx, json.json)?,
        Action::Reload => diagnostics::reload(ctx)?,
        Action::ClosePane => panes::close(ctx, false)?,
        Action::CloseWindow => panes::close(ctx, true)?,
        Action::Plugins { command } => match command {
            PluginAction::Install => plugins::install(&ctx.paths)?,
            PluginAction::Load => plugins::load(ctx)?,
            PluginAction::Status(json) => diagnostics::plugin_status(ctx, json.json)?,
            PluginAction::Fingers => return plugins::fingers(ctx),
            PluginAction::Save => recovery::run(ctx, false, true)?,
            PluginAction::Restore => recovery::run(ctx, true, true)?,
        },
        Action::ClientUpdate { origin } => clients::update(ctx, false, None, origin.as_deref())?,
        Action::ClientRemove { tty } => clients::update(ctx, true, tty.as_deref(), None)?,
        Action::Pick { data, result, done } => {
            let picked = ui::pick(&data, &result);
            if let Some(done) = done {
                std::fs::write(done, "")?;
            }
            picked?;
        }
        Action::Report { data } => return ui::show_report(&data),
        Action::Keys => ui::key_reader()?,
        Action::HostClient { target, session } => {
            return integrations::host(ctx, Some(&target), session.as_deref(), true);
        }
        Action::AgentFollowClient => return integrations::agent_client(ctx, false),
        Action::AgentRecoverClient => return integrations::agent_client(ctx, true),
    }
    Ok(0)
}

pub fn action(ctx: &mut Context, command: &str) -> Result<i32> {
    let cli = Cli::try_parse_from(["tmux-workspace", command])?;
    dispatch(ctx, cli.command.ok_or("action required")?)
}

pub fn needs_tmux(action: &Action) -> bool {
    !matches!(
        action,
        Action::Doctor(_)
            | Action::Pick { .. }
            | Action::Report { .. }
            | Action::Keys
            | Action::Host { .. }
            | Action::HostClient { .. }
            | Action::ClientRemove { .. }
            | Action::Plugins {
                command: PluginAction::Install | PluginAction::Status(_) | PluginAction::Load
            }
    )
}
