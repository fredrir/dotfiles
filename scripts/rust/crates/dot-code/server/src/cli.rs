use clap::{ArgAction, Parser};
use workstation::{Completable, Completions};

/// The language server is normally started by the VS Code client with no
/// arguments; the flags here are for the terminal. `-v` is short for
/// `--version` rather than clap's default `-V`.
#[derive(Parser)]
#[command(
    name = "dot-code-server",
    version,
    disable_version_flag = true,
    about = "Language server behind the dot-code VS Code extension"
)]
pub struct Cli {
    // `ArgAction::Version` prints the version and exits while parsing, so the
    // field stays empty. `()` keeps the derived parser from reading a value back.
    #[arg(
        short = 'v',
        long = "version",
        action = ArgAction::Version,
        help = "Print the version and exit"
    )]
    version: (),

    #[command(flatten)]
    completions: Completions,
}

impl Completable for Cli {
    fn completions(&self) -> &Completions {
        &self.completions
    }
}
