#![forbid(unsafe_code)]

mod cli;

use std::process::ExitCode;

use lsp_server::Connection;
use workstation::run;

use crate::cli::Cli;

const PROGRAM: &str = "dot-code-server";

fn main() -> ExitCode {
    run(PROGRAM, serve)
}

fn serve(_: Cli) -> Result<ExitCode, String> {
    let (connection, threads) = Connection::stdio();
    let served = dot_code_server::run(&connection, dot_code_server::features());
    drop(connection);
    match served.and(threads.join().map_err(Into::into)) {
        Ok(()) => Ok(ExitCode::SUCCESS),
        Err(error) => Err(error.to_string()),
    }
}
