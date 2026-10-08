#![forbid(unsafe_code)]

mod args;
mod placement;
mod run;

use std::process::ExitCode;

fn main() -> ExitCode {
    workstation::run("dotfmt", run::run)
}
