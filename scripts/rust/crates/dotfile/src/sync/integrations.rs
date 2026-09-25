use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::context::{Context, write_atomic};
use crate::event::{Action, Event, EventSink, Phase};

use crate::config::Configuration;

#[derive(Default)]
pub struct IntegrationOutcome {
    pub checked: usize,
    pub changed: usize,
    pub generated: usize,
}

type GitConfig = std::collections::HashMap<String, String>;

/// Starts reading the repository's Git settings, so `git` start-up overlaps the rest of sync.
pub fn prefetch(context: &Context) -> std::thread::JoinHandle<GitConfig> {
    let root = context.root.clone();
    std::thread::spawn(move || git_config_map(&root))
}

pub fn synchronize(
    context: &Context,
    configuration: &Configuration,
    dry_run: bool,
    events: &dyn EventSink,
    git_config: std::thread::JoinHandle<GitConfig>,
) -> Result<IntegrationOutcome, String> {
    events.emit(Event::PhaseStarted {
        phase: Phase::Integrations,
        total: None,
    });
    let mut outcome = IntegrationOutcome::default();
    let mut warnings = Vec::new();
    if configuration
        .groups
        .iter()
        .any(|group| group == "linux/hyprland")
    {
        hyprland(context, dry_run, events, &mut outcome, &mut warnings)?;
    }
    if command_exists("systemctl") {
        user_units(context, dry_run, events, &mut outcome, &mut warnings)?;
    }
    let mut git_configs = git_config.join().unwrap_or_default();
    git_settings(
        context,
        &mut git_configs,
        dry_run,
        &mut outcome,
        &mut warnings,
    );
    secret_health(context, &git_configs, events, &mut outcome, &mut warnings);
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
        let destination = context.external_config.join("elephant/files.toml");
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
    let wallpaper = context.external_config.join("hypr/wallpaper.png");
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

/// Reloads systemd when a linked user unit changed on disk, then restarts the running ones.
fn user_units(
    context: &Context,
    dry_run: bool,
    events: &dyn EventSink,
    outcome: &mut IntegrationOutcome,
    warnings: &mut Vec<(String, Option<String>)>,
) -> Result<(), String> {
    crate::cancel::check()?;
    let directory = context.external_config.join("systemd/user");
    let linked = linked_units(context, &directory);
    if linked.is_empty() {
        return Ok(());
    }
    outcome.checked += linked.len();
    let mut show = context.command("systemctl");
    show.args([
        "--user",
        "show",
        "--property=Id,NeedDaemonReload,ActiveState",
        "--",
    ])
    .args(&linked);
    let stale = match systemctl(&mut show, Duration::from_secs(5)) {
        Ok(stdout) => stale_units(&stdout),
        Err(message) => {
            warnings.push((message, None));
            return Ok(());
        }
    };
    if stale.is_empty() {
        return Ok(());
    }
    let running: Vec<&str> = stale
        .iter()
        .filter(|(_, running)| *running)
        .map(|(unit, _)| unit.as_str())
        .collect();
    let mut restarted = true;
    if !dry_run {
        crate::cancel::check()?;
        let mut reload = context.command("systemctl");
        reload.args(["--user", "daemon-reload"]);
        if let Err(message) = systemctl(&mut reload, Duration::from_secs(30)) {
            warnings.push((message, Some("systemctl --user daemon-reload".to_string())));
            return Ok(());
        }
        if !running.is_empty() {
            let mut restart = context.command("systemctl");
            restart.args(["--user", "try-restart", "--"]).args(&running);
            if let Err(message) = systemctl(&mut restart, Duration::from_secs(60)) {
                restarted = false;
                warnings.push((
                    message,
                    Some(format!("systemctl --user restart {}", running.join(" "))),
                ));
            }
        }
    }
    for (unit, running) in &stale {
        outcome.changed += 1;
        events.emit(Event::Item {
            action: Action::Sync,
            path: directory.join(unit),
            detail: match (running, restarted) {
                (true, true) => "restarted",
                (true, false) => "reloaded; restart failed",
                (false, _) => "reloaded",
            }
            .to_string(),
            changed: true,
        });
    }
    Ok(())
}

fn linked_units(context: &Context, directory: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut units: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let target = fs::read_link(entry.path()).ok()?;
            let name = entry.file_name().into_string().ok()?;
            (directory.join(target).starts_with(&context.root)
                && crate::system::units::valid(&name))
            .then_some(name)
        })
        .collect();
    units.sort();
    units
}

/// `systemctl show` prints one blank-line separated block per unit, in any property order.
fn stale_units(stdout: &str) -> Vec<(String, bool)> {
    stdout
        .split("\n\n")
        .filter_map(|block| {
            let mut id = None;
            let mut stale = false;
            let mut running = false;
            for line in block.lines() {
                match line.split_once('=') {
                    Some(("Id", value)) => id = Some(value.to_string()),
                    Some(("NeedDaemonReload", value)) => stale = value == "yes",
                    Some(("ActiveState", value)) => {
                        running = matches!(value, "active" | "activating" | "reloading");
                    }
                    _ => {}
                }
            }
            match (id, stale) {
                (Some(id), true) => Some((id, running)),
                _ => None,
            }
        })
        .collect()
}

fn systemctl(command: &mut Command, timeout: Duration) -> Result<String, String> {
    let label = format!(
        "systemctl {}",
        command
            .get_args()
            .map(|argument| argument.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    );
    let output =
        crate::process::output(command, hostkit::process::CaptureLimits::default(), timeout)
            .map_err(|error| format!("{label}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{label}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Repository-local Git settings this repository depends on: hooks that run the
/// secret scan, and a SOPS diff filter that never caches plaintext in `.git`.
fn git_settings(
    context: &Context,
    configs: &mut std::collections::HashMap<String, String>,
    dry_run: bool,
    outcome: &mut IntegrationOutcome,
    warnings: &mut Vec<(String, Option<String>)>,
) {
    if !context.root.join(".git").exists() {
        return;
    }
    let identity = crate::secret::vault::identity_path(context);
    let wanted = [
        (
            "core.hooksPath",
            context.root.join(".githooks").display().to_string(),
        ),
        (
            "diff.sops.textconv",
            format!("SOPS_AGE_KEY_FILE={} sops -d", identity.display()),
        ),
        ("diff.sops.cachetextconv", "false".to_string()),
    ];
    for (key, value) in wanted {
        outcome.checked += 1;
        let lower = key.to_ascii_lowercase();
        if configs.get(&lower) == Some(&value) {
            continue;
        }
        if dry_run {
            outcome.generated += 1;
            continue;
        }
        let set = Command::new("git")
            .arg("-C")
            .arg(&context.root)
            .args(["config", key])
            .arg(&value)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        match set {
            Ok(status) if status.success() => {
                outcome.generated += 1;
                configs.insert(lower, value);
            }
            _ => warnings.push((
                format!("could not set git {key}"),
                Some(format!("git config {key} {value}")),
            )),
        }
    }
}

fn secret_health(
    context: &Context,
    configs: &std::collections::HashMap<String, String>,
    events: &dyn EventSink,
    outcome: &mut IntegrationOutcome,
    warnings: &mut Vec<(String, Option<String>)>,
) {
    let sops = command_exists("sops");
    let mut encrypted = BTreeSet::new();
    collect_encrypted(
        &context.root,
        &mut encrypted,
        if sops { 1 } else { usize::MAX },
    );
    if encrypted.is_empty() {
        return;
    }
    outcome.checked += 1;
    if !sops {
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
    let identity = context.root_config.join("age/keys.txt");
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
    let recipients = load_recipients(&context.root_config.join("keys.dotfile"));
    outcome.checked += 1;
    if recipients.is_empty() {
        health_issue(
            events,
            warnings,
            context.root_config.join("keys.dotfile"),
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
            context.root_config.join("keys.dotfile"),
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
    let configured_hooks = configs.get("core.hookspath").map(|path| {
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
    if configs
        .get("diff.sops.cachetextconv")
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
    let canaries = context.root_config.join("canaries");
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
        context.home.join("dotfiles/config/sops/age/keys.txt"),
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

/// Files at each level come before its subdirectories, so a limit of one stops at the shallowest.
fn collect_encrypted(directory: &Path, found: &mut BTreeSet<PathBuf>, limit: usize) {
    if matches!(
        directory.file_name().and_then(|name| name.to_str()),
        Some(".git" | "target" | ".venv" | "scripts" | "docs" | ".githooks" | "node_modules")
    ) {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut directories = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            directories.push(path);
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
    for directory in directories {
        if found.len() >= limit {
            return;
        }
        collect_encrypted(&directory, found, limit);
    }
}

fn command_exists(command: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|directory| directory.join(command).is_file())
}

fn git_config_map(root: &Path) -> GitConfig {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["config", "--get-regexp", r"^(core\.hookspath|diff\.sops\.)"])
        .output()
        .ok();
    let mut map = std::collections::HashMap::new();
    if let Some(output) = output
        && output.status.success()
    {
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if let Some((key, val)) = line.split_once(' ') {
                map.insert(key.to_ascii_lowercase(), val.trim().to_string());
            }
        }
    }
    map
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
