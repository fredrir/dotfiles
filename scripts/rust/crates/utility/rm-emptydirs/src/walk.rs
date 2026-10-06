use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use workstation::path;
use workstation::walk::SKIP;

#[derive(Default)]
pub struct Gathered {
    /// Every directory that would be empty once the empty ones inside it are
    /// gone, deepest first. The target roots are never included.
    pub empty: Vec<PathBuf>,
    pub unreadable: usize,
}

pub fn gather(targets: &[PathBuf], all: bool) -> Result<Gathered, String> {
    let resolved: Vec<PathBuf> = targets
        .iter()
        .map(|target| {
            require_directory(target)?;
            fs::canonicalize(target).map_err(|error| format!("{}: {error}", target.display()))
        })
        .collect::<Result<_, String>>()?;
    let mut gathered = Gathered::default();
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    for root in &resolved {
        let mut found = Vec::new();
        descend(root, all, &mut gathered.unreadable, &mut found);
        for candidate in found {
            if resolved.contains(&candidate) || !seen.insert(candidate.clone()) {
                continue;
            }
            gathered.empty.push(candidate);
        }
    }
    Ok(gathered)
}

fn require_directory(target: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(target).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => format!("no such file or directory: {}", target.display()),
        _ => format!("{}: {error}", target.display()),
    })?;
    if metadata.is_symlink() {
        return Err(format!("{}: a symbolic link", target.display()));
    }
    path::require_directory(target)
}

/// Collects every directory below `directory` that is empty of survivors,
/// deepest first. `true` means nothing survives inside it.
fn descend(directory: &Path, all: bool, unreadable: &mut usize, empty: &mut Vec<PathBuf>) -> bool {
    let listing = match fs::read_dir(directory) {
        Ok(listing) => listing,
        Err(_) => {
            // What cannot be read cannot be known to be empty.
            *unreadable += 1;
            return true;
        }
    };
    let mut survivor = false;
    for entry in listing {
        let Ok(entry) = entry else {
            *unreadable += 1;
            survivor = true;
            continue;
        };
        let Ok(kind) = entry.file_type() else {
            survivor = true;
            continue;
        };
        if kind.is_dir() {
            let name = entry.file_name();
            if !all && (path::hidden(&name) || skipped(&name)) {
                survivor = true;
                continue;
            }
            if !descend(&entry.path(), all, unreadable, empty) {
                survivor = true;
            }
        } else {
            // A file, a symbolic link or anything else keeps its directory.
            survivor = true;
        }
    }
    if !survivor {
        empty.push(directory.to_path_buf());
    }
    !survivor
}

fn skipped(name: &OsStr) -> bool {
    SKIP.iter().any(|skip| name == OsStr::new(skip))
}
