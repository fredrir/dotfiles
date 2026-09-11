use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand, ValueHint};
use workstation::{Completable, Completions, Style};

use crate::bios::{check, diff, export, live, spec};
use crate::env::Sysfs;
use crate::hwmon::{self, Hwmon};
use crate::paths::Paths;
use crate::rows;
use crate::stress::cpu::CpuOptions;
use crate::stress::gpu::{GpuOptions, GpuTool};
use crate::stress::mem::{MemOptions, MemTool};
use crate::stress::monitor::Monitor;
use crate::stress::{self, Profile};
use crate::table;
use crate::time;
use crate::{bench, status};

pub const PROGRAM: &str = "hwtune";

#[derive(Parser)]
#[command(
    name = "hwtune",
    version,
    about = "Hardware tuning, benchmarks, and stability tests"
)]
pub struct Cli {
    #[arg(
        long,
        global = true,
        value_name = "NAME",
        help = "Host name; default is the short hostname"
    )]
    pub host: Option<String>,

    #[command(subcommand)]
    pub command: Option<Command>,

    #[command(flatten)]
    pub completions: Completions,
}

impl Completable for Cli {
    fn completions(&self) -> &Completions {
        &self.completions
    }
}

pub struct BenchArgs(pub clap::ArgMatches);

impl Args for BenchArgs {
    fn augment_args(_: clap::Command) -> clap::Command {
        bench::command()
    }
    fn augment_args_for_update(command: clap::Command) -> clap::Command {
        Self::augment_args(command)
    }
}
impl FromArgMatches for BenchArgs {
    fn from_arg_matches(matches: &clap::ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self(matches.clone()))
    }
    fn update_from_arg_matches(&mut self, matches: &clap::ArgMatches) -> Result<(), clap::Error> {
        self.0 = matches.clone();
        Ok(())
    }
}

pub fn surface_document() -> workstation::surface::Document {
    use workstation::surface::{Command as SurfaceCommand, Completion};
    fn annotate(command: &mut SurfaceCommand) {
        let name = command.name().to_string();
        for parameter in &mut command.params {
            let source = match (name.as_str(), parameter.name.as_str()) {
                ("run", "host") | ("hwtune", "host") => Some("known-hosts"),
                (_, "host") => Some("bench-hosts"),
                (_, "target" | "left" | "right" | "before" | "after") => Some("runs"),
                ("trend", "metric") => Some("metrics"),
                _ => None,
            };
            if let Some(source) = source {
                parameter.completion = Some(Completion::Call {
                    source: source.into(),
                });
            }
            if parameter.name == "workdir" {
                parameter.completion = Some(Completion::Dirs);
            }
            if parameter.name == "only" {
                parameter.choices = bench::runner::FAMILIES.map(String::from).to_vec();
                parameter.delimiter = Some(',');
            }
        }
        for child in &mut command.children {
            annotate(child);
        }
    }
    let mut command = Cli::command();
    command.build();
    let mut document = workstation::surface::document(&command, PROGRAM);
    annotate(&mut document.command);
    document
}

pub fn entry() -> Result<ExitCode, String> {
    let cli: Cli = workstation::cli::parse();
    if cli.completions.dump {
        println!(
            "{}",
            serde_json::to_string(&surface_document()).map_err(|e| e.to_string())?
        );
        return Ok(ExitCode::SUCCESS);
    }
    if cli.completions.is_zsh() {
        print!(
            "{}",
            workstation::surface::zsh::script(&surface_document().command)
        );
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(code) = cli.completions.emit::<Cli>(PROGRAM) {
        return Ok(code);
    }
    run(cli)
}

#[derive(Subcommand)]
pub enum Command {
    #[command(about = "Import, compare, and verify BIOS setting exports")]
    Bios {
        #[command(subcommand)]
        command: BiosCommand,
    },
    #[command(about = "Show daemons, CPU policy, fans, GPU, and kernel error counts")]
    Status,
    #[command(about = "Run a stability test while sampling temperatures and fans")]
    Stress {
        #[command(subcommand)]
        command: StressCommand,
    },
    #[command(about = "Sample temperatures and fans for a while and print the peaks")]
    Sample {
        #[arg(long, default_value_t = 5, help = "Duration in minutes")]
        minutes: u64,
    },
    #[command(about = "Plan and apply measured hardware tuning")]
    Tune {
        #[command(subcommand)]
        command: crate::tune::Command,
    },
    #[command(about = "Measure this machine and compare runs over time")]
    Bench(BenchArgs),
    #[command(name = "__complete", hide = true)]
    Complete { source: String },
}

#[derive(Subcommand)]
pub enum BiosCommand {
    #[command(
        about = "Normalize an ASUS export into config/bios/exports and diff it against the previous one"
    )]
    Import {
        #[arg(value_hint = ValueHint::FilePath)]
        file: PathBuf,
        #[arg(
            long,
            value_name = "VERSION",
            help = "BIOS version; default is read from DMI"
        )]
        bios_version: Option<String>,
        #[arg(short = 'n', long, help = "Show the result without writing")]
        dry_run: bool,
    },
    #[command(about = "Diff two exports; default is the latest two")]
    Diff {
        #[arg(value_name = "A")]
        first: Option<String>,
        #[arg(value_name = "B")]
        second: Option<String>,
    },
    #[command(about = "Check the spec against the latest export and against the running system")]
    Check {
        #[arg(long, help = "Skip checks that read the running system")]
        no_live: bool,
    },
    #[command(about = "List imported exports")]
    List,
}

#[derive(Subcommand)]
pub enum StressCommand {
    #[command(about = "stress-ng on the CPU: all cores, light load, or one core at a time")]
    Cpu {
        #[arg(long, value_enum)]
        profile: Profile,
        #[arg(
            long,
            default_value_t = 10,
            help = "Minutes per run, or per core for per-core"
        )]
        minutes: u64,
        #[arg(
            long,
            value_name = "LIST",
            help = "Cores for per-core, e.g. 0-7 or 0,2,4"
        )]
        cores: Option<String>,
        #[arg(
            long,
            allow_hyphen_values = true,
            help = "Curve Optimizer offset under test, for the log"
        )]
        offset: Option<i32>,
        #[arg(long, help = "Do not append to the stability log")]
        no_log: bool,
    },
    #[command(about = "Memory verification with stress-ng or memtester")]
    Mem {
        #[arg(long, default_value_t = 30)]
        minutes: u64,
        #[arg(long, default_value_t = 80, help = "Share of available memory to test")]
        percent: u8,
        #[arg(long, value_enum, default_value_t = MemTool::StressNg)]
        tool: MemTool,
        #[arg(long, help = "Do not append to the stability log")]
        no_log: bool,
    },
    #[command(about = "Loop a GPU benchmark and watch for Xid errors")]
    Gpu {
        #[arg(long, default_value_t = 15)]
        minutes: u64,
        #[arg(long, value_enum, default_value_t = GpuTool::Vkmark)]
        tool: GpuTool,
        #[arg(long, help = "Do not append to the stability log")]
        no_log: bool,
    },
}

pub fn run(cli: Cli) -> Result<ExitCode, String> {
    let style = Style::for_stdout();
    let sys = Sysfs::from_env();
    let host = cli.host.as_deref();
    let Some(command) = cli.command else {
        workstation::cli::command::<Cli>()
            .print_help()
            .map_err(|e| e.to_string())?;
        return Ok(ExitCode::SUCCESS);
    };
    match command {
        Command::Bios { command } => bios(command, &Paths::discover(host)?, &sys, &style),
        Command::Status => status::run(Paths::discover(host).ok().as_ref(), &sys, &style),
        Command::Stress { command } => {
            let paths = Paths::discover(host).ok();
            stress_command(command, paths.as_ref(), &sys, &style)
        }
        Command::Sample { minutes } => sample(minutes, &sys),
        Command::Tune { command } => crate::tune::run(command, host, &sys),
        Command::Bench(args) => {
            bench::run(&args.0)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Complete { source } => {
            let values = if source == "known-hosts" {
                bench::hosts::load_hosts().map(|hosts| {
                    hosts
                        .into_iter()
                        .map(|host| {
                            format!(
                                "{}:{}",
                                host.name,
                                host.role.split_whitespace().collect::<Vec<_>>().join(" ")
                            )
                        })
                        .collect()
                })
            } else {
                bench::complete(&source)
            };
            if let Ok(values) = values {
                for value in values {
                    println!("{value}");
                }
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn stress_command(
    command: StressCommand,
    paths: Option<&Paths>,
    sys: &Sysfs,
    style: &Style,
) -> Result<ExitCode, String> {
    let _measurement = bench::store::measurement_lock()?;
    let context = |no_log: bool| stress::Context {
        sys,
        paths,
        style,
        log: !no_log,
    };
    match command {
        StressCommand::Cpu {
            profile,
            minutes,
            cores,
            offset,
            no_log,
        } => stress::cpu::run(
            CpuOptions {
                profile,
                minutes,
                cores,
                offset,
            },
            &context(no_log),
        ),
        StressCommand::Mem {
            minutes,
            percent,
            tool,
            no_log,
        } => stress::mem::run(
            MemOptions {
                minutes,
                percent,
                tool,
            },
            &context(no_log),
        ),
        StressCommand::Gpu {
            minutes,
            tool,
            no_log,
        } => stress::gpu::run(GpuOptions { minutes, tool }, &context(no_log)),
    }
}

fn sample(minutes: u64, sys: &Sysfs) -> Result<ExitCode, String> {
    let session = time::session_id("sample");
    let mut monitor = Monitor::start(&session, sys, false)?;
    monitor.run(Duration::from_secs(minutes * 60), None)?;
    print!(
        "{}",
        table::render(&["peak", "value"], &monitor.peaks.rows())
    );
    println!(
        "\n  samples {}  journal {}  csv {}",
        monitor.peaks.samples,
        monitor.journal.summary(),
        monitor.csv_path().display()
    );
    Ok(ExitCode::SUCCESS)
}

fn bios(
    command: BiosCommand,
    paths: &Paths,
    sys: &Sysfs,
    style: &Style,
) -> Result<ExitCode, String> {
    match command {
        BiosCommand::Import {
            file,
            bios_version,
            dry_run,
        } => import(paths, sys, style, &file, bios_version, dry_run),
        BiosCommand::Diff { first, second } => diff_exports(paths, first, second),
        BiosCommand::Check { no_live } => check_spec(paths, sys, style, !no_live),
        BiosCommand::List => list_exports(paths),
    }
}

fn resolve_export(paths: &Paths, name: &str) -> Result<PathBuf, String> {
    let direct = PathBuf::from(name);
    if direct.is_file() {
        return Ok(direct);
    }
    let candidate = paths.exports_dir().join(name);
    if candidate.is_file() {
        return Ok(candidate);
    }
    let with_suffix = paths.exports_dir().join(format!("{name}.txt"));
    if with_suffix.is_file() {
        return Ok(with_suffix);
    }
    Err(format!("export not found: {name}"))
}

fn import(
    paths: &Paths,
    sys: &Sysfs,
    style: &Style,
    file: &Path,
    bios_version: Option<String>,
    dry_run: bool,
) -> Result<ExitCode, String> {
    let (text, parsed) = export::load(file)?;
    if parsed.settings.is_empty() {
        return Err(format!("{}: no `Name [Value]` lines found", file.display()));
    }
    let version = match bios_version {
        Some(version) => version,
        None => export::bios_version(sys)?,
    };
    let date = parsed.header_date().unwrap_or_else(time::today_compact);
    let target = paths
        .exports_dir()
        .join(export::file_name(&paths.host, &version, &date));
    let previous = export::latest(&paths.exports_dir(), &paths.host)?;
    println!(
        "{} settings, header {}, sha {}",
        parsed.settings.len(),
        parsed.header.as_deref().unwrap_or("none"),
        export::sha8(&text)
    );
    if let Some(previous) = &previous
        && previous != &target
    {
        let (before_text, before) = export::load(previous)?;
        let changes = export::changed(&before, &parsed);
        println!(
            "against {}: {} changed",
            previous
                .file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default(),
            changes.len()
        );
        if !changes.is_empty() {
            print!(
                "{}",
                table::render(
                    &["setting", "before", "after"],
                    &diff::summary(&changes, &before)
                )
            );
        }
        let _ = before_text;
    }
    if dry_run {
        println!("{} {}", style.dim("would write"), target.display());
        return Ok(ExitCode::SUCCESS);
    }
    if target.is_file() && fs::read_to_string(&target).map_err(|e| e.to_string())? == text {
        println!("{} {}", style.dim("unchanged"), target.display());
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    fs::write(&target, &text).map_err(|e| format!("{}: {e}", target.display()))?;
    println!("{} {}", style.green("wrote"), target.display());
    Ok(ExitCode::SUCCESS)
}

fn diff_exports(
    paths: &Paths,
    first: Option<String>,
    second: Option<String>,
) -> Result<ExitCode, String> {
    let (a, b) = match (first, second) {
        (Some(a), Some(b)) => (resolve_export(paths, &a)?, resolve_export(paths, &b)?),
        (Some(a), None) => {
            let latest =
                export::latest(&paths.exports_dir(), &paths.host)?.ok_or("no exports imported")?;
            (resolve_export(paths, &a)?, latest)
        }
        _ => {
            let mut all = export::list(&paths.exports_dir(), &paths.host)?;
            let b = all.pop().ok_or("no exports imported")?;
            let a = all.pop().ok_or("only one export imported")?;
            (a, b)
        }
    };
    let (text_a, export_a) = export::load(&a)?;
    let (text_b, export_b) = export::load(&b)?;
    let name = |path: &Path| {
        path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    };
    let changes = export::changed(&export_a, &export_b);
    println!("{} → {}: {} changed", name(&a), name(&b), changes.len());
    if !changes.is_empty() {
        print!(
            "{}",
            table::render(
                &["setting", "before", "after"],
                &diff::summary(&changes, &export_a)
            )
        );
        println!();
        print!(
            "{}",
            diff::unified(&text_a, &text_b, (&name(&a), &name(&b)))
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn check_spec(
    paths: &Paths,
    sys: &Sysfs,
    style: &Style,
    with_live: bool,
) -> Result<ExitCode, String> {
    let spec_file = paths.spec_file();
    let spec = spec::load(&spec_file)?;
    let latest = export::latest(&paths.exports_dir(), &paths.host)?.ok_or("no exports imported")?;
    let (_, export) = export::load(&latest)?;
    println!(
        "{} against {}",
        spec_file
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default(),
        latest
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default()
    );
    let mut all = check::compare(&spec, &export);
    if with_live {
        let chip = Hwmon::find(sys, hwmon::CHIP).ok();
        let rows = live::all(&spec, sys, chip.as_ref());
        if !rows.is_empty() {
            all.push(rows::Row::note("live", "running system"));
            all.extend(rows);
        }
    }
    print!("{}", rows::render(&all, style));
    let (ok, bad, warn) = rows::counts(&all);
    println!("  {ok} ok, {bad} bad, {warn} warn");
    Ok(rows::exit_code(&all))
}

fn list_exports(paths: &Paths) -> Result<ExitCode, String> {
    let rows = export::list(&paths.exports_dir(), &paths.host)?
        .into_iter()
        .map(|path| {
            let (text, parsed) = export::load(&path)?;
            Ok(vec![
                path.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
                parsed.header.unwrap_or_default(),
                parsed.settings.len().to_string(),
                export::sha8(&text),
            ])
        })
        .collect::<Result<Vec<_>, String>>()?;
    if rows.is_empty() {
        println!("no exports imported for {}", paths.host);
        return Ok(ExitCode::SUCCESS);
    }
    print!(
        "{}",
        table::render(&["export", "taken", "settings", "sha"], &rows)
    );
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
#[path = "../tests/unit/cli_tests.rs"]
mod tests;
