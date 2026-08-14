//! Shared command-line plumbing for the workstation tools.
//!
//! Every binary in this workspace agrees on two things: a failure is one
//! `program: message` line on stderr and a non-zero status, and
//! `--completions <shell>` prints a completion script instead of doing the
//! tool's work. Both live here so a new tool inherits the conventions —
//! including the shell wiring in `shared/zsh/conf.d/55-completions.zsh`,
//! which assumes every tool answers the same flag.

use std::fmt::Display;
use std::io;
use std::process::ExitCode;

use clap::{Args, CommandFactory};
use clap_complete::Shell;

// The `--completions <SHELL>` flag, flattened into each tool's parser. A
// required positional has to opt out of being required when the flag is
// present, with `#[arg(required_unless_present = "shell")]`.
//
// Deliberately not a doc comment: clap hands a flattened struct's doc comment
// to the command it is flattened into, which would replace every tool's own
// `about` in `--help` with this.
#[derive(Args)]
pub struct Completions {
    /// Print shell completions and exit
    #[arg(long = "completions", value_name = "SHELL", exclusive = true)]
    pub shell: Option<Shell>,
}

impl Completions {
    /// `Some(status)` when the flag was given, for `main` to return straight
    /// away; `None` when the tool should get on with its actual work.
    pub fn emit<C: CommandFactory>(&self, program: &str) -> Option<ExitCode> {
        let shell = self.shell?;
        clap_complete::generate(shell, &mut C::command(), program, &mut io::stdout());
        Some(ExitCode::SUCCESS)
    }
}

/// Report a failure the way every tool here reports one.
pub fn fail(program: &str, message: impl Display) -> ExitCode {
    eprintln!("{program}: {message}");
    ExitCode::FAILURE
}
