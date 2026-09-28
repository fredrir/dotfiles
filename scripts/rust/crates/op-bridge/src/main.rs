#![forbid(unsafe_code)]

mod broker;
mod client;
mod daemon;
mod onepassword;
mod paths;
mod protocol;
mod touchid;
mod tunnel;

use std::ffi::OsString;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use workstation::{Completable, Completions};

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
            help = "Vault the peer may read; repeat for more"
        )]
        vaults: Vec<String>,
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
        None => Err("choose a command: daemon or op; see --help".to_string()),
        Some(Command::Daemon { vaults }) => daemon::run(vaults).map(|()| ExitCode::SUCCESS),
        Some(Command::Op { args }) => client::run(args),
    })
}
