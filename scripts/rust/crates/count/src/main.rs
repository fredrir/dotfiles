
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, ValueHint};
use rayon::prelude::*;
use workstation::Completions;

const PROGRAM: &str = "count";

#[derive(Parser)]
#[command(version, about = "Count items inside a directory")]
struct Cli {
    #[arg(value_hint = ValueHint::DirPath, required_unless_present = "shell")]
    directory: Option<PathBuf>,

    #[arg(short = 'r', long = "recursive")]
    recursive: bool,

    #[arg(short = 'd', long = "no-hidden")]
    no_hidden: bool,

    #[command(flatten)]
    completions: Completions,
}

#[derive(Default, Clone, Copy)]
struct Tally {
    entries: u64,
    unreadable: usize,
}

impl Tally {
    fn add(&mut self, other: Tally) {
        self.entries += other.entries;
        self.unreadable += other.unreadable;
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Some(status) = cli.completions.emit::<Cli>(PROGRAM) {
        return status;
    }
    let Some(directory) = cli.directory.as_deref() else {
        return workstation::fail(PROGRAM, "missing directory");
    };
    if let Err(message) = require_directory(directory) {
        return workstation::fail(PROGRAM, message);
    }

    if cli.recursive {
        let tally = count_recursive(directory, cli.no_hidden);
        println!("{}", tally.entries);
        if tally.unreadable > 0 {
            let plural = if tally.unreadable == 1 {
                "directory"
            } else {
                "directories"
            };
            eprintln!("{PROGRAM}: {} {plural} could not be read", tally.unreadable);
        }
        ExitCode::SUCCESS
    } else {
        match count_children(directory, cli.no_hidden) {
            Ok(entries) => {
                println!("{entries}");
                ExitCode::SUCCESS
            }
            Err(error) => workstation::fail(PROGRAM, format!("{}: {error}", directory.display())),
        }
    }
}

fn require_directory(directory: &Path) -> Result<(), String> {
    match fs::metadata(directory) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(format!("not a directory: {}", directory.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(format!(
            "no such file or directory: {}",
            directory.display()
        )),
        Err(error) => Err(format!("{}: {error}", directory.display())),
    }
}

fn hidden(name: &OsStr) -> bool {
    name.as_encoded_bytes().starts_with(b".")
}

fn count_children(directory: &Path, no_hidden: bool) -> io::Result<u64> {
    let mut entries = 0;
    for entry in fs::read_dir(directory)? {
        if no_hidden && hidden(&entry?.file_name()) {
            continue;
        }
        entries += 1;
    }
    Ok(entries)
}

fn count_recursive(directory: &Path, no_hidden: bool) -> Tally {
    let Ok(listing) = fs::read_dir(directory) else {
        return Tally {
            entries: 0,
            unreadable: 1,
        };
    };
    // An entry that fails mid-listing is one this walk cannot see, which is
    // the same thing an unreadable directory is: say so rather than let the
    // total quietly come up short.
    let mut unread = Tally::default();
    let entries: Vec<fs::DirEntry> = listing
        .filter_map(|entry| match entry {
            Ok(entry) => Some(entry),
            Err(_) => {
                unread.unreadable += 1;
                None
            }
        })
        .collect();
    let mut tally = entries
        .into_par_iter()
        .map(|entry| {
            if no_hidden && hidden(&entry.file_name()) {
                return Tally::default();
            }
            let mut tally = Tally {
                entries: 1,
                unreadable: 0,
            };
            // `file_type` reads the directory entry rather than the target, so
            // a symlink is never mistaken for the directory it points at.
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                tally.add(count_recursive(&entry.path(), no_hidden));
            }
            tally
        })
        .reduce(Tally::default, |mut left, right| {
            left.add(right);
            left
        });
    tally.add(unread);
    tally
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let path = |name: &str| root.path().join(name);
        fs::write(path("a"), "").unwrap();
        fs::write(path(".b"), "").unwrap();
        fs::create_dir(path("sub")).unwrap();
        fs::write(path("sub/c"), "").unwrap();
        fs::create_dir(path("sub/.hid")).unwrap();
        fs::write(path("sub/.hid/d"), "").unwrap();
        fs::create_dir(path(".hidden")).unwrap();
        fs::write(path(".hidden/e"), "").unwrap();
        root
    }

    #[test]
    fn counts_direct_children() {
        let root = tree();
        assert_eq!(count_children(root.path(), false).unwrap(), 4);
    }

    #[test]
    fn counts_every_descendant() {
        let root = tree();
        assert_eq!(count_recursive(root.path(), false).entries, 8);
    }

    #[test]
    fn skips_hidden_children() {
        let root = tree();
        assert_eq!(count_children(root.path(), true).unwrap(), 2);
    }

    #[test]
    fn hidden_directories_take_their_subtree_with_them() {
        let root = tree();
        assert_eq!(count_recursive(root.path(), true).entries, 3);
    }

    #[test]
    fn an_empty_directory_counts_zero() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(count_children(root.path(), false).unwrap(), 0);
        assert_eq!(count_recursive(root.path(), false).entries, 0);
    }

    #[test]
    fn unreadable_directories_are_reported_not_counted() {
        let root = tree();
        let tally = count_recursive(&root.path().join("missing"), false);
        assert_eq!(tally.entries, 0);
        assert_eq!(tally.unreadable, 1);
    }

    #[cfg(unix)]
    #[test]
    fn linked_directories_count_once_and_are_not_followed() {
        let root = tree();
        std::os::unix::fs::symlink(root.path().join("sub"), root.path().join("link")).unwrap();
        assert_eq!(count_children(root.path(), false).unwrap(), 5);
        assert_eq!(count_recursive(root.path(), false).entries, 9);
    }

    #[test]
    fn a_file_is_not_a_directory() {
        let root = tree();
        assert!(require_directory(&root.path().join("a")).is_err());
        assert!(require_directory(&root.path().join("missing")).is_err());
        assert!(require_directory(root.path()).is_ok());
    }
}
