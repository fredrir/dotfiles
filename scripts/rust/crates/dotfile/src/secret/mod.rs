mod canaries;
mod cli;
mod doctor;
mod patterns;
pub mod recipients;
pub mod scan;
pub mod sops;
mod store;
pub mod variables;
pub mod vault;
pub use cli::Args;

use crate::context::Context;
use cli::Command;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub fn expand(context: &Context, path: &Path) -> PathBuf {
    if path == Path::new("~") {
        context.home.clone()
    } else if let Ok(relative) = path.strip_prefix("~/") {
        context.home.join(relative)
    } else if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| context.root.clone())
            .join(path)
    }
}

pub fn configuration(context: &Context) -> Result<crate::config::Configuration, String> {
    let profile = context.profile(None)?;
    crate::config::Configuration::load(context, &profile, &[], &crate::event::VecSink::default())
}

pub fn run(args: Args, context: &Context) -> Result<ExitCode, String> {
    let Some(command) = args.command else {
        <Args as clap::Args>::augment_args(clap::Command::new("dotfile secret"))
            .print_help()
            .map_err(|e| e.to_string())?;
        println!();
        return Ok(ExitCode::SUCCESS);
    };
    let _lock = if command.mutates() {
        Some(crate::lock::MutationLock::acquire(context)?)
    } else {
        None
    };
    if _lock.is_some() {
        recipients::recover(context)?;
    }
    match command {
        Command::Scan {
            paths,
            staged,
            commits,
            no_canaries,
            all,
        } => {
            return scan::run(
                context,
                &paths,
                staged,
                commits.as_deref(),
                !no_canaries,
                all,
            );
        }
        Command::Redact => canaries::stream(context)?,
        Command::Init => {
            let path = vault::identity_path(context);
            sops::generate(context, &path)?;
            let key = sops::public_key(context, &path)?;
            println!(
                "created {} (0600)\n\npublic key  {key}\n\nenrolling needs a key that already decrypts; run this on a machine that has one:\n\n    dotfile secret enroll {} {key}\n\nor with a recovery identity:\n\n    dotfile secret enroll {} --using /path/to/recovery.txt",
                path.display(),
                doctor::suggested_label(context),
                doctor::suggested_label(context)
            );
        }
        Command::Keys => {
            let recipients = recipients::load(context)?;
            let mine = sops::public_key(context, &vault::identity_path(context)).ok();
            if recipients.is_empty() {
                println!("no recipients in config/keys.dotfile");
            }
            for (label, key) in recipients {
                println!(
                    "  {label}  {key}{}",
                    if mine.as_ref() == Some(&key) {
                        "  this machine"
                    } else {
                        ""
                    }
                );
            }
        }
        Command::Enroll { label, key, using } => {
            if !recipients::valid_label(&label) {
                return Err(format!("bad label '{label}'"));
            }
            let mut recipients = recipients::load(context)?;
            let key = match key {
                Some(key) => key,
                None => sops::public_key(context, &vault::identity_path(context))?,
            };
            validate_new_key(&recipients, &label, &key)?;
            if recipients.get(&label) == Some(&key) {
                println!("{label} is already enrolled");
                return Ok(ExitCode::SUCCESS);
            }
            if recipients.contains_key(&label) {
                return Err(format!(
                    "'{label}' already has a different key (revoke it first, or use roll)"
                ));
            }
            recipients.insert(label.clone(), key);
            let identity = operation_identity(context, using.as_deref())?;
            recipients::rewrite(context, &recipients, &identity, false, None)?;
            println!("enrolled {label}; commit and push so the new machine can read them");
        }
        Command::Revoke { label, using } => {
            let mut recipients = recipients::load(context)?;
            if !recipients.contains_key(&label) {
                return Err(format!("not enrolled: {label}"));
            }
            if recipients.len() == 1 {
                return Err("that is the only recipient; enroll another before revoking it".into());
            }
            recipients.remove(&label);
            let identity = operation_identity(context, using.as_deref())?;
            recipients::rewrite(context, &recipients, &identity, true, None)?;
            println!("revoked {label}");
            recipients::caveat();
        }
        Command::Rekey { using } => {
            let recipients = recipients::load(context)?;
            let identity = operation_identity(context, using.as_deref())?;
            recipients::rewrite(context, &recipients, &identity, true, None)?;
            recipients::caveat();
        }
        Command::Roll { label, key, using } => {
            let mut recipients = recipients::load(context)?;
            let old = recipients
                .get(&label)
                .ok_or_else(|| format!("not enrolled: {label}"))?
                .clone();
            let identity = operation_identity(context, using.as_deref())?;
            if let Some(key) = key {
                validate_new_key(&recipients, &label, &key)?;
                if old == key {
                    println!("{label} already has that key");
                    return Ok(ExitCode::SUCCESS);
                }
                recipients.insert(label.clone(), key);
                recipients::rewrite(context, &recipients, &identity, true, None)?;
            } else {
                if sops::public_key(context, &vault::identity_path(context))? != old {
                    return Err(format!(
                        "'{label}' is not this machine's key; pass the new public key"
                    ));
                }
                let staging = tempfile::tempdir().map_err(|e| e.to_string())?;
                let fresh = staging.path().join("keys.txt");
                sops::generate(context, &fresh)?;
                recipients.insert(label.clone(), sops::public_key(context, &fresh)?);
                recipients::rewrite(context, &recipients, &identity, true, Some(&fresh))?;
            }
            println!("rolled {label}");
            recipients::caveat();
        }
        Command::Sync { rewrap, using } => {
            let recipients = recipients::load(context)?;
            if recipients.is_empty() {
                return Err("no recipients enrolled (run: dotfile secret enroll <label>)".into());
            }
            if rewrap {
                let identity = operation_identity(context, using.as_deref())?;
                recipients::rewrite(context, &recipients, &identity, false, None)?;
            } else {
                println!(
                    "{}",
                    if recipients::save(context, &recipients)? {
                        "wrote .sops.yaml"
                    } else {
                        ".sops.yaml already current"
                    }
                );
            }
        }
        Command::Doctor { all } => return doctor::run(context, all),
        Command::Add(args) => store::add(context, args)?,
        Command::Edit { path } => return store::edit(context, &path),
        Command::Apply { dry_run, force } => {
            let configuration = configuration(context)?;
            let sink = SecretSink;
            let outcome = vault::synchronize(context, &configuration, dry_run, force, &sink)?;
            println!(
                "{} {} secrets, {} blocked",
                if dry_run { "would apply" } else { "applied" },
                outcome.changed,
                outcome.blocked
            );
            if outcome.blocked != 0 {
                return Ok(ExitCode::FAILURE);
            }
        }
        Command::Status => return store::status(context, false, false),
        Command::Clean { dry_run } => return store::status(context, true, dry_run),
        Command::Vars { unused } => return store::vars(context, unused),
    }
    Ok(ExitCode::SUCCESS)
}

// Public-only changes need no private identity until a ciphertext is encountered.
// Explicit --using paths are still validated even for an empty vault.
fn operation_identity(context: &Context, using: Option<&Path>) -> Result<PathBuf, String> {
    using.map_or_else(
        || Ok(vault::identity_path(context)),
        |path| sops::require_identity(context, Some(path)),
    )
}

fn validate_new_key(
    recipients: &recipients::Recipients,
    label: &str,
    key: &str,
) -> Result<(), String> {
    if !recipients::valid_key(key) {
        return Err("not an age public key".into());
    }
    if let Some((owner, _)) = recipients
        .iter()
        .find(|(owner, value)| *owner != label && *value == key)
    {
        return Err(format!("that key is already enrolled as '{owner}'"));
    }
    Ok(())
}

struct SecretSink;
impl crate::event::EventSink for SecretSink {
    fn emit(&self, event: crate::event::Event) {
        match event {
            crate::event::Event::Item { path, detail, .. } => {
                println!("  {detail} {}", path.display())
            }
            crate::event::Event::Warning { message, .. } => eprintln!("  {message}"),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
