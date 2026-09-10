mod catalog;
mod plan;
mod report;
mod runner;

use std::ffi::OsString;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use workstation::Completions;

#[derive(Parser)]
#[command(
    name = "dotfile dev",
    version,
    disable_help_subcommand = true,
    about = "Test and lint the repository"
)]
pub struct Cli {
    #[command(flatten)]
    completions: Completions,
    #[arg(long, hide = true, exclusive = true)]
    list_packages: bool,
    #[arg(long, hide = true, exclusive = true)]
    list_languages: bool,
    #[command(subcommand)]
    action: Option<Action>,
}

#[derive(Subcommand)]
enum Action {
    #[command(about = "Run tests")]
    Test(Options),
    #[command(about = "Run linters")]
    Lint(Options),
    #[command(about = "Run linters and tests")]
    Check(Options),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum Language {
    #[value(alias = "rs")]
    Rust,
    #[value(alias = "py")]
    Python,
    #[value(alias = "js")]
    Javascript,
    Lua,
    #[value(alias = "sh", alias = "zsh", alias = "bash")]
    Shell,
    Toml,
    #[value(alias = "yml")]
    Yaml,
    #[value(alias = "jsonc")]
    Json,
}

impl Language {
    fn name(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Python => "python",
            Self::Javascript => "javascript",
            Self::Lua => "lua",
            Self::Shell => "shell",
            Self::Toml => "toml",
            Self::Yaml => "yaml",
            Self::Json => "json",
        }
    }
}

#[derive(Args)]
struct Options {
    #[arg(
        short = 'p',
        long = "pkg",
        value_name = "TARGET",
        value_delimiter = ',',
        help = "Select packages; repeat or comma-separate"
    )]
    packages: Vec<String>,
    #[arg(
        short = 'l',
        long = "lang",
        value_name = "LANGUAGE",
        value_enum,
        value_delimiter = ',',
        help = "Select languages; repeat or comma-separate"
    )]
    languages: Vec<Language>,
    #[arg(short = 'n', long, help = "Show commands without running them")]
    dry_run: bool,
    #[arg(
        short,
        long,
        help = "Show commands, live tool output, and per-task timings"
    )]
    verbose: bool,
    #[arg(short = 'j', long, value_name = "N", value_parser = clap::value_parser!(u16).range(1..), help = "Total worker budget; defaults to CPU count")]
    jobs: Option<u16>,
    #[arg(long, default_value = "2", value_name = "N", value_parser = clap::value_parser!(u16).range(1..), help = "Maximum simultaneous tasks")]
    concurrency: u16,
    #[arg(
        last = true,
        value_name = "ARGS",
        help = "Forward arguments to one runner; select one language and action"
    )]
    arguments: Vec<OsString>,
}

pub fn dispatch(arguments: impl IntoIterator<Item = OsString>) -> ExitCode {
    let cli = match Cli::try_parse_from(
        std::iter::once(OsString::from("dotfile dev")).chain(arguments),
    ) {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            return workstation::exit_code(code);
        }
    };
    if let Some(code) = cli.completions.emit::<Cli>("dotfile dev") {
        return code;
    }
    if cli.list_languages {
        for language in Language::value_variants() {
            println!("{}", language.name());
        }
        return ExitCode::SUCCESS;
    }
    if cli.list_packages {
        return match package_names() {
            Ok(names) => {
                for name in names {
                    println!("{name}");
                }
                ExitCode::SUCCESS
            }
            Err(error) => workstation::fail("dotfile dev", error),
        };
    }
    let Some(action) = cli.action else {
        use clap::CommandFactory;
        let _ = Cli::command().print_help();
        println!();
        return ExitCode::SUCCESS;
    };
    match execute(action) {
        Ok(code) => code,
        Err(error) => workstation::fail("dotfile dev", error),
    }
}

fn package_names() -> Result<std::collections::BTreeSet<String>, String> {
    let root = doc_keybinds::root(None)?;
    let catalog = catalog::Catalog::read(&root, true)?;
    let mut names = std::collections::BTreeSet::new();
    names.extend(catalog.rust.iter().map(|package| package.name.clone()));
    names.extend(catalog.python);
    names.extend(
        catalog
            .files
            .iter()
            .filter(|file| catalog::language(file).is_some())
            .map(|file| catalog::package(file).to_string()),
    );
    Ok(names)
}

fn execute(action: Action) -> Result<ExitCode, String> {
    let (test, lint, options) = match action {
        Action::Test(options) => (true, false, options),
        Action::Lint(options) => (false, true, options),
        Action::Check(options) => (true, true, options),
    };
    let _signals =
        workstation::screen::SignalGuard::with_options(workstation::screen::SignalOptions {
            reraise_on_drop: false,
            ..Default::default()
        })
        .map_err(|error| error.to_string())?;
    let root = doc_keybinds::root(None)?;
    let catalog = catalog::Catalog::read(&root, lint)?;
    let mut tasks = plan::tasks(&root, &catalog, &options, test, lint, 1)?;
    let budget = runner::Budget::new(&options, &tasks);
    for task in &mut tasks {
        task.workers = budget.workers;
    }
    if options.dry_run {
        for task in &tasks {
            println!("{}: {}", task.name, task.display());
        }
        return Ok(ExitCode::SUCCESS);
    }
    let action = match (test, lint) {
        (true, true) => "check",
        (true, false) => "test",
        _ => "lint",
    };
    runner::run(tasks, budget, action, options.verbose)
}
