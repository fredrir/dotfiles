#![forbid(unsafe_code)]

mod walk;

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, ValueHint};
use ui_batch::{Decision, Options, Row, Run};
use workstation::{Completable, Completions, Style, path, text};

const PROGRAM: &str = "rm-emptydirs";

#[derive(Parser)]
#[command(
    version,
    about = "Delete empty directories below the given targets",
    long_about = "Delete empty directories below the given targets.",
    after_long_help = "Examples:
  rm-emptydirs .                Delete all empty folders below here, after asking
  rm-emptydirs src --dry        Show what would go, and change nothing
  rm-emptydirs . -y             Delete without being asked
  rm-emptydirs . -y -a          Also delete folders the usual skip list holds"
)]
struct Cli {
    #[arg(value_name = "TARGET", value_hint = ValueHint::AnyPath)]
    targets: Vec<PathBuf>,

    #[arg(long)]
    dry: bool,

    #[arg(short, long)]
    yes: bool,

    #[arg(short, long)]
    verbose: bool,

    #[arg(short, long)]
    all: bool,

    #[command(flatten)]
    completions: Completions,
}

impl Completable for Cli {
    fn completions(&self) -> &Completions {
        &self.completions
    }
}

fn main() -> ExitCode {
    workstation::run::<Cli>(PROGRAM, |cli| {
        if cli.targets.is_empty() {
            workstation::cli::command::<Cli>().print_help().ok();
            println!();
            return Ok(ExitCode::SUCCESS);
        }
        let removed = run(&cli)?;
        Ok(if removed {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        })
    })
}

fn run(cli: &Cli) -> Result<bool, String> {
    let gathered = walk::gather(&cli.targets, cli.all)?;
    if gathered.unreadable > 0 {
        eprintln!(
            "{PROGRAM}: {} {} could not be read",
            gathered.unreadable,
            text::plural(gathered.unreadable, "directory", "directories")
        );
    }
    let style = Style::for_stdout();
    let labels: Vec<String> = cli.targets.iter().map(|target| path::shorten(target)).collect();
    let run = Run::new(
        PROGRAM,
        &labels,
        &style,
        Options {
            every_row: cli.verbose,
            dry: cli.dry,
            yes: cli.yes,
        },
    );

    let empty = gathered.empty;
    if empty.is_empty() {
        println!("{PROGRAM}: nothing to remove");
        return Ok(true);
    }

    // Display in path order; delete in the walk's post-order below.
    let mut listed: Vec<&PathBuf> = empty.iter().collect();
    listed.sort();
    let rows: Vec<Row> = listed
        .iter()
        .map(|path| Row::new(path::shorten(path)))
        .collect();
    run.heading();
    run.list(&rows);
    run.summary(&text::counted(empty.len(), "empty folder", "empty folders"));

    match run.decide() {
        Decision::Stop => return Ok(true),
        Decision::Interrupted => return Ok(false),
        Decision::Proceed => {}
    }

    // Post-order from the walk, so a directory is only reached once the
    // directories inside it are already gone.
    let failures = run.apply(&empty, |path| {
        fs::remove_dir(path).map_err(|error| format!("{}: {error}", path.display()))
    });
    Ok(failures == 0)
}
