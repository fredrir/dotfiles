mod block;
mod conf;
mod config;
mod native;
mod render;
mod select;
mod walk;

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{CommandFactory, Parser, ValueHint};
use rayon::prelude::*;

use config::{Config, Configs};
use native::{Done, Outcome};
use render::{Report, Tally};
use walk::Found;
use workstation::path::home_relative;
use workstation::{Completable, Completions};

const PROGRAM: &str = "dotfmt";

#[derive(Parser)]
#[command(
    version,
    about = "Format .conf, .config and .dotfile files",
    long_about = "Format .conf, .config and .dotfile files",
    after_long_help = "Examples:
  dotfmt .                       Format every file it owns below here
  dotfmt --check .               Report what is not formatted, and change nothing
  dotfmt --stdin hosts.dotfile   Format a body read on stdin, onto stdout
  dotfmt --owns < paths          Answer with the paths it would format"
)]
struct Cli {
    #[arg(value_name = "TARGET", value_hint = ValueHint::AnyPath)]
    targets: Vec<PathBuf>,

    #[arg(long)]
    check: bool,

    #[arg(
        long,
        value_name = "FILENAME",
        value_hint = ValueHint::FilePath,
        conflicts_with_all = ["targets", "check"]
    )]
    stdin: Option<PathBuf>,

    #[arg(long, conflicts_with_all = ["targets", "check", "stdin"])]
    owns: bool,

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
        if let Some(name) = &cli.stdin {
            return through(name);
        }
        if cli.owns {
            return owned();
        }
        if cli.targets.is_empty() && !cli.check {
            // Nothing was asked, so answer with what there is to ask for.
            Cli::command().print_help().ok();
            return Ok(ExitCode::SUCCESS);
        }
        Ok(run(&cli))
    })
}

fn through(name: &Path) -> Result<ExitCode, String> {
    let raw = drained()?;
    let label = home_relative(name);
    let text = String::from_utf8(raw).map_err(|_| format!("{label}: not UTF-8"))?;
    let config = Config::resolve(&config::beside(name))?;
    let formatted = native::format(name, &label, &text, &config)?;
    io::stdout()
        .write_all(formatted.as_bytes())
        .map_err(|error| format!("stdout: {error}"))?;
    Ok(ExitCode::SUCCESS)
}

fn drained() -> Result<Vec<u8>, String> {
    let mut raw = Vec::new();
    io::stdin()
        .read_to_end(&mut raw)
        .map_err(|error| format!("stdin: {error}"))?;
    Ok(raw)
}

fn owned() -> Result<ExitCode, String> {
    let raw = drained()?;
    let text = String::from_utf8(raw).map_err(|_| "stdin: not UTF-8".to_string())?;
    let configs = Configs::new();
    let mut answer = String::new();
    let mut problems: Vec<String> = Vec::new();
    for candidate in text.split('\0').filter(|piece| !piece.is_empty()) {
        match configs.for_file(Path::new(candidate)) {
            // Answered from the path alone, so a candidate that is not there
            // is simply not owned rather than an error: the caller is asking
            // which paths dotfmt would take, not which it can read.
            Ok(config) => {
                if config.owns(Path::new(candidate)).is_some() {
                    answer.push_str(candidate);
                    answer.push('\0');
                }
            }
            Err(message) => problems.push(message),
        }
    }
    if !problems.is_empty() {
        problems.sort();
        problems.dedup();
        let report = Report::new(false, false, false);
        for message in &problems {
            report.failed(message);
        }
        return Ok(ExitCode::FAILURE);
    }
    io::stdout()
        .write_all(answer.as_bytes())
        .map_err(|error| format!("stdout: {error}"))?;
    Ok(ExitCode::SUCCESS)
}

fn run(cli: &Cli) -> ExitCode {
    let report = Report::new(cli.verbose, cli.quiet, cli.check);
    let mut tally = Tally::default();
    let targets: &[PathBuf] = if cli.targets.is_empty() {
        &[PathBuf::from(".")]
    } else {
        &cli.targets
    };
    // One cache across every target, so two targets in the same tree read the
    // same chain of configs once between them.
    let configs = Configs::new();
    for target in targets {
        format_target(target, cli, &configs, &report, &mut tally);
    }
    report.summary(&tally);
    if tally.failed > 0 || (cli.check && tally.changed > 0) {
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn format_target(target: &Path, cli: &Cli, configs: &Configs, report: &Report, tally: &mut Tally) {
    let gathered = match walk::gather(target, configs) {
        Ok(gathered) => gathered,
        Err(message) => {
            report.failed(&message);
            tally.failed += 1;
            return;
        }
    };
    for message in &gathered.problems {
        report.failed(message);
        tally.failed += 1;
    }
    report.unreadable(gathered.unreadable);
    if let Ok(config) = configs.for_directory(&beside_target(target)) {
        report.settings(config.source.as_deref());
    }

    // Formatted in parallel and reported in order, so two runs over the same
    // tree read the same way round. The config is asked for per file, which
    // costs one lookup in a map the walk has already filled.
    let outcomes: Vec<(String, Result<Outcome, String>)> = gathered
        .files
        .par_iter()
        .map(|Found { path, kind }| {
            let label = render::label(target, path);
            let done = configs
                .for_file(path)
                .and_then(|config| native::apply(path, &label, *kind, &config, !cli.check));
            (label, done)
        })
        .collect();

    tally.total += outcomes.len();
    for (label, outcome) in outcomes {
        match outcome {
            Ok(Outcome {
                done: Done::Changed,
                mode,
            }) => {
                tally.changed += 1;
                report.changed(&label, mode);
            }
            Ok(Outcome {
                done: Done::Unchanged,
                mode,
            }) => report.unchanged(&label, mode),
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
#[path = "../tests/unit/main_tests.rs"]
mod tests;
