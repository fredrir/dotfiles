
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::target::Target;

pub enum Outcome {
    Ready(Fetched),
    Refused(i32),
}

pub struct Fetched {
    temp: tempfile::TempDir,
    source: PathBuf,
    branch: Option<String>,
}

impl Fetched {
    pub fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }

    pub fn install(self, destination: &Path) -> Result<(), String> {
        if let Ok(existing) = fs::symlink_metadata(destination) {
            let removed = if existing.is_dir() {
                fs::remove_dir_all(destination)
            } else {
                fs::remove_file(destination)
            };
            removed.map_err(|error| format!("{}: {error}", destination.display()))?;
        }
        fs::rename(&self.source, destination)
            .map_err(|error| format!("{}: {error}", destination.display()))?;
        // Only so the clone is not still around while the rename happens.
        drop(self.temp);
        Ok(())
    }
}

pub fn fetch(target: &Target, beside: &Path) -> Result<Outcome, String> {
    let temp = tempfile::Builder::new()
        .prefix(".gget-")
        .tempdir_in(beside)
        .map_err(|error| format!("{}: {error}", beside.display()))?;
    let clone = temp.path().join("clone");
    let at = text(&clone)?;
    let url = target.url();
    // Without a path it is the repository itself that is wanted, and then a
    // plain shallow clone is both the fewest steps and the whole answer.
    let whole = target.path.is_empty();

    let mut arguments = vec!["clone", "--quiet", "--depth", "1"];
    if !whole {
        arguments.extend(["--filter=blob:none", "--no-checkout"]);
    }
    if let Some(reference) = &target.reference {
        arguments.extend(["--branch", reference]);
    }
    arguments.extend([url.as_str(), at]);
    if let Some(refused) = run(&arguments)? {
        return Ok(refused);
    }

    if !whole {
        let pattern = pattern(&target.path);
        let steps = [
            vec!["-C", at, "sparse-checkout", "set", "--no-cone", &pattern],
            vec!["-C", at, "checkout", "--quiet"],
        ];
        for step in steps {
            if let Some(refused) = run(&step)? {
                return Ok(refused);
            }
        }
    }

    // Read before the repository is taken apart, since this is the only place
    // that knows which branch the server called its default.
    let branch = branch(&clone);
    let source = if whole {
        clone.clone()
    } else {
        clone.join(&target.path)
    };
    if fs::symlink_metadata(&source).is_err() {
        return Err(format!(
            "no {} in {}",
            target.path,
            reported(target, branch.as_deref())
        ));
    }
    if whole {
        // What is wanted is the files, not a repository to work in — and
        // certainly not one shallow clone's worth of history.
        fs::remove_dir_all(clone.join(".git"))
            .map_err(|error| format!("{}: {error}", clone.join(".git").display()))?;
    }
    Ok(Outcome::Ready(Fetched {
        temp,
        source,
        branch,
    }))
}

pub fn reported(target: &Target, branch: Option<&str>) -> String {
    match branch.or(target.reference.as_deref()) {
        Some(branch) => format!("{}@{branch}", target.slug()),
        None => target.slug(),
    }
}

pub fn run(arguments: &[&str]) -> Result<Option<Outcome>, String> {
    let status = Command::new("git")
        .args(arguments)
        .status()
        .map_err(|error| format!("git: {error}"))?;
    Ok(match status.code() {
        Some(0) => None,
        // A step killed by a signal has no status of its own; call it a
        // failure.
        code => Some(Outcome::Refused(code.unwrap_or(1))),
    })
}

fn pattern(path: &str) -> String {
    let mut pattern = String::with_capacity(path.len() + 1);
    pattern.push('/');
    for character in path.chars() {
        if matches!(character, '*' | '?' | '[' | ']' | '\\') {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    pattern
}

pub fn branch(clone: &Path) -> Option<String> {
    let head = fs::read_to_string(clone.join(".git/HEAD")).ok()?;
    let branch = head.trim().strip_prefix("ref: refs/heads/")?;
    (!branch.is_empty()).then(|| branch.to_string())
}

pub fn text(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| format!("{}: not valid UTF-8", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pattern_is_anchored_at_the_root() {
        assert_eq!(pattern("folder_8/folder_9"), "/folder_8/folder_9");
        assert_eq!(pattern("README.md"), "/README.md");
    }

    #[test]
    fn a_pattern_spells_out_what_gitignore_would_read() {
        assert_eq!(pattern("src/[id]/*.rs"), "/src/\\[id\\]/\\*.rs");
    }
}
