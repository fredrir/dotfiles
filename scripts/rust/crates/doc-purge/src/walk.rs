use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::lang::{self, Dialect};

pub const LIMIT: u64 = 8 * 1024 * 1024;

const AVOIDED: &[&str] = &[
    "node_modules",
    "bower_components",
    "lua_modules",
    "luarocks",
    "site-packages",
    "third_party",
    "target",
    "dist",
    "build",
    "vendor",
    "Pods",
    "coverage",
    "__pycache__",
    "venv",
];

pub struct Found {
    pub path: PathBuf,
    pub dialect: Dialect,
}

pub struct Note {
    pub path: PathBuf,
    pub reason: &'static str,
}

#[derive(Default)]
pub struct Wanted {
    pub extensions: Option<Vec<String>>,
}

impl Wanted {
    fn allows(&self, path: &Path) -> bool {
        let Some(extensions) = &self.extensions else {
            return true;
        };
        path.extension()
            .and_then(OsStr::to_str)
            .map(|found| found.to_ascii_lowercase())
            .is_some_and(|found| extensions.contains(&found))
    }
}

pub struct Gathered {
    pub files: Vec<Found>,
    pub notes: Vec<Note>,
    pub unreadable: usize,
}

pub fn gather(targets: &[PathBuf], wanted: &Wanted) -> Result<Gathered, String> {
    let mut gathered = Gathered {
        files: Vec::new(),
        notes: Vec::new(),
        unreadable: 0,
    };
    for target in targets {
        let metadata = fs::symlink_metadata(target).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => {
                format!("no such file or directory: {}", target.display())
            }
            _ => format!("{}: {error}", target.display()),
        })?;
        if metadata.is_symlink() {
            gathered.notes.push(Note {
                path: target.clone(),
                reason: "a symbolic link",
            });
            continue;
        }
        if metadata.is_dir() {
            descend(target, wanted, &mut gathered);
            continue;
        }
        match lang::for_path(target) {
            Some(language) if wanted.allows(target) => gathered.files.push(Found {
                path: target.clone(),
                dialect: language.dialect,
            }),
            Some(_) => {}
            None => gathered.notes.push(Note {
                path: target.clone(),
                reason: "not a file type doc-purge reads",
            }),
        }
    }
    Ok(gathered)
}

fn descend(directory: &Path, wanted: &Wanted, gathered: &mut Gathered) {
    let Ok(listing) = fs::read_dir(directory) else {
        gathered.unreadable += 1;
        return;
    };
    let mut directories = Vec::new();
    for entry in listing.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with('.') || AVOIDED.contains(&name) {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        let path = entry.path();
        if kind.is_dir() {
            directories.push(path);
            continue;
        }
        let Some(language) = lang::for_path(&path) else {
            continue;
        };
        if !wanted.allows(&path) {
            continue;
        }
        gathered.files.push(Found {
            path,
            dialect: language.dialect,
        });
    }
    directories.sort();
    for path in directories {
        descend(&path, wanted, gathered);
    }
}
