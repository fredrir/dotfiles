#![forbid(unsafe_code)]
use std::process::ExitCode;

fn main() -> ExitCode {
    match hwtune::cli::entry() {
        Ok(code) => code,
        Err(error) => workstation::fail("hwtune", error),
    }
}
