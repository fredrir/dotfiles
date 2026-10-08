use std::fs;
use std::path::{Path, PathBuf};

use dotfmt_core::config::Resolver;
use dotfmt_core::diagnostic::{Diagnostic, DiagnosticKind};
use workstation::walk::Policy;
pub use workstation::walk::Symlinks;

#[derive(Debug)]
pub struct Gathered {
    pub files: Vec<PathBuf>,
    pub unreadable: usize,
}

pub fn gather(
    target: &Path,
    symlinks: Symlinks,
    accepts: impl Fn(&Path) -> bool + Sync,
) -> Result<Gathered, Diagnostic> {
    let found = fs::metadata(target).map_err(|error| {
        Diagnostic::new(DiagnosticKind::Io, error.to_string()).with_path(target)
    })?;
    if !found.is_dir() {
        if !found.is_file() {
            return Err(
                Diagnostic::new(DiagnosticKind::Io, "not a regular file or directory")
                    .with_path(target),
            );
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

pub(super) struct Candidate {
    pub path: PathBuf,
    pub absolute: PathBuf,
    pub explicit: bool,
}

#[derive(Default)]
pub(super) struct Discovered {
    pub candidates: Vec<Candidate>,
    pub directories: Vec<PathBuf>,
    pub diagnostics: Vec<Diagnostic>,
    pub unreadable: usize,
}

pub(super) fn discover(targets: &[PathBuf], resolver: &Resolver) -> Discovered {
    let mut result = Discovered::default();
    let mut normalized = Vec::new();
    for target in targets {
        match resolver.absolute_path(target) {
            Ok(absolute) => normalized.push((target, absolute)),
            Err(error) => result.diagnostics.push(error),
        }
    }
    normalized.sort_by_key(|(_, absolute)| absolute.components().count());
    let mut roots = Vec::new();
    for (target, absolute) in normalized {
        let is_dir = absolute.is_dir();
        if is_dir
            && roots
                .iter()
                .any(|root: &PathBuf| directory_covered(root, &absolute))
        {
            continue;
        }
        if is_dir {
            result.directories.push(target.clone());
        }
        match gather(&absolute, Symlinks::Report, |_| true) {
            Ok(found) => {
                if is_dir && found.unreadable == 0 {
                    roots.push(absolute.clone());
                }
                result.unreadable += found.unreadable;
                for path in found.files {
                    let shown = if is_dir {
                        target.join(path.strip_prefix(&absolute).unwrap_or(&path))
                    } else {
                        target.clone()
                    };
                    result.candidates.push(Candidate {
                        path: shown,
                        absolute: path,
                        explicit: !is_dir,
                    });
                }
            }
            Err(mut error) => {
                error.path = Some(target.clone());
                result.diagnostics.push(error);
            }
        }
    }
    result.candidates.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| right.explicit.cmp(&left.explicit))
    });
    result
}

fn directory_covered(root: &Path, target: &Path) -> bool {
    if !target.starts_with(root) {
        return false;
    }
    target
        .ancestors()
        .take_while(|path| *path != root)
        .all(|path| {
            !path
                .file_name()
                .is_some_and(|name| workstation::walk::SKIP.iter().any(|skip| name == *skip))
                && fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir())
        })
}

#[cfg(test)]
#[path = "../../tests/app/discovery.rs"]
mod tests;
