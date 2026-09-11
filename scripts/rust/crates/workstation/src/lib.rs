use std::io;
use std::process::ExitCode;

use clap::{Args, CommandFactory};
use clap_complete::Shell;

pub mod blocks;
pub mod native;
pub mod path;
pub mod surface;
pub mod text;
pub mod units;
#[cfg(feature = "walk")]
pub mod walk;

pub use ui_cli as cli;
pub use ui_cli::{Answer, confirm, confirm_each, fail};
pub use ui_terminal::{terminal_height, terminal_width};
pub use ui_theme::{ColorMode, Style};

// The `--completions <SHELL>` flag, flattened into each tool's parser. A
// required positional has to opt out of being required when the flag is
// present, with `#[arg(required_unless_present = "shell")]`.
//
// Deliberately not a doc comment: clap hands a flattened struct's doc comment
// to the command it is flattened into, which would replace every tool's own
// `about` in `--help` with this.
#[derive(Args)]
pub struct Completions {
    #[arg(
        long = "completions",
        value_name = "SHELL",
        exclusive = true,
        help = "Generate shell completions"
    )]
    pub shell: Option<Shell>,

    #[arg(long = "command-dump", exclusive = true, hide = true)]
    pub dump: bool,
}

impl Completions {
    pub fn is_zsh(&self) -> bool {
        self.shell == Some(Shell::Zsh)
    }

    pub fn emit<C: CommandFactory>(&self, program: &str) -> Option<ExitCode> {
        if self.dump {
            let mut command = C::command();
            command.build();
            return Some(
                match serde_json::to_string(&surface::document(&command, program)) {
                    Ok(document) => {
                        println!("{document}");
                        ExitCode::SUCCESS
                    }
                    Err(error) => fail(program, error),
                },
            );
        }
        let shell = self.shell?;
        clap_complete::generate(shell, &mut C::command(), program, &mut io::stdout());
        Some(ExitCode::SUCCESS)
    }
}

pub fn exit_code(code: i32) -> ExitCode {
    ExitCode::from(exit_byte(code))
}

fn exit_byte(code: i32) -> u8 {
    u8::try_from(code).unwrap_or(1)
}

pub trait Completable {
    fn completions(&self) -> &Completions;
}

pub fn run<C>(program: &str, body: impl FnOnce(C) -> Result<ExitCode, String>) -> ExitCode
where
    C: clap::Parser + CommandFactory + Completable,
{
    let cli = ui_cli::parse::<C>();
    if let Some(status) = cli.completions().emit::<C>(program) {
        return status;
    }
    match body(cli) {
        Ok(status) => status,
        Err(message) => fail(program, message),
    }
}

#[cfg(test)]
#[path = "../tests/unit/lib_tests.rs"]
mod tests;
