#![forbid(unsafe_code)]

mod native;
mod report;
mod walk;

use std::collections::HashSet;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, ValueHint};
use luafmt::{
    config::{self, Config, Configs},
    dialect::Dialect,
    format,
};
use rayon::prelude::*;
use workstation::{Completable, Completions};

const PROGRAM: &str = "luafmt";
const WORKER_STACK_SIZE: usize = 32 * 1024 * 1024;

#[derive(Parser)]
#[command(
    version,
    about = "Format Lua with configurable, syntax-aware formatting"
)]
struct Cli {
    /// Files or directories to format
    #[arg(value_name = "TARGET", value_hint = ValueHint::AnyPath)]
    targets: Vec<PathBuf>,
    /// Emit only formatted Lua or errors.
    #[arg(short, long)]
    editor: bool,
    /// Lua dialect
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
    let allowed = config.files.allows(name);
    let output = if allowed {
        format(&input, &config)?
    } else {
        input.clone()
    };
    tally.total += usize::from(allowed);
    tally.changed += usize::from(output != input);
    if !cli.check {
        io::stdout()
            .write_all(output.as_bytes())
            .map_err(|e| format!("stdout: {e}"))?;
    }
    Ok(())
}

fn run(cli: &Cli) -> Result<ExitCode, String> {
    if cli.targets.len() > 1 || cli.targets.iter().any(|target| target.is_dir()) {
        // AST formatting is recursive. Rayon's default worker stacks overflow
        // on valid deeply nested Lua that fits on the main thread's stack.
        // Install walking and formatting together to reuse this one pool.
        return rayon::ThreadPoolBuilder::new()
            .stack_size(WORKER_STACK_SIZE)
            .build()
            .map_err(|error| format!("worker pool: {error}"))?
            .install(|| run_targets(cli));
    }
    // A single worker avoids pool startup for editors while providing the same
    // recursion headroom in debug builds and on platforms with small main stacks.
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name(PROGRAM.into())
            .stack_size(WORKER_STACK_SIZE)
            .spawn_scoped(scope, || run_targets(cli))
            .map_err(|error| format!("formatting worker: {error}"))?
            .join()
            .map_err(|_| "formatting worker panicked".to_string())?
    })
}

fn run_targets(cli: &Cli) -> Result<ExitCode, String> {
    let report = report::Report::new(
        PROGRAM,
        cli.verbose,
        cli.quiet || cli.editor,
        cli.check,
        report::Unreadable::Always,
    );
    let mut tally = report::Tally::default();
    let configs = Configs::new();
    let defaults = [PathBuf::from("-")];
    let targets = if cli.targets.is_empty() {
        &defaults[..]
    } else {
        &cli.targets
    };
    let streamed = targets.iter().any(|target| target == Path::new("-"));
    let mut files = Vec::new();
    for target in targets {
        if target == Path::new("-") {
            if let Err(error) = through(
                cli.stdin.as_deref().unwrap_or(Path::new("stdin.lua")),
                cli,
                &report,
                &mut tally,
            ) {
                report.failed(&format!("stdin: {error}"));
                tally.failed += 1;
            }
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
        files.extend(
            gathered
                .files
                .into_iter()
                .map(|path| (report::label(target, &path), path)),
        );
    }
    // Overlapping targets and explicit symlinks must not race to replace the
    // same file. The first target supplies its label and configuration path.
    let mut seen = HashSet::new();
    files.retain(|(_, path)| {
        seen.insert(std::fs::canonicalize(path).unwrap_or_else(|_| path.clone()))
    });
    let apply = |(_, path): &(String, PathBuf)| {
        configs.for_file(path).and_then(|config| {
            if !config.files.allows(path) {
                return Ok(None);
            }
            native::apply(path, &config, cli.dialect, !cli.check)
                .map(|changed| Some((changed, config)))
        })
    };
    // Editor/single-file invocations avoid starting a worker pool. Multiple
    // explicit files share the same parallel batch as directory inputs.
    let outcomes: Vec<_> = if files.len() <= 1 {
        files.iter().map(apply).collect()
    } else {
        files.par_iter().map(apply).collect()
    };
    for ((label, _), outcome) in files.iter().zip(outcomes) {
        match outcome {
            Ok(None) => report.skipped(label),
            Ok(Some((changed, config))) => {
                tally.total += 1;
                if cli.verbose {
                    report.settings(config.source.as_deref());
                }
                if changed {
                    tally.changed += 1;
                    report.changed(label);
                } else {
                    report.unchanged(label);
                }
            }
            Err(error) => {
                tally.total += 1;
                tally.failed += 1;
                report.failed(&format!("{label}: {error}"));
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
