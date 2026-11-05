//! One formatting session per workspace root.
//!
//! A session caches configuration and validated engines, so it is discarded
//! whenever `dotfmt.dotfile` files or editor settings change.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use dotfmt::app::Session;

use crate::settings::SettingsStore;

pub struct Workspace {
    roots: Mutex<Vec<PathBuf>>,
    sessions: Mutex<HashMap<PathBuf, Arc<Mutex<Session>>>>,
    /// When present, sessions apply the native `dotfmt.format` overlay of their root.
    settings: Option<Arc<SettingsStore>>,
    /// The client-resolved global config path; falls back to the process environment.
    global: Option<PathBuf>,
}

impl Workspace {
    pub fn new(roots: Vec<PathBuf>) -> Self {
        Self {
            roots: Mutex::new(normalized(roots)),
            sessions: Mutex::new(HashMap::new()),
            settings: None,
            global: global_config(),
        }
    }

    /// Creates sessions that apply the settings store's native overlay.
    pub fn with_settings(
        roots: Vec<PathBuf>,
        settings: Arc<SettingsStore>,
        global: Option<PathBuf>,
    ) -> Self {
        Self {
            roots: Mutex::new(normalized(roots)),
            sessions: Mutex::new(HashMap::new()),
            settings: Some(settings),
            global: global.or_else(global_config),
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
        nearest_root(&self.roots(), &path).cloned()
    }

    /// Whether `path` belongs to `root` as its *nearest* workspace root.
    ///
    /// A path inside a nested workspace root never belongs to the parent root.
    /// Removed workspace folders still own their old subtree so cached state
    /// can be dropped, but a nested root that remains registered is never
    /// claimed by its former parent.
    pub fn in_root(&self, path: &Path, root: &Path) -> bool {
        belongs_to(&self.roots(), &normalize(path), &normalize(root))
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
        // Resolve the overlay under the sessions lock: a concurrent settings
        // change invalidates this map, and an overlay read before that
        // invalidation must not be inserted afterwards.
        let overlay = self
            .settings
            .as_ref()
            .and_then(|settings| settings.overlay_for(&root))
            .map(|overlay| (*overlay).clone());
        let global = self.global.clone();
        Arc::clone(sessions.entry(root.clone()).or_insert_with(move || {
            Arc::new(Mutex::new(Session::with_paths_overlay(
                root, global, overlay,
            )))
        }))
    }

    /// Drops every cached session; in-flight requests keep their old snapshot.
    pub fn invalidate(&self) {
        self.sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
    }

    /// Drops the sessions whose nearest workspace root is exactly `root`.
    ///
    /// Sessions for nested workspace roots keep their cached configuration.
    pub fn invalidate_root(&self, root: &Path) {
        let root = normalize(root);
        let roots = self.roots();
        self.sessions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|session_root, _| !belongs_to(&roots, session_root, &root));
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

fn nearest_root<'a>(roots: &'a [PathBuf], path: &Path) -> Option<&'a PathBuf> {
    roots
        .iter()
        .filter(|root| path.starts_with(root))
        .max_by_key(|root| root.components().count())
}

fn belongs_to(roots: &[PathBuf], path: &Path, root: &Path) -> bool {
    let nearest = nearest_root(roots, path);
    if roots.iter().any(|candidate| candidate == root) {
        return nearest.is_some_and(|nearest| nearest == root);
    }
    path.starts_with(root) && nearest.is_none_or(|nearest| !nearest.starts_with(root))
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

pub(crate) fn lexical(path: &Path) -> PathBuf {
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
    fn invalidating_a_parent_root_keeps_a_nested_root_session() {
        let workspace = Workspace::new(vec![PathBuf::from("/work"), PathBuf::from("/work/app")]);
        let outer = workspace.session(Path::new("/work/src/a.ts"));
        let nested = workspace.session(Path::new("/work/app/src/a.ts"));
        workspace.invalidate_root(Path::new("/work"));
        assert!(!Arc::ptr_eq(
            &outer,
            &workspace.session(Path::new("/work/src/a.ts"))
        ));
        assert!(Arc::ptr_eq(
            &nested,
            &workspace.session(Path::new("/work/app/src/a.ts"))
        ));
    }

    #[test]
    fn in_root_uses_the_nearest_registered_root() {
        let workspace = Workspace::new(vec![PathBuf::from("/work"), PathBuf::from("/work/app")]);
        assert!(workspace.in_root(Path::new("/work/src/a.ts"), Path::new("/work")));
        assert!(!workspace.in_root(Path::new("/work/app/src/a.ts"), Path::new("/work")));
        assert!(workspace.in_root(Path::new("/work/app/src/a.ts"), Path::new("/work/app")));
        assert!(!workspace.in_root(Path::new("/work/src/a.ts"), Path::new("/work/app")));
    }

    #[test]
    fn a_removed_parent_root_does_not_claim_a_remaining_nested_root() {
        let workspace = Workspace::new(vec![PathBuf::from("/work/app")]);
        assert!(workspace.in_root(Path::new("/work/src/a.ts"), Path::new("/work")));
        assert!(!workspace.in_root(Path::new("/work/app/src/a.ts"), Path::new("/work")));
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
