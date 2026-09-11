use crate::Palette;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

const POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemeSource {
    File(PathBuf),
    Fallback,
}

pub fn discover_paths() -> Vec<PathBuf> {
    if let Some(path) = std::env::var_os("DOTFILE_UI_THEME").filter(|value| !value.is_empty()) {
        return vec![PathBuf::from(path)];
    }
    if let Some(root) = std::env::var_os("DOTFILE_ROOT").filter(|value| !value.is_empty()) {
        return vec![PathBuf::from(root).join("shared/ui/theme.json")];
    }
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    let mut paths = Vec::with_capacity(2);
    if let Some(config) = config {
        paths.push(config.join("dotfile/ui/theme.json"));
    }
    if let Some(root) = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(5) {
        paths.push(root.join("shared/ui/theme.json"));
    }
    paths
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    path: PathBuf,
    modified: Option<SystemTime>,
    length: u64,
}

#[derive(Clone, Debug)]
pub struct ThemeHandle {
    paths: Vec<PathBuf>,
    palette: Arc<Palette>,
    source: ThemeSource,
    last_attempt: Option<Stamp>,
    next_poll: Instant,
    error: Option<String>,
}

impl ThemeHandle {
    pub fn discover() -> Self {
        Self::from_paths(discover_paths())
    }

    pub fn from_path(path: impl Into<PathBuf>) -> Self {
        Self::from_paths(vec![path.into()])
    }

    pub fn from_paths(paths: Vec<PathBuf>) -> Self {
        let now = Instant::now();
        let mut handle = Self {
            paths,
            palette: Arc::new(Palette::default()),
            source: ThemeSource::Fallback,
            last_attempt: None,
            next_poll: now,
            error: None,
        };
        handle.poll_at(now);
        handle
    }

    pub fn palette(&self) -> &Arc<Palette> {
        &self.palette
    }

    pub fn source(&self) -> &ThemeSource {
        &self.source
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn poll(&mut self) -> bool {
        self.poll_at(Instant::now())
    }

    pub fn poll_at(&mut self, now: Instant) -> bool {
        if now < self.next_poll {
            return false;
        }
        self.next_poll = now + POLL_INTERVAL;
        let mut failure = None;
        for path in &self.paths {
            let metadata = match std::fs::metadata(path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        failure = Some(format!("{}: {error}", path.display()));
                    }
                    continue;
                }
            };
            let stamp = Stamp {
                path: path.clone(),
                modified: metadata.modified().ok(),
                length: metadata.len(),
            };
            if self.last_attempt.as_ref() == Some(&stamp) {
                return false;
            }
            self.last_attempt = Some(stamp);
            match Palette::from_path(path) {
                Ok(palette) => {
                    self.source = ThemeSource::File(path.clone());
                    self.error = None;
                    if palette == *self.palette {
                        return false;
                    }
                    self.palette = Arc::new(palette);
                    return true;
                }
                Err(error) => {
                    failure = Some(error);
                    // An invalid authoritative file must not silently select another theme.
                    break;
                }
            }
        }
        self.error = failure;
        false
    }
}

#[cfg(test)]
#[path = "../tests/unit/runtime.rs"]
mod tests;
