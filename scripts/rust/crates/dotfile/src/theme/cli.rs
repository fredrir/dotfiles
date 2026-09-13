use super::{
    Result, emitters,
    model::Repository,
    plan, preview,
    selection::{self, Selection},
};
use crate::{context::Context, lock::MutationLock};
use clap::Subcommand;
use std::{
    fs,
    io::Write,
    process::{ExitCode, Stdio},
};
#[derive(Clone, Debug, clap::Args)]
#[command(about = "Stamp selected theme profiles into generated configuration files")]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<Command>,
}
#[derive(Clone, Debug, Subcommand)]
pub enum Command {
    #[command(about = "Regenerate generated theme configs")]
    Sync,
    #[command(about = "Print a dry run of sync")]
    Dry,
    #[command(about = "Validate every profile and resolved application color pair")]
    Check,
    #[command(about = "Print a resolved contrast matrix")]
    Contrast { profile: Option<String> },
    #[command(about = "Show resolved profiles and drift")]
    Status,
    #[command(about = "Preview a profile")]
    Preview { profile: Option<String> },
    #[command(about = "Browse shared UI components and themes")]
    Gallery { profile: Option<String> },
    #[command(about = "Assign a profile globally, to one group, or to one package")]
    Switch {
        profile: Option<String>,
        scope: Option<String>,
    },
    #[command(about = "Print the files this generator owns")]
    Outputs {
        #[arg(long)]
        staged: bool,
    },
    #[command(hide = true)]
    Profiles,
    #[command(hide = true)]
    Palette {
        profile: Option<String>,
        #[arg(long)]
        json: bool,
    },
}
pub fn run(args: Args, context: &Context) -> Result<ExitCode> {
    if args.command.is_none() && !preview::interactive() {
        let command = <Args as clap::Args>::augment_args(clap::Command::new("dotfile theme"));
        ui_cli::decorate(command)
            .print_help()
            .map_err(|error| error.to_string())?;
        println!();
        return Ok(ExitCode::SUCCESS);
    }
    if matches!(args.command, Some(Command::Profiles)) {
        for name in super::model::profile_names(&context.root)? {
            println!("{name}");
        }
        return Ok(ExitCode::SUCCESS);
    }
    let _lock = if matches!(args.command, Some(Command::Sync | Command::Switch { .. })) {
        Some(MutationLock::acquire(context)?)
    } else {
        None
    };
    let repo = Repository::load(&context.root, &context.root_config)?;
    if let Some(Command::Check) = args.command {
        super::validate::all(&repo)?;
        println!("  {} profiles valid", repo.themes.len());
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(Command::Contrast { profile }) = &args.command {
        let names = profile
            .as_ref()
            .map_or_else(|| repo.names(), |s| vec![s.clone()]);
        let reports = names
            .iter()
            .map(|name| {
                super::validate::matrix(repo.theme(name)?).map(|s| s.trim_end().to_string())
            })
            .collect::<Result<Vec<_>>>()?;
        println!("{}", reports.join("\n\n"));
        return Ok(ExitCode::SUCCESS);
    }
    let targets = emitters::targets(&repo)?;
    if let Some(Command::Outputs { staged }) = args.command {
        for target in &targets {
            if !staged || target.staged {
                println!("{}", target.path);
            }
        }
        return Ok(ExitCode::SUCCESS);
    }
    let selection = Selection::load(&repo, &targets)?;
    let Some(command) = args.command else {
        drop(_lock);
        return match preview::choose(&repo, &selection, &targets, "")? {
            Some(command) => run(
                Args {
                    command: Some(command),
                },
                context,
            ),
            None => Ok(ExitCode::SUCCESS),
        };
    };
    match command {
        Command::Sync | Command::Dry | Command::Status => {
            let dry = !matches!(command, Command::Sync);
            let changes = plan::generate(&repo, &targets, &selection)?;
            if matches!(command, Command::Status) {
                preview::status(&repo, &selection, &targets, &changes)?;
            } else {
                if !dry {
                    plan::apply(context, &changes, None)?;
                }
                plan::print(&changes, dry);
            }
            if matches!(command, Command::Dry) && !changes.is_empty() {
                return Ok(ExitCode::FAILURE);
            }
        }
        Command::Preview { profile } => {
            let name = match profile {
                Some(name) => name,
                None if !preview::interactive() => selection.default().into(),
                None => {
                    let choice = preview::choose(&repo, &selection, &targets, "preview")?;
                    return match choice {
                        Some(command) => run(
                            Args {
                                command: Some(command),
                            },
                            context,
                        ),
                        None => Ok(ExitCode::SUCCESS),
                    };
                }
            };
            let theme = repo.theme(&name)?;
            super::validate::validate(theme)?;
            preview::show(theme, &selection, &targets)?;
        }
        Command::Switch { profile, scope } => {
            let Some(profile) = profile else {
                if !preview::interactive() {
                    return Err(format!(
                        "a profile is required (available: {})",
                        repo.names().join(", ")
                    ));
                }
                drop(_lock);
                let choice = preview::choose(&repo, &selection, &targets, "switch")?;
                return match choice {
                    Some(command) => run(
                        Args {
                            command: Some(command),
                        },
                        context,
                    ),
                    None => Ok(ExitCode::SUCCESS),
                };
            };
            repo.theme(&profile)?;
            let (group, key, global) =
                selection::scope(scope.as_deref().unwrap_or("shared"), &targets)?;
            if global && !selection.overrides().is_empty() {
                println!("  drops the assignments below:");
                for (g, k) in selection.overrides() {
                    println!(
                        "      {} = {}",
                        if k == "theme" {
                            g.clone()
                        } else {
                            format!("{g}/{k}")
                        },
                        selection.current(&g, &k)
                    );
                }
                if preview::interactive() && !preview::confirm("drop them?")? {
                    return Ok(ExitCode::SUCCESS);
                }
            }
            let path = repo.root_config.join("profiles.dotfile");
            let source = fs::read_to_string(&path).map_err(|e| e.to_string())?;
            let candidate =
                selection::switched(&source, &selection, &group, &key, global, &profile);
            let candidate = if candidate == source {
                candidate
            } else {
                formatted(context, &candidate)?
            };
            let selected = Selection::parse(&repo, &targets, &candidate)?;
            let changes = plan::generate(&repo, &targets, &selected)?;
            plan::apply(
                context,
                &changes,
                (candidate != source).then_some(candidate.as_str()),
            )?;
            println!(
                "  {} → {profile}",
                if global {
                    "global".into()
                } else if key == "theme" {
                    group
                } else {
                    format!("{group}/{key}")
                }
            );
            plan::print(&changes, false);
            if changes
                .iter()
                .any(|c| c.path.starts_with("linux/kde/plasma/"))
            {
                println!(
                    "      plasma reads its own copy: systemctl --user restart plasma-plasmashell"
                );
            }
        }
        Command::Palette { profile, .. } => {
            let theme = repo.theme(
                profile
                    .as_deref()
                    .unwrap_or(selection.current("shared", "ui")),
            )?;
            print!("{}", emitters::ui::render(theme)?);
        }
        Command::Gallery { profile } => {
            let palette = profile
                .as_deref()
                .map(|name| emitters::ui::palette(repo.theme(name)?))
                .transpose()?;
            ui_gallery::run(palette).map_err(|error| error.to_string())?;
        }
        Command::Check | Command::Contrast { .. } | Command::Outputs { .. } | Command::Profiles => {
            unreachable!("handled before selection")
        }
    }
    Ok(ExitCode::SUCCESS)
}
fn formatted(context: &Context, text: &str) -> Result<String> {
    use std::io::{Seek, SeekFrom};
    let attempt = || -> std::io::Result<crate::process::CapturedOutput> {
        let mut input = tempfile::tempfile()?;
        input.write_all(text.as_bytes())?;
        input.seek(SeekFrom::Start(0))?;
        let mut command = context.command("dotfmt");
        command
            .args(["--stdin", "config/profiles.dotfile"])
            .current_dir(&context.root)
            .stdin(Stdio::from(input));
        crate::process::output(
            &mut command,
            crate::process::CaptureLimits {
                stdout: 1_048_576,
                stderr: 16_384,
            },
            std::time::Duration::from_secs(2),
        )
    };
    let output = attempt();
    crate::cancel::check()?;
    Ok(match output {
        Ok(output) if output.status.success() && !output.stdout.is_empty() => {
            String::from_utf8(output.stdout).unwrap_or_else(|_| text.into())
        }
        _ => text.into(),
    })
}
