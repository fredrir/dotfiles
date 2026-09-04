use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::context::Context;
use crate::event::{Action, Event, EventSink, Phase};

use super::config::{Configuration, Package, PackageKind, never_fold};

const FILE_MODE: u32 = 0o600;
const DIRECTORY_MODE: u32 = 0o700;

#[derive(Clone, Debug, Eq, PartialEq)]
enum SecretKind {
    Encrypted,
    Template,
    Plain,
}

#[derive(Clone, Debug)]
struct SecretEntry {
    source: PathBuf,
    destination: PathBuf,
    kind: SecretKind,
}

#[derive(Default)]
pub struct SecretOutcome {
    pub checked: usize,
    pub changed: usize,
    pub secrets: usize,
    pub blocked: usize,
}

pub fn synchronize(
    context: &Context,
    configuration: &Configuration,
    dry_run: bool,
    force: bool,
    events: &dyn EventSink,
) -> Result<SecretOutcome, String> {
    let entries = plan(context, configuration)?;
    events.emit(Event::PhaseStarted {
        phase: Phase::Secrets,
        total: Some(entries.len()),
    });
    if entries.is_empty() {
        return Ok(SecretOutcome::default());
    }
    let total = entries.len();
    let variables = load_variables(context);
    let mut outcome = SecretOutcome::default();
    let mut blocked = 0;
    let mut warnings = Vec::new();
    for (index, mut entry) in entries.into_iter().enumerate() {
        crate::cancel::check()?;
        outcome.checked += 1;
        let result = materialize(context, &mut entry, &variables, dry_run, force)?;
        if result.changed {
            outcome.changed += 1;
            outcome.secrets += 1;
        }
        if result.blocked {
            blocked += 1;
        }
        if result.warning {
            warnings.push((
                entry.destination.clone(),
                result.detail.clone(),
                result.hint.clone(),
            ));
        }
        events.emit(Event::Item {
            action: Action::Secret,
            path: entry.destination.clone(),
            detail: result.detail,
            changed: result.changed,
        });
        events.emit(Event::Progress {
            phase: Phase::Secrets,
            completed: index + 1,
            total: Some(total),
            label: entry.destination.display().to_string(),
        });
    }
    for directory in secure_package_directories(context, configuration)? {
        if mode_of(&directory)? & 0o077 != 0 {
            if !dry_run {
                set_mode(&directory, DIRECTORY_MODE)?;
            }
            outcome.changed += 1;
            outcome.secrets += 1;
            events.emit(Event::Item {
                action: Action::Secret,
                path: directory,
                detail: "secured directory permissions".to_string(),
                changed: true,
            });
        }
    }
    if let Some((path, detail, hint)) = warnings.first() {
        events.emit(Event::Warning {
            message: format!(
                "{} secret{} need attention; first: {} ({detail})",
                warnings.len(),
                if warnings.len() == 1 { "" } else { "s" },
                path.display()
            ),
            hint: hint
                .clone()
                .or_else(|| Some("use -v to inspect every secret".to_string())),
        });
    }
    if blocked == 0 {
        Ok(outcome)
    } else {
        outcome.blocked = blocked;
        Ok(outcome)
    }
}

struct SecretResult {
    changed: bool,
    blocked: bool,
    warning: bool,
    detail: String,
    hint: Option<String>,
}

fn materialize(
    context: &Context,
    entry: &mut SecretEntry,
    variables: &Variables,
    dry_run: bool,
    force: bool,
) -> Result<SecretResult, String> {
    let produced = match entry.kind {
        SecretKind::Plain => {
            return Ok(SecretResult {
                changed: false,
                blocked: true,
                warning: true,
                detail: "plaintext file inside a .secret package".to_string(),
                hint: Some("encrypt it with dotfile secret add".to_string()),
            });
        }
        SecretKind::Encrypted => {
            if !identity_path(context).is_file() {
                return Ok(SecretResult {
                    changed: false,
                    blocked: false,
                    warning: true,
                    detail: "sealed; no age identity on this machine".to_string(),
                    hint: Some("install or import this machine's age identity".to_string()),
                });
            }
            match decrypt(context, &entry.source) {
                Ok(content) => content,
                Err(error) => {
                    return Ok(SecretResult {
                        changed: false,
                        blocked: true,
                        warning: true,
                        detail: format!("decryption failed: {error}"),
                        hint: Some("verify that this machine is an enrolled recipient".to_string()),
                    });
                }
            }
        }
        SecretKind::Template => {
            let text = fs::read_to_string(&entry.source)
                .map_err(|error| format!("read {}: {error}", entry.source.display()))?;
            let references = references(&text);
            if !references.is_empty() && !variables.ok {
                return Ok(SecretResult {
                    changed: false,
                    blocked: false,
                    warning: true,
                    detail: variables.note.clone(),
                    hint: Some("install or import this machine's age identity".to_string()),
                });
            }
            let (rendered, missing) = render_template(&text, &variables.values);
            if !missing.is_empty() {
                return Ok(SecretResult {
                    changed: false,
                    blocked: true,
                    warning: true,
                    detail: format!("unresolved variables: {}", missing.join(" ")),
                    hint: Some("define them in vars.enc.yaml".to_string()),
                });
            }
            rendered.into_bytes()
        }
    };
    let metadata = match fs::symlink_metadata(&entry.destination) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(format!("read {}: {error}", entry.destination.display()));
        }
    };
    if metadata
        .as_ref()
        .is_some_and(|value| value.file_type().is_symlink())
    {
        return Ok(SecretResult {
            changed: false,
            blocked: true,
            warning: true,
            detail: "destination is a symlink".to_string(),
            hint: Some("move it aside before applying the secret".to_string()),
        });
    }
    if metadata.is_some() {
        let current = fs::read(&entry.destination)
            .map_err(|error| format!("read {}: {error}", entry.destination.display()))?;
        if current != produced {
            if !force {
                return Ok(SecretResult {
                    changed: false,
                    blocked: true,
                    warning: true,
                    detail: "edited on this machine".to_string(),
                    hint: Some(
                        "use --force to discard it or adopt it with dotfile secret edit"
                            .to_string(),
                    ),
                });
            }
            if !dry_run {
                write_private(&entry.destination, &produced)?;
            }
            return Ok(SecretResult {
                changed: true,
                blocked: false,
                warning: false,
                detail: "restored encrypted source".to_string(),
                hint: None,
            });
        }
        if mode_of(&entry.destination)? != FILE_MODE {
            if !dry_run {
                set_mode(&entry.destination, FILE_MODE)?;
            }
            return Ok(SecretResult {
                changed: true,
                blocked: false,
                warning: false,
                detail: "secured permissions".to_string(),
                hint: None,
            });
        }
        return Ok(SecretResult {
            changed: false,
            blocked: false,
            warning: false,
            detail: "current".to_string(),
            hint: None,
        });
    }
    if !dry_run {
        write_private(&entry.destination, &produced)?;
    }
    Ok(SecretResult {
        changed: true,
        blocked: false,
        warning: false,
        detail: if dry_run {
            "would decrypt"
        } else {
            "decrypted"
        }
        .to_string(),
        hint: None,
    })
}

fn plan(context: &Context, configuration: &Configuration) -> Result<Vec<SecretEntry>, String> {
    let mut entries = Vec::new();
    for package in &configuration.packages {
        match package.kind {
            PackageKind::Secret => {
                collect_entries(context, configuration, package, true, &mut entries)?
            }
            PackageKind::Link => {
                collect_entries(context, configuration, package, false, &mut entries)?
            }
            PackageKind::NoLink | PackageKind::System => {}
        }
    }
    entries.sort_by(|left, right| left.destination.cmp(&right.destination));
    Ok(entries)
}

fn collect_entries(
    context: &Context,
    configuration: &Configuration,
    package: &Package,
    whole_package: bool,
    entries: &mut Vec<SecretEntry>,
) -> Result<(), String> {
    let mut files = Vec::new();
    collect_files(&package.directory, &mut files)?;
    for source in files {
        let name = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("secret path is not valid UTF-8: {}", source.display()))?;
        if matches!(name, ".secret" | ".nolink" | ".system") {
            continue;
        }
        let kind = kind_of(&source);
        if !whole_package && kind == SecretKind::Plain {
            continue;
        }
        let relative = source.strip_prefix(&package.directory).map_err(|error| {
            format!(
                "map {} below {}: {error}",
                source.display(),
                package.directory.display()
            )
        })?;
        let relative_text = relative
            .to_str()
            .ok_or_else(|| format!("secret path is not valid UTF-8: {}", source.display()))?;
        let full = format!("{}/{}", package.name, relative_text);
        let mapped = configuration.map_destination(context, &full, &package.package, relative);
        let plain = plain_name(
            mapped
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| {
                    format!(
                        "secret destination is not valid UTF-8: {}",
                        mapped.display()
                    )
                })?,
        );
        let destination = mapped.parent().unwrap_or_else(|| Path::new("")).join(plain);
        entries.push(SecretEntry {
            source,
            destination,
            kind,
        });
    }
    Ok(())
}

fn collect_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("read {}: {error}", directory.display())),
    };
    let mut entries = entries
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("read {}: {error}", directory.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            collect_files(&path, files)?;
        } else {
            files.push(path);
        }
    }
    Ok(())
}

fn kind_of(path: &Path) -> SecretKind {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if name.ends_with(".enc") || name.contains(".enc.") {
        SecretKind::Encrypted
    } else if name.ends_with(".tmpl") {
        SecretKind::Template
    } else {
        SecretKind::Plain
    }
}

fn plain_name(name: &str) -> String {
    let name = name.strip_suffix(".tmpl").unwrap_or(name);
    if let Some(name) = name.strip_suffix(".enc") {
        name.to_string()
    } else {
        name.replacen(".enc.", ".", 1)
    }
}

struct Variables {
    values: BTreeMap<String, String>,
    ok: bool,
    note: String,
}

fn load_variables(context: &Context) -> Variables {
    let source = context.root.join("vars.enc.yaml");
    if !source.is_file() {
        return Variables {
            values: BTreeMap::new(),
            ok: true,
            note: String::new(),
        };
    }
    if !identity_path(context).is_file() {
        return Variables {
            values: BTreeMap::new(),
            ok: false,
            note: "vars.enc.yaml needs an age identity to read".to_string(),
        };
    }
    let output = Command::new("sops")
        .args(["-d", "--output-type", "json"])
        .arg(&source)
        .env("SOPS_AGE_KEY_FILE", identity_path(context))
        .output();
    let Ok(output) = output else {
        return Variables {
            values: BTreeMap::new(),
            ok: false,
            note: "sops is not available to decrypt vars.enc.yaml".to_string(),
        };
    };
    if !output.status.success() {
        return Variables {
            values: BTreeMap::new(),
            ok: false,
            note: "vars.enc.yaml did not decrypt on this machine".to_string(),
        };
    }
    let Ok(Value::Object(document)) = serde_json::from_slice::<Value>(&output.stdout) else {
        return Variables {
            values: BTreeMap::new(),
            ok: false,
            note: "vars.enc.yaml must hold a mapping".to_string(),
        };
    };
    let mut values = BTreeMap::new();
    match flatten_variables(&document, "", &mut values) {
        Ok(()) => Variables {
            values,
            ok: true,
            note: String::new(),
        },
        Err(note) => Variables {
            values: BTreeMap::new(),
            ok: false,
            note,
        },
    }
}

fn flatten_variables(
    values: &serde_json::Map<String, Value>,
    prefix: &str,
    output: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    for (key, value) in values {
        let name = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            Value::Object(children) => flatten_variables(children, &name, output)?,
            Value::Array(_) => {
                return Err(format!("'{name}' is a list; a var must be a single value"));
            }
            Value::Null => return Err(format!("'{name}' has no value")),
            Value::Bool(value) => {
                output.insert(name, value.to_string());
            }
            Value::String(value) => {
                output.insert(name, value.clone());
            }
            Value::Number(value) => {
                output.insert(name, value.to_string());
            }
        }
    }
    Ok(())
}

fn references(template: &str) -> Vec<String> {
    let mut found = BTreeSet::new();
    let mut remaining = template;
    while let Some(open) = remaining.find("{{") {
        remaining = &remaining[open + 2..];
        let Some(close) = remaining.find("}}") else {
            break;
        };
        let name = remaining[..close].trim();
        if valid_variable(name) {
            found.insert(name.to_string());
        }
        remaining = &remaining[close + 2..];
    }
    found.into_iter().collect()
}

fn render_template(template: &str, values: &BTreeMap<String, String>) -> (String, Vec<String>) {
    let mut rendered = String::with_capacity(template.len());
    let mut missing = BTreeSet::new();
    let mut remaining = template;
    while let Some(open) = remaining.find("{{") {
        rendered.push_str(&remaining[..open]);
        let candidate = &remaining[open + 2..];
        let Some(close) = candidate.find("}}") else {
            rendered.push_str(&remaining[open..]);
            remaining = "";
            break;
        };
        let name = candidate[..close].trim();
        if valid_variable(name) {
            if let Some(value) = values.get(name) {
                rendered.push_str(value);
            } else {
                missing.insert(name.to_string());
                rendered.push_str(&remaining[open..open + 2 + close + 2]);
            }
        } else {
            rendered.push_str(&remaining[open..open + 2 + close + 2]);
        }
        remaining = &candidate[close + 2..];
    }
    rendered.push_str(remaining);
    (rendered, missing.into_iter().collect())
}

fn valid_variable(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_.-".contains(character))
}

fn decrypt(context: &Context, source: &Path) -> Result<Vec<u8>, String> {
    let output = Command::new("sops")
        .args(["-d"])
        .arg(source)
        .env("SOPS_AGE_KEY_FILE", identity_path(context))
        .output()
        .map_err(|error| format!("cannot run sops: {error}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if detail.is_empty() {
            "sops failed".to_string()
        } else {
            detail
        })
    }
}

fn identity_path(context: &Context) -> PathBuf {
    context.state.join("age/keys.txt")
}

fn secure_package_directories(
    context: &Context,
    configuration: &Configuration,
) -> Result<Vec<PathBuf>, String> {
    let mut directories = Vec::new();
    for package in configuration
        .packages
        .iter()
        .filter(|package| package.kind == PackageKind::Secret)
    {
        let destination =
            configuration.map_destination(context, &package.name, &package.package, Path::new(""));
        if never_fold(context, &destination) {
            continue;
        }
        match fs::metadata(&destination) {
            Ok(metadata) if metadata.is_dir() => directories.push(destination),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("read {}: {error}", destination.display())),
        }
    }
    Ok(directories)
}

fn write_private(path: &Path, content: &[u8]) -> Result<(), String> {
    write_private_before_persist(path, content, |_| Ok(()))
}

fn write_private_before_persist<F>(
    path: &Path,
    content: &[u8],
    before_persist: F,
) -> Result<(), String>
where
    F: FnOnce(&Path) -> Result<(), String>,
{
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent", path.display()))?;
    create_private_directories(parent)?;
    use std::io::Write;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("create temporary file in {}: {error}", parent.display()))?;
    set_file_mode(temporary.as_file(), FILE_MODE, path)?;
    temporary
        .write_all(content)
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    temporary
        .flush()
        .map_err(|error| format!("flush {}: {error}", path.display()))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| format!("sync {}: {error}", path.display()))?;
    before_persist(temporary.path())?;
    temporary
        .persist(path)
        .map_err(|error| format!("replace {}: {}", path.display(), error.error))?;
    Ok(())
}

fn create_private_directories(path: &Path) -> Result<(), String> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => return Ok(()),
        Ok(_) => return Err(format!("{} is not a directory", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    }
    if let Some(parent) = path.parent()
        && parent != path
    {
        create_private_directories(parent)?;
    }
    fs::create_dir(path).map_err(|error| format!("create {}: {error}", path.display()))?;
    set_mode(path, DIRECTORY_MODE)
}

#[cfg(unix)]
fn mode_of(path: &Path) -> Result<u32, String> {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map_err(|error| format!("read {}: {error}", path.display()))
        .map(|metadata| metadata.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
fn mode_of(_path: &Path) -> Result<u32, String> {
    Ok(FILE_MODE)
}

#[cfg(unix)]
fn set_file_mode(file: &fs::File, mode: u32, path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(mode))
        .map_err(|error| format!("chmod {}: {error}", path.display()))
}

#[cfg(not(unix))]
fn set_file_mode(_file: &fs::File, _mode: u32, _path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|error| format!("chmod {}: {error}", path.display()))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/sync/secrets_tests.rs"]
mod tests;
