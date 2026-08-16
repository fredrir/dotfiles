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

    /// Internal: registry-only guarded cold-recovery service surface
    /// (plan §15.3). The coordinator deliberately rejects inherited mux
    /// endpoint variables before taking any recovery fence.
    #[command(name = "_recovery", hide = true)]
    RecoveryInternal {
        #[command(subcommand)]
        cmd: RecoveryInternalCmd,
    },

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
        manifest_dir: PathBuf,

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
        candidate: PathBuf,

        #[arg(long)]
        destination: PathBuf,
    },
}

#[derive(Subcommand)]
enum RecoveryCmd {
    /// Show the current owner's durable recovery state.
    Status {
        #[arg(long, value_enum, default_value_t = RecoveryFormat::Human)]
        format: RecoveryFormat,
    },

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
enum RecoveryFormat {
    Human,
    Json,
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
    let dynamic_host_command = wez_first_enabled()
        && matches!(
            &cli.command,
            Some(Cmd::Con { .. })
                | Some(Cmd::New { .. })
                | Some(Cmd::Recovery { .. })
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
        None => bare(host_given, &context),
        Some(Cmd::Ls {
            tmux,
            wez,
            json,
            names,
        }) => list::run(&context, tmux, wez, json, names),
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
        Some(Cmd::Recovery { cmd }) => Ok(recovery_cmd(&context, cli.host.as_deref(), cmd)),
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
        Some(Cmd::BridgeKey) => (|| {
            let runtime = dmux::runtime::dmux_runtime_dir().map_err(|e| e.to_string())?;
            dmux::gui::ensure_bridge_key(&runtime).map_err(|e| e.to_string())?;
            Ok(ExitCode::SUCCESS)
        })(),
        Some(Cmd::GuiInternal { origin_json, argv }) => Ok(ExitCode::from(
            dmux::gui_cli::run_production_argv(origin_json.as_deref(), &argv),
        )),
        Some(Cmd::RecoveryInternal { cmd }) => Ok(recovery_internal_cmd(cmd)),
        Some(Cmd::TmuxBootstrap {
            namespace,
            data_dir,
            lock_dir,
        }) => tmux_bootstrap_cmd(namespace, data_dir, lock_dir),
        Some(Cmd::Group { cmd }) => space_cli::group(cmd),
        Some(Cmd::Split { cmd }) => space_cli::split(cmd),
        Some(Cmd::Context { cmd }) => space_cli::context(cmd),
        Some(Cmd::Repair { cmd }) => space_cli::repair(cmd),
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
        RecoveryCoordinatorOptions, ensure_wez_backend_instance, publish_snapshot_manifest,
        run_recovery_coordinator,
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
                manifest_dir,
                server_pid,
                server_start_token,
                helper_bin,
                resume_failed,
                abort_failed,
            } => {
                let mut options = RecoveryCoordinatorOptions::new(
                    config,
                    runtime_dir,
                    manifest_dir,
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
                candidate,
                destination,
            } => {
                let report = publish_snapshot_manifest(
                    config,
                    BackendInstanceUid(backend_instance),
                    &candidate,
                    &destination,
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
fn recovery_cmd(context: &Context, explicit_host: Option<&str>, cmd: RecoveryCmd) -> ExitCode {
    use dmux::error::{ErrorCode, TypedError};

    let json = matches!(
        cmd,
        RecoveryCmd::Status {
            format: RecoveryFormat::Json
        }
    );
    if let RecoveryCmd::Abort { yes } = &cmd
        && !*yes
    {
        if !io::stdin().is_terminal() {
            let error = TypedError::new(
                ErrorCode::ConfirmationRequired,
                "recovery abort requires confirmation (re-run with --yes)",
            );
            return render_recovery_error(error, false);
        }
        eprint!(
            "Abort the failed recovery generation on {}? [y/N] ",
            explicit_host.unwrap_or_else(|| context.host.name())
        );
        let _ = io::stderr().flush();
        let mut answer = String::new();
        if io::stdin().read_line(&mut answer).is_err() || !answer.trim().eq_ignore_ascii_case("y") {
            let error = TypedError::new(
                ErrorCode::ConfirmationDeclined,
                "recovery abort declined; nothing changed",
            );
            return render_recovery_error(error, false);
        }
    }

    match cmd {
        RecoveryCmd::Status { format } => match recovery_inspection(explicit_host) {
            Ok(inspection) => {
                if format == RecoveryFormat::Json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "schema_version": 1,
                            "ok": true,
                            "result": inspection,
                        })
                    );
                } else {
                    render_recovery_status(&inspection);
                }
                ExitCode::SUCCESS
            }
            Err(error) => render_recovery_error(error, json),
        },
        RecoveryCmd::Resume => {
            match recovery_control(
                explicit_host,
                dmux::remote::client::RecoveryOwnerCommand::Resume,
            ) {
                Ok(receipt) => {
                    println!(
                        "resume requested for recovery {} at epoch {}",
                        receipt.request_uid, receipt.server_epoch.0
                    );
                    ExitCode::SUCCESS
                }
                Err(error) => render_recovery_error(error, false),
            }
        }
        RecoveryCmd::Abort { .. } => {
            match recovery_control(
                explicit_host,
                dmux::remote::client::RecoveryOwnerCommand::Abort,
            ) {
                Ok(receipt) => {
                    println!(
                        "abort requested for recovery {} at epoch {}",
                        receipt.request_uid, receipt.server_epoch.0
                    );
                    ExitCode::SUCCESS
                }
                Err(error) => render_recovery_error(error, false),
            }
        }
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

fn render_recovery_error(error: dmux::error::TypedError, json: bool) -> ExitCode {
    let exit = error.code.exit_status().code();
    if json {
        println!(
            "{}",
            serde_json::json!({
                "schema_version": 1,
                "ok": false,
                "errors": [error],
            })
        );
    } else {
        eprintln!("dmux: {}", error.message);
    }
    ExitCode::from(exit)
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

fn wez_first_enabled() -> bool {
    std::env::var("DMUX_WEZ_FIRST").as_deref() == Ok("1")
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
        Ok(NewOutcome::Exec { plan, .. }) => {
            if let Err(error) = dmux::connect_cli::commit_production_exec_history(&plan) {
                return render_connect_error(error);
            }
            attach::exec_plan(plan.into_argv(), &[])
        }
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
    use dmux::connect_cli::{
        ConnectOutcome, ConnectRequest, commit_production_exec_history, plan_connect_production,
    };

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
        ConnectOutcome::Exec(plan) => {
            if let Err(error) = commit_production_exec_history(&plan) {
                return render_connect_error(error);
            }
            attach::exec_plan(plan.into_argv(), &[])
        }
    }
}

fn render_connect_error(error: dmux::error::TypedError) -> ExitCode {
    eprintln!("dmux: {}", error.message);
    ExitCode::from(error.code.exit_status().code())
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
    fn public_recovery_contract_parses_status_resume_and_confirmed_abort() {
        let parsed = Cli::try_parse_from([
            "dmux", "--host", "archie", "recovery", "status", "--format", "json",
        ])
        .unwrap();
        assert!(matches!(
            parsed.command,
            Some(Cmd::Recovery {
                cmd: RecoveryCmd::Status {
                    format: RecoveryFormat::Json
                }
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
