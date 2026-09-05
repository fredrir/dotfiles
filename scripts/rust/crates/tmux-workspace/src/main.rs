mod cli;
mod clients;
mod config;
mod diagnostics;
mod integrations;
mod panes;
mod plugins;
mod process;
mod projects;
mod recovery;
mod tmux;
mod ui;

use clap::CommandFactory;
use std::process::ExitCode;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn main() -> ExitCode {
    workstation::run::<cli::Cli>("tmux-workspace", |args| {
        let Some(command) = args.command else {
            let _ = cli::Cli::command().print_help();
            println!();
            return Ok(ExitCode::SUCCESS);
        };
        let attached = args.socket.is_some()
            || std::env::var_os("TMUX").is_some()
            || std::env::var_os("TMUX_WORKSPACE_SOCKET").is_some();
        let tmux = tmux::Tmux::new(args.socket);
        let configured = args.config.or_else(|| {
            if !attached || std::env::var_os("DOTFILES_TMUX_CONFIG").is_some() {
                return None;
            }
            let value = tmux.option("@workspace_config");
            (!value.is_empty()).then(|| value.into())
        });
        let mut ctx = tmux::Context {
            tmux,
            paths: config::Paths::new(configured),
            pane: args.pane.or_else(|| std::env::var("TMUX_PANE").ok()),
            client: args.client,
        };
        let result = (|| -> Result<i32> {
            if cli::needs_tmux(&command) {
                ctx.tmux.require_version()?;
            }
            cli::dispatch(&mut ctx, command)
        })();
        match result {
            Ok(code) => Ok(workstation::exit_code(code)),
            Err(error) => {
                if ctx.client.is_some() {
                    ctx.notice(&format!("tmux-workspace: {error}"));
                }
                Err(error.to_string())
            }
        }
    })
}
