#![cfg_attr(not(target_os = "macos"), forbid(unsafe_code))]

use std::process::ExitCode;

use clap::Parser;
use workstation::{Completable, Completions};

#[cfg(target_os = "macos")]
mod skylight;

const PROGRAM: &str = "dwin";

#[derive(Parser)]
#[command(
    version,
    about = "Focus a window without raising its app's other windows"
)]
struct Cli {
    #[arg(value_name = "WINDOW_ID", required_unless_present = "shell")]
    window: Option<u32>,

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
    focus(cli.window.unwrap_or_default())?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(target_os = "macos")]
fn focus(window: u32) -> Result<(), String> {
    skylight::focus(window)
}

#[cfg(not(target_os = "macos"))]
fn focus(_: u32) -> Result<(), String> {
    Err("macOS only".into())
}
