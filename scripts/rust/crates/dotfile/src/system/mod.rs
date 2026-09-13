//! Inspection and explicit installation of root-owned configuration files.
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use crate::config::{Configuration, PackageKind};
use crate::context::Context;
use crate::event::VecSink;
use crate::secret::vault::{self, SecretEntry, SecretKind, Variables};
use clap::{Args as ClapArgs, Subcommand};

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Compare tracked system files with what is installed.
    Status,
    /// Show what would change on disk, without touching anything.
    Diff { path: Option<String> },
    /// Install tracked system files to their destinations as root.
    Install {
        #[arg(short = 'n', long)]
        dry_run: bool,
        #[arg(long)]
        yes: bool,
    },
    /// Copy a root-owned file into the repository.
    Add {
        path: PathBuf,
        #[arg(long, required = true)]
        pkg: String,
        #[arg(long, default_value = "linux/arch")]
        group: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum State {
    Current,
    Absent,
    Sealed,
    Drifted,
    Failed,
    Unresolved,
    Refused,
    Unreadable,
}

impl State {
    fn label(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Absent => "absent",
            Self::Sealed => "sealed",
            Self::Drifted => "drifted",
            Self::Failed => "failed",
            Self::Unresolved => "unresolved",
            Self::Refused => "refused",
            Self::Unreadable => "unreadable",
        }
    }
    fn blocked(self) -> bool {
        matches!(
            self,
            Self::Drifted | Self::Failed | Self::Unresolved | Self::Refused | Self::Unreadable
        )
    }
}

struct Inspected {
    entry: SecretEntry,
    state: State,
    detail: String,
    wanted: Option<Vec<u8>>,
    current: Option<Vec<u8>>,
}

pub fn run(args: Args, context: &Context) -> Result<ExitCode, String> {
    let Some(command) = args.command else {
        workstation::cli::decorate(<Args as clap::Args>::augment_args(clap::Command::new(
            "dotfile system",
        )))
        .print_help()
        .map_err(|e| e.to_string())?;
        println!();
        return Ok(ExitCode::SUCCESS);
    };
    let _lock = if matches!(
        command,
        Command::Add { .. } | Command::Install { dry_run: false, .. }
    ) {
        Some(crate::lock::MutationLock::acquire(context)?)
    } else {
        None
    };
    if let Command::Add { path, pkg, group } = command {
        return add(context, &path, &pkg, &group);
    }
    let configuration =
        Configuration::load(context, &context.profile(None)?, &[], &VecSink::default())?;
    let mut entries = Vec::new();
    for package in &configuration.packages {
        if package.kind == PackageKind::System {
            entries.extend(vault::package_entries(&configuration, package, true)?);
        }
    }
    entries.sort_by(|left, right| left.destination.cmp(&right.destination));
    if entries.is_empty() {
        println!("no system files tracked");
        return Ok(ExitCode::SUCCESS);
    }
    let variables = vault::load_variables(context);
    if !variables.note.is_empty() {
        println!("  {}", variables.note);
    }
    let results = entries
        .into_iter()
        .map(|entry| inspect(context, entry, &variables))
        .collect::<Vec<_>>();
    match command {
        Command::Status => {
            for result in &results {
                show(result);
            }
            println!("{}", counted(&results));
            Ok(if results.iter().any(|entry| entry.state.blocked()) {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            })
        }
        Command::Diff { path } => {
            diff(&results, path.as_deref());
            Ok(ExitCode::SUCCESS)
        }
        Command::Install { dry_run, yes } => install(context, &results, dry_run, yes),
        Command::Add { .. } => unreachable!(),
    }
}

fn private(entry: &SecretEntry) -> bool {
    !matches!(entry.kind, SecretKind::Plain)
}

fn mode(entry: &SecretEntry) -> u32 {
    if private(entry) {
        return 0o600;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::metadata(&entry.source)
            .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
        {
            return 0o755;
        }
    }
    0o644
}

use crate::fs::resolved;

fn refusal(context: &Context, destination: &Path) -> Result<(), String> {
    if !destination.is_absolute() {
        return Err("destination is not absolute".into());
    }
    if destination.parent().is_none() {
        return Err("destination is the filesystem root".into());
    }
    if destination
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err("destination contains parent traversal".into());
    }
    let actual = resolved(destination)?;
    if actual.starts_with(resolved(&context.home)?) {
        return Err("destination is under $HOME; use dotfile sync".into());
    }
    if actual.starts_with(resolved(&context.root)?) {
        return Err("destination is inside the repository".into());
    }
    if fs::symlink_metadata(destination).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err("destination is a symlink".into());
    }
    if fs::metadata(destination).is_ok_and(|metadata| !metadata.is_file()) {
        return Err("destination is not a regular file".into());
    }
    let top = destination.components().take(2).collect::<PathBuf>();
    if !top.is_dir() {
        return Err(format!("{} does not exist", top.display()));
    }
    Ok(())
}

fn inspect(context: &Context, entry: SecretEntry, variables: &Variables) -> Inspected {
    let mut result = Inspected {
        entry,
        state: State::Current,
        detail: String::new(),
        wanted: None,
        current: None,
    };
    if let Err(error) = refusal(context, &result.entry.destination) {
        result.state = State::Refused;
        result.detail = error;
        return result;
    }
    let wanted = if private(&result.entry) {
        vault::produce(context, &result.entry, variables)
    } else {
        fs::read(&result.entry.source).map_err(|error| error.to_string())
    };
    match wanted {
        Ok(data) => result.wanted = Some(data),
        Err(error) => {
            result.state = if matches!(result.entry.kind, SecretKind::Template) && variables.ok {
                State::Unresolved
            } else if !vault::identity_path(context).is_file() && private(&result.entry) {
                State::Sealed
            } else {
                State::Failed
            };
            // Decryption tools may include fragments of private data in error text.
            result.detail = if private(&result.entry) {
                "private content could not be rendered".into()
            } else {
                error
            };
            return result;
        }
    }
    match fs::read(&result.entry.destination) {
        Ok(data) => result.current = Some(data),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            result.state = State::Absent;
            return result;
        }
        Err(error) => {
            result.state =
                if error.kind() == std::io::ErrorKind::PermissionDenied && sealed(&result.entry) {
                    State::Sealed
                } else {
                    State::Unreadable
                };
            return result;
        }
    }
    if result.current != result.wanted {
        result.state = State::Drifted;
    }
    #[cfg(unix)]
    if let Ok(metadata) = fs::metadata(&result.entry.destination) {
        use std::os::unix::fs::MetadataExt;
        let actual = metadata.mode() & 0o7777;
        let wanted = mode(&result.entry);
        if actual != wanted {
            result.state = State::Drifted;
            result.detail = format!("mode {actual:04o}, want {wanted:04o}");
        }
        if metadata.uid() != 0 || metadata.gid() != 0 {
            result.state = State::Drifted;
            if !result.detail.is_empty() {
                result.detail.push_str("; ");
            }
            result.detail.push_str(&format!(
                "owner {}:{}, want root:root",
                metadata.uid(),
                metadata.gid()
            ));
        }
    }
    result
}

fn sealed(entry: &SecretEntry) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(&entry.destination).is_ok_and(|metadata| {
            sealed_parts(
                private(entry),
                metadata.mode(),
                metadata.uid(),
                metadata.gid(),
            )
        })
    }
    #[cfg(not(unix))]
    {
        let _ = entry;
        false
    }
}

fn sealed_parts(private: bool, mode: u32, uid: u32, gid: u32) -> bool {
    private && mode & 0o7777 == 0o600 && uid == 0 && gid == 0
}

fn show(result: &Inspected) {
    println!(
        "  {:<10} {}{}",
        result.state.label(),
        result.entry.destination.display(),
        if result.detail.is_empty() {
            String::new()
        } else {
            format!("  {}", result.detail)
        }
    );
}

fn counted(results: &[Inspected]) -> String {
    let mut counts = BTreeMap::<&str, usize>::new();
    for result in results {
        *counts.entry(result.state.label()).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(state, count)| format!("{count} {state}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn diff(results: &[Inspected], filter: Option<&str>) {
    let mut shown = false;
    for result in results {
        if filter.is_some_and(|filter| {
            !result.entry.source.to_string_lossy().contains(filter)
                && !result.entry.destination.to_string_lossy().contains(filter)
        }) {
            continue;
        }
        if result.wanted.is_none()
            || matches!(
                result.state,
                State::Refused | State::Unreadable | State::Sealed
            )
        {
            show(result);
            shown = true;
            continue;
        }
        if result.current == result.wanted {
            continue;
        }
        shown = true;
        if private(&result.entry) {
            println!(
                "{}: private rendered content differs",
                result.entry.destination.display()
            );
            continue;
        }
        println!(
            "--- {}\n+++ {}",
            result.entry.destination.display(),
            result.entry.source.display()
        );
        let before = String::from_utf8_lossy(result.current.as_deref().unwrap_or_default());
        let after = String::from_utf8_lossy(result.wanted.as_deref().unwrap_or_default());
        // A complete replacement hunk remains a valid unified diff, including empty files.
        println!(
            "@@ -{},{} +{},{} @@",
            usize::from(!before.is_empty()),
            before.lines().count(),
            usize::from(!after.is_empty()),
            after.lines().count()
        );
        for line in before.lines() {
            println!("-{line}");
        }
        for line in after.lines() {
            println!("+{line}");
        }
    }
    if !shown {
        println!("nothing to install");
    }
}

fn install(
    context: &Context,
    results: &[Inspected],
    dry_run: bool,
    yes: bool,
) -> Result<ExitCode, String> {
    for result in results
        .iter()
        .filter(|result| result.state != State::Current)
    {
        show(result);
    }
    if results.iter().any(|result| {
        (result.state.blocked() && result.state != State::Drifted) || result.wanted.is_none()
    }) {
        return Err("refusing to install while any file is unresolved".into());
    }
    let pending = results
        .iter()
        .filter(|result| matches!(result.state, State::Absent | State::Drifted | State::Sealed))
        .collect::<Vec<_>>();
    if pending.is_empty() {
        println!("nothing to install  {}", counted(results));
        return Ok(ExitCode::SUCCESS);
    }
    for result in &pending {
        println!(
            "  {:04o} root:root  {}",
            mode(&result.entry),
            result.entry.destination.display()
        );
    }
    if !dry_run && std::env::consts::OS != "linux" {
        return Err("system install is supported on Linux; use --dry-run to inspect the plan on this platform".into());
    }
    if !dry_run && !yes {
        print!("install {} file(s) as root? [y/N] ", pending.len());
        std::io::stdout()
            .flush()
            .map_err(|error| error.to_string())?;
        let mut answer = String::new();
        std::io::stdin()
            .read_line(&mut answer)
            .map_err(|error| error.to_string())?;
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            return Ok(ExitCode::FAILURE);
        }
    }
    let mut written = Vec::new();
    for result in &pending {
        crate::cancel::check()?;
        let entry = &result.entry;
        refusal(context, &entry.destination)?;
        if dry_run {
            println!(
                "  would: sudo install -D -o root -g root -m {:04o} <rendered> {}",
                mode(entry),
                entry.destination.display()
            );
        } else {
            let mut temporary = tempfile::NamedTempFile::new()
                .map_err(|error| format!("create private staging file: {error}"))?;
            temporary
                .write_all(
                    result
                        .wanted
                        .as_deref()
                        .ok_or("rendered content is missing")?,
                )
                .map_err(|error| format!("stage system file: {error}"))?;
            temporary
                .as_file()
                .sync_all()
                .map_err(|error| format!("flush system file: {error}"))?;
            let mut command = context.command("sudo");
            command
                .args([
                    "install",
                    "-D",
                    "-o",
                    "root",
                    "-g",
                    "root",
                    "-m",
                    &format!("{:04o}", mode(entry)),
                ])
                .arg(temporary.path())
                .arg(&entry.destination);
            let status = crate::process::status(&mut command)
                .map_err(|error| format!("install {}: {error}", entry.destination.display()))?;
            if !status.success() {
                eprintln!("  failed {}", entry.destination.display());
                continue;
            }
        }
        written.push(&entry.destination);
    }
    println!(
        "{} {} of {}",
        if dry_run {
            "would install"
        } else {
            "installed"
        },
        written.len(),
        pending.len()
    );
    for (prefix, hint) in [
        ("/etc/systemd/system", "sudo systemctl daemon-reload"),
        (
            "/etc/systemd/network",
            "sudo udevadm control --reload && sudo udevadm trigger --subsystem-match=net",
        ),
        (
            "/etc/NetworkManager",
            "sudo systemctl reload NetworkManager",
        ),
        ("/etc/sysctl.d", "sudo sysctl --system"),
    ] {
        if written.iter().any(|path| path.starts_with(prefix)) {
            println!("  then: {hint}");
        }
    }
    Ok(if written.len() == pending.len() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn add(context: &Context, path: &Path, package: &str, group: &str) -> Result<ExitCode, String> {
    crate::manage::validate_group(group)?;
    crate::manage::validate_package(package)?;
    let source = crate::manage::expand(context, path)?;
    refusal(context, &source)?;
    if !source.is_file() {
        return Err(format!("not a file: {}", source.display()));
    }
    let data =
        fs::read(&source).map_err(|error| format!("cannot read {}: {error}", source.display()))?;
    let relative = source
        .strip_prefix("/")
        .map_err(|error| error.to_string())?;
    let package_dir = context.root.join(group).join(package);
    crate::manage::require_within(&context.root, &package_dir)?;
    let destination = package_dir.join(relative);
    if fs::symlink_metadata(&destination).is_ok() {
        return Err(format!("already tracked: {}", destination.display()));
    }
    let mut transaction = crate::manage::transaction::Transaction::new(context)?;
    transaction.write(&destination, &data)?;
    let marker = package_dir.join(".system");
    if !marker.exists() {
        transaction.write(&marker, b"")?;
    }
    let top = relative
        .components()
        .next()
        .ok_or("source has no top-level directory")?
        .as_os_str()
        .to_string_lossy();
    let mapping = format!("{group}/{package}/{top} = /{top}");
    crate::manage::append_mapping(context, &mut transaction, &mapping)?;
    transaction.commit()?;
    println!(
        "copied {} -> {}",
        source.display(),
        destination
            .strip_prefix(&context.root)
            .unwrap_or(&destination)
            .display()
    );
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn permission_denied_only_seals_root_owned_private_files() {
        assert!(sealed_parts(true, 0o100600, 0, 0));
        assert!(!sealed_parts(false, 0o100600, 0, 0));
        assert!(!sealed_parts(true, 0o100644, 0, 0));
        assert!(!sealed_parts(true, 0o100600, 501, 0));
        assert!(!sealed_parts(true, 0o100600, 0, 20));
    }
}
