#![forbid(unsafe_code)]

use std::process::ExitCode;

use lsp_server::Connection;

fn main() -> ExitCode {
    let (connection, threads) = Connection::stdio();
    let served = dot_code_server::run(&connection, dot_code_server::features());
    drop(connection);
    match served.and(threads.join().map_err(Into::into)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("dot-code-server: {error}");
            ExitCode::FAILURE
        }
    }
}
