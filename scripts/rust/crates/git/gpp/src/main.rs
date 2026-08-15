//! Stage everything, commit with the given message, and push.
//!
//! `gpp fix the parser` is `git add .`, then `git commit -m 'fix the parser'`,
//! then `git push`. Each step's output goes straight to the terminal and the
//! first failure ends the sequence, so a failed stage never commits and a
//! failed commit never pushes. The failing step's own status is what `gpp`
//! exits with, which keeps git's vocabulary — 128 for "not a repository" and
//! so on — intact for anything chained after it.
//!
//! Those three stay with git: they are the steps that run hooks, sign, and
//! reach the network with the user's credentials, and none of that is worth
//! reimplementing. The one question between them is answered here. `git
//! commit` with nothing staged prints a status report and fails, which reads
//! like an error in the tool rather than an answer, so the index is asked
//! first — and since git leaves the tree its index would write in the index
//! itself, the usual answer is one comparison of two hashes rather than
//! another process.

use std::process::ExitCode;

use clap::Parser;
use gitkit::Repo;
use workstation::Completions;

const PROGRAM: &str = "gpp";

#[derive(Parser)]
#[command(
    version,
    about = "Stage everything, commit with the given message, and push"
)]
struct Cli {
    /// Commit message; the words are joined with spaces
    #[arg(
        value_name = "MESSAGE",
        required_unless_present = "shell",
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
    let added = gitkit::git(&["add", "."])?;
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

/// A process status is a byte on the platforms this runs on; anything that
/// is not one came from somewhere unexpected and counts as a failure.
fn byte(code: i32) -> u8 {
    u8::try_from(code).unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_statuses_pass_through() {
        assert_eq!(byte(0), 0);
        assert_eq!(byte(1), 1);
        assert_eq!(byte(128), 128);
    }

    #[test]
    fn statuses_outside_a_byte_still_fail() {
        assert_eq!(byte(-1), 1);
        assert_eq!(byte(300), 1);
    }
}
