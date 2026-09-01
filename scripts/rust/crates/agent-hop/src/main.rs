#![forbid(unsafe_code)]

use std::process::ExitCode;

use agent_hop::cli::Cli;
use clap::Parser;

const PROGRAM: &str = "agent-hop";

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Some(status) = cli.completions.emit::<Cli>(PROGRAM) {
        return status;
    }
    match agent_hop::run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => workstation::fail(PROGRAM, message),
    }
}
