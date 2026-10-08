use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::read_manifest;
pub use crate::config::sorted_directories as directories;
use crate::context::Context;

pub const DEFAULT_GROUPS: &[&str] = &[
    "shared",
    "linux/common",
    "linux/arch",
    "linux/ubuntu",
    "linux/kde",
    "linux/hyprland",
    "linux/server",
    "macos",
];

pub fn package_groups(context: &Context) -> Result<Vec<String>, String> {
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

pub fn validate_packages(context: &Context, groups: &[String]) -> Result<(), String> {
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
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        if metadata.is_dir() {
            collect_named_files(&path, name, found)?;
        } else if path.file_name().and_then(|value| value.to_str()) == Some(name) {
            found.push(path);
        }
    }
    Ok(())
}

pub fn validate_group(group: &str) -> Result<(), String> {
    crate::config::validate_relative(group)?;
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

pub fn validate_package(package: &str) -> Result<(), String> {
    crate::config::validate_relative(package)?;
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
