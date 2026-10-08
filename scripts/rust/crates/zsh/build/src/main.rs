#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueHint};
use workstation::{Completable, Completions};
use zsh_build::{Options, build, default_root, origins};

const PROGRAM: &str = "zsh-build";

#[derive(Parser)]
#[command(version, about = "Compile the zsh startup config into one bundle")]
struct Cli {
    #[arg(short = 'n', long)]
    dry_run: bool,

    #[arg(long, global = true, value_hint = ValueHint::DirPath)]
    root: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,

    #[command(flatten)]
    completions: Completions,
}

#[derive(Subcommand)]
enum Command {
    /// Print where bundled shell functions were defined
    Where {
        #[arg(required = true)]
        names: Vec<String>,
    },
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
    if let Some(Command::Where { names }) = cli.command {
        return locate(&root, &names);
    }
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
        let system = match target.system {
            Some(true) => "  system compiled",
            Some(false) => "  system left to zsh",
            None => "",
        };
        println!(
            "{}  {}  {status}  {} inlined  {} folded{system}",
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

fn locate(root: &std::path::Path, names: &[String]) -> Result<ExitCode, String> {
    let origins = origins(root)?;
    let mut code = ExitCode::SUCCESS;
    for name in names {
        match origins.get(name) {
            Some(location) => println!("{name}  {location}"),
            None => {
                eprintln!("{PROGRAM}: {name}: not a bundled function");
                code = ExitCode::FAILURE;
            }
        }
    }
    Ok(code)
}
