use crate::model::RenderOptions;
use crate::{collect, health, presentation, report, top};
use clap::{Arg, ArgAction, ArgGroup, ArgMatches, Args, Command, CommandFactory, value_parser};
use hostkit::Host;
use std::ffi::OsString;
use std::io::{IsTerminal, Write};
use std::time::Instant;

pub fn command() -> Command {
    workstation::Completions::augment_args(
        Command::new("sysinfo")
            .version(env!("CARGO_PKG_VERSION"))
            .about("Summarise the environment and hardware of this machine.")
            .arg(
                Arg::new("pretty")
                    .short('p')
                    .long("pretty")
                    .action(ArgAction::SetTrue)
                    .conflicts_with("json")
                    .help("Show the system dashboard"),
            )
            .arg(
                Arg::new("full")
                    .short('f')
                    .long("full")
                    .action(ArgAction::SetTrue)
                    .help("Include the extended inventory and detail view"),
            )
            .arg(
                Arg::new("health")
                    .long("health")
                    .action(ArgAction::SetTrue)
                    .help("Explain active errors and warnings (alias: -hh)"),
            )
            .arg(
                Arg::new("json")
                    .long("json")
                    .action(ArgAction::SetTrue)
                    .help("Emit a versioned normalized system report as JSON"),
            )
            .arg(
                Arg::new("timings")
                    .long("timings")
                    .action(ArgAction::SetTrue)
                    .help("Report probe timings to stderr"),
            )
            .arg(
                Arg::new("system")
                    .short('s')
                    .long("system")
                    .action(ArgAction::SetTrue)
                    .conflicts_with_all(["pretty", "full", "health"])
                    .help("Show the processes using the most resources"),
            )
            .arg(
                Arg::new("cpu")
                    .short('c')
                    .long("cpu")
                    .action(ArgAction::SetTrue)
                    .requires("system")
                    .help("Rank by CPU"),
            )
            .arg(
                Arg::new("memory")
                    .short('m')
                    .long("memory")
                    .action(ArgAction::SetTrue)
                    .requires("system")
                    .help("Rank by memory"),
            )
            .arg(
                Arg::new("gpu")
                    .short('g')
                    .long("gpu")
                    .action(ArgAction::SetTrue)
                    .requires("system")
                    .help("Rank by GPU"),
            )
            .group(ArgGroup::new("rank").args(["cpu", "memory", "gpu"]))
            .arg(
                Arg::new("number")
                    .short('n')
                    .long("number")
                    .value_name("N")
                    .value_parser(value_parser!(u16).range(1..))
                    .default_value("5")
                    .requires("system")
                    .help("Rows to show"),
            )
            .arg(
                Arg::new("target")
                    .short('t')
                    .long("target")
                    .value_name("HOST")
                    .value_parser(value_parser!(Host))
                    .requires("system")
                    .help("Host to inspect"),
            )
            .arg(
                Arg::new("split")
                    .long("split")
                    .action(ArgAction::SetTrue)
                    .requires("system")
                    .help("One row per process instead of per app"),
            ),
    )
}
struct Factory;
impl CommandFactory for Factory {
    fn command() -> Command {
        command()
    }
    fn command_for_update() -> Command {
        command()
    }
}

pub fn surface_document() -> workstation::surface::Document {
    use workstation::surface::Command as SurfaceCommand;
    fn annotate(command: &mut SurfaceCommand) {
        let name = command.name().to_string();
        for parameter in &mut command.params {
            if name == "sysinfo" && parameter.name == "health" {
                parameter.secondary.push("-hh".into());
            }
        }
        for child in &mut command.children {
            annotate(child);
        }
    }
    let mut cli = command();
    cli.build();
    let mut document = workstation::surface::document(&cli, "sysinfo");
    annotate(&mut document.command);
    document
}
pub fn normalize_arguments(arguments: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    let mut arguments = arguments.into_iter();
    let mut normalized = arguments.next().into_iter().collect::<Vec<_>>();
    let mut root_flags = true;
    let mut completion_shell = false;
    for argument in arguments {
        if completion_shell {
            completion_shell = false;
        } else if root_flags && argument == "-hh" {
            normalized.push(OsString::from("--health"));
            continue;
        } else if argument == "--completions" {
            completion_shell = true;
        } else if argument == "--" || !argument.as_encoded_bytes().starts_with(b"-") {
            root_flags = false;
        }
        normalized.push(argument);
    }
    normalized
}
pub fn run() -> Result<(), String> {
    let started = Instant::now();
    let matches = workstation::cli::decorate(command())
        .get_matches_from(normalize_arguments(std::env::args_os()));
    let completions = workstation::Completions {
        shell: matches.get_one::<clap_complete::Shell>("shell").copied(),
        dump: matches.get_flag("dump"),
    };
    if completions.dump {
        println!(
            "{}",
            serde_json::to_string(&surface_document()).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    if completions.is_zsh() {
        print!(
            "{}",
            workstation::surface::zsh::script(&surface_document().command)
        );
        return Ok(());
    }
    if completions.emit::<Factory>("sysinfo").is_some() {
        return Ok(());
    }
    let timings = matches.get_flag("timings");
    if matches.get_flag("system") {
        let output = processes(&matches, timings)?;
        write_stdout(&output)?;
        if timings {
            eprintln!("total: {:?}", started.elapsed());
        }
        return Ok(());
    }
    let options = RenderOptions {
        full: matches.get_flag("full"),
        health: matches.get_flag("health"),
    };
    // The dashboard renders gauges, disks, and health, so it skips the identity
    // probes and the enrichment only the detail views show.
    let scope = if options.full {
        collect::Scope::Full
    } else if matches.get_flag("pretty") {
        collect::Scope::Dashboard
    } else {
        collect::Scope::Summary
    };
    let snapshot = collect::collect_snapshot_with_timings(scope, timings);
    let view = presentation::build_view(&snapshot);
    let issues = health::health_issues(&snapshot);
    let output = if matches.get_flag("json") {
        serde_json::to_string_pretty(&serde_json::json!({"schema":1,"hardware":report::describe_hardware(&snapshot),"installation":report::describe_install(&snapshot),"system":view,"health":issues})).map_err(|e|e.to_string())?+"\n"
    } else if matches.get_flag("pretty") {
        presentation::render_pretty(&view, &issues, options)?
    } else {
        presentation::render_plain(&view, &issues, options)
    };
    write_stdout(&output)?;
    if timings {
        eprintln!("subprocess probes: {}", collect::probe_count());
        eprintln!("total: {:?}", started.elapsed());
    }
    Ok(())
}

fn write_stdout(output: &str) -> Result<(), String> {
    match std::io::stdout().lock().write_all(output.as_bytes()) {
        Err(error) if error.kind() != std::io::ErrorKind::BrokenPipe => Err(error.to_string()),
        _ => Ok(()),
    }
}

fn processes(matches: &ArgMatches, timings: bool) -> Result<String, String> {
    let sort = if matches.get_flag("cpu") {
        top::Sort::Cpu
    } else if matches.get_flag("memory") {
        top::Sort::Memory
    } else if matches.get_flag("gpu") {
        top::Sort::Gpu
    } else {
        top::Sort::Total
    };
    let options = top::Options {
        sort,
        count: matches
            .get_one::<u16>("number")
            .copied()
            .unwrap_or(5)
            .into(),
        split: matches.get_flag("split"),
    };
    let remote = matches
        .get_one::<Host>("target")
        .copied()
        .filter(|host| Host::this().ok() != Some(*host));
    let report = match remote {
        Some(host) => top::remote::fetch(host, options)?,
        None => {
            let sample = top::sample(top::WINDOW);
            if timings {
                eprintln!("window: {:?}", sample.window);
            }
            top::report(&sample, options)
        }
    };
    if sort == top::Sort::Gpu && !report.gpu {
        eprintln!("sysinfo: no per-process GPU data on {}", report.host);
    }
    if matches.get_flag("json") {
        return serde_json::to_string_pretty(&report)
            .map(|json| json + "\n")
            .map_err(|error| error.to_string());
    }
    let terminal = std::io::stdout().is_terminal();
    let width = terminal.then(workstation::terminal_width).flatten();
    Ok(top::render::render(
        &report,
        sort,
        &workstation::Style::for_stdout(),
        width,
    ))
}
