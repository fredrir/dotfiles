use super::{sops, vault};
use crate::context::Context;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub type Recipients = BTreeMap<String, String>;

pub fn is_recovery(label: &str) -> bool {
    label.to_ascii_lowercase().starts_with("recovery")
}

pub fn valid_label(label: &str) -> bool {
    label
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

pub fn valid_key(key: &str) -> bool {
    key.len() == 62
        && key.starts_with("age1")
        && key[4..]
            .bytes()
            .all(|b| b"023456789acdefghjklmnpqrstuvwxyz".contains(&b))
}

pub fn block(path: &Path, expected: &str) -> Result<Vec<(usize, String)>, String> {
    let entries = match crate::config::blocks::read(path) {
        Ok(entries) => entries,
        Err(_) if !path.exists() => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut found = Vec::new();
    for entry in entries {
        if entry.block != expected {
            return Err(format!(
                "{}:{}: expected '{expected}' block",
                path.display(),
                entry.number
            ));
        }
        if !entry.opens {
            found.push((entry.number, entry.text));
        }
    }
    Ok(found)
}

pub fn load(context: &Context) -> Result<Recipients, String> {
    let mut recipients = Recipients::new();
    for (number, line) in block(&context.root_config.join("keys.dotfile"), "recipients")? {
        let (label, key) = line.split_once('=').ok_or_else(|| {
            format!("config/keys.dotfile:{number}: expected <label> = <age public key>")
        })?;
        let (label, key) = (label.trim(), key.trim());
        if !valid_label(label) || !valid_key(key) {
            return Err(format!(
                "config/keys.dotfile:{number}: invalid recipient label or age public key"
            ));
        }
        if recipients
            .insert(label.to_string(), key.to_string())
            .is_some()
        {
            return Err(format!(
                "config/keys.dotfile:{number}: duplicate label '{label}'"
            ));
        }
    }
    Ok(recipients)
}

pub fn document(recipients: &Recipients) -> String {
    let width = recipients.keys().map(String::len).max().unwrap_or(0);
    let mut text = String::from("recipients {\n");
    for (label, key) in recipients {
        text.push_str(&format!("  {label:<width$} = {key}\n"));
    }
    text.push_str("}\n");
    text
}

pub fn policy(recipients: &Recipients) -> String {
    if recipients.is_empty() {
        String::new()
    } else {
        format!(
            "creation_rules:\n  - age: {}\n",
            recipients.values().cloned().collect::<Vec<_>>().join(",")
        )
    }
}

pub fn save(context: &Context, recipients: &Recipients) -> Result<bool, String> {
    let expected = policy(recipients);
    let path = context.root.join(".sops.yaml");
    let changed = fs::read_to_string(&path).unwrap_or_default() != expected;
    let mut changes = Vec::new();
    if load(context)? != *recipients {
        changes.push((
            context.root_config.join("keys.dotfile"),
            document(recipients).into_bytes(),
        ));
    }
    if changed {
        changes.push((path, expected.into_bytes()));
    }
    commit(context, changes)?;
    Ok(changed)
}

#[derive(Serialize, Deserialize)]
struct Backup {
    path: PathBuf,
    existed: bool,
    mode: u32,
}

/// Restores an interrupted transaction before another writer can proceed.
/// The journal contains only originals, so recovery always rolls back safely.
pub fn recover(context: &Context) -> Result<(), String> {
    let directory = context.root_config.join("secret-transaction");
    let manifest = directory.join("manifest.json");
    match directory.symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("inspect secret recovery journal: {error}")),
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
            return Err("secret recovery journal is not a regular directory".into());
        }
        Ok(_) => {}
    }
    if !manifest.exists() {
        fs::remove_dir_all(&directory)
            .map_err(|e| format!("clear incomplete secret journal: {e}"))?;
        return Ok(());
    }
    let metadata = manifest.symlink_metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("secret recovery manifest is not a regular file".into());
    }
    let backups: Vec<Backup> =
        serde_json::from_slice(&fs::read(&manifest).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    for (index, backup) in backups.iter().enumerate() {
        validate_destination(context, &backup.path)?;
        if backup.mode > 0o777 {
            return Err("secret recovery journal contains invalid permissions".into());
        }
        if backup.existed {
            let metadata = directory
                .join(index.to_string())
                .symlink_metadata()
                .map_err(|e| format!("inspect secret recovery backup: {e}"))?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err("secret recovery backup is not a regular file".into());
            }
        }
    }
    for (index, backup) in backups.iter().enumerate() {
        if backup.existed {
            let bytes = fs::read(directory.join(index.to_string()))
                .map_err(|e| format!("read secret recovery backup: {e}"))?;
            vault::write_private(&backup.path, &bytes)?;
            vault::set_mode(&backup.path, backup.mode)?;
        } else if backup.path.exists() {
            fs::remove_file(&backup.path).map_err(|e| e.to_string())?;
        }
    }
    fs::remove_dir_all(directory).map_err(|e| e.to_string())?;
    eprintln!("dotfile: restored files from an interrupted secret transaction");
    Ok(())
}

pub fn commit(context: &Context, changes: Vec<(PathBuf, Vec<u8>)>) -> Result<(), String> {
    commit_inner(context, changes, |_| Ok(()))
}

pub(super) fn commit_inner(
    context: &Context,
    changes: Vec<(PathBuf, Vec<u8>)>,
    mut before_install: impl FnMut(usize) -> Result<(), String>,
) -> Result<(), String> {
    if changes.is_empty() {
        return Ok(());
    }
    for (path, _) in &changes {
        validate_destination(context, path)?;
    }
    let changes: Vec<_> = changes
        .into_iter()
        .map(|(path, data)| (path, zeroize::Zeroizing::new(data)))
        .collect();
    let directory = context.root_config.join("secret-transaction");
    if directory.exists() {
        return Err("unfinished secret transaction; run a secret mutation to recover first".into());
    }
    vault::create_private_directories(&directory)?;
    let mut backups = Vec::new();
    for (index, (path, _)) in changes.iter().enumerate() {
        if path
            .symlink_metadata()
            .is_ok_and(|m| !m.is_file() || m.file_type().is_symlink())
        {
            return Err(format!(
                "refusing non-regular transaction destination: {}",
                path.display()
            ));
        }
        let existed = path.exists();
        let mode = if existed {
            vault::mode_of(path)?
        } else if *path == vault::identity_path(context) {
            0o600
        } else {
            0o644
        };
        if existed {
            vault::write_private(
                &directory.join(index.to_string()),
                &fs::read(path).map_err(|e| e.to_string())?,
            )?;
        }
        backups.push(Backup {
            path: path.clone(),
            existed,
            mode,
        });
    }
    vault::write_private(
        &directory.join("manifest.json"),
        &serde_json::to_vec(&backups).map_err(|e| e.to_string())?,
    )?;
    sync_directory(&directory)?;
    sync_directory(&context.root_config)?;
    for (index, ((path, data), backup)) in changes.iter().zip(&backups).enumerate() {
        if let Err(error) = crate::cancel::check()
            .and_then(|()| before_install(index))
            .and_then(|()| vault::write_private(path, data))
            .and_then(|()| vault::set_mode(path, backup.mode))
            .and_then(|()| {
                sync_directory(
                    path.parent()
                        .ok_or("transaction destination has no parent")?,
                )
            })
        {
            recover(context).map_err(|recovery| {
                format!(
                    "{error}; recovery failed: {recovery}; journal retained at {}",
                    directory.display()
                )
            })?;
            return Err(error);
        }
    }
    // Removing the manifest commits the transaction; leftovers are then disposable.
    fs::remove_file(directory.join("manifest.json")).map_err(|e| e.to_string())?;
    sync_directory(&directory)?;
    fs::remove_dir_all(directory).map_err(|e| e.to_string())?;
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), String> {
    fs::File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| format!("sync {}: {e}", path.display()))
}

pub fn stage(context: &Context, paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut command = context.command("git");
    command
        .arg("-C")
        .arg(&context.root)
        .args(["add", "--"])
        .args(paths);
    sops::capture(&mut command, 4096, "git add")?;
    Ok(())
}

/// Rewrites copies and verifies every result before committing any live file.
pub fn rewrite(
    context: &Context,
    recipients: &Recipients,
    identity: &Path,
    rotate: bool,
    installed_identity: Option<&Path>,
) -> Result<(), String> {
    if recipients.is_empty() {
        return Err("no recipients enrolled".into());
    }
    let paths = super::scan::encrypted_paths(context)?;
    let temporary = tempfile::tempdir().map_err(|e| e.to_string())?;
    let policy_path = temporary.path().join(".sops.yaml");
    fs::write(&policy_path, policy(recipients)).map_err(|e| e.to_string())?;
    let verify_identity = installed_identity.unwrap_or(identity);
    if !paths.is_empty() {
        let pubkey = sops::public_key(context, verify_identity)?;
        if !recipients.values().any(|key| *key == pubkey) {
            return Err("the verifying identity is not a remaining recipient; use --using with a remaining identity".into());
        }
    }
    let mut changes = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        crate::cancel::check()?;
        let source = context.root.join(path);
        let expected = sops::decrypt(context, &source, Some(identity), false).map_err(|_| {
            format!(
                "this machine cannot read {}; fix that before changing recipients",
                path.display()
            )
        })?;
        let staging = temporary.path().join(index.to_string());
        fs::create_dir(&staging).map_err(|e| e.to_string())?;
        let staged = staging.join(source.file_name().ok_or("invalid encrypted filename")?);
        fs::copy(&source, &staged).map_err(|e| e.to_string())?;
        let mut update = sops::command(context, Some(identity));
        update
            .arg("--config")
            .arg(&policy_path)
            .args(["updatekeys", "-y"])
            .arg(&staged);
        sops::capture(&mut update, 64 * 1024, "SOPS recipient update")?;
        if rotate {
            let mut command = sops::command(context, Some(verify_identity));
            command
                .arg("--config")
                .arg(&policy_path)
                .args(["-r", "-i"])
                .arg(&staged);
            sops::capture(&mut command, 64 * 1024, "SOPS data-key rotation")?;
        }
        let actual = sops::decrypt(context, &staged, Some(verify_identity), false)?;
        if *actual != *expected {
            return Err(format!(
                "verification changed secret content: {}",
                path.display()
            ));
        }
        changes.push((source, fs::read(staged).map_err(|e| e.to_string())?));
    }
    let mut staged_paths: Vec<_> = changes.iter().map(|(path, _)| path.clone()).collect();
    if load(context)? != *recipients {
        let path = context.root_config.join("keys.dotfile");
        changes.push((path.clone(), document(recipients).into_bytes()));
        staged_paths.push(path);
    }
    let policy_file = context.root.join(".sops.yaml");
    changes.push((policy_file.clone(), policy(recipients).into_bytes()));
    staged_paths.push(policy_file);
    if let Some(fresh) = installed_identity {
        changes.push((
            vault::identity_path(context),
            fs::read(fresh).map_err(|e| e.to_string())?,
        ));
    }
    commit(context, changes)?;
    stage(context, &staged_paths)?;
    println!(
        "{} {} of {} file(s); staged",
        if rotate {
            "re-wrapped and gave a new data key to"
        } else {
            "re-wrapped"
        },
        paths.len(),
        paths.len()
    );
    Ok(())
}

pub fn caveat() {
    println!(
        "older clones retain ciphertext the old key can read; rotate anything that key actually protected"
    );
}

fn validate_destination(context: &Context, path: &Path) -> Result<(), String> {
    let anchor = if path == vault::identity_path(context) {
        &context.root_config
    } else if path.starts_with(&context.root) {
        &context.root
    } else {
        return Err(
            "secret transaction destination is outside the repository and identity state".into(),
        );
    };
    let relative = path
        .strip_prefix(anchor)
        .map_err(|_| "invalid secret transaction destination")?;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err("secret transaction destination contains traversal".into());
    }
    let mut current = anchor.clone();
    let components = relative.components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        current.push(component.as_os_str());
        match current.symlink_metadata() {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "secret transaction refuses a symlink: {}",
                    current.display()
                ));
            }
            Ok(metadata) if index + 1 < components.len() && !metadata.is_dir() => {
                return Err(format!(
                    "secret transaction parent is not a directory: {}",
                    current.display()
                ));
            }
            Ok(metadata) if index + 1 == components.len() && !metadata.is_file() => {
                return Err(format!(
                    "secret transaction destination is not a regular file: {}",
                    current.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "inspect transaction path {}: {error}",
                    current.display()
                ));
            }
        }
    }
    Ok(())
}
