use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::context::{Context, write_atomic};
use crate::event::{Action, Event, EventSink, Phase};

const DEFAULT_GROUPS: &[&str] = &[
    "shared",
    "linux/common",
    "linux/arch",
    "linux/ubuntu",
    "linux/kde",
    "linux/hyprland",
    "linux/server",
    "macos",
];

pub fn synchronize(
    context: &Context,
    dry_run: bool,
    events: &dyn EventSink,
) -> Result<usize, String> {
    events.emit(Event::PhaseStarted {
        phase: Phase::Artifacts,
        total: Some(2),
    });
    let metadata = load_metadata(&context.packages_config)?;
    let groups = package_groups(context)?;
    validate_packages(context, &groups)?;
    let (config, document) = render(context, &groups, &metadata)?;
    let outputs = [
        (&context.packages_config, config, "package index"),
        (&context.packages_doc, document, "package documentation"),
    ];
    let mut changed = 0;
    for (index, (path, content, detail)) in outputs.into_iter().enumerate() {
        let differs = match fs::read(path) {
            Ok(current) => current != content.as_bytes(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
            Err(error) => return Err(format!("read {}: {error}", path.display())),
        };
        if differs {
            changed += 1;
            if !dry_run {
                write_atomic(path, content.as_bytes())?;
            }
        }
        events.emit(Event::Item {
            action: Action::Generate,
            path: path.clone(),
            detail: detail.to_string(),
            changed: differs,
        });
        events.emit(Event::Progress {
            phase: Phase::Artifacts,
            completed: index + 1,
            total: Some(2),
            label: detail.to_string(),
        });
    }
    Ok(changed)
}

fn load_metadata(path: &Path) -> Result<BTreeMap<String, String>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BTreeMap::new());
        }
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    let mut current = None::<String>;
    let mut metadata = BTreeMap::new();
    for (offset, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        if let Some(group) = line.strip_suffix('{') {
            let group = group.trim();
            validate_group(group)
                .map_err(|error| format!("{}:{}: {error}", path.display(), offset + 1))?;
            current = Some(group.to_string());
            continue;
        }
        if line == "}" {
            current = None;
            continue;
        }
        let group = current
            .as_ref()
            .ok_or_else(|| format!("{}:{}: package outside a group", path.display(), offset + 1))?;
        let (name, description) = line
            .split_once('=')
            .map_or((line, ""), |(name, description)| {
                (name.trim(), description.trim())
            });
        validate_package(name)
            .map_err(|error| format!("{}:{}: {error}", path.display(), offset + 1))?;
        let key = format!("{group}/{name}");
        if metadata
            .insert(key.clone(), description.to_string())
            .is_some()
        {
            return Err(format!(
                "{}:{}: duplicate package: {key}",
                path.display(),
                offset + 1
            ));
        }
    }
    Ok(metadata)
}

fn package_groups(context: &Context) -> Result<Vec<String>, String> {
    let mut groups = DEFAULT_GROUPS
        .iter()
        .map(|group| (*group).to_string())
        .collect::<Vec<_>>();
    let mut manifests = Vec::new();
    collect_named_files(&context.environment_dir, "manifest", &mut manifests)?;
    manifests.sort();
    for manifest in manifests {
        groups.extend(read_manifest(&manifest)?);
    }
    let mut seen = BTreeSet::new();
    groups.retain(|group| !group.is_empty() && seen.insert(group.clone()));
    Ok(groups)
}

fn validate_packages(context: &Context, groups: &[String]) -> Result<(), String> {
    for group in groups {
        for package in directories(&context.root.join(group))? {
            let name = package
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    format!(
                        "package directory name is not valid UTF-8: {}",
                        package.display()
                    )
                })?;
            if name != "overrides" {
                validate_package(name).map_err(|_| {
                    format!("package directory has an unsupported name: {group}/{name}")
                })?;
            }
        }
    }
    Ok(())
}

fn render(
    context: &Context,
    groups: &[String],
    metadata: &BTreeMap<String, String>,
) -> Result<(String, String), String> {
    let mut config = String::new();
    let mut document = String::new();
    let mut wrote_group = false;
    for group in groups {
        let mut packages = Vec::new();
        for path in directories(&context.root.join(group))? {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    format!(
                        "package directory name is not valid UTF-8: {}",
                        path.display()
                    )
                })?;
            if name != "overrides" {
                packages.push(name.to_string());
            }
        }
        if packages.is_empty() {
            continue;
        }
        if wrote_group {
            config.push('\n');
        }
        config.push_str(group);
        config.push_str(" {\n");
        document.push_str("\n## `");
        document.push_str(group);
        document.push_str("`\n\n");
        let width = packages
            .iter()
            .filter(|package| {
                metadata
                    .get(&format!("{group}/{package}"))
                    .is_some_and(|description| !description.is_empty())
            })
            .map(String::len)
            .max()
            .unwrap_or(0)
            + 2;
        for package in packages {
            let description = metadata
                .get(&format!("{group}/{package}"))
                .map(String::as_str)
                .unwrap_or_default();
            config.push_str("  ");
            config.push_str(&package);
            if !description.is_empty() {
                config.push_str(&" ".repeat(width.saturating_sub(package.len())));
                config.push_str("= ");
                config.push_str(description);
            }
            config.push('\n');
            document.push_str("- `");
            document.push_str(&package);
            document.push('`');
            if !description.is_empty() {
                document.push_str(" — ");
                document.push_str(description);
            }
            document.push('\n');
        }
        config.push_str("}\n");
        wrote_group = true;
    }
    if config.ends_with('\n') {
        config.pop();
    }
    Ok((config, document))
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

fn directories(path: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("read {}: {error}", path.display()))?;
        let child = entry.path();
        let metadata =
            fs::metadata(&child).map_err(|error| format!("read {}: {error}", child.display()))?;
        if metadata.is_dir() {
            found.push(child);
        }
    }
    found.sort_by(|left, right| left.file_name().cmp(&right.file_name()));
    Ok(found)
}

fn collect_named_files(
    directory: &Path,
    name: &str,
    found: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("read {}: {error}", directory.display())),
    };
    for entry in entries {
        let entry = entry.map_err(|error| format!("read {}: {error}", directory.display()))?;
        let path = entry.path();
        let metadata =
            fs::metadata(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
        if metadata.is_dir() {
            collect_named_files(&path, name, found)?;
        } else if path.file_name().and_then(|value| value.to_str()) == Some(name) {
            found.push(path);
        }
    }
    Ok(())
}

fn validate_group(group: &str) -> Result<(), String> {
    if group.is_empty() {
        return Err("empty group".to_string());
    }
    if group
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "._/-".contains(character))
    {
        Ok(())
    } else {
        Err(format!("invalid group: {group}"))
    }
}

fn validate_package(package: &str) -> Result<(), String> {
    if package.is_empty() {
        return Err("empty package".to_string());
    }
    if package
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "._+@-".contains(character))
    {
        Ok(())
    } else {
        Err(format!("invalid package: {package}"))
    }
}
