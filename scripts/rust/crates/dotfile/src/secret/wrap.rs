use std::io;
use std::path::{Path, PathBuf};

use age::secrecy::{ExposeSecret, SecretString};
use zeroize::{Zeroize, Zeroizing};

use super::{recipients, vault};
use crate::context::Context;

const MINIMUM_PASSPHRASE: usize = 12;
const ATTEMPTS: usize = 3;
const MAX_WRAPPED_BYTES: usize = 64 * 1024;

pub fn path(context: &Context, host: &str) -> PathBuf {
    context.root_config.join("age").join(format!("{host}.age"))
}

/// The wrapped identity this machine would restore, when one is committed.
pub fn available(context: &Context) -> Option<(String, PathBuf)> {
    let host = crate::hosts::this(context)?;
    let source = path(context, &host);
    source.is_file().then_some((host, source))
}

pub fn wrap(context: &Context) -> Result<(), String> {
    let host = crate::hosts::this(context).ok_or_else(|| {
        format!(
            "cannot tell which machine this is; list its hostname in {}",
            crate::hosts::FILE
        )
    })?;
    let identity = Zeroizing::new(
        vault::read_source(&vault::identity_path(context), MAX_WRAPPED_BYTES)
            .map_err(|_| "no age identity on this machine (run: dotfile secret init)")?,
    );
    let key = public_key(&identity)?;
    if recipients::load(context)?.get(&host) != Some(&key) {
        return Err(format!(
            "this machine's identity is not enrolled as '{host}'; enroll or roll it first"
        ));
    }
    let passphrase = new_passphrase(&host)?;
    let armored = age::encrypt_and_armor(&age::scrypt::Recipient::new(passphrase), &identity)
        .map_err(|error| format!("wrap identity: {error}"))?;
    let destination = path(context, &host);
    recipients::commit(context, vec![(destination.clone(), armored.into_bytes())])?;
    recipients::stage(context, std::slice::from_ref(&destination))?;
    println!(
        "wrapped {host} ({key}) into {}; staged",
        relative(context, &destination)
    );
    Ok(())
}

pub fn unwrap(context: &Context) -> Result<(), String> {
    let (host, source) = available(context).ok_or_else(|| {
        "no wrapped identity for this machine (run: dotfile secret wrap)".to_string()
    })?;
    let destination = vault::identity_path(context);
    if destination.symlink_metadata().is_ok() {
        return Err(format!("{} already exists", destination.display()));
    }
    let enrolled = recipients::load(context)?.get(&host).cloned();
    let ciphertext = vault::read_source(&source, MAX_WRAPPED_BYTES)?;
    let identity = decrypt(&host, &ciphertext)?;
    let key = public_key(&identity)?;
    if enrolled.as_ref() != Some(&key) {
        return Err(format!(
            "{} holds {key}, not enrolled as '{host}'; re-wrap the enrolled identity",
            relative(context, &source)
        ));
    }
    let parent = destination.parent().ok_or("identity has no parent")?;
    vault::create_private_directories(parent)?;
    vault::set_mode(parent, 0o700)?;
    recipients::commit(context, vec![(destination.clone(), identity.to_vec())])?;
    println!(
        "restored {} (0600) for {host}\n\npublic key  {key}",
        destination.display()
    );
    Ok(())
}

fn decrypt(host: &str, ciphertext: &[u8]) -> Result<Zeroizing<Vec<u8>>, String> {
    for attempt in 1..=ATTEMPTS {
        let passphrase = ask(&format!("passphrase for {host}'s age identity: "))?;
        match age::decrypt(&age::scrypt::Identity::new(passphrase), ciphertext) {
            Ok(identity) => return Ok(Zeroizing::new(identity)),
            Err(age::DecryptError::DecryptionFailed | age::DecryptError::NoMatchingKeys) => {
                if attempt < ATTEMPTS {
                    eprintln!("wrong passphrase, try again");
                }
            }
            Err(error) => return Err(format!("unwrap identity: {error}")),
        }
    }
    Err(format!("wrong passphrase {ATTEMPTS} times"))
}

fn new_passphrase(host: &str) -> Result<SecretString, String> {
    let passphrase = ask(&format!("new passphrase for {host}'s age identity: "))?;
    if passphrase.expose_secret().chars().count() < MINIMUM_PASSPHRASE {
        return Err(format!(
            "passphrase under {MINIMUM_PASSPHRASE} characters; the wrapped identity is public"
        ));
    }
    if ask("repeat the passphrase: ")?.expose_secret() != passphrase.expose_secret() {
        return Err("the passphrases differ".into());
    }
    Ok(passphrase)
}

fn ask(question: &str) -> Result<SecretString, String> {
    let _signals = ui_terminal::SignalGuard::with_options(ui_terminal::SignalOptions {
        cancellation: Some(crate::cancel::flag()),
        reraise_on_drop: false,
        ..Default::default()
    })
    .map_err(|error| error.to_string())?;
    match ui_cli::hidden(question) {
        Ok(answer) if !crate::cancel::requested() => Ok(SecretString::from(answer)),
        Ok(mut answer) => {
            answer.zeroize();
            Err("cancelled".into())
        }
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::Interrupted | io::ErrorKind::UnexpectedEof
            ) =>
        {
            Err("cancelled".into())
        }
        Err(error) => Err(format!("read passphrase from the terminal: {error}")),
    }
}

fn public_key(identity: &[u8]) -> Result<String, String> {
    let text = std::str::from_utf8(identity).map_err(|_| "not an age identity")?;
    let mut keys = text
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("AGE-SECRET-KEY-"));
    match (keys.next(), keys.next()) {
        (Some(line), None) => line
            .parse::<age::x25519::Identity>()
            .map(|identity| identity.to_public().to_string())
            .map_err(|_| "not an age identity".into()),
        _ => Err("expected exactly one age identity".into()),
    }
}

fn relative(context: &Context, path: &Path) -> String {
    path.strip_prefix(&context.root)
        .unwrap_or(path)
        .display()
        .to_string()
}
