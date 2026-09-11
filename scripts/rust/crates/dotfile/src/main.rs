#![forbid(unsafe_code)]

fn main() -> std::process::ExitCode {
    dotfile_cli::cli::dispatch(std::env::args_os().skip(1).collect())
}
