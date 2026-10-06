mod canaries;
pub mod cli;
mod doctor;
pub mod identity;
mod patterns;
pub mod recipients;
pub mod scan;
pub mod sops;
mod stamps;
mod store;
pub mod variables;
pub mod vault;
pub use cli::Args;

use crate::context::Context;
use cli::Command;
use identity::Identity;
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
        workstation::cli::decorate(<Args as clap::Args>::augment_args(clap::Command::new(
            "dotfile secret",
        )))
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
            review,
            no_canaries,
            all,
        } => {
            return scan::run(context, &paths, staged, &commits, !no_canaries, all, review);
        }
        Command::Redact => canaries::stream(context)?,
        Command::Init => {
            crate::tooling::requirements::ensure(
                context,
                &crate::tooling::requirements::SECRET_TOOLS,
            )?;
            let (host, reference) = identity::this(context)?;
            if identity::exists(context, &reference)? {
                return Err(format!("{reference} already exists; roll it instead"));
            }
            let (secret, key) = identity::generate();
            identity::store(context, &reference, &host, &secret, &key)?;
            let shared = identity::append(context, &secret)?;
            println!(
                "stored a new key in {reference} and {}\n\npublic key  {key}\n\nenrolling needs a key that already decrypts; run this on a machine that has one:\n\n    dotfile secret enroll {host} {key}",
                shared.display(),
            );
        }
        Command::KeyCommand => println!("{}", identity::key_command(context)?),
        Command::Keys => {
            let recipients = recipients::load(context)?;
            let mine = identity::this(context).ok().map(|(host, _)| host);
            if recipients.is_empty() {
                println!("no recipients in config/keys.dotfile");
            }
            for (label, key) in recipients {
                println!(
                    "  {label}  {key}{}",
                    if mine.as_ref() == Some(&label) {
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
                None => {
                    let (_, reference) = identity::this(context)?;
                    identity::public_key(&identity::secret(context, &reference)?)?
                }
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
            if let Some(key) = key {
                let identity = operation_identity(context, using.as_deref())?;
                validate_new_key(&recipients, &label, &key)?;
                if old == key {
                    println!("{label} already has that key");
                    return Ok(ExitCode::SUCCESS);
                }
                recipients.insert(label.clone(), key);
                recipients::rewrite(context, &recipients, &identity, true, None)?;
            } else {
                roll_this_machine(context, &mut recipients, &label, &old, using.as_deref())?;
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
            let consent = crate::consent::Consent::settled(crate::decision::Subject::Secret, false);
            let outcome =
                vault::reconcile(context, &configuration, dry_run, force, &consent, &sink)?;
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

fn operation_identity(context: &Context, using: Option<&Path>) -> Result<Identity, String> {
    using.map_or(Ok(Identity::OnePassword), |path| {
        sops::require_identity(context, path).map(Identity::File)
    })
}

/// 1Password gets the new key before any file is rewritten, and the old one back if that fails.
fn roll_this_machine(
    context: &Context,
    recipients: &mut recipients::Recipients,
    label: &str,
    old: &str,
    using: Option<&Path>,
) -> Result<(), String> {
    let (host, reference) = identity::this(context)?;
    if label != host {
        return Err(format!(
            "'{label}' is not this machine; pass the new public key"
        ));
    }
    let current = identity::secret(context, &reference)?;
    if identity::public_key(&current)? != old {
        return Err(format!(
            "{reference} does not hold the key enrolled as '{label}'"
        ));
    }
    let staging = tempfile::tempdir().map_err(|e| e.to_string())?;
    let previous = staging.path().join("previous.txt");
    vault::write_private(&previous, current.as_bytes())?;
    let decrypting = match using {
        Some(path) => operation_identity(context, Some(path))?,
        None => Identity::File(previous),
    };
    let (secret, key) = identity::generate();
    let fresh = staging.path().join("fresh.txt");
    vault::write_private(&fresh, secret.as_bytes())?;
    recipients.insert(label.to_string(), key.clone());
    identity::store(context, &reference, &host, &secret, &key)?;
    identity::reload(context);
    if let Err(error) = recipients::rewrite(
        context,
        recipients,
        &decrypting,
        true,
        Some(&Identity::File(fresh)),
    ) {
        return Err(
            match identity::store(context, &reference, &host, &current, old) {
                Ok(()) => {
                    identity::reload(context);
                    format!("{error}; restored the old key in {reference}")
                }
                Err(restore) => format!(
                    "{error}; restoring the old key in {reference} failed too ({restore}); recover it from the item's history"
                ),
            },
        );
    }
    let shared = identity::append(context, &secret)?;
    println!(
        "stored the new key in {reference}; added it to {} next to the old one",
        shared.display()
    );
    Ok(())
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
