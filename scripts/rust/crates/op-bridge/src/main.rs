#![forbid(unsafe_code)]

mod broker;
mod client;
mod daemon;
mod onepassword;
mod paths;
mod presence;
mod protocol;
mod setup;
mod tunnel;

use std::ffi::OsString;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use workstation::{Completable, Completions, Style};

const PROGRAM: &str = "op-bridge";

#[derive(Parser)]
#[command(
    version,
    about = "1Password reads on archie, approved with Touch ID on macie"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[command(flatten)]
    completions: Completions,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Answer the peer's reads until stopped; run by launchd on macie")]
    Daemon {
        #[arg(
            long = "vault",
            value_name = "VAULT",
            default_value = "Dev",
            help = "Vault served without Touch ID until macie sleeps; repeat for more"
        )]
        silent: Vec<String>,

        #[arg(
            long = "prompt-vault",
            value_name = "VAULT",
            help = "Vault that needs Touch ID per reference; repeat for more"
        )]
        prompt: Vec<String>,
    },

    #[command(
        about = "Clear the daemon's memory and refetch its references, e.g. after a rotation"
    )]
    Reload,

    #[command(about = "Install the signed app on macie and (re)start its launchd agent")]
    Setup {
        #[arg(
            long = "identity",
            value_name = "NAME",
            default_value = setup::DEFAULT_IDENTITY,
            help = "Code signing identity; any unique part of its name"
        )]
        identity: String,

        #[arg(
            short = 'n',
            long = "dry-run",
            help = "Show missing steps without applying them"
        )]
        dry_run: bool,
    },

    #[command(
        about = "Run as op: reads go to macie, everything else to the real op",
        disable_help_flag = true
    )]
    Op {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
}

impl Completable for Cli {
    fn completions(&self) -> &Completions {
        &self.completions
    }
}

fn main() -> ExitCode {
    workstation::run::<Cli>(PROGRAM, |cli| match cli.command {
        None => Err("choose a command: daemon, reload, setup or op; see --help".to_string()),
        Some(Command::Daemon { silent, prompt }) => {
            daemon::run(silent, prompt).map(|()| ExitCode::SUCCESS)
        }
        Some(Command::Setup { identity, dry_run }) => {
            setup::run(&Style::for_stdout(), dry_run, &identity).map(|()| ExitCode::SUCCESS)
        }
        Some(Command::Reload) => client::reload(),
        Some(Command::Op { args }) => client::run(args),
    })
}
