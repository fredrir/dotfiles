//! Keeps each completion dump that `compinit -C` trusts in step with the
//! completion functions on disk; a stale dump is removed, so the next shell
//! rebuilds it with a full `compinit`.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::git;

/// Written next to a dump by the bundle: the `fpath` it was built for.
const STAMP: &str = ".fpath";

/// Removes stale dumps under `dir`, and dumps whose `fpath` holds a
/// directory in `rebound`, whose files bind other commands now; returns the removed dumps.
pub fn refresh(
    dir: &Path,
    omz: Option<&Path>,
    rebound: &[&Path],
    dry_run: bool,
) -> Result<Vec<PathBuf>, String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Ok(Vec::new()),
    };
    let mut stamps: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".zcompdump") && name.ends_with(STAMP))
        })
        .collect();
    stamps.sort();
    let mut removed = Vec::new();
    for stamp in stamps {
        let dump = PathBuf::from(stamp.to_string_lossy().trim_end_matches(STAMP).to_string());
        let exists = dump.is_file();
        if exists && !is_stale(&dump, &stamp, omz, rebound) {
            continue;
        }
        if !dry_run {
            for path in [dump.clone(), wordcode(&dump), stamp.clone()] {
                remove(&path)?;
            }
        }
        if exists {
            removed.push(dump);
        }
    }
    Ok(removed)
}

fn wordcode(dump: &Path) -> PathBuf {
    PathBuf::from(format!("{}.zwc", dump.display()))
}

fn remove(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("remove {}: {error}", path.display()))
        }
        _ => Ok(()),
    }
}

/// compinit's own test (the number of completion files), plus renamed or
/// removed functions, rebound directories and a changed oh-my-zsh revision.
fn is_stale(dump: &Path, stamp: &Path, omz: Option<&Path>, rebound: &[&Path]) -> bool {
    let (Ok(text), Ok(fpath)) = (fs::read(dump), fs::read_to_string(stamp)) else {
        return true;
    };
    if fpath.lines().any(|dir| rebound.contains(&Path::new(dir))) {
        return true;
    }
    let text = String::from_utf8_lossy(&text);
    let Some(count) = text
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("#files: "))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|count| count.parse::<usize>().ok())
    else {
        return true;
    };
    let on_disk = completion_files(fpath.lines().map(Path::new));
    if on_disk.len() != count {
        return true;
    }
    let names: BTreeSet<&str> = on_disk.iter().map(String::as_str).collect();
    if !dumped_functions(&text)
        .iter()
        .all(|name| names.contains(name))
    {
        return true;
    }
    let recorded = text
        .lines()
        .find_map(|line| line.strip_prefix("#omz revision: "));
    let current = omz.and_then(|omz| git::revision(&omz.join(".git")));
    match (recorded, current) {
        (Some(recorded), Some(current)) => recorded.trim() != current,
        _ => false,
    }
}

/// Names compinit counts: `$^fpath/^([^_]*|*~|*.zwc)(N)`, one per file.
fn completion_files<'a>(fpath: impl Iterator<Item = &'a Path>) -> Vec<String> {
    let mut files = Vec::new();
    for dir in fpath.filter(|dir| !dir.as_os_str().is_empty()) {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        files.extend(
            entries
                .filter_map(Result::ok)
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| {
                    name.starts_with('_') && !name.ends_with('~') && !name.ends_with(".zwc")
                }),
        );
    }
    files
}

/// Functions in the dump's first `autoload -Uz` statement.
fn dumped_functions(text: &str) -> Vec<&str> {
    let Some(start) = text.find("\nautoload -Uz ") else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for line in text[start + 1..].lines() {
        let continued = line.trim_end().ends_with('\\');
        names.extend(
            line.split_whitespace()
                .filter(|word| word.starts_with('_') && *word != "\\"),
        );
        if !continued {
            break;
        }
    }
    names
}

#[cfg(test)]
#[path = "../tests/unit/compdump_tests.rs"]
mod tests;
