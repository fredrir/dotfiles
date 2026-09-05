use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use workstation::walk::Policy;

use crate::config::Configs;
use crate::native::{self, Kind};
use crate::select::Token;

#[derive(Debug)]
pub struct Found {
    pub path: PathBuf,
    pub kind: Kind,
}

#[derive(Debug)]
pub struct Gathered {
    pub files: Vec<Found>,
    pub problems: Vec<String>,
    pub unreadable: usize,
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
            unreadable: 0,
        });
    }

    let trouble: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let walked = workstation::walk::walk(target, &Policy::new(), |directory, entries| {
        let here: Vec<PathBuf> = entries
            .iter()
            .filter(|entry| !entry.is_dir())
            .map(|entry| entry.path.clone())
            .collect();
        if here.is_empty() {
            return Vec::new();
        }
        // One config for the whole directory, because every file in it resolves
        // to the same one, and none at all when the directory holds no files to
        // ask about.
        match configs.for_directory(directory) {
            Ok(config) => here
                .into_iter()
                .filter_map(|path| {
                    config.owns(&path).map(|token| Found {
                        kind: native::formatter(token),
                        path,
                    })
                })
                .collect(),
            Err(message) => {
                let mut said = trouble.lock().unwrap_or_else(|held| held.into_inner());
                said.push(message);
                Vec::new()
            }
        }
    });

    let mut files = walked.items;
    files.sort_by(|one, other| one.path.cmp(&other.path));
    let mut problems = trouble
        .into_inner()
        .unwrap_or_else(|held| held.into_inner());
    problems.sort();
    problems.dedup();
    Ok(Gathered {
        files,
        problems,
        unreadable: walked.unreadable,
    })
}

fn refusal(path: &Path) -> String {
    match Token::of(path) {
        None => format!("not a .conf, .config or .dotfile file: {}", path.display()),
        Some(_) => format!("not selected by this config: {}", path.display()),
    }
}
