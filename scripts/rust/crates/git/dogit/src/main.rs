#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use workstation::{Completable, Completions};

#[derive(Parser)]
#[command(version, about = "A deliberate Git workbench")]
struct Cli {
    #[arg(short = 'C', long, global = true, default_value = ".")]
    directory: PathBuf,
    #[command(subcommand)]
    command: Option<Start>,
    #[command(flatten)]
    completions: Completions,
}

#[derive(Subcommand)]
enum Start {
    /// Explore commits and their changes.
    Log,
    /// Compare two revisions.
    Compare { left: String, right: String },
}

impl Completable for Cli {
    fn completions(&self) -> &Completions {
        &self.completions
    }
}

fn main() -> ExitCode {
    workstation::run("dogit", |cli: Cli| {
        let start = match cli.command {
            Some(Start::Log) => dogit::app::Start::History,
            Some(Start::Compare { left, right }) => dogit::app::Start::Compare(left, right),
            None => dogit::app::Start::Changes,
        };
        dogit::app::run(&cli.directory, start)?;
        Ok(ExitCode::SUCCESS)
    })
}
