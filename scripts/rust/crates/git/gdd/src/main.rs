use std::process::ExitCode;

use clap::{Parser, ValueHint};
use gitkit::{Fate, Repo, View};
use gix::bstr::BString;
use workstation::{Completions, Style};

const PROGRAM: &str = "gdd";

const ROWS: usize = 12;

const WIDTH: usize = 100;

#[derive(Parser)]
#[command(
    version,
    about = "Discard every change in the working tree",
    long_about = "Discard every change in the working tree.",
    after_long_help = "Examples:
  gdd                 Discard everything in the repository
  gdd -n              Show what that would be and stop
  gdd docs shared     Discard only what those paths match"
)]
struct Cli {
    #[arg(value_name = "PATH", value_hint = ValueHint::AnyPath)]
    paths: Vec<String>,

    #[arg(short = 'n', long = "dry-run")]
    dry_run: bool,

    #[arg(short, long)]
    all: bool,

    #[arg(short, long)]
    yes: bool,

    #[command(flatten)]
    completions: Completions,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Some(status) = cli.completions.emit::<Cli>(PROGRAM) {
        return status;
    }
    match discard(&cli) {
        Ok(status) => status,
        Err(error) => workstation::fail(PROGRAM, error),
    }
}

fn discard(cli: &Cli) -> gitkit::Result<ExitCode> {
    let repo = Repo::here()?;
    let paths: Vec<BString> = cli.paths.iter().map(|path| path.as_str().into()).collect();
    let survey = repo.survey(&paths)?;
    if survey.is_empty() {
        println!("{PROGRAM}: nothing to discard");
        return Ok(ExitCode::SUCCESS);
    }

    let view = View {
        program: PROGRAM,
        style: Style::for_stdout(),
        rows: if cli.all { usize::MAX } else { ROWS },
        // A terminal too narrow to hold a row is not worth laying out for.
        width: workstation::terminal_width()
            .filter(|width| *width >= 40)
            .unwrap_or(WIDTH),
    };
    view.plan(&survey, &paths);

    if cli.dry_run {
        return Ok(ExitCode::SUCCESS);
    }
    // A plan of nothing but nested repositories has nothing to do.
    if survey.with(Fate::Keep).count() == survey.entries.len() {
        println!("{PROGRAM}: nothing to discard");
        return Ok(ExitCode::SUCCESS);
    }
    if !cli.yes {
        match workstation::confirm("Continue? [Y/n] ") {
            Some(true) => {}
            Some(false) => {
                println!("{PROGRAM}: cancelled");
                return Ok(ExitCode::SUCCESS);
            }
            // The answers ran out; leave the prompt's line and stop.
            None => {
                println!();
                return Ok(ExitCode::FAILURE);
            }
        }
    }

    repo.discard(&survey)?;
    println!("  {}", view.style.dim("done"));
    Ok(ExitCode::SUCCESS)
}
