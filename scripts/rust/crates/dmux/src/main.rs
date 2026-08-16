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
mod space_cli;
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
existing session (a trailing `-w N` picks its window), and `dmux -` toggles
back to the previous one."
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
    #[command(visible_alias = "list")]
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

        #[arg(long, hide = true, conflicts_with = "json")]
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

        /// Create the session like `dmux new` when it does not exist
        #[arg(short = 'A', long = "create")]
        create: bool,
    },

    /// Create a session if needed, then attach it
    New {
        /// Session name: letters, numbers, _ and -
        name: String,

        /// Working directory for the new session
        #[arg(long, value_name = "PATH")]
        dir: Option<String>,

        /// Command to run in the new session, after `--`
        #[arg(last = true, value_name = "CMD")]
        command: Vec<String>,
    },

    /// Detach the current client from its tmux session
    Detach,

    /// Kill sessions, or one window of a session with -w
    #[command(visible_aliases = ["kill", "delete"])]
    Rm {
        /// Session names or indices from `dmux ls`
        #[arg(
            required_unless_present = "all",
            conflicts_with = "all",
            add = ArgValueCompleter::new(complete_sessions)
        )]
        targets: Vec<String>,

        /// Kill every tmux session listed (keeps the one this client is in)
        #[arg(long, conflicts_with = "window")]
        all: bool,

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
    Doctor {
        /// Machine-readable report
        #[arg(long)]
        json: bool,
    },

    /// Internal: keep the reserved mux sentinel pane alive (plan §15.1).
    /// Never a Space, Group, or Split; excluded from all listings.
    #[command(name = "_mux-idle", hide = true)]
    MuxIdle,

    /// Internal: stamp a fresh server epoch on a managed tmux server and
    /// publish the binding (plan §11.2). Invoked by the managed-server
    /// session-created hook or by explicit adoption; `ls` never runs this.
    #[command(name = "_tmux-bootstrap", hide = true)]
    TmuxBootstrap {
        /// Managed `-L` namespace; inferred from $TMUX inside the server.
        #[arg(long)]
        namespace: Option<String>,

        /// Test seam: directory holding registry.sqlite3.
        #[arg(long, hide = true)]
        data_dir: Option<String>,

        /// Test seam: kernel-lock directory.
        #[arg(long, hide = true)]
        lock_dir: Option<String>,
    },

    /// Groups (tabs/windows) of a managed Space
    Group {
        #[command(subcommand)]
        cmd: space_cli::GroupCmd,
    },

    /// Splits (panes) of a managed Space
    Split {
        #[command(subcommand)]
        cmd: space_cli::SplitCmd,
    },

    /// Pane marker context for adopted Spaces (plan §10.3)
    Context {
        #[command(subcommand)]
        cmd: space_cli::ContextCmd,
    },

    /// Enroll a host over SSH and open an interactive session (plan §12.2)
    Ssh { target: String },

    /// Enrolled hosts and their routes (plan §7.3)
    #[command(name = "host")]
    HostAdmin {
        #[command(subcommand)]
        cmd: space_cli::HostCmd,
    },

    /// Internal: revalidate the invoking pane's markers (plan §13.1). Reads
    /// DMUX_SPACE_UID plus TMUX_PANE/WEZTERM_PANE from the environment and
    /// prints one validated marker JSON document; any mismatch is a typed
    /// error and no marker (never a guess).
    #[command(name = "_context", hide = true)]
    ContextInternal {
        /// Test seam: directory holding registry.sqlite3.
        #[arg(long, hide = true)]
        data_dir: Option<String>,

        /// Test seam: kernel-lock directory.
        #[arg(long, hide = true)]
        lock_dir: Option<String>,
    },

    /// Internal: owner-agent RPC endpoint (plan §12.1). One JSON request
    /// envelope on stdin, one response envelope on stdout, typed exit.
    #[command(name = "_agent", hide = true)]
    Agent {
        /// Exact protocol version the caller speaks; v1 requires a match.
        #[arg(long)]
        protocol: u32,

        /// Method name from the frozen envelope contract.
        method: String,

        /// Test seam: directory holding registry.sqlite3.
        #[arg(long, hide = true)]
        data_dir: Option<String>,

        /// Test seam: kernel-lock directory.
        #[arg(long, hide = true)]
        lock_dir: Option<String>,
    },

    /// Internal: single-use-token PTY attach channel (plan §12.1). Verifies
    /// the token and execs the exact owner-generated tmux attach argv.
    #[command(name = "_attach", hide = true)]
    Attach {
        #[arg(long)]
        token: String,

        /// Test seam: directory holding registry.sqlite3.
        #[arg(long, hide = true)]
        data_dir: Option<String>,

        /// Test seam: kernel-lock directory.
        #[arg(long, hide = true)]
        lock_dir: Option<String>,
    },

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
        Some(Cmd::Con {
            target,
            window,
            create,
        }) => attach::con(&context, &target, window.as_deref(), create),
        Some(Cmd::New { name, dir, command }) => {
            attach::new_session_in(&context, &name, dir.as_deref(), &command)
        }
        Some(Cmd::Detach) => attach::detach(&context),
        Some(Cmd::Rm {
            targets,
            all,
            window,
            yes,
        }) => attach::remove(&context, &targets, all, window.as_deref(), yes),
        Some(Cmd::Rename { old, new_name }) => attach::rename(&context, &old, &new_name),
        Some(Cmd::Keys { man, tmux, wez }) => keys::run(man, tmux, wez),
        Some(Cmd::Doctor { json }) => Ok(doctor::run(&context, json)),
        Some(Cmd::MuxIdle) => loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        },
        Some(Cmd::TmuxBootstrap {
            namespace,
            data_dir,
            lock_dir,
        }) => tmux_bootstrap_cmd(namespace, data_dir, lock_dir),
        Some(Cmd::Group { cmd }) => space_cli::group(cmd),
        Some(Cmd::Split { cmd }) => space_cli::split(cmd),
        Some(Cmd::Context { cmd }) => space_cli::context(cmd),
        Some(Cmd::Ssh { target }) => {
            let code = dmux::remote::enroll::run(&target);
            Ok(ExitCode::from(u8::try_from(code).unwrap_or(1)))
        }
        Some(Cmd::HostAdmin { cmd }) => space_cli::host(cmd),
        Some(Cmd::ContextInternal { data_dir, lock_dir }) => context_cmd(data_dir, lock_dir),
        Some(Cmd::Agent {
            protocol,
            method,
            data_dir,
            lock_dir,
        }) => {
            let code = dmux::remote::agent::run(&dmux::remote::agent::AgentArgs {
                protocol,
                method,
                data_dir: data_dir.map(std::path::PathBuf::from),
                lock_dir: lock_dir.map(std::path::PathBuf::from),
            });
            Ok(ExitCode::from(u8::try_from(code).unwrap_or(1)))
        }
        Some(Cmd::Attach {
            token,
            data_dir,
            lock_dir,
        }) => {
            let code = dmux::remote::attach::run(&dmux::remote::attach::AttachArgs {
                token,
                data_dir: data_dir.map(std::path::PathBuf::from),
                lock_dir: lock_dir.map(std::path::PathBuf::from),
            });
            Ok(ExitCode::from(u8::try_from(code).unwrap_or(1)))
        }
        Some(Cmd::Other(args)) => other(&context, &args),
    };
    match outcome {
        Ok(status) => status,
        Err(message) => workstation::fail(PROGRAM, message),
    }
}

/// `dmux _context` (plan §13.1): one validated marker JSON document on
/// stdout, or a typed error and no output. The shell prompt hook consumes
/// this; a failure must never fabricate markers.
fn context_cmd(data_dir: Option<String>, lock_dir: Option<String>) -> Result<ExitCode, String> {
    use dmux::backend::InventoryScope;
    use dmux::model::{Backend, SpaceUid};
    use dmux::operations::{self, OperationEnv};

    let env = match (data_dir, lock_dir) {
        (Some(data), Some(lock)) => OperationEnv {
            db_path: std::path::PathBuf::from(data).join("registry.sqlite3"),
            lock_dir: std::path::PathBuf::from(lock),
        },
        _ => OperationEnv::production().map_err(|e| e.to_string())?,
    };
    let space_uid = std::env::var("DMUX_SPACE_UID")
        .ok()
        .and_then(|v| v.parse::<uuid::Uuid>().ok())
        .map(SpaceUid)
        .ok_or("no DMUX_SPACE_UID in this pane's environment")?;

    let context = if let (Ok(tmux), Ok(pane)) = (std::env::var("TMUX"), std::env::var("TMUX_PANE"))
    {
        let namespace = operations::namespace_from_tmux_env(&tmux)
            .ok_or("not a managed -L tmux server (pass --namespace paths explicitly)")?;
        let provider = dmux::backend::tmux::TmuxProvider::new(namespace.clone());
        let scope = InventoryScope {
            backend: Backend::Tmux,
            endpoint: namespace,
            expected_epoch: None,
        };
        operations::context_read(&env, &provider, &scope, space_uid, &pane)
    } else if let Ok(pane) = std::env::var("WEZTERM_PANE") {
        let descriptor = dmux::runtime::read_wez_descriptor()
            .map_err(|e| e.to_string())?
            .ok_or("managed mux descriptor absent (service not running)")?;
        let (bin, config) = space_cli::production_wez_paths();
        let provider = dmux::backend::wez::WezProvider::new(&bin, config);
        let scope = InventoryScope {
            backend: Backend::Wez,
            endpoint: descriptor.socket,
            expected_epoch: None,
        };
        operations::context_read(&env, &provider, &scope, space_uid, &pane)
    } else {
        return Err("neither TMUX_PANE nor WEZTERM_PANE is set".into());
    }
    .map_err(|e| e.to_string())?;

    println!(
        "{}",
        serde_json::to_string(&context).map_err(|e| e.to_string())?
    );
    Ok(ExitCode::SUCCESS)
}

/// `dmux _tmux-bootstrap`: silent on success (the session-created hook's
/// run-shell output would land in a pane), typed message + nonzero on
/// failure.
fn tmux_bootstrap_cmd(
    namespace: Option<String>,
    data_dir: Option<String>,
    lock_dir: Option<String>,
) -> Result<ExitCode, String> {
    use dmux::operations::{self, OperationEnv};
    // Over bare ssh a POSIX-locale tmux client mangles the provider's
    // U+001F separators (P7 handoff risk 1); normalize like `_agent` does.
    dmux::remote::normalize_utf8_locale();
    let namespace = namespace
        .or_else(|| {
            std::env::var("TMUX")
                .ok()
                .and_then(|t| operations::namespace_from_tmux_env(&t))
        })
        .ok_or_else(|| {
            "usage: dmux _tmux-bootstrap --namespace <name> \
             (or run inside the managed server)"
                .to_string()
        })?;
    let env = match (data_dir, lock_dir) {
        (Some(data), Some(lock)) => OperationEnv {
            db_path: std::path::PathBuf::from(data).join("registry.sqlite3"),
            lock_dir: std::path::PathBuf::from(lock),
        },
        _ => OperationEnv::production().map_err(|e| e.to_string())?,
    };
    operations::tmux_bootstrap(&env, &namespace).map_err(|e| e.to_string())?;
    Ok(ExitCode::SUCCESS)
}

/// clap cannot parse a bare `-` as a subcommand, so the toggle spelling is
/// rewritten into a marker the external-subcommand arm recognises; `@` keeps
/// it out of the valid session namespace.
fn normalized_args() -> Vec<OsString> {
    let mut args: Vec<OsString> = std::env::args_os().collect();
    if let Some(position) = toggle_position(&args) {
        args[position] = "@prev".into();
    }
    args
}

/// Only the standalone toggle invocation is rewritten: a bare `-` standing
/// where the subcommand would, after nothing but the global host flag
/// (`dmux -`, `dmux -H peer -`). A `-` anywhere else — `dmux rm -` — reaches
/// clap untouched and earns its ordinary error.
fn toggle_position(args: &[OsString]) -> Option<usize> {
    let mut position = 1;
    while position < args.len() {
        let arg = args[position].to_str()?;
        match arg {
            "-" => return Some(position),
            "-H" | "--host" => position += 2,
            _ if arg.starts_with("--host=") || (arg.starts_with("-H") && arg.len() > 2) => {
                position += 1;
            }
            _ => return None,
        }
    }
    None
}

fn other(context: &Context, args: &[String]) -> Result<ExitCode, String> {
    match args {
        [word] if word == "@prev" => attach::toggle(context),
        [word, ..] if word == "@prev" => Err("`dmux -` takes no arguments".to_string()),
        [target] => attach::con(context, target, None, false),
        [target, rest @ ..] => match parse_window(rest) {
            Some(window) => attach::con(context, target, Some(window), false),
            None => Err(format!("unexpected arguments: {}", args.join(" "))),
        },
        [] => Err("unexpected arguments".to_string()),
    }
}

/// The one flag the fallthrough shares with `con`: `-w/--window`, in every
/// spelling clap would accept there. Anything else keeps the strict error.
fn parse_window(rest: &[String]) -> Option<&str> {
    match rest {
        [flag, window] if flag == "-w" || flag == "--window" => Some(window),
        [joined] => {
            if let Some(window) = joined.strip_prefix("--window=") {
                return Some(window);
            }
            if joined.starts_with("--") {
                return None;
            }
            joined
                .strip_prefix("-w")
                .filter(|window| !window.is_empty())
        }
        _ => None,
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
/// otherwise. Declining to choose is not an error; a broken picker or an
/// answer that names no row is.
fn pick(rows: &[list::Row]) -> Result<Option<usize>, String> {
    let lines = list::render(rows, &Style::for_stdout());
    let chosen = if attach::on_path("fzf") {
        fzf_pick(&lines)?
    } else {
        number_pick(&lines, rows.len())?
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
        // fzf draws its interface on /dev/tty, so stderr carries only real
        // complaints and is safe to capture.
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("fzf: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(lines.join("\n").as_bytes());
        let _ = stdin.write_all(b"\n");
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("fzf: {error}"))?;
    fzf_outcome(
        output.status.code(),
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(&output.stderr),
    )
}

/// fzf's exit codes are an interface: 0 selected, 1 matched nothing, 130
/// cancelled — the last two are a quiet "no thanks". Anything else (2 is
/// fzf's own error code) is a real failure and surfaces fzf's stderr.
fn fzf_outcome(code: Option<i32>, stdout: &str, stderr: &str) -> Result<Option<usize>, String> {
    match code {
        Some(0) => Ok(leading_index(stdout)),
        Some(1) | Some(130) | None => Ok(None),
        Some(code) => {
            let reason = stderr.lines().map(str::trim).find(|line| !line.is_empty());
            Err(match reason {
                Some(reason) => format!("fzf failed (exit {code}): {reason}"),
                None => format!("fzf failed (exit {code})"),
            })
        }
    }
}

fn number_pick(lines: &[String], count: usize) -> Result<Option<usize>, String> {
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
        Ok(_) => parse_pick(answer.trim(), count),
    }
}

/// An empty answer declines; anything else must name a listed index —
/// garbage and out-of-range numbers are errors, not silent successes.
fn parse_pick(answer: &str, count: usize) -> Result<Option<usize>, String> {
    if answer.is_empty() {
        return Ok(None);
    }
    match answer.parse::<usize>() {
        Ok(number) if (1..=count).contains(&number) => Ok(Some(number)),
        _ => Err(format!("no such index '{answer}'")),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    #[test]
    fn only_a_leading_dash_is_the_toggle() {
        assert_eq!(toggle_position(&args(&["dmux", "-"])), Some(1));
        assert_eq!(
            toggle_position(&args(&["dmux", "-H", "archie", "-"])),
            Some(3)
        );
        assert_eq!(
            toggle_position(&args(&["dmux", "--host", "archie", "-"])),
            Some(3)
        );
        assert_eq!(
            toggle_position(&args(&["dmux", "--host=archie", "-"])),
            Some(2)
        );
        assert_eq!(toggle_position(&args(&["dmux", "-Harchie", "-"])), Some(2));
        assert_eq!(toggle_position(&args(&["dmux"])), None);
        assert_eq!(toggle_position(&args(&["dmux", "rm", "-"])), None);
        assert_eq!(toggle_position(&args(&["dmux", "con", "-"])), None);
        assert_eq!(toggle_position(&args(&["dmux", "ls", "--host", "-"])), None);
    }

    #[test]
    fn the_fallthrough_window_takes_every_con_spelling() {
        let words = |items: &[&str]| -> Vec<String> {
            items.iter().map(|item| (*item).to_string()).collect()
        };
        assert_eq!(parse_window(&words(&["-w", "2"])), Some("2"));
        assert_eq!(parse_window(&words(&["--window", "2"])), Some("2"));
        assert_eq!(parse_window(&words(&["--window=2"])), Some("2"));
        assert_eq!(parse_window(&words(&["-w2"])), Some("2"));
        assert_eq!(parse_window(&words(&["-w"])), None);
        assert_eq!(parse_window(&words(&["--window"])), None);
        assert_eq!(parse_window(&words(&["-x", "2"])), None);
        assert_eq!(parse_window(&words(&["2"])), None);
        assert_eq!(parse_window(&words(&["-w", "2", "extra"])), None);
    }

    #[test]
    fn a_picked_number_must_name_a_row() {
        assert_eq!(parse_pick("", 3), Ok(None));
        assert_eq!(parse_pick("2", 3), Ok(Some(2)));
        assert_eq!(parse_pick("3", 3), Ok(Some(3)));
        assert_eq!(parse_pick("0", 3).unwrap_err(), "no such index '0'");
        assert_eq!(parse_pick("4", 3).unwrap_err(), "no such index '4'");
        assert_eq!(parse_pick("nope", 3).unwrap_err(), "no such index 'nope'");
    }

    #[test]
    fn fzf_cancel_is_quiet_and_failure_is_loud() {
        assert_eq!(fzf_outcome(Some(0), "2  beta\n", ""), Ok(Some(2)));
        assert_eq!(fzf_outcome(Some(1), "", ""), Ok(None));
        assert_eq!(fzf_outcome(Some(130), "", ""), Ok(None));
        assert_eq!(fzf_outcome(None, "", ""), Ok(None));
        assert_eq!(
            fzf_outcome(Some(2), "", "unknown option: --bogus\n"),
            Err("fzf failed (exit 2): unknown option: --bogus".to_string())
        );
        assert_eq!(
            fzf_outcome(Some(2), "", ""),
            Err("fzf failed (exit 2)".to_string())
        );
    }
}
