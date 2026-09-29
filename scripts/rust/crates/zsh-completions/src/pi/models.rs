use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cache::{self, Source};
use crate::context::{Context, fingerprint};
use crate::pi::spec::Pi;
use crate::process;

const LIST_TIMEOUT: Duration = Duration::from_secs(15);
// Levels a model only supports when its catalog entry maps them explicitly.
const EXPLICIT_LEVELS: [&str; 2] = ["xhigh", "max"];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Model {
    pub provider: String,
    pub id: String,
    pub name: String,
    pub reasoning: bool,
    // Explicit per-level mapping from the catalog; a `None` value marks an unsupported level.
    pub levels: Option<HashMap<String, Option<String>>>,
}

#[derive(Debug, Default)]
struct CatalogEntry {
    name: String,
    reasoning: bool,
    levels: Option<HashMap<String, Option<String>>>,
}

impl Model {
    pub fn thinking_levels(&self, all: &[String]) -> Vec<String> {
        if !self.reasoning {
            return all
                .iter()
                .filter(|level| *level == "off")
                .cloned()
                .collect();
        }
        all.iter()
            .filter(|level| {
                let mapped = self
                    .levels
                    .as_ref()
                    .and_then(|levels| levels.get(level.as_str()));
                match mapped {
                    Some(None) => false,
                    Some(Some(_)) => true,
                    None => !EXPLICIT_LEVELS.contains(&level.as_str()),
                }
            })
            .cloned()
            .collect()
    }
}

pub struct ModelsSource<'a> {
    pub pi: &'a Pi,
    pub binary: PathBuf,
}

impl Source for ModelsSource<'_> {
    type Value = Vec<Model>;

    fn key(&self) -> String {
        "pi-models".into()
    }

    // Which models are listed depends on the catalog, custom models, and stored credentials.
    fn stamp(&self, _ctx: &Context) -> String {
        [
            self.binary.clone(),
            self.pi.agent_dir.join("models.json"),
            self.pi.agent_dir.join("models-store.json"),
            self.pi.agent_dir.join("auth.json"),
        ]
        .iter()
        .map(|path| fingerprint(path))
        .collect::<Vec<_>>()
        .join("|")
    }

    fn ttl(&self) -> Option<Duration> {
        Some(Duration::from_secs(24 * 3600))
    }

    fn job(&self) -> Vec<String> {
        vec!["pi-models".into()]
    }

    fn build(&self, ctx: &Context) -> Option<Vec<Model>> {
        let text = process::output(
            &self.binary,
            &["--offline", "--list-models"],
            &ctx.cwd,
            LIST_TIMEOUT,
        )?;
        let catalog = catalog(self.pi);
        let models = parse_list(&text)
            .into_iter()
            .map(|(provider, id, reasoning)| {
                let entry = catalog.get(&(provider.clone(), id.clone()));
                Model {
                    name: entry.map(|entry| entry.name.clone()).unwrap_or_default(),
                    reasoning: reasoning
                        .or(entry.map(|entry| entry.reasoning))
                        .unwrap_or(true),
                    levels: entry.and_then(|entry| entry.levels.clone()),
                    provider,
                    id,
                }
            })
            .collect();
        Some(models)
    }
}

pub fn load(ctx: &Context, pi: &Pi) -> Vec<Model> {
    pi.binary
        .clone()
        .and_then(|binary| cache::load(ctx, &ModelsSource { pi, binary }))
        .unwrap_or_default()
}

// Rows of `pi --list-models`: provider, model, and whether it thinks when that column exists.
pub fn parse_list(text: &str) -> Vec<(String, String, Option<bool>)> {
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let Some(header) = lines.next() else {
        return Vec::new();
    };
    let columns: Vec<&str> = header.split_whitespace().collect();
    let position = |name: &str| columns.iter().position(|column| *column == name);
    let (Some(provider), Some(model)) = (position("provider"), position("model")) else {
        return Vec::new();
    };
    let thinking = position("thinking");
    lines
        .filter_map(|line| {
            let cells: Vec<&str> = line.split_whitespace().collect();
            let reasoning = thinking
                .and_then(|index| cells.get(index))
                .map(|cell| *cell == "yes");
            Some((
                cells.get(provider)?.to_string(),
                cells.get(model)?.to_string(),
                reasoning,
            ))
        })
        .collect()
}

fn catalog(pi: &Pi) -> HashMap<(String, String), CatalogEntry> {
    let mut catalog = HashMap::new();
    for dir in catalog_dirs(pi) {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Some(Value::Object(apis)) = read_json(&entry.path()) else {
                continue;
            };
            for models in apis.values() {
                let Value::Object(models) = models else {
                    continue;
                };
                for model in models.values() {
                    add_model(&mut catalog, model, None);
                }
            }
        }
    }
    let custom = [
        (pi.agent_dir.join("models.json"), true),
        (pi.agent_dir.join("models-store.json"), false),
    ];
    for (path, nested) in custom {
        let Some(root) = read_json(&path) else {
            continue;
        };
        let providers = if nested {
            root.get("providers").cloned()
        } else {
            Some(root)
        };
        let Some(Value::Object(providers)) = providers else {
            continue;
        };
        for (provider, config) in providers {
            let Some(Value::Array(models)) = config.get("models") else {
                continue;
            };
            for model in models {
                add_model(&mut catalog, model, Some(&provider));
            }
        }
    }
    catalog
}

fn catalog_dirs(pi: &Pi) -> Vec<PathBuf> {
    let Some(package) = &pi.package else {
        return Vec::new();
    };
    let sibling = package
        .parent()
        .map(|parent| parent.join("ai/dist/providers/data"));
    let nested = glob::glob(
        &package
            .join("node_modules/@*/pi-ai/dist/providers/data")
            .to_string_lossy(),
    )
    .map(|paths| paths.flatten().collect::<Vec<_>>())
    .unwrap_or_default();
    sibling
        .into_iter()
        .chain(nested)
        .filter(|dir| dir.is_dir())
        .collect()
}

fn add_model(
    catalog: &mut HashMap<(String, String), CatalogEntry>,
    model: &Value,
    provider: Option<&str>,
) {
    let Some(id) = model.get("id").and_then(Value::as_str) else {
        return;
    };
    let Some(provider) = model.get("provider").and_then(Value::as_str).or(provider) else {
        return;
    };
    let levels = model
        .get("thinkingLevelMap")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(level, mapped)| {
                    let mapped = (!mapped.is_null()).then(|| {
                        mapped
                            .as_str()
                            .map_or_else(|| mapped.to_string(), String::from)
                    });
                    (level.clone(), mapped)
                })
                .collect()
        });
    catalog.insert(
        (provider.to_string(), id.to_string()),
        CatalogEntry {
            name: model
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            reasoning: model
                .get("reasoning")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            levels,
        },
    );
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

#[cfg(test)]
#[path = "../../tests/unit/pi/models_tests.rs"]
mod tests;
