use std::fs;
use std::path::Path;

use tempfile::TempDir;

pub fn tree(lines: &[&str]) -> TempDir {
    let root = tempfile::tempdir().unwrap();
    for line in lines {
        let (path, contents) = line.split_once('=').unwrap_or((line, ""));
        entry(root.path(), path, contents);
    }
    root
}

pub fn tree_pairs(entries: &[(&str, &str)]) -> TempDir {
    let root = tempfile::tempdir().unwrap();
    for (path, contents) in entries {
        entry(root.path(), path, contents);
    }
    root
}

pub fn at(root: &TempDir, path: &str) -> String {
    root.path().join(path).display().to_string()
}

#[cfg(unix)]
pub fn executable(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

pub fn names(path: &Path) -> Vec<String> {
    let mut found: Vec<String> = fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().display().to_string())
        .collect();
    found.sort();
    found
}

fn entry(root: &Path, path: &str, contents: &str) {
    let target = root.join(path);
    if path.ends_with('/') {
        fs::create_dir_all(&target).unwrap();
        return;
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&target, contents).unwrap();
}

#[cfg(test)]
#[path = "../tests/unit/tree_tests.rs"]
mod tests;
