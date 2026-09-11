use std::ffi::OsString;

use clap::{Parser, ValueEnum};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum Resolution {
    #[default]
    Skip,
    Repo,
    Live,
}

#[derive(Clone, Debug, Parser)]
#[command(
    name = "dotfile sync",
    version,
    about = "Reconcile the repository, generated metadata, and this workstation"
)]
pub struct SyncCli {
    #[arg(
        value_name = "PROFILE",
        help = "Profile to reconcile; the saved profile by default"
    )]
    pub profile: Option<String>,

    #[arg(
        short = 'n',
        long = "dry-run",
        help = "Plan without changing files or contacting the peer"
    )]
    pub dry_run: bool,

    #[arg(
        long = "override",
        value_name = "GROUP=NAME",
        help = "Select a machine override with GROUP=NAME|none"
    )]
    pub overrides: Vec<String>,

    #[arg(
        long,
        conflicts_with = "resolve",
        help = "Resolve local edits from the repository; discard remote edits with --push"
    )]
    pub force: bool,

    #[arg(
        long,
        value_enum,
        default_value_t,
        help = "Choose how locally edited merged configs are settled"
    )]
    pub resolve: Resolution,

    #[arg(short = 'p', long, help = "Push commits, then pull and sync the peer")]
    pub push: bool,

    #[arg(long, value_name = "HOST", help = "Select the peer; implies --push")]
    pub to: Option<String>,

    #[arg(
        short = 'v',
        long,
        help = "Show every link, merge, generated file, and remote action"
    )]
    pub verbose: bool,
}

impl SyncCli {
    pub fn parse_tail(arguments: impl IntoIterator<Item = OsString>) -> Result<Self, clap::Error> {
        let values = std::iter::once(OsString::from("dotfile sync")).chain(arguments);
        workstation::cli::try_parse_from(values)
    }
}

#[derive(Parser)]
#[command(name = "dotfile", version, about = "The dotfile manager")]
pub struct Cli {
    #[command(flatten)]
    pub completions: Completions,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(clap::Args)]
pub struct Completions {
    #[arg(long = "completions", value_name = "SHELL", exclusive = true, value_parser = ["zsh"], help = "Generate zsh completions")]
    pub shell: Option<String>,
    #[arg(long = "command-dump", exclusive = true, hide = true)]
    pub dump: bool,
}

#[derive(clap::Subcommand)]
pub enum Command {
    #[command(about = "Reconcile the repository, generated metadata, and this workstation")]
    Sync(SyncCli),
    #[command(about = "Test and lint the repository")]
    Dev(crate::dev::Cli),
    #[command(about = "Generate and check repository documentation")]
    Docs(crate::docs::Args),
    #[command(about = "Keep private material out of the repository")]
    Secret(crate::secret::Args),
    #[command(about = "Manage root-owned system files")]
    System(crate::system::Args),
    #[command(about = "Generate application themes")]
    Theme(crate::theme::Args),
    #[command(about = "Move a live config into the repository and symlink it back")]
    Add(crate::manage::AddArgs),
    #[command(about = "Move a tracked path out of the repository and keep it live")]
    Remove(crate::manage::RemoveArgs),
    #[command(about = "Check the profile, links, tools and packages")]
    Doctor(crate::doctor::Args),
    #[command(hide = true)]
    Profiles {
        #[arg(long)]
        relevant: bool,
    },
    #[command(hide = true)]
    Link(LinkArgs),
    #[command(hide = true)]
    Completions {
        #[arg(
            long,
            value_name = "DIRECTORY",
            conflicts_with = "program",
            required_unless_present = "program"
        )]
        dir: Option<std::path::PathBuf>,
        #[arg(
            long,
            value_name = "NAME",
            conflicts_with = "dir",
            required_unless_present = "dir"
        )]
        program: Option<String>,
        #[arg(long, default_value = "zsh", value_parser = ["zsh"])]
        shell: String,
    },
    #[command(name = "__complete", hide = true)]
    Complete {
        source: String,
        arguments: Vec<String>,
    },
    #[command(external_subcommand)]
    External(Vec<OsString>),
}

#[derive(clap::Args)]
pub struct LinkArgs {
    pub profile: Option<String>,
    #[arg(short = 'n', long)]
    pub dry_run: bool,
    #[arg(long = "override")]
    pub overrides: Vec<String>,
    #[arg(long, conflicts_with = "resolve")]
    pub force: bool,
    #[arg(long, value_enum, default_value_t)]
    pub resolve: Resolution,
}

pub fn dispatch(arguments: Vec<OsString>) -> std::process::ExitCode {
    if arguments.first().and_then(|a| a.to_str()) == Some("sync")
        && let Some(code) = crate::wire::dispatch(&arguments[1..])
    {
        return code;
    }
    let original_arguments = arguments.clone();
    use std::process::ExitCode;
    let cli = match workstation::cli::try_parse_from::<Cli, _, _>(
        std::iter::once(OsString::from("dotfile")).chain(arguments),
    ) {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            return workstation::exit_code(code);
        }
    };
    if cli.completions.dump {
        let document = workstation::surface::Document {
            version: workstation::surface::VERSION,
            command: crate::surface::metadata::native(),
        };
        return match serde_json::to_string(&document) {
            Ok(document) => {
                println!("{document}");
                ExitCode::SUCCESS
            }
            Err(error) => workstation::fail("dotfile", error),
        };
    }
    if cli.completions.shell.is_some() {
        return match crate::surface::completions::emit(clap_complete::Shell::Zsh) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("dotfile: {error}");
                ExitCode::from(2)
            }
        };
    }
    let Some(command) = cli.command else {
        let _ = workstation::cli::command::<Cli>().print_help();
        println!();
        return ExitCode::SUCCESS;
    };
    if let Command::External(arguments) = command {
        return external(arguments);
    }
    if let Command::Dev(cli) = command {
        return crate::dev::run(cli);
    }
    if let Command::Sync(cli) = command {
        return synchronize(cli, original_arguments);
    }
    crate::cancel::reset();
    let _signals = match ui_terminal::SignalGuard::with_options(ui_terminal::SignalOptions {
        cancellation: Some(crate::cancel::flag()),
        reraise_on_drop: false,
        ..Default::default()
    }) {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("dotfile: {error}");
            return ExitCode::FAILURE;
        }
    };
    let result = crate::context::Context::discover().and_then(|context| execute(command, &context));
    if crate::cancel::requested() {
        let signal = crate::cancel::signal();
        return workstation::exit_code(if signal > 0 { 128 + signal } else { 130 });
    }
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("dotfile: {error}");
            ExitCode::FAILURE
        }
    }
}

fn execute(
    command: Command,
    context: &crate::context::Context,
) -> Result<std::process::ExitCode, String> {
    use std::process::ExitCode;
    match command {
        Command::Secret(args) => crate::secret::run(args, context),
        Command::System(args) => crate::system::run(args, context),
        Command::Theme(args) => crate::theme::run(args, context),
        Command::Add(args) => crate::manage::add(args, context),
        Command::Remove(args) => crate::manage::remove(args, context),
        Command::Doctor(args) => crate::doctor::run(args, context),
        Command::Profiles { relevant } => {
            let profiles = if relevant {
                crate::config::profiles::relevant(context)?
            } else {
                context.profiles()?
            };
            for name in profiles {
                println!("{name}");
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Complete { source, arguments } => {
            for line in crate::surface::values::lines(context, &source, &arguments) {
                println!("{line}");
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Completions {
            dir,
            program,
            shell,
        } => {
            if let Some(dir) = dir {
                let count = crate::surface::completions::write_all(context, &dir)?;
                println!(
                    "  {count} tools completed by {}",
                    dir.join("tools-completion.zsh").display()
                );
            } else if let Some(program) = program {
                crate::surface::completions::emit_program(context, &program, &shell)?;
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Link(args) => {
            let profile = context.profile(args.profile.as_deref())?;
            let cli = SyncCli {
                profile: Some(profile.clone()),
                dry_run: args.dry_run,
                overrides: args.overrides,
                force: args.force,
                resolve: args.resolve,
                push: false,
                to: None,
                verbose: false,
            };
            let _lock = if cli.dry_run {
                None
            } else {
                Some(crate::lock::MutationLock::acquire(context)?)
            };
            if cli.dry_run {
                println!("  would: link {profile}");
            }
            let (client, server) = crate::decision::channel();
            let (sender, receiver) = crossbeam_channel::bounded(256);
            let context = context.clone();
            let worker = std::thread::spawn(move || {
                crate::sync::engine::reconcile(&context, &profile, &cli, &client, &sender)
            });
            if let Err(error) = crate::ui::run(receiver, server, worker, true) {
                println!("  conflicts: {error}");
                return Err(error);
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Sync(_) => Err("sync dispatch unavailable".to_string()),
        Command::Dev(cli) => Ok(crate::dev::run(cli)),
        Command::Docs(args) => crate::docs::run(context, args),
        Command::External(arguments) => Ok(external(arguments)),
    }
}

fn external(arguments: Vec<OsString>) -> std::process::ExitCode {
    use std::process::{Command, ExitCode};
    let Some(name) = arguments.first() else {
        return ExitCode::SUCCESS;
    };
    let special = match name.to_str() {
        Some("status") => {
            Some("'status' is included in 'dotfile doctor'; run that instead.".to_string())
        }
        Some(name @ "packages") => Some(format!(
            "'{name}' is included in 'dotfile sync'; run that instead."
        )),
        _ => None,
    };
    let mut program = OsString::from("dotfile-");
    program.push(name);
    let mut command = Command::new(&program);
    command.args(&arguments[1..]);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = command.exec();
        if error.kind() != std::io::ErrorKind::NotFound {
            eprintln!("dotfile: {}: {error}", program.to_string_lossy());
            return ExitCode::FAILURE;
        }
    }
    #[cfg(not(unix))]
    if let Ok(status) = command.status() {
        return workstation::exit_code(status.code().unwrap_or(1));
    }
    if let Some(message) = special {
        eprintln!("dotfile: {message}");
    } else {
        eprintln!(
            "dotfile: No such command '{}'. Run ./setup.sh",
            name.to_string_lossy()
        );
    }
    ExitCode::from(2)
}

fn synchronize(cli: SyncCli, original_arguments: Vec<OsString>) -> std::process::ExitCode {
    let refresh = match crate::tooling::pending(&cli) {
        Ok(refresh) => refresh,
        Err(error) => return failure(error),
    };
    if let Some(refresh) = refresh {
        crate::cancel::reset();
        let (sender, receiver) = crossbeam_channel::bounded(256);
        let (_decision_client, decision_server) = crate::decision::channel();
        let update = refresh.clone();
        let worker = std::thread::spawn(move || update.run(&sender));
        if let Err(error) = crate::ui::run(receiver, decision_server, worker, cli.verbose) {
            return failure(error);
        }
        return match refresh.reexec(&original_arguments) {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => failure(error),
        };
    }
    let verbose = cli.verbose;
    crate::cancel::reset();
    let (sender, receiver) = crossbeam_channel::bounded(256);
    let (decision_client, decision_server) = crate::decision::channel();
    let worker = std::thread::spawn(move || crate::sync::run(&cli, &sender, &decision_client));
    match crate::ui::run(receiver, decision_server, worker, verbose) {
        Ok(summary) => {
            println!("{}", crate::ui::completion_line(&summary));
            std::process::ExitCode::SUCCESS
        }
        Err(error) => failure(error),
    }
}

fn failure(error: String) -> std::process::ExitCode {
    eprintln!("dotfile: {error}");
    if let Some(code) = crate::ui::signal_exit_code() {
        std::process::ExitCode::from(code)
    } else if crate::cancel::requested() {
        std::process::ExitCode::from(130)
    } else {
        std::process::ExitCode::FAILURE
    }
}
