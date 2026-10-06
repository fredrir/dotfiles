use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use age::secrecy::ExposeSecret;
use serde_json::{Value, json};
use zeroize::Zeroizing;

use super::{recipients, sops, vault};
use crate::context::Context;

pub const BLOCK: &str = "identities";
const PUBLIC: &str = "username";
const SECRET: &str = "credential";
const MASKED_CONFIG_HOME: &str = "/var/empty";
const MAX_ITEM_BYTES: usize = 256 * 1024;
const MAX_LIST_BYTES: usize = 4 * 1024 * 1024;
const BRIDGE_UNREACHABLE: i32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub vault: String,
    pub item: String,
}

pub type Identities = BTreeMap<String, Reference>;

#[derive(Debug, Clone)]
pub enum Identity {
    OnePassword,
    File(PathBuf),
}

impl Identity {
    pub fn public_key(&self, context: &Context) -> Result<String, String> {
        match self {
            Self::File(path) => sops::public_key(path),
            Self::OnePassword => enrolled_key(context),
        }
    }
}

impl Reference {
    pub fn parse(text: &str) -> Option<Self> {
        let (vault, item) = text.strip_prefix("op://")?.split_once('/')?;
        let valid = |part: &str| {
            !part.trim().is_empty()
                && part == part.trim()
                && !part.contains(['/', '\'', '"', '$', '`', '\\'])
                && !part.chars().any(char::is_control)
        };
        (valid(vault) && valid(item)).then(|| Self {
            vault: vault.to_string(),
            item: item.to_string(),
        })
    }

    pub fn field(&self, field: &str) -> String {
        format!("op://{}/{}/{field}", self.vault, self.item)
    }
}

impl std::fmt::Display for Reference {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "op://{}/{}", self.vault, self.item)
    }
}

pub fn load(context: &Context) -> Result<Identities, String> {
    let mut identities = Identities::new();
    for (number, line) in recipients::section(
        &context.root_config.join("keys.dotfile"),
        BLOCK,
        &recipients::BLOCKS,
    )? {
        let parsed = line.split_once('=').and_then(|(label, reference)| {
            let label = label.trim();
            recipients::valid_label(label)
                .then_some(label)
                .zip(Reference::parse(reference.trim()))
        });
        let (label, reference) = parsed.ok_or_else(|| {
            format!("config/keys.dotfile:{number}: expected <label> = op://<vault>/<item>")
        })?;
        if identities.insert(label.to_string(), reference).is_some() {
            return Err(format!(
                "config/keys.dotfile:{number}: duplicate identity '{label}'"
            ));
        }
    }
    Ok(identities)
}

pub fn document(identities: &Identities) -> String {
    if identities.is_empty() {
        return String::new();
    }
    let width = identities.keys().map(String::len).max().unwrap_or(0);
    let mut text = format!("\n{BLOCK} {{\n");
    for (label, reference) in identities {
        text.push_str(&format!("  {label:<width$} = {reference}\n"));
    }
    text.push_str("}\n");
    text
}

/// This machine's label and 1Password item.
pub fn this(context: &Context) -> Result<(String, Reference), String> {
    let host = crate::hosts::this(context).ok_or_else(|| {
        format!(
            "cannot tell which machine this is; list its hostname in {}",
            crate::hosts::FILE
        )
    })?;
    let reference = load(context)?.remove(&host).ok_or_else(|| {
        format!("no 1Password identity for '{host}'; add it to the identities block in config/keys.dotfile")
    })?;
    Ok((host, reference))
}

pub fn available(context: &Context) -> bool {
    this(context).is_ok() && context.program("op").is_some()
}

/// The public key this machine is enrolled with, taken from config rather than 1Password.
pub fn enrolled_key(context: &Context) -> Result<String, String> {
    let (host, _) = this(context)?;
    recipients::load(context)?
        .remove(&host)
        .ok_or_else(|| format!("'{host}' is not an enrolled recipient"))
}

/// `SOPS_AGE_KEY_CMD` for this machine; `op` must not inherit the masked config home.
pub fn key_command(context: &Context) -> Result<String, String> {
    let (_, reference) = this(context)?;
    let environment = match context.env("XDG_CONFIG_HOME") {
        None => "env -u XDG_CONFIG_HOME".to_string(),
        Some(value) => {
            let value = value
                .into_string()
                .ok()
                .filter(|value| !value.contains(['\'', '"', '$', '`', '\\']))
                .ok_or("XDG_CONFIG_HOME cannot be quoted for SOPS_AGE_KEY_CMD")?;
            format!("env XDG_CONFIG_HOME='{value}'")
        }
    };
    Ok(format!(
        "{environment} op read '{}'",
        reference.field(SECRET)
    ))
}

/// Points sops at exactly one identity and hides the shared default key file from it.
pub fn configure(
    context: &Context,
    command: &mut Command,
    identity: Option<&Identity>,
) -> Result<(), String> {
    command
        .env_remove("SOPS_AGE_KEY")
        .env_remove("SOPS_AGE_KEY_FILE")
        .env_remove("SOPS_AGE_KEY_CMD")
        .env("XDG_CONFIG_HOME", MASKED_CONFIG_HOME);
    match identity {
        None => {}
        Some(Identity::File(path)) => {
            command.env("SOPS_AGE_KEY_FILE", path);
        }
        Some(Identity::OnePassword) => {
            command.env("SOPS_AGE_KEY_CMD", key_command(context)?);
        }
    }
    Ok(())
}

/// A shell prefix with the same effect as [`configure`], for git's textconv.
pub fn shell_prefix(context: &Context) -> Result<String, String> {
    Ok(format!(
        "env -u SOPS_AGE_KEY -u SOPS_AGE_KEY_FILE XDG_CONFIG_HOME={MASKED_CONFIG_HOME} SOPS_AGE_KEY_CMD=\"{}\"",
        key_command(context)?
    ))
}

pub fn secret(context: &Context, reference: &Reference) -> Result<Zeroizing<String>, String> {
    let mut command = op(context);
    command
        .args(["read", "--no-newline"])
        .arg(reference.field(SECRET));
    let output = sops::capture(&mut command, 4096, "op read")?;
    let text = std::str::from_utf8(&output).map_err(|_| "op read: value is not UTF-8")?;
    Ok(Zeroizing::new(text.trim().to_string()))
}

pub fn public_key(secret: &str) -> Result<String, String> {
    let mut keys = secret
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

pub fn generate() -> (Zeroizing<String>, String) {
    let identity = age::x25519::Identity::generate();
    let public = identity.to_public().to_string();
    (
        Zeroizing::new(identity.to_string().expose_secret().to_string()),
        public,
    )
}

/// Where sops looks when nothing is configured; the key lives here for other projects.
pub fn default_path(context: &Context) -> PathBuf {
    let base = match context
        .env("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
    {
        Some(value) => PathBuf::from(value),
        None if cfg!(target_os = "macos") => context.home.join("Library/Application Support"),
        None => context.home.join(".config"),
    };
    base.join("sops/age/keys.txt")
}

/// Writes this machine's key to sops' default location when it is missing.
pub fn install(context: &Context) -> Result<Option<PathBuf>, String> {
    let destination = default_path(context);
    if destination.symlink_metadata().is_ok() {
        return Ok(None);
    }
    let (host, reference) = this(context)?;
    let secret = secret(context, &reference)?;
    let key = public_key(&secret)?;
    if enrolled_key(context)? != key {
        return Err(format!(
            "{reference} holds {key}, which is not enrolled as '{host}'"
        ));
    }
    write_key_file(&destination, &secret)?;
    Ok(Some(destination))
}

/// Adds a rolled key to the default key file, keeping older keys for other projects.
pub fn append(context: &Context, secret: &str) -> Result<PathBuf, String> {
    let destination = default_path(context);
    let mut text = Zeroizing::new(match fs::read(&destination) {
        Ok(bytes) => String::from_utf8(bytes).map_err(|_| "default age key file is not text")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("read {}: {error}", destination.display())),
    });
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(secret);
    text.push('\n');
    write_key_file(&destination, &text)?;
    Ok(destination)
}

fn write_key_file(destination: &Path, secret: &str) -> Result<(), String> {
    let parent = destination.parent().ok_or("age key file has no parent")?;
    vault::create_private_directories(parent)?;
    let mut text = Zeroizing::new(secret.trim_end().to_string());
    text.push('\n');
    crate::fs::write_private(destination, text.as_bytes()).map(drop)
}

/// Creates or updates the API Credential item: username holds the public key, credential the secret.
pub fn store(
    context: &Context,
    reference: &Reference,
    host: &str,
    secret: &str,
    public: &str,
) -> Result<(), String> {
    let existing = item(context, reference)?;
    let created = existing.is_none();
    let mut document = existing.unwrap_or_else(|| {
        json!({
            "title": reference.item,
            "category": "API_CREDENTIAL",
            "fields": [],
        })
    });
    set_field(&mut document, PUBLIC, "STRING", public)?;
    set_field(&mut document, SECRET, "CONCEALED", secret)?;
    set_field(&mut document, "hostname", "STRING", host)?;
    let template = tempfile::Builder::new()
        .prefix("dotfile-identity-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let path = template.path().join("item.json");
    let bytes = Zeroizing::new(serde_json::to_vec(&document).map_err(|error| error.to_string())?);
    vault::write_private(&path, &bytes)?;
    let mut command = op(context);
    if created {
        command.args(["item", "create", "--vault", &reference.vault, "--template"]);
    } else {
        let id = document
            .get("id")
            .and_then(Value::as_str)
            .ok_or("1Password item has no id")?;
        command.args([
            "item",
            "edit",
            id,
            "--vault",
            &reference.vault,
            "--template",
        ]);
    }
    command.arg(&path);
    sops::capture(&mut command, MAX_ITEM_BYTES, "op item").map(drop)
}

pub fn exists(context: &Context, reference: &Reference) -> Result<bool, String> {
    item(context, reference).map(|item| item.is_some())
}

fn item(context: &Context, reference: &Reference) -> Result<Option<Value>, String> {
    let mut list = op(context);
    list.args(["item", "list", "--vault", &reference.vault])
        .args(["--format", "json"]);
    let items: Vec<Value> =
        serde_json::from_slice(&sops::capture(&mut list, MAX_LIST_BYTES, "op item list")?)
            .map_err(|_| format!("op item list {}: unexpected output", reference.vault))?;
    let ids: Vec<&str> = items
        .iter()
        .filter(|item| item.get("title").and_then(Value::as_str) == Some(reference.item.as_str()))
        .filter_map(|item| item.get("id").and_then(Value::as_str))
        .collect();
    let id = match ids.as_slice() {
        [] => return Ok(None),
        [id] => *id,
        _ => return Err(format!("{reference}: several items share that title")),
    };
    let mut get = op(context);
    get.args(["item", "get", id, "--vault", &reference.vault])
        .args(["--format", "json"]);
    let output = sops::capture(&mut get, MAX_ITEM_BYTES, "op item get")?;
    serde_json::from_slice(&output)
        .map(Some)
        .map_err(|_| format!("op item get {reference}: unexpected output"))
}

fn set_field(document: &mut Value, id: &str, kind: &str, value: &str) -> Result<(), String> {
    let fields = document
        .get_mut("fields")
        .and_then(Value::as_array_mut)
        .ok_or("1Password item has no fields")?;
    match fields
        .iter_mut()
        .find(|field| field.get("id").and_then(Value::as_str) == Some(id))
    {
        Some(field) => field["value"] = Value::String(value.to_string()),
        None => fields.push(json!({
            "id": id,
            "type": kind,
            "label": id,
            "value": value,
        })),
    }
    Ok(())
}

/// Clears op-bridge's cached copy so the next read returns a rolled key.
pub fn reload(context: &Context) {
    if context.program("op-bridge").is_none() {
        return;
    }
    let mut command = context.command("op-bridge");
    command
        .arg("reload")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let status = command.status().ok().and_then(|status| status.code());
    if !matches!(status, Some(0 | BRIDGE_UNREACHABLE)) {
        eprintln!(
            "dotfile: op-bridge reload failed; its cache may hold the old key for up to 30 min"
        );
    }
}

fn op(context: &Context) -> Command {
    let mut command = context.command("op");
    command.stdin(Stdio::null());
    command
}
