//! One formatting session per workspace root.
//!
//! A session caches configuration and validated engines, so it is discarded
//! whenever `dotfmt.dotfile` files or editor settings change.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use dotfmt::app::Session;

pub struct Workspace {
    roots: Mutex<Vec<PathBuf>>,
    sessions: Mutex<HashMap<PathBuf, Arc<Mutex<Session>>>>,
}

impl Workspace {
    pub fn new(roots: Vec<PathBuf>) -> Self {
        Self {
            roots: Mutex::new(normalized(roots)),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn set_roots(&self, roots: Vec<PathBuf>) {
        let mut current = self.roots.lock().unwrap_or_else(|error| error.into_inner());
        *current = normalized(roots);
        self.invalidate();
    }

    pub fn roots(&self) -> Vec<PathBuf> {
        self.roots
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// The nearest workspace root containing `path`, or its directory outside one.
    pub fn root_for(&self, path: &Path) -> PathBuf {
        let roots = self.roots();
        roots
            .iter()
            .filter(|root| path.starts_with(root))
            .max_by_key(|root| root.components().count())
            .cloned()
            .or_else(|| roots.first().cloned())
            .or_else(|| path.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."))
    }

    pub fn session(&self, path: &Path) -> Arc<Mutex<Session>> {
        let root = self.root_for(path);
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        Arc::clone(
            sessions.entry(root.clone()).or_insert_with(|| {
                Arc::new(Mutex::new(Session::with_paths(root, global_config())))
            }),
        )
    }

    /// Drops every cached session; in-flight requests keep their old snapshot.
    pub fn invalidate(&self) {
        self.sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
    }
}

/// The same global configuration precedence as the CLI.
fn global_config() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".config"))
        })?;
    Some(base.join("dotfmt").join("dotfmt.dotfile"))
}

fn normalized(roots: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut roots = roots;
    roots.sort();
    roots.dedup();
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooses_the_nearest_root_and_falls_back_to_the_parent() {
        let workspace =
            Workspace::new(vec![PathBuf::from("/work"), PathBuf::from("/work/project")]);
        assert_eq!(
            workspace.root_for(Path::new("/work/project/src/a.ts")),
            PathBuf::from("/work/project")
        );
        assert_eq!(
            workspace.root_for(Path::new("/work/other/a.ts")),
            PathBuf::from("/work")
        );
        assert_eq!(
            workspace.root_for(Path::new("/elsewhere/a.ts")),
            PathBuf::from("/work")
        );
    }
}
