use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use ignore::gitignore::GitignoreBuilder;
use workstation::walk::{Policy, SKIP};

/// The selection rules shared by discovery, `--explain` and `--owns`.
///
/// Skip names are relative to the scan root, so naming a skipped directory
/// explicitly still formats it. `.prettierignore` rules are always relative to
/// the directory that contains the ignore file.
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
        let relative = path.strip_prefix(&self.root).unwrap_or(path);
        relative.components().any(|component| {
            let Component::Normal(name) = component else {
                return false;
            };
            SKIP.iter().any(|skip| name == OsStr::new(skip))
        })
    }

    pub fn ignored(&self, path: &Path) -> bool {
        prettier_ignored(path)
    }

    pub fn allows(&self, path: &Path) -> bool {
        !self.skips(path) && !self.ignored(path)
    }
}

/// Prettier compatibility: honor `.prettierignore` files from the nearest
/// directory upward. Parent-aware matching makes `generated/` exclude files
/// below it; the starts-with guard keeps a relative path from being matched
/// against an unrelated ignore root.
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
