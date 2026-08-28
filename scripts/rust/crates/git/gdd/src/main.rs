//! Throw away every change: tracked files back to `HEAD`, untracked files
//! deleted.
//!
//! The plan is printed first, because half of it cannot be undone: a restored
//! file is still in `HEAD`, while a deleted untracked file is nowhere. Then
//! the question, and only then the discard.
//!
//! Nothing here shells out. The survey, the counts, the restore and the
//! deletions are all gitoxide, which is what makes the plan cheap enough to
//! always print — the old shell version spent a `git diff` process on every
//! untracked file just to fill in the line counts.

use std::process::ExitCode;

use clap::{Parser, ValueHint};
use gitkit::{Fate, Repo, View};
use gix::bstr::BString;
use workstation::{Completions, Style};

const PROGRAM: &str = "gdd";

/// Rows of a section to show before the rest are summed up.
const ROWS: usize = 12;

/// How wide to assume the terminal is when there is nobody to ask.
const WIDTH: usize = 100;

#[derive(Parser)]
#[command(
    version,
    about = "Discard every change in the working tree",
    long_about = "Discard every change in the working tree. Tracked files are restored to \
HEAD and untracked files are deleted. Without a path the whole repository is \
discarded; paths limit it to what they match.

Ignored files and nested repositories are kept.

The line counts are the diff against HEAD that would be thrown away. A \
restored file is still in HEAD; a deleted untracked file is nowhere.",
    after_long_help = "Examples:
  gdd                 Discard everything in the repository
  gdd -n              Show what that would be and stop
  gdd docs shared     Discard only what those paths match"
)]
struct Cli {
    /// Paths to discard; without one, the whole repository
    #[arg(value_name = "PATH", value_hint = ValueHint::AnyPath)]
    paths: Vec<String>,

    /// Show what would be discarded and stop
    #[arg(short = 'n', long = "dry-run")]
    dry_run: bool,

    /// List every entry instead of the first 12 of a section
    #[arg(short, long)]
    all: bool,

    /// Discard without asking
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
