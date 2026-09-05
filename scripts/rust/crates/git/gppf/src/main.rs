use std::process::ExitCode;

use clap::Parser;
use gitkit::Repo;
use workstation::{Completable, Completions};

const PROGRAM: &str = "gppf";

#[derive(Parser)]
#[command(
    version,
    about = "Stage everything, commit with the given message, and push"
)]
struct Cli {
    #[arg(
        value_name = "MESSAGE",
        default_value = ".",
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    message: Vec<String>,

    #[command(flatten)]
    completions: Completions,
}

impl Completable for Cli {
    fn completions(&self) -> &Completions {
        &self.completions
    }
}

fn main() -> ExitCode {
    workstation::run(PROGRAM, |cli: Cli| publish(&cli.message.join(" ")))
}

fn publish(message: &str) -> Result<ExitCode, String> {
    // `:/` is the pathspec for the working tree's top, so everything is
    // staged from the repository root no matter which subdirectory this runs
    // in — and outside a repository, git still gets to say so itself.
    let added = gitkit::git(&["add", ":/"])?;
    if added != 0 {
        return Ok(workstation::exit_code(added));
    }
    // Only now: staging is what git had to be in the repository to do, so a
    // repository that isn't there has already reported itself.
    if Repo::here()?.index_matches_head()? {
        return Ok(workstation::fail(PROGRAM, "nothing to commit"));
    }
    let committed = gitkit::git(&["commit", "-m", message])?;
    if committed != 0 {
        return Ok(workstation::exit_code(committed));
    }
    Ok(workstation::exit_code(gitkit::git(&["push"])?))
}
