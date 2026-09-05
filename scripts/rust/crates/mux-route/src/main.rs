mod domain;
mod probe;
mod render;

use std::process::ExitCode;

use clap::Parser;
use hostkit::Host;
use workstation::{Completions, Style};

const PROGRAM: &str = "mux-route";

#[derive(Parser)]
#[command(
    version,
    about = "Print the wezterm mux domain for the best route to the peer"
)]
struct Cli {
    #[arg(value_name = "HOST")]
    host: Option<Host>,

    #[arg(short = 'l', long = "list")]
    list: bool,

    #[command(flatten)]
    completions: Completions,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Some(status) = cli.completions.emit::<Cli>(PROGRAM) {
        return status;
    }
    match route(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => workstation::fail(PROGRAM, message),
    }
}

fn route(cli: &Cli) -> Result<(), String> {
    let this = Host::this()?;
    let peer = domain::target(cli.host, this)?;
    let answers = probe::probe(this, peer)?;
    if cli.list {
        println!("{}", render::list(&Style::for_stdout(), peer, &answers));
        return Ok(());
    }
    println!("{}", probe::pick(peer, &answers)?);
    Ok(())
}
