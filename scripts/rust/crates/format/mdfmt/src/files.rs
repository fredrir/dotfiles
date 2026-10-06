use std::path::{Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

#[derive(Clone, Debug)]
pub struct Files {
    root: PathBuf,
    whitelist: Gitignore,
    blacklist: Gitignore,
}

impl Default for Files {
    fn default() -> Self {
        Self {
            root: PathBuf::new(),
            whitelist: Gitignore::empty(),
            blacklist: Gitignore::empty(),
        }
    }
}

impl Files {
    pub(crate) fn build(
        root: &Path,
        whitelist: &GitignoreBuilder,
        blacklist: &GitignoreBuilder,
    ) -> Result<Self, String> {
        Ok(Self {
            root: root.to_path_buf(),
            whitelist: whitelist.build().map_err(|error| error.to_string())?,
            blacklist: blacklist.build().map_err(|error| error.to_string())?,
        })
    }

    pub fn allows(&self, path: &Path) -> bool {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let ancestors = || {
            path.ancestors()
                .take_while(|candidate| *candidate != self.root && candidate.parent().is_some())
                .enumerate()
        };
        // A negated file pattern cannot reopen an excluded parent directory.
        if ancestors().any(|(depth, path)| self.blacklist.matched(path, depth > 0).is_ignore()) {
            return false;
        }
        self.whitelist.is_empty()
            || ancestors()
                .map(|(depth, path)| self.whitelist.matched(path, depth > 0))
                .find(|matched| !matched.is_none())
                .is_some_and(|matched| matched.is_ignore())
    }
}
