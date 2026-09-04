use std::env;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::{Directory, DirectoryStatus, Entry, EntryKind, FileSource, InputKind};

#[derive(Clone, Debug)]
pub struct LocalSource {
    home: Option<PathBuf>,
}

impl LocalSource {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_home(home: impl Into<PathBuf>) -> Self {
        Self {
            home: Some(home.into()),
        }
    }

    pub fn without_home() -> Self {
        Self { home: None }
    }

    pub fn home(&self) -> Option<&Path> {
        self.home.as_deref()
    }
}

impl Default for LocalSource {
    fn default() -> Self {
        Self {
            home: env::var_os("HOME").map(PathBuf::from),
        }
    }
}

#[derive(Debug)]
pub enum LocalError {
    HomeUnavailable,
    UnsupportedHome(String),
}

impl fmt::Display for LocalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HomeUnavailable => formatter.write_str("home directory is unavailable"),
            Self::UnsupportedHome(value) => {
                write!(formatter, "named home expansion is unsupported: {value}")
            }
        }
    }
}

impl std::error::Error for LocalError {}

impl FileSource for LocalSource {
    type Location = PathBuf;
    type Error = LocalError;

    fn read_directory(&self, location: &PathBuf) -> Result<Directory<PathBuf>, Self::Error> {
        let parent = parent_of(location);
        let label = display_path(location);
        let read = match fs::read_dir(location) {
            Ok(read) => read,
            Err(error) => {
                let status = match error.kind() {
                    io::ErrorKind::NotFound => DirectoryStatus::Missing,
                    _ => DirectoryStatus::Unreadable(error.to_string()),
                };
                return Ok(Directory {
                    location: location.clone(),
                    parent,
                    label,
                    entries: Vec::new(),
                    status,
                });
            }
        };
        let location = opened_location(location);
        let parent = parent_of(&location);
        let label = display_path(&location);

        let mut entries = Vec::new();
        for result in read {
            let entry = match result {
                Ok(entry) => entry,
                Err(error) => {
                    return Ok(Directory {
                        location: location.clone(),
                        parent,
                        label,
                        entries: Vec::new(),
                        status: DirectoryStatus::Unreadable(error.to_string()),
                    });
                }
            };
            let kind = match entry.file_type() {
                Ok(file_type) if file_type.is_dir() => EntryKind::Directory,
                Ok(file_type) if file_type.is_file() => EntryKind::File,
                Ok(file_type) if file_type.is_symlink() => match fs::metadata(entry.path()) {
                    Ok(metadata) if metadata.is_dir() => EntryKind::SymlinkDirectory,
                    _ => EntryKind::Symlink,
                },
                Ok(_) => EntryKind::Other,
                Err(error) => {
                    return Ok(Directory {
                        location: location.clone(),
                        parent,
                        label,
                        entries: Vec::new(),
                        status: DirectoryStatus::Unreadable(error.to_string()),
                    });
                }
            };
            let name = entry.file_name();
            entries.push(Entry {
                location: location.join(&name),
                name: display_os_str(&name),
                kind,
            });
        }
        entries.sort_by(|left, right| {
            entry_rank(left.kind)
                .cmp(&entry_rank(right.kind))
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.location.cmp(&right.location))
        });

        Ok(Directory {
            location,
            parent,
            label,
            entries,
            status: DirectoryStatus::Present,
        })
    }

    fn input_kind(&self, text: &str) -> InputKind {
        let windows_path = cfg!(windows)
            && (text.starts_with(".\\") || text.starts_with("..\\") || text.contains('\\'));
        if text.starts_with('~')
            || matches!(text, "." | "..")
            || text.starts_with("./")
            || text.starts_with("../")
            || Path::new(text).is_absolute()
            || text.contains('/')
            || windows_path
        {
            InputKind::Location
        } else {
            InputKind::Search
        }
    }

    fn resolve_input(&self, current: &PathBuf, text: &str) -> Result<PathBuf, Self::Error> {
        if text.is_empty() {
            return Ok(current.clone());
        }
        let expanded = if text == "~" {
            self.home.clone().ok_or(LocalError::HomeUnavailable)?
        } else if let Some(rest) = home_relative(text) {
            self.home
                .as_ref()
                .ok_or(LocalError::HomeUnavailable)?
                .join(rest)
        } else if text.starts_with('~') {
            return Err(LocalError::UnsupportedHome(text.to_string()));
        } else {
            let path = Path::new(text);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                current.join(path)
            }
        };
        Ok(expanded)
    }
}

fn opened_location(path: &Path) -> PathBuf {
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    } else {
        path.to_path_buf()
    }
}

fn parent_of(path: &Path) -> Option<PathBuf> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty() && *parent != path)
        .map(Path::to_path_buf)
}

fn entry_rank(kind: EntryKind) -> u8 {
    match kind {
        EntryKind::Directory => 0,
        EntryKind::SymlinkDirectory => 0,
        EntryKind::File => 1,
        EntryKind::Symlink => 2,
        EntryKind::Other => 3,
    }
}

fn display_path(path: &Path) -> String {
    sanitize(&path.to_string_lossy())
}

fn display_os_str(value: &OsStr) -> String {
    sanitize(&value.to_string_lossy())
}

fn sanitize(value: &str) -> String {
    let mut shown = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\n' => shown.push_str("\\n"),
            '\r' => shown.push_str("\\r"),
            '\t' => shown.push_str("\\t"),
            '\u{1b}' => shown.push_str("\\e"),
            character if character.is_control() => {
                shown.push_str(&format!("\\u{{{:x}}}", character as u32));
            }
            character => shown.push(character),
        }
    }
    shown
}

fn home_relative(text: &str) -> Option<&str> {
    text.strip_prefix("~/").or_else(|| {
        if cfg!(windows) {
            text.strip_prefix("~\\")
        } else {
            None
        }
    })
}

#[cfg(test)]
#[path = "../tests/unit/local_tests.rs"]
mod tests;
