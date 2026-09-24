#![forbid(unsafe_code)]

mod config;
mod daemon;
mod docker;
mod forward;
mod listener;
mod master;
mod plan;
mod render;
mod setup;
mod state;
mod stream;
mod watch;

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use hostkit::Host;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use workstation::{Completable, Completions, Style};

use crate::config::Config;
use crate::state::Paths;

const PROGRAM: &str = "hport";

#[derive(Parser)]
#[command(
    version,
    about = "Forward the peer's listening ports to <peer>:PORT and localhost:PORT"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[arg(long = "json", help = "Print the daemon state as JSON")]
    json: bool,

    #[command(flatten)]
    completions: Completions,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Forward the peer's ports until stopped")]
    Daemon,

    #[command(about = "Print this machine's forwardable listeners as JSON")]
    Listeners {
        #[arg(short = 'w', long = "watch", help = "Print again on every change")]
        watch: bool,
    },

    #[command(about = "Map the peer name, add the loopback alias, start the daemon")]
    Setup {
        #[arg(
            short = 'n',
            long = "dry-run",
            help = "Show missing steps without applying them"
        )]
        dry_run: bool,
    },
}

impl Completable for Cli {
    fn completions(&self) -> &Completions {
        &self.completions
    }
}

fn main() -> ExitCode {
    workstation::run::<Cli>(PROGRAM, |cli| {
        match cli.command {
            None => status(cli.json),
            Some(Command::Daemon) => daemon::run(&Config::load()?),
            Some(Command::Listeners { watch }) => watch::run(watch),
            Some(Command::Setup { dry_run }) => setup::run(&Style::for_stdout(), dry_run),
        }
        .map(|()| ExitCode::SUCCESS)
    })
}

fn status(json: bool) -> Result<(), String> {
    let this = Host::this()?;
    let state = state::read(&Paths::resolve()?)?
        .filter(|state| running(state.pid))
        .ok_or("daemon not running; run hport setup")?;
    if json {
        let text = serde_json::to_string_pretty(&state).map_err(|error| error.to_string())?;
        println!("{text}");
    } else {
        println!("{}", render::status(&Style::for_stdout(), this, &state));
    }
    Ok(())
}

fn running(pid: u32) -> bool {
    i32::try_from(pid).is_ok_and(|pid| kill(Pid::from_raw(pid), None).is_ok())
}
