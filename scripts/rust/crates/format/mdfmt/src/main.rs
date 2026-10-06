#![forbid(unsafe_code)]

mod native;
mod report;
mod walk;

use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, ValueHint};
use mdfmt::{
    config::{self, Config, Configs},
    format,
};
use rayon::prelude::*;
use workstation::{Completable, Completions};

const PROGRAM: &str = "mdfmt";

#[derive(Parser)]
#[command(
    version,
    about = "Format Markdown with compact tables and consistent spacing"
)]
struct Cli {
    /// Files or directories to format; - reads standard input.
    #[arg(value_name = "TARGET", value_hint = ValueHint::AnyPath)]
    targets: Vec<PathBuf>,
    /// Report formatting differences without writing files.
    #[arg(long)]
    check: bool,
    /// Read standard input using the configuration beside this filename.
    #[arg(long, value_name = "FILENAME", conflicts_with = "targets")]
    stdin: Option<PathBuf>,
    /// Name unchanged files and the configuration in use.
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
        if let Some(name) = &cli.stdin {
            return through(name, cli.check);
        }
        if cli.targets.iter().any(|p| p == Path::new("-")) {
            if cli.targets.len() != 1 {
                return Err("standard input cannot be combined with file targets".into());
            }
            return through(Path::new("stdin.md"), cli.check);
        }
        if cli.targets.is_empty() && !cli.check {
            if !io::stdin().is_terminal() {
                return through(Path::new("stdin.md"), false);
            }
            workstation::cli::command::<Cli>()
                .print_help()
                .map_err(|e| e.to_string())?;
            return Ok(ExitCode::SUCCESS);
        }
        Ok(run(&cli))
    })
}

fn through(name: &Path, check: bool) -> Result<ExitCode, String> {
    let config = Config::resolve(&config::beside(name))?;
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| format!("stdin: {e}"))?;
    let output = format(&input, &config)?;
    if check {
        return Ok(if output == input {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        });
    }
    io::stdout()
        .write_all(output.as_bytes())
        .map_err(|e| format!("stdout: {e}"))?;
    Ok(ExitCode::SUCCESS)
}

fn run(cli: &Cli) -> ExitCode {
    let report = report::Report::new(cli.verbose, cli.quiet, cli.check);
    let mut tally = report::Tally::default();
    let configs = Configs::new();
    let defaults = [PathBuf::from(".")];
    let targets = if cli.targets.is_empty() {
        &defaults[..]
    } else {
        &cli.targets
    };
    for target in targets {
        let gathered = match walk::gather(target) {
            Ok(gathered) => gathered,
            Err(error) => {
                report.failed(&error);
                tally.failed += 1;
                continue;
            }
        };
        report.unreadable(gathered.unreadable);
        tally.failed += gathered.unreadable;
        let outcomes: Vec<_> = gathered
            .files
            .par_iter()
            .map(|path| {
                let label = report::label(target, path);
                let outcome = configs.for_file(path).and_then(|config| {
                    native::apply(path, &config, !cli.check).map(|changed| (changed, config))
                });
                (label, outcome)
            })
            .collect();
        tally.total += outcomes.len();
        for (label, outcome) in outcomes {
            match outcome {
                Ok((changed, config)) => {
                    if cli.verbose {
                        report.settings(config.source.as_deref());
                    }
                    if changed {
                        tally.changed += 1;
                        report.changed(&label);
                    } else {
                        report.unchanged(&label);
                    }
                }
                Err(error) => {
                    tally.failed += 1;
                    report.failed(&format!("{label}: {error}"));
                }
            }
        }
    }
    report.summary(&tally);
    if tally.failed > 0 || (cli.check && tally.changed > 0) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
