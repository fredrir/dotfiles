#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, ValueHint};
use dotfmt::render;
use workstation::{Completable, Completions, Style};

const PROGRAM: &str = "dotfmt";

#[derive(Parser)]
#[command(version, about = "The one formatter to rule them all")]
struct Cli {
    #[arg(value_name = "TARGET", value_hint = ValueHint::AnyPath)]
    target: Option<PathBuf>,

    /// Check formatting without writing (placeholder).
    #[arg(long, conflicts_with_all = ["add", "sync"])]
    check: bool,

    /// Offer formatter configuration to the target (placeholder).
    #[arg(short = 'a', long, conflicts_with = "sync")]
    add: bool,

    /// Refresh the target's formatter configuration (placeholder).
    #[arg(short = 's', long)]
    sync: bool,

    /// Select a formatter dialect (placeholder).
    #[arg(long, value_name = "DIALECT")]
    dialect: Option<String>,

    /// Format standard input for an editor (placeholder).
    #[arg(short, long)]
    editor: bool,

    /// Treat standard input as the named file (placeholder).
    #[arg(long, value_name = "FILENAME", value_hint = ValueHint::FilePath, conflicts_with = "target")]
    stdin: Option<PathBuf>,

    /// Report owned files from standard input (placeholder).
    #[arg(long, conflicts_with_all = ["target", "check", "stdin", "editor", "add", "sync"])]
    owns: bool,

    /// Show detailed output.
    #[arg(short, long, conflicts_with = "quiet")]
    verbose: bool,

    /// Report only failures.
    #[arg(short, long)]
    quiet: bool,

    #[command(flatten)]
    completions: Completions,
}

impl Completable for Cli {
    fn completions(&self) -> &Completions {
        &self.completions
    }
}

fn main() -> ExitCode {
    workstation::run(PROGRAM, |cli: Cli| {
        if cli.target.is_none()
            && !cli.check
            && !cli.add
            && !cli.sync
            && cli.dialect.is_none()
            && !cli.editor
            && cli.stdin.is_none()
            && !cli.owns
        {
            workstation::cli::command::<Cli>()
                .print_help()
                .map_err(|error| error.to_string())?;
            println!();
            return Ok(ExitCode::SUCCESS);
        }

        if cli.verbose {
            let target = cli
                .target
                .as_deref()
                .or(cli.stdin.as_deref())
                .unwrap_or(Path::new("."));
            let action = if cli.check {
                "check"
            } else if cli.add {
                "add"
            } else if cli.sync {
                "sync"
            } else {
                ""
            };
            for line in render::heading(PROGRAM, target, action, &Style::for_stderr()) {
                eprintln!("{line}");
            }
        }
        Err("not implemented yet: formatting and configuration actions are placeholders".into())
    })
}
