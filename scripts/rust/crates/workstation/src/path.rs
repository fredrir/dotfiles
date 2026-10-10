use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

pub fn home_relative(path: &Path) -> String {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return path.display().to_string();
    };
    home_relative_in(path, &home)
}

pub fn home_relative_in(path: &Path, home: &Path) -> String {
    if !home
        .components()
        .any(|component| matches!(component, Component::Normal(_)))
    {
        return path.display().to_string();
    }
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

pub fn shorten(path: &Path) -> String {
    if let Ok(here) = std::env::current_dir()
        && let Ok(rest) = path.strip_prefix(&here)
        && !rest.as_os_str().is_empty()
    {
        return rest.display().to_string();
    }
    home_relative(path)
}

pub fn require_directory(directory: &Path) -> Result<(), String> {
    match fs::metadata(directory) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(format!("not a directory: {}", directory.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(format!(
            "no such file or directory: {}",
            directory.display()
        )),
        Err(error) => Err(format!("{}: {error}", directory.display())),
    }
}

pub fn hidden(name: &OsStr) -> bool {
    name.as_encoded_bytes().starts_with(b".")
}

pub fn repo_root() -> PathBuf {
    if let Some(root) = std::env::var_os("DOTFILE_ROOT").filter(|v| !v.is_empty()) {
        return PathBuf::from(root);
    }
    let executable = std::env::current_exe().ok();
    let compiled = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for start in executable.iter().chain(std::iter::once(&compiled)) {
        for path in start.ancestors() {
            if path.join("config/targets.dotfile").is_file() || path.join(".git").exists() {
                return path.to_path_buf();
            }
        }
    }
    compiled
        .join("../../../..")
        .canonicalize()
        .unwrap_or(compiled)
}

#[cfg(test)]
#[path = "../tests/unit/path_tests.rs"]
mod tests;
