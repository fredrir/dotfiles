//! Format `.conf`, `.config` and `.dotfile` files, and nothing else.
//!
//! One provider, a peer of stylua and shfmt rather than a driver of them.
//! `dotfmt <target>` formats a file or a tree, `--check` reports what is not
//! formatted and writes nothing, and `--stdin <filename>` lays out a body read
//! on stdin and hands the result back on stdout, which is how an editor asks.
//!
//! **stdout carries data and nothing else.** Every human-readable line goes to
//! stderr, in every mode, `--quiet` or not: conform.nvim replaces the buffer
//! with whatever stdout said, so a progress line there is a line in somebody's
//! file. On any `--stdin` failure nothing at all is written to stdout and the
//! status is 1, because conform discards the output of a run that failed and
//! the buffer is then left exactly as it was. Partial output is never emitted.
//!
//! Run with no arguments at all it prints its help and stops. A formatter that
//! rewrote the current directory when somebody typed its name to see what it
//! did would be a formatter people ran by accident.

mod block;
mod conf;
mod config;
mod native;
mod render;
mod walk;

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{CommandFactory, Parser, ValueHint};
use rayon::prelude::*;

use config::Config;
use native::{Done, Outcome};
use render::{Report, Tally};
use workstation::Completions;

const PROGRAM: &str = "dotfmt";

#[derive(Parser)]
#[command(
    version,
    about = "Format .conf, .config and .dotfile files",
    long_about = "Format .conf, .config and .dotfile files.

A target is a file or a directory; a directory is walked for the three \
extensions this tool owns and nothing else. --check reports what is not \
formatted, writes nothing, and exits 1 if it found anything. --stdin reads a \
body from stdin and writes the formatted bytes to stdout, so an editor can \
format a buffer that was never saved.

.conf and .config files are laid out in one of three modes chosen by the \
path: hypr, kitty, or plain, which trims trailing whitespace and leaves the \
structure alone. .dotfile files are laid out by their blocks, with the = of \
each group of entries sharing a column.

Everything a person reads is written to stderr, whatever the mode, so stdout \
is only ever the formatted body. Settings come from the nearest \
dotfile.dotfile at or above the target, then ~/.config/dotfmt/dotfile.dotfile, \
then the defaults built in.",
    after_long_help = "Examples:
  dotfmt .                       Format every file it owns below here
  dotfmt --check .               Report what is not formatted, and change nothing
  dotfmt --stdin hosts.dotfile   Format a body read on stdin, onto stdout"
)]
struct Cli {
    /// Files or directories to format
    #[arg(value_name = "TARGET", value_hint = ValueHint::AnyPath)]
    targets: Vec<PathBuf>,

    /// Report what is not formatted and write nothing
    #[arg(long)]
    check: bool,

    /// Format a body read on stdin as if it were this file, onto stdout
    #[arg(
        long,
        value_name = "FILENAME",
        value_hint = ValueHint::FilePath,
        conflicts_with_all = ["targets", "check"]
    )]
    stdin: Option<PathBuf>,

    /// Name every file, formatted or not
    #[arg(short, long, conflicts_with = "quiet")]
    verbose: bool,

    /// Say nothing that is not a failure
    #[arg(short, long)]
    quiet: bool,

    #[command(flatten)]
    completions: Completions,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Some(status) = cli.completions.emit::<Cli>(PROGRAM) {
        return status;
    }
    if let Some(name) = &cli.stdin {
        return through(name);
    }
    if cli.targets.is_empty() && !cli.check {
        // Nothing was asked, so answer with what there is to ask for.
        Cli::command().print_help().ok();
        return ExitCode::SUCCESS;
    }
    run(&cli)
}

/// `--stdin`: a body in, the formatted bytes out, and nothing on stdout at all
/// if any part of that failed.
///
/// The whole result is written in one call after every check has passed, so
/// there is no run that puts half a file on stdout and then reports a problem.
fn through(name: &Path) -> ExitCode {
    let mut raw = Vec::new();
    if let Err(error) = io::stdin().read_to_end(&mut raw) {
        return workstation::fail(PROGRAM, format!("stdin: {error}"));
    }
    let label = render::shorten(name);
    let Ok(text) = String::from_utf8(raw) else {
        return workstation::fail(PROGRAM, format!("{label}: not UTF-8"));
    };
    let config = match Config::resolve(&beside(name)) {
        Ok(config) => config,
        Err(message) => return workstation::fail(PROGRAM, message),
    };
    let formatted = match native::format(name, &label, &text, &config) {
        Ok(formatted) => formatted,
        Err(message) => return workstation::fail(PROGRAM, message),
    };
    match io::stdout().write_all(formatted.as_bytes()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => workstation::fail(PROGRAM, format!("stdout: {error}")),
    }
}

/// Format or check every target, and add up what happened.
///
/// A target that fails is reported and the rest still run. `format.py` aborts
/// the whole batch on the first odd path, which means one bad argument hides
/// every other file's answer.
fn run(cli: &Cli) -> ExitCode {
    let report = Report::new(cli.verbose, cli.quiet, cli.check);
    let mut tally = Tally::default();
    let targets: &[PathBuf] = if cli.targets.is_empty() {
        &[PathBuf::from(".")]
    } else {
        &cli.targets
    };
    for target in targets {
        format_target(target, cli, &report, &mut tally);
    }
    report.summary(&tally);
    if tally.failed > 0 || (cli.check && tally.changed > 0) {
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn format_target(target: &Path, cli: &Cli, report: &Report, tally: &mut Tally) {
    let files = match walk::gather(target) {
        Ok(files) => files,
        Err(message) => {
            report.failed(&message);
            tally.failed += 1;
            return;
        }
    };
    let config = match Config::resolve(&beside_target(target)) {
        Ok(config) => config,
        Err(message) => {
            report.failed(&message);
            tally.failed += 1;
            return;
        }
    };
    report.settings(config.source.as_deref());

    // Formatted in parallel and reported in order, so two runs over the same
    // tree read the same way round.
    let outcomes: Vec<(String, Result<Outcome, String>)> = files
        .par_iter()
        .map(|path| {
            let label = render::label(target, path);
            let done = native::apply(path, &label, &config, !cli.check);
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

/// The directory whose `dotfile.dotfile` governs a target.
fn beside_target(target: &Path) -> PathBuf {
    if target.is_dir() {
        return target.to_path_buf();
    }
    beside(target)
}

/// The directory a file sits in, as somewhere to start looking upward from.
fn beside(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

#[cfg(test)]
mod tests;
