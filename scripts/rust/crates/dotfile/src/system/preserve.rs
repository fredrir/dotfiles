use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::blocks;
use crate::secret::vault::SecretEntry;

pub type Preserved = BTreeMap<PathBuf, Vec<String>>;

pub fn load(marker: &Path, entries: &[SecretEntry]) -> Result<Preserved, String> {
    let mut preserved = Preserved::new();
    for entry in blocks::read(marker)? {
        let at = || format!("{} line {}", marker.display(), entry.number);
        if entry.block != "preserve" {
            return Err(format!("{}: unknown block {}", at(), entry.block));
        }
        if entry.opens {
            continue;
        }
        let (destination, prefix) = entry.split();
        let destination = PathBuf::from(destination);
        if prefix.is_empty() || !destination.is_absolute() {
            return Err(format!("{}: expected /destination = line prefix", at()));
        }
        if !entries
            .iter()
            .any(|tracked| tracked.destination == destination)
        {
            return Err(format!(
                "{}: {} is not tracked",
                at(),
                destination.display()
            ));
        }
        preserved
            .entry(destination)
            .or_default()
            .push(prefix.into());
    }
    Ok(preserved)
}

fn owner<'a>(line: &[u8], prefixes: &'a [String]) -> Option<&'a str> {
    prefixes
        .iter()
        .map(String::as_str)
        .find(|prefix| line.starts_with(prefix.as_bytes()))
}

pub fn carry(wanted: &[u8], installed: &[u8], prefixes: &[String]) -> Vec<u8> {
    let installed = installed.split(|byte| *byte == b'\n').collect::<Vec<_>>();
    let mut carried = Vec::with_capacity(wanted.len());
    for line in wanted.split_inclusive(|byte| *byte == b'\n') {
        let (body, newline) = line
            .strip_suffix(b"\n")
            .map_or((line, &b""[..]), |body| (body, &b"\n"[..]));
        let live = owner(body, prefixes).and_then(|prefix| {
            installed
                .iter()
                .find(|candidate| owner(candidate, prefixes) == Some(prefix))
        });
        carried.extend_from_slice(live.copied().unwrap_or(body));
        carried.extend_from_slice(newline);
    }
    carried
}

#[cfg(test)]
#[path = "../../tests/unit/system/preserve_tests.rs"]
mod tests;
