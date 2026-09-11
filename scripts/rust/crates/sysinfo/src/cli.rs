use crate::model::RenderOptions;
use crate::{collect, health, presentation, report};
use clap::{Arg, ArgAction, Args, Command, CommandFactory};
use std::ffi::OsString;
use std::io::Write;
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
                    .help("Show the complete branded hardware presentation"),
            )
            .arg(
                Arg::new("full")
                    .short('f')
                    .long("full")
                    .action(ArgAction::SetTrue)
                    .help("Include the extended inventory"),
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
    let options = RenderOptions {
        full: matches.get_flag("full"),
        health: matches.get_flag("health"),
    };
    let timings = matches.get_flag("timings");
    let snapshot =
        collect::collect_snapshot_with_timings(options.full || matches.get_flag("pretty"), timings);
    let view = presentation::build_view(&snapshot);
    let issues = health::health_issues(&snapshot);
    let output = if matches.get_flag("json") {
        serde_json::to_string_pretty(&serde_json::json!({"schema":1,"hardware":report::describe_hardware(&snapshot),"installation":report::describe_install(&snapshot),"system":view,"health":issues})).map_err(|e|e.to_string())?+"\n"
    } else if matches.get_flag("pretty") {
        presentation::render_pretty(&view, &issues, options)?
    } else {
        presentation::render_plain(&view, &issues, options)
    };
    if let Err(error) = std::io::stdout().lock().write_all(output.as_bytes())
        && error.kind() != std::io::ErrorKind::BrokenPipe
    {
        return Err(error.to_string());
    }
    if timings {
        eprintln!("subprocess probes: {}", collect::probe_count());
        eprintln!("total: {:?}", started.elapsed());
    }
    Ok(())
}
