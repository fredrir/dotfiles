use std::cell::Cell;
use std::fs;
use std::path::Path;

use crate::decision::{Choice, Client, Prompt, Subject};

const PREVIEW_BYTES: usize = 128 * 1024;
const PREVIEW_ENTRIES: usize = 64;

pub struct Request<'a> {
    pub path: &'a Path,
    pub detail: String,
    pub repo: Option<String>,
    pub live: Option<String>,
    pub index: usize,
    pub total: usize,
}

/// Asks once per blocked path and remembers "all" and "skip" for the rest.
pub struct Consent<'a> {
    decisions: Option<&'a Client>,
    subject: Subject,
    remembered: Cell<Option<bool>>,
}

impl<'a> Consent<'a> {
    pub fn new(decisions: &'a Client, subject: Subject, enabled: bool) -> Self {
        Self {
            decisions: (enabled && decisions.promptable()).then_some(decisions),
            subject,
            remembered: Cell::new(None),
        }
    }

    pub fn settled(subject: Subject, answer: bool) -> Self {
        Self {
            decisions: None,
            subject,
            remembered: Cell::new(Some(answer)),
        }
    }

    pub fn interactive(&self) -> bool {
        self.decisions.is_some()
    }

    pub fn ask(&self, request: Request<'_>) -> Result<bool, String> {
        if let Some(remembered) = self.remembered.get() {
            return Ok(remembered);
        }
        let Some(decisions) = self.decisions else {
            return Ok(false);
        };
        let choice = decisions.choose(Prompt::Overwrite {
            subject: self.subject,
            path: request.path.to_path_buf(),
            detail: request.detail,
            repo: request.repo,
            live: request.live,
            index: request.index,
            total: request.total,
        })?;
        if let Some(remembered) = Prompt::batched(choice) {
            self.remembered.set(Some(remembered));
            return Ok(remembered);
        }
        match choice {
            Choice::Overwrite => Ok(true),
            Choice::Keep => Ok(false),
            _ => Err("invalid overwrite decision".to_string()),
        }
    }
}

pub fn preview(path: &Path) -> Option<String> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if metadata.file_type().is_symlink() {
        return Some(format!("→ {}", fs::read_link(path).ok()?.display()));
    }
    if metadata.is_dir() {
        return Some(directory_preview(path));
    }
    if metadata.len() as usize > PREVIEW_BYTES {
        return Some(format!("{} bytes", metadata.len()));
    }
    fs::read(path)
        .ok()
        .map(|bytes| match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(error) => format!("binary, {} bytes", error.into_bytes().len()),
        })
}

pub fn describe(path: &Path) -> String {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => "unmanaged symlink".to_string(),
        Ok(metadata) if metadata.is_dir() => "unmanaged directory".to_string(),
        Ok(_) => "unmanaged file".to_string(),
        Err(_) => "unmanaged path".to_string(),
    }
}

fn directory_preview(path: &Path) -> String {
    let Ok(entries) = fs::read_dir(path) else {
        return "unreadable directory".to_string();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if entry.path().is_dir() {
                format!("{name}/")
            } else {
                name
            }
        })
        .collect();
    names.sort();
    let total = names.len();
    names.truncate(PREVIEW_ENTRIES);
    if total > PREVIEW_ENTRIES {
        names.push(format!("… {} more", total - PREVIEW_ENTRIES));
    }
    names.join("\n")
}
