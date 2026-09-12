use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use hostkit::process::{CaptureLimits, output};
use serde::Deserialize;

use super::Language;

pub(super) struct Package {
    pub name: String,
    pub directory: PathBuf,
    pub library: bool,
}

impl Package {
    pub fn matches(&self, target: &str) -> bool {
        self.name == target
            || self
                .directory
                .file_name()
                .is_some_and(|name| name == target)
    }
}

pub(super) struct Catalog {
    pub rust: Vec<Package>,
    pub python: Vec<String>,
    pub files: Vec<PathBuf>,
    pub affected: Option<std::collections::BTreeSet<String>>,
}

#[derive(Deserialize)]
struct WorkspaceManifest {
    workspace: Workspace,
}

#[derive(Deserialize)]
struct Workspace {
    members: Vec<String>,
}

#[derive(Deserialize)]
struct PackageManifest {
    package: PackageName,
    lib: Option<toml::Value>,
}

#[derive(Deserialize)]
struct PackageName {
    name: String,
}

impl Catalog {
    pub fn read(root: &Path, lint: bool) -> Result<Self, String> {
        let manifest: WorkspaceManifest = read_toml(&root.join("scripts/rust/Cargo.toml"))?;
        let rust = manifest
            .workspace
            .members
            .into_iter()
            .map(|member| {
                let directory = Path::new("scripts/rust").join(member);
                let manifest: PackageManifest =
                    read_toml(&root.join(&directory).join("Cargo.toml"))?;
                Ok(Package {
                    name: manifest.package.name,
                    library: root.join(&directory).join("src/lib.rs").is_file()
                        || manifest.lib.is_some(),
                    directory,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut python = Vec::new();
        for entry in
            fs::read_dir(root.join("scripts/python/tests")).map_err(|error| error.to_string())?
        {
            let entry = entry.map_err(|error| error.to_string())?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_dir()
                && !name.starts_with(['.', '_'])
            {
                python.push(name);
            }
        }
        python.sort();
        let files = if lint {
            tracked_files(root)?
        } else {
            Vec::new()
        };
        Ok(Self {
            rust,
            python,
            files,
            affected: None,
        })
    }

    pub fn selected(&self, name: &str) -> bool {
        self.affected
            .as_ref()
            .is_none_or(|names| names.contains(name))
    }

    pub fn selected_file(&self, file: &Path) -> bool {
        if self.affected.is_none() {
            return true;
        }
        if file.starts_with("scripts/python") {
            return self.python.iter().any(|name| self.selected(name));
        }
        self.rust
            .iter()
            .find(|package| file.starts_with(&package.directory))
            .map_or_else(
                || {
                    if file.starts_with("scripts/rust") {
                        self.rust.iter().any(|package| self.selected(&package.name))
                    } else {
                        self.selected(package(file))
                    }
                },
                |package| self.selected(&package.name),
            )
    }

    pub fn known(&self, root: &Path, target: &str, languages: &[Language]) -> bool {
        let accepts = |lang| languages.is_empty() || languages.contains(&lang);
        (accepts(Language::Rust) && self.rust.iter().any(|package| package.matches(target)))
            || (accepts(Language::Python)
                && self
                    .python
                    .iter()
                    .any(|name| super::suites::matches(name, target, self)))
            || (accepts(Language::Javascript)
                && target == "agent-transcripts"
                && root
                    .join("shared/obsidian/plugins/agent-transcripts")
                    .is_dir())
            || (accepts(Language::Lua)
                && target == "wezterm"
                && root.join("shared/wezterm").is_dir())
            || (accepts(Language::Lua)
                && target == "nvim"
                && root.join("shared/nvim/tests/shell.lua").is_file())
            || (accepts(Language::Shell)
                && target == "zsh"
                && root.join("shared/zsh/tests").is_dir())
            || self
                .files
                .iter()
                .any(|file| language(file).is_some_and(accepts) && self.matches(file, target))
    }

    pub fn matches(&self, file: &Path, target: &str) -> bool {
        package(file) == target
            || self
                .rust
                .iter()
                .any(|package| package.matches(target) && file.starts_with(&package.directory))
    }
}

fn read_toml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let content =
        fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    toml::from_str(&content).map_err(|error| format!("{}: {error}", path.display()))
}

fn tracked_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let result = output(
        Command::new("git").current_dir(root).args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--deduplicate",
        ]),
        CaptureLimits {
            stdout: 16 * 1024 * 1024,
            stderr: 64 * 1024,
        },
        Duration::from_secs(10),
    )
    .map_err(|error| format!("git ls-files: {error}"))?;
    if !result.status.success() || result.stdout_truncated {
        return Err(format!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    let mut files = result
        .stdout
        .split(|byte| *byte == 0)
        .filter(|bytes| !bytes.is_empty())
        .map(|bytes| {
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStrExt;
                PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
            }
            #[cfg(not(unix))]
            PathBuf::from(String::from_utf8_lossy(bytes).as_ref())
        })
        .filter(|path| root.join(path).is_file())
        .collect::<Vec<_>>();
    files.sort();
    files.dedup();
    Ok(files)
}

pub(super) fn language(path: &Path) -> Option<Language> {
    if is_zsh(path)
        || path.starts_with(".githooks")
        || path.starts_with("shared/tmux/bin")
        || path.starts_with("shared/tmux/libexec")
    {
        return Some(Language::Shell);
    }
    Some(match path.extension()?.to_str()? {
        "rs" => Language::Rust,
        "py" | "pyi" => Language::Python,
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => Language::Javascript,
        "lua" => Language::Lua,
        "sh" | "bash" | "zsh" => Language::Shell,
        "toml" => Language::Toml,
        "yaml" | "yml" => Language::Yaml,
        "json" | "jsonc" => Language::Json,
        _ => return None,
    })
}

pub(super) fn is_zsh(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "zsh")
        || matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some(".zshrc" | ".zshenv" | ".zprofile" | ".zlogin" | ".zlogout")
        )
}

pub(super) fn package(path: &Path) -> &str {
    let mut components = path.iter().filter_map(|part| part.to_str());
    match components.next() {
        Some("shared") => match components.next() {
            Some("obsidian") if path.starts_with("shared/obsidian/plugins/agent-transcripts") => {
                "agent-transcripts"
            }
            Some(name) => name,
            None => "root",
        },
        Some("macos") => components.next().unwrap_or("root"),
        Some("linux") => components.nth(1).unwrap_or("root"),
        Some("config") => "config",
        _ => "root",
    }
}
