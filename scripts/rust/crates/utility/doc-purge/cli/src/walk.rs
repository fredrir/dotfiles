use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use ignore::{DirEntry, WalkBuilder};

use doc_purge_core::lang::{self, Dialect, Language};

use crate::config::{CONFIG_NAME, Config, Generated, Resolver};

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
    pub explicit: bool,
    /// A reason known from the filename alone, such as `generated`.
    pub skip: Option<&'static str>,
    /// Check the first 2 KiB for a generated-file marker before purging.
    pub check_generated: bool,
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
    pub fn allows(&self, path: &Path) -> bool {
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
    pub warnings: Vec<String>,
    pub unreadable: usize,
}

pub fn gather(
    targets: &[PathBuf],
    wanted: &Wanted,
    resolver: &Resolver,
) -> Result<Gathered, String> {
    let mut gathered = Gathered {
        files: Vec::new(),
        notes: Vec::new(),
        warnings: Vec::new(),
        unreadable: 0,
    };
    for target in targets {
        let metadata = fs::symlink_metadata(target).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => {
                format!("no such file or directory: {}", target.display())
            }
            _ => format!("{}: {error}", target.display()),
        })?;
        let config = resolver
            .for_path(target)
            .map_err(|error| error.to_string())?;
        for warning in &config.warnings {
            if !gathered.warnings.contains(warning) {
                gathered.warnings.push(warning.clone());
            }
        }
        if metadata.is_symlink() {
            gathered.notes.push(Note {
                path: target.clone(),
                reason: "a symbolic link",
            });
            continue;
        }
        if !metadata.is_file() && !metadata.is_dir() {
            gathered.notes.push(Note {
                path: target.clone(),
                reason: "not a regular file",
            });
            continue;
        }
        if metadata.is_dir() {
            fs::read_dir(target).map_err(|error| format!("{}: {error}", target.display()))?;
            gather_directory(target, &config, wanted, resolver, &mut gathered);
            continue;
        }
        gather_explicit(target, &config, wanted, resolver, &mut gathered);
    }
    Ok(gathered)
}

fn gather_directory(
    directory: &Path,
    config: &Config,
    wanted: &Wanted,
    resolver: &Resolver,
    gathered: &mut Gathered,
) {
    gathered.unreadable += visit_files(directory, config.respect_gitignore, |path| {
        let Some(language) = claim(path, wanted, config, resolver) else {
            return;
        };
        gathered.files.push(Found {
            path: path.to_path_buf(),
            dialect: language.dialect,
            explicit: false,
            skip: filename_skip(path, config),
            check_generated: config.generated == Generated::Skip,
        });
    });
}

fn gather_explicit(
    target: &Path,
    config: &Config,
    wanted: &Wanted,
    resolver: &Resolver,
    gathered: &mut Gathered,
) {
    let absolute = resolver.absolute(target);
    if !config.allows_path(&absolute) {
        gathered.notes.push(Note {
            path: target.to_path_buf(),
            reason: "excluded by doc-purge.dotfile",
        });
        return;
    }
    let Some(language) = lang::for_path(target) else {
        gathered.notes.push(Note {
            path: target.to_path_buf(),
            reason: "not a file type doc-purge reads",
        });
        return;
    };
    if !wanted.allows(target) {
        return;
    }
    if !config.language_enabled(language.name, &absolute) {
        gathered.notes.push(Note {
            path: target.to_path_buf(),
            reason: "disabled by doc-purge.dotfile",
        });
        return;
    }
    gathered.files.push(Found {
        path: target.to_path_buf(),
        dialect: language.dialect,
        explicit: true,
        skip: filename_skip(target, config),
        check_generated: config.generated == Generated::Skip,
    });
}

fn claim(
    path: &Path,
    wanted: &Wanted,
    config: &Config,
    resolver: &Resolver,
) -> Option<&'static Language> {
    let language = lang::for_path(path)?;
    if !wanted.allows(path) {
        return None;
    }
    let absolute = resolver.absolute(path);
    if !config.allows_path(&absolute) {
        return None;
    }
    if !config.language_enabled(language.name, &absolute) {
        return None;
    }
    Some(language)
}

fn filename_skip(path: &Path, config: &Config) -> Option<&'static str> {
    (config.generated == Generated::Skip && generated_name(path)).then_some("generated")
}

fn visit_files(directory: &Path, respect_gitignore: bool, mut visit: impl FnMut(&Path)) -> usize {
    let mut builder = WalkBuilder::new(directory);
    builder
        .hidden(true)
        .follow_links(false)
        .require_git(false)
        .git_ignore(respect_gitignore)
        .git_global(respect_gitignore)
        .git_exclude(respect_gitignore)
        .parents(respect_gitignore)
        .filter_entry(|entry| !avoided(entry));
    let mut unreadable = 0usize;
    for result in builder.build() {
        match result {
            Ok(entry) => {
                if entry.depth() == 0 || !entry.file_type().is_some_and(|kind| kind.is_file()) {
                    continue;
                }
                visit(entry.path());
            }
            Err(_) => unreadable += 1,
        }
    }
    unreadable
}

fn avoided(entry: &DirEntry) -> bool {
    if entry.depth() == 0 || !entry.file_type().is_some_and(|kind| kind.is_dir()) {
        return false;
    }
    entry
        .file_name()
        .to_str()
        .is_some_and(|name| AVOIDED.contains(&name))
}

/// Generated files named by convention, without opening them.
pub fn generated_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    name.ends_with(".min.js")
        || name.ends_with(".map")
        || name.ends_with("-lock.json")
        || name.ends_with(".lock")
        || name.ends_with(".tsbuildinfo")
}

/// Generated-file markers in the first 2 KiB of a source.
/// Generated by name or by the markers in the first 2 KiB.
pub fn generated_file(path: &Path) -> bool {
    generated_name(path) || generated_head(path)
}

pub fn generated(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(2048)];
    let text = String::from_utf8_lossy(head).to_ascii_lowercase();
    [
        "@generated",
        "do not edit",
        "spdx-filetype: generated",
        "code generated by",
    ]
    .iter()
    .any(|marker| text.contains(marker))
}

pub struct Uncovered {
    pub keys: Vec<(String, usize)>,
    pub warnings: Vec<String>,
    pub unreadable: usize,
}

pub fn uncover(targets: &[PathBuf], resolver: &Resolver) -> Result<Uncovered, String> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut warnings = Vec::new();
    let mut unreadable = 0usize;
    for target in targets {
        let metadata = fs::symlink_metadata(target).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => {
                format!("no such file or directory: {}", target.display())
            }
            _ => format!("{}: {error}", target.display()),
        })?;
        let config = resolver
            .for_path(target)
            .map_err(|error| error.to_string())?;
        for warning in &config.warnings {
            if !warnings.contains(warning) {
                warnings.push(warning.clone());
            }
        }
        if metadata.is_symlink() || !metadata.is_file() && !metadata.is_dir() {
            continue;
        }
        if metadata.is_dir() {
            fs::read_dir(target).map_err(|error| format!("{}: {error}", target.display()))?;
            unreadable += visit_files(target, config.respect_gitignore, |path| {
                note_uncovered(path, &config, resolver, &mut counts);
            });
            continue;
        }
        note_uncovered(target, &config, resolver, &mut counts);
    }
    let mut keys: Vec<(String, usize)> = counts.into_iter().collect();
    keys.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    Ok(Uncovered {
        keys,
        warnings,
        unreadable,
    })
}

fn note_uncovered(
    path: &Path,
    config: &Config,
    resolver: &Resolver,
    counts: &mut BTreeMap<String, usize>,
) {
    if lang::for_path(path).is_some() {
        return;
    }
    if path.file_name().and_then(OsStr::to_str) == Some(CONFIG_NAME) {
        return;
    }
    let absolute = resolver.absolute(path);
    if !config.allows_path(&absolute) {
        return;
    }
    if config.generated == Generated::Skip && generated_name(path) {
        return;
    }
    if config.generated == Generated::Skip && generated_head(path) {
        return;
    }
    *counts.entry(uncovered_key(path)).or_default() += 1;
}

fn generated_head(path: &Path) -> bool {
    use std::io::Read;
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let mut head = Vec::new();
    if file.take(2048).read_to_end(&mut head).is_err() {
        return false;
    }
    generated(&head)
}

fn uncovered_key(path: &Path) -> String {
    path.extension()
        .and_then(OsStr::to_str)
        .map(|extension| extension.to_ascii_lowercase())
        .or_else(|| {
            path.file_name()
                .and_then(OsStr::to_str)
                .map(|name| name.to_ascii_lowercase())
        })
        .unwrap_or_else(|| "(unnamed)".to_string())
}
