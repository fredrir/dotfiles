use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

pub struct Output {
    pub path: PathBuf,
    pub content: Option<Vec<u8>>,
    base: Option<PathBuf>,
}

impl Output {
    pub fn text(path: impl Into<PathBuf>, content: String) -> Self {
        Self {
            path: path.into(),
            content: Some(content.into_bytes()),
            base: None,
        }
    }

    pub fn remove(path: PathBuf) -> Self {
        Self {
            path,
            content: None,
            base: None,
        }
    }

    pub(super) fn in_directory(mut self, directory: PathBuf) -> Self {
        self.base = Some(directory);
        self
    }

    fn destination(&self, root: &Path) -> PathBuf {
        self.base.as_deref().unwrap_or(root).join(&self.path)
    }

    fn label(&self) -> PathBuf {
        self.base
            .as_ref()
            .map_or_else(|| self.path.clone(), |base| base.join(&self.path))
    }
}

struct Change {
    output: Output,
    previous: Option<Vec<u8>>,
}

#[derive(Serialize)]
pub struct ChangeReport {
    pub path: PathBuf,
    pub action: &'static str,
    pub before_bytes: usize,
    pub after_bytes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
}

pub struct Plan {
    changes: Vec<Change>,
}

impl Plan {
    pub fn new(root: &Path, mut outputs: Vec<Output>) -> Result<Self, String> {
        outputs.sort_by(|a, b| a.path.cmp(&b.path));
        let mut seen = BTreeSet::new();
        let mut changes = Vec::new();
        for output in outputs {
            let relative = output.path.to_str().ok_or("non-UTF-8 documentation path")?;
            crate::config::validate_relative(relative)?;
            if relative.is_empty() || !seen.insert(output.destination(root)) {
                return Err(format!("duplicate or empty documentation path: {relative}"));
            }
            let path = output.destination(root);
            validate_path(output.base.as_deref().unwrap_or(root), &output.path)?;
            let previous = match fs::read(&path) {
                Ok(bytes) => Some(bytes),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(format!("read {}: {e}", path.display())),
            };
            if previous != output.content {
                changes.push(Change { output, previous });
            }
        }
        Ok(Self { changes })
    }

    pub fn paths(&self) -> Vec<PathBuf> {
        self.changes
            .iter()
            .map(|change| change.output.label())
            .collect()
    }

    pub fn report(&self, include_diff: bool) -> Vec<ChangeReport> {
        self.changes
            .iter()
            .map(|change| {
                let path = change.output.label();
                let action = if change.previous.is_none() {
                    "create"
                } else if change.output.content.is_none() {
                    "remove"
                } else {
                    "update"
                };
                let diff = include_diff.then(|| {
                    let previous =
                        String::from_utf8_lossy(change.previous.as_deref().unwrap_or_default());
                    let updated = String::from_utf8_lossy(
                        change.output.content.as_deref().unwrap_or_default(),
                    );
                    let before = if change.previous.is_none() {
                        "/dev/null".into()
                    } else {
                        format!("a/{}", path.display())
                    };
                    let after = if change.output.content.is_none() {
                        "/dev/null".into()
                    } else {
                        format!("b/{}", path.display())
                    };
                    similar::TextDiff::from_lines(previous.as_ref(), updated.as_ref())
                        .unified_diff()
                        .context_radius(3)
                        .header(&before, &after)
                        .to_string()
                });
                ChangeReport {
                    path,
                    action,
                    before_bytes: change.previous.as_ref().map_or(0, Vec::len),
                    after_bytes: change.output.content.as_ref().map_or(0, Vec::len),
                    diff,
                }
            })
            .collect()
    }

    pub fn apply(self, root: &Path) -> Result<(), String> {
        if self.changes.is_empty() {
            return Ok(());
        }
        for change in &self.changes {
            let output = &change.output;
            validate_path(output.base.as_deref().unwrap_or(root), &output.path)?;
            let path = output.destination(root);
            let current = match fs::read(&path) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(format!("read {}: {error}", path.display())),
            };
            if current != change.previous {
                return Err(format!(
                    "{}: changed after planning",
                    output.label().display()
                ));
            }
        }
        let mut transaction = crate::fs::transaction::Transaction::for_root(root)?;
        for change in self.changes {
            let output = change.output;
            let path = output.destination(root);
            if let Some(bytes) = output.content {
                transaction.write(&path, &bytes)?;
            } else {
                transaction.discard(&path)?;
            }
        }
        transaction.commit()
    }
}

fn validate_path(root: &Path, relative: &Path) -> Result<(), String> {
    let mut path = root.to_path_buf();
    for part in relative.components() {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_symlink() => {
                return Err(format!("{}: symlink refused", path.display()));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    Ok(())
}

pub(super) fn read(path: &Path) -> Result<String, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("read {}: {e}", path.display())),
    }
}

#[cfg(test)]
#[path = "../../tests/unit/docs/plan_tests.rs"]
mod tests;
