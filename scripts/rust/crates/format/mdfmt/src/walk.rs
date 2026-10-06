use std::fs;
use std::path::{Path, PathBuf};

use workstation::walk::{Policy, walk};

#[derive(Debug)]
pub struct Gathered {
    pub files: Vec<PathBuf>,
    pub unreadable: usize,
}

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
            .filter(|entry| entry.is_file())
            .map(|entry| entry.path.clone())
            .filter(|path| is_markdown(path))
            .collect()
    });
    let mut files = walked.items;
    files.sort();
    Ok(Gathered {
        files,
        unreadable: walked.unreadable,
    })
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ["md", "markdown", "mdown", "mkd"]
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
}
