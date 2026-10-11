#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueHint};
use similar::TextDiff;
use workstation::{Completable, Completions};
use zsh_build::verify::{self, compare};
use zsh_build::{Options, build, default_root, origins};

const PROGRAM: &str = "zsh-build";

/// Characters of each value `verify` shows.
const EXCERPT: usize = 60;

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
    /// Compare a shell started from the bundle with one started from the sources
    Verify {
        /// Show differing values in full
        #[arg(short, long)]
        verbose: bool,

        /// Source the bundle in place of ~/.zshrc
        #[arg(short, long)]
        direct: bool,
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
    match cli.command {
        Some(Command::Where { names }) => return locate(&root, &names),
        Some(Command::Verify { verbose, direct }) => return check(&root, verbose, direct),
        None => {}
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
            "{}  {}  {status}  {} inlined  {} folded  {} deferred{system}",
            target.name,
            target.path.display(),
            target.inlined,
            target.folded,
            target.deferred
        );
        for dump in &target.refreshed {
            println!("{}  completion dump refreshed", dump.display());
        }
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

fn check(root: &std::path::Path, verbose: bool, direct: bool) -> Result<ExitCode, String> {
    let verified = verify::verify(root, direct)?;
    for bundle in &verified.stale {
        eprintln!("{PROGRAM}: {}: stale", bundle.display());
    }
    if verified.differences.is_empty() {
        println!("bundle matches sources");
        return Ok(ExitCode::SUCCESS);
    }
    let mut table = None;
    for difference in &verified.differences {
        if table != Some(&difference.table) {
            println!("{}", difference.table);
            table = Some(&difference.table);
        }
        if verbose {
            println!("  {}", difference.name);
            print_full(difference);
        } else {
            let (bundle, sources) = difference.excerpt(EXCERPT);
            println!(
                "  {}  bundle: {bundle}  sources: {sources}",
                difference.name
            );
        }
    }
    Ok(ExitCode::FAILURE)
}

fn print_full(difference: &compare::Difference) {
    match (&difference.bundle, &difference.sources) {
        (Some(bundle), Some(sources)) if bundle.contains('\n') || sources.contains('\n') => {
            let diff = TextDiff::from_lines(bundle, sources);
            for line in diff
                .unified_diff()
                .header("bundle", "sources")
                .to_string()
                .lines()
            {
                println!("    {}", compare::visible(line));
            }
        }
        (bundle, sources) => {
            let full = |value: &Option<String>| {
                value
                    .as_deref()
                    .map_or("(none)".to_string(), compare::visible)
            };
            println!("    bundle: {}", full(bundle));
            println!("    sources: {}", full(sources));
        }
    }
}
