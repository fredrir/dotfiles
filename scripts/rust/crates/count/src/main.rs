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
#[path = "../tests/unit/main_tests.rs"]
mod tests;
