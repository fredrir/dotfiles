//! Opening a repository, and the one thing that is still git's job.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::Result;

/// A repository with a working tree, found from the current directory.
pub struct Repo {
    pub(crate) git: gix::Repository,
    root: PathBuf,
}

impl Repo {
    /// The repository the current directory is in.
    pub fn here() -> Result<Repo> {
        let mut git = match gix::discover(".") {
            Ok(git) => git,
            // Not being in a repository is an answer, not a fault, and it is
            // the one every user of these tools will see at some point.
            Err(gix::discover::Error::Discover(_)) => return Err("not a git repository".into()),
            Err(error) => return Err(error.into()),
        };
        // Every changed path is looked up in `HEAD`'s tree, and the trees on
        // the way there are the same few over and over; a cache turns all but
        // the first walk down a directory into a hash lookup.
        git.object_cache_size_if_unset(8 * 1024 * 1024);
        // Discovery answers relative to where it started; every path here is
        // joined onto this one and one of them is printed, so resolve it the
        // way `git rev-parse --show-toplevel` does.
        let root = git
            .workdir()
            .ok_or("a bare repository has no working tree")?;
        let root = gix::path::realpath(root)?;
        Ok(Repo { git, root })
    }

    /// The working tree's root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Whether the index holds anything `HEAD` does not, which is the question
    /// `git diff --cached --quiet` answers.
    ///
    /// git leaves the tree its index would write in the index itself, so the
    /// usual answer is one comparison of two hashes. Only when that cache is
    /// missing or stale — after a merge, or after this crate wrote the index —
    /// does the whole tree have to be compared.
    pub fn index_matches_head(&self) -> Result<bool> {
        let head = self.git.head_tree_id_or_empty()?.detach();
        let index = self.git.index_or_empty()?;
        if let Some(cached) = index.tree()
            && cached.num_entries.is_some()
        {
            return Ok(cached.id == head);
        }
        let mut same = true;
        self.git.tree_index_status(
            &head,
            &index,
            None,
            gix::status::tree_index::TrackRenames::Disabled,
            |_, _, _| {
                same = false;
                Ok::<_, std::convert::Infallible>(std::ops::ControlFlow::Break(()))
            },
        )?;
        Ok(same)
    }
}

/// Hand a step to `git` and wait for it.
///
/// Steps that run hooks, sign, or reach the network stay with git: it is the
/// one that knows the user's configuration and credentials. Its output goes
/// straight to the terminal and its status comes back untouched, so a failing
/// step reports itself and keeps git's vocabulary — 128 for "not a
/// repository", and so on — for whatever is chained after it.
pub fn git(arguments: &[&str]) -> Result<i32> {
    let status = Command::new("git")
        .args(arguments)
        .status()
        .map_err(|error| format!("git {}: {error}", arguments[0]))?;
    // A step killed by a signal has no status of its own; call it a failure.
    Ok(status.code().unwrap_or(1))
}
