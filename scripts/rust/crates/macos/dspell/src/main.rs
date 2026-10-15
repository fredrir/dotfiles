#![cfg_attr(not(target_os = "macos"), forbid(unsafe_code))]

use std::process::ExitCode;

use clap::Parser;
use workstation::{Completable, Completions};

#[cfg(target_os = "macos")]
mod spelling;
#[cfg(target_os = "macos")]
mod typo;

const PROGRAM: &str = "dspell";

#[derive(Parser)]
#[command(
    version,
    about = "Find the misspelled word at or before the cursor and its guesses"
)]
struct Cli {
    #[arg(
        value_name = "BEFORE",
        required_unless_present = "shell",
        help = "Text before the cursor"
    )]
    before: Option<String>,

    #[arg(
        value_name = "AFTER",
        default_value = "",
        help = "Text after the cursor"
    )]
    after: String,

    #[arg(
        short,
        long = "lang",
        value_name = "LANGUAGE",
        value_delimiter = ',',
        default_values = ["en", "nb"],
        help = "Languages a typo must be misspelled in"
    )]
    languages: Vec<String>,

    #[command(flatten)]
    completions: Completions,
}

impl Completable for Cli {
    fn completions(&self) -> &Completions {
        &self.completions
    }
}

fn main() -> ExitCode {
    workstation::run(PROGRAM, run)
}

fn run(cli: Cli) -> Result<ExitCode, String> {
    let before = cli.before.unwrap_or_default();
    println!("{}", find(&before, &cli.after, &cli.languages)?);
    Ok(ExitCode::SUCCESS)
}

#[cfg(target_os = "macos")]
fn find(before: &str, after: &str, languages: &[String]) -> Result<String, String> {
    let typo = typo::find(before, after, languages)?;
    serde_json::to_string(&typo).map_err(|error| error.to_string())
}

#[cfg(not(target_os = "macos"))]
fn find(_: &str, _: &str, _: &[String]) -> Result<String, String> {
    Err("macOS only".into())
}
