#![forbid(unsafe_code)]

mod client;
mod endpoint;
mod native;
mod osc52;
mod proto;
mod serve;
mod session;
mod state;
mod tls;

use std::io::{self, Read, Write};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use hostkit::Host;
use workstation::{Completable, Completions};

const PROGRAM: &str = "dclip";

pub const PEER_READ: Duration = Duration::from_secs(2);

#[derive(Parser)]
#[command(
    version,
    about = "Copy stdin to the clipboard; print it with -o",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[arg(short = 'o', long = "output", help = "Print the clipboard")]
    output: bool,

    #[command(subcommand)]
    command: Option<Command>,

    #[command(flatten)]
    completions: Completions,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Serve this clipboard to the peer")]
    Serve,
}

impl Completable for Cli {
    fn completions(&self) -> &Completions {
        &self.completions
    }
}

fn main() -> ExitCode {
    #[cfg(target_os = "linux")]
    if native::is_holder() {
        return match native::hold() {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => ExitCode::FAILURE,
        };
    }
    workstation::run::<Cli>(PROGRAM, |cli| run(&cli).map(|()| ExitCode::SUCCESS))
}

fn run(cli: &Cli) -> Result<(), String> {
    match cli.command {
        Some(Command::Serve) => serve::serve(Host::this()?),
        None if cli.output => paste(),
        None => copy(),
    }
}

fn copy() -> Result<(), String> {
    let mut input = Vec::new();
    io::stdin()
        .read_to_end(&mut input)
        .map_err(|error| format!("stdin: {error}"))?;
    let text = String::from_utf8(input).map_err(|_| "input is not UTF-8".to_string())?;
    let text = trimmed(&text);
    if native::available() {
        native::write(text)
    } else {
        osc52::copy(text)
    }
}

fn trimmed(text: &str) -> &str {
    text.strip_suffix('\n').unwrap_or(text)
}

fn paste() -> Result<(), String> {
    let text = if native::available() {
        native::read()?
    } else {
        remote()?
    };
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
        .or_else(|error| match error.kind() {
            io::ErrorKind::BrokenPipe => Ok(()),
            _ => Err(format!("stdout: {error}")),
        })
}

fn remote() -> Result<String, String> {
    let this = Host::this()?;
    let stamp = std::env::var("HWIRE_SESSION")
        .ok()
        .filter(|stamp| !stamp.is_empty())
        .ok_or_else(|| "no clipboard".to_string())?;
    let stamp = session::parse(&stamp, this)?;
    let cache = state::path();
    let cached = cache.as_deref().and_then(state::load);
    let preferred: Vec<_> = [Some(stamp.route), cached].into_iter().flatten().collect();
    let (route, text) = client::paste(stamp.origin, &preferred)?;
    if let Some(cache) = cache.filter(|_| cached != Some(route)) {
        let _ = state::save(&cache, route);
    }
    Ok(text)
}
