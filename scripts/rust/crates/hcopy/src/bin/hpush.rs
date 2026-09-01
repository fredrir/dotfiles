#![forbid(unsafe_code)]

use std::process::ExitCode;

use clap::Parser;
use hcopy::cli::Push;

const PROGRAM: &str = "hpush";

fn main() -> ExitCode {
    let cli = Push::parse();
    if let Some(status) = cli.common.completions.emit::<Push>(PROGRAM) {
        return status;
    }
    match hcopy::main(cli.into()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => workstation::fail(PROGRAM, message),
    }
}
