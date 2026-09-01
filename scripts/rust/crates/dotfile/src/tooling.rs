use std::ffi::OsString;
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Instant, SystemTime};

use crate::cli::SyncCli;
use crate::context::Context;
use crate::event::{Event, EventSink, Phase, Summary};

#[derive(Clone)]
pub struct Refresh {
    root: PathBuf,
    executable: PathBuf,
}

pub fn pending(cli: &SyncCli) -> Result<Option<Refresh>, String> {
    if cli.dry_run || std::env::var_os("DOTFILE_REEXECED").is_some() {
        return Ok(None);
    }
    let context = Context::discover()?;
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    if !is_installed(&context.home, &executable) || !stale(&context.root, &executable)? {
        return Ok(None);
    }
    Ok(Some(Refresh {
        root: context.root,
        executable,
    }))
}

impl Refresh {
    pub fn run(&self, events: &dyn EventSink) -> Result<Summary, String> {
        let started = Instant::now();
        events.emit(Event::PhaseStarted {
            phase: Phase::Tooling,
            total: None,
        });
        events.emit(Event::Progress {
            phase: Phase::Tooling,
            completed: 0,
            total: None,
            label: "updating workstation commands".to_string(),
        });
        if !std::io::stderr().is_terminal() {
            eprintln!("dotfile: updating workstation commands…");
        }
        let output = Command::new(self.root.join("setup.sh"))
            .arg("--commands-only")
            .stdin(Stdio::null())
            .output()
            .map_err(|error| format!("cannot update workstation commands: {error}"))?;
        if !output.status.success() {
            let message = String::from_utf8_lossy(&output.stderr)
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .unwrap_or("command update failed")
                .to_string();
            events.emit(Event::Failed {
                phase: Phase::Tooling,
                message: message.clone(),
                hint: None,
            });
            return Err(message);
        }
        crate::cancel::check()?;
        events.emit(Event::Progress {
            phase: Phase::Tooling,
            completed: 1,
            total: Some(1),
            label: "workstation commands ready".to_string(),
        });
        Ok(Summary {
            profile: String::new(),
            peer: None,
            remote_changed: None,
            checked: 0,
            changed: 0,
            links: 0,
            merges: 0,
            secrets: 0,
            generated: 0,
            dry_run: false,
            elapsed: started.elapsed(),
        })
    }

    pub fn reexec(&self, arguments: &[OsString]) -> Result<(), String> {
        reexec(&self.executable, arguments)
    }
}

pub(crate) fn native_current() -> Result<bool, String> {
    let context = Context::discover()?;
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    Ok(!stale(&context.root, &executable)?)
}

fn is_installed(home: &Path, executable: &Path) -> bool {
    executable == home.join(".local/bin/dotfile")
}

fn stale(root: &Path, executable: &Path) -> Result<bool, String> {
    let installed = fs::metadata(executable)
        .and_then(|metadata| metadata.modified())
        .map_err(|error| format!("{}: {error}", executable.display()))?;
    let inputs = [
        root.join("setup.sh"),
        root.join("scripts/python/pyproject.toml"),
        root.join("scripts/python/uv.lock"),
        root.join("scripts/rust"),
        root.join("shared/tools"),
    ];
    for input in inputs {
        if newest(&input)? > installed {
            return Ok(true);
        }
    }
    Ok(false)
}

fn newest(path: &Path) -> Result<SystemTime, String> {
    if matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("target" | ".venv" | "__pycache__")
    ) {
        return Ok(SystemTime::UNIX_EPOCH);
    }
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SystemTime::UNIX_EPOCH);
        }
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let mut latest = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(|error| format!("{}: {error}", path.display()))? {
            let child = entry
                .map_err(|error| format!("{}: {error}", path.display()))?
                .path();
            latest = latest.max(newest(&child)?);
        }
    }
    Ok(latest)
}

fn reexec(executable: &Path, arguments: &[OsString]) -> Result<(), String> {
    let mut command = Command::new(executable);
    command.args(arguments).env("DOTFILE_REEXECED", "1");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(format!(
            "cannot restart updated dotfile: {}",
            command.exec()
        ))
    }
    #[cfg(not(unix))]
    {
        let status = command.status().map_err(|error| error.to_string())?;
        std::process::exit(status.code().unwrap_or(1));
    }
}
