use std::fs;
use std::path::{Path, PathBuf};

use workstation::walk::{Policy, walk};

#[derive(Debug)]
pub struct Gathered {
    pub files: Vec<PathBuf>,
    pub unreadable: usize,
}

/// A file named on the command line is formatted whatever it is called: the
/// caller has said what they want by naming it. A directory is walked for
/// `*.json`, and the walk's own policy keeps the trees nobody means.
pub fn gather(target: &Path) -> Result<Gathered, String> {
    let found = fs::metadata(target).map_err(|error| format!("{}: {error}", target.display()))?;
    if !found.is_dir() {
        return Ok(Gathered {
            files: vec![target.to_path_buf()],
            unreadable: 0,
        });
    }

    let walked = walk(target, &Policy::new(), |_directory, entries| {
        entries
            .iter()
            .filter(|entry| entry.is_file() || entry.is_symlink())
            .map(|entry| entry.path.clone())
            .filter(|path| is_json(path))
            .collect()
    });
    let mut files = walked.items;
    files.sort();
    Ok(Gathered {
        files,
        unreadable: walked.unreadable,
    })
}

fn is_json(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
}
