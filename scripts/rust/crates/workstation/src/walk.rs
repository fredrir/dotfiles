use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use rayon::prelude::*;

pub const SKIP: &[&str] = &[
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: OsString,
    pub path: PathBuf,
    pub relative: PathBuf,
    pub kind: Kind,
}

impl Entry {
    pub fn is_file(&self) -> bool {
        self.kind == Kind::File
    }

    pub fn is_dir(&self) -> bool {
        self.kind == Kind::Directory
    }

    pub fn is_symlink(&self) -> bool {
        self.kind == Kind::Symlink
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Symlinks {
    Report,
    Drop,
}

#[derive(Clone, Copy)]
pub struct Policy {
    skip: &'static [&'static str],
    skip_hidden: bool,
    max_depth: Option<usize>,
    max_files: Option<usize>,
    symlinks: Symlinks,
}

impl Default for Policy {
    fn default() -> Policy {
        Policy {
            skip: SKIP,
            skip_hidden: false,
            max_depth: None,
            max_files: None,
            symlinks: Symlinks::Report,
        }
    }
}

impl Policy {
    pub fn new() -> Policy {
        Policy::default()
    }

    pub fn skipping(mut self, names: &'static [&'static str]) -> Policy {
        self.skip = names;
        self
    }

    pub fn skip_hidden(mut self, skip: bool) -> Policy {
        self.skip_hidden = skip;
        self
    }

    pub fn max_depth(mut self, depth: usize) -> Policy {
        self.max_depth = Some(depth);
        self
    }

    pub fn max_files(mut self, files: usize) -> Policy {
        self.max_files = Some(files);
        self
    }

    pub fn symlinks(mut self, symlinks: Symlinks) -> Policy {
        self.symlinks = symlinks;
        self
    }

    fn skips(&self, name: &OsStr) -> bool {
        self.skip.iter().any(|skip| OsStr::new(skip) == name)
    }

    fn hides(&self, name: &OsStr) -> bool {
        self.skip_hidden && crate::path::hidden(name)
    }

    fn too_deep(&self, depth: usize) -> bool {
        self.max_depth.is_some_and(|max| depth > max)
    }
}

#[derive(Debug)]
pub struct Walked<T> {
    pub items: Vec<T>,
    pub unreadable: usize,
    pub deep: usize,
    pub capped: bool,
}

impl<T> Default for Walked<T> {
    fn default() -> Walked<T> {
        Walked {
            items: Vec::new(),
            unreadable: 0,
            deep: 0,
            capped: false,
        }
    }
}

impl<T> Walked<T> {
    fn absorb(&mut self, other: Walked<T>) {
        self.items.extend(other.items);
        self.unreadable += other.unreadable;
        self.deep += other.deep;
        self.capped |= other.capped;
    }
}

pub fn walk<T, V>(root: &Path, policy: &Policy, visit: V) -> Walked<T>
where
    T: Send,
    V: Fn(&Path, &[Entry]) -> Vec<T> + Sync,
{
    let taken = AtomicUsize::new(0);
    read(root, Path::new(""), 0, policy, &visit, &taken)
}

pub fn list(directory: &Path, policy: &Policy) -> Result<Vec<Entry>, String> {
    let listing = fs::read_dir(directory).map_err(|error| trouble(directory, &error))?;
    let mut entries = Vec::new();
    for found in listing {
        let found = found.map_err(|error| trouble(directory, &error))?;
        if let Some(entry) = describe(&found, Path::new(""), policy) {
            entries.push(entry);
        }
    }
    entries.sort_by(|one, other| one.name.cmp(&other.name));
    Ok(entries)
}

fn trouble(directory: &Path, error: &io::Error) -> String {
    format!("{}: {error}", directory.display())
}

fn read<T, V>(
    directory: &Path,
    prefix: &Path,
    depth: usize,
    policy: &Policy,
    visit: &V,
    taken: &AtomicUsize,
) -> Walked<T>
where
    T: Send,
    V: Fn(&Path, &[Entry]) -> Vec<T> + Sync,
{
    let Ok(listing) = fs::read_dir(directory) else {
        return Walked {
            unreadable: 1,
            ..Walked::default()
        };
    };
    let mut walked = Walked::default();
    let mut entries = Vec::new();
    for found in listing {
        let Ok(found) = found else {
            walked.unreadable += 1;
            continue;
        };
        if let Some(entry) = describe(&found, prefix, policy) {
            entries.push(entry);
        }
    }
    entries.sort_by(|one, other| one.name.cmp(&other.name));

    let mut below = Vec::new();
    for entry in &entries {
        if !entry.is_dir() {
            continue;
        }
        if policy.too_deep(depth + 1) {
            walked.deep += 1;
            continue;
        }
        below.push((entry.path.clone(), entry.relative.clone()));
    }

    let (items, capped) = within(visit(directory, &entries), policy, taken);
    walked.items = items;
    walked.capped |= capped;

    let deeper: Vec<Walked<T>> = below
        .into_par_iter()
        .map(|(path, relative)| read(&path, &relative, depth + 1, policy, visit, taken))
        .collect();
    for other in deeper {
        walked.absorb(other);
    }
    walked
}

fn describe(found: &fs::DirEntry, prefix: &Path, policy: &Policy) -> Option<Entry> {
    let kind = found
        .file_type()
        .map_or(Kind::Other, |reported| classify(&reported));
    let name = found.file_name();
    let refused = policy.hides(&name)
        || (kind == Kind::Symlink && policy.symlinks == Symlinks::Drop)
        || (kind == Kind::Directory && policy.skips(&name));
    if refused {
        return None;
    }
    Some(Entry {
        path: found.path(),
        relative: prefix.join(&name),
        name,
        kind,
    })
}

fn classify(kind: &fs::FileType) -> Kind {
    if kind.is_symlink() {
        Kind::Symlink
    } else if kind.is_dir() {
        Kind::Directory
    } else if kind.is_file() {
        Kind::File
    } else {
        Kind::Other
    }
}

fn within<T>(mut items: Vec<T>, policy: &Policy, taken: &AtomicUsize) -> (Vec<T>, bool) {
    let Some(max) = policy.max_files else {
        return (items, false);
    };
    let room = max.saturating_sub(taken.fetch_add(items.len(), Ordering::Relaxed));
    if items.len() <= room {
        return (items, false);
    }
    items.truncate(room);
    (items, true)
}

#[cfg(test)]
#[path = "../tests/unit/walk_tests.rs"]
mod tests;
