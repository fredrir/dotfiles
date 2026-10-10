//! Commits read straight from a git directory, without running git.

use std::fs;
use std::path::{Path, PathBuf};

/// The git directory of the checkout holding `path`; `None` outside one, or
/// when the nearest `.git` is a file, as in a linked worktree or submodule.
pub fn dir(path: &Path) -> Option<PathBuf> {
    let git = path
        .ancestors()
        .map(|dir| dir.join(".git"))
        .find(|git| fs::symlink_metadata(git).is_ok())?;
    git.is_dir().then_some(git)
}

/// The commit `HEAD` names in the git directory `git`.
pub fn revision(git: &Path) -> Option<String> {
    let head = fs::read_to_string(git.join("HEAD")).ok()?;
    let head = head.trim();
    let Some(reference) = head.strip_prefix("ref: ") else {
        return Some(head.to_string());
    };
    if let Ok(commit) = fs::read_to_string(git.join(reference)) {
        return Some(commit.trim().to_string());
    }
    fs::read_to_string(git.join("packed-refs"))
        .ok()?
        .lines()
        .find_map(|line| {
            line.split_once(' ')
                .filter(|(_, name)| *name == reference)
                .map(|(commit, _)| commit.to_string())
        })
}

#[cfg(test)]
#[path = "../tests/unit/git_tests.rs"]
mod tests;
