use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const STATE_LIMIT: usize = 2 * 1024 * 1024;
const HISTORY_LIMIT: usize = 1024 * 1024;
const DETAIL_LIMIT: usize = 32 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OperationRecord {
    pub action: String,
    pub outcome: String,
    pub detail: String,
    pub failed: bool,
    pub timestamp: u64,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedState {
    pub widths: BTreeMap<String, u16>,
    pub operations: Vec<OperationRecord>,
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
        let mut state: Self = serde_json::from_slice(&bytes).unwrap_or_default();
        state.bound_history();
        state
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
        let mut state = self.clone();
        state.bound_history();
        serde_json::to_writer(&mut file, &state)?;
        file.flush()?;
        file.persist(path).map_err(|error| error.error)?;
        Ok(())
    }

    pub fn push(&mut self, record: OperationRecord) {
        self.operations.insert(0, record);
        self.bound_history();
    }

    fn bound_history(&mut self) {
        self.operations.truncate(100);
        let mut size = 0;
        self.operations.retain_mut(|record| {
            truncate_text(&mut record.action, 256);
            truncate_text(&mut record.outcome, 4096);
            truncate_text(&mut record.detail, DETAIL_LIMIT);
            // Serialized size includes JSON escaping of terminal transcripts.
            size += serde_json::to_vec(record).map_or(HISTORY_LIMIT + 1, |bytes| bytes.len());
            size <= HISTORY_LIMIT
        });
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

fn truncate_text(text: &mut String, limit: usize) {
    if text.len() <= limit {
        return;
    }
    let marker = "\n[output truncated]";
    let mut end = limit.saturating_sub(marker.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push_str(marker);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbose_history_and_layout_survive_round_trip_with_bounded_unicode_text() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        let mut state = SavedState::default();
        state.widths.insert("Changes".into(), 47);
        for index in 0..100 {
            state.push(OperationRecord {
                action: format!("Operation {index}"),
                outcome: "Done".into(),
                detail: "\u{1b}λ🦀".repeat(32 * 1024),
                failed: false,
                timestamp: index,
            });
        }
        assert!(state.operations.len() < 100);
        assert_eq!(state.operations[0].action, "Operation 99");
        assert!(
            state
                .operations
                .iter()
                .all(|record| record.detail.len() <= DETAIL_LIMIT)
        );
        state.save_path(&path).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() < STATE_LIMIT as u64);
        let loaded = SavedState::load_path(&path);
        assert_eq!(loaded.widths["Changes"], 47);
        assert_eq!(loaded.operations.len(), state.operations.len());
        assert_eq!(loaded.operations[0].action, "Operation 99");
        assert!(loaded.operations[0].detail.ends_with("[output truncated]"));
    }

    #[test]
    fn malformed_and_oversized_state_use_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        std::fs::write(&path, b"not json").unwrap();
        assert!(SavedState::load_path(&path).operations.is_empty());
        std::fs::File::create(&path)
            .unwrap()
            .set_len(STATE_LIMIT as u64 + 1)
            .unwrap();
        assert!(SavedState::load_path(&path).widths.is_empty());
    }
}
