use clap::{ArgAction, Parser};
use workstation::{Completable, Completions};

#[derive(Parser)]
#[command(
    name = "dot-code-server",
    version,
    disable_version_flag = true,
    about = "Language server behind the dot-code VS Code extension"
)]
pub struct Cli {
    // `ArgAction::Version` exits while parsing, so the unit field never holds a value.
    #[arg(
        short = 'v',
        long = "version",
        action = ArgAction::Version,
        help = "Print the version and exit"
    )]
    version: (),

    // LSP clients pass `--stdio`; it is the only transport, so the flag changes nothing.
    #[arg(long, hide = true)]
    stdio: bool,

    #[command(flatten)]
    completions: Completions,
}

impl Completable for Cli {
    fn completions(&self) -> &Completions {
        &self.completions
    }
}
