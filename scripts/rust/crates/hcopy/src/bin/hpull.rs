use std::process::ExitCode;

use clap::Parser;
use hcopy::cli::Pull;

const PROGRAM: &str = "hpull";

fn main() -> ExitCode {
    let cli = Pull::parse();
    if let Some(status) = cli.common.completions.emit::<Pull>(PROGRAM) {
        return status;
    }
    match hcopy::main(cli.into()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => workstation::fail(PROGRAM, message),
    }
}
