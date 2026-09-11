use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PerCore {
    pub session: String,
    pub core: u32,
    pub started: String,
    pub offset: Option<i32>,
    pub boot_id: String,
    pub pid: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recovered {
    Rebooted,
    Interrupted,
}

pub fn path() -> Result<PathBuf, String> {
    Ok(paths::state_dir()?.join("percore.json"))
}

pub fn boot_id() -> String {
    if let Ok(id) = std::env::var("HWTUNE_BOOT_ID") {
        return id;
    }
    fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|id| id.trim().to_string())
        .unwrap_or_default()
}

pub fn write(path: &Path, state: &PerCore) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn take(path: &Path) -> Result<Option<PerCore>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let state: PerCore =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    fs::remove_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Some(state))
}

pub fn clear(path: &Path) {
    let _ = fs::remove_file(path);
}

pub fn verdict(previous: &PerCore, boot_id: &str) -> Recovered {
    if previous.boot_id == boot_id {
        Recovered::Interrupted
    } else {
        Recovered::Rebooted
    }
}

#[cfg(test)]
#[path = "../../tests/unit/stress/state_tests.rs"]
mod tests;
