use std::process::ExitCode;

use clap::Parser;
use gitkit::Repo;
use workstation::Completions;

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

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Some(status) = cli.completions.emit::<Cli>(PROGRAM) {
        return status;
    }
    match publish(&cli.message.join(" ")) {
        Ok(status) => status,
        Err(message) => workstation::fail(PROGRAM, message),
    }
}

fn publish(message: &str) -> gitkit::Result<ExitCode> {
    // `:/` is the pathspec for the working tree's top, so everything is
    // staged from the repository root no matter which subdirectory this runs
    // in — and outside a repository, git still gets to say so itself.
    let added = gitkit::git(&["add", ":/"])?;
    if added != 0 {
        return Ok(exit(added));
    }
    // Only now: staging is what git had to be in the repository to do, so a
    // repository that isn't there has already reported itself.
    if Repo::here()?.index_matches_head()? {
        return Ok(workstation::fail(PROGRAM, "nothing to commit"));
    }
    let committed = gitkit::git(&["commit", "-m", message])?;
    if committed != 0 {
        return Ok(exit(committed));
    }
    Ok(exit(gitkit::git(&["push"])?))
}

fn exit(code: i32) -> ExitCode {
    ExitCode::from(byte(code))
}

fn byte(code: i32) -> u8 {
    u8::try_from(code).unwrap_or(1)
}

#[cfg(test)]
#[path = "../tests/unit/main_tests.rs"]
mod tests;
