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
//! `DMUX_DRY_RUN=1` prints legacy command plans. Wez-first presentation
//! refuses previews before resolving a target because planning may itself
//! authenticate a GUI or mint a single-use remote attach credential.

mod attach;
mod doctor;
mod hosts;
mod keys;
mod list;
mod space_cli;
mod state;

use std::ffi::{OsStr, OsString};
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::{Command as Process, ExitCode, Stdio};

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::CompleteEnv;
use clap_complete::engine::{ArgValueCompleter, CompletionCandidate};
use workstation::{Completions, Style};

use hosts::{Context, Host};

const PROGRAM: &str = "dmux";

/// One release of overlap for the `--json` flags (plan §16.2): their exact
/// legacy payload stays on stdout because scripts compare it byte for byte,
/// so the migration hint has nowhere to go but stderr.
const JSON_FLAG_HINT: &str = "dmux: --json is deprecated; use --format json";

#[derive(Parser)]
#[command(
    name = "dmux",
    about = "Wezterm-mux and tmux sessions: list, attach, create",
    after_long_help = "Environment:
  DMUX_DRY_RUN=1  print legacy plans; Wez-first connect refuses safely

Bare `dmux` opens a picker (or creates `main` when nothing runs); with --host
it attaches the peer the way the old ssa/ssm did. `dmux <name>` attaches an
existing session (a trailing `-w N` picks its window), and `dmux -` toggles
back to the previous one."
)]
struct Cli {
    /// Host whose sessions to use (default: this machine)
    #[arg(short = 'H', long, global = true, value_name = "HOST")]
    host: Option<String>,

    /// Output shape for bounded commands: the human table, or one versioned
    /// JSON document
    #[arg(long, global = true, value_enum, value_name = "FORMAT")]
    format: Option<dmux::output::OutputFormat>,

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
    #[command(visible_alias = "list", long_about = dmux::ls_cli::SCOPES_HELP)]
    Ls {
        /// Every enrolled host instead of one
        #[arg(long, conflicts_with = "host")]
        all_hosts: bool,

        /// Only this backend
        #[arg(long, value_enum)]
        backend: Option<ConnectBackend>,

        /// Show each Space's Groups and Splits beneath it
        #[arg(long)]
        tree: bool,

        /// Deprecated: only tmux sessions (use --backend tmux)
        #[arg(long)]
        tmux: bool,

        /// Deprecated: only wezterm workspaces (use --backend wez)
        #[arg(long)]
        wez: bool,

        /// Deprecated: bare row array (use --format json)
        #[arg(long)]
        json: bool,

        #[arg(long, hide = true, conflicts_with = "json")]
        names: bool,
    },

    /// Attach an existing session ("continue")
    #[command(visible_aliases = ["attach", "a"])]
    Con {
        /// Stable Space ref (or legacy exact name when the Wez-first flag is off)
        #[arg(
            required_unless_present = "name",
            conflicts_with = "name",
            add = ArgValueCompleter::new(complete_sessions)
        )]
        target: Option<String>,

        /// Treat VALUE as an exact logical Space name rather than parsing a ref
        #[arg(long, value_name = "VALUE", required_unless_present = "target")]
        name: Option<String>,

        /// Require exactly this backend; never fall back to the other one
        #[arg(long, value_enum)]
        backend: Option<ConnectBackend>,

        /// Focus this epoch-qualified Group after connecting
        #[arg(long, value_name = "GROUP_REF", conflicts_with = "split")]
        group: Option<String>,

        /// Focus this epoch-qualified Split after connecting
        #[arg(long, value_name = "SPLIT_REF", conflicts_with = "group")]
        split: Option<String>,

        /// Start a managed GUI and attach only to an existing Wez Space
        #[arg(long)]
        launch_gui: bool,

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

        /// Creation policy backend (automatic when omitted)
        #[arg(long, value_enum)]
        backend: Option<NewBackend>,

        /// Working directory for the new session
        #[arg(long, value_name = "PATH")]
        dir: Option<String>,

        /// Create or select without presenting/attaching
        #[arg(long, conflicts_with = "launch_gui")]
        no_connect: bool,

        /// Permit creation beside one selectable opposite-backend match
        #[arg(long)]
        allow_name_collision: bool,

        /// Start a managed GUI and attach only to a Wez Space
        #[arg(long, conflicts_with = "no_connect")]
        launch_gui: bool,

        /// Command to run in the new session, after `--`
        #[arg(last = true, value_name = "CMD")]
        command: Vec<String>,
    },

    /// Disconnect the invoking client/domain without removing owner panes.
    #[command(visible_alias = "detach")]
    Disconnect {
        /// Detach the entire current imported Wez domain.
        #[arg(long)]
        domain: bool,
    },

    /// Inspect or control guarded Wez mux recovery
    Recovery {
        #[command(subcommand)]
        cmd: RecoveryCmd,
    },

    /// Kill sessions, or one window of a session with -w
    #[command(visible_aliases = ["kill", "delete"])]
    Rm {
        /// Stable Space refs (or legacy exact names when the Wez-first
        /// flag is off); a bare digit is a permanent SpaceNo, never a row
        #[arg(
            required_unless_present_any = ["all", "row", "name"],
            conflicts_with = "all",
            add = ArgValueCompleter::new(complete_sessions)
        )]
        targets: Vec<String>,

        /// Treat VALUE as the exact logical name of the Space to remove
        #[arg(long, value_name = "VALUE", conflicts_with_all = ["all", "row", "targets"])]
        name: Option<String>,

        /// Remove every Space on exactly one host, optionally backend-filtered;
        /// pre-gate this is tmux only and keeps the session this client is in
        #[arg(long, conflicts_with = "window")]
        all: bool,

        /// Deprecated one-release escape: 1-based position in the `dmux ls`
        /// listing, repeatable. Resolved to a stable ref and reported first
        #[arg(long, value_name = "N", conflicts_with = "all")]
        row: Vec<u64>,

        /// Require exactly this backend; never fall back to the other one
        #[arg(long, value_enum)]
        backend: Option<ConnectBackend>,

        /// Kill one window of the session instead
        #[arg(short, long)]
        window: Option<String>,

        /// Kill without asking
        #[arg(short, long)]
        yes: bool,
    },

    /// Rename a Space (a tmux session when the Wez-first flag is off)
    Rename {
        /// Stable Space ref; with --name/--row it is the new name instead
        #[arg(
            required_unless_present_any = ["name", "row"],
            add = ArgValueCompleter::new(complete_sessions)
        )]
        old: Option<String>,

        #[arg(value_name = "NEW")]
        new_name: Option<String>,

        /// Treat VALUE as the exact logical name of the Space to rename
        #[arg(long, value_name = "VALUE", conflicts_with = "row")]
        name: Option<String>,

        /// Deprecated one-release escape: 1-based position in the `dmux ls`
        /// listing. Resolved to a stable ref and reported first
        #[arg(long, value_name = "N")]
        row: Option<u64>,

        /// Require exactly this backend; never fall back to the other one
        #[arg(long, value_enum)]
        backend: Option<ConnectBackend>,

        /// Permit a name one opposite-backend Space already holds
        #[arg(long)]
        allow_name_collision: bool,
    },

    /// Adopt one unmanaged native resource listed by `dmux ls`
    Adopt {
        /// Opaque NATIVE_REF from an unmanaged row; never a backend command
        native_ref: String,

        /// Logical name for the adopted Space (default: its native name)
        #[arg(long, value_name = "NAME")]
        name: Option<String>,
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
        /// Deprecated: bare probe object (use --format json)
        #[arg(long)]
        json: bool,
    },

    /// Bring existing sessions and workspaces under management, once
    Migrate {
        /// Apply the printed plan; without it nothing is adopted or stamped
        #[arg(long)]
        commit: bool,

        /// Commit without asking
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Internal: keep the reserved mux sentinel pane alive (plan §15.1).
    /// Never a Space, Group, or Split; excluded from all listings.
    #[command(name = "_mux-idle", hide = true)]
    MuxIdle,

    /// Internal: provision the per-boot GUI bridge HMAC key beneath the
    /// verified runtime directory. The raw key is never printed.
    #[command(name = "_bridge-key", hide = true)]
    BridgeKey,

    /// Internal: authority-revalidated GUI controller. The trailing argv is
    /// parsed by the library so every bounded invocation, including a verb
    /// usage error, emits exactly one JSON response document.
    #[command(name = "_gui", hide = true)]
    GuiInternal {
        #[arg(long)]
        origin_json: Option<String>,

        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        argv: Vec<String>,
    },

    /// Internal: detach one bounded post-exec GUI-history monitor. It can
    /// finalize only an already-staged exact pending transition.
    #[command(name = "_gui-exec-finalize", hide = true)]
    GuiExecFinalize {
        #[arg(long)]
        pending_uid: String,
    },

    /// Internal: registry-only guarded cold-recovery service surface
    /// (plan §15.3). The coordinator deliberately rejects inherited mux
    /// endpoint variables before taking any recovery fence.
    #[command(name = "_recovery", hide = true)]
    RecoveryInternal {
        #[command(subcommand)]
        cmd: RecoveryInternalCmd,
    },

    /// Internal: every user-visible verb and alias, one per line, straight
    /// from clap's command tree. The shell wrappers keep a verb allowlist so a
    /// lone bare word is not mistaken for a Space name, and its sync test reads
    /// this rather than `--help` or `--completions`: both of those render only
    /// *visible* aliases, so a plain `#[command(alias = ...)]` was invisible to
    /// them and silently turned `ssa <that alias>` into a Space constructor.
    #[command(name = "_verbs", hide = true)]
    Verbs,

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

    /// Internal: refresh one exact outer-GUI marker from a managed tmux
    /// client hook after attach, session change, or native child selection.
    #[command(name = "_tmux-context-refresh", hide = true)]
    TmuxContextRefresh {
        #[command(flatten)]
        args: dmux::tmux_hook_cli::TmuxContextRefreshArgs,
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

    /// Repair managed-plane state (plan §10.3)
    Repair {
        #[command(subcommand)]
        cmd: space_cli::RepairCmd,
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

#[derive(Subcommand)]
enum RecoveryInternalCmd {
    /// Resolve or register the fixed local Wez backend instance.
    Instance {
        #[arg(long)]
        socket: PathBuf,

        #[arg(long)]
        service_label: String,
    },

    /// Hold the instance fence while Lua restores one manifest generation.
    Coordinate {
        #[arg(long)]
        backend_instance: uuid::Uuid,

        #[arg(long)]
        server_epoch: uuid::Uuid,

        #[arg(long)]
        runtime_dir: PathBuf,

        #[arg(long)]
        server_pid: i64,

        #[arg(long)]
        server_start_token: String,

        #[arg(long)]
        helper_bin: String,

        /// Explicit operator resume of a failed journal generation.
        #[arg(long, conflicts_with = "abort_failed")]
        resume_failed: bool,

        /// Explicit operator abort of a failed journal generation.
        #[arg(long, conflicts_with = "resume_failed")]
        abort_failed: bool,
    },

    /// Validate and atomically publish one complete recovery manifest.
    SnapshotPublish {
        #[arg(long)]
        backend_instance: uuid::Uuid,

        #[arg(long)]
        candidate_id: String,

        #[arg(long)]
        server_epoch: uuid::Uuid,

        #[arg(long)]
        runtime_dir: PathBuf,

        #[arg(long)]
        server_pid: i64,

        #[arg(long)]
        server_start_token: String,
    },
}

#[derive(Subcommand)]
enum RecoveryCmd {
    /// Show the current owner's durable recovery state.
    Status,

    /// Resume the owner's failed recovery generation.
    Resume,

    /// Abort the owner's failed recovery generation without resurrecting it.
    Abort {
        /// Abort without an interactive confirmation.
        #[arg(short = 'y', long)]
        yes: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum ConnectBackend {
    Wez,
    Tmux,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum NewBackend {
    Auto,
    Wez,
    Tmux,
}

impl NewBackend {
    fn constraint(self) -> Option<dmux::model::Backend> {
        match self {
            NewBackend::Auto => None,
            NewBackend::Wez => Some(dmux::model::Backend::Wez),
            NewBackend::Tmux => Some(dmux::model::Backend::Tmux),
        }
    }
}

impl From<ConnectBackend> for dmux::model::Backend {
    fn from(value: ConnectBackend) -> Self {
        match value {
            ConnectBackend::Wez => Self::Wez,
            ConnectBackend::Tmux => Self::Tmux,
        }
    }
}

struct ConnectCliArgs {
    target: Option<String>,
    name: Option<String>,
    backend: Option<ConnectBackend>,
    group: Option<String>,
    split: Option<String>,
    launch_gui: bool,
    window: Option<String>,
    create: bool,
}

struct NewCliArgs {
    name: String,
    backend: Option<NewBackend>,
    dir: Option<String>,
    no_connect: bool,
    allow_name_collision: bool,
    launch_gui: bool,
    command: Vec<String>,
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
    let host_given = cli.host.is_some();
    // Case 43: `--format json` is an output shape, not a Wez-first
    // behaviour, so nothing below gates the flag itself — but a verb with no
    // bounded JSON result refuses it here as one document rather than
    // printing its human report under a flag it never reads.
    if cli.format == Some(dmux::output::OutputFormat::Json)
        && let Some(verb) = unbounded_json_verb(cli.command.as_ref(), host_given)
    {
        return refuse(
            verb,
            cli.format,
            dmux::error::TypedError::new(
                dmux::error::ErrorCode::Usage,
                format!(
                    "{verb} emits no bounded JSON document (plan §16.2); \
                     --format json is refused, never ignored"
                ),
            ),
        );
    }
    let dynamic_host_command = wez_first_enabled()
        && matches!(
            &cli.command,
            Some(Cmd::Con { .. })
                | Some(Cmd::New { .. })
                | Some(Cmd::Recovery { .. })
                | Some(Cmd::Ls { .. })
                | Some(Cmd::Rm { .. })
                | Some(Cmd::Rename { .. })
                | Some(Cmd::Adopt { .. })
                | Some(Cmd::Migrate { .. })
                | Some(Cmd::Other(_))
        );
    let legacy_host = match cli.host.as_deref().map(legacy_host) {
        Some(Some(host)) => Some(host),
        Some(None) if dynamic_host_command => None,
        Some(None) => {
            eprintln!(
                "dmux: unknown legacy host {:?}; enrolled aliases/labels/UIDs require a Wez-first con/new command",
                cli.host.as_deref().unwrap_or_default()
            );
            return ExitCode::from(2);
        }
        None => None,
    };
    let context = match Context::resolve(legacy_host) {
        Ok(context) => context,
        Err(message) => return workstation::fail(PROGRAM, message),
    };
    let outcome = match cli.command {
        None => bare(host_given, &context, cli.format),
        Some(Cmd::Ls {
            all_hosts,
            backend,
            tree,
            tmux,
            wez,
            json,
            names,
        }) => ls(
            &context,
            cli.format,
            dmux::ls_cli::LsArgs {
                host: cli.host.clone(),
                all_hosts,
                backend: backend.map(dmux::model::Backend::from),
                tree,
                json,
                only_tmux: tmux,
                only_wez: wez,
                names,
            },
        ),
        Some(Cmd::Con {
            target,
            name,
            backend,
            group,
            split,
            launch_gui,
            window,
            create,
        }) => {
            if wez_first_enabled() {
                Ok(connect_command(
                    &context,
                    cli.host.as_deref(),
                    ConnectCliArgs {
                        target,
                        name,
                        backend,
                        group,
                        split,
                        launch_gui,
                        window,
                        create,
                    },
                ))
            } else {
                if name.is_some()
                    || backend.is_some()
                    || group.is_some()
                    || split.is_some()
                    || launch_gui
                {
                    Ok(render_connect_error(dmux::error::TypedError::new(
                        dmux::error::ErrorCode::Usage,
                        "--name/--backend/--group/--split/--launch-gui require DMUX_WEZ_FIRST=1",
                    )))
                } else {
                    let target = target.expect("clap requires a con target");
                    attach::con(&context, &target, window.as_deref(), create)
                }
            }
        }
        Some(Cmd::New {
            name,
            backend,
            dir,
            no_connect,
            allow_name_collision,
            launch_gui,
            command,
        }) => {
            if wez_first_enabled() {
                Ok(new_command(
                    &context,
                    cli.host.as_deref(),
                    NewCliArgs {
                        name,
                        backend,
                        dir,
                        no_connect,
                        allow_name_collision,
                        launch_gui,
                        command,
                    },
                ))
            } else if backend.is_some() || no_connect || allow_name_collision || launch_gui {
                Ok(render_connect_error(dmux::error::TypedError::new(
                    dmux::error::ErrorCode::Usage,
                    "--backend/--no-connect/--allow-name-collision/--launch-gui require DMUX_WEZ_FIRST=1",
                )))
            } else {
                attach::new_session_in(&context, &name, dir.as_deref(), &command)
            }
        }
        Some(Cmd::Disconnect { domain }) => disconnect(&context, domain, host_given),
        Some(Cmd::Recovery { cmd }) => {
            Ok(recovery_cmd(&context, cli.host.as_deref(), cli.format, cmd))
        }
        Some(Cmd::Rm {
            targets,
            name,
            all,
            row,
            backend,
            window,
            yes,
        }) => {
            if wez_first_enabled() && dmux::rm_cli::IMPLEMENTED {
                Ok(exit_code(dmux::rm_cli::remove(
                    cli.format,
                    dmux::rm_cli::RmArgs {
                        host: cli.host.clone(),
                        targets,
                        name,
                        rows: row,
                        all,
                        backend: backend.map(dmux::model::Backend::from),
                        window,
                        yes,
                    },
                )))
            } else if !row.is_empty() || name.is_some() || backend.is_some() || cli.format.is_some()
            {
                Ok(refuse(
                    "rm",
                    cli.format,
                    dmux::error::TypedError::new(
                        dmux::error::ErrorCode::Usage,
                        "--name/--row/--backend/--format require DMUX_WEZ_FIRST=1",
                    ),
                ))
            } else {
                attach::remove(&context, &targets, all, window.as_deref(), yes)
            }
        }
        Some(Cmd::Rename {
            old,
            new_name,
            name,
            row,
            backend,
            allow_name_collision,
        }) => {
            if wez_first_enabled() && dmux::rm_cli::IMPLEMENTED {
                Ok(exit_code(dmux::rm_cli::rename(
                    cli.format,
                    dmux::rm_cli::RenameArgs {
                        host: cli.host.clone(),
                        old,
                        new_name,
                        name,
                        row,
                        backend: backend.map(dmux::model::Backend::from),
                        allow_name_collision,
                    },
                )))
            } else if name.is_some()
                || row.is_some()
                || backend.is_some()
                || allow_name_collision
                || cli.format.is_some()
            {
                Ok(refuse(
                    "rename",
                    cli.format,
                    dmux::error::TypedError::new(
                        dmux::error::ErrorCode::Usage,
                        "--name/--row/--backend/--allow-name-collision/--format require DMUX_WEZ_FIRST=1",
                    ),
                ))
            } else {
                // Both positionals are optional so `--name OLD NEW` can parse;
                // the legacy two-word spelling has to be checked here instead.
                match (old, new_name) {
                    (Some(old), Some(new_name)) => attach::rename(&context, &old, &new_name),
                    _ => Ok(render_connect_error(dmux::error::TypedError::new(
                        dmux::error::ErrorCode::Usage,
                        "rename takes an existing session and a new name",
                    ))),
                }
            }
        }
        Some(Cmd::Adopt { native_ref, name }) => {
            if wez_first_enabled() && dmux::adopt_cli::IMPLEMENTED {
                Ok(exit_code(dmux::adopt_cli::adopt(
                    cli.format,
                    dmux::adopt_cli::AdoptArgs {
                        host: cli.host.clone(),
                        native_ref,
                        name,
                    },
                )))
            } else if wez_first_enabled() {
                Ok(refuse(
                    "adopt",
                    cli.format,
                    dmux::error::TypedError::new(
                        dmux::error::ErrorCode::Usage,
                        "adopt is not implemented yet",
                    ),
                ))
            } else {
                Ok(refuse(
                    "adopt",
                    cli.format,
                    dmux::error::TypedError::new(
                        dmux::error::ErrorCode::Usage,
                        "adopt requires DMUX_WEZ_FIRST=1",
                    ),
                ))
            }
        }
        Some(Cmd::Migrate { commit, yes }) => {
            if wez_first_enabled() && dmux::migrate_cli::IMPLEMENTED {
                Ok(exit_code(dmux::migrate_cli::run(
                    cli.format,
                    dmux::migrate_cli::MigrateArgs {
                        commit,
                        yes,
                        previous_sessions: state::entries(),
                    },
                )))
            } else if wez_first_enabled() {
                Ok(refuse(
                    "migrate",
                    cli.format,
                    dmux::error::TypedError::new(
                        dmux::error::ErrorCode::Usage,
                        "migrate is not implemented yet",
                    ),
                ))
            } else {
                Ok(refuse(
                    "migrate",
                    cli.format,
                    dmux::error::TypedError::new(
                        dmux::error::ErrorCode::Usage,
                        "migrate requires DMUX_WEZ_FIRST=1",
                    ),
                ))
            }
        }
        Some(Cmd::Verbs) => {
            // build() first: clap materialises its own `help` subcommand there,
            // and `ssa help` must print help rather than name a Space.
            let mut root = Cli::command();
            root.build();
            for command in root.get_subcommands() {
                if command.is_hide_set() {
                    continue;
                }
                println!("{}", command.get_name());
                for alias in command.get_all_aliases() {
                    println!("{alias}");
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Some(Cmd::Keys { man, tmux, wez }) => keys::run(man, tmux, wez),
        Some(Cmd::Doctor { json }) => Ok(doctor::run(&context, json, cli.format)),
        Some(Cmd::MuxIdle) => loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        },
        Some(Cmd::BridgeKey) => (|| {
            let runtime = dmux::runtime::dmux_runtime_dir().map_err(|e| e.to_string())?;
            dmux::gui::ensure_bridge_key(&runtime).map_err(|e| e.to_string())?;
            Ok(ExitCode::SUCCESS)
        })(),
        Some(Cmd::GuiInternal { origin_json, argv }) => Ok(ExitCode::from(
            dmux::gui_cli::run_production_argv(origin_json.as_deref(), &argv),
        )),
        Some(Cmd::GuiExecFinalize { pending_uid }) => Ok(gui_exec_finalize_cmd(&pending_uid)),
        Some(Cmd::RecoveryInternal { cmd }) => Ok(recovery_internal_cmd(cmd)),
        Some(Cmd::TmuxBootstrap {
            namespace,
            data_dir,
            lock_dir,
        }) => tmux_bootstrap_cmd(namespace, data_dir, lock_dir),
        Some(Cmd::TmuxContextRefresh { args }) => {
            Ok(ExitCode::from(dmux::tmux_hook_cli::run(&args)))
        }
        Some(Cmd::Group { cmd }) => Ok(space_cli::group(cmd, cli.format)),
        Some(Cmd::Split { cmd }) => Ok(space_cli::split(cmd, cli.format)),
        Some(Cmd::Context { cmd }) => Ok(space_cli::context(cmd, cli.format)),
        Some(Cmd::Repair { cmd }) => Ok(space_cli::repair(cmd, cli.format)),
        Some(Cmd::Ssh { target }) => {
            let code = dmux::remote::enroll::run(&target);
            Ok(ExitCode::from(u8::try_from(code).unwrap_or(1)))
        }
        Some(Cmd::HostAdmin { cmd }) => Ok(space_cli::host(cmd, cli.format)),
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
        Some(Cmd::Other(args)) => other(cli.host.as_deref(), &context, &args),
    };
    match outcome {
        Ok(status) => status,
        Err(message) => workstation::fail(PROGRAM, message),
    }
}

/// Service-only recovery commands use one bounded JSON error on stderr and
/// a stable exit class. Successful coordinator/publication calls emit one
/// JSON document; `instance` is intentionally the single UUID line consumed
/// by the service wrapper.
fn recovery_internal_cmd(cmd: RecoveryInternalCmd) -> ExitCode {
    use dmux::model::{BackendInstanceUid, ServerEpoch};
    use dmux::recovery::{
        RecoveryCoordinatorOptions, ensure_wez_backend_instance, production_recovery_manifest_dir,
        publish_snapshot_manifest, run_recovery_coordinator,
    };
    use dmux::registry::RegistryConfig;

    let result = (|| -> dmux::recovery::Result<()> {
        let config = RegistryConfig::production()?;
        match cmd {
            RecoveryInternalCmd::Instance {
                socket,
                service_label,
            } => {
                let instance = ensure_wez_backend_instance(config, &socket, &service_label)?;
                println!("{}", instance.0);
            }
            RecoveryInternalCmd::Coordinate {
                backend_instance,
                server_epoch,
                runtime_dir,
                server_pid,
                server_start_token,
                helper_bin,
                resume_failed,
                abort_failed,
            } => {
                let mut options = RecoveryCoordinatorOptions::new(
                    config,
                    runtime_dir,
                    production_recovery_manifest_dir()?,
                    BackendInstanceUid(backend_instance),
                    ServerEpoch(server_epoch),
                    server_pid,
                    server_start_token,
                    helper_bin,
                );
                options.resume_failed = resume_failed;
                options.abort_failed = abort_failed;
                let report = run_recovery_coordinator(options)?;
                println!("{}", serde_json::to_string(&report)?);
            }
            RecoveryInternalCmd::SnapshotPublish {
                backend_instance,
                candidate_id,
                server_epoch,
                runtime_dir,
                server_pid,
                server_start_token,
            } => {
                let report = publish_snapshot_manifest(
                    config,
                    BackendInstanceUid(backend_instance),
                    &candidate_id,
                    &runtime_dir,
                    ServerEpoch(server_epoch),
                    server_pid,
                    &server_start_token,
                )?;
                println!("{}", serde_json::to_string(&report)?);
            }
        }
        Ok(())
    })();

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!(
                "{}",
                serde_json::json!({
                    "schema_version": 1,
                    "ok": false,
                    "error": {
                        "code": error.stable_code(),
                        "message": error.to_string(),
                    },
                })
            );
            ExitCode::from(recovery_error_exit(&error))
        }
    }
}

/// Public recovery control always executes at the backend owner. A remote
/// resume/abort first inspects the owner and then qualifies the mutation with
/// that exact backend-instance/epoch pair, so a restart between the two calls
/// is a stale-target refusal rather than an action against the replacement.
fn recovery_cmd(
    context: &Context,
    explicit_host: Option<&str>,
    format: Option<dmux::output::OutputFormat>,
    cmd: RecoveryCmd,
) -> ExitCode {
    use dmux::error::{ErrorCode, TypedError};

    let action = match &cmd {
        RecoveryCmd::Status => "recovery_status",
        RecoveryCmd::Resume => "recovery_resume",
        RecoveryCmd::Abort { .. } => "recovery_abort",
    };
    let json = format == Some(dmux::output::OutputFormat::Json);
    let host = explicit_host.unwrap_or_else(|| context.host.name());
    if let RecoveryCmd::Abort { yes } = &cmd
        && !*yes
    {
        // §7.4: a JSON destructive command never prompts. It answers with
        // the one confirmation document and changes nothing.
        if json {
            let (document, exit) =
                dmux::output::confirmation_required(action, host, production_authority_revision());
            println!("{document}");
            return ExitCode::from(exit.code());
        }
        if !io::stdin().is_terminal() {
            let error = TypedError::new(
                ErrorCode::ConfirmationRequired,
                "recovery abort requires confirmation (re-run with --yes)",
            );
            return refuse(action, format, error);
        }
        eprint!("Abort the failed recovery generation on {host}? [y/N] ");
        let _ = io::stderr().flush();
        let mut answer = String::new();
        if io::stdin().read_line(&mut answer).is_err() || !answer.trim().eq_ignore_ascii_case("y") {
            let error = TypedError::new(
                ErrorCode::ConfirmationDeclined,
                "recovery abort declined; nothing changed",
            );
            return refuse(action, format, error);
        }
    }

    match cmd {
        RecoveryCmd::Status => match recovery_inspection(explicit_host) {
            Ok(inspection) => {
                if json {
                    // The whole inspection, inside the same envelope every
                    // other bounded verb emits (§16.2): its own pre-P11
                    // three-field shape was the only one left.
                    emit_recovery_document(action, serde_json::json!(inspection));
                } else {
                    render_recovery_status(&inspection);
                }
                ExitCode::SUCCESS
            }
            Err(error) => refuse(action, format, error),
        },
        RecoveryCmd::Resume => recovery_receipt(
            action,
            format,
            "resume",
            recovery_control(
                explicit_host,
                dmux::remote::client::RecoveryOwnerCommand::Resume,
            ),
        ),
        RecoveryCmd::Abort { .. } => recovery_receipt(
            action,
            format,
            "abort",
            recovery_control(
                explicit_host,
                dmux::remote::client::RecoveryOwnerCommand::Abort,
            ),
        ),
    }
}

fn emit_recovery_document(action: &str, result: serde_json::Value) {
    let revision = production_authority_revision();
    println!(
        "{}",
        dmux::output::document(action, true, result, &[], revision)
    );
}

/// Resume and abort answer with the request they filed, in whichever shape
/// was asked for. Before P11 wired the global flag they printed the human
/// sentence under `--format json` too.
fn recovery_receipt(
    action: &str,
    format: Option<dmux::output::OutputFormat>,
    verb: &str,
    outcome: Result<dmux::recovery::RecoveryControlRequest, dmux::error::TypedError>,
) -> ExitCode {
    match outcome {
        Ok(receipt) => {
            if format == Some(dmux::output::OutputFormat::Json) {
                emit_recovery_document(
                    action,
                    serde_json::json!({
                        "request_uid": receipt.request_uid.to_string(),
                        "server_epoch": receipt.server_epoch.0.to_string(),
                    }),
                );
            } else {
                println!(
                    "{verb} requested for recovery {} at epoch {}",
                    receipt.request_uid, receipt.server_epoch.0
                );
            }
            ExitCode::SUCCESS
        }
        Err(error) => refuse(action, format, error),
    }
}

fn recovery_inspection(
    explicit_host: Option<&str>,
) -> Result<dmux::recovery::RecoveryInspection, dmux::error::TypedError> {
    use dmux::error::{ErrorCode, TypedError};
    use dmux::model::Backend;
    use dmux::registry::{Registry, RegistryConfig};
    use dmux::remote::client::{
        RecoveryOwnerCommand, RecoveryOwnerContext, RecoveryOwnerReply, call_recovery_owner,
    };

    let config = RegistryConfig::production().map_err(|error| {
        TypedError::new(
            ErrorCode::OperationFailed,
            format!("recovery registry paths: {error}"),
        )
    })?;
    let mut registry = Registry::open(config.clone()).map_err(TypedError::from)?;
    let identity = registry.identity().map_err(TypedError::from)?;
    let owner = match explicit_host {
        Some(host_ref) => dmux::remote::hosts::resolve_host(&registry, host_ref)?.host_uid,
        None => identity.host_uid,
    };
    if owner == identity.host_uid {
        let instance = registry
            .backend_instance_for_backend(Backend::Wez)
            .map_err(TypedError::from)?
            .ok_or_else(|| {
                TypedError::new(
                    ErrorCode::NotFound,
                    "this owner has no registered Wez backend instance",
                )
            })?;
        let runtime = dmux::runtime::dmux_runtime_dir().map_err(|error| {
            TypedError::new(
                ErrorCode::OperationFailed,
                format!("recovery runtime directory: {error}"),
            )
        })?;
        return dmux::recovery::inspect_recovery(config, &runtime, instance)
            .map_err(recovery_typed_error);
    }

    let outcome = call_recovery_owner(
        &mut registry,
        RecoveryOwnerContext::new(owner),
        RecoveryOwnerCommand::Status,
    )?;
    match outcome.reply {
        RecoveryOwnerReply::Status(inspection) => Ok(inspection),
        RecoveryOwnerReply::Control(_) => Err(TypedError::new(
            ErrorCode::ProtocolMismatch,
            "recovery status returned a control receipt",
        )),
    }
}

fn recovery_control(
    explicit_host: Option<&str>,
    command: dmux::remote::client::RecoveryOwnerCommand,
) -> Result<dmux::recovery::RecoveryControlRequest, dmux::error::TypedError> {
    use dmux::error::{ErrorCode, TypedError};
    use dmux::model::Backend;
    use dmux::recovery::{request_recovery_abort, request_recovery_resume};
    use dmux::registry::{Registry, RegistryConfig};
    use dmux::remote::client::{
        RecoveryOwnerCommand, RecoveryOwnerContext, RecoveryOwnerReply, call_recovery_owner,
    };

    let config = RegistryConfig::production().map_err(|error| {
        TypedError::new(
            ErrorCode::OperationFailed,
            format!("recovery registry paths: {error}"),
        )
    })?;
    let mut registry = Registry::open(config.clone()).map_err(TypedError::from)?;
    let identity = registry.identity().map_err(TypedError::from)?;
    let owner = match explicit_host {
        Some(host_ref) => dmux::remote::hosts::resolve_host(&registry, host_ref)?.host_uid,
        None => identity.host_uid,
    };
    if owner == identity.host_uid {
        let instance = registry
            .backend_instance_for_backend(Backend::Wez)
            .map_err(TypedError::from)?
            .ok_or_else(|| {
                TypedError::new(
                    ErrorCode::NotFound,
                    "this owner has no registered Wez backend instance",
                )
            })?;
        let runtime = dmux::runtime::dmux_runtime_dir().map_err(|error| {
            TypedError::new(
                ErrorCode::OperationFailed,
                format!("recovery runtime directory: {error}"),
            )
        })?;
        return match command {
            RecoveryOwnerCommand::Resume => {
                request_recovery_resume(config, &runtime, instance).map_err(recovery_typed_error)
            }
            RecoveryOwnerCommand::Abort => {
                request_recovery_abort(config, &runtime, instance).map_err(recovery_typed_error)
            }
            RecoveryOwnerCommand::Status => Err(TypedError::new(
                ErrorCode::Usage,
                "status is not a recovery control action",
            )),
        };
    }

    let status = call_recovery_owner(
        &mut registry,
        RecoveryOwnerContext::new(owner),
        RecoveryOwnerCommand::Status,
    )?;
    let inspection = match status.reply {
        RecoveryOwnerReply::Status(inspection) => inspection,
        RecoveryOwnerReply::Control(_) => {
            return Err(TypedError::new(
                ErrorCode::ProtocolMismatch,
                "recovery status returned a control receipt",
            ));
        }
    };
    let epoch = inspection.server_epoch.ok_or_else(|| {
        TypedError::new(
            ErrorCode::ProviderUnavailable,
            "remote Wez backend has no running server epoch",
        )
    })?;
    let outcome = call_recovery_owner(
        &mut registry,
        RecoveryOwnerContext::qualified(owner, inspection.backend_instance_uid, epoch),
        command,
    )?;
    match outcome.reply {
        RecoveryOwnerReply::Control(receipt) => Ok(receipt),
        RecoveryOwnerReply::Status(_) => Err(TypedError::new(
            ErrorCode::ProtocolMismatch,
            "recovery control returned a status payload",
        )),
    }
}

fn recovery_typed_error(error: dmux::recovery::RecoveryError) -> dmux::error::TypedError {
    use dmux::error::{ErrorCode, TypedError};
    use dmux::recovery::RecoveryError;

    let code = match &error {
        RecoveryError::Registry(inner) => inner.error_code(),
        RecoveryError::InvalidManifest(_) | RecoveryError::InvalidSnapshot(_) => ErrorCode::Usage,
        RecoveryError::NonEmpty(_) => ErrorCode::OperationInProgress,
        RecoveryError::FenceLost(_) => ErrorCode::BackendEpochChanged,
        RecoveryError::Protocol(_) => ErrorCode::ProtocolMismatch,
        RecoveryError::TimedOut(_) => ErrorCode::ProviderUnavailable,
        RecoveryError::Io(_) | RecoveryError::Json(_) | RecoveryError::Failed(_) => {
            ErrorCode::OperationFailed
        }
    };
    TypedError::new(code, error.to_string())
}

fn render_recovery_status(inspection: &dmux::recovery::RecoveryInspection) {
    let epoch = inspection
        .server_epoch
        .map(|value| value.0.to_string())
        .unwrap_or_else(|| "stopped".into());
    let durable = inspection
        .journal
        .iter()
        .find(|row| row.manifest_node_path == dmux::recovery::GENERATION_ROOT_PATH)
        .map(|row| row.node_state.as_str());
    let state = inspection
        .status
        .as_ref()
        .map(|status| match status.state {
            dmux::recovery::RecoveryStatusState::Starting => "starting",
            dmux::recovery::RecoveryStatusState::Recovering => "recovering",
            dmux::recovery::RecoveryStatusState::Ready => "ready",
            dmux::recovery::RecoveryStatusState::Failed => "failed",
            dmux::recovery::RecoveryStatusState::Aborted => "aborted",
        })
        .or(durable)
        .unwrap_or("idle");
    println!(
        "{state}\tinstance={}\tepoch={epoch}",
        inspection.backend_instance_uid.0
    );
    if let Some(status) = &inspection.status {
        if let Some(manifest) = &status.manifest_id {
            println!("manifest\t{manifest}");
        }
        if let Some(node) = &status.current_node {
            println!("node\t{node}");
        }
        if let Some(error) = &status.error {
            println!("error\t{error}");
        }
    }
}

fn recovery_error_exit(error: &dmux::recovery::RecoveryError) -> u8 {
    match error.stable_code() {
        "recovery_manifest_invalid" | "recovery_protocol_error" => 2,
        "operation_in_progress" | "recovery_ineligible" | "recovery_fence_lost" => 4,
        "recovery_timeout" => 6,
        _ => 1,
    }
}

/// `dmux _context` (plan §13.1): one validated marker JSON document on
/// stdout, or a typed error and no output. The shell prompt hook consumes
/// this; a failure must never fabricate markers.
fn context_cmd(data_dir: Option<String>, lock_dir: Option<String>) -> Result<ExitCode, String> {
    use dmux::backend::InventoryScope;
    use dmux::model::{Backend, SpaceUid};
    use dmux::operations::{self, OperationEnv};

    // Like the remote/bootstrap endpoints, this command may be invoked by a
    // non-interactive prompt wrapper with a POSIX locale.  tmux sanitizes the
    // provider's U+001F identity separators in that locale, so normalize
    // before the first provider subprocess is spawned.
    dmux::remote::normalize_utf8_locale();

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
        let (socket, epoch) = space_cli::verified_wez_target(&env, None).map_err(|e| e.message)?;
        let (bin, config) = space_cli::production_wez_paths();
        let provider = dmux::backend::wez::WezProvider::new(&bin, config);
        let scope = InventoryScope {
            backend: Backend::Wez,
            endpoint: socket,
            expected_epoch: Some(epoch),
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

/// Canonical non-destructive disconnect. Inside tmux the process itself is
/// the only trustworthy source of the invoking client identity, so the
/// native current-client path is used and `--domain` is meaningless. Inside
/// a managed Wez pane the signed GUI broker performs the owner/heartbeat
/// correlation. A headless invocation is an idempotent no-op.
fn disconnect(context: &Context, domain: bool, host_given: bool) -> Result<ExitCode, String> {
    if host_given || !context.local {
        eprintln!("dmux: disconnect operates on the invoking local client and rejects --host");
        return Ok(ExitCode::from(2));
    }
    if context.inside_tmux {
        if domain {
            eprintln!("dmux: --domain is only valid for an imported Wez domain");
            return Ok(ExitCode::from(2));
        }
        return attach::detach(context);
    }
    if context.inside_wezterm && std::env::var("DMUX_WEZ_FIRST").as_deref() == Ok("1") {
        let response =
            dmux::gui_cli::dispatch_ambient_production(&dmux::gui_cli::GuiCommand::Disconnect {
                domain,
            });
        let exit = response.exit_code();
        if response.ok {
            if let Some(message) = gui_response_message(&response) {
                println!("{message}");
            }
        } else {
            eprintln!(
                "dmux: {}",
                response
                    .message
                    .as_deref()
                    .unwrap_or("GUI disconnect failed closed")
            );
        }
        return Ok(ExitCode::from(exit));
    }
    if context.inside_wezterm {
        return attach::detach(context);
    }
    println!("nothing attached");
    Ok(ExitCode::SUCCESS)
}

fn gui_response_message(response: &dmux::gui_cli::GuiResponse) -> Option<&str> {
    response.message.as_deref().or_else(|| {
        response.result.as_ref().and_then(|result| {
            result
                .get("message")
                .or_else(|| result.get("hint"))
                .and_then(serde_json::Value::as_str)
        })
    })
}

/// Bootstrap a detached, bounded monitor without leaving a zombie beneath
/// the tmux/ssh process that replaces the public dmux caller. The bootstrap
/// is itself synchronously reaped by `connect_cli`; its forked worker is
/// orphaned when this function returns and owns no terminal descriptors.
fn gui_exec_finalize_cmd(raw_pending_uid: &str) -> ExitCode {
    let pending_uid = match uuid::Uuid::parse_str(raw_pending_uid) {
        Ok(uid) if uid.to_string() == raw_pending_uid => uid,
        _ => {
            return render_connect_error(dmux::error::TypedError::new(
                dmux::error::ErrorCode::InvalidRef,
                "GUI transition pending UID is not one canonical UUID",
            ));
        }
    };

    // SAFETY: this hidden bootstrap is a freshly exec'd, single-threaded dmux
    // process. The child immediately detaches and either runs the bounded
    // finalizer or exits via `_exit`; the parent performs no shared mutation.
    let worker = unsafe { libc::fork() };
    if worker < 0 {
        return render_connect_error(dmux::error::TypedError::new(
            dmux::error::ErrorCode::OperationFailed,
            "cannot fork the GUI transition finalizer",
        ));
    }
    if worker > 0 {
        return ExitCode::SUCCESS;
    }

    // SAFETY: this is the fork child described above. All inherited stdio is
    // `/dev/null`; a new session prevents reacquiring the invoking terminal.
    if unsafe { libc::setsid() } < 0 {
        let _ = dmux::gui_cli::cancel_correlated_gui_exec_transition_production(pending_uid);
        unsafe { libc::_exit(1) };
    }
    let outcome = dmux::gui_cli::finalize_correlated_gui_exec_transition_production(
        pending_uid,
        // Remote attach tokens remain valid for 60 seconds. The pending
        // record carries the plan-kind deadline (locals stay shorter); this
        // outer bound leaves a small post-attach hook margin without making
        // any monitor unbounded.
        std::time::Duration::from_secs(70),
    );
    let status = match outcome {
        Ok(_) => 0,
        Err(_) => {
            let _ = dmux::gui_cli::cancel_correlated_gui_exec_transition_production(pending_uid);
            1
        }
    };
    // SAFETY: the detached worker must not unwind through state copied by
    // fork or flush any inherited userspace buffers.
    unsafe { libc::_exit(status) }
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

fn other(
    explicit_host: Option<&str>,
    context: &Context,
    args: &[String],
) -> Result<ExitCode, String> {
    if wez_first_enabled() {
        return match args {
            [word] if word == "@prev" => Ok(connect_selector(
                context,
                explicit_host,
                dmux::connect_cli::ConnectSelector::Previous,
                None,
                None,
                false,
            )),
            [word, ..] if word == "@prev" => {
                Ok(render_connect_error(dmux::error::TypedError::new(
                    dmux::error::ErrorCode::Usage,
                    "`dmux -` takes no arguments",
                )))
            }
            [target] => Ok(connect_selector(
                context,
                explicit_host,
                dmux::connect_cli::ConnectSelector::Ref(target.clone()),
                None,
                None,
                false,
            )),
            [_, ..] => Ok(render_connect_error(dmux::error::TypedError::new(
                dmux::error::ErrorCode::Usage,
                format!(
                    "unexpected arguments: {}; use `dmux con --help` for child/backend options",
                    args.join(" ")
                ),
            ))),
            [] => Ok(render_connect_error(dmux::error::TypedError::new(
                dmux::error::ErrorCode::Usage,
                "unexpected arguments",
            ))),
        };
    }
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

/// The automatic policy that applies when neither switch is set — what an
/// ordinary shell on an ordinary host gets.
///
/// **The plan §21 step 9 cutover is this constant becoming `true`, and
/// nothing else.** Do not flip it before the step 7 canary and the full P11
/// gate pass (ADR 010 §2); flipping it early makes every host that never
/// opted in start creating Wez Spaces. Hosts already canarying under
/// `DMUX_WEZ_FIRST=1` see no change when it flips, hosts that stated
/// `DMUX_WEZ_FIRST=0` stay legacy across it, and `DMUX_LEGACY_POLICY=1` is
/// the emergency opt-out that reverses it for the one release the legacy path
/// is still shipped.
///
/// It is also only half of the cutover. This constant governs CLI invocations
/// in shells that never inherited the variable; the service units exporting
/// `DMUX_WEZ_FIRST=1` are what make the GUI and mux run managed, and without
/// that half `decide_backend` still picks tmux (ADR 010 §5).
const WEZ_FIRST_BY_DEFAULT: bool = false;

/// Whether this invocation gets the Wez-first surface and automatic policy.
///
/// Every gated arm calls this instead of reading the environment itself, so
/// the cutover stays the one constant above.
fn wez_first_enabled() -> bool {
    resolve_wez_first(
        std::env::var("DMUX_LEGACY_POLICY").ok().as_deref(),
        std::env::var("DMUX_WEZ_FIRST").ok().as_deref(),
    )
}

/// `DMUX_LEGACY_POLICY` beats `DMUX_WEZ_FIRST` beats the default.
///
/// That precedence is what makes the emergency opt-out usable at all: the
/// canary host exported `DMUX_WEZ_FIRST=1` into launchd/systemd months
/// earlier, and the escape hatch must not require finding and unsetting that
/// first. `DMUX_LEGACY_POLICY=1` alone returns the host to legacy tmux
/// creation (§21 rollback, "switch creation policy back to legacy tmux").
///
/// `DMUX_LEGACY_POLICY` is an opt-in switch whose only recognised value is
/// `"1"`. `DMUX_WEZ_FIRST` is three-valued: `"1"` states Wez-first, `"0"`
/// states legacy, and anything else — unset, empty, `"yes"` — states no
/// preference and defers to the default. The `"0"` arm is not decoration
/// today, when it agrees with the default: it is what stops the flip from
/// re-reading an existing `DMUX_WEZ_FIRST=0` as an opt-*in*. Every other read
/// site tests `== '1'`, so `0` is already off everywhere else
/// (`shared/wezterm/mux/dmux-mux-start.sh:55` defaults it to `0`;
/// `shared/zsh/conf.d/94-dmux-context.zsh:16` treats `!= 1` as off), and the
/// resolver would have been the one place it meant the opposite (ADR 010 §5).
fn resolve_wez_first(legacy_policy: Option<&str>, wez_first: Option<&str>) -> bool {
    resolve_wez_first_with_default(legacy_policy, wez_first, WEZ_FIRST_BY_DEFAULT)
}

/// The resolver with the §21 step 9 default injected rather than read, so the
/// post-flip half of the truth table is testable before the flip. Production
/// has exactly one caller, above; the tests are the reason it takes an
/// argument at all.
fn resolve_wez_first_with_default(
    legacy_policy: Option<&str>,
    wez_first: Option<&str>,
    wez_first_by_default: bool,
) -> bool {
    if legacy_policy == Some("1") {
        return false;
    }
    match wez_first {
        Some("1") => true,
        Some("0") => false,
        _ => wez_first_by_default,
    }
}

fn legacy_host(raw: &str) -> Option<Host> {
    match raw {
        "macie" => Some(Host::Macie),
        "archie" => Some(Host::Archie),
        _ => None,
    }
}

fn authority_host_selector(raw: &str) -> dmux::connect_cli::HostSelector {
    use dmux::connect_cli::HostSelector;

    if let Ok(uid) = uuid::Uuid::parse_str(raw)
        && raw == uid.to_string()
    {
        return HostSelector::Uid(dmux::model::HostUid(uid));
    }
    if let (Some(requested), Ok(local)) = (legacy_host(raw), Host::this()) {
        let alias = if requested == local { "a" } else { "b" };
        return HostSelector::AliasOrLabel(alias.to_string());
    }
    HostSelector::AliasOrLabel(raw.to_string())
}

fn new_command(_context: &Context, explicit_host: Option<&str>, args: NewCliArgs) -> ExitCode {
    use dmux::error::{ErrorCode, TypedError};
    use dmux::new_cli::{NewFailure, NewOutcome, NewRequest, plan_new_production};

    let backend_constraint = args.backend.and_then(NewBackend::constraint);
    if args.allow_name_collision && backend_constraint.is_none() {
        return render_connect_error(TypedError::new(
            ErrorCode::Usage,
            "--allow-name-collision requires explicit --backend wez|tmux",
        ));
    }
    if args.launch_gui && args.no_connect {
        return render_connect_error(TypedError::new(
            ErrorCode::Usage,
            "--launch-gui conflicts with --no-connect",
        ));
    }
    if args.launch_gui && backend_constraint == Some(dmux::model::Backend::Tmux) {
        return render_connect_error(TypedError::new(
            ErrorCode::Usage,
            "--launch-gui is valid only with the Wez backend",
        ));
    }
    if attach::dry_run() {
        return render_connect_error(TypedError::new(
            ErrorCode::Usage,
            "DMUX_DRY_RUN cannot preview a Wez-first new operation without risking identity reservation, native creation, presentation, or bearer minting",
        ));
    }
    let request = NewRequest {
        name: args.name,
        explicit_host: explicit_host.map(authority_host_selector),
        backend_constraint,
        cwd: args.dir,
        no_connect: args.no_connect,
        allow_name_collision: args.allow_name_collision,
        launch_gui: args.launch_gui,
        program: args.command,
    };
    match plan_new_production(&request) {
        Ok(NewOutcome::Completed { result, .. }) => {
            render_new_receipt(&result);
            ExitCode::SUCCESS
        }
        Ok(NewOutcome::Exec { plan, .. }) => exec_owner_plan(*plan),
        Err(NewFailure { error, result }) => {
            if let Some(result) = result {
                render_new_receipt(&result);
                eprintln!("dmux: {}", error.message);
                ExitCode::from(7)
            } else {
                render_connect_error(error)
            }
        }
    }
}

fn render_new_receipt(result: &dmux::new_cli::NewReceipt) {
    println!(
        "{}\tbackend={}\tcreated={}\tconnected={}\treplayed={}",
        result.stable_ref, result.backend, result.created, result.connected, result.replayed
    );
}

fn connect_command(
    context: &Context,
    explicit_host: Option<&str>,
    args: ConnectCliArgs,
) -> ExitCode {
    use dmux::connect_cli::{ConnectSelector, parse_requested_child};
    use dmux::error::{ErrorCode, TypedError};
    use dmux::model::ChildKind;

    if args.create {
        return render_connect_error(TypedError::new(
            ErrorCode::Usage,
            "`con --create` is disabled in Wez-first mode; use the explicit `dmux new` policy",
        ));
    }
    if args.window.is_some() {
        return render_connect_error(TypedError::new(
            ErrorCode::Usage,
            "native window selectors are disabled in Wez-first mode; use --group or --split",
        ));
    }
    if args.launch_gui && args.backend == Some(ConnectBackend::Tmux) {
        return render_connect_error(TypedError::new(
            ErrorCode::Usage,
            "--launch-gui is valid only for a Wez Space",
        ));
    }
    let child = match (args.group.as_deref(), args.split.as_deref()) {
        (Some(group), None) => parse_requested_child(group, ChildKind::Group).map(Some),
        (None, Some(split)) => parse_requested_child(split, ChildKind::Split).map(Some),
        (None, None) => Ok(None),
        (Some(_), Some(_)) => Err(TypedError::new(
            ErrorCode::Usage,
            "--group and --split are mutually exclusive",
        )),
    };
    let child = match child {
        Ok(value) => value,
        Err(error) => return render_connect_error(error),
    };
    let selector = match (args.target, args.name) {
        (Some(target), None) => ConnectSelector::Ref(target),
        (None, Some(name)) => ConnectSelector::ExactName(name),
        _ => {
            return render_connect_error(TypedError::new(
                ErrorCode::Usage,
                "provide exactly one Space ref or --name",
            ));
        }
    };
    connect_selector(
        context,
        explicit_host,
        selector,
        args.backend.map(Into::into),
        child,
        args.launch_gui,
    )
}

fn connect_selector(
    _context: &Context,
    explicit_host: Option<&str>,
    selector: dmux::connect_cli::ConnectSelector,
    backend_constraint: Option<dmux::model::Backend>,
    child: Option<dmux::connect_cli::RequestedChild>,
    launch_gui: bool,
) -> ExitCode {
    use dmux::connect_cli::{ConnectOutcome, ConnectRequest, plan_connect_production};

    if attach::dry_run() {
        return render_connect_error(dmux::error::TypedError::new(
            dmux::error::ErrorCode::Usage,
            "DMUX_DRY_RUN cannot preview a Wez-first connection without performing presentation or minting a bearer token",
        ));
    }
    let request = ConnectRequest {
        selector,
        explicit_host: explicit_host.map(authority_host_selector),
        backend_constraint,
        child,
        launch_gui,
    };
    let outcome = match plan_connect_production(&request) {
        Ok(outcome) => outcome,
        Err(error) => return render_connect_error(error),
    };
    match outcome {
        ConnectOutcome::Completed(_) => ExitCode::SUCCESS,
        ConnectOutcome::Exec(plan) => exec_owner_plan(plan),
    }
}

/// The single feature-on terminal handoff boundary shared by public Connect
/// and New. It captures the GUI source, stages a post-attach transition, and
/// commits terminal history before the validated argv is consumed. Global
/// GUI history moves only when the detached monitor proves the hook-published
/// destination after exec.
fn exec_owner_plan(plan: dmux::connect_cli::OwnerExecPlan) -> ExitCode {
    let _handoff = match dmux::connect_cli::prepare_production_exec_handoff(&plan) {
        Ok(handoff) => handoff,
        Err(error) => return render_connect_error(error),
    };
    attach::exec_plan(plan.into_argv(), &[])
}

fn render_connect_error(error: dmux::error::TypedError) -> ExitCode {
    eprintln!("dmux: {}", error.message);
    ExitCode::from(error.code.exit_status().code())
}

/// Case 43: exactly one §16.2 document per `--format json` invocation,
/// refusals included, so stdout is either one document or empty. Human mode
/// keeps the one-line diagnostic on stderr.
fn refuse(
    action: &str,
    format: Option<dmux::output::OutputFormat>,
    error: dmux::error::TypedError,
) -> ExitCode {
    if format != Some(dmux::output::OutputFormat::Json) {
        return render_connect_error(error);
    }
    println!(
        "{}",
        dmux::output::document(
            action,
            false,
            serde_json::Value::Null,
            std::slice::from_ref(&error),
            production_authority_revision(),
        )
    );
    ExitCode::from(error.code.exit_status().code())
}

/// The verbs that have no bounded JSON document. §16.2 makes interactive
/// attach the rule: `con`, `new`, the bare picker and the `dmux <name>`
/// fallthrough all end in a terminal handoff, and `keys`/`ssh`/`disconnect`
/// report on this process rather than on authority state. The hidden `_`
/// service surfaces are deliberately absent — each answers its own frozen
/// protocol envelope and never reads `--format`.
fn unbounded_json_verb(command: Option<&Cmd>, host_given: bool) -> Option<&'static str> {
    match command {
        // Bare `dmux` is `ls` only on a pipe; otherwise it is the picker or
        // a plain attach of the named host.
        None => (host_given || io::stdout().is_terminal()).then_some("connect"),
        Some(Cmd::Con { .. } | Cmd::Other(_)) => Some("connect"),
        Some(Cmd::New { .. }) => Some("new"),
        Some(Cmd::Keys { .. }) => Some("keys"),
        Some(Cmd::Ssh { .. }) => Some("ssh"),
        Some(Cmd::Disconnect { .. }) => Some("disconnect"),
        _ => None,
    }
}

/// The `authority_revision` a document carries when the command holds no
/// registry handle of its own (plan §16.2). It reads the head of a registry
/// that is already there and answers 0 for one that is absent or unreadable,
/// rather than creating an authority store to fill an output field.
pub(crate) fn production_authority_revision() -> u64 {
    use dmux::registry::{Registry, RegistryConfig};

    let Some(db_path) = dmux::registry::production_db_path().filter(|path| path.exists()) else {
        return 0;
    };
    dmux::runtime::dmux_runtime_dir()
        .ok()
        .and_then(|lock_dir| Registry::open(RegistryConfig::new(&db_path, lock_dir)).ok())
        .and_then(|registry| registry.authority_head().ok())
        .map_or(0, |head| head.revision)
}

/// Wez-first library commands answer with the plan's typed exit table
/// (§16.3); the process carries only the number.
fn exit_code(status: dmux::error::ExitStatus) -> ExitCode {
    ExitCode::from(status.code())
}

/// The single `ls` entry point, so the `Ls` arm and bare `dmux` on a pipe
/// cannot drift apart (`cli::bare_dmux_on_a_pipe_is_ls`).
fn ls(
    context: &Context,
    format: Option<dmux::output::OutputFormat>,
    args: dmux::ls_cli::LsArgs,
) -> Result<ExitCode, String> {
    if wez_first_enabled() && dmux::ls_cli::IMPLEMENTED {
        return Ok(exit_code(dmux::ls_cli::run(format, args)));
    }
    if args.all_hosts || args.backend.is_some() || args.tree || format.is_some() {
        return Ok(refuse(
            "list",
            format,
            dmux::error::TypedError::new(
                dmux::error::ErrorCode::Usage,
                "--all-hosts/--backend/--tree/--format require DMUX_WEZ_FIRST=1",
            ),
        ));
    }
    list::run(
        context,
        args.only_tmux,
        args.only_wez,
        args.json,
        args.names,
    )
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

fn bare(
    host_given: bool,
    context: &Context,
    format: Option<dmux::output::OutputFormat>,
) -> Result<ExitCode, String> {
    if host_given {
        return attach::bare(context);
    }
    if !io::stdout().is_terminal() {
        return ls(context, format, dmux::ls_cli::LsArgs::default());
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

    /// The §21 step 9 resolver across every value either switch can hold.
    /// Written against `WEZ_FIRST_BY_DEFAULT` rather than against `true`/
    /// `false`, so the table stays correct after the flip: the rows that
    /// change meaning are exactly the ones where nobody stated a preference.
    #[test]
    fn the_policy_resolver_answers_every_switch_combination() {
        // `DMUX_LEGACY_POLICY=1` is the emergency opt-out: legacy wins over
        // an opt-in, over a stale `0`, and over the default whichever way it
        // is set.
        for wez_first in [None, Some("1"), Some("0"), Some(""), Some("yes")] {
            assert!(
                !resolve_wez_first(Some("1"), wez_first),
                "DMUX_LEGACY_POLICY=1 must force legacy with DMUX_WEZ_FIRST={wez_first:?}"
            );
        }
        // Without the opt-out, `DMUX_WEZ_FIRST` decides whenever it states
        // something: `1` on, `0` off. Only the spellings that state nothing
        // defer to the default, and those are the only rows the flip moves.
        for legacy in [None, Some("0"), Some(""), Some("yes")] {
            assert!(
                resolve_wez_first(legacy, Some("1")),
                "the canary opt-in must survive DMUX_LEGACY_POLICY={legacy:?}"
            );
            assert!(
                !resolve_wez_first(legacy, Some("0")),
                "DMUX_WEZ_FIRST=0 is an opt-out with DMUX_LEGACY_POLICY={legacy:?}"
            );
            assert_eq!(resolve_wez_first(legacy, None), WEZ_FIRST_BY_DEFAULT);
            assert_eq!(resolve_wez_first(legacy, Some("")), WEZ_FIRST_BY_DEFAULT);
            assert_eq!(resolve_wez_first(legacy, Some("yes")), WEZ_FIRST_BY_DEFAULT);
        }
    }

    /// The same table evaluated against both settings of the §21 step 9
    /// default, which is the only way to prove the `0` arm before the flip:
    /// with the default still legacy, `DMUX_WEZ_FIRST=0` and "unset" agree by
    /// accident, and a resolver that ignores `0` passes every assertion
    /// above. Flip the injected default and they part company — `0` stays
    /// legacy, unset becomes Wez-first — which is exactly the mid-canary
    /// surprise this exists to prevent, since all 28 Lua/zsh/shell read sites
    /// test `== '1'` and would go on treating `0` as off.
    #[test]
    fn an_explicit_zero_survives_the_flip_as_an_opt_out() {
        for wez_first_by_default in [false, true] {
            let resolve = |legacy, wez_first| {
                resolve_wez_first_with_default(legacy, wez_first, wez_first_by_default)
            };
            for legacy in [None, Some("0"), Some(""), Some("yes")] {
                assert!(
                    !resolve(legacy, Some("0")),
                    "DMUX_WEZ_FIRST=0 must be legacy with default \
                     {wez_first_by_default} and DMUX_LEGACY_POLICY={legacy:?}"
                );
                assert!(resolve(legacy, Some("1")));
                // Stating nothing is what the flip moves, and all of it.
                assert_eq!(resolve(legacy, None), wez_first_by_default);
                assert_eq!(resolve(legacy, Some("")), wez_first_by_default);
            }
            // The emergency opt-out keeps beating both spellings after the
            // flip; it is the §21 rollback, not a tie-break.
            for wez_first in [None, Some("1"), Some("0"), Some(""), Some("yes")] {
                assert!(!resolve(Some("1"), wez_first));
            }
        }
        // And the shipped resolver is the `WEZ_FIRST_BY_DEFAULT` row of that
        // table, so the two tests cannot drift apart.
        assert_eq!(
            resolve_wez_first(None, None),
            resolve_wez_first_with_default(None, None, WEZ_FIRST_BY_DEFAULT)
        );
    }

    /// The property the opt-out exists for: a host that canaried under
    /// `DMUX_WEZ_FIRST=1` escapes by setting one variable, without first
    /// hunting down the `launchctl setenv` it did in W6 (ADR 010 §2).
    #[test]
    fn the_legacy_opt_out_beats_an_opt_in_that_is_still_exported() {
        assert!(resolve_wez_first(None, Some("1")));
        assert!(!resolve_wez_first(Some("1"), Some("1")));
    }

    /// Guards the flip itself. §21 step 9 is gated on the step 7 canary and
    /// the full P11 gate, so a host that stated no preference still gets
    /// legacy. This one assertion is the second and last line the cutover
    /// edits (`assert!(resolve_wez_first(None, None))`), which is also why it
    /// goes through the resolver rather than reading the constant: an
    /// `assert!` on a `const` is a clippy warning and folds away.
    #[test]
    fn the_shipped_default_is_still_legacy_until_step_9() {
        assert!(!resolve_wez_first(None, None));
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

    #[test]
    fn gui_internal_preserves_the_origin_and_trailing_verb_argv() {
        let parsed = Cli::try_parse_from([
            "dmux",
            "_gui",
            "--origin-json",
            r#"{"protocol_version":1}"#,
            "split-resize",
            "--direction",
            "left",
            "--amount",
            "3",
        ])
        .unwrap();
        let Some(Cmd::GuiInternal { origin_json, argv }) = parsed.command else {
            panic!("expected hidden GUI command")
        };
        assert_eq!(origin_json.as_deref(), Some(r#"{"protocol_version":1}"#));
        assert_eq!(
            argv,
            ["split-resize", "--direction", "left", "--amount", "3"]
        );
    }

    #[test]
    fn gui_internal_allows_empty_argv_for_one_json_usage_response() {
        let parsed = Cli::try_parse_from(["dmux", "_gui"]).unwrap();
        let Some(Cmd::GuiInternal { origin_json, argv }) = parsed.command else {
            panic!("expected hidden GUI command")
        };
        assert!(origin_json.is_none());
        assert!(argv.is_empty());
    }

    #[test]
    fn gui_exec_finalizer_accepts_only_the_fixed_pending_uid_surface() {
        let uid = "11111111-1111-4111-8111-111111111111";
        let parsed =
            Cli::try_parse_from(["dmux", "_gui-exec-finalize", "--pending-uid", uid]).unwrap();
        assert!(matches!(
            parsed.command,
            Some(Cmd::GuiExecFinalize { pending_uid }) if pending_uid == uid
        ));
        assert!(
            Cli::try_parse_from(["dmux", "_gui-exec-finalize", "--pending-uid", uid, "extra",])
                .is_err()
        );
    }

    #[test]
    fn public_recovery_contract_parses_status_resume_and_confirmed_abort() {
        let parsed = Cli::try_parse_from([
            "dmux", "--host", "archie", "recovery", "status", "--format", "json",
        ])
        .unwrap();
        assert_eq!(parsed.format, Some(dmux::output::OutputFormat::Json));
        assert!(matches!(
            parsed.command,
            Some(Cmd::Recovery {
                cmd: RecoveryCmd::Status
            })
        ));

        let parsed = Cli::try_parse_from(["dmux", "recovery", "resume"]).unwrap();
        assert!(matches!(
            parsed.command,
            Some(Cmd::Recovery {
                cmd: RecoveryCmd::Resume
            })
        ));

        let parsed = Cli::try_parse_from(["dmux", "recovery", "abort", "--yes"]).unwrap();
        assert!(matches!(
            parsed.command,
            Some(Cmd::Recovery {
                cmd: RecoveryCmd::Abort { yes: true }
            })
        ));
    }

    /// Case 24 asks for four *documented* listing scopes. A constant nobody
    /// renders documents nothing, so the assertion is on the help clap
    /// actually prints for `dmux ls`.
    #[test]
    fn ls_long_help_renders_every_documented_scope() {
        let mut root = Cli::command();
        root.build();
        let mut ls = root
            .get_subcommands()
            .find(|command| command.get_name() == "ls")
            .expect("ls is a subcommand")
            .clone();
        let help = ls.render_long_help().to_string();
        for phrase in [
            "dmux ls --tree",
            "dmux ls --all-hosts",
            "dmux host ls",
            "hosts and their routes only, never Spaces",
            "--all-hosts controls host breadth",
        ] {
            assert!(
                help.contains(phrase),
                "`dmux ls --help` is missing {phrase:?}:\n{help}"
            );
        }
    }

    /// §17.13: under the canary a bare digit is a permanent SpaceNo, so
    /// `rm`'s help must not keep calling one an index — that is the exact
    /// confusion case 44 exists to prevent, printed by `dmux rm --help`.
    #[test]
    fn rm_help_calls_a_bare_digit_a_ref_not_a_row_index() {
        let mut root = Cli::command();
        root.build();
        let mut rm = root
            .get_subcommands()
            .find(|command| command.get_name() == "rm")
            .expect("rm is a subcommand")
            .clone();
        let help = rm.render_long_help().to_string();
        assert!(
            help.contains("permanent SpaceNo"),
            "`dmux rm --help` must say what a bare digit is:\n{help}"
        );
        assert!(
            !help.contains("names or indices"),
            "`dmux rm --help` still calls a bare digit an index:\n{help}"
        );
    }

    /// §7.4: `rm --all` is "every Space on exactly one selected host",
    /// and under the gate `all_spaces` sweeps both backends with no
    /// exclusion for the caller's own session — the pre-gate tmux path is
    /// the only one that keeps it, so the help may not promise it outright.
    #[test]
    fn rm_all_help_does_not_promise_an_unconditional_current_session_escape() {
        let mut root = Cli::command();
        root.build();
        let mut rm = root
            .get_subcommands()
            .find(|command| command.get_name() == "rm")
            .expect("rm is a subcommand")
            .clone();
        let help = rm.render_long_help().to_string();
        assert!(
            help.contains("exactly one host"),
            "`dmux rm --help` must state the single-host scope of --all:\n{help}"
        );
        assert!(
            !help.contains("(keeps the one this client is in)"),
            "`dmux rm --help` still promises --all spares this client's session:\n{help}"
        );
    }

    #[test]
    fn disconnect_is_canonical_and_detach_is_its_compatible_alias() {
        for command in ["disconnect", "detach"] {
            let parsed = Cli::try_parse_from(["dmux", command, "--domain"]).unwrap();
            assert!(matches!(
                parsed.command,
                Some(Cmd::Disconnect { domain: true })
            ));
        }
    }

    #[test]
    fn disconnect_renders_the_nothing_else_to_present_domain_hint() {
        let response = dmux::gui_cli::GuiResponse::success(serde_json::json!({
            "outcome": "nothing_else_to_present",
            "hint": "use --domain to detach the current imported domain",
        }));
        assert_eq!(
            gui_response_message(&response),
            Some("use --domain to detach the current imported domain")
        );
    }
}
