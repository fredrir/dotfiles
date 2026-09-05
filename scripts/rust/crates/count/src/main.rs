use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, ValueHint};
use workstation::walk::{self, Policy, Walked};
use workstation::{Completable, Completions, path, text};

const PROGRAM: &str = "count";

#[derive(Parser)]
#[command(version, about = "Count items inside a directory")]
struct Cli {
    #[arg(value_hint = ValueHint::DirPath, required_unless_present = "shell")]
    directory: Option<PathBuf>,

    #[arg(short = 'r', long = "recursive")]
    recursive: bool,

    #[arg(short = 'd', long = "no-hidden")]
    no_hidden: bool,

    #[command(flatten)]
    completions: Completions,
}

impl Completable for Cli {
    fn completions(&self) -> &Completions {
        &self.completions
    }
}

fn main() -> ExitCode {
    workstation::run::<Cli>(PROGRAM, |cli| {
        let Some(directory) = cli.directory.as_deref() else {
            return Err("missing directory".to_string());
        };
        path::require_directory(directory)?;
        let policy = counting(cli.no_hidden);

        if cli.recursive {
            let walked = descendants(directory, &policy);
            println!("{}", walked.items.len());
            if walked.unreadable > 0 {
                eprintln!(
                    "{PROGRAM}: {} {} could not be read",
                    walked.unreadable,
                    text::plural(walked.unreadable, "directory", "directories")
                );
            }
        } else {
            println!("{}", walk::list(directory, &policy)?.len());
        }
        Ok(ExitCode::SUCCESS)
    })
}

fn counting(no_hidden: bool) -> Policy {
    Policy::new().skipping(&[]).skip_hidden(no_hidden)
}

fn descendants(directory: &Path, policy: &Policy) -> Walked<()> {
    walk::walk(directory, policy, |_, entries| {
        entries.iter().map(|_| ()).collect()
    })
}

#[cfg(test)]
#[path = "../tests/unit/main_tests.rs"]
mod tests;
