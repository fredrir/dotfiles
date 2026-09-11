use std::process::ExitCode;

fn main() -> ExitCode {
    workstation::run("hwtune", hwtune::cli::run)
}
