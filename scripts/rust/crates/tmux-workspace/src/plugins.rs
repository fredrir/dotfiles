use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    Result,
    config::{self, Paths, identity},
    process::{self, Output},
    tmux::Context,
};

#[derive(Deserialize)]
pub struct Lock {
    pub resurrect: Resurrect,
    pub fingers: Fingers,
}
#[derive(Deserialize)]
pub struct Resurrect {
    pub repository: String,
    pub revision: String,
}
#[derive(Deserialize)]
pub struct Fingers {
    pub version: String,
    pub assets: std::collections::BTreeMap<String, Asset>,
}
#[derive(Deserialize)]
pub struct Asset {
    pub url: String,
    pub sha256: String,
}

impl Lock {
    pub fn read(paths: &Paths) -> Result<Self> {
        let lock: Self =
            serde_json::from_str(&fs::read_to_string(paths.config.join("plugins.lock.json"))?)?;
        if !regex::Regex::new(r"^[0-9a-f]{40}$")?.is_match(&lock.resurrect.revision) {
            return Err("invalid resurrect revision".into());
        }
        if !regex::Regex::new(r"^\d+\.\d+\.\d+$")?.is_match(&lock.fingers.version) {
            return Err("invalid fingers version".into());
        }
        let checksum = regex::Regex::new(r"^[0-9a-f]{64}$")?;
        for asset in lock.fingers.assets.values() {
            if !asset
                .url
                .starts_with("https://github.com/Morantron/tmux-fingers/releases/download/")
                || !checksum.is_match(&asset.sha256)
            {
                return Err("invalid fingers asset".into());
            }
        }
        Ok(lock)
    }
    pub fn resurrect(&self, paths: &Paths) -> PathBuf {
        paths
            .data
            .join(format!("resurrect-{}", self.resurrect.revision))
    }
    pub fn fingers(&self, paths: &Paths) -> PathBuf {
        paths
            .data
            .join(format!("fingers-{}/tmux-fingers", self.fingers.version))
    }
}

#[derive(Default, Deserialize, Serialize)]
struct InstallState {
    attempted: u64,
    errors: Vec<String>,
}

fn installed_resurrect(lock: &Lock, paths: &Paths) -> bool {
    ["scripts/save.sh", "scripts/restore.sh"]
        .iter()
        .all(|file| lock.resurrect(paths).join(file).is_file())
}

fn fingers_path(lock: &Lock, paths: &Paths) -> Result<Option<PathBuf>> {
    let bundled = lock.fingers(paths);
    let candidate =
        process::which(&bundled.to_string_lossy()).or_else(|| process::which("tmux-fingers"));
    if let Some(candidate) = candidate {
        let result = process::run(Command::new(&candidate).arg("version"))?;
        if result.out.trim() != lock.fingers.version {
            return Err(format!(
                "fingers version mismatch: {} reports {}; expected {}",
                candidate.display(),
                result.out.trim(),
                lock.fingers.version
            )
            .into());
        }
        return Ok(Some(candidate));
    }
    Ok(None)
}

pub fn asset_key() -> String {
    format!(
        "{}-{}",
        if cfg!(target_os = "macos") {
            "Darwin"
        } else {
            "Linux"
        },
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            "arm64"
        } else {
            std::env::consts::ARCH
        }
    )
}

fn install_resurrect(lock: &Lock, paths: &Paths) -> Result<()> {
    let destination = lock.resurrect(paths);
    if installed_resurrect(lock, paths) {
        return Ok(());
    }
    if destination.exists() {
        return Err(format!(
            "incomplete plugin: {}; move it aside and retry",
            destination.display()
        )
        .into());
    }
    let staging = tempfile::Builder::new()
        .prefix(".resurrect-")
        .tempdir_in(&paths.data)?;
    let checkout = staging.path().join("repo");
    process::run(Command::new("git").args(["init", "-q"]).arg(&checkout))?;
    process::capture(
        Command::new("git")
            .arg("-C")
            .arg(&checkout)
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "fetch",
                "--depth=1",
                "--",
                &lock.resurrect.repository,
                &lock.resurrect.revision,
            ])
            .env("GIT_TERMINAL_PROMPT", "0"),
        None,
        Some(Duration::from_secs(90)),
    )?
    .checked()?;
    process::run(Command::new("git").arg("-C").arg(&checkout).args([
        "-c",
        "core.hooksPath=/dev/null",
        "checkout",
        "--detach",
        "FETCH_HEAD",
    ]))?;
    let found = process::run(
        Command::new("git")
            .arg("-C")
            .arg(&checkout)
            .args(["rev-parse", "HEAD"]),
    )?;
    if found.out.trim() != lock.resurrect.revision {
        return Err("resurrect revision mismatch".into());
    }
    if !["scripts/save.sh", "scripts/restore.sh"]
        .iter()
        .all(|file| checkout.join(file).is_file())
    {
        return Err("incomplete resurrect checkout".into());
    }
    fs::rename(checkout, destination)?;
    Ok(())
}

fn install_fingers(lock: &Lock, paths: &Paths) -> Result<()> {
    if fingers_path(lock, paths).is_ok_and(|v| v.is_some()) {
        return Ok(());
    }
    let destination = lock.fingers(paths);
    let parent = destination.parent().ok_or("invalid fingers path")?;
    if parent.exists() {
        return Err(format!(
            "incomplete or incompatible plugin: {}; move it aside and retry",
            parent.display()
        )
        .into());
    }
    let asset = lock.fingers.assets.get(&asset_key()).ok_or_else(|| {
        format!(
            "fingers: no standalone build for {}; install tmux-fingers {}",
            asset_key(),
            lock.fingers.version
        )
    })?;
    let staging = tempfile::Builder::new()
        .prefix(".fingers-")
        .tempdir_in(&paths.data)?;
    let binary = staging.path().join("tmux-fingers");
    process::capture(
        Command::new("curl")
            .args([
                "--fail",
                "--silent",
                "--show-error",
                "--location",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--connect-timeout",
                "15",
                "--max-time",
                "60",
                "--max-filesize",
                "33554432",
                "--output",
            ])
            .arg(&binary)
            .arg(&asset.url),
        None,
        Some(Duration::from_secs(65)),
    )?
    .checked()?;
    let data = fs::read(&binary)?;
    if data.len() > 32 * 1024 * 1024 || format!("{:x}", Sha256::digest(&data)) != asset.sha256 {
        return Err("fingers checksum mismatch".into());
    }
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))?;
    let found = process::run(Command::new(&binary).arg("version"))?;
    if found.out.trim() != lock.fingers.version {
        return Err(format!("fingers version mismatch: {}", found.out.trim()).into());
    }
    let ready = staging.path().join("ready");
    config::private_dir(&ready)?;
    fs::rename(binary, ready.join("tmux-fingers"))?;
    fs::rename(ready, parent)?;
    Ok(())
}

pub fn install(paths: &Paths) -> Result<()> {
    if std::env::var("DOTFILES_TMUX_VALIDATE").as_deref() == Ok("1") {
        return Ok(());
    }
    let lock = Lock::read(paths)?;
    config::private_dir(&paths.data)?;
    let _guard = config::lock(&paths.data.join(".install.lock"), true)?;
    let mut errors = Vec::new();
    if std::env::var("DOTFILES_TMUX_OFFLINE").as_deref() == Ok("1") {
        if !installed_resurrect(&lock, paths) {
            errors.push("resurrect unavailable offline".into());
        }
        match fingers_path(&lock, paths) {
            Ok(Some(_)) => {}
            Ok(None) => errors.push("fingers unavailable offline".into()),
            Err(e) => errors.push(e.to_string()),
        }
    } else {
        for installer in [install_resurrect, install_fingers] {
            if let Err(error) = installer(&lock, paths) {
                errors.push(error.to_string());
            }
        }
    }
    config::atomic_json(
        &paths.data.join("installation.json"),
        &InstallState {
            attempted: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
            errors: errors.clone(),
        },
    )?;
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n").into())
    }
}

pub fn status(ctx: &Context) -> Result<serde_json::Value> {
    let lock = Lock::read(&ctx.paths)?;
    let installed: InstallState = fs::read_to_string(ctx.paths.data.join("installation.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let (path, error) = match fingers_path(&lock, &ctx.paths) {
        Ok(value) => (value, None),
        Err(error) => (None, Some(error.to_string())),
    };
    Ok(serde_json::json!({
        "resurrect": {"revision": lock.resurrect.revision, "installed": installed_resurrect(&lock, &ctx.paths), "path": lock.resurrect(&ctx.paths)},
        "fingers": {"version": lock.fingers.version, "installed": path.is_some(), "path": path, "error": error},
        "installation": installed,
        "server": {"socket": ctx.tmux.socket, "state": ctx.tmux.option("@workspace-plugins-state"), "error": ctx.tmux.option("@workspace-plugins-error")}
    }))
}

pub fn load(ctx: &Context) -> Result<()> {
    if std::env::var("DOTFILES_TMUX_VALIDATE").as_deref() == Ok("1") {
        return Ok(());
    }
    let result = load_inner(ctx);
    ctx.tmux.set(
        "@workspace-plugins-state",
        if result.is_ok() { "" } else { "error" },
    )?;
    ctx.tmux.set(
        "@workspace-plugins-error",
        &result
            .as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_default(),
    )?;
    result
}

fn load_inner(ctx: &Context) -> Result<()> {
    let lock = Lock::read(&ctx.paths)?;
    let mut errors = Vec::new();
    if installed_resurrect(&lock, &ctx.paths) {
        for action in ["save", "restore"] {
            ctx.tmux.set(
                &format!("@resurrect-{action}-script-path"),
                &lock
                    .resurrect(&ctx.paths)
                    .join(format!("scripts/{action}.sh"))
                    .display()
                    .to_string(),
            )?;
        }
        recovery_dir(ctx)?;
    } else {
        errors.push("resurrect unavailable; run tmux-workspace plugins install".into());
    }
    match fingers_path(&lock, &ctx.paths) {
        Ok(Some(binary)) => {
            ctx.tmux.set("@fingers-enable-bindings", "0")?;
            if ctx.tmux.option("@fingers-hint-style").is_empty() {
                errors.push("fingers: generated theme unavailable; regenerate theme.conf".into());
            } else if let Err(error) = external(
                ctx,
                Command::new(binary).arg("load-config"),
                Some(Duration::from_secs(30)),
            )
            .and_then(Output::checked)
            {
                errors.push(error.to_string());
            }
        }
        Ok(None) => errors.push("fingers unavailable; run tmux-workspace plugins install".into()),
        Err(error) => errors.push(error.to_string()),
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; ").into())
    }
}

pub fn recovery_dir(ctx: &Context) -> Result<PathBuf> {
    let configured = ctx.tmux.option("@resurrect-dir");
    let path = if configured.is_empty() {
        ctx.paths
            .state
            .join("resurrect")
            .join(identity(&ctx.tmux.socket()?))
    } else {
        config::expand(&configured)
    };
    config::private_dir(&path)?;
    if configured.is_empty() {
        ctx.tmux
            .set("@resurrect-dir", &path.display().to_string())?;
    }
    Ok(path)
}

pub fn external(ctx: &Context, command: &mut Command, timeout: Option<Duration>) -> Result<Output> {
    // Upstream scripts invoke bare tmux; bind that name to the selected test/runtime binary.
    let directory = tempfile::Builder::new()
        .prefix("tmux-plugin-path-")
        .tempdir()?;
    let binary = process::which(&ctx.tmux.binary.to_string_lossy())
        .ok_or("tmux: not installed")?
        .canonicalize()?;
    std::os::unix::fs::symlink(binary, directory.path().join("tmux"))?;
    if process::which("hostname").is_none() {
        std::os::unix::fs::symlink(
            ctx.paths.config.join("libexec/hostname"),
            directory.path().join("hostname"),
        )?;
    }
    let mut paths = vec![directory.path().to_path_buf()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    command.env("PATH", std::env::join_paths(paths)?);
    ctx.tmux.subprocess_env(command)?;
    process::capture(command, None, timeout)
}

pub fn fingers(ctx: &Context) -> Result<i32> {
    let pane = ctx.pane()?;
    if ctx.tmux.format("#{pane_floating_flag}", Some(pane), None)? == "1"
        || ctx.tmux.clients()?.len() > 1
    {
        return Ok(3);
    }
    let lock = Lock::read(&ctx.paths)?;
    let Some(binary) = fingers_path(&lock, &ctx.paths)? else {
        return Ok(3);
    };
    let mut clipboard = vec![
        ctx.tmux.binary.display().to_string(),
        "-S".into(),
        ctx.tmux.socket()?,
        "load-buffer".into(),
        "-w".into(),
    ];
    if let Some(client) = &ctx.client {
        clipboard.extend(["-t".into(), client.clone()]);
    }
    clipboard.push("-".into());
    let action = process::shell(&clipboard);
    let result = external(
        ctx,
        Command::new(binary).args([
            "start",
            pane,
            "--main-action",
            &action,
            "--ctrl-action",
            &action,
        ]),
        None,
    )?;
    Ok(result.code)
}
