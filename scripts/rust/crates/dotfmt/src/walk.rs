
use std::fs;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::config::Configs;
use crate::native::{self, Kind};
use crate::select::Token;

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

#[derive(Debug)]
pub struct Found {
    pub path: PathBuf,
    pub kind: Kind,
}

#[derive(Debug)]
pub struct Gathered {
    pub files: Vec<Found>,
    pub problems: Vec<String>,
}

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

fn refusal(path: &Path) -> String {
    match Token::of(path) {
        None => format!("not a .conf, .config or .dotfile file: {}", path.display()),
        Some(_) => format!("not selected by this config: {}", path.display()),
    }
}

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
