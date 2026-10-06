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
    dialect::Dialect,
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
    /// Read stdin for an editor and emit only formatted Markdown or errors.
    #[arg(short, long)]
    editor: bool,
    /// Markdown dialect (auto uses Obsidian in vaults, otherwise GFM).
    #[arg(long, value_enum, default_value_t = Dialect::Auto)]
    dialect: Dialect,
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
        if cli.editor && cli.targets.iter().any(|target| target != Path::new("-")) {
            return Err("--editor reads stdin; use --stdin FILENAME for per-file settings".into());
        }
        if cli.stdin.is_none() && !cli.editor && cli.targets.is_empty() && io::stdin().is_terminal()
        {
            workstation::cli::command::<Cli>()
                .print_help()
                .map_err(|e| e.to_string())?;
            return Ok(ExitCode::SUCCESS);
        }
        run(&cli)
    })
}

fn through(
    name: &Path,
    cli: &Cli,
    report: &report::Report,
    tally: &mut report::Tally,
) -> Result<(), String> {
    let mut config = Config::resolve(&config::beside(name))?;
    config.dialect = cli.dialect.resolve(config.dialect, name);
    report.settings(config.source.as_deref());
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| format!("stdin: {e}"))?;
    let output = format(&input, &config)?;
    tally.total += 1;
    tally.changed += usize::from(output != input);
    if !cli.check {
        io::stdout()
            .write_all(output.as_bytes())
            .map_err(|e| format!("stdout: {e}"))?;
    }
    Ok(())
}

fn run(cli: &Cli) -> Result<ExitCode, String> {
    let report = report::Report::new(cli.verbose, cli.quiet || cli.editor, cli.check);
    let mut tally = report::Tally::default();
    let configs = Configs::new();
    let defaults = [PathBuf::from("-")];
    let targets = if cli.targets.is_empty() {
        &defaults[..]
    } else {
        &cli.targets
    };
    let streamed = targets.iter().any(|target| target == Path::new("-"));
    for target in targets {
        if target == Path::new("-") {
            through(
                cli.stdin.as_deref().unwrap_or(Path::new("stdin.md")),
                cli,
                &report,
                &mut tally,
            )?;
            continue;
        }
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
                    native::apply(path, &config, cli.dialect, !cli.check)
                        .map(|changed| (changed, config))
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
    if !streamed || cli.verbose {
        report.summary(&tally);
    }
    if tally.failed > 0 || (cli.check && tally.changed > 0) {
        Ok(ExitCode::FAILURE)
    } else {
        Ok(ExitCode::SUCCESS)
    }
}
