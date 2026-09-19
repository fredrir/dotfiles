use std::ffi::OsString;
use std::io::IsTerminal;

use clap::CommandFactory;

use crate::cli::SyncCli;
use crate::context::Context;

/// `./setup.sh --macos` spelled a profile as a flag. The set of real flags comes
/// from the parser itself, so a new sync option can never be read as a profile.
pub fn normalize(arguments: Vec<OsString>) -> Vec<OsString> {
    let flags: Vec<String> = SyncCli::command()
        .get_arguments()
        .flat_map(|argument| {
            argument
                .get_long()
                .into_iter()
                .chain(argument.get_all_aliases().into_iter().flatten())
                .map(|long| format!("--{long}"))
                .collect::<Vec<_>>()
        })
        .chain(["--help".to_string(), "--version".to_string()])
        .collect();
    let mut normalized = Vec::with_capacity(arguments.len());
    for argument in arguments {
        // `./setup.sh --macos -- -n` used to need the separator; sync takes both directly.
        if argument == "--" {
            continue;
        }
        let profile = argument.to_str().is_some_and(|text| {
            text.starts_with("--") && !text.contains('=') && !flags.iter().any(|f| f == text)
        });
        if profile {
            let text = argument.to_str().unwrap_or_default();
            normalized.push(OsString::from(text.trim_start_matches('-')));
        } else {
            normalized.push(argument);
        }
    }
    normalized
}

/// First reconcile on a machine picks a profile and its machine overrides; every
/// later one reuses what it saved. An explicit profile never prompts.
pub fn resolve(context: &Context, cli: &mut SyncCli) -> Result<(), String> {
    if cli.profile.is_some() || context.profile(None).is_ok() {
        return Ok(());
    }
    if !interactive() {
        let profiles = context.profiles()?;
        return Err(format!(
            "no profile saved for this machine; pass one of:\n  {}",
            profiles.join("\n  ")
        ));
    }
    let relevant = crate::config::profiles::relevant(context)?;
    if relevant.is_empty() {
        return Err(format!(
            "no relevant installed environment found; pass one of:\n  {}",
            context.profiles()?.join("\n  ")
        ));
    }
    let default = (std::env::consts::OS == "macos").then(|| "macos".to_string());
    let profile = pick("select environment", &relevant, default.as_deref())?
        .ok_or_else(|| "cancelled".to_string())?;
    cli.overrides.extend(overrides(context, &profile)?);
    cli.profile = Some(profile);
    Ok(())
}

fn overrides(context: &Context, profile: &str) -> Result<Vec<String>, String> {
    let manifest = context.environment_dir.join(profile).join("manifest");
    let saved = crate::config::load_overrides(&context.overrides_file)?;
    let mut selected = Vec::new();
    for group in crate::config::read_manifest(&manifest)? {
        let directory = context.root.join(&group).join("overrides");
        if !directory.is_dir() {
            continue;
        }
        let mut names: Vec<String> = crate::config::sorted_directories(&directory)?
            .iter()
            .filter_map(|path| path.file_name()?.to_str().map(str::to_string))
            .collect();
        if names.is_empty() {
            continue;
        }
        names.push("none".to_string());
        let title = format!("select machine override for {group}");
        let chosen = pick(&title, &names, saved.get(&group).map(String::as_str))?
            .ok_or_else(|| "cancelled".to_string())?;
        selected.push(format!("{group}={chosen}"));
    }
    Ok(selected)
}

/// Restores or creates this machine's age identity before the first sync needs
/// to read a secret. A wrong passphrase leaves the secrets sealed; Ctrl-C stops sync.
pub fn identity(context: &Context, dry_run: bool) -> Result<(), String> {
    use crate::secret::cli::{Args, Command};
    if dry_run
        || !interactive()
        || crate::secret::vault::identity_path(context).is_file()
        || !context.root.join(".sops.yaml").is_file()
    {
        return Ok(());
    }
    let command = if crate::secret::wrap::available(context).is_some() {
        Command::Unwrap
    } else if context.program("age-keygen").is_some() {
        Command::Init
    } else {
        return Ok(());
    };
    println!();
    let result = crate::secret::run(
        Args {
            command: Some(command),
        },
        context,
    );
    println!();
    match result {
        Err(error) if crate::cancel::requested() => Err(error),
        Err(error) => {
            eprintln!("dotfile: {error}");
            Ok(())
        }
        Ok(_) => Ok(()),
    }
}

fn pick(title: &str, options: &[String], default: Option<&str>) -> Result<Option<String>, String> {
    use ui_picker::{Item, Outcome, Picker};
    let style = ui_theme::Style::for_stdout();
    let mut picker = Picker::new(
        title,
        options
            .iter()
            .enumerate()
            .map(|(index, label)| Item::new(index, label)),
        &style,
    );
    if let Some(index) = default.and_then(|value| options.iter().position(|o| o == value)) {
        picker = picker.initial_focus(&index);
    }
    match picker.run().map_err(|error| error.to_string())? {
        Outcome::Selected(selected) => Ok(selected.first().map(|index| options[*index].clone())),
        Outcome::Unavailable => Err("terminal unavailable".to_string()),
        Outcome::Cancelled | Outcome::Interrupted => Ok(None),
    }
}

fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}
