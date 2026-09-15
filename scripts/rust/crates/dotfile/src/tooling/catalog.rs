use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Language {
    Rust,
    Python,
    Go,
}

impl Language {
    pub const ALL: [Self; 3] = [Self::Rust, Self::Python, Self::Go];

    /// Stamp file name under `config/sync`.
    pub fn key(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Python => "python",
            Self::Go => "go",
        }
    }

    pub fn source(self) -> &'static str {
        match self {
            Self::Rust => "scripts/rust",
            Self::Python => "scripts/python",
            Self::Go => "scripts/go",
        }
    }

    /// The build tool that must be on PATH to produce this language's binaries.
    pub fn driver(self) -> &'static str {
        match self {
            Self::Rust => "cargo",
            Self::Python => "uv",
            Self::Go => "go",
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.key())
    }
}

pub struct Stage {
    pub language: Language,
    pub binaries: Vec<String>,
    pub inputs: Vec<PathBuf>,
}

pub struct Toolchain {
    pub stages: Vec<Stage>,
}

impl Toolchain {
    /// Reads what each language builds from the manifests that build it, so a new
    /// crate, command, or entry point installs itself without a list to update.
    pub fn read(root: &Path) -> Result<Self, String> {
        let mut stages = Vec::new();
        for language in Language::ALL {
            let binaries = match language {
                Language::Rust => rust_binaries(root)?,
                Language::Python => python_binaries(root)?,
                Language::Go => go_binaries(root)?,
            };
            if binaries.is_empty() {
                continue;
            }
            stages.push(Stage {
                language,
                binaries,
                inputs: inputs(root, language),
            });
        }
        Ok(Self { stages })
    }

    pub fn stage(&self, language: Language) -> Option<&Stage> {
        self.stages
            .iter()
            .find(|stage| stage.language == language)
    }

    pub fn binaries(&self) -> impl Iterator<Item = &str> {
        self.stages
            .iter()
            .flat_map(|stage| stage.binaries.iter().map(String::as_str))
    }
}

fn inputs(root: &Path, language: Language) -> Vec<PathBuf> {
    match language {
        Language::Rust => vec![root.join("scripts/rust"), root.join("shared/tools")],
        Language::Python => vec![
            root.join("scripts/python/pyproject.toml"),
            root.join("scripts/python/uv.lock"),
        ],
        Language::Go => vec![root.join("scripts/go")],
    }
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
struct CrateManifest {
    package: CrateName,
    #[serde(default, rename = "bin")]
    binaries: Vec<BinaryTarget>,
}

#[derive(Deserialize)]
struct CrateName {
    name: String,
}

#[derive(Deserialize)]
struct BinaryTarget {
    name: String,
}

/// A crate's explicit `[[bin]]` targets, or its package name when `src/main.rs`
/// makes it an implicit one; library-only crates contribute nothing.
fn rust_binaries(root: &Path) -> Result<Vec<String>, String> {
    let workspace = root.join("scripts/rust");
    if !workspace.is_dir() {
        return Ok(Vec::new());
    }
    let manifest: WorkspaceManifest = read_toml(&workspace.join("Cargo.toml"))?;
    let mut names = Vec::new();
    for member in manifest.workspace.members {
        let directory = workspace.join(&member);
        let manifest: CrateManifest = read_toml(&directory.join("Cargo.toml"))?;
        if manifest.binaries.is_empty() {
            if directory.join("src/main.rs").is_file() {
                names.push(manifest.package.name);
            }
        } else {
            names.extend(manifest.binaries.into_iter().map(|target| target.name));
        }
    }
    names.sort();
    names.dedup();
    Ok(names)
}

#[derive(Deserialize)]
struct ProjectManifest {
    project: Project,
}

#[derive(Deserialize)]
struct Project {
    #[serde(default)]
    scripts: std::collections::BTreeMap<String, String>,
}

fn python_binaries(root: &Path) -> Result<Vec<String>, String> {
    let manifest = root.join("scripts/python/pyproject.toml");
    if !manifest.is_file() {
        return Ok(Vec::new());
    }
    let manifest: ProjectManifest = read_toml(&manifest)?;
    Ok(manifest.project.scripts.into_keys().collect())
}

/// Go's own convention: one command per `cmd/<name>`, built as `<name>`.
fn go_binaries(root: &Path) -> Result<Vec<String>, String> {
    let commands = root.join("scripts/go/cmd");
    if !commands.is_dir() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for entry in fs::read_dir(&commands).map_err(|e| format!("{}: {e}", commands.display()))? {
        let entry = entry.map_err(|e| format!("{}: {e}", commands.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.path().is_dir() && !name.starts_with(['.', '_']) {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

fn read_toml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let content = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    toml::from_str(&content).map_err(|e| format!("{}: {e}", path.display()))
}
