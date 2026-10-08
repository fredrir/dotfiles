#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, ValueHint};
use workstation::{Completable, Completions};
use zsh_build::{Options, build, default_root};

const PROGRAM: &str = "zsh-build";

#[derive(Parser)]
#[command(version, about = "Compile the zsh startup config into one bundle")]
struct Cli {
    #[arg(short = 'n', long)]
    dry_run: bool,

    #[arg(long, value_hint = ValueHint::DirPath)]
    root: Option<PathBuf>,

    #[command(flatten)]
    completions: Completions,
}

impl Completable for Cli {
    fn completions(&self) -> &Completions {
        &self.completions
    }
}

fn main() -> ExitCode {
    workstation::run(PROGRAM, run)
}

fn run(cli: Cli) -> Result<ExitCode, String> {
    let root = cli
        .root
        .or_else(default_root)
        .ok_or("dotfiles root not found")?;
    let built = build(&Options {
        root,
        dry_run: cli.dry_run,
    })?;
    for target in built {
        let status = match (target.changed, cli.dry_run) {
            (false, _) => "current",
            (true, true) => "would update",
            (true, false) => "updated",
        };
        println!(
            "{}  {}  {status}  {} inlined  {} folded",
            target.name,
            target.path.display(),
            target.inlined,
            target.folded
        );
        for note in target.warnings.iter().chain(&target.skipped) {
            eprintln!("{PROGRAM}: {note}");
        }
    }
    Ok(ExitCode::SUCCESS)
}
