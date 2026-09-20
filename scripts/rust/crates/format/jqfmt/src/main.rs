#![forbid(unsafe_code)]

mod commented;
mod config;
mod dialect;
mod native;
mod number;
mod parse;
mod render;
mod repair;
mod report;
mod value;
mod walk;

use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, ValueHint};
use rayon::prelude::*;

use config::Configs;
use dialect::Dialect;
use native::{Done, Outcome};
use report::{Report, Tally};
use workstation::{Completable, Completions};

const PROGRAM: &str = "jqfmt";

#[derive(Parser)]
#[command(
    version,
    about = "jq inspired json formatter",
    after_long_help = "Examples:
  jqfmt .                        Format JSON, JSONC and HuJSON files below here
  jqfmt --check .                Report what is not formatted, and change nothing
  jqfmt - < settings.json        Format a body read on stdin, onto stdout
  jqfmt --editor < settings.json Read it as well as it can be read, and say what it fixed"
)]
struct Cli {
    /// Files to format in place, directories to walk, or `-` for stdin
    #[arg(value_name = "TARGET", value_hint = ValueHint::AnyPath)]
    targets: Vec<PathBuf>,

    /// Convert to strict JSON, repairing comments, trailing commas, single quotes,
    /// unquoted keys, Python names, missing commas and a byte order mark
    #[arg(short, long)]
    editor: bool,

    /// Input dialect (auto detects file extensions; stdin defaults to JSON)
    #[arg(long, value_enum, default_value_t = Dialect::Auto)]
    dialect: Dialect,

    /// Report what is not formatted rather than writing it
    #[arg(long)]
    check: bool,

    #[arg(short, long, conflicts_with = "quiet")]
    verbose: bool,

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
        // Nothing named and nothing piped in: there is no body to format and no
        // reason to hold a terminal open waiting for one.
        if cli.targets.is_empty() && io::stdin().is_terminal() {
            workstation::cli::command::<Cli>().print_help().ok();
            return Ok(ExitCode::SUCCESS);
        }
        run(&cli)
    })
}

fn run(cli: &Cli) -> Result<ExitCode, String> {
    let report = Report::new(cli.verbose, cli.quiet, cli.check);
    let mut tally = Tally::default();
    let configs = Configs::new();
    let targets: Vec<PathBuf> = if cli.targets.is_empty() {
        vec![PathBuf::from("-")]
    } else {
        cli.targets.clone()
    };
    // An editor integration reads stderr as a complaint rather than a report,
    // so a run that streamed to stdout says nothing when there is nothing to
    // say. A run over files is read by a person, who is owed the total.
    let streamed = targets.iter().any(|target| target.as_os_str() == "-");

    for target in &targets {
        if target.as_os_str() == "-" {
            through(cli, &configs, &report, &mut tally)?;
        } else {
            format_target(target, cli, &configs, &report, &mut tally);
        }
    }

    if !streamed || cli.verbose {
        report.summary(&tally);
    }
    if tally.failed > 0 || (cli.check && tally.changed > 0) {
        return Ok(ExitCode::FAILURE);
    }
    Ok(ExitCode::SUCCESS)
}

/// stdin onto stdout, which is the shape every editor integration asks for and
/// the shape this had before there was a flag to add to it.
fn through(cli: &Cli, configs: &Configs, report: &Report, tally: &mut Tally) -> Result<(), String> {
    let mut raw = Vec::new();
    io::stdin()
        .read_to_end(&mut raw)
        .map_err(|error| format!("stdin: {error}"))?;
    let here = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let config = configs.for_directory(&here)?;
    report.settings(config.source.as_deref(), &config.warnings);

    tally.total += 1;
    match native::format(
        "stdin",
        &raw,
        &config,
        cli.editor,
        cli.dialect.resolve(Path::new("-")),
    ) {
        Ok(formatted) => {
            report.repaired("stdin", formatted.repairs);
            if formatted.text.as_bytes() != raw.as_slice() {
                tally.changed += 1;
            }
            if !cli.check {
                io::stdout()
                    .write_all(formatted.text.as_bytes())
                    .map_err(|error| format!("stdout: {error}"))?;
            }
        }
        Err(message) => {
            tally.failed += 1;
            report.failed(&message);
        }
    }
    Ok(())
}

fn format_target(target: &Path, cli: &Cli, configs: &Configs, report: &Report, tally: &mut Tally) {
    let gathered = match walk::gather(target) {
        Ok(gathered) => gathered,
        Err(message) => {
            report.failed(&message);
            tally.failed += 1;
            return;
        }
    };
    report.unreadable(gathered.unreadable);
    if let Ok(config) = configs.for_directory(&beside_target(target)) {
        report.settings(config.source.as_deref(), &config.warnings);
    }

    // Formatted in parallel and reported in order, so two runs over the same
    // tree read the same way round. The config is asked for per file, which
    // costs one lookup in a map the walk has already filled.
    let outcomes: Vec<(String, Result<Outcome, String>)> = gathered
        .files
        .par_iter()
        .map(|path| {
            let label = report::label(target, path);
            let done = configs.for_file(path).and_then(|config| {
                native::apply(
                    path,
                    &label,
                    &config,
                    cli.editor,
                    !cli.check,
                    cli.dialect.resolve(path),
                )
            });
            (label, done)
        })
        .collect();

    tally.total += outcomes.len();
    for (label, outcome) in outcomes {
        match outcome {
            Ok(Outcome {
                done: Done::Changed,
                repairs,
            }) => {
                tally.changed += 1;
                report.changed(&label);
                report.repaired(&label, repairs);
            }
            Ok(Outcome {
                done: Done::Unchanged,
                repairs,
            }) => {
                report.unchanged(&label);
                report.repaired(&label, repairs);
            }
            Err(message) => {
                tally.failed += 1;
                report.failed(&message);
            }
        }
    }
}

fn beside_target(target: &Path) -> PathBuf {
    if target.is_dir() {
        return target.to_path_buf();
    }
    config::beside(target)
}

#[cfg(test)]
#[path = "../tests/unit/config_tests.rs"]
mod config_tests;
#[cfg(test)]
#[path = "../tests/unit/native_tests.rs"]
mod native_tests;
#[cfg(test)]
#[path = "../tests/unit/number_tests.rs"]
mod number_tests;
#[cfg(test)]
#[path = "../tests/unit/parse_tests.rs"]
mod parse_tests;
#[cfg(test)]
#[path = "../tests/unit/render_tests.rs"]
mod render_tests;
#[cfg(test)]
#[path = "../tests/unit/walk_tests.rs"]
mod walk_tests;

#[cfg(test)]
#[path = "../tests/unit/commented_tests.rs"]
mod commented_tests;
