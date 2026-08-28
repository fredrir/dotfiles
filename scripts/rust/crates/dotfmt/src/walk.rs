//! Finding the files a run is about.
//!
//! Both a named file and a walked tree go through the `include` and `exclude`
//! blocks, so `dotfmt a.conf`, `dotfmt .` and `dotfmt --owns` cannot disagree
//! about which files dotfmt owns. A named file that the config does not pick
//! up is a failure rather than a silent skip: somebody who names a file is
//! owed an answer about it.
//!
//! Settings are per directory, so two subtrees can select differently and the
//! walk asks [`Configs`] once for each directory it reads. The directories
//! nothing wants formatted are skipped whole — a `node_modules` or a `target`
//! is not somebody's config — and symlinked directories are not descended
//! into, which keeps a link loop from turning a format run into a hang.

use std::fs;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::config::Configs;
use crate::native::{self, Kind};
use crate::select::Token;

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

/// One file a run is about, and which formatter its config picked it up for.
#[derive(Debug)]
pub struct Found {
    pub path: PathBuf,
    pub kind: Kind,
}

/// What a target came to.
#[derive(Debug)]
pub struct Gathered {
    /// The files to format, in a settled order.
    pub files: Vec<Found>,
    /// Configs below the target that could not be read. Each is reported and
    /// counted as a failure; the directories that *did* resolve are still
    /// formatted, because one unreadable config should not hide a whole tree.
    pub problems: Vec<String>,
}

/// Every file under `target` its own config picks up.
///
/// A missing target is a failure rather than an empty list. `blocks.read`
/// answers `[]` for a file it cannot open, which is the right default for a
/// reader and the wrong one here: a typo in a path would report a clean run
/// over nothing at all.
pub fn gather(target: &Path, configs: &Configs) -> Result<Gathered, String> {
    let found = fs::metadata(target).map_err(|error| format!("{}: {error}", target.display()))?;
    if !found.is_dir() {
        let config = configs.for_file(target)?;
        let Some(token) = config.owns(target) else {
            return Err(refusal(target));
        };
        return Ok(Gathered {
            files: vec![Found {
                path: target.to_path_buf(),
                kind: native::formatter(token),
            }],
            problems: Vec::new(),
        });
    }
    let (mut files, mut problems) = read(target.to_path_buf(), configs);
    files.sort_by(|one, other| one.path.cmp(&other.path));
    problems.sort();
    problems.dedup();
    Ok(Gathered { files, problems })
}

/// Why a named file is not going to be formatted.
///
/// Two answers, because they call for two different things: a `.py` is a file
/// dotfmt has no formatter for at all, while a `LICENSE` or a `.conf` is one
/// it could format and was told not to.
fn refusal(path: &Path) -> String {
    match Token::of(path) {
        None => format!("not a .conf, .config or .dotfile file: {}", path.display()),
        Some(_) => format!("not selected by this config: {}", path.display()),
    }
}

/// Each directory's subdirectories are read in parallel: the walk spends its
/// time waiting on directory reads, and a thread per core hides most of that
/// on a large tree.
fn read(path: PathBuf, configs: &Configs) -> (Vec<Found>, Vec<String>) {
    let Ok(listing) = fs::read_dir(&path) else {
        return (Vec::new(), Vec::new());
    };
    let mut below = Vec::new();
    let mut here = Vec::new();
    for entry in listing.flatten() {
        // `file_type` reads the directory entry rather than what it points at,
        // so a symlink is never mistaken for the directory on the other end.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if !SKIP.contains(&entry.file_name().display().to_string().as_str()) {
                below.push(entry.path());
            }
            continue;
        }
        here.push(entry.path());
    }

    // One config for the whole directory, because every file in it resolves to
    // the same one, and none at all when the directory holds no files to ask
    // about.
    let mut files = Vec::new();
    let mut problems = Vec::new();
    if !here.is_empty() {
        match configs.for_directory(&path) {
            Ok(config) => files.extend(here.into_iter().filter_map(|path| {
                config.owns(&path).map(|token| Found {
                    kind: native::formatter(token),
                    path,
                })
            })),
            Err(message) => problems.push(message),
        }
    }

    let deeper: Vec<(Vec<Found>, Vec<String>)> =
        below.into_par_iter().map(|at| read(at, configs)).collect();
    for (found, trouble) in deeper {
        files.extend(found);
        problems.extend(trouble);
    }
    (files, problems)
}
