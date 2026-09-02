use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tempfile::NamedTempFile;

use crate::tui::FavoriteStore;

const VERSION: u64 = 1;

#[derive(Clone)]
pub(crate) struct Favorites {
    path: PathBuf,
    keys: BTreeSet<String>,
    warning: Option<String>,
}

impl FavoriteStore for Favorites {
    fn set_favorite(&mut self, key: &str, favorite: bool) -> Result<(), String> {
        self.set(key, favorite)
    }
}

impl Favorites {
    pub(crate) fn load(home: &Path) -> Self {
        Self::load_from(config_path(home))
    }

    fn load_from(path: PathBuf) -> Self {
        let mut found = Self {
            path,
            keys: BTreeSet::new(),
            warning: None,
        };
        let text = match fs::read_to_string(&found.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return found,
            Err(error) => {
                found.warning = Some(format!(
                    "could not read favorites {}: {error}",
                    found.path.display()
                ));
                return found;
            }
        };
        match parse(&text) {
            Ok(keys) => found.keys = keys,
            Err(error) => {
                found.warning = Some(format!(
                    "could not read favorites {}: {error}",
                    found.path.display()
                ));
            }
        }
        found
    }

    pub(crate) fn contains(&self, key: &str) -> bool {
        self.keys.contains(key)
    }

    pub(crate) fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    pub(crate) fn set(&mut self, key: &str, favorite: bool) -> Result<(), String> {
        if key.is_empty() || key.chars().any(char::is_control) {
            return Err("favorite key is invalid".to_string());
        }
        let previous = self.keys.contains(key);
        if favorite {
            self.keys.insert(key.to_string());
        } else {
            self.keys.remove(key);
        }
        if let Err(error) = self.save() {
            if previous {
                self.keys.insert(key.to_string());
            } else {
                self.keys.remove(key);
            }
            return Err(error);
        }
        Ok(())
    }

    fn save(&mut self) -> Result<(), String> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| "favorites path has no parent directory".to_string())?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
        let document = json!({
            "version": VERSION,
            "sessions": self.keys.iter().collect::<Vec<_>>(),
        });
        let mut temporary = NamedTempFile::new_in(parent)
            .map_err(|error| format!("could not create favorites file: {error}"))?;
        serde_json::to_writer_pretty(temporary.as_file_mut(), &document)
            .map_err(|error| format!("could not encode favorites: {error}"))?;
        temporary
            .as_file_mut()
            .write_all(b"\n")
            .map_err(|error| format!("could not finish favorites: {error}"))?;
        temporary
            .as_file_mut()
            .sync_all()
            .map_err(|error| format!("could not finish favorites: {error}"))?;
        temporary.persist(&self.path).map_err(|error| {
            format!(
                "could not replace favorites {}: {}",
                self.path.display(),
                error.error
            )
        })?;
        self.warning = None;
        Ok(())
    }
}

fn config_path(home: &Path) -> PathBuf {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(value) if !value.is_empty() && Path::new(&value).is_absolute() => PathBuf::from(value),
        _ => home.join(".config"),
    };
    base.join("agent-hop/favorites.json")
}

fn parse(text: &str) -> Result<BTreeSet<String>, String> {
    let value: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
    if value.get("version").and_then(Value::as_u64) != Some(VERSION) {
        return Err("unsupported favorites version".to_string());
    }
    let sessions = value
        .get("sessions")
        .and_then(Value::as_array)
        .ok_or_else(|| "favorites has no session list".to_string())?;
    let mut keys = BTreeSet::new();
    for value in sessions {
        let key = value
            .as_str()
            .ok_or_else(|| "favorite session key is not a string".to_string())?;
        if key.is_empty() || key.chars().any(char::is_control) {
            return Err("favorite session key is invalid".to_string());
        }
        keys.insert(key.to_string());
    }
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn favorites_round_trip_in_stable_order() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("favorites.json");
        let mut favorites = Favorites::load_from(path.clone());
        favorites.set("macie:codex:z", true).unwrap();
        favorites.set("archie:claude:a", true).unwrap();

        let loaded = Favorites::load_from(path);
        assert!(loaded.contains("macie:codex:z"));
        assert!(loaded.contains("archie:claude:a"));
        assert!(loaded.warning().is_none());
    }

    #[test]
    fn removing_a_favorite_is_persistent() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("favorites.json");
        let mut favorites = Favorites::load_from(path.clone());
        favorites.set("macie:codex:id", true).unwrap();
        favorites.set("macie:codex:id", false).unwrap();
        assert!(!Favorites::load_from(path).contains("macie:codex:id"));
    }

    #[test]
    fn an_invalid_file_becomes_a_warning_instead_of_blocking_the_picker() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("favorites.json");
        fs::write(&path, "not json").unwrap();
        let favorites = Favorites::load_from(path);
        assert!(
            favorites
                .warning()
                .unwrap()
                .contains("could not read favorites")
        );
    }

    #[test]
    fn control_characters_are_never_persisted() {
        let directory = tempfile::tempdir().unwrap();
        let mut favorites = Favorites::load_from(directory.path().join("favorites.json"));
        assert!(favorites.set("bad\u{1b}key", true).is_err());
    }

    #[test]
    fn failed_saves_roll_back_in_memory_state() {
        let directory = tempfile::tempdir().unwrap();
        let blocked = directory.path().join("blocked");
        fs::write(&blocked, "not a directory").unwrap();
        let mut favorites = Favorites::load_from(blocked.join("favorites.json"));
        assert!(favorites.set("macie:codex:id", true).is_err());
        assert!(!favorites.contains("macie:codex:id"));
    }
}
