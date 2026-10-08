#![forbid(unsafe_code)]

mod cache;
mod context;
mod help;
mod http;
mod line;
mod node;
mod pi;
mod process;
mod reply;
mod shadcn;
mod shared;

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use ui_theme::{ColorMode, Style};
use workstation::Completions;

use context::Context;
use line::Line;
use node::manager::{Manager, SpecSource};
use reply::Reply;

const PROGRAM: &str = "zcomp";
const SHIM: &str = include_str!("../assets/zcomp.zsh");

#[derive(Parser)]
#[command(
    version,
    about = "Completion candidates for node package managers, pi and shadcn, answered for zsh"
)]
struct Cli {
    #[command(subcommand)]
    action: Option<Action>,

    #[command(flatten)]
    completions: Completions,
}

#[derive(Subcommand)]
enum Action {
    #[command(about = "Print candidates for the word under the cursor")]
    Complete {
        #[arg(long, value_name = "COMMAND")]
        command: String,

        #[arg(
            long,
            value_name = "INDEX",
            help = "Position of the cursor word, counting from 1"
        )]
        current: usize,

        #[arg(
            long,
            allow_hyphen_values = true,
            help = "The cursor word up to the cursor"
        )]
        prefix: Option<String>,

        #[arg(long, help = "Color the candidates with the dotfile theme")]
        color: bool,

        #[arg(last = true, value_name = "WORDS")]
        words: Vec<String>,
    },

    #[command(about = "Rebuild one cached source")]
    Refresh {
        #[arg(value_name = "SOURCE")]
        source: String,

        #[arg(value_name = "ARGS")]
        args: Vec<String>,
    },

    #[command(about = "Rebuild missing or stale caches in the background")]
    Warm,
}

fn main() -> ExitCode {
    let cli = workstation::cli::parse::<Cli>();
    if let Some(status) = cli.completions.emit::<Cli>(PROGRAM) {
        if cli.completions.is_zsh() {
            print!("{}", shim());
        }
        return status;
    }
    match cli.action {
        Some(Action::Complete {
            command,
            current,
            prefix,
            color,
            words,
        }) => {
            let ctx = Context::from_env();
            let line = Line::new(words, current, prefix);
            let style = if color {
                Style::for_mode(ColorMode::Auto, true)
            } else {
                Style::plain()
            };
            print!("{}", complete(&ctx, &command, &line).render(&style));
            ExitCode::SUCCESS
        }
        Some(Action::Refresh { source, args }) => {
            let ctx = Context::from_env();
            if refresh(&ctx, &source, &args) {
                ExitCode::SUCCESS
            } else {
                workstation::fail(PROGRAM, format!("could not rebuild {source}"))
            }
        }
        Some(Action::Warm) => {
            warm(&Context::from_env());
            ExitCode::SUCCESS
        }
        None => workstation::fail(PROGRAM, "a subcommand is required; see --help"),
    }
}

fn complete(ctx: &Context, command: &str, line: &Line) -> Reply {
    let command = command.rsplit('/').next().unwrap_or(command);
    if command == "pi" {
        return pi::complete(ctx, line);
    }
    if command == "shadcn" {
        return shadcn::complete(ctx, line);
    }
    match Manager::from_command(command) {
        Some((manager, runner)) => node::complete(ctx, line, manager, runner),
        None => Reply::new(),
    }
}

fn refresh(ctx: &Context, source: &str, args: &[String]) -> bool {
    let pi = pi::spec::Pi::locate(ctx);
    match source {
        "node-spec" => {
            let manager = args
                .first()
                .and_then(|program| Manager::from_command(program))
                .map(|(manager, _)| manager);
            let Some(manager) = manager else { return false };
            let Some(binary) = manager.binary(ctx) else {
                return false;
            };
            cache::refresh(ctx, &SpecSource { manager, binary })
        }
        "node-popular" => cache::refresh(ctx, &node::popular::PopularSource),
        "pi-spec" => pi
            .binary
            .clone()
            .is_some_and(|binary| cache::refresh(ctx, &pi::spec::SpecSource { pi: &pi, binary })),
        "pi-models" => pi.binary.clone().is_some_and(|binary| {
            cache::refresh(ctx, &pi::models::ModelsSource { pi: &pi, binary })
        }),
        "pi-gallery" => cache::refresh(ctx, &pi::gallery::Gallery),
        "shadcn-spec" => ctx
            .which("shadcn")
            .is_some_and(|binary| cache::refresh(ctx, &shadcn::spec::SpecSource { binary })),
        _ => false,
    }
}

fn warm(ctx: &Context) {
    if let Some(binary) = ctx.which("shadcn") {
        cache::warm(ctx, &shadcn::spec::SpecSource { binary });
    }
    let mut any_manager = false;
    for manager in Manager::ALL {
        if let Some(binary) = manager.binary(ctx) {
            any_manager = true;
            cache::warm(ctx, &SpecSource { manager, binary });
        }
    }
    if any_manager {
        cache::warm(ctx, &node::popular::PopularSource);
    }
    let pi = pi::spec::Pi::locate(ctx);
    if let Some(binary) = pi.binary.clone() {
        cache::warm(
            ctx,
            &pi::spec::SpecSource {
                pi: &pi,
                binary: binary.clone(),
            },
        );
        cache::warm(ctx, &pi::models::ModelsSource { pi: &pi, binary });
        cache::warm(ctx, &pi::gallery::Gallery);
    }
}

fn shim() -> String {
    let commands: Vec<&str> = Manager::ALL
        .iter()
        .flat_map(|manager| manager.commands().iter().map(|(name, _)| *name))
        .chain(["pi", "shadcn"])
        .collect();
    SHIM.replace("{{COMMANDS}}", &commands.join(" "))
}

#[cfg(test)]
#[path = "../tests/unit/main_tests.rs"]
mod tests;
