pub use super::variables::{
    Variables, flatten_variables, load_variables, references, render_template,
};
use std::fs;
use std::path::{Path, PathBuf};

use crate::context::Context;
use crate::event::{Action, Event, EventSink, Phase};

use crate::config::{Configuration, Package, PackageKind, never_fold};

const FILE_MODE: u32 = 0o600;
const DIRECTORY_MODE: u32 = 0o700;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SecretKind {
    Encrypted,
    Template,
    Plain,
}

#[derive(Clone, Debug)]
pub struct SecretEntry {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub kind: SecretKind,
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
    let entries = plan(configuration)?;
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

pub struct SecretResult {
    pub changed: bool,
    pub blocked: bool,
    pub warning: bool,
    pub detail: String,
    pub hint: Option<String>,
}

pub fn materialize(
    context: &Context,
    entry: &mut SecretEntry,
    variables: &Variables,
    dry_run: bool,
    force: bool,
) -> Result<SecretResult, String> {
    let produced = match production(context, entry, variables)? {
        Production::Ready(content) => content,
        Production::Sealed(detail) => {
            return Ok(SecretResult {
                changed: false,
                blocked: false,
                warning: true,
                detail,
                hint: Some("install or import this machine's age identity".into()),
            });
        }
        Production::Invalid(detail) => {
            return Ok(SecretResult {
                changed: false,
                blocked: true,
                warning: true,
                detail,
                hint: Some("check the encrypted file and vars.enc.yaml".into()),
            });
        }
        Production::Plaintext => {
            return Ok(SecretResult {
                changed: false,
                blocked: true,
                warning: true,
                detail: "plaintext file inside a .secret package".into(),
                hint: Some("encrypt it with dotfile secret add".into()),
            });
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
    if let Some(metadata) = metadata {
        if !metadata.is_file() {
            return Ok(SecretResult {
                changed: false,
                blocked: true,
                warning: true,
                detail: "destination is not a regular file".into(),
                hint: None,
            });
        }
        if !matches_content(&entry.destination, &metadata, &produced)? {
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

pub fn plan(configuration: &Configuration) -> Result<Vec<SecretEntry>, String> {
    let mut entries = Vec::new();
    for package in &configuration.packages {
        match package.kind {
            PackageKind::Secret => {
                collect_entries(configuration, package, true, &mut entries)?
            }
            PackageKind::Link => {
                collect_entries(configuration, package, false, &mut entries)?
            }
            PackageKind::NoLink | PackageKind::System => {}
        }
    }
    entries.sort_by(|left, right| left.destination.cmp(&right.destination));
    Ok(entries)
}

pub fn collect_entries(
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
        let mapped = configuration.map_destination(&full).ok_or_else(|| {
            format!("no target declared for {full}; add a rule to config/targets.dotfile")
        })?;
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

pub fn collect_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
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

pub fn kind_of(path: &Path) -> SecretKind {
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

pub fn plain_name(name: &str) -> String {
    let name = name.strip_suffix(".tmpl").unwrap_or(name);
    if let Some(name) = name.strip_suffix(".enc") {
        name.to_string()
    } else {
        name.replacen(".enc.", ".", 1)
    }
}

pub fn decrypt(context: &Context, source: &Path) -> Result<Vec<u8>, String> {
    super::sops::decrypt(context, source, None, false).map(|data| data.to_vec())
}

pub fn identity_path(context: &Context) -> PathBuf {
    context.root_config.join("age/keys.txt")
}

pub fn secure_package_directories(
    context: &Context,
    configuration: &Configuration,
) -> Result<Vec<PathBuf>, String> {
    let mut directories = Vec::new();
    for package in configuration
        .packages
        .iter()
        .filter(|package| package.kind == PackageKind::Secret)
    {
        let Some(destination) = configuration.map_destination(&package.name) else {
            continue;
        };
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

pub fn write_private(path: &Path, content: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent", path.display()))?;
    create_private_directories(parent)?;
    crate::fs::write_private(path, content).map(|_| ())
}

#[cfg(test)]
fn write_private_before_persist<F>(
    path: &Path,
    content: &[u8],
    before_persist: F,
) -> Result<(), String>
where
    F: FnOnce(&Path) -> Result<(), String>,
{
    let temporary = tempfile::NamedTempFile::new_in(path.parent().ok_or("no parent")?)
        .map_err(|e| e.to_string())?;
    before_persist(temporary.path())?;
    write_private(path, content)
}

pub fn create_private_directories(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => return Ok(()),
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
pub fn mode_of(path: &Path) -> Result<u32, String> {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map_err(|error| format!("read {}: {error}", path.display()))
        .map(|metadata| metadata.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
pub fn mode_of(_path: &Path) -> Result<u32, String> {
    Ok(FILE_MODE)
}

#[cfg(unix)]
pub fn set_mode(path: &Path, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .and_then(|file| file.set_permissions(fs::Permissions::from_mode(mode)))
        .map_err(|error| format!("chmod {}: {error}", path.display()))
}

#[cfg(not(unix))]
pub fn set_mode(_path: &Path, _mode: u32) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/sync/secrets_tests.rs"]
mod tests;

pub fn package_entries(
    configuration: &Configuration,
    package: &Package,
    whole_package: bool,
) -> Result<Vec<SecretEntry>, String> {
    let mut entries = Vec::new();
    collect_entries(configuration, package, whole_package, &mut entries)?;
    Ok(entries)
}

enum Production {
    Ready(zeroize::Zeroizing<Vec<u8>>),
    Sealed(String),
    Invalid(String),
    Plaintext,
}

pub(super) fn read_source(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err(format!("not a regular secret: {}", path.display()));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err(format!(
            "secret source exceeds size limit: {}",
            path.display()
        ));
    }
    Ok(bytes)
}

fn read_template(path: &Path) -> Result<String, String> {
    String::from_utf8(read_source(path, super::sops::MAX_SECRET_BYTES)?)
        .map_err(|_| "template is not text".into())
}

fn production(
    context: &Context,
    entry: &SecretEntry,
    variables: &Variables,
) -> Result<Production, String> {
    match entry.kind {
        SecretKind::Plain => Ok(Production::Plaintext),
        SecretKind::Encrypted => {
            if !identity_path(context).is_file() {
                return Ok(Production::Sealed(
                    "sealed; no age identity on this machine".into(),
                ));
            }
            Ok(
                match super::sops::decrypt(context, &entry.source, None, false) {
                    Ok(content) => Production::Ready(content),
                    Err(error) => Production::Invalid(format!("decryption failed: {error}")),
                },
            )
        }
        SecretKind::Template => {
            let template = read_template(&entry.source)?;
            if !references(&template).is_empty() && !variables.ok {
                return Ok(Production::Sealed(format!("sealed; {}", variables.note)));
            }
            let (content, missing) = render_template(&template, &variables.values);
            if !missing.is_empty() {
                return Ok(Production::Invalid(format!(
                    "unknown: {}",
                    missing.join(" ")
                )));
            }
            if content.len() > super::sops::MAX_SECRET_BYTES {
                return Err("rendered template exceeds size limit".into());
            }
            Ok(Production::Ready(zeroize::Zeroizing::new(
                content.into_bytes(),
            )))
        }
    }
}

/// Produce destination bytes using the same policy as sync, status and apply.
/// System callers handle their public (Plain) entries themselves.
pub fn produce(
    context: &Context,
    entry: &SecretEntry,
    variables: &Variables,
) -> Result<Vec<u8>, String> {
    match production(context, entry, variables)? {
        Production::Ready(content) => Ok(content.to_vec()),
        Production::Sealed(detail) | Production::Invalid(detail) => Err(detail),
        Production::Plaintext => Err("plaintext file inside a .secret package".into()),
    }
}

pub fn inspect(
    context: &Context,
    entry: &SecretEntry,
    variables: &Variables,
) -> Result<&'static str, String> {
    let content = match production(context, entry, variables)? {
        Production::Ready(content) => content,
        Production::Sealed(_) => return Ok("sealed"),
        Production::Invalid(error) => return Err(error),
        Production::Plaintext => return Ok("plaintext"),
    };
    match fs::symlink_metadata(&entry.destination) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok("absent"),
        Err(e) => Err(e.to_string()),
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => Ok("drifted"),
        Ok(metadata) => {
            if !matches_content(&entry.destination, &metadata, &content)? {
                Ok("drifted")
            } else if mode_of(&entry.destination)? != FILE_MODE {
                Ok("remoded")
            } else {
                Ok("current")
            }
        }
    }
}

pub fn clean(
    context: &Context,
    entry: &SecretEntry,
    variables: &Variables,
    dry_run: bool,
) -> Result<&'static str, String> {
    if entry
        .destination
        .symlink_metadata()
        .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
    {
        return Ok("absent");
    }
    let state = inspect(context, entry, variables)?;
    if matches!(state, "current" | "remoded") {
        if !dry_run {
            fs::remove_file(&entry.destination).map_err(|e| e.to_string())?;
        }
        Ok("cleaned")
    } else {
        Ok(state)
    }
}

fn matches_content(path: &Path, metadata: &fs::Metadata, expected: &[u8]) -> Result<bool, String> {
    use std::io::Read;
    if metadata.len() != expected.len() as u64 {
        return Ok(false);
    }
    let mut current = zeroize::Zeroizing::new(Vec::new());
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Ok(false);
    }
    file.take(expected.len() as u64 + 1)
        .read_to_end(&mut current)
        .map_err(|e| e.to_string())?;
    Ok(current.as_slice() == expected)
}
