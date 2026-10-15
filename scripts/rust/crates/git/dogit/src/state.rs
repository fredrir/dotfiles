use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const STATE_LIMIT: usize = 2 * 1024 * 1024;
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedState {
    pub widths: BTreeMap<String, u16>,
}

impl SavedState {
    pub fn load(root: &Path) -> Self {
        Self::path(root).map_or_else(Self::default, |path| Self::load_path(&path))
    }

    fn load_path(path: &Path) -> Self {
        let mut bytes = Vec::new();
        let result = std::fs::File::open(path)
            .and_then(|file| file.take(STATE_LIMIT as u64 + 1).read_to_end(&mut bytes));
        if result.is_err() || bytes.len() > STATE_LIMIT {
            return Self::default();
        }
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    pub fn save(&self, root: &Path) -> std::io::Result<()> {
        let Some(path) = Self::path(root) else {
            return Ok(());
        };
        self.save_path(&path)
    }

    fn save_path(&self, path: &Path) -> std::io::Result<()> {
        use std::io::Write;
        let parent = path
            .parent()
            .ok_or_else(|| std::io::Error::other("state directory missing"))?;
        std::fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer(&mut file, self)?;
        file.flush()?;
        file.persist(path).map_err(|error| error.error)?;
        Ok(())
    }

    fn path(root: &Path) -> Option<PathBuf> {
        let base = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
            })?;
        let key = Sha256::digest(root.as_os_str().as_encoded_bytes());
        Some(base.join("dogit").join(format!("{key:x}.json")))
    }
}
