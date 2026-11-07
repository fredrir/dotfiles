use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use ignore::gitignore::GitignoreBuilder;
use workstation::walk::{Policy, SKIP};

#[derive(Clone, Debug)]
pub struct ScanPolicy {
    root: PathBuf,
}

impl ScanPolicy {
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn current() -> Self {
        Self::at(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }

    pub fn walk(&self) -> Policy {
        Policy::new()
    }

    pub fn skips(&self, path: &Path) -> bool {
        let Some(relative) = self.relative(path) else {
            return false;
        };
        relative.components().any(|component| {
            let Component::Normal(name) = component else {
                return false;
            };
            SKIP.iter().any(|skip| name == OsStr::new(skip))
        })
    }

    fn relative(&self, path: &Path) -> Option<PathBuf> {
        if let Ok(relative) = path.strip_prefix(&self.root) {
            return Some(relative.to_path_buf());
        }
        let root = std::fs::canonicalize(&self.root).ok()?;
        let path = std::fs::canonicalize(path).ok()?;
        path.strip_prefix(root).ok().map(Path::to_path_buf)
    }

    pub fn ignored(&self, path: &Path) -> bool {
        prettier_ignored(path)
    }

    pub fn allows(&self, path: &Path) -> bool {
        !self.skips(path) && !self.ignored(path)
    }
}

fn prettier_ignored(path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    let mut directory = Some(parent);
    while let Some(current) = directory {
        let ignore = current.join(".prettierignore");
        if ignore.is_file() && path.starts_with(current) {
            let mut builder = GitignoreBuilder::new(current);
            builder.add(&ignore);
            if let Ok(patterns) = builder.build()
                && patterns
                    .matched_path_or_any_parents(path, false)
                    .is_ignore()
            {
                return true;
            }
        }
        directory = current.parent();
    }
    false
}
