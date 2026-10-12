use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tempfile::TempDir;

use super::install::{BUILD_TIMEOUT, run};
use crate::context::Context;
use crate::process::CaptureLimits;

// A module's patch is `patches/<module path>.patch`: a git diff relative to the module root.
const PATCHES: &str = "patches";

pub struct Patched {
    directory: TempDir,
}

impl Patched {
    pub fn modfile(&self) -> PathBuf {
        self.directory.path().join("go.mod")
    }
}

/// The module cache is read-only, so each patched module is replaced with a patched copy.
pub fn prepare(context: &Context, source: &Path) -> Result<Option<Patched>, String> {
    let patches = patches(&source.join(PATCHES))?;
    if patches.is_empty() {
        return Ok(None);
    }
    let directory = TempDir::new().map_err(|e| format!("create temporary directory: {e}"))?;
    let mut modfile = read(&source.join("go.mod"))?;
    for (module, patch) in &patches {
        let copy = directory.path().join("modules").join(module);
        copy_tree(&module_dir(context, source, module)?, &copy)?;
        run(
            context
                .command("git")
                .arg("-C")
                .arg(&copy)
                .arg("apply")
                .arg(patch)
                // Without a ceiling, a temporary directory inside a repository would make git apply paths from its root.
                .env("GIT_CEILING_DIRECTORIES", directory.path()),
            &format!("apply {}", patch.display()),
        )?;
        modfile.push_str(&format!("\nreplace {module} => {}\n", copy.display()));
    }
    write(&directory.path().join("go.mod"), &modfile)?;
    let sum = source.join("go.sum");
    if sum.is_file() {
        write(&directory.path().join("go.sum"), &read(&sum)?)?;
    }
    Ok(Some(Patched { directory }))
}

pub fn modules(source: &Path) -> Result<Vec<String>, String> {
    Ok(patches(&source.join(PATCHES))?
        .into_iter()
        .map(|(module, _)| module)
        .collect())
}

pub fn patch_path(source: &Path, module: &str) -> PathBuf {
    source.join(PATCHES).join(format!("{module}.patch"))
}

fn patches(directory: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let mut found = Vec::new();
    if directory.is_dir() {
        collect(directory, directory, &mut found)?;
    }
    found.sort();
    Ok(found)
}

fn collect(
    root: &Path,
    directory: &Path,
    found: &mut Vec<(String, PathBuf)>,
) -> Result<(), String> {
    for entry in
        fs::read_dir(directory).map_err(|e| format!("read {}: {e}", directory.display()))?
    {
        let path = entry
            .map_err(|e| format!("read {}: {e}", directory.display()))?
            .path();
        if path.is_dir() {
            collect(root, &path, found)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "patch")
        {
            let relative = path.with_extension("");
            let module = relative
                .strip_prefix(root)
                .map_err(|e| format!("{}: {e}", path.display()))?
                .components()
                .map(|part| part.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            found.push((module, path));
        }
    }
    Ok(())
}

fn module_dir(context: &Context, source: &Path, module: &str) -> Result<PathBuf, String> {
    let output = crate::process::output(
        context
            .command("go")
            .current_dir(source)
            .args(["mod", "download", "-json", module])
            .stdin(Stdio::null()),
        CaptureLimits::default(),
        BUILD_TIMEOUT,
    )
    .map_err(|e| format!("go mod download {module}: {e}"))?;
    let download: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("go mod download {module}: {e}"))?;
    match download["Dir"].as_str() {
        Some(dir) if output.status.success() => Ok(PathBuf::from(dir)),
        _ => Err(format!(
            "go mod download {module}: {}",
            download["Error"].as_str().unwrap_or("no module directory")
        )),
    }
}

// Written rather than copied, so the files lose the module cache's read-only mode.
fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| format!("create {}: {e}", to.display()))?;
    for entry in fs::read_dir(from).map_err(|e| format!("read {}: {e}", from.display()))? {
        let entry = entry.map_err(|e| format!("read {}: {e}", from.display()))?;
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            let bytes = fs::read(entry.path())
                .map_err(|e| format!("read {}: {e}", entry.path().display()))?;
            fs::write(&target, bytes).map_err(|e| format!("write {}: {e}", target.display()))?;
        }
    }
    Ok(())
}

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))
}

fn write(path: &Path, contents: &str) -> Result<(), String> {
    fs::write(path, contents).map_err(|e| format!("write {}: {e}", path.display()))
}
