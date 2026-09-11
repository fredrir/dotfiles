#![forbid(unsafe_code)]

fn main() -> std::process::ExitCode {
    workstation::run::<dcloud::cli::Cli>("dcloud", dcloud::app::run)
}
