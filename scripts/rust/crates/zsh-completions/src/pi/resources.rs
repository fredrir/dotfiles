use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::context::Context;
use crate::pi::spec::Pi;

const THEME_DIR: &str = "themes";

#[derive(Debug, Clone, Default)]
pub struct Settings {
    pub default_provider: Option<String>,
    pub default_model: Option<String>,
    pub session_dir: Option<String>,
    pub packages: Vec<PackageSource>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSource {
    pub source: String,
    pub project: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub name: String,
    pub origin: String,
}

impl Settings {
    // Project settings override global ones, as pi merges them.
    pub fn load(ctx: &Context, pi: &Pi) -> Settings {
        let mut settings = Settings::default();
        let [project, global] = pi.settings_files(ctx);
        for (path, is_project) in [(project, true), (global, false)] {
            let Some(value) = read_json(&path) else {
                continue;
            };
            let text = |key: &str| value.get(key).and_then(Value::as_str).map(String::from);
            settings.default_provider = settings
                .default_provider
                .or_else(|| text("defaultProvider"));
            settings.default_model = settings.default_model.or_else(|| text("defaultModel"));
            settings.session_dir = settings.session_dir.or_else(|| text("sessionDir"));
            let Some(Value::Array(packages)) = value.get("packages") else {
                continue;
            };
            for package in packages {
                let source = package
                    .as_str()
                    .or_else(|| package.get("source").and_then(Value::as_str));
                if let Some(source) = source {
                    settings.packages.push(PackageSource {
                        source: source.to_string(),
                        project: is_project,
                    });
                }
            }
        }
        settings
    }
}

pub fn themes(ctx: &Context, pi: &Pi, settings: &Settings) -> Vec<Theme> {
    let mut found: Vec<Theme> = Vec::new();
    let mut add = |dir: &Path, origin: &str| {
        for file in json_files(dir) {
            add_theme(&mut found, &file, origin);
        }
    };
    if let Some(package) = &pi.package {
        add(&package.join("dist/modes/interactive/theme"), "built-in");
    }
    add(&pi.agent_dir.join(THEME_DIR), "user");
    add(&Pi::project_dir(ctx).join(THEME_DIR), "project");
    for package in &settings.packages {
        let Some(dir) = package_dir(ctx, pi, package) else {
            continue;
        };
        let origin = package.source.clone();
        for file in package_themes(&dir) {
            add_theme(&mut found, &file, &origin);
        }
    }
    found
}

fn add_theme(found: &mut Vec<Theme>, file: &Path, origin: &str) {
    let stem = file
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string());
    if stem.as_deref().is_some_and(|stem| stem.contains("schema")) {
        return;
    }
    let name = read_json(file)
        .and_then(|theme| theme.get("name").and_then(Value::as_str).map(String::from))
        .or(stem);
    if let Some(name) = name
        && !found.iter().any(|theme| theme.name == name)
    {
        found.push(Theme {
            name,
            origin: origin.to_string(),
        });
    }
}

// Where an installed package lives: npm packages under pi's npm directory, local paths as written.
pub fn package_dir(ctx: &Context, pi: &Pi, package: &PackageSource) -> Option<PathBuf> {
    let base = if package.project {
        Pi::project_dir(ctx)
    } else {
        pi.agent_dir.clone()
    };
    if let Some(spec) = package.source.strip_prefix("npm:") {
        let name = crate::node::registry::split_version(spec).map_or(spec, |(name, _)| name);
        return Some(base.join("npm/node_modules").join(name));
    }
    let source = package.source.as_str();
    if source.starts_with(['.', '/', '~']) {
        let path = if let Some(rest) = source.strip_prefix("~/") {
            ctx.home.join(rest)
        } else {
            base.join(source)
        };
        return Some(path);
    }
    None
}

fn package_themes(dir: &Path) -> Vec<PathBuf> {
    let declared = read_json(&dir.join("package.json"))
        .and_then(|manifest| manifest.pointer("/pi/themes").cloned());
    let Some(Value::Array(entries)) = declared else {
        return json_files(&dir.join(THEME_DIR));
    };
    let mut files = Vec::new();
    for entry in entries.iter().filter_map(Value::as_str) {
        let path = dir.join(entry.trim_start_matches("./"));
        if path.is_dir() {
            files.extend(json_files(&path));
        } else if let Ok(matches) = glob::glob(&path.to_string_lossy()) {
            files.extend(matches.flatten());
        }
    }
    files
}

fn json_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json" || extension == "jsonc")
        })
        .collect();
    files.sort();
    files
}

// Built-in tool names as pi declares them in its source.
pub fn tools(pi: &Pi) -> Vec<String> {
    pi.package
        .as_ref()
        .and_then(|package| fs::read_to_string(package.join("dist/core/tools/index.js")).ok())
        .map(|source| declared_tools(&source))
        .unwrap_or_default()
}

pub fn declared_tools(source: &str) -> Vec<String> {
    let Some(start) = source.find("allToolNames") else {
        return Vec::new();
    };
    let rest = &source[start..];
    let Some(open) = rest.find('[') else {
        return Vec::new();
    };
    let Some(close) = rest[open..].find(']') else {
        return Vec::new();
    };
    rest[open + 1..open + close]
        .split(',')
        .map(|name| name.trim().trim_matches(['"', '\'', '`']).to_string())
        .filter(|name| crate::help::is_word(name))
        .collect()
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

#[cfg(test)]
#[path = "../../tests/unit/pi/resources_tests.rs"]
mod tests;
