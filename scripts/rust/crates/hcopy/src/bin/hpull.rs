use std::process::ExitCode;

use hcopy::cli::Pull;

const PROGRAM: &str = "hpull";

fn main() -> ExitCode {
    workstation::run::<Pull>(PROGRAM, |cli| {
        hcopy::main(cli.into()).map(|()| ExitCode::SUCCESS)
    })
}
