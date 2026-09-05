use std::process::ExitCode;

use hcopy::cli::Push;

const PROGRAM: &str = "hpush";

fn main() -> ExitCode {
    workstation::run::<Push>(PROGRAM, |cli| {
        hcopy::main(cli.into()).map(|()| ExitCode::SUCCESS)
    })
}
