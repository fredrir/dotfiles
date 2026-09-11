use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::env::{self, Sysfs};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    pub line: usize,
    pub name: String,
    pub value: String,
}

#[derive(Debug, Default)]
pub struct Export {
    pub header: Option<String>,
    pub settings: Vec<Setting>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Change {
    pub name: String,
    pub occurrence: usize,
    pub from: Option<String>,
    pub to: Option<String>,
}

impl Export {
    pub fn occurrences(&self, name: &str) -> Vec<&Setting> {
        self.settings
            .iter()
            .filter(|setting| setting.name == name)
            .collect()
    }

    pub fn header_date(&self) -> Option<String> {
        let header = self.header.as_ref()?;
        let inner = header.trim().strip_prefix('[')?.strip_suffix(']')?;
        let date = inner.split_whitespace().next()?;
        let parts = date.split('/').collect::<Vec<_>>();
        match parts.as_slice() {
            [year, month, day] if year.len() == 4 && month.len() == 2 && day.len() == 2 => {
                Some(format!("{year}{month}{day}"))
            }
            _ => None,
        }
    }

    pub fn value(&self, name: &str) -> Option<&str> {
        self.occurrences(name)
            .first()
            .map(|setting| setting.value.as_str())
    }
}

pub fn decode(bytes: &[u8]) -> Result<String, String> {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, true);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, false);
    }
    let rest = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8(rest.to_vec()).map_err(|_| "export is neither UTF-8 nor UTF-16".to_string())
}

fn utf16(bytes: &[u8], little_endian: bool) -> Result<String, String> {
    if !bytes.len().is_multiple_of(2) {
        return Err("UTF-16 export has an odd byte count".into());
    }
    let (pairs, _) = bytes.as_chunks::<2>();
    let units = pairs
        .iter()
        .map(|pair| {
            if little_endian {
                u16::from_le_bytes(*pair)
            } else {
                u16::from_be_bytes(*pair)
            }
        })
        .collect::<Vec<_>>();
    String::from_utf16(&units).map_err(|_| "UTF-16 export is malformed".to_string())
}

pub fn normalize(text: &str) -> String {
    let mut lines = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).trim_end())
        .collect::<Vec<_>>();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

pub fn parse(text: &str) -> Export {
    let mut export = Export::default();
    for (offset, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if export.header.is_none()
            && export.settings.is_empty()
            && line.starts_with('[')
            && line.ends_with(']')
            && !line.contains(" [")
        {
            export.header = Some(line.to_string());
            continue;
        }
        if let Some(body) = line.strip_suffix(']')
            && let Some((name, value)) = body.rsplit_once(" [")
        {
            export.settings.push(Setting {
                line: offset + 1,
                name: name.trim().to_string(),
                value: value.trim().to_string(),
            });
        }
    }
    export
}

pub fn sha8(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn file_name(host: &str, bios_version: &str, date: &str) -> String {
    format!("{host}-{bios_version}-{date}.txt")
}

pub fn bios_version(sys: &Sysfs) -> Result<String, String> {
    env::read_text(&sys.sys.join("class/dmi/id/bios_version"))
}

fn sort_key(path: &Path, host: &str) -> Option<(String, String)> {
    let stem = path.file_name()?.to_str()?.strip_suffix(".txt")?;
    let rest = stem.strip_prefix(host)?.strip_prefix('-')?;
    let (version, date) = rest.rsplit_once('-')?;
    Some((date.to_string(), version.to_string()))
}

pub fn list(dir: &Path, host: &str) -> Result<Vec<PathBuf>, String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    let mut found = entries
        .flatten()
        .map(|entry| entry.path())
        .filter_map(|path| sort_key(&path, host).map(|key| (key, path)))
        .collect::<Vec<_>>();
    found.sort();
    Ok(found.into_iter().map(|(_, path)| path).collect())
}

pub fn latest(dir: &Path, host: &str) -> Result<Option<PathBuf>, String> {
    Ok(list(dir, host)?.pop())
}

pub fn load(path: &Path) -> Result<(String, Export), String> {
    let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = normalize(&decode(&bytes)?);
    let export = parse(&text);
    Ok((text, export))
}

fn keyed(export: &Export) -> Vec<((String, usize), String)> {
    let mut seen = std::collections::HashMap::<&str, usize>::new();
    export
        .settings
        .iter()
        .map(|setting| {
            let count = seen.entry(setting.name.as_str()).or_insert(0);
            *count += 1;
            ((setting.name.clone(), *count), setting.value.clone())
        })
        .collect()
}

pub fn changed(before: &Export, after: &Export) -> Vec<Change> {
    let old = keyed(before);
    let new = keyed(after);
    let old_map = old
        .iter()
        .cloned()
        .collect::<std::collections::HashMap<_, _>>();
    let new_map = new
        .iter()
        .cloned()
        .collect::<std::collections::HashMap<_, _>>();
    let mut changes = Vec::new();
    for ((name, occurrence), value) in &new {
        match old_map.get(&(name.clone(), *occurrence)) {
            Some(previous) if previous == value => {}
            previous => changes.push(Change {
                name: name.clone(),
                occurrence: *occurrence,
                from: previous.cloned(),
                to: Some(value.clone()),
            }),
        }
    }
    for ((name, occurrence), value) in &old {
        if !new_map.contains_key(&(name.clone(), *occurrence)) {
            changes.push(Change {
                name: name.clone(),
                occurrence: *occurrence,
                from: Some(value.clone()),
                to: None,
            });
        }
    }
    changes
}

#[cfg(test)]
#[path = "../../tests/unit/bios/export_tests.rs"]
mod tests;
