use std::fs;
use std::path::{Path, PathBuf};

use workstation::walk::Policy;
pub use workstation::walk::Symlinks;

#[derive(Clone, Copy)]
pub enum Explicit {
    Any,
    Regular,
}

#[derive(Debug)]
pub struct Gathered {
    pub files: Vec<PathBuf>,
    pub unreadable: usize,
}

pub fn gather(
    target: &Path,
    symlinks: Symlinks,
    explicit: Explicit,
    accepts: impl Fn(&Path) -> bool + Sync,
) -> Result<Gathered, String> {
    let found = fs::metadata(target).map_err(|error| format!("{}: {error}", target.display()))?;
    if !found.is_dir() {
        if matches!(explicit, Explicit::Regular) && !found.is_file() {
            return Err(format!(
                "{}: not a regular file or directory",
                target.display()
            ));
        }
        // Explicit targets bypass the language's filename filter.
        return Ok(Gathered {
            files: vec![target.to_path_buf()],
            unreadable: 0,
        });
    }

    let walked = workstation::walk::walk(
        target,
        &Policy::new().symlinks(symlinks),
        |_directory, entries| {
            entries
                .iter()
                .filter(|entry| entry.is_file() || entry.is_symlink())
                .filter(|entry| accepts(&entry.path))
                .map(|entry| entry.path.clone())
                .collect()
        },
    );
    let mut files = walked.items;
    files.sort();
    Ok(Gathered {
        files,
        unreadable: walked.unreadable,
    })
}
