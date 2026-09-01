use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::context::{Context, write_atomic};
use crate::event::{Event, EventSink};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PackageKind {
    Link,
    NoLink,
    Secret,
    System,
}

#[derive(Clone, Debug)]
pub struct Package {
    pub kind: PackageKind,
    pub directory: PathBuf,
    pub name: String,
    pub package: String,
}

#[derive(Clone, Debug)]
pub struct Configuration {
    pub targets: BTreeMap<String, PathBuf>,
    pub groups: Vec<String>,
    pub active_override_dirs: Vec<PathBuf>,
    pub overrides: BTreeMap<String, String>,
    pub packages: Vec<Package>,
}

impl Configuration {
    pub fn load(
        context: &Context,
        profile: &str,
        requested_overrides: &[String],
        events: &dyn EventSink,
    ) -> Result<Self, String> {
        let targets = load_targets(context)?;
        let mut overrides = load_overrides(&context.overrides_file)?;
        for specification in requested_overrides {
            let (group, name) = specification
                .split_once('=')
                .ok_or_else(|| "--override needs <group>=<name|none>".to_string())?;
            let directory = context.root.join(group).join("overrides");
            if !directory.is_dir() {
                return Err(format!("group has no overrides: {group}"));
            }
            if name != "none" && !directory.join(name).is_dir() {
                let available = sorted_directories(&directory)?
                    .into_iter()
                    .filter_map(|path| {
                        path.file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                return Err(format!(
                    "unknown override '{name}' for {group} (available: {available})"
                ));
            }
            overrides.insert(group.to_string(), name.to_string());
        }
        let manifest = read_manifest(&context.manifest(profile))?;
        let mut groups = Vec::new();
        let mut active_override_dirs = Vec::new();
        let mut notices = Vec::new();
        for group in manifest {
            groups.push(group.clone());
            let directory = context.root.join(&group).join("overrides");
            if !directory.is_dir() {
                continue;
            }
            match overrides.get(&group).map(String::as_str) {
                None | Some("") => {
                    let available = sorted_directories(&directory)?
                        .into_iter()
                        .filter_map(|path| {
                            path.file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    notices.push((
                        format!("no machine override selected for '{group}'"),
                        format!("use --override {group}=<name> or =none; available: {available}"),
                    ));
                    events.emit(Event::Item {
                        action: crate::event::Action::Check,
                        path: directory.clone(),
                        detail: "no machine override selected".to_string(),
                        changed: false,
                    });
                }
                Some("none") => {}
                Some(name) => {
                    let selected = directory.join(name);
                    if selected.is_dir() {
                        groups.push(format!("{group}/overrides/{name}"));
                        active_override_dirs.push(selected);
                    } else {
                        notices.push((
                            format!("saved override no longer exists: {group}/{name}"),
                            format!("choose another with --override {group}=<name|none>"),
                        ));
                        events.emit(Event::Item {
                            action: crate::event::Action::Check,
                            path: selected,
                            detail: "saved override is missing".to_string(),
                            changed: false,
                        });
                    }
                }
            }
        }
        if let Some((message, hint)) = notices.first() {
            events.emit(Event::Warning {
                message: if notices.len() == 1 {
                    message.clone()
                } else {
                    format!(
                        "{} override selections need attention; first: {message}",
                        notices.len()
                    )
                },
                hint: Some(hint.clone()),
            });
        }
        let packages = collect_packages(context, &groups)?;
        Ok(Self {
            targets,
            groups,
            active_override_dirs,
            overrides,
            packages,
        })
    }

    pub fn save_overrides(&self, context: &Context, dry_run: bool) -> Result<(), String> {
        if dry_run {
            return Ok(());
        }
        let content = self
            .overrides
            .iter()
            .map(|(group, name)| format!("{group}={name}\n"))
            .collect::<String>();
        write_atomic(&context.overrides_file, content.as_bytes())
    }

    pub fn map_destination(
        &self,
        context: &Context,
        full: &str,
        package: &str,
        relative: &Path,
    ) -> PathBuf {
        let best = self
            .targets
            .keys()
            .filter(|key| {
                full == key.as_str()
                    || full
                        .strip_prefix(key.as_str())
                        .is_some_and(|rest| rest.starts_with('/'))
            })
            .max_by_key(|key| key.len());
        match best {
            None => {
                let mut destination = context.home.join(".config").join(package);
                if !relative.as_os_str().is_empty() {
                    destination.push(relative);
                }
                destination
            }
            Some(key) if full == key => self.targets[key].clone(),
            Some(key) => self.targets[key].join(&full[key.len() + 1..]),
        }
    }

    pub fn has_target_under(&self, prefix: &str) -> bool {
        self.targets.keys().any(|key| {
            key.strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('/'))
        })
    }
}

pub fn never_fold(context: &Context, path: &Path) -> bool {
    [
        context.home.clone(),
        context.home.join(".config"),
        context.home.join(".local"),
        context.home.join(".local/share"),
        context.home.join(".local/bin"),
        context.home.join(".config/systemd"),
        context.home.join(".config/systemd/user"),
    ]
    .iter()
    .any(|protected| protected == path)
}

fn read_manifest(path: &Path) -> Result<Vec<String>, String> {
    let text =
        fs::read_to_string(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    Ok(text
        .lines()
        .filter_map(|line| {
            let group = line.split('#').next().unwrap_or_default().trim();
            (!group.is_empty()).then(|| group.to_string())
        })
        .collect())
}

fn load_overrides(path: &Path) -> Result<BTreeMap<String, String>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BTreeMap::new());
        }
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    Ok(text
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(group, name)| (group.to_string(), name.to_string()))
        .collect())
}

fn load_targets(context: &Context) -> Result<BTreeMap<String, PathBuf>, String> {
    let text = match fs::read_to_string(&context.targets_file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BTreeMap::new());
        }
        Err(error) => {
            return Err(format!("read {}: {error}", context.targets_file.display()));
        }
    };
    let family = platform_family()?;
    let mut unscoped = BTreeMap::new();
    let mut scoped = BTreeMap::<String, BTreeMap<String, PathBuf>>::new();
    for (offset, raw) in text.lines().enumerate() {
        let Some((key, value)) = raw.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = expand_home(context, value.trim());
        let (scope, path) = split_scope(key).map_err(|error| {
            format!("{}:{}: {error}", context.targets_file.display(), offset + 1)
        })?;
        if let Some(scope) = scope {
            scoped
                .entry(scope.to_string())
                .or_default()
                .insert(path.to_string(), value);
        } else {
            unscoped.insert(path.to_string(), value);
        }
    }
    if let Some(selected) = scoped.remove(family) {
        unscoped.extend(selected);
    }
    Ok(unscoped)
}

fn platform_family() -> Result<&'static str, String> {
    if let Ok(forced) = std::env::var("DOTFILE_PLATFORM") {
        return match forced.as_str() {
            "macos" => Ok("macos"),
            "linux" => Ok("linux"),
            _ => Err("DOTFILE_PLATFORM must be one of: macos, linux".to_string()),
        };
    }
    if cfg!(target_os = "macos") {
        Ok("macos")
    } else if cfg!(target_os = "linux") {
        Ok("linux")
    } else {
        Ok("")
    }
}

fn split_scope(key: &str) -> Result<(Option<&str>, &str), String> {
    let Some((scope, path)) = key.split_once(':') else {
        return Ok((None, key));
    };
    if !matches!(scope, "macos" | "linux") {
        return Err(format!(
            "unknown target scope '{scope}:' (expected one of: macos, linux)"
        ));
    }
    if path.is_empty() {
        return Err(format!("missing path after '{scope}:'"));
    }
    Ok((Some(scope), path))
}

fn expand_home(context: &Context, value: &str) -> PathBuf {
    value.strip_prefix('~').map_or_else(
        || PathBuf::from(value),
        |suffix| context.home.join(suffix.trim_start_matches('/')),
    )
}

fn collect_packages(context: &Context, groups: &[String]) -> Result<Vec<Package>, String> {
    let mut packages = Vec::new();
    for group in groups {
        let group_directory = context.root.join(group);
        for directory in sorted_directories(&group_directory)? {
            let package = directory
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
                .ok_or_else(|| {
                    format!(
                        "package directory name is not valid UTF-8: {}",
                        directory.display()
                    )
                })?;
            if package == "overrides" {
                continue;
            }
            let kind = if marker_exists(&directory.join(".nolink"))? {
                PackageKind::NoLink
            } else if marker_exists(&directory.join(".secret"))? {
                PackageKind::Secret
            } else if marker_exists(&directory.join(".system"))? {
                PackageKind::System
            } else {
                PackageKind::Link
            };
            packages.push(Package {
                kind,
                directory,
                name: format!("{group}/{package}"),
                package,
            });
        }
    }
    Ok(packages)
}

fn sorted_directories(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("read {}: {error}", directory.display())),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("read {}: {error}", directory.display()))?;
        let path = entry.path();
        let metadata =
            fs::metadata(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
        if metadata.is_dir() {
            found.push(path);
        }
    }
    found.sort_by(|left, right| left.file_name().cmp(&right.file_name()));
    Ok(found)
}

fn marker_exists(path: &Path) -> Result<bool, String> {
    match fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("read {}: {error}", path.display())),
    }
}
