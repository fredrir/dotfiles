use std::fs;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::Value;

use crate::context::Context;
use crate::node::manager::Manager;
use crate::node::npmrc::Npmrc;

const DEPENDENCY_KINDS: [(&str, &str); 4] = [
    ("dependencies", ""),
    ("devDependencies", "dev"),
    ("optionalDependencies", "optional"),
    ("peerDependencies", "peer"),
];

#[derive(Debug, Default, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub scripts: IndexMap<String, Value>,
    #[serde(default)]
    pub workspaces: Option<Workspaces>,
    #[serde(flatten)]
    rest: IndexMap<String, Value>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Workspaces {
    List(Vec<String>),
    Object {
        #[serde(default)]
        packages: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    pub name: String,
    pub range: String,
    pub kind: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub name: String,
    pub path: String,
}

#[derive(Deserialize)]
struct PnpmWorkspace {
    #[serde(default)]
    packages: Vec<String>,
}

impl Manifest {
    pub fn read(path: &Path) -> Option<Manifest> {
        let bytes = fs::read(path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    pub fn dependencies(&self) -> Vec<Dependency> {
        let mut found: Vec<Dependency> = Vec::new();
        for (field, kind) in DEPENDENCY_KINDS {
            let Some(Value::Object(map)) = self.rest.get(field) else {
                continue;
            };
            for (name, range) in map {
                if found.iter().any(|known| &known.name == name) {
                    continue;
                }
                found.push(Dependency {
                    name: name.clone(),
                    range: range.as_str().unwrap_or("").to_string(),
                    kind,
                });
            }
        }
        found
    }

    pub fn scripts(&self) -> Vec<(String, String)> {
        self.scripts
            .iter()
            .map(|(name, command)| (name.clone(), command.as_str().unwrap_or("").to_string()))
            .collect()
    }

    fn workspace_patterns(&self) -> Vec<String> {
        match &self.workspaces {
            Some(Workspaces::List(list)) => list.clone(),
            Some(Workspaces::Object { packages }) => packages.clone(),
            None => Vec::new(),
        }
    }
}

pub fn nearest(ctx: &Context) -> Option<Manifest> {
    ctx.ancestors()
        .find_map(|dir| Manifest::read(&dir.join("package.json")))
}

pub fn workspaces(ctx: &Context) -> Vec<Workspace> {
    for dir in ctx.ancestors() {
        let patterns = workspace_patterns(dir);
        if !patterns.is_empty() {
            return expand_workspaces(dir, &patterns);
        }
    }
    Vec::new()
}

fn workspace_patterns(dir: &Path) -> Vec<String> {
    if let Ok(text) = fs::read_to_string(dir.join("pnpm-workspace.yaml"))
        && let Ok(workspace) = serde_saphyr::from_str::<PnpmWorkspace>(&text)
    {
        return workspace.packages;
    }
    Manifest::read(&dir.join("package.json"))
        .map(|manifest| manifest.workspace_patterns())
        .unwrap_or_default()
}

fn expand_workspaces(root: &Path, patterns: &[String]) -> Vec<Workspace> {
    let excluded: Vec<glob::Pattern> = patterns
        .iter()
        .filter_map(|pattern| pattern.strip_prefix('!'))
        .filter_map(|pattern| glob::Pattern::new(pattern.trim_end_matches('/')).ok())
        .collect();
    let mut found: Vec<Workspace> = Vec::new();
    for pattern in patterns.iter().filter(|pattern| !pattern.starts_with('!')) {
        let pattern = pattern.trim_start_matches("./").trim_end_matches('/');
        let full = root.join(pattern).join("package.json");
        let Ok(matches) = glob::glob(&full.to_string_lossy()) else {
            continue;
        };
        for manifest_path in matches.flatten() {
            let Some(dir) = manifest_path.parent() else {
                continue;
            };
            let Ok(relative) = dir.strip_prefix(root) else {
                continue;
            };
            let relative = relative.to_string_lossy().to_string();
            if relative.split('/').any(|part| part == "node_modules")
                || excluded.iter().any(|pattern| pattern.matches(&relative))
            {
                continue;
            }
            let Some(name) = Manifest::read(&manifest_path).and_then(|manifest| manifest.name)
            else {
                continue;
            };
            if !found.iter().any(|known| known.name == name) {
                found.push(Workspace {
                    name,
                    path: relative,
                });
            }
        }
    }
    found
}

pub fn local_binaries(ctx: &Context) -> Vec<String> {
    let mut found = Vec::new();
    for dir in ctx.ancestors() {
        let Ok(entries) = fs::read_dir(dir.join("node_modules/.bin")) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with('.') && !found.contains(&name) {
                found.push(name);
            }
        }
    }
    found.sort();
    found
}

pub fn global_packages(ctx: &Context, manager: Manager) -> Vec<Dependency> {
    match manager {
        Manager::Npm => npm_global_root(ctx)
            .map(|root| installed_in(&root))
            .unwrap_or_default(),
        Manager::Pnpm => pnpm_home(ctx)
            .map(|home| {
                let pattern = home.join("global/*/package.json");
                glob::glob(&pattern.to_string_lossy())
                    .map(|paths| {
                        paths
                            .flatten()
                            .filter_map(|path| Manifest::read(&path))
                            .flat_map(|manifest| manifest.dependencies())
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default(),
        Manager::Yarn => {
            let config = ctx
                .var_path("XDG_CONFIG_HOME")
                .unwrap_or_else(|| ctx.home.join(".config"));
            Manifest::read(&config.join("yarn/global/package.json"))
                .map(|manifest| manifest.dependencies())
                .unwrap_or_default()
        }
        Manager::Bun => {
            let root = ctx
                .var_path("BUN_INSTALL")
                .unwrap_or_else(|| ctx.home.join(".bun"));
            Manifest::read(&root.join("install/global/package.json"))
                .map(|manifest| manifest.dependencies())
                .unwrap_or_default()
        }
    }
}

// Where `npm root -g` points, without starting node.
pub fn npm_global_root(ctx: &Context) -> Option<PathBuf> {
    let prefix = Npmrc::load(ctx)
        .get("prefix")
        .map(|prefix| ctx.expand(&prefix))
        .or_else(|| {
            let node = fs::canonicalize(ctx.which("node")?).ok()?;
            Some(node.parent()?.parent()?.to_path_buf())
        })?;
    Some(prefix.join("lib/node_modules"))
}

fn pnpm_home(ctx: &Context) -> Option<PathBuf> {
    ctx.var_path("PNPM_HOME").or_else(|| {
        let linux = ctx
            .var_path("XDG_DATA_HOME")
            .unwrap_or_else(|| ctx.home.join(".local/share"))
            .join("pnpm");
        let macos = ctx.home.join("Library/pnpm");
        [linux, macos].into_iter().find(|dir| dir.is_dir())
    })
}

fn installed_in(root: &Path) -> Vec<Dependency> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return found;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if name.starts_with('@') {
            let Ok(scoped) = fs::read_dir(entry.path()) else {
                continue;
            };
            for inner in scoped.flatten() {
                let full = format!("{name}/{}", inner.file_name().to_string_lossy());
                found.push(installed(&inner.path(), full));
            }
        } else {
            found.push(installed(&entry.path(), name));
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

fn installed(dir: &Path, name: String) -> Dependency {
    let version = Manifest::read(&dir.join("package.json"))
        .and_then(|manifest| manifest.version)
        .unwrap_or_default();
    Dependency {
        name,
        range: version,
        kind: "global",
    }
}

#[cfg(test)]
#[path = "../../tests/unit/node/project_tests.rs"]
mod tests;
