use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use hostkit::process::{CaptureLimits, CapturedOutput, output};
use serde::Deserialize;

use super::catalog::Catalog;

pub(super) fn select(root: &Path, catalog: &mut Catalog, reference: &str) -> Result<(), String> {
    let files = changed_files(root, reference)?;
    let mut names = BTreeSet::new();
    let mut all_python = false;
    for file in files {
        if let Some(package) = catalog
            .rust
            .iter()
            .find(|package| file.starts_with(&package.directory))
        {
            names.insert(package.name.clone());
        } else if file.starts_with("scripts/rust")
            || file
                .file_name()
                .is_some_and(|name| name == "rust-toolchain.toml")
        {
            names.extend(catalog.rust.iter().map(|package| package.name.clone()));
            all_python = true;
        } else if let Ok(relative) = file.strip_prefix("scripts/python/tests") {
            let group = relative
                .iter()
                .next()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if catalog.python.iter().any(|name| name == group) {
                names.insert(group.to_owned());
            } else {
                all_python = true;
            }
        } else if file.starts_with("scripts/python") {
            all_python = true;
        } else if file.starts_with("shared/tmux") {
            names.extend(["tmux-workspace", "tmux", "zsh", "wezterm"].map(String::from));
        } else if file.starts_with("shared/zsh") {
            names.extend(["zsh", "tmux"].map(String::from));
        } else if file.starts_with("shared/wezterm") {
            names.extend(["wezterm", "tmux"].map(String::from));
        } else if file.starts_with("shared/obsidian/plugins/agent-transcripts") {
            names.extend(["agent-transcripts", "transcript"].map(String::from));
        } else {
            return Ok(());
        }
    }
    if catalog
        .rust
        .iter()
        .any(|package| names.contains(&package.name))
    {
        dependents(root, &mut names)?;
        if names.contains("dotfile-cli")
            || names.contains("doc-keybinds")
            || names.contains("dotfmt")
        {
            all_python = true;
        }
        if names.contains("tmux-workspace") || names.contains("agent-hop") {
            names.insert("tmux".into());
        }
        if names.contains("sysinfo-collect") {
            names.insert("utils".into());
        }
    }
    if all_python {
        names.extend(catalog.python.iter().cloned());
    }
    catalog.affected = Some(names);
    Ok(())
}

fn changed_files(root: &Path, reference: &str) -> Result<Vec<PathBuf>, String> {
    let head = capture(Command::new("git").current_dir(root).args([
        "rev-parse",
        "--verify",
        "--quiet",
        "HEAD",
    ]))?;
    let mut files = if !head.status.success() && reference == "HEAD" {
        checked(
            Command::new("git")
                .current_dir(root)
                .args(["ls-files", "-z", "--cached"]),
        )?
    } else {
        let commit = checked(Command::new("git").current_dir(root).args([
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{reference}^{{commit}}"),
        ]))?;
        let commit = String::from_utf8_lossy(&commit);
        let base = checked(Command::new("git").current_dir(root).args([
            "merge-base",
            "HEAD",
            commit.trim(),
        ]))?;
        let base = String::from_utf8_lossy(&base);
        checked(Command::new("git").current_dir(root).args([
            "diff",
            "--name-only",
            "-z",
            "--no-renames",
            base.trim(),
            "--",
        ]))?
    };
    files.extend(checked(Command::new("git").current_dir(root).args([
        "ls-files",
        "-z",
        "--others",
        "--exclude-standard",
    ]))?);
    let mut paths: Vec<_> = files
        .split(|byte| *byte == 0)
        .filter(|value| !value.is_empty())
        .map(|value| {
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStrExt;
                PathBuf::from(std::ffi::OsStr::from_bytes(value))
            }
            #[cfg(not(unix))]
            PathBuf::from(String::from_utf8_lossy(value).as_ref())
        })
        .collect();
    paths.sort();
    paths.dedup();
    Ok(paths)
}

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    dependencies: Vec<Dependency>,
}

#[derive(Deserialize)]
struct Dependency {
    name: String,
}

fn dependents(root: &Path, names: &mut BTreeSet<String>) -> Result<(), String> {
    let bytes = checked(
        Command::new("cargo")
            .current_dir(root.join("scripts/rust"))
            .args(["metadata", "--locked", "--no-deps", "--format-version", "1"]),
    )?;
    let metadata: Metadata =
        serde_json::from_slice(&bytes).map_err(|error| format!("cargo metadata: {error}"))?;
    loop {
        let affected: Vec<_> = metadata
            .packages
            .iter()
            .filter(|package| {
                !names.contains(&package.name)
                    && package
                        .dependencies
                        .iter()
                        .any(|dependency| names.contains(&dependency.name))
            })
            .map(|package| package.name.clone())
            .collect();
        if affected.is_empty() {
            return Ok(());
        }
        names.extend(affected);
    }
}

fn checked(command: &mut Command) -> Result<Vec<u8>, String> {
    let result = capture(command)?;
    if result.status.success() && !result.stdout_truncated {
        Ok(result.stdout)
    } else {
        Err(format!(
            "{}: {}",
            command.get_program().to_string_lossy(),
            String::from_utf8_lossy(&result.stderr).trim()
        ))
    }
}

fn capture(command: &mut Command) -> Result<CapturedOutput, String> {
    output(
        command,
        CaptureLimits {
            stdout: 16 * 1024 * 1024,
            stderr: 64 * 1024,
        },
        Duration::from_secs(30),
    )
    .map_err(|error| error.to_string())
}
