use std::process::ExitCode;

use agent_hop::cli::Cli;

const PROGRAM: &str = "agent-hop";

fn main() -> ExitCode {
    workstation::run::<Cli>(PROGRAM, |cli| {
        agent_hop::run(cli).map(|()| ExitCode::SUCCESS)
    })
}
