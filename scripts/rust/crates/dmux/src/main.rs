//! One front door for terminal sessions on the two machines: wezterm-mux
//! workspaces and tmux sessions, local or on the peer, listed and attached
//! through the same verbs. This subsumes the old ssa/ssm shell functions.
//!
//! The transport policy is the point. A named session is always tmux —
//! sessions are a tmux concept — so `con` and `new` end in `exec tmux ...`,
//! over `ssh -t` when the host is remote, which replaces this process and
//! hands the terminal over cleanly. Only a bare attach of a remote host from
//! inside wezterm goes native instead: it spawns a tab in the peer's mux
//! domain, `<peer>-usb` when a TCP probe over the cable answers and
//! `<peer>-ts` otherwise. Both domains reach the same wezterm-mux-server, so
//! the choice is only a route — this mirrors `wez/remote/mux.lua`.
//!
//! `dmux -` toggles to the previous session, read from a per-host state file;
//! every con/new records the session it is leaving. A bare `-` is rewritten
//! before clap sees it, since clap cannot parse it as a subcommand, and an
//! unknown first word falls through to `con`, which only attaches sessions
//! that already exist — a typo cannot create anything.
//!
//! `DMUX_DRY_RUN=1` prints the command a verb would exec instead of running
//! it; it is how the tests, and a curious operator, inspect transport
//! selection.

mod attach;
mod doctor;
mod hosts;
mod keys;
mod list;
mod state;

use std::ffi::{OsStr, OsString};
use std::io::{self, IsTerminal, Write};
use std::process::{Command as Process, ExitCode, Stdio};

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::CompleteEnv;
use clap_complete::engine::{ArgValueCompleter, CompletionCandidate};
use workstation::{Completions, Style};

use hosts::{Context, Host};

const PROGRAM: &str = "dmux";

#[derive(Parser)]
#[command(
    name = "dmux",
    about = "Wezterm-mux and tmux sessions: list, attach, create",
    after_long_help = "Environment:
  DMUX_DRY_RUN=1  print the command a verb would exec instead of running it

Bare `dmux` opens a picker (or creates `main` when nothing runs); with --host
it attaches the peer the way the old ssa/ssm did. `dmux <name>` attaches an
existing session, and `dmux -` toggles back to the previous one."
)]
struct Cli {
    /// Host whose sessions to use (default: this machine)
    #[arg(short = 'H', long, global = true, value_enum)]
    host: Option<Host>,

    /// Print the version and exit
    #[arg(short = 'v', long = "version")]
    version: bool,

    #[command(flatten)]
    completions: Completions,

    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// List wezterm workspaces and tmux sessions
    Ls {
        /// Only tmux sessions
        #[arg(long)]
        tmux: bool,

        /// Only wezterm workspaces
        #[arg(long)]
        wez: bool,

        /// Machine-readable listing
        #[arg(long)]
        json: bool,

        #[arg(long, hide = true)]
        names: bool,
    },

    /// Attach an existing session ("continue")
    #[command(visible_aliases = ["attach", "a"])]
    Con {
        /// Session name, or an index from `dmux ls`
        #[arg(add = ArgValueCompleter::new(complete_sessions))]
        target: String,

        /// Window to select after attaching
        #[arg(short, long)]
        window: Option<String>,
    },

    /// Create a session if needed, then attach it
    New {
        /// Session name: letters, numbers, _ and -
        name: String,
    },

    /// Kill sessions, or one window of a session with -w
    #[command(visible_alias = "kill")]
    Rm {
        /// Session names or indices from `dmux ls`
        #[arg(required = true, add = ArgValueCompleter::new(complete_sessions))]
        targets: Vec<String>,

        /// Kill one window of the session instead
        #[arg(short, long)]
        window: Option<String>,

        /// Kill without asking
        #[arg(short, long)]
        yes: bool,
    },

    /// Rename a tmux session
    Rename {
        #[arg(add = ArgValueCompleter::new(complete_sessions))]
        old: String,

        #[arg(value_name = "NEW")]
        new_name: String,
    },

    /// Show the live wezterm and tmux key bindings
    Keys {
        /// Render as a man page
        #[arg(long)]
        man: bool,

        /// Only the tmux bindings
        #[arg(long)]
        tmux: bool,

        /// Only the wezterm bindings
        #[arg(long)]
        wez: bool,
    },

    /// Probe the environment transport selection depends on
    Doctor,

    #[command(external_subcommand)]
    Other(Vec<String>),
}

fn main() -> ExitCode {
    CompleteEnv::with_factory(Cli::command).complete();
    let cli = Cli::parse_from(normalized_args());
    if let Some(status) = cli.completions.emit::<Cli>(PROGRAM) {
        return status;
    }
    if cli.version {
        println!("{} (dmux)", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let context = match Context::resolve(cli.host) {
        Ok(context) => context,
        Err(message) => return workstation::fail(PROGRAM, message),
    };
    let outcome = match cli.command {
        None => bare(cli.host.is_some(), &context),
        Some(Cmd::Ls {
            tmux,
            wez,
            json,
            names,
        }) => list::run(&context, tmux, wez, json, names),
        Some(Cmd::Con { target, window }) => attach::con(&context, &target, window.as_deref()),
        Some(Cmd::New { name }) => attach::new_session(&context, &name),
        Some(Cmd::Rm {
            targets,
            window,
            yes,
        }) => attach::remove(&context, &targets, window.as_deref(), yes),
        Some(Cmd::Rename { old, new_name }) => attach::rename(&context, &old, &new_name),
        Some(Cmd::Keys { man, tmux, wez }) => keys::run(man, tmux, wez),
        Some(Cmd::Doctor) => Ok(doctor::run(&context)),
        Some(Cmd::Other(args)) => other(&context, &args),
    };
    match outcome {
        Ok(status) => status,
        Err(message) => workstation::fail(PROGRAM, message),
    }
}

/// clap refuses a bare `-` outright, so it is rewritten into a marker the
/// external-subcommand arm recognises; `@` keeps it out of the valid session
/// namespace.
fn normalized_args() -> Vec<OsString> {
    std::env::args_os()
        .map(|arg| if arg == "-" { "@prev".into() } else { arg })
        .collect()
}

fn other(context: &Context, args: &[String]) -> Result<ExitCode, String> {
    match args {
        [word] if word == "@prev" => attach::toggle(context),
        [target] => attach::con(context, target, None),
        _ => Err(format!("unexpected arguments: {}", args.join(" "))),
    }
}

fn bare(host_given: bool, context: &Context) -> Result<ExitCode, String> {
    if host_given {
        return attach::bare(context);
    }
    if !io::stdout().is_terminal() {
        return list::run(context, false, false, false, false);
    }
    let rows = list::gather(context, true, true)?;
    if rows.is_empty() {
        return attach::new_session(context, "main");
    }
    match pick(&rows)? {
        Some(index) => attach::attach_row(context, &rows[index], None),
        None => Ok(ExitCode::SUCCESS),
    }
}

/// fzf over the same lines `ls` prints when it is around, a numbered prompt
/// otherwise. No selection is not an error.
fn pick(rows: &[list::Row]) -> Result<Option<usize>, String> {
    let lines = list::render(rows, &Style::for_stdout());
    let chosen = if attach::on_path("fzf") {
        fzf_pick(&lines)?
    } else {
        number_pick(&lines)?
    };
    Ok(chosen
        .and_then(|index| index.checked_sub(1))
        .filter(|index| *index < rows.len()))
}

fn fzf_pick(lines: &[String]) -> Result<Option<usize>, String> {
    let mut child = Process::new("fzf")
        .args(["--ansi", "--height=~40%", "--reverse"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| format!("fzf: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(lines.join("\n").as_bytes());
        let _ = stdin.write_all(b"\n");
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("fzf: {error}"))?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(leading_index(&String::from_utf8_lossy(&output.stdout)))
}

fn number_pick(lines: &[String]) -> Result<Option<usize>, String> {
    for line in lines {
        println!("{line}");
    }
    print!("attach: ");
    io::stdout().flush().ok();
    let mut answer = String::new();
    match io::stdin().read_line(&mut answer) {
        Ok(0) | Err(_) => {
            println!();
            Ok(None)
        }
        Ok(_) => Ok(leading_index(&answer)),
    }
}

fn leading_index(text: &str) -> Option<usize> {
    let digits: String = text
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// Live tmux session names for dynamic shell completion. Empty on any
/// failure: completion must never hang or complain.
fn complete_sessions(current: &OsStr) -> Vec<CompletionCandidate> {
    let Some(prefix) = current.to_str() else {
        return Vec::new();
    };
    list::completion_names()
        .into_iter()
        .filter(|name| name.starts_with(prefix))
        .map(CompletionCandidate::new)
        .collect()
}
