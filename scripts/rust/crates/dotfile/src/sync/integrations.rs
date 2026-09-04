use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use crate::context::{Context, write_atomic};
use crate::event::{Action, Event, EventSink, Phase};

use super::config::Configuration;

#[derive(Default)]
pub struct IntegrationOutcome {
    pub checked: usize,
    pub changed: usize,
    pub generated: usize,
}

pub fn synchronize(
    context: &Context,
    configuration: &Configuration,
    dry_run: bool,
    events: &dyn EventSink,
) -> Result<IntegrationOutcome, String> {
    synchronize_with_systemd(
        context,
        configuration,
        dry_run,
        command_exists("systemctl"),
        events,
    )
}

fn synchronize_with_systemd(
    context: &Context,
    configuration: &Configuration,
    dry_run: bool,
    systemd_available: bool,
    events: &dyn EventSink,
) -> Result<IntegrationOutcome, String> {
    events.emit(Event::PhaseStarted {
        phase: Phase::Integrations,
        total: None,
    });
    let mut outcome = IntegrationOutcome::default();
    let mut warnings = Vec::new();
    if systemd_available {
        systemd_legacy_cleanup(context, dry_run, events, &mut outcome)?;
    }
    if systemd_available
        && configuration
            .groups
            .iter()
            .any(|group| group == "linux/common")
    {
        systemd_theme_watch(context, dry_run, events, &mut outcome, &mut warnings)?;
    }
    if configuration
        .groups
        .iter()
        .any(|group| group == "linux/hyprland")
    {
        hyprland(context, dry_run, events, &mut outcome, &mut warnings)?;
    }
    secret_health(context, events, &mut outcome, &mut warnings);
    if let Some((message, hint)) = warnings.first() {
        events.emit(Event::Warning {
            message: if warnings.len() == 1 {
                message.clone()
            } else {
                format!(
                    "{} integration checks need attention; first: {message}",
                    warnings.len()
                )
            },
            hint: hint.clone(),
        });
    }
    Ok(outcome)
}

fn systemd_theme_watch(
    context: &Context,
    dry_run: bool,
    events: &dyn EventSink,
    outcome: &mut IntegrationOutcome,
    warnings: &mut Vec<(String, Option<String>)>,
) -> Result<(), String> {
    crate::cancel::check()?;
    let unit_directory = context.home.join(".config/systemd/user");
    let watcher = unit_directory.join("theme-watch.path");
    if !file_exists(&watcher)? {
        return Ok(());
    }
    outcome.checked += 1;
    let enabled = systemctl(&["is-enabled", "--quiet", "theme-watch.path"]);
    let active = systemctl(&["is-active", "--quiet", "theme-watch.path"]);
    let current = enabled && active;
    let mut enabled_now = false;
    if !current && !dry_run {
        crate::cancel::check()?;
        let _ = systemctl(&["daemon-reload"]);
        enabled_now = systemctl(&["enable", "--now", "theme-watch.path"]);
        if !enabled_now {
            warnings.push((
                "theme-watch.path could not be enabled".to_string(),
                Some("run systemctl --user enable --now theme-watch.path".to_string()),
            ));
        }
    }
    let changed = !current && (dry_run || enabled_now);
    if changed {
        outcome.changed += 1;
    }
    events.emit(Event::Item {
        action: Action::Sync,
        path: watcher,
        detail: if current {
            "enabled"
        } else if dry_run {
            "would enable"
        } else if enabled_now {
            "enabled now"
        } else {
            "enable failed"
        }
        .to_string(),
        changed,
    });
    Ok(())
}

fn systemd_legacy_cleanup(
    context: &Context,
    dry_run: bool,
    events: &dyn EventSink,
    outcome: &mut IntegrationOutcome,
) -> Result<(), String> {
    crate::cancel::check()?;
    let unit_directory = context.home.join(".config/systemd/user");
    let old_path = unit_directory.join("generate-theme.path");
    let old_wants = unit_directory.join("default.target.wants/generate-theme.path");
    outcome.checked += 1;
    if symlink_exists(&old_path)? || symlink_exists(&old_wants)? {
        if !dry_run {
            let _ = systemctl(&["disable", "generate-theme.path"]);
            for path in [
                old_path.clone(),
                unit_directory.join("generate-theme.service"),
                old_wants,
            ] {
                remove_file_if_present(&path)?;
            }
            let _ = systemctl(&["daemon-reload"]);
        }
        outcome.changed += 1;
        events.emit(Event::Item {
            action: Action::Prune,
            path: old_path,
            detail: "legacy theme watcher".to_string(),
            changed: true,
        });
    }
    Ok(())
}

fn hyprland(
    context: &Context,
    dry_run: bool,
    events: &dyn EventSink,
    outcome: &mut IntegrationOutcome,
    warnings: &mut Vec<(String, Option<String>)>,
) -> Result<(), String> {
    crate::cancel::check()?;
    let elephant_source = context.root.join("linux/hyprland/elephant/files.toml");
    if file_exists(&elephant_source)? {
        outcome.checked += 1;
        let template = fs::read_to_string(&elephant_source)
            .map_err(|error| format!("read {}: {error}", elephant_source.display()))?;
        let rendered = template.replace("$HOME", &context.home.to_string_lossy());
        let destination = context.home.join(".config/elephant/files.toml");
        let differs = fs::read_to_string(&destination).map_or(true, |current| current != rendered);
        if differs && !dry_run {
            write_atomic(&destination, rendered.as_bytes())?;
        }
        if differs {
            outcome.changed += 1;
            outcome.generated += 1;
        }
        events.emit(Event::Item {
            action: Action::Generate,
            path: destination,
            detail: if differs { "expanded $HOME" } else { "current" }.to_string(),
            changed: differs,
        });
    }
    let stale = context.root.join("linux/hyprland/hypr/conf.d/local.conf");
    outcome.checked += 1;
    if symlink_exists(&stale)? && !target_exists(&stale)? {
        if !dry_run {
            fs::remove_file(&stale)
                .map_err(|error| format!("remove {}: {error}", stale.display()))?;
        }
        outcome.changed += 1;
        events.emit(Event::Item {
            action: Action::Prune,
            path: stale,
            detail: "broken local Hyprland override".to_string(),
            changed: true,
        });
    }
    let wallpaper = context.home.join(".config/hypr/wallpaper.png");
    outcome.checked += 1;
    if !file_exists(&wallpaper)? {
        warnings.push((
            format!("wallpaper is missing: {}", wallpaper.display()),
            Some("place a wallpaper at that path".to_string()),
        ));
        events.emit(Event::Item {
            action: Action::Check,
            path: wallpaper,
            detail: "missing wallpaper".to_string(),
            changed: false,
        });
    }
    if !dry_run && command_exists("hyprctl") {
        crate::cancel::check()?;
        let _ = Command::new("hyprctl").arg("reload").output();
    }
    Ok(())
}

fn secret_health(
    context: &Context,
    events: &dyn EventSink,
    outcome: &mut IntegrationOutcome,
    warnings: &mut Vec<(String, Option<String>)>,
) {
    let mut encrypted = BTreeSet::new();
    collect_encrypted(&context.root, &mut encrypted);
    if encrypted.is_empty() {
        return;
    }
    outcome.checked += 1;
    if !command_exists("sops") {
        health_issue(
            events,
            warnings,
            context.root.join("vars.enc.yaml"),
            format!(
                "{} encrypted file{} tracked but sops is missing",
                encrypted.len(),
                if encrypted.len() == 1 { " is" } else { "s are" }
            ),
            Some("install sops before applying secrets".to_string()),
        );
    }
    let identity = context.state.join("age/keys.txt");
    outcome.checked += 1;
    if !identity.is_file() {
        health_issue(
            events,
            warnings,
            identity.clone(),
            "this machine has no age identity".to_string(),
            Some("run dotfile secret init or import an existing identity".to_string()),
        );
    } else if mode_of(&identity).is_some_and(|mode| mode & 0o077 != 0) {
        health_issue(
            events,
            warnings,
            identity.clone(),
            format!(
                "age identity permissions are too broad: {mode:04o}",
                mode = mode_of(&identity).unwrap_or_default()
            ),
            Some(format!("chmod 600 {}", identity.display())),
        );
    }
    let recipients = load_recipients(&context.root.join("config/keys.dotfile"));
    outcome.checked += 1;
    if recipients.is_empty() {
        health_issue(
            events,
            warnings,
            context.root.join("config/keys.dotfile"),
            "no age recipients are enrolled".to_string(),
            Some("run dotfile secret enroll <label>".to_string()),
        );
    } else if !recipients
        .keys()
        .any(|label| label.to_ascii_lowercase().starts_with("recovery"))
    {
        health_issue(
            events,
            warnings,
            context.root.join("config/keys.dotfile"),
            "no recovery recipient is enrolled".to_string(),
            Some("enroll an offline recipient named recovery*".to_string()),
        );
    }
    if !recipients.is_empty() {
        outcome.checked += 1;
        let expected = format!(
            "creation_rules:\n  - age: {}\n",
            recipients.values().cloned().collect::<Vec<_>>().join(",")
        );
        let sops_config = context.root.join(".sops.yaml");
        if fs::read_to_string(&sops_config).ok().as_deref() != Some(expected.as_str()) {
            health_issue(
                events,
                warnings,
                sops_config,
                ".sops.yaml does not match config/keys.dotfile".to_string(),
                Some("run dotfile secret sync".to_string()),
            );
        }
    }
    outcome.checked += 1;
    let configured_hooks =
        git_config(context, &["--path", "--get", "core.hooksPath"]).map(|path| {
            let path = PathBuf::from(path);
            normalize_path(if path.is_absolute() {
                path
            } else {
                context.root.join(path)
            })
        });
    let expected_hooks = normalize_path(context.root.join(".githooks"));
    if configured_hooks.as_ref() != Some(&expected_hooks) {
        health_issue(
            events,
            warnings,
            context.root.join(".git/config"),
            "git hooksPath does not resolve to this repository's .githooks".to_string(),
            Some("git config core.hooksPath .githooks".to_string()),
        );
    }
    for hook in ["pre-commit", "pre-push"] {
        let path = context.root.join(".githooks").join(hook);
        outcome.checked += 1;
        if !is_executable(&path) {
            health_issue(
                events,
                warnings,
                path,
                format!("{hook} hook is missing or not executable"),
                Some(format!("chmod +x .githooks/{hook}")),
            );
        }
    }
    outcome.checked += 1;
    if git_config(context, &["--bool", "--get", "diff.sops.cachetextconv"])
        .is_some_and(|value| value.eq_ignore_ascii_case("true"))
    {
        health_issue(
            events,
            warnings,
            context.root.join(".git/config"),
            "diff.sops.cachetextconv would cache plaintext in .git".to_string(),
            Some("git config diff.sops.cachetextconv false".to_string()),
        );
    }
    let canaries = context.state.join("canaries");
    outcome.checked += 1;
    if canaries.is_file() && mode_of(&canaries).is_some_and(|mode| mode & 0o077 != 0) {
        health_issue(
            events,
            warnings,
            canaries.clone(),
            "secret canaries are readable beyond this user".to_string(),
            Some(format!("chmod 600 {}", canaries.display())),
        );
    }
    for stray in [
        context.home.join(".config/sops/age/keys.txt"),
        context
            .home
            .join("Library/Application Support/sops/age/keys.txt"),
    ] {
        outcome.checked += 1;
        if stray.is_file() {
            health_issue(
                events,
                warnings,
                stray.clone(),
                format!(
                    "stray age identity outside dotfile state: {}",
                    stray.display()
                ),
                Some("remove it after confirming the managed identity works".to_string()),
            );
        }
    }
}

fn health_issue(
    events: &dyn EventSink,
    warnings: &mut Vec<(String, Option<String>)>,
    path: PathBuf,
    message: String,
    hint: Option<String>,
) {
    events.emit(Event::Item {
        action: Action::Check,
        path,
        detail: message.clone(),
        changed: false,
    });
    warnings.push((message, hint));
}

fn load_recipients(path: &Path) -> std::collections::BTreeMap<String, String> {
    let Ok(text) = fs::read_to_string(path) else {
        return std::collections::BTreeMap::new();
    };
    text.lines()
        .filter_map(|raw| {
            let line = raw.split('#').next().unwrap_or_default().trim();
            let (label, key) = line.split_once('=')?;
            let label = label.trim();
            let key = key.trim();
            (!label.is_empty() && key.starts_with("age1"))
                .then(|| (label.to_string(), key.to_string()))
        })
        .collect()
}

fn collect_encrypted(directory: &Path, found: &mut BTreeSet<PathBuf>) {
    if matches!(
        directory.file_name().and_then(|name| name.to_str()),
        Some(".git" | "target" | ".venv")
    ) {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_encrypted(&path, found);
        } else {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if name.ends_with(".enc") || name.contains(".enc.") {
                found.insert(path);
            }
        }
    }
}

fn systemctl(arguments: &[&str]) -> bool {
    Command::new("systemctl")
        .arg("--user")
        .args(arguments)
        .output()
        .is_ok_and(|output| output.status.success())
}

fn command_exists(command: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|directory| directory.join(command).is_file())
}

fn git_config(context: &Context, arguments: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(&context.root)
        .arg("config")
        .args(arguments)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn normalize_path(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn file_exists(path: &Path) -> Result<bool, String> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.is_file()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("read {}: {error}", path.display())),
    }
}

fn target_exists(path: &Path) -> Result<bool, String> {
    match fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("read {}: {error}", path.display())),
    }
}

fn symlink_exists(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.file_type().is_symlink()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("read {}: {error}", path.display())),
    }
}

fn remove_file_if_present(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("remove {}: {error}", path.display())),
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(unix)]
fn mode_of(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
fn mode_of(_path: &Path) -> Option<u32> {
    None
}

#[cfg(all(test, unix))]
#[path = "../../tests/unit/sync/integrations_tests.rs"]
mod tests;
