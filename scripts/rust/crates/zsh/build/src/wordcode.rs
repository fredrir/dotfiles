//! Wordcode compiled at build time by the zsh that runs the shell; zsh
//! ignores a `.zwc` made by another version, or one older than its source.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use hostkit::process::{CaptureLimits, output};

use crate::cache::{self, Cache, Key};
use crate::state::State;

const TIMEOUT: Duration = Duration::from_secs(60);

/// `$SHELL` when it is a zsh, else the first `zsh` on the search path.
pub fn shell(search: &State) -> Option<PathBuf> {
    std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|shell| {
            shell
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("zsh"))
        })
        .and_then(|shell| search.command(&shell.to_string_lossy()).flatten())
        .or_else(|| search.command("zsh").flatten())
}

/// The `ZSH_VERSION` of `zsh`, as its wordcode records it.
pub fn shell_version(zsh: &Path, cache: &mut Cache) -> Result<String, String> {
    let argv = [
        zsh.to_string_lossy().into_owned(),
        "-fc".to_string(),
        "print -r -- $ZSH_VERSION".to_string(),
    ];
    let output = cache.get_or_run(Key::new(&argv, zsh, None), || {
        cache::run(&argv, zsh, None, TIMEOUT)
    })?;
    match output.trim() {
        "" => Err(format!("{}: version unknown", zsh.display())),
        version => Ok(version.to_string()),
    }
}

pub fn path(source: &Path) -> PathBuf {
    PathBuf::from(format!("{}.zwc", source.display()))
}

/// The `ZSH_VERSION` a `.zwc` header records.
pub fn version(wordcode: &Path) -> Option<String> {
    let mut header = Vec::new();
    fs::File::open(wordcode)
        .ok()?
        .take(64)
        .read_to_end(&mut header)
        .ok()?;
    if !(header.starts_with(&[7, 6, 5, 4]) || header.starts_with(&[4, 5, 6, 7])) {
        return None;
    }
    let version = header.get(8..)?;
    let end = version.iter().position(|byte| *byte == 0)?;
    String::from_utf8(version[..end].to_vec()).ok()
}

/// Whether zsh `version` would load `wordcode` in place of `source`.
pub fn current(source: &Path, wordcode: &Path, version: &str) -> bool {
    let newer = match (modified(wordcode), modified(source)) {
        (Some(wordcode), Some(source)) => wordcode > source,
        _ => false,
    };
    newer && self::version(wordcode).as_deref() == Some(version)
}

/// `zcompile -U` into a staging file, renamed over the source's `.zwc` once
/// it is strictly newer than the source.
pub fn compile(zsh: &Path, source: &Path) -> Result<(), String> {
    let wordcode = path(source);
    let staging = PathBuf::from(format!("{}.{}.zwc", source.display(), std::process::id()));
    let result = zcompile(zsh, &staging, source).and_then(|()| {
        let source_time =
            modified(source).ok_or_else(|| format!("{}: no mtime", source.display()))?;
        if modified(&staging).is_none_or(|time| time <= source_time) {
            fs::File::open(&staging)
                .and_then(|file| file.set_modified(source_time + Duration::from_secs(1)))
                .map_err(|error| format!("touch {}: {error}", staging.display()))?;
        }
        fs::rename(&staging, &wordcode)
            .map_err(|error| format!("write {}: {error}", wordcode.display()))
    });
    if result.is_err() {
        let _ = fs::remove_file(&staging);
    }
    result
}

fn zcompile(zsh: &Path, staging: &Path, source: &Path) -> Result<(), String> {
    let mut command = Command::new(zsh);
    command
        .args(["-fc", "zcompile -U -- \"$1\" \"$2\"", "zsh"])
        .arg(staging)
        .arg(source)
        .stdin(Stdio::null());
    let captured = output(&mut command, CaptureLimits::default(), TIMEOUT)
        .map_err(|error| format!("zcompile {}: {error}", source.display()))?;
    if captured.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&captured.stderr);
    let reason = stderr.lines().next().unwrap_or_default();
    Err(format!(
        "zcompile {}: {}",
        source.display(),
        if reason.is_empty() {
            captured.status.to_string()
        } else {
            reason.to_string()
        }
    ))
}

fn modified(path: &Path) -> Option<std::time::SystemTime> {
    fs::metadata(path).and_then(|meta| meta.modified()).ok()
}

#[cfg(test)]
#[path = "../tests/unit/wordcode_tests.rs"]
mod tests;
