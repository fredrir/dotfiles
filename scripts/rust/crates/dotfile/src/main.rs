use std::ffi::OsString;
use std::process::ExitCode;

use dotfile_cli::cli::SyncCli;

fn main() -> ExitCode {
    let mut arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    if arguments.first().and_then(|value| value.to_str()) != Some("sync") {
        return dotfile_cli::backend::delegate(arguments);
    }
    if let Some(code) = dotfile_cli::wire::dispatch(&arguments[1..]) {
        return code;
    }
    let original_arguments = arguments.clone();
    let sync_arguments = arguments.split_off(1);
    let cli = match SyncCli::parse_tail(sync_arguments) {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            return ExitCode::from(code as u8);
        }
    };
    if let Err(error) = dotfile_cli::tooling::refresh(&cli, &original_arguments) {
        eprintln!("dotfile: {error}");
        return ExitCode::FAILURE;
    }
    let verbose = cli.verbose;
    dotfile_cli::cancel::reset();
    let (sender, receiver) = crossbeam_channel::bounded(256);
    let (decision_client, decision_server) = dotfile_cli::decision::channel();
    let worker =
        std::thread::spawn(move || dotfile_cli::sync::run(&cli, &sender, &decision_client));
    match dotfile_cli::ui::run(receiver, decision_server, worker, verbose) {
        Ok(summary) => {
            println!("{}", dotfile_cli::ui::completion_line(&summary));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("dotfile: {error}");
            if let Some(code) = dotfile_cli::ui::signal_exit_code() {
                ExitCode::from(code)
            } else if dotfile_cli::cancel::requested() {
                ExitCode::from(130)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}
