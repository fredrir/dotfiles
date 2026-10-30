//! Next and previous file in the same directory, in Explorer order.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::exclude::Exclude;
pub use crate::sort::Explorer;
use crate::sort::{Entry, Sorter};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Params {
    pub path: PathBuf,
    /// `1` for the next file, `-1` for the previous one.
    pub offset: isize,
    /// Workspace folder that `files.exclude` patterns are relative to.
    #[serde(default)]
    pub root: Option<PathBuf>,
    /// BCP 47 tag of the VS Code display language.
    #[serde(default)]
    pub locale: String,
    #[serde(default)]
    pub explorer: Explorer,
    #[serde(default)]
    pub exclude: Map<String, Value>,
}

/// The file `offset` steps away from `path`, wrapping around; `None` when there is no other file.
pub fn sibling(params: &Params) -> io::Result<Option<PathBuf>> {
    let (Some(dir), Some(current)) = (
        params.path.parent(),
        params.path.file_name().and_then(|name| name.to_str()),
    ) else {
        return Ok(None);
    };
    let sorter = Sorter::new(&params.explorer, &params.locale)?;
    let exclude = Exclude::new(&params.exclude);
    let prefix = params
        .root
        .as_deref()
        .and_then(|root| dir.strip_prefix(root).ok())
        .map(slash_separated)
        .unwrap_or_default();

    let children: Vec<(String, bool)> = fs::read_dir(dir)?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let is_file = entry
                .file_type()
                .is_ok_and(|kind| kind.is_file() || kind.is_symlink() && entry.path().is_file());
            Some((name, is_file))
        })
        .collect();
    let siblings: HashSet<&str> = children.iter().map(|(name, _)| name.as_str()).collect();

    let mut files: Vec<Entry> = children
        .iter()
        .filter(|(name, is_file)| {
            *is_file && (name == current || !exclude.matches(&relative(&prefix, name), &siblings))
        })
        .map(|(name, _)| Entry {
            name: name.clone(),
            modified: if sorter.by_modified() {
                modified(&dir.join(name))
            } else {
                0
            },
        })
        .collect();
    files.sort_by(|a, b| sorter.compare(a, b));

    let Some(index) = files.iter().position(|file| file.name == current) else {
        return Ok(None);
    };
    let target = (index as isize + params.offset).rem_euclid(files.len() as isize) as usize;
    Ok(Some(&files[target].name)
        .filter(|name| *name != current)
        .map(|name| dir.join(name)))
}

fn slash_separated(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn relative(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}/{name}")
    }
}

fn modified(path: &Path) -> u128 {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |elapsed| elapsed.as_millis())
}
