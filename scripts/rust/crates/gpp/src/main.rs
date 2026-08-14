//! Stage everything, commit with the given message, and push.
//!
//! `gpp fix the parser` is `git add .`, then `git commit -m 'fix the parser'`,
//! then `git push`. Each step's output goes straight to the terminal and the
//! first failure ends the sequence, so a failed stage never commits and a
//! failed commit never pushes. The failing step's own status is what `gpp`
//! exits with, which keeps git's vocabulary — 128 for "not a repository" and
//! so on — intact for anything chained after it.
//!
//! The empty commit is the one case worth catching here: `git commit` with
//! nothing staged prints a status report and fails, which reads like an
//! error in the tool rather than an answer, so `git diff --cached --quiet`
//! asks first. It reports an empty index as 0 and a staged change as 1;
//! anything else is git failing, and that status is passed through too.

use std::process::{Command, ExitCode, ExitStatus};

use clap::Parser;
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

fn publish(message: &str) -> Result<ExitCode, String> {
    let added = code(git(&["add", "."])?);
    if added != 0 {
        return Ok(exit(added));
    }
    match code(git(&["diff", "--cached", "--quiet"])?) {
        0 => return Ok(workstation::fail(PROGRAM, "nothing to commit")),
        1 => {}
        failed => return Ok(exit(failed)),
    }
    let committed = code(git(&["commit", "-m", message])?);
    if committed != 0 {
        return Ok(exit(committed));
    }
    Ok(exit(code(git(&["push"])?)))
}

/// Errors here are git not starting at all, which is worth naming; git
/// failing once it has started reports itself.
fn git(arguments: &[&str]) -> Result<ExitStatus, String> {
    Command::new("git")
        .args(arguments)
        .status()
        .map_err(|error| format!("git {}: {error}", arguments[0]))
}

/// A step killed by a signal has no status of its own; call it a failure.
fn code(status: ExitStatus) -> i32 {
    status.code().unwrap_or(1)
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
