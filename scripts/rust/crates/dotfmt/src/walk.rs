//! Finding the files a run is about.
//!
//! A named file is used as it is given, extension and all, because somebody
//! who names a file has already decided. A directory is walked for the three
//! extensions dotfmt owns, and the directories nothing wants formatted are
//! skipped whole — a `node_modules` or a `target` is not somebody's config.
//!
//! Symlinked directories are not descended into, which keeps a link loop from
//! turning a format run into a hang.

use std::fs;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::native;

/// Directories a formatter has no business inside, whatever they hold.
const SKIP: &[&str] = &[
    ".git",
    ".jj",
    ".hg",
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    ".venv",
    "venv",
    "__pycache__",
    ".mypy_cache",
    ".ruff_cache",
    ".pytest_cache",
    ".next",
    ".turbo",
    "vendor",
    ".terraform",
    ".gradle",
    ".idea",
    ".direnv",
    ".cache",
];

/// Every file under `target` that dotfmt owns, in a settled order.
///
/// A missing target is a failure rather than an empty list. `blocks.read`
/// answers `[]` for a file it cannot open, which is the right default for a
/// reader and the wrong one here: a typo in a path would report a clean run
/// over nothing at all.
pub fn gather(target: &Path) -> Result<Vec<PathBuf>, String> {
    let found = fs::metadata(target).map_err(|error| format!("{}: {error}", target.display()))?;
    if !found.is_dir() {
        if native::kind(target).is_none() {
            return Err(format!(
                "not a .conf, .config or .dotfile file: {}",
                target.display()
            ));
        }
        return Ok(vec![target.to_path_buf()]);
    }
    let mut files = read(target.to_path_buf());
    files.sort();
    Ok(files)
}

/// Each directory's subdirectories are read in parallel: the walk spends its
/// time waiting on directory reads, and a thread per core hides most of that
/// on a large tree.
fn read(path: PathBuf) -> Vec<PathBuf> {
    let Ok(listing) = fs::read_dir(&path) else {
        return Vec::new();
    };
    let mut below = Vec::new();
    let mut files = Vec::new();
    for entry in listing.flatten() {
        // `file_type` reads the directory entry rather than what it points at,
        // so a symlink is never mistaken for the directory on the other end.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name();
        if kind.is_dir() {
            if !SKIP.contains(&name.display().to_string().as_str()) {
                below.push(entry.path());
            }
            continue;
        }
        if native::kind(&entry.path()).is_some() {
            files.push(entry.path());
        }
    }
    let deeper: Vec<Vec<PathBuf>> = below.into_par_iter().map(read).collect();
    files.extend(deeper.into_iter().flatten());
    files
}
