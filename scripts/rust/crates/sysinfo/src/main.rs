use std::process::ExitCode;
fn main() -> ExitCode {
    match workstation_sysinfo::cli::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => workstation::fail("sysinfo", error),
    }
}
