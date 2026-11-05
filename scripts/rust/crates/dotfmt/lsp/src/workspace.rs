//! One formatting session per workspace root.
//!
//! A session caches configuration and validated engines, so it is discarded
//! whenever `dotfmt.dotfile` files or editor settings change.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
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
    }

    pub fn roots(&self) -> Vec<PathBuf> {
        self.roots
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// The nearest workspace root containing `path`, without a fallback.
    pub fn containing_root(&self, path: &Path) -> Option<PathBuf> {
        let path = normalize(path);
        self.roots()
            .iter()
            .filter(|root| path.starts_with(root))
            .max_by_key(|root| root.components().count())
            .cloned()
    }

    /// The nearest workspace root containing `path`, or its directory outside one.
    pub fn root_for(&self, path: &Path) -> PathBuf {
        let path = normalize(path);
        self.containing_root(&path)
            .or_else(|| {
                path.parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                    .map(Path::to_path_buf)
            })
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

    /// Drops the sessions for one workspace folder and any nested folders.
    pub fn invalidate_root(&self, root: &Path) {
        let root = normalize(root);
        self.sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|session_root, _| !session_root.starts_with(&root));
    }
}

/// The same global configuration precedence as the CLI.
pub fn global_config() -> Option<PathBuf> {
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
    let mut roots: Vec<PathBuf> = roots.iter().map(|root| normalize(root)).collect();
    roots.sort();
    roots.dedup();
    roots
}

/// Canonicalises existing paths, then the nearest existing parent, and
/// otherwise removes `.`/`..` components.
pub(crate) fn normalize(path: &Path) -> PathBuf {
    if let Ok(path) = std::fs::canonicalize(path) {
        return path;
    }
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name())
        && let Ok(parent) = std::fs::canonicalize(parent)
    {
        return parent.join(name);
    }
    lexical(path)
}

fn lexical(path: &Path) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => output.push(prefix.as_os_str()),
            Component::RootDir => output.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !output.pop() {
                    output.push("..");
                }
            }
            Component::Normal(part) => output.push(part),
        }
    }
    output
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
            PathBuf::from("/elsewhere")
        );
    }

    #[test]
    fn invalidating_one_root_keeps_other_sessions() {
        let workspace = Workspace::new(vec![PathBuf::from("/work/a"), PathBuf::from("/work/b")]);
        let a = workspace.session(Path::new("/work/a/src/a.ts"));
        let b = workspace.session(Path::new("/work/b/src/b.ts"));
        workspace.invalidate_root(Path::new("/work/a"));
        assert!(!Arc::ptr_eq(
            &a,
            &workspace.session(Path::new("/work/a/src/a.ts"))
        ));
        assert!(Arc::ptr_eq(
            &b,
            &workspace.session(Path::new("/work/b/src/b.ts"))
        ));
    }

    #[test]
    fn a_containing_root_does_not_fall_back_to_the_parent() {
        let workspace = Workspace::new(vec![PathBuf::from("/work")]);
        assert_eq!(
            workspace.containing_root(Path::new("/work/a.ts")),
            Some(PathBuf::from("/work"))
        );
        assert_eq!(
            workspace.containing_root(Path::new("/elsewhere/a.ts")),
            None
        );
    }

    #[test]
    fn normalises_roots_and_queries_before_containment() {
        let workspace = Workspace::new(vec![PathBuf::from("/work/project/../project/")]);
        assert_eq!(
            workspace.root_for(Path::new("/work/project/./src/../src/a.ts")),
            PathBuf::from("/work/project")
        );
    }

    #[test]
    fn canonicalises_an_existing_root() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("project");
        std::fs::create_dir_all(root.join("src")).unwrap();
        let file = root.join("src").join("a.ts");
        std::fs::write(&file, "").unwrap();
        let canonical = std::fs::canonicalize(&root).unwrap();
        let workspace = Workspace::new(vec![root.join("..").join("project")]);
        assert_eq!(workspace.root_for(&file), canonical);
    }
}
