//! Which package manager installs a project, and where to run it.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const AGENTS: &[&str] = &["npm", "pnpm", "yarn", "bun", "deno"];

/// Checked in order; mirrors `package-manager-detector`.
const LOCKFILES: &[(&str, &str)] = &[
    ("bun.lock", "bun"),
    ("bun.lockb", "bun"),
    ("deno.lock", "deno"),
    ("pnpm-lock.yaml", "pnpm"),
    ("pnpm-workspace.yaml", "pnpm"),
    ("yarn.lock", "yarn"),
    ("package-lock.json", "npm"),
    ("npm-shrinkwrap.json", "npm"),
];

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Params {
    /// Directory to start looking from.
    pub path: PathBuf,
    /// Package manager picked by the user when none was detected.
    #[serde(default)]
    pub agent: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub cwd: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<[&'static str; 2]>,
    /// Package managers to pick from when `command` is missing.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub choices: &'static [&'static str],
}

impl Plan {
    fn install(cwd: PathBuf, agent: &'static str) -> Self {
        Self {
            cwd,
            command: Some([agent, "install"]),
            choices: &[],
        }
    }

    fn choose(cwd: PathBuf) -> Self {
        Self {
            cwd,
            command: None,
            choices: AGENTS,
        }
    }
}

/// Runs at the closest lockfile or `packageManager` root, else asks at the nearest `package.json`.
pub fn plan(params: &Params) -> Option<Plan> {
    if let Some(agent) = &params.agent {
        let agent = known(agent)?;
        return nearest_manifest(&params.path).map(|cwd| Plan::install(cwd, agent));
    }
    params
        .path
        .ancestors()
        .find_map(|dir| detect(dir).map(|agent| Plan::install(dir.to_path_buf(), agent)))
        .or_else(|| nearest_manifest(&params.path).map(Plan::choose))
}

fn detect(dir: &Path) -> Option<&'static str> {
    let declared = declared(&dir.join("package.json"));
    LOCKFILES
        .iter()
        .find(|(lockfile, _)| dir.join(lockfile).is_file())
        .map(|&(_, agent)| declared.unwrap_or(agent))
        .or(declared)
}

/// The `packageManager` field, falling back to `devEngines.packageManager.name`.
fn declared(manifest: &Path) -> Option<&'static str> {
    let manifest: Value = serde_json::from_slice(&fs::read(manifest).ok()?).ok()?;
    let name = match manifest.get("packageManager").and_then(Value::as_str) {
        Some(spec) => spec.trim_start_matches('^').split('@').next()?,
        None => manifest
            .pointer("/devEngines/packageManager/name")?
            .as_str()?,
    };
    known(name)
}

fn known(name: &str) -> Option<&'static str> {
    AGENTS.iter().copied().find(|agent| *agent == name)
}

fn nearest_manifest(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join("package.json").is_file())
        .map(Path::to_path_buf)
}
